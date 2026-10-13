import Foundation

// Everything the app and the Finder Sync extension share: the `fastzip://` URL
// grammar, the app-group config file and the job inbox.

// MARK: - fastzip:// deep links (FR-512)

public enum ExtractMode: String, Sendable, CaseIterable {
    case smart, here, named, ask, other
}

/// URL grammar: `fastzip://<action>?path=<abs path>&path=...&<options>`.
public enum DeepLink: Equatable, Sendable {
    case open(paths: [String])
    case new(paths: [String], preset: String?)
    case extract(paths: [String], mode: ExtractMode, deleteArchive: Bool)
    case compress(paths: [String], format: ArchiveFormat, deleteSources: Bool, each: Bool, preset: String?, rule: String?)
    case test(paths: [String])
    case checksum(paths: [String])

    public static let scheme = "fastzip"

    public var url: URL {
        var c = URLComponents()
        c.scheme = Self.scheme
        var items: [URLQueryItem] = []
        func addPaths(_ p: [String]) { items += p.map { URLQueryItem(name: "path", value: $0) } }
        switch self {
        case .open(let p):
            c.host = "open"; addPaths(p)
        case .new(let p, let preset):
            c.host = "new"; addPaths(p)
            if let preset { items.append(URLQueryItem(name: "preset", value: preset)) }
        case .extract(let p, let mode, let del):
            c.host = "extract"; addPaths(p)
            items.append(URLQueryItem(name: "mode", value: mode.rawValue))
            if del { items.append(URLQueryItem(name: "delete", value: "1")) }
        case .compress(let p, let format, let del, let each, let preset, let rule):
            c.host = "compress"; addPaths(p)
            items.append(URLQueryItem(name: "format", value: format.rawValue))
            if del { items.append(URLQueryItem(name: "delete", value: "1")) }
            if each { items.append(URLQueryItem(name: "each", value: "1")) }
            if let preset { items.append(URLQueryItem(name: "preset", value: preset)) }
            if let rule { items.append(URLQueryItem(name: "rule", value: rule)) }
        case .test(let p):
            c.host = "test"; addPaths(p)
        case .checksum(let p):
            c.host = "checksum"; addPaths(p)
        }
        c.queryItems = items
        return c.url ?? URL(string: "fastzip://open")!
    }

    public static func parse(_ url: URL) -> DeepLink? {
        guard url.absoluteString.utf8.count <= 1024 * 1024,
              url.scheme == scheme,
              let c = URLComponents(url: url, resolvingAgainstBaseURL: false),
              let host = c.host, c.user == nil, c.password == nil, c.port == nil,
              c.fragment == nil, c.path.isEmpty else { return nil }
        let items = c.queryItems ?? []
        let paths = items.filter { $0.name == "path" }.compactMap(\.value)
        guard !paths.isEmpty, paths.count <= 1024,
              paths.allSatisfy({ $0.hasPrefix("/") && $0.utf8.count <= 4096 && !$0.contains("\0") && !$0.split(separator: "/").contains("..") }) else { return nil }
        func value(_ n: String) -> String? { items.first { $0.name == n }?.value }
        let flag: (String) -> Bool = { value($0) == "1" }

        switch host {
        case "open": return .open(paths: paths)
        case "new": return .new(paths: paths, preset: value("preset"))
        case "extract":
            return .extract(paths: paths, mode: ExtractMode(rawValue: value("mode") ?? "") ?? .smart,
                            deleteArchive: flag("delete"))
        case "compress":
            return .compress(paths: paths, format: ArchiveFormat(rawValue: value("format") ?? "") ?? .zip,
                             deleteSources: flag("delete"), each: flag("each"),
                             preset: value("preset"), rule: value("rule"))
        case "test": return .test(paths: paths)
        case "checksum": return .checksum(paths: paths)
        default: return nil
        }
    }
}

// MARK: - App group

public enum AppGroup {
    public static var userHomePath: String {
        NSHomeDirectory().components(separatedBy: "/Library/Containers/").first ?? NSHomeDirectory()
    }

