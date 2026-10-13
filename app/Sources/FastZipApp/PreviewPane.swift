import SwiftUI
import AppKit
import PDFKit
import AVKit
import FastZipKit

/// Inline preview for the selected archive entry (FR-010).
///
/// Extracts the single entry to a private temp folder (the same mechanism the
/// double-click open flow uses) and renders images, text, PDFs and media
/// inline; everything else gets a generic representation with Open/Reveal.
struct PreviewPane: View {
    let archive: URL
    let entry: ArchiveEntry?
    let password: String?
    let codepage: String

    @EnvironmentObject private var app: AppModel
    @State private var fileURL: URL?
    @State private var kind: PreviewKind?
    @State private var phase: Phase = .idle
    @State private var loadID = UUID()

    private enum Phase: Equatable { case idle, loading, readyKind(Int), unsupported }

    var body: some View {
        Group {
            switch phase {
            case .idle:
                Label("Select an item to see its details.", systemImage: "sidebar.trailing")
                    .font(.callout).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            case .loading:
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            case .unsupported:
                emptyState(symbol: "doc.badge.ellipsis")
            case .readyKind:
                if let kind { render(kind) } else { emptyState(symbol: "doc") }
            }
        }
        .frame(maxWidth: .infinity, minHeight: 180, maxHeight: .infinity)
        .task(id: [archive.path, entry?.name ?? "", codepage, password ?? ""]) { await load() }
        .onDisappear { loadID = UUID(); releasePreview() }
    }

    @ViewBuilder
    private func render(_ kind: PreviewKind) -> some View {
        switch kind {
        case .image(let url):
            if let ns = NSImage(contentsOf: url) {
                ImageView(nsImage: ns)
            } else {
                emptyState(symbol: "photo")
            }
        case .text(let url):
            ScrollView {
                Text((try? String(contentsOf: url, encoding: .utf8).prefix(262_144)) ?? "")
                    .font(.system(.caption, design: .monospaced))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled)
                    .padding(8)
            }
        case .pdf(let url):
            PDFKitView(url: url)
        case .media(let url):
            VideoPlayer(player: AVPlayer(url: url))
                .aspectRatio(1, contentMode: .fit)
        }
    }

    private func emptyState(symbol: String) -> some View {
        VStack(spacing: 10) {
            Image(systemName: symbol).font(.system(size: 30, weight: .light)).foregroundStyle(.tertiary)
            if let e = entry {
                Text(e.displayName).font(.callout).lineLimit(2).multilineTextAlignment(.center)
                Text(Format.bytes(e.size)).font(.caption).foregroundStyle(.secondary)
            }
            HStack {
                if let fileURL {
                    Button("Open") { NSWorkspace.shared.open(fileURL) }
                    Button("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting([fileURL]) }
                }
            }
        }
        .padding(12)
    }

    private func load() async {
        let currentLoad = UUID()
        loadID = currentLoad
        releasePreview()
        guard let entry, !entry.isDirectory, entry.size < 512 * 1024 * 1024 else {
            phase = entry == nil ? .idle : .unsupported
            return
        }
        let previewKind = PreviewKind(entry: entry)
        guard previewKind != nil || PreviewKind.wantsExtract(entry) else {
            phase = .unsupported
            return
        }
        phase = .loading
        if let url = await app.extractToTemp(archive: archive, entry: entry, password: password, codepage: codepage) {
            guard !Task.isCancelled, loadID == currentLoad else {
                app.releaseTemporaryFile(url)
                return
            }
            fileURL = url
            self.kind = previewKind?.with(url: url)
            phase = previewKind == nil ? .unsupported : .readyKind(1)
        } else if loadID == currentLoad && !Task.isCancelled {
            phase = .unsupported
        }
    }

    private func releasePreview() {
        // Drop PDF/media/image consumers before unlinking their plaintext files.
        kind = nil
        if let fileURL { app.releaseTemporaryFile(fileURL) }
        fileURL = nil
    }
}

private enum PreviewKind {
    case image(URL), text(URL), pdf(URL), media(URL)

    init?(entry: ArchiveEntry) {
        let ext = (entry.name as NSString).pathExtension.lowercased()
        let images: Set<String> = ["jpg", "jpeg", "png", "gif", "tif", "tiff", "bmp", "webp", "heic", "heif", "avif", "svg", "ico", "jp2"]
        let texts: Set<String> = ["txt", "md", "json", "xml", "yaml", "yml", "csv", "tsv", "log", "ini", "cfg", "conf", "swift", "rs", "c", "h", "cpp", "hpp", "m", "mm", "py", "rb", "go", "js", "ts", "css", "html", "sh", "sql", "plist"]
        let media: Set<String> = ["mp4", "mov", "m4v", "avi", "mkv", "mp3", "m4a", "aac", "wav", "aiff", "flac", "ogg"]
        if images.contains(ext) { self = .image(URL(fileURLWithPath: "")) }
        else if texts.contains(ext) { self = .text(URL(fileURLWithPath: "")) }
        else if ext == "pdf" { self = .pdf(URL(fileURLWithPath: "")) }
        else if media.contains(ext) { self = .media(URL(fileURLWithPath: "")) }
        else { return nil }
    }

    /// Whether the unsupported branch should still extract for the Open button.
    static func wantsExtract(_ entry: ArchiveEntry) -> Bool {
        entry.size < 64 * 1024 * 1024
    }

    func with(url: URL) -> PreviewKind {
        switch self {
        case .image: return .image(url)
        case .text: return .text(url)
        case .pdf: return .pdf(url)
        case .media: return .media(url)
        }
    }
}

private struct ImageView: NSViewRepresentable {
    let nsImage: NSImage

    func makeNSView(context: Context) -> NSImageView {
        let v = NSImageView()
        v.imageScaling = .scaleProportionallyUpOrDown
        v.image = nsImage
        return v
    }

    func updateNSView(_ v: NSImageView, context: Context) {
        v.image = nsImage
    }
}

private struct PDFKitView: NSViewRepresentable {
    let url: URL

    func makeNSView(context: Context) -> PDFView {
        let v = PDFView()
        v.autoScales = true
        v.document = PDFDocument(url: url)
        return v
    }

    func updateNSView(_ v: PDFView, context: Context) {
        if v.document?.documentURL != url {
            v.document = PDFDocument(url: url)
        }
    }
}
