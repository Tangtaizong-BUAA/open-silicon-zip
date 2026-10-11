import SwiftUI
import FastZipKit

extension ArchiveEntry {
    // Non-optional sort keys for Table's KeyPathComparator columns.
    var modifiedSort: Date { modified ?? .distantPast }
    var crcSort: UInt32 { crc ?? 0 }
    var encryptionSort: String { encrypted ? (encryptionMethod ?? "Yes") : "" }
    var systemImage: String {
        if isDirectory { return "folder" }
        if isSymlink { return "link" }
        switch fileExtension {
        case "jpg", "jpeg", "png", "gif", "tiff", "webp", "heic", "avif": return "photo"
        case "mp4", "mov", "avi", "mkv": return "film"
        case "mp3", "wav", "flac", "m4a", "aac": return "music.note"
        case "pdf": return "doc.richtext"
        case "txt", "md", "rtf": return "doc.text"
        case "zip", "7z", "rar", "tar", "gz": return "archivebox"
        default: return "doc"
        }
    }
}

enum BrowserViewMode: String, CaseIterable, Identifiable {
    case icons, list, details
    var id: String { rawValue }
    var title: String {
        switch self {
        case .icons: return L10n.t("Icons")
        case .list: return L10n.t("List")
        case .details: return L10n.t("Details")
        }
    }
    var systemImage: String {
        switch self {
        case .icons: return "square.grid.2x2"
        case .list: return "list.bullet"
        case .details: return "tablecells"
        }
    }
}

@MainActor
final class BrowserModel: ObservableObject {
    enum LoadState: Equatable {
        case loading
        case ready
        case failed(FZError)
    }

    let url: URL
    @Published private(set) var state: LoadState = .loading
    @Published private(set) var index = ArchiveIndex(entries: [])
    @Published private(set) var visible: [ArchiveEntry] = []
    @Published var codepage: CodePage = CodePageCatalog.auto
    @Published var currentFolder = "" { didSet { if oldValue != currentFolder { selection = []; refresh() } } }
    @Published var selection = Set<String>()
    @Published var search = ""
    @Published var viewMode: BrowserViewMode = .details
    @Published var showInspector = false
    @Published var showPreview = true
    @Published var sortOrder: [KeyPathComparator<ArchiveEntry>] = [KeyPathComparator(\ArchiveEntry.displayName)] {
        didSet { refresh() }
    }
    private(set) var password: String?
    private var searchResults: [ArchiveEntry]?

    init(url: URL) { self.url = url }

    var rootNodes: [FolderNode] {
        [FolderNode(path: "", name: url.lastPathComponent, children: index.root.isEmpty ? nil : index.root)]
    }

    var pathComponents: [(name: String, path: String)] {
        var out = [(name: url.lastPathComponent, path: "")]
        var acc: [String] = []
        for c in currentFolder.split(separator: "/") {
            acc.append(String(c))
            out.append((name: String(c), path: acc.joined(separator: "/")))
        }
        return out
    }

    var selectedEntries: [ArchiveEntry] { visible.filter { selection.contains($0.id) } }

    func load(using app: AppModel) async {
        state = .loading
        do {
            let session = try await app.openSession(url, codepage: codepage.id, password: password)
            password = session.password
            let built = await Task.detached { ArchiveIndex(entries: session.entries) }.value
            index = built
            state = .ready
            refresh()
        } catch let e as FZError {
            state = .failed(e)
        } catch {
            state = .failed(FZError(code: .io, message: error.localizedDescription))
        }
    }

    func setCodepage(_ cp: CodePage, using app: AppModel) {
        guard cp != codepage else { return }
        codepage = cp
        Task { await load(using: app) }
    }

    /// Debounced, off-main search (FR-008).
    func runSearch() async {
        let q = search.trimmingCharacters(in: .whitespaces)
        if q.isEmpty {
            searchResults = nil
            refresh()
            return
        }
        try? await Task.sleep(nanoseconds: 150_000_000)
        if Task.isCancelled { return }
        let idx = index
        let folder = currentFolder
        let results = await Task.detached { idx.search(q, in: folder) }.value
        if Task.isCancelled { return }
        searchResults = results
        refresh()
    }

    func refresh() {
        let base = searchResults ?? index.items(in: currentFolder)
        let sorted = base.sorted(using: sortOrder)
        visible = sorted.filter { $0.isDirectory } + sorted.filter { !$0.isDirectory }
    }

    func open(_ entry: ArchiveEntry, using app: AppModel) {
        if entry.isDirectory {
            search = ""
            searchResults = nil
            currentFolder = entry.name
        } else {
            app.openInPlace(archive: url, entry: entry, password: password, codepage: codepage.id)
        }
    }

    func goUp() {
        guard !currentFolder.isEmpty else { return }
        currentFolder = currentFolder.split(separator: "/").dropLast().joined(separator: "/")
    }

    /// ZIP archives can be modified in place through `fz edit` (FR-301..303).
    var isEditableZip: Bool {
        ["zip", "zipx"].contains(url.pathExtension.lowercased())
    }

    @Published var editError: String?
    @Published var editBusy = false

    func beginDelete(_ ids: Set<String>, using app: AppModel) {
        let names = index.items(in: currentFolder).filter { ids.contains($0.id) }.map(\.name)
        let count = names.count
        Task { @MainActor in
            guard await Dialogs.confirm(
                title: L10n.f("Delete %ld item(s) from the archive?", count),
                message: String(localized: "The archive is rewritten safely; unrelated entries stay byte-identical.")) else { return }
            await runEdit(app: app, delete: names, rename: [], add: [])
        }
    }

    func beginRename(_ ids: Set<String>, using app: AppModel) {
        guard let name = index.items(in: currentFolder).first(where: { ids.contains($0.id) })?.name else { return }
        Task { @MainActor in
            guard let newName = await Dialogs.askText(
                title: String(localized: "Rename File"),
                message: name,
                initial: name) else { return }
            await runEdit(app: app, delete: [], rename: [(name, newName)], add: [])
        }
    }

    func addFiles(using app: AppModel) {
        Task { @MainActor in
            let urls = await FilePanels.chooseFiles(allowFolders: false, message: L10n.t("Add files to archive"))
            guard !urls.isEmpty else { return }
            await runEdit(app: app, delete: [], rename: [], add: urls.map(\.path))
        }
    }

    private func runEdit(app: AppModel, delete: [String], rename: [(String, String)], add: [String]) async {
        editBusy = true
        editError = nil
        defer { editBusy = false }
        do {
            try await app.editArchive(url, delete: delete, rename: rename, add: add)
            await load(using: app)
        } catch let e as FZError {
            editError = e.message.isEmpty ? e.code.title : e.message
        } catch {
            editError = error.localizedDescription
        }
    }

    func extractSelected(using app: AppModel, toAskedFolder: Bool = true) {
        let paths = ArchiveIndex.extractionPaths(for: selectedEntries)
        guard !paths.isEmpty else { return }
        Task {
            guard let dest = await FilePanels.chooseFolder(prompt: L10n.t("Extract"), message: L10n.t("Extract selected items to")) else { return }
            app.extract(url, mode: .here, only: paths, into: dest, codepage: codepage.id, password: password)
        }
    }
}
