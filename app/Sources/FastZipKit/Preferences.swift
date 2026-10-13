import Foundation

// Preferences are one human-readable, versioned JSON document (FR-902, FR-908).
// Tabs mirror FR-901: General, File Association, Finder Menu, Extraction,
// Compression, View/Edit, Advanced, Sandbox, Language.

public enum DoubleClickAction: String, Codable, Sendable, CaseIterable, Identifiable {
    case openInApp, smartExtract, extractToNamedFolder, contextMenu, askDestination
    public var id: String { rawValue }
    public var displayName: String {
        switch self {
        case .openInApp: return L10n.t("Open in FastZip")
        case .smartExtract: return L10n.t("Smart extract in place")
        case .extractToNamedFolder: return L10n.t("Extract to named folder")
        case .contextMenu: return L10n.t("Show context menu")
        case .askDestination: return L10n.t("Ask for destination")
        }
    }
}

public enum ExtractDestinationMode: String, Codable, Sendable, CaseIterable, Identifiable {
    case currentFolder, namedFolder, desktop, custom, alwaysAsk
    public var id: String { rawValue }
    public var displayName: String {
        switch self {
        case .currentFolder: return L10n.t("Current folder")
        case .namedFolder: return L10n.t("Folder named after the archive")
        case .desktop: return L10n.t("Desktop")
        case .custom: return L10n.t("Custom folder")
        case .alwaysAsk: return L10n.t("Always ask")
        }
    }
}

public enum LinkPolicy: String, Codable, Sendable, CaseIterable, Identifiable {
    case storeAsLink, dereference, follow
    public var id: String { rawValue }
    public var displayName: String {
        switch self {
        case .storeAsLink: return L10n.t("Store as link")
        case .dereference: return L10n.t("Store target contents")
        case .follow: return L10n.t("Follow links")
        }
    }
}

public enum RowHeight: String, Codable, Sendable, CaseIterable, Identifiable {
    case low, normal, high
    public var id: String { rawValue }
    public var points: Double {
        switch self {
        case .low: return 20
        case .normal: return 26
        case .high: return 34
        }
    }
}

public enum AutoPasswordMode: String, Codable, Sendable, CaseIterable, Identifiable {
    case never, allArchives, allExceptRar
    public var id: String { rawValue }
    public var displayName: String {
        switch self {
        case .never: return L10n.t("Never")
        case .allArchives: return L10n.t("All archives")
        case .allExceptRar: return L10n.t("All except RAR")
        }
    }
}

public struct Preferences: Codable, Sendable, Equatable {
    public static let currentVersion = 2

    public struct General: Codable, Sendable, Equatable {
        public var escToClose = true
        public var quitWhenLastWindowCloses = false
        public var showDashboardOnLaunch = true
        public var notifyOnCompletion = true
        public var openDestinationAfterExtract = false
        public var closeProgressOnSuccess = false
        public var maxConcurrentJobs = 2
        public var autoPassword = AutoPasswordMode.never
        public init() {}
    }

    public struct FileAssociation: Codable, Sendable, Equatable {
        public var doubleClick = DoubleClickAction.openInApp
        public var dockDropAction = DoubleClickAction.openInApp
        public var wizardShown = false
        public init() {}
    }

    public struct FinderMenu: Codable, Sendable, Equatable {
        public var cascaded = false
        public var enableOnExternalVolumes = false
        public var hideIcon = false
        public var showChecksumSubmenu = false
        public var showOnArchiveFiles = false
        public var customFilenameRule = "${NAME}"
        public init() {}
    }

    public struct Extraction: Codable, Sendable, Equatable {
        public var destination = ExtractDestinationMode.namedFolder
        public var customFolder: String?
        public var rememberChoice = false
        public var wrapSingleFiles = false
        public var deleteArchiveAfter = false
        public var filterMacMetadata = true
        public var normalization = "NFC"
        public var atomicTarCompound = true
        public var jumpIntoSingleRoot = false
        public var deletePartialOnCancel = true
        public var fasterWrites = true
        public var memoryBudgetMB = 1024
        public var defaultConflict = ConflictPolicy.ask
        public var allowExternalSymlinks = false
        public init() {}
    }

    public struct Compression: Codable, Sendable, Equatable {
        public var format = ArchiveFormat.zip
        public var level = CompressionLevel.normal
        public var highSpeedMode = "on"        // off | on | aggressive
        public var skipExtensions = Preferences.defaultSkipList
        public var excludeGlobs = [".DS_Store", "Thumbs.db", "__MACOSX/", "._*"]
        public var includeMacMetadata = false
        public var wrapSingleFolder = true
        public var linkPolicy = LinkPolicy.storeAsLink
        public var threads = 0                 // 0 = Auto
        public var zipLegacyCodePage = "utf-8"
        public init() {}
    }

    public struct View: Codable, Sendable, Equatable {
        public var rowHeight = RowHeight.normal
        public var exactSizes = false
        public var textColor = "auto"          // auto | white | black
        public var accentLight = "#2F6FEB"
        public var accentDark = "#6EA2FF"
        public var showPathBar = true
        public var visibleColumns = ["name", "size", "csize", "type", "modified"]
        public var autoExpandTree = false
        public init() {}
    }

    public struct Advanced: Codable, Sendable, Equatable {
        public var tempFolder: String?
        public var extraArchiveExtensions: [String] = []
        public var useTrash = true
        public var recentFilesLimit = 20
        public var detailedLogging = false
        public var defaultCodePage = "auto"
        public var unixZipUTF8Detection = true
        public var appNapPolicy = "allow"      // allow | prevent
        public var preciseProgressTime = false
        public var enginePath: String?
        public init() {}
    }

