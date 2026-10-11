import Foundation

/// Where Smart Extract will put things. Mirrors 05-typical-feature-specs section 1.
public struct SmartExtractPlan: Equatable, Sendable {
    public enum Kind: Equatable, Sendable {
        /// Archive has no (usable) entries.
        case nothing
        /// Step 2: exactly one root directory; its contents land directly in the destination.
        case stripSingleRoot(String)
        /// Step 3 (default): exactly one root file; extracted directly.
        case singleFile(String)
        /// Step 3 with "wrap single files" or step 4 (multiple roots): `<ArchiveName>/`.
        case wrapped(String)
    }

    public var kind: Kind
    /// Folder (relative to the destination) to create and extract into; nil = destination itself.
    public var wrapperFolder: String?
    /// Leading path components the engine must strip (1 for `stripSingleRoot`).
    public var stripComponents: Int
    /// Names that will appear directly inside the destination; used for the FR-211 pre-check.
    public var collisionCandidates: [String]
    /// Entries with absolute paths, drive letters or `..` (FR-216); the engine must refuse them.
    public var unsafeEntries: [String]

    public init(kind: Kind, wrapperFolder: String?, stripComponents: Int,
                collisionCandidates: [String], unsafeEntries: [String]) {
        self.kind = kind
        self.wrapperFolder = wrapperFolder
        self.stripComponents = stripComponents
        self.collisionCandidates = collisionCandidates
        self.unsafeEntries = unsafeEntries
    }
}

public struct PlanInput: Sendable, Equatable {
    public var path: String
    public var isDirectory: Bool
    public var isSymlink: Bool
    public init(path: String, isDirectory: Bool = false, isSymlink: Bool = false) {
        self.path = path
        self.isDirectory = isDirectory
        self.isSymlink = isSymlink
    }
}

public enum SmartExtract {

    // MARK: Planning

    public static func plan(entries: [ArchiveEntry], archiveName: String,
                            wrapSingleFiles: Bool = false, ignoreJunk: Bool = true) -> SmartExtractPlan {
        plan(inputs: entries.map { PlanInput(path: $0.name, isDirectory: $0.isDirectory, isSymlink: $0.isSymlink) },
             archiveName: archiveName, wrapSingleFiles: wrapSingleFiles, ignoreJunk: ignoreJunk)
    }

    public static func plan(inputs: [PlanInput], archiveName: String,
                            wrapSingleFiles: Bool = false, ignoreJunk: Bool = true) -> SmartExtractPlan {
        struct Root {
            var displayName: String
            var isDirectory = false
            var childKeys: [String: String] = [:]   // key -> display name of first-level children
            var childOrder: [String] = []
        }

        var roots: [String: Root] = [:]
        var rootOrder: [String] = []
        var unsafe: [String] = []

        for input in inputs {
            let comps = components(of: input.path)
            if comps.isEmpty { continue }
            if ignoreJunk && isJunk(comps) { continue }
            if isUnsafe(input.path, comps) { unsafe.append(input.path) }

            let key = normalizedKey(comps[0])
            var root = roots[key] ?? Root(displayName: comps[0])
            if roots[key] == nil { rootOrder.append(key) }

            if comps.count > 1 {
                root.isDirectory = true
                let childKey = normalizedKey(comps[1])
                if root.childKeys[childKey] == nil {
                    root.childKeys[childKey] = comps[1]
                    root.childOrder.append(childKey)
                }
            } else if input.isDirectory && !input.isSymlink {
                root.isDirectory = true
            }
            roots[key] = root
        }

        let base = archiveBaseName(archiveName)

        guard !rootOrder.isEmpty else {
            return SmartExtractPlan(kind: .nothing, wrapperFolder: nil, stripComponents: 0,
                                    collisionCandidates: [], unsafeEntries: unsafe)
        }

        if rootOrder.count == 1, let root = roots[rootOrder[0]] {
            if root.isDirectory {
                let children = root.childOrder.compactMap { root.childKeys[$0] }
                return SmartExtractPlan(kind: .stripSingleRoot(root.displayName), wrapperFolder: nil,
                                        stripComponents: 1, collisionCandidates: children,
                                        unsafeEntries: unsafe)
            }
            if wrapSingleFiles {
                return SmartExtractPlan(kind: .wrapped(base), wrapperFolder: base, stripComponents: 0,
                                        collisionCandidates: [base], unsafeEntries: unsafe)
            }
            return SmartExtractPlan(kind: .singleFile(root.displayName), wrapperFolder: nil, stripComponents: 0,
                                    collisionCandidates: [root.displayName], unsafeEntries: unsafe)
        }

        return SmartExtractPlan(kind: .wrapped(base), wrapperFolder: base, stripComponents: 0,
                                collisionCandidates: [base], unsafeEntries: unsafe)
    }

