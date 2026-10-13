import SwiftUI
import AppKit
import UserNotifications
import FastZipKit

/// Seed for the New Archive window/sheet.
struct NewArchiveSeed: Codable, Hashable {
    var paths: [String] = []
    var presetName: String?
    var nonce = UUID()
}

struct CreateRequest {
    var inputs: [URL]
    var settings: CreateSettings
    var destination: URL
    var baseName: String
    var password: String?
}

enum PostAction {
    case openDestination(URL)
    case reveal(URL)
    case openFile(URL)
    case trash([URL])
    case test(URL, String?, [PostAction])
}

/// Used when the `fz` binary cannot be found, so the UI still launches and explains why.
struct MissingEngine: EngineClient {
    func start(_ command: EngineCommand) -> EngineRun {
        let stream = AsyncThrowingStream<EngineEvent, Error> { c in
            c.finish(throwing: FZError(code: .engineMissing,
                                       message: L10n.t("Set FZ_BINARY or the engine path in Preferences > Advanced.")))
        }
        return EngineRun(events: stream, pause: {}, resume: {}, cancel: {})
    }
}

/// App-wide state. All heavy work goes through `JobQueue` (an actor) or detached tasks;
/// this class only holds published snapshots and orchestrates flows on the main actor.
@MainActor
final class AppModel: ObservableObject {
    static let shared = AppModel()

    @Published var prefs: Preferences { didSet { prefsDidChange(old: oldValue) } }
    @Published private(set) var jobs: [Job] = []
    @Published private(set) var summary = QueueSummary()
    @Published private(set) var throughput: Double = 0
    @Published private(set) var recents: [URL] = []
    @Published private(set) var presets: [Preset] = []
    @Published private(set) var engineAvailable = true

    let engine: EngineClient
    let queue: JobQueue
    let passwordStore = PasswordStore()
    let presetStore = PresetStore()
    private let prefsStore = PreferencesStore()
    // Sandbox-container access can wait on macOS. Keep Finder config writes
    // ordered without blocking startup, progress windows or the updater UI.
    private let finderConfigQueue = DispatchQueue(label: "org.opensiliconzip.finder-config", qos: .utility)

    private(set) var lastExternalRequest = Date.distantPast
    var openDashboardWindow: (() -> Void)?
    let quickWindows = QuickActionWindows()
    private var quickProgress: [QuickProgress] = []

    private func beginQuickProgress(_ title: String) -> QuickProgress {
        let progress = quickWindows.progress(title: title, app: self)
        quickProgress.append(progress)
        let close = progress.onFinished
        progress.onFinished = { [weak self, weak progress] in
            close?()
            self?.quickProgress.removeAll { $0 === progress }
        }
        return progress
    }

    private var pendingLinks: [DeepLink] = []
    var approveExternalRequest: (ExternalRequestReview) -> Bool = ExternalRequestDialog.confirm
    private var dashboardRequested = false
    func requestDashboard() {
        if let openDashboardWindow { openDashboardWindow() }
        else { dashboardRequested = true }
    }
    func windowsReady() {
        if dashboardRequested, let openDashboardWindow {
            dashboardRequested = false
            openDashboardWindow()
        }
        let links = pendingLinks
        pendingLinks.removeAll()
        links.forEach { handle($0) }
    }

    // Window openers, registered by the root views.
    var openArchiveWindow: ((URL) -> Void)?
    var openNewArchiveWindow: ((NewArchiveSeed) -> Void)?
    var openJobsWindow: (() -> Void)?

    private var postActions: [JobID: [PostAction]] = [:]
    private var wasBusy = false
    private var temporaryFiles = try? TemporaryExtractionStore()
    private var openedTemporaryFiles: [String: [URL]] = [:]
    private var openFileGeneration: [String: UUID] = [:]
    private static let recentsKey = "FastZip.recents"