    public struct Sandbox: Codable, Sendable, Equatable {
        /// Folders the Finder extension may act on (FR-511). Shared with the extension via the app group.
        public var accessibleFolders: [String] = []
        public init() {}
    }

    public struct Language: Codable, Sendable, Equatable {
        public var code = "zh-Hans"
        public init() {}
    }

    public var version: Int = Preferences.currentVersion
    public var general = General()
    public var fileAssociation = FileAssociation()
    public var finderMenu = FinderMenu()
    public var extraction = Extraction()
    public var compression = Compression()
    public var view = View()
    public var advanced = Advanced()
    public var sandbox = Sandbox()
    public var language = Language()

    public init() {}

    /// FR-115 default incompressible-extension list (05 section 2).
    public static let defaultSkipList: [String] = {
        let raw = """
        7z xz lzma ace arc arj bz tbz bz2 tbz2 cab deb gz tgz ha lha lzh lzo lzx pak rar rpm sit zoo zip jar ear war msi zst br lz4 zpaq egg alz \
        3gp avi mov mpeg mpg mpe wmv aac ape fla flac la mp3 m4a mp4 ofr ogg pac ra rm rka shn swa tta wv wma wav swf \
        gif jpeg jpg jp2 png tiff bmp ico psd psp \
        pdf doc docx dot rtf xls xlr xlt xlw ppt sxc sxd sxi sxg sxw odt ott odg otg odp otp ods ots odf \
        exe dll ocx com obj lib so dylib sfx sys tlb class \
        iso bin nrg mdf img vfd vhd vmdk dsk nvram vmem \
        awg ps eps ai otf ttf fon
        """
        var seen = Set<String>()
        return raw.split(whereSeparator: { $0 == " " || $0 == "\n" }).map(String.init).filter { seen.insert($0).inserted }
    }()
}

public enum PreferencesError: Error, LocalizedError {
    case newerVersion(Int)
    case invalid

    public var errorDescription: String? {
        switch self {
        case .newerVersion(let v): return L10n.f("This settings file was written by a newer FastZip (format %@).", String(v))
        case .invalid: return L10n.t("This is not a valid FastZip settings file.")
        }
    }
}

/// Load/save/import/export with forward-compatible merging: missing keys fall
/// back to defaults, so adding a setting never invalidates older files.
public struct PreferencesStore: Sendable {
    public let fileURL: URL

    public init(fileURL: URL = PreferencesStore.defaultFileURL) {
        self.fileURL = fileURL
    }

    public static var defaultFileURL: URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSTemporaryDirectory())
        return base.appendingPathComponent("OpenSiliconZip", isDirectory: true).appendingPathComponent("preferences.json")
    }

    public func load() -> Preferences {
        guard let data = try? Data(contentsOf: fileURL),
              let prefs = try? Self.decode(data) else { return Preferences() }
        return prefs
    }

    public func save(_ prefs: Preferences) throws {
        try FileManager.default.createDirectory(at: fileURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Self.encode(prefs).write(to: fileURL, options: .atomic)
    }

    public func reset() throws -> Preferences {
        let p = Preferences()
        try save(p)
        return p
    }

    public func export(to url: URL, prefs: Preferences) throws {
        try Self.encode(prefs).write(to: url, options: .atomic)
    }

    public func importFile(_ url: URL) throws -> Preferences {
        var prefs = try Self.decode(Data(contentsOf: url))
        // An imported configuration is data, not authorization to execute a new
        // program (which would also receive future archive passwords on stdin).
        // Only the local Advanced preference can change the engine executable.
        prefs.advanced.enginePath = load().advanced.enginePath
        try save(prefs)
        return prefs
    }

    public static func encode(_ prefs: Preferences) throws -> Data {
        let e = JSONEncoder()
        e.outputFormatting = [.prettyPrinted, .sortedKeys]
        return try e.encode(prefs)
    }

    public static func decode(_ data: Data) throws -> Preferences {
        guard var incoming = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else {
            throw PreferencesError.invalid
        }
        let version = (incoming["version"] as? Int) ?? 0
        if version > Preferences.currentVersion { throw PreferencesError.newerVersion(version) }
        incoming = migrate(incoming, from: version)

        let defaults = try JSONSerialization.jsonObject(with: encode(Preferences())) as? [String: Any] ?? [:]
        let merged = deepMerge(defaults, incoming)
        let mergedData = try JSONSerialization.data(withJSONObject: merged)
        return try JSONDecoder().decode(Preferences.self, from: mergedData)
    }

    /// Migration hook: add one `if version < N` step per schema bump.
    static func migrate(_ dict: [String: Any], from version: Int) -> [String: Any] {
        var d = dict
        // Version 2 restores direct Finder actions and defaults unspecified languages to Chinese.
        if version < 2 {
            var menu = d["finderMenu"] as? [String: Any] ?? [:]
            menu["cascaded"] = false
            d["finderMenu"] = menu
            var language = d["language"] as? [String: Any] ?? [:]
            if !["en", "zh-Hans", "zh-Hant"].contains(language["code"] as? String ?? "") { language["code"] = "zh-Hans" }
            d["language"] = language
        }
        d["version"] = Preferences.currentVersion
        return d
    }

    static func deepMerge(_ base: [String: Any], _ overlay: [String: Any]) -> [String: Any] {
        var out = base
        for (k, v) in overlay {
            if let sub = v as? [String: Any], let baseSub = out[k] as? [String: Any] {
                out[k] = deepMerge(baseSub, sub)
            } else {
                out[k] = v
            }
        }
        return out
    }
}