    public static let identifier = "group.org.opensiliconzip.mac"

    /// The shared container, or Application Support/OpenSiliconZip when unsigned/unsandboxed (development).
    public static var containerURL: URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSTemporaryDirectory())
        return base.appendingPathComponent("OpenSiliconZip", isDirectory: true)
    }

    /// Mirror inside the FinderSync extension's own sandbox container. Without a
    /// provisioning profile the app-group container does not exist, so the
    /// sandboxed extension cannot read `containerURL`; it CAN always read its
    /// own container, and the unsandboxed app can write there. Reads try this
    /// path when the primary one is unreadable (i.e. when running sandboxed).
    public static var extensionContainerMirrorURL: URL {
        let home = FileManager.default.homeDirectoryForCurrentUser
        if home.path.contains("/Library/Containers/") {
            return home.appendingPathComponent("Library/Application Support/OpenSiliconZip", isDirectory: true)
        }
        return home
            .appendingPathComponent("Library/Containers/org.opensiliconzip.mac.findersync/Data/Library/Application Support/OpenSiliconZip", isDirectory: true)
    }
}

/// Written by the app, read by the Finder extension (FR-502/504/510/511).
public struct FinderConfig: Codable, Sendable, Equatable {
    public var accessibleFolders: [String] = [AppGroup.userHomePath]
    public var cascaded = false
    public var enableOnExternalVolumes = false
    public var showChecksumSubmenu = false
    public var showOnArchiveFiles = false
    public var hideIcon = false
    public var languageCode: String? = "zh-Hans"
    public var presetNames: [String] = []
    public var defaultPresetName: String?
    public var customFilenameRule = "${NAME}"
    public var archiveExtensions: [String] = FinderConfig.defaultArchiveExtensions
    public init() {}

    public static let defaultArchiveExtensions = [
        "zip", "zipx", "7z", "rar", "tar", "gz", "tgz", "bz2", "tbz", "xz", "txz",
        "lzma", "zst", "lz4", "br", "iso", "cab", "jar", "001",
    ]

    public init(from prefs: Preferences, presets: [Preset]) {
        accessibleFolders = prefs.sandbox.accessibleFolders.isEmpty ? [AppGroup.userHomePath] : prefs.sandbox.accessibleFolders
        cascaded = prefs.finderMenu.cascaded
        enableOnExternalVolumes = prefs.finderMenu.enableOnExternalVolumes
        showChecksumSubmenu = prefs.finderMenu.showChecksumSubmenu
        showOnArchiveFiles = prefs.finderMenu.showOnArchiveFiles
        hideIcon = prefs.finderMenu.hideIcon
        languageCode = prefs.language.code
        presetNames = presets.map(\.name)
        defaultPresetName = presets.first { $0.isDefault }?.name
        customFilenameRule = prefs.finderMenu.customFilenameRule
        archiveExtensions = FinderConfig.defaultArchiveExtensions + prefs.advanced.extraArchiveExtensions
    }

    public static var fileURL: URL { AppGroup.containerURL.appendingPathComponent("finder-config.json") }

    public static var mirrorFileURL: URL { AppGroup.extensionContainerMirrorURL.appendingPathComponent("finder-config.json") }

    public static func read() -> FinderConfig {
        if let data = try? Data(contentsOf: fileURL),
           let cfg = try? JSONDecoder().decode(FinderConfig.self, from: data) {
            return cfg
        }
        // Sandboxed extension: the primary path is unreadable, use the container mirror.
        if let data = try? Data(contentsOf: mirrorFileURL),
           let cfg = try? JSONDecoder().decode(FinderConfig.self, from: data) {
            return cfg
        }
        return FinderConfig()
    }