    init(engine suppliedEngine: EngineClient? = nil, preferences: Preferences? = nil, observe: Bool = true) {
        let loaded = preferences ?? PreferencesStore().load()
        prefs = loaded
        let enginePath = loaded.advanced.enginePath.map { URL(fileURLWithPath: $0) } ?? ProcessEngineClient.locate()
        if let suppliedEngine {
            engine = suppliedEngine
        } else if let enginePath {
            engine = ProcessEngineClient(executableURL: enginePath)
            engineAvailable = true
        } else {
            engine = MissingEngine()
            engineAvailable = false
        }
        queue = JobQueue(engine: engine, maxConcurrent: loaded.general.maxConcurrentJobs)
        presets = presetStore.all()
        recents = (UserDefaults.standard.stringArray(forKey: Self.recentsKey) ?? []).map { URL(fileURLWithPath: $0) }
        if observe {
            startObserving()
            syncFinderConfig()
        }
    }

    // MARK: Observation

    private func startObserving() {
        let queue = self.queue
        Task { [weak self] in
            for await snapshot in await queue.updates() {
                self?.apply(snapshot: snapshot)
            }
        }
        Task { [weak self] in
            for await link in JobInbox.stream() {
                self?.handle(link)
            }
        }
    }

    private func apply(snapshot: [Job]) {
        jobs = snapshot
        quickProgress.forEach { $0.update(snapshot) }
        quickProgress.removeAll { !$0.preparing && ($0.error != nil || (!$0.ids.isEmpty && $0.jobs.count == $0.ids.count && $0.jobs.allSatisfy { $0.state.isTerminal })) }
        var s = QueueSummary()
        var rate = 0.0
        var processed: UInt64 = 0
        var total: UInt64 = 0
        for j in snapshot {
            s.total += 1
            switch j.state {
            case .queued: s.queued += 1
            case .running:
                s.running += 1
                rate += j.progress.throughput
                processed += j.progress.bytesProcessed
                total += j.progress.bytesTotal
            case .paused: s.paused += 1
            case .success: s.success += 1; runPostActions(for: j.id)
            case .failed: s.failed += 1
            case .skipped: s.skipped += 1
            case .cancelled: s.cancelled += 1
            }
        }
        summary = s
        throughput = rate

        let dock = NSApp?.dockTile
        if s.running > 0, total > 0 {
            dock?.badgeLabel = "\(Int(Double(processed) / Double(total) * 100))%"
        } else {
            dock?.badgeLabel = s.failed > 0 && s.isIdle ? "!" : nil
        }

        let busy = !s.isIdle
        if wasBusy && !busy && prefs.general.notifyOnCompletion && s.total > 0 {
            Notifier.post(title: s.failed == 0 ? String(localized: "FastZip finished") : String(localized: "FastZip finished with errors"),
                          body: L10n.f("%ld succeeded", s.success) + (s.failed > 0 ? L10n.f(", %ld failed", s.failed) : ""))
        }
        wasBusy = busy
    }

    private func registerPostActions(_ actions: [PostAction], for id: JobID) {
        postActions[id] = actions
        // A tiny job can finish while enqueue() yields to the queue actor.
        if jobs.first(where: { $0.id == id })?.state == .success { runPostActions(for: id) }
    }

    private func runPostActions(for id: JobID) {
        guard let actions = postActions.removeValue(forKey: id) else { return }
        let useTrash = prefs.advanced.useTrash
        for a in actions {
            switch a {
            case .openDestination(let u): NSWorkspace.shared.open(u)
            case .reveal(let u): NSWorkspace.shared.activateFileViewerSelecting([u])
            case .openFile(let u): NSWorkspace.shared.open(u)
            case .trash(let urls):
                Task.detached {
                    for u in urls {
                        if useTrash { try? FileManager.default.trashItem(at: u, resultingItemURL: nil) }
                        else { try? FileManager.default.removeItem(at: u) }
                    }
                }
            case .test(let u, let pw, let following):
                Task {
                    let check = await self.queue.enqueue(.test(archive: u.path, password: pw))
                    self.registerPostActions(following, for: check)
                }
            }
        }
    }

    // MARK: Preferences

