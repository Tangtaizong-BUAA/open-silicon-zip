import Foundation

// MARK: - Formats, levels, policies

public enum ArchiveFormat: String, Codable, Sendable, CaseIterable, Identifiable {
    case zip, zipx
    case sevenZ = "7z"
    case tar, gz, tgz, bz2, tbz, xz, txz, lzma, zst, lz4, br

    public var id: String { rawValue }
    /// Value passed to `fz create --format`.
    public var cliName: String { rawValue }
    public var fileExtension: String { rawValue }

    public var displayName: String {
        switch self {
        case .sevenZ: return "7Z"
        default: return rawValue.uppercased()
        }
    }

    /// FR-106: only ZIP/ZIPX/7Z carry encryption.
    public var supportsPassword: Bool {
        self == .zip || self == .zipx || self == .sevenZ
    }

    /// Per-volume ceiling for spanned ZIP (05 section 4).
    public var maxVolumeBytes: UInt64? {
        switch self {
        case .zip, .zipx: return 4 * 1024 * 1024 * 1024
        default: return nil
        }
    }
}

public enum CompressionLevel: String, Codable, Sendable, CaseIterable, Identifiable {
    case store, fast, normal, max

    public var id: String { rawValue }
    public var cliName: String { rawValue }
    public var displayName: String {
        switch self {
        case .store: return L10n.t("Store")
        case .fast: return L10n.t("Fast")
        case .normal: return L10n.t("Normal")
        case .max: return L10n.t("Maximum")
        }
    }
}

/// Behaviour when a target already exists (FR-112 / FR-211).
public enum ConflictPolicy: String, Codable, Sendable, CaseIterable, Identifiable {
    case ask, skip, overwrite, rename

    public var id: String { rawValue }
    public var displayName: String {
        switch self {
        case .ask: return L10n.t("Ask")
        case .skip: return L10n.t("Skip")
        case .overwrite: return L10n.t("Overwrite")
        case .rename: return L10n.t("Keep both (rename)")
        }
    }
}

// MARK: - Split sizes (FR-104, FR-1201)

public enum SplitSize {
    public static let minimumBytes: UInt64 = 100 * 1024

    public struct Preset: Identifiable, Hashable, Sendable {
        public let label: String
        public let bytes: UInt64
        public var id: UInt64 { bytes }
    }

    public static let presets: [Preset] = [
        Preset(label: "10 MB", bytes: 10 * 1_048_576),
        Preset(label: "20 MB", bytes: 20 * 1_048_576),
        Preset(label: "25 MB", bytes: 25 * 1_048_576),
        Preset(label: "100 MB", bytes: 100 * 1_048_576),
        Preset(label: "650 MB", bytes: 650 * 1_048_576),
        Preset(label: "700 MB", bytes: 700 * 1_048_576),
        Preset(label: "1.95 GB", bytes: UInt64(1.95 * 1_073_741_824)),
        Preset(label: "4 GB", bytes: 4 * 1_073_741_824),
    ]

    /// Parses "700", "700MB", "1.5 GB", "512k" (plain numbers are megabytes).
    public static func parse(_ text: String) -> UInt64? {
        let t = text.trimmingCharacters(in: .whitespaces).lowercased()
        guard !t.isEmpty else { return nil }
        var number = ""
        var unit = ""
        for ch in t {
            if ch.isNumber || ch == "." { number.append(ch) }
            else if ch == " " { continue }
            else { unit.append(ch) }
        }
        guard let value = Double(number), value > 0 else { return nil }
        let mult: Double
        switch unit {
        case "", "m", "mb", "mib": mult = 1_048_576
        case "k", "kb", "kib": mult = 1024
        case "g", "gb", "gib": mult = 1_073_741_824
        case "b": mult = 1
        default: return nil
        }
        let bytes = value * mult
        guard bytes < Double(UInt64.max) else { return nil }
        return UInt64(bytes)
    }

    public static func validate(_ bytes: UInt64) -> FZError? {
        bytes >= minimumBytes ? nil : FZError(code: .invalidSplitSize, message: "Volume size must be at least 100 KB.")
    }
}

// MARK: - Create settings and presets (FR-1101)

public struct CreateOptionsFlags: Codable, Sendable, Equatable {
    public var testAfter = false
    public var openAfter = false
    public var deleteSources = false
    public var oneArchivePerItem = false
    public init() {}
}