    public func write() throws {
        let e = JSONEncoder()
        e.outputFormatting = [.prettyPrinted, .sortedKeys]
        let data = try e.encode(self)
        try FileManager.default.createDirectory(at: AppGroup.containerURL, withIntermediateDirectories: true)
        try data.write(to: Self.fileURL, options: .atomic)
        // Mirror into the extension's sandbox container (no-op there on failure).
        if FileManager.default.fileExists(atPath: AppGroup.extensionContainerMirrorURL.deletingLastPathComponent().deletingLastPathComponent().path) {
            try? FileManager.default.createDirectory(at: AppGroup.extensionContainerMirrorURL, withIntermediateDirectories: true)
            try? data.write(to: Self.mirrorFileURL, options: .atomic)
        }
    }

    /// True when `path` is inside one of the user-authorised roots (05 section 8).
    public func isAuthorised(path: String, isExternalVolume: Bool = false) -> Bool {
        guard path.hasPrefix("/"), !path.contains("\0"), !path.split(separator: "/").contains("..") else { return false }
        let resolved = ExternalRequestPolicy.canonicalPath(path)
        if (isExternalVolume || resolved.hasPrefix("/Volumes/")) && !enableOnExternalVolumes { return false }
        return accessibleFolders.contains { root in
            guard root.hasPrefix("/"), !root.contains("\0") else { return false }
            let canonicalRoot = ExternalRequestPolicy.canonicalPath(root)
            let prefix = canonicalRoot.hasSuffix("/") ? canonicalRoot : canonicalRoot + "/"
            return resolved == canonicalRoot || resolved.hasPrefix(prefix)
        }
    }

    public func isArchive(path: String) -> Bool {
        archiveExtensions.contains((path as NSString).pathExtension.lowercased())
    }
}

/// Hand-off from the extension to the app when the app is not frontmost: each
/// request is a small file holding one `fastzip://` URL. The app drains the
/// folder at launch and while running.
public enum JobInbox {
    public static var directory: URL { AppGroup.containerURL.appendingPathComponent("Inbox", isDirectory: true) }

    /// The extension's own container is always writable from inside the appex.
    public static var mirrorDirectory: URL {
        AppGroup.extensionContainerMirrorURL.appendingPathComponent("Inbox", isDirectory: true)
    }

    public static var hasPendingRequests: Bool {
        [directory, mirrorDirectory].contains { dir in
            ((try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil)) ?? [])
                .contains { $0.pathExtension == "url" }
        }
    }

    public static func post(_ link: DeepLink) throws {
        let payload = Data(link.url.absoluteString.utf8)
        let name = "\(UUID().uuidString).url"
        var lastError: Error?
        for dir in [directory, mirrorDirectory] {
            do {
                try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
                try payload.write(to: dir.appendingPathComponent(name), options: .atomic)
                return
            } catch { lastError = error }
        }
        throw lastError ?? CocoaError(.fileWriteUnknown)
    }

    /// Reads and deletes all pending requests, oldest first.
    public static func drain() -> [DeepLink] {
        drain(directory: directory) + drain(directory: mirrorDirectory)
    }

    private static func drain(directory: URL) -> [DeepLink] {
        let fm = FileManager.default
        guard let files = try? fm.contentsOfDirectory(at: directory, includingPropertiesForKeys: [.creationDateKey]) else { return [] }
        let sorted = files.filter { $0.pathExtension == "url" }.sorted {
            let a = (try? $0.resourceValues(forKeys: [.creationDateKey]).creationDate) ?? .distantPast
            let b = (try? $1.resourceValues(forKeys: [.creationDateKey]).creationDate) ?? .distantPast
            return a < b
        }
        var out: [DeepLink] = []
        for f in sorted {
            if let s = try? String(contentsOf: f, encoding: .utf8),
               let u = URL(string: s.trimmingCharacters(in: .whitespacesAndNewlines)),
               let link = DeepLink.parse(u) {
                out.append(link)
            }
            try? fm.removeItem(at: f)
        }
        return out
    }

    /// Polls the inbox once per second (simple and sandbox-safe) until the consuming task is cancelled.
    public static func stream(interval: Duration = .seconds(1)) -> AsyncStream<DeepLink> {
        AsyncStream { continuation in
            let task = Task {
                while !Task.isCancelled {
                    for link in drain() { continuation.yield(link) }
                    try? await Task.sleep(for: interval)
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}