    private func prefsDidChange(old: Preferences) {
        let p = prefs
        let store = prefsStore
        try? store.save(p)
        if old.language.code != p.language.code { L10n.configure(language: p.language.code) }
        if old.general.maxConcurrentJobs != p.general.maxConcurrentJobs {
            let n = p.general.maxConcurrentJobs
            let q = queue
            Task { await q.setMaxConcurrent(n) }
        }
        syncFinderConfig()
    }

    func resetPreferences() { prefs = Preferences() }

    func exportPreferences(to url: URL) throws { try prefsStore.export(to: url, prefs: prefs) }

    func importPreferences(from url: URL) throws { prefs = try prefsStore.importFile(url) }

    /// Publishes accessible folders, presets and menu settings to the Finder extension (app group).
    func syncFinderConfig() {
        let cfg = FinderConfig(from: prefs, presets: presets)
        finderConfigQueue.async { try? cfg.write() }
    }

    // MARK: Presets

    func reloadPresets() {
        presets = presetStore.all()
        syncFinderConfig()
    }

    func savePreset(_ p: Preset) throws {
        try presetStore.save(p)
        reloadPresets()
    }

    func deletePreset(_ id: UUID) throws {
        try presetStore.delete(id)
        reloadPresets()
    }

    // MARK: Recents (FR-018)

    func addRecent(_ url: URL) {
        var list = recents.filter { $0 != url }
        list.insert(url, at: 0)
        list = Array(list.prefix(max(0, prefs.advanced.recentFilesLimit)))
        recents = list
        UserDefaults.standard.set(list.map(\.path), forKey: Self.recentsKey)
        NSDocumentController.shared.noteNewRecentDocumentURL(url)
    }

    func clearRecents() {
        recents = []
        UserDefaults.standard.removeObject(forKey: Self.recentsKey)
        NSDocumentController.shared.clearRecentDocuments(nil)
    }

    // MARK: Routing (URLs, Finder, Services, Dock)

    func handle(url: URL) {
        if url.isFileURL {
            route(files: [url], action: prefs.fileAssociation.doubleClick)
        } else if let link = DeepLink.parse(url) {
            handle(link)
        }
    }

    /// Files opened via Finder double-click (FR-505) or dropped on the Dock icon (FR-507).
    func route(files: [URL], action: DoubleClickAction) {
        let cfg = FinderConfig(from: prefs, presets: presets)
        let archives = files.filter { cfg.isArchive(path: $0.path) }
        if archives.count != files.count && !files.isEmpty && archives.isEmpty {
            openNewArchiveWindow?(NewArchiveSeed(paths: files.map(\.path)))
            return
        }
        for a in archives {
            switch action {
            case .openInApp, .contextMenu: openArchive(a)
            case .smartExtract: extract(a, mode: .smart)
            case .extractToNamedFolder: extract(a, mode: .named)
            case .askDestination: extract(a, mode: .ask)
            }
        }
    }