public struct CreateSettings: Codable, Sendable, Equatable {
    public var format: ArchiveFormat = .zip
    public var level: CompressionLevel = .normal
    public var splitBytes: UInt64?
    public var threads: Int?              // nil = Auto
    public var options = CreateOptionsFlags()
    public var filenameRule: String = "${NAME}"
    public var conflict: ConflictPolicy = .ask
    public var excludeGlobs: [String] = []
    public init() {}
}

public struct Preset: Codable, Sendable, Equatable, Identifiable {
    public var id: UUID
    public var name: String
    public var settings: CreateSettings
    /// FR-605/1104: reference into the PasswordStore; never the secret itself.
    public var passwordItemID: UUID?
    public var isDefault: Bool

    public init(id: UUID = UUID(), name: String, settings: CreateSettings = CreateSettings(),
                passwordItemID: UUID? = nil, isDefault: Bool = false) {
        self.id = id
        self.name = name
        self.settings = settings
        self.passwordItemID = passwordItemID
        self.isDefault = isDefault
    }
}

// MARK: - Entries

public struct ArchiveEntry: Identifiable, Hashable, Sendable {
    public var name: String
    public var size: UInt64
    public var compressedSize: UInt64
    public var modified: Date?
    public var method: String
    public var encrypted: Bool
    public var encryptionMethod: String?
    public var crc: UInt32?
    public var attributes: UInt32?
    public var isDirectory: Bool
    public var isSymlink: Bool
    public var comment: String?

    public var id: String { name }

    public init(name: String, size: UInt64 = 0, compressedSize: UInt64 = 0, modified: Date? = nil,
                method: String = "", encrypted: Bool = false, encryptionMethod: String? = nil,
                crc: UInt32? = nil, attributes: UInt32? = nil, isDirectory: Bool = false,
                isSymlink: Bool = false, comment: String? = nil) {
        self.name = name
        self.size = size
        self.compressedSize = compressedSize
        self.modified = modified
        self.method = method
        self.encrypted = encrypted
        self.encryptionMethod = encryptionMethod
        self.crc = crc
        self.attributes = attributes
        self.isDirectory = isDirectory
        self.isSymlink = isSymlink
        self.comment = comment
    }

    /// Path components after separator normalisation.
    public var components: [String] {
        name.replacingOccurrences(of: "\\", with: "/")
            .split(separator: "/", omittingEmptySubsequences: true)
            .map(String.init)
            .filter { $0 != "." }
    }

    public var displayName: String { components.last ?? name }

    public var parentPath: String { components.dropLast().joined(separator: "/") }

    public var fileExtension: String {
        guard !isDirectory else { return "" }
        let n = displayName
        guard let dot = n.lastIndex(of: "."), dot != n.startIndex else { return "" }
        return String(n[n.index(after: dot)...]).lowercased()
    }

    public var kindDescription: String {
        if isDirectory { return L10n.t("Folder") }
        if isSymlink { return L10n.t("Symbolic link") }
        let e = fileExtension
        return e.isEmpty ? L10n.t("File") : L10n.f("%@ file", e.uppercased())
    }

    public var compressionRatio: Double? {
        guard size > 0, !isDirectory else { return nil }
        return 1.0 - Double(compressedSize) / Double(size)
    }

    /// Detects a unix symlink from the high 16 bits of external attributes.
    public static func symlinkFromAttributes(_ attrs: UInt32?) -> Bool {
        guard let attrs else { return false }
        return ((attrs >> 16) & 0o170000) == 0o120000
    }
}

public struct HashResult: Sendable, Equatable, Identifiable {
    public var path: String
    /// Lowercased algorithm name ("crc32", "md5", "sha1", "sha256", "crc64") to hex digest.
    public var digests: [String: String]
    public var id: String { path }
    public init(path: String, digests: [String: String]) {
        self.path = path
        self.digests = digests
    }
}

// MARK: - Progress

public struct ProgressSnapshot: Sendable, Equatable {
    public var phase: String = ""
    public var current: String = ""
    public var bytesRead: UInt64 = 0
    public var bytesWritten: UInt64 = 0
    public var bytesTotal: UInt64 = 0
    public var filesDone: Int = 0
    public var filesTotal: Int = 0
    /// Filled in by `ProgressTracker`, not by the engine.
    public var elapsed: TimeInterval = 0
    public var throughput: Double = 0      // bytes per second, smoothed
    public var eta: TimeInterval?

