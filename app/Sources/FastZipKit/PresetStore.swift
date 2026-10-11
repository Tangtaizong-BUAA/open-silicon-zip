import Foundation

public enum PresetStoreError: Error, Equatable, LocalizedError {
    case duplicateName(String)
    case emptyName
    case notFound

    public var errorDescription: String? {
        switch self {
        case .duplicateName(let n): return L10n.f("A preset named \"%@\" already exists.", n)
        case .emptyName: return L10n.t("Preset name cannot be empty.")
        case .notFound: return L10n.t("Preset not found.")
        }
    }
}

/// Presets as one JSON file each in `Application Support/OpenSiliconZip/Presets` (FR-1101..1103).
/// Passwords are never stored here, only a `passwordItemID` into `PasswordStore`.
public struct PresetStore: Sendable {
    public let directory: URL

    public init(directory: URL = PresetStore.defaultDirectory) {
        self.directory = directory
    }

    public static var defaultDirectory: URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSTemporaryDirectory())
        return base.appendingPathComponent("OpenSiliconZip", isDirectory: true)
            .appendingPathComponent("Presets", isDirectory: true)
    }

    private var encoder: JSONEncoder {
        let e = JSONEncoder()
        e.outputFormatting = [.prettyPrinted, .sortedKeys]
        return e
    }

    private func fileURL(_ id: UUID) -> URL {
        directory.appendingPathComponent("\(id.uuidString).json")
    }

    public func all() -> [Preset] {
        let fm = FileManager.default
        guard let files = try? fm.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil) else { return [] }
        let decoder = JSONDecoder()
        return files
            .filter { $0.pathExtension == "json" }
            .compactMap { try? decoder.decode(Preset.self, from: Data(contentsOf: $0)) }
            .sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
    }

    public func preset(named name: String) -> Preset? {
        all().first { $0.name.caseInsensitiveCompare(name) == .orderedSame }
    }

    public func defaultPreset() -> Preset? { all().first { $0.isDefault } }

    /// Creates or updates. Duplicate names (case-insensitive) are rejected.
    @discardableResult
    public func save(_ preset: Preset) throws -> Preset {
        let name = preset.name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { throw PresetStoreError.emptyName }
        let existing = all()
        if existing.contains(where: { $0.id != preset.id && $0.name.caseInsensitiveCompare(name) == .orderedSame }) {
            throw PresetStoreError.duplicateName(name)
        }
        var p = preset
        p.name = name
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        // Only one default preset at a time.
        if p.isDefault {
            for var other in existing where other.isDefault && other.id != p.id {
                other.isDefault = false
                try encoder.encode(other).write(to: fileURL(other.id), options: .atomic)
            }
        }
        try encoder.encode(p).write(to: fileURL(p.id), options: .atomic)
        return p
    }

    public func delete(_ id: UUID) throws {
        let url = fileURL(id)
        guard FileManager.default.fileExists(atPath: url.path) else { throw PresetStoreError.notFound }
        try FileManager.default.removeItem(at: url)
    }

    public func duplicate(_ id: UUID) throws -> Preset {
        guard let src = all().first(where: { $0.id == id }) else { throw PresetStoreError.notFound }
        let names = Set(all().map { $0.name.lowercased() })
        var n = 2
        var candidate = "\(src.name) copy"
        while names.contains(candidate.lowercased()) {
            candidate = "\(src.name) copy \(n)"
            n += 1
        }
        return try save(Preset(name: candidate, settings: src.settings, passwordItemID: src.passwordItemID))
    }
}