    func handle(_ link: DeepLink) {
        lastExternalRequest = Date()
        do { try ExternalRequestPolicy.validate(link.paths, config: FinderConfig(from: prefs, presets: presets)) }
        catch {
            Task { await Dialogs.showError(error as? FZError ?? FZError(code: .unsafePath), context: L10n.t("External request")) }
            return
        }
        switch link {
        case .new, .compress, .extract:
            for window in NSApp.windows {
                let id = window.identifier?.rawValue ?? ""
                if id.hasPrefix("dashboard") || id == "jobs" { window.orderOut(nil) }
            }
        default: break
        }
        if case .open = link, openArchiveWindow == nil {
            if pendingLinks.count < 32 { pendingLinks.append(link) }
            return
        }
        // Compression is confirmed after resolving presets and the destination.
        // All other external channels use the same gate, including inbox files.
        let review: ExternalRequestReview?
        switch link {
        case .new, .compress: review = nil
        case .extract(let paths, let mode, let deletes):
            let destinations = mode == .ask || mode == .other ? [L10n.t("Choose a destination folder next.")]
                : paths.map { URL(fileURLWithPath: $0).deletingLastPathComponent().path }
            review = ExternalRequestReview(action: L10n.t("Extract"), paths: paths, destinations: destinations, deletesOriginals: deletes)
        case .open(let paths):
            review = ExternalRequestReview(action: L10n.t("Open"), paths: paths, destinations: [], deletesOriginals: false)
        case .test(let paths):
            review = ExternalRequestReview(action: L10n.t("Test"), paths: paths, destinations: [], deletesOriginals: false)
        case .checksum(let paths):
            review = ExternalRequestReview(action: L10n.t("Checksums"), paths: paths, destinations: [], deletesOriginals: false)
        }
        if let review, !approveExternalRequest(review) { return }
        switch link {
        case .open(let paths):
            paths.forEach { openArchive(URL(fileURLWithPath: $0)) }
        case .new(let paths, let preset):
            if let preset {
                quickCompress(paths: paths, format: .zip, deleteSources: false, each: false, preset: preset, rule: nil)
            } else { quickWindows.chooseCompression(paths: paths, app: self) }
        case .extract(let paths, let mode, let del):
            for p in paths { extract(URL(fileURLWithPath: p), mode: mode == .smart ? .named : mode, deleteArchive: del, compact: true) }
        case .compress(let paths, let format, let del, let each, let preset, let rule):
            quickCompress(paths: paths, format: format, deleteSources: del, each: each, preset: preset, rule: rule)
        case .test(let paths):
            for p in paths { Task { await self.queue.enqueue(.test(archive: p, password: nil)) } }
            openJobsWindow?()
        case .checksum(let paths):
            Task { await self.queue.enqueue(.hash(files: paths), title: String(localized: "Checksums")) }
            openJobsWindow?()
        }
    }

    func openArchive(_ url: URL) {
        addRecent(url)
        openArchiveWindow?(url)
    }

    // MARK: Listing with password handling (FR-601/604/607)

    struct Session {
        var entries: [ArchiveEntry]
        var password: String?
    }

    func storedPasswords() async -> [PasswordItem] {
        (try? await passwordStore.all()) ?? []
    }

    func openSession(_ archive: URL, codepage: String? = nil, password: String? = nil) async throws -> Session {
        var pw = password
        var candidates: [String] = []
        if prefs.general.autoPassword != .never,
           !(prefs.general.autoPassword == .allExceptRar && archive.pathExtension.lowercased() == "rar") {
            candidates = (try? await passwordStore.autoPasswordCandidates()) ?? []
        }
        while true {
            do {
                let entries = try await engine.listEntries(archive: archive.path, password: pw,
                                                           codepage: codepage ?? prefs.advanced.defaultCodePage)
                return Session(entries: entries, password: pw)
            } catch let e as FZError where e.code == .passwordRequired || e.code == .wrongPassword {
                if !candidates.isEmpty {
                    pw = candidates.removeFirst()
                    continue
                }
                let answer = await Dialogs.askPassword(archive: archive.lastPathComponent,
                                                       retry: pw != nil || e.code == .wrongPassword,
                                                       stored: await storedPasswords())
                guard let answer else { throw FZError(code: .cancelled) }
                pw = answer.password
                if answer.save {
                    try? await passwordStore.upsert(PasswordItem(name: archive.lastPathComponent, secret: answer.password))
                }
            }
        }
    }

    // MARK: Extraction (FR-201..212)

    func extract(_ archive: URL, mode: ExtractMode, only entries: [String] = [], deleteArchive: Bool? = nil,
                 into override: URL? = nil, codepage: String? = nil, password: String? = nil, compact: Bool = false) {
        Task { await self.performExtract(archive, mode: mode, entries: entries, deleteArchive: deleteArchive,
                                    override: override, codepage: codepage, password: password, compact: compact) }
    }