    // MARK: Collisions and destination checks

    /// Candidates that already exist in the destination. Never merge silently (step 5).
    public static func collisions(for plan: SmartExtractPlan, existingNames: [String],
                                  caseInsensitive: Bool = true) -> [String] {
        let existing = Set(existingNames.map { caseInsensitive ? normalizedKey($0) : $0.precomposedStringWithCanonicalMapping })
        return plan.collisionCandidates.filter {
            existing.contains(caseInsensitive ? normalizedKey($0) : $0.precomposedStringWithCanonicalMapping)
        }
    }

    /// Pre-flight for the "read-only destination" shape. `isWritable` is injected for testing.
    public static func validateDestination(_ path: String,
                                           isWritable: (String) -> Bool = { FileManager.default.isWritableFile(atPath: $0) }) throws {
        guard isWritable(path) else {
            throw FZError(code: .permissionDenied, message: "\(path) is read-only.")
        }
    }

    // MARK: Path normalisation

    /// Separator-normalised components without empty or `.` parts.
    public static func components(of path: String) -> [String] {
        path.replacingOccurrences(of: "\\", with: "/")
            .split(separator: "/", omittingEmptySubsequences: true)
            .map(String.init)
            .filter { $0 != "." }
    }

    /// Case- and Unicode-normalised comparison key (CJK-safe, NFC/NFD-safe).
    public static func normalizedKey(_ s: String) -> String {
        s.precomposedStringWithCanonicalMapping.lowercased()
    }

    static func isJunk(_ comps: [String]) -> Bool {
        if comps[0] == "__MACOSX" { return true }
        guard let last = comps.last else { return false }
        return last.hasPrefix("._") || last == ".DS_Store" || last == "Thumbs.db"
    }

    static func isUnsafe(_ original: String, _ comps: [String]) -> Bool {
        if original.hasPrefix("/") || original.hasPrefix("\\") { return true }
        if comps.contains("..") { return true }
        if let first = comps.first, first.count == 2, first.hasSuffix(":"),
           first.first?.isLetter == true { return true }
        return false
    }

    // MARK: Archive name

    private static let archiveExtensions: Set<String> = [
        "zip", "zipx", "7z", "rar", "tar", "gz", "tgz", "bz2", "tbz", "tbz2", "xz", "txz",
        "lzma", "zst", "lz4", "br", "cab", "iso", "jar", "war", "ear", "z",
    ]

    /// "photos.tar.gz" -> "photos", "a.7z.001" -> "a", "b.z01" -> "b", "c.part1.rar" -> "c".
    public static func archiveBaseName(_ fileName: String) -> String {
        var name = (fileName as NSString).lastPathComponent
        func dropExt(_ cond: (String) -> Bool) -> Bool {
            let ns = name as NSString
            let ext = ns.pathExtension.lowercased()
            if !ext.isEmpty && cond(ext) {
                name = ns.deletingPathExtension
                return true
            }
            return false
        }
        // volume suffixes: .001 / .z01
        _ = dropExt { $0.count == 3 && $0.allSatisfy { $0.isNumber } }
        _ = dropExt { $0.count == 3 && $0.hasPrefix("z") && $0.dropFirst().allSatisfy { $0.isNumber } }
        _ = dropExt { archiveExtensions.contains($0) }
        // .tar.<x> and .partN
        _ = dropExt { $0 == "tar" }
        let ns = name as NSString
        let ext = ns.pathExtension.lowercased()
        if ext.hasPrefix("part"), ext.dropFirst(4).allSatisfy({ $0.isNumber }), ext.count > 4 {
            name = ns.deletingPathExtension
        }
        return name.isEmpty ? "Archive" : name
    }
}