    public init() {}

    public var bytesProcessed: UInt64 { max(bytesRead, bytesWritten) }

    /// nil when indeterminate.
    public var fraction: Double? {
        if bytesTotal > 0 { return min(1, Double(bytesProcessed) / Double(bytesTotal)) }
        if filesTotal > 0 { return min(1, Double(filesDone) / Double(filesTotal)) }
        return nil
    }
}

/// Smooths throughput (EMA) and derives elapsed/ETA. Pure and clock-injected.
public struct ProgressTracker: Sendable {
    private var startedAt: Date
    private var lastAt: Date?
    private var lastBytes: UInt64 = 0
    private var smoothed: Double = 0
    private var pausedTotal: TimeInterval = 0
    private var pausedAt: Date?
    private let alpha: Double = 0.3

    public init(startedAt: Date = Date()) { self.startedAt = startedAt }

    public mutating func markPaused(at now: Date) {
        if pausedAt == nil { pausedAt = now }
    }

    public mutating func markResumed(at now: Date) {
        if let p = pausedAt { pausedTotal += now.timeIntervalSince(p) }
        pausedAt = nil
        lastAt = now
    }

    public mutating func ingest(_ snapshot: ProgressSnapshot, at now: Date) -> ProgressSnapshot {
        var s = snapshot
        let bytes = s.bytesProcessed
        if let last = lastAt {
            let dt = now.timeIntervalSince(last)
            if dt > 0.05 {
                let delta = bytes >= lastBytes ? Double(bytes - lastBytes) : 0
                let instant = delta / dt
                smoothed = smoothed == 0 ? instant : (alpha * instant + (1 - alpha) * smoothed)
                lastAt = now
                lastBytes = bytes
            }
        } else {
            lastAt = now
            lastBytes = bytes
        }
        s.elapsed = max(0, now.timeIntervalSince(startedAt) - pausedTotal)
        s.throughput = smoothed
        if smoothed > 1, s.bytesTotal > bytes {
            s.eta = Double(s.bytesTotal - bytes) / smoothed
        } else {
            s.eta = nil
        }
        return s
    }
}

// MARK: - Job kinds and states

public enum JobKind: String, Codable, Sendable, CaseIterable {
    case create, extract, test, list, hash, repair, edit

    public var displayName: String {
        switch self {
        case .create: return L10n.t("Compress")
        case .extract: return L10n.t("Extract")
        case .test: return L10n.t("Test")
        case .list: return L10n.t("List")
        case .hash: return L10n.t("Checksum")
        case .repair: return L10n.t("Repair")
        case .edit: return L10n.t("Edit")
        }
    }

    public var systemImage: String {
        switch self {
        case .create: return "archivebox"
        case .extract: return "arrow.down.doc"
        case .test: return "checkmark.shield"
        case .list: return "list.bullet"
        case .hash: return "number"
        case .repair: return "wrench.and.screwdriver"
        case .edit: return "pencil"
        }
    }
}

public enum JobState: String, Codable, Sendable, CaseIterable {
    case queued, running, paused, success, failed, skipped, cancelled

    public var isTerminal: Bool {
        switch self {
        case .success, .failed, .skipped, .cancelled: return true
        default: return false
        }
    }

    /// The legal transition table used by `JobQueue`.
    public func canTransition(to next: JobState) -> Bool {
        switch (self, next) {
        case (.queued, .running), (.queued, .skipped), (.queued, .cancelled):
            return true
        case (.running, .paused), (.running, .success), (.running, .failed), (.running, .cancelled):
            return true
        case (.paused, .running), (.paused, .cancelled), (.paused, .failed):
            return true
        case (.failed, .queued), (.cancelled, .queued):   // retry
            return true
        default:
            return false
        }
    }

    public var displayName: String {
        switch self {
        case .queued: return L10n.t("Queued")
        case .running: return L10n.t("Running")
        case .paused: return L10n.t("Paused")
        case .success: return L10n.t("Done")
        case .failed: return L10n.t("Failed")
        case .skipped: return L10n.t("Skipped")
        case .cancelled: return L10n.t("Cancelled")
        }
    }
}