    private func performExtract(_ archive: URL, mode: ExtractMode, entries selected: [String], deleteArchive: Bool?,
                                override: URL?, codepage: String?, password: String?, compact: Bool) async {
        let name = archive.lastPathComponent
        let progress = compact ? beginQuickProgress(name) : nil
        var progressError: String?
        defer { progress?.finish(jobs, error: progressError) }
        do {
            let session = try await openSession(archive, codepage: codepage, password: password)

            var baseDir: URL
            if let override {
                baseDir = override
            } else if mode == .ask || mode == .other {
                guard let chosen = await FilePanels.chooseFolder(prompt: String(localized: "Extract"), message: L10n.f("Extract \"%@\" to", name)) else { return }
                baseDir = chosen
            } else {
                baseDir = archive.deletingLastPathComponent()
            }
            try SmartExtract.validateDestination(baseDir.path)

            var destination = baseDir
            var smart = false
            var candidates: [String]
            var finalFolder: URL?
            switch mode {
            case .smart where selected.isEmpty:
                smart = true
                let plan = SmartExtract.plan(entries: session.entries, archiveName: name,
                                             wrapSingleFiles: prefs.extraction.wrapSingleFiles)
                if plan.kind == .nothing { return }
                candidates = plan.collisionCandidates
                finalFolder = plan.wrapperFolder.map { baseDir.appendingPathComponent($0) } ?? baseDir
            case _ where selected.isEmpty && (mode == .named || (compact && mode != .here)):
                destination = try ExtractionFolder.reserve(in: baseDir, archiveName: name)
                candidates = []
                finalFolder = destination
            default:
                candidates = Self.topLevelNames(session.entries)
                finalFolder = baseDir
            }

            // Collision pre-check: never merge silently (05 section 1, step 5).
            let dir = baseDir
            let existing = await Task.detached {
                (try? FileManager.default.contentsOfDirectory(atPath: dir.path)) ?? []
            }.value
            let colliding = SmartExtract.collisions(
                for: SmartExtractPlan(kind: .nothing, wrapperFolder: nil, stripComponents: 0,
                                      collisionCandidates: candidates, unsafeEntries: []),
                existingNames: existing)

            var policy = prefs.extraction.defaultConflict
            if !colliding.isEmpty && policy == .ask {
                guard let resolved = await resolveCollisions(colliding, baseDir: baseDir, entries: session.entries) else { return }
                policy = resolved
            }

            let cmd = ExtractCommand(archive: archive.path, destination: destination.path, smart: smart,
                                     password: session.password, codepage: codepage, entries: selected,
                                     overwrite: policy == .ask ? nil : policy)
            let id = await queue.enqueue(.extract(cmd), title: name)

            var actions: [PostAction] = []
            progress?.ids.insert(id)
            if !compact && prefs.general.openDestinationAfterExtract, let f = finalFolder { actions.append(.openDestination(f)) }
            if deleteArchive ?? prefs.extraction.deleteArchiveAfter { actions.append(.trash([archive])) }
            registerPostActions(actions, for: id)
            if !compact { openJobsWindow?() }
        } catch let e as FZError {
            if e.code != .cancelled {
                if compact { progressError = e.localizedDescription }
                else { await Dialogs.showError(e, context: name) }
            }
        } catch {
            if compact { progressError = error.localizedDescription }
            else { await Dialogs.showError(FZError(code: .io, message: error.localizedDescription), context: name) }
        }
    }

    private func resolveCollisions(_ names: [String], baseDir: URL, entries: [ArchiveEntry]) async -> ConflictPolicy? {
        var decisions: [ConflictPolicy] = []
        for (i, n) in names.enumerated() {
            let existingURL = baseDir.appendingPathComponent(n)
            let attrs = try? FileManager.default.attributesOfItem(atPath: existingURL.path)
            let incoming = entries.first { $0.components.first == n && $0.components.count == 1 }
            let info = ConflictInfo(name: n,
                                    existingSize: (attrs?[.size] as? NSNumber)?.uint64Value,
                                    existingDate: attrs?[.modificationDate] as? Date,
                                    incomingSize: incoming?.size, incomingDate: incoming?.modified,
                                    remaining: names.count - i - 1)
            let answer = await Dialogs.resolveConflict(info)
            if answer.cancelled { return nil }
            if answer.applyToAll { return answer.policy }
            decisions.append(answer.policy)
        }
        // One engine-wide policy: if answers differ, fall back to the non-destructive "keep both".
        return Set(decisions).count == 1 ? decisions[0] : .rename
    }

    nonisolated static func topLevelNames(_ entries: [ArchiveEntry]) -> [String] {
        var seen = Set<String>()
        var out: [String] = []
        for e in entries {
            let c = SmartExtract.components(of: e.name)
            guard let first = c.first, seen.insert(first).inserted else { continue }
            out.append(first)
        }
        return out
    }

    /// FR-009: extract to a temp folder and open with the associated app; changes are discarded.
    /// URL of the engine binary for out-of-queue runners (recovery wizard).
    var engineExecutableURL: URL? {
        (engine as? ProcessEngineClient)?.executableURL
    }

    /// Extracts one entry into a private temp dir and returns its URL (preview, FR-010).
    /// Fire-and-forget: failures return nil; nothing is added to the job queue.
    func extractToTemp(archive: URL, entry: ArchiveEntry, password: String?, codepage: String?) async -> URL? {
        guard !Task.isCancelled, !entry.isSymlink, !entry.name.hasPrefix("/"), !entry.name.hasPrefix("\\"),
              !entry.components.contains(".."), let temporaryFiles,
              let tmp = try? temporaryFiles.createDirectory() else { return nil }
        let cmd = ExtractCommand(archive: archive.path, destination: tmp.path, password: password,
                                 codepage: codepage, entries: [entry.name], overwrite: .overwrite)
        let run = engine.start(.extract(cmd))
        do {
            try await withTaskCancellationHandler {
                for try await event in run.events {
                    try Task.checkCancellation()
                    if case .done(false) = event { throw FZError(code: .brokenArchive) }
                    if case .error(let code, let message) = event { throw FZError(code: code, message: message) }
                }
                try Task.checkCancellation()
            } onCancel: { run.cancel() }
            await run.waitForTermination()
        } catch {
            run.cancel()
            await run.waitForTermination()
            temporaryFiles.remove(tmp)
            return nil
        }
        let out = tmp.appendingPathComponent(entry.name)
        var isDir: ObjCBool = false
        guard !Task.isCancelled, out.resolvingSymlinksInPath().path.hasPrefix(tmp.path + "/"),
              FileManager.default.fileExists(atPath: out.path, isDirectory: &isDir), !isDir.boolValue else {
            temporaryFiles.remove(tmp)
            return nil
        }
        return out
    }

    func releaseTemporaryFile(_ url: URL) { temporaryFiles?.removeFile(url) }

    func releaseOpenFiles(for archive: URL) {
        openFileGeneration[archive.path] = UUID()
        for file in openedTemporaryFiles.removeValue(forKey: archive.path) ?? [] { releaseTemporaryFile(file) }
    }

    func cleanUpTemporaryFiles() {
        openedTemporaryFiles.removeAll()
        temporaryFiles?.cleanUp()
    }

    /// Applies ZIP modifications through `fz edit` (FR-301..303). Throws on engine failure.
    func editArchive(_ url: URL, delete: [String] = [], rename: [(String, String)] = [], add: [String] = []) async throws {
        precondition(!delete.isEmpty || !rename.isEmpty || !add.isEmpty)
        let command = EngineCommand.edit(archive: url.path, delete: delete,
            rename: rename.map { "\($0.0)=\($0.1)" }, add: add)
        for try await event in engine.start(command).events {
            if case .done(false) = event { throw FZError(code: .brokenArchive, message: "Archive edit failed") }
        }
    }

    func openInPlace(archive: URL, entry: ArchiveEntry, password: String?, codepage: String?) {
        let generation = openFileGeneration[archive.path] ?? UUID()
        openFileGeneration[archive.path] = generation
        Task {
            guard let file = await self.extractToTemp(archive: archive, entry: entry, password: password, codepage: codepage) else { return }
            guard self.openFileGeneration[archive.path] == generation else { self.releaseTemporaryFile(file); return }
            self.openedTemporaryFiles[archive.path, default: []].append(file)
            NSWorkspace.shared.open(file)
        }
    }

    // MARK: Creation (FR-101..124)

    func create(_ r: CreateRequest, compact: Bool = false) async {
        let progress = compact ? beginQuickProgress(r.baseName) : nil
        defer { progress?.finish(jobs) }
        let rule = r.settings.filenameRule.isEmpty ? "${NAME}" : r.settings.filenameRule
        var groups: [(inputs: [URL], base: String)]
        if r.settings.options.oneArchivePerItem {
            groups = r.inputs.map { (inputs: [$0], base: $0.lastPathComponent) }
        } else {
            groups = [(inputs: r.inputs, base: r.baseName)]
        }
        var policy = compact ? ConflictPolicy.rename : r.settings.conflict
        for g in groups {
            let expanded = FilenameRule.expand(rule, name: g.base)
            let fileName = FilenameRule.ensureExtension(expanded, r.settings.format.fileExtension)
            var out = r.destination.appendingPathComponent(fileName)

            if FileManager.default.fileExists(atPath: out.path) {
                if policy == .ask {
                    let attrs = try? FileManager.default.attributesOfItem(atPath: out.path)
                    let a = await Dialogs.resolveConflict(ConflictInfo(
                        name: fileName,
                        existingSize: (attrs?[.size] as? NSNumber)?.uint64Value,
                        existingDate: attrs?[.modificationDate] as? Date))
                    if a.cancelled { return }
                    if a.applyToAll { policy = a.policy }
                    if a.policy == .skip { continue }
                    if a.policy == .rename { out = Self.uniqueURL(out) }
                } else if policy == .skip {
                    continue
                } else if policy == .rename {
                    out = Self.uniqueURL(out)
                }
            }

            if r.settings.options.deleteSources && g.inputs.contains(where: {
                let input = $0.resolvingSymlinksInPath().standardizedFileURL.path
                let output = out.resolvingSymlinksInPath().standardizedFileURL.path
                return output == input || output.hasPrefix(input + "/")
            }) {
                await Dialogs.showError(FZError(code: .unsafePath,
                    message: L10n.t("The output cannot be inside a source scheduled for deletion.")), context: fileName)
                return
            }

            var cmd = CreateCommand(output: out.path, inputs: g.inputs.map(\.path), format: r.settings.format,
                                    level: r.settings.level,
                                    password: r.settings.format.supportsPassword ? r.password : nil,
                                    splitBytes: r.settings.splitBytes, threads: r.settings.threads)
            cmd.excludeGlobs = prefs.compression.excludeGlobs + r.settings.excludeGlobs
            let id = await queue.enqueue(.create(cmd), title: fileName)
            var actions: [PostAction] = []

            progress?.ids.insert(id)
            if !compact && r.settings.options.openAfter { actions.append(.reveal(out)) }
            if r.settings.options.deleteSources { actions.append(.trash(g.inputs)) }
            if r.settings.options.testAfter || r.settings.options.deleteSources {
                registerPostActions([.test(out, cmd.password, actions)], for: id)
            } else { registerPostActions(actions, for: id) }
        }
        if !compact { openJobsWindow?() }
    }

    private static func uniqueURL(_ url: URL) -> URL {
        let dir = url.deletingLastPathComponent()
        let unique = FilenameRule.uniqueName(url.lastPathComponent) {
            FileManager.default.fileExists(atPath: dir.appendingPathComponent($0).path)
        }
        return dir.appendingPathComponent(unique)
    }

    /// Finder / Services one-click compression (FR-502, FR-506).
    func quickCompress(paths: [String], format: ArchiveFormat, deleteSources: Bool, each: Bool,
                       preset presetName: String?, rule: String?, destination: URL? = nil) {
        let inputs = paths.map { URL(fileURLWithPath: $0) }
        guard let first = inputs.first else { return }
        var settings = CreateSettings()
        settings.format = format
        settings.level = prefs.compression.level
        var presetPasswordID: UUID?
        if let presetName, let p = presets.first(where: { $0.name.caseInsensitiveCompare(presetName) == .orderedSame }) {
            settings = p.settings
            presetPasswordID = p.passwordItemID
        }
        settings = ExternalRequestPolicy.compressionSettings(settings, deleteSources: deleteSources)
        if each { settings.options.oneArchivePerItem = true }
        if let rule { settings.filenameRule = rule }
        let configured = FinderConfig(from: prefs, presets: presets)
        let resolvedSettings = settings
        Task {
            do { try ExternalRequestPolicy.validate(paths, config: configured) }
            catch { await Dialogs.showError(error as? FZError ?? FZError(code: .unsafePath), context: L10n.t("External request")); return }
            let outputDirectory: URL
            if let chosen = destination ?? ExternalRequestPolicy.commonSourceDirectory(paths) {
                outputDirectory = chosen
            } else {
                guard let chosen = await FilePanels.chooseFolder(prompt: L10n.t("Compress"), message: L10n.t("Choose the destination for these source files.")) else { return }
                outputDirectory = chosen
            }
            let base = inputs.count == 1 ? first.lastPathComponent : outputDirectory.lastPathComponent
            var r = CreateRequest(inputs: inputs, settings: resolvedSettings, destination: outputDirectory, baseName: base, password: nil)
            let rule = r.settings.filenameRule.isEmpty ? "${NAME}" : r.settings.filenameRule
            let names = r.settings.options.oneArchivePerItem ? inputs.map(\.lastPathComponent) : [base]
            let outputs = names.map {
                outputDirectory.appendingPathComponent(FilenameRule.ensureExtension(
                    FilenameRule.expand(rule, name: $0), r.settings.format.fileExtension)).path
            }
            guard self.approveExternalRequest(ExternalRequestReview(action: L10n.t("Compress"), paths: paths,
                destinations: outputs, deletesOriginals: r.settings.options.deleteSources)) else { return }
            // Check the paths again after a potentially asynchronous folder picker.
            do { try ExternalRequestPolicy.validate(paths, config: FinderConfig(from: self.prefs, presets: self.presets)) }
            catch { await Dialogs.showError(error as? FZError ?? FZError(code: .unsafePath), context: L10n.t("External request")); return }
            if let id = presetPasswordID { r.password = (try? await self.passwordStore.item(id: id))?.secret }
            await self.create(r, compact: true)
        }
    }

    // MARK: Job actions

    func retryWithPassword(_ job: Job) async {
        guard let answer = await Dialogs.askPassword(archive: job.title,
                                                     retry: job.error?.code == .wrongPassword,
                                                     stored: await storedPasswords()) else { return }
        var cmd = job.command
        switch cmd {
        case .extract(var e): e.password = answer.password; cmd = .extract(e)
        case .test(let a, _): cmd = .test(archive: a, password: answer.password)
        case .list(let a, _, let cp): cmd = .list(archive: a, password: answer.password, codepage: cp)
        default: break
        }
        if answer.save {
            try? await passwordStore.upsert(PasswordItem(name: job.title, secret: answer.password))
        }
        await queue.retry(job.id, replacing: cmd)
    }

    func trash(_ urls: [URL]) {
        let useTrash = prefs.advanced.useTrash
        Task.detached {
            for u in urls {
                if useTrash { try? FileManager.default.trashItem(at: u, resultingItemURL: nil) }
                else { try? FileManager.default.removeItem(at: u) }
            }
        }
    }
}

enum Notifier {
    static func post(title: String, body: String, identifier: String = UUID().uuidString) {
        // UNUserNotificationCenter traps when not running from an .app bundle (e.g. `swift run`).
        guard Bundle.main.bundleURL.pathExtension == "app" else { return }
        let center = UNUserNotificationCenter.current()
        center.requestAuthorization(options: [.alert, .sound]) { granted, _ in
            guard granted else { return }
            let content = UNMutableNotificationContent()
            content.title = title
            content.body = body
            center.add(UNNotificationRequest(identifier: identifier, content: content, trigger: nil))
        }
    }
}
