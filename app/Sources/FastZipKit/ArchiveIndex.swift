import Foundation

/// Folder node for the browser sidebar (FR-002).
public struct FolderNode: Identifiable, Hashable, Sendable {
    public let path: String            // "" for root
    public let name: String
    public var children: [FolderNode]? // nil = leaf (so SwiftUI's OutlineGroup shows no disclosure)
    public var id: String { path }

    public init(path: String, name: String, children: [FolderNode]? = nil) {
        self.path = path
        self.name = name
        self.children = children
    }
}

/// In-memory index of an archive listing: tree, per-folder listings with
/// synthesised implied directories, aggregate folder sizes (FR-006) and search (FR-008).
public struct ArchiveIndex: Sendable {
    public let entries: [ArchiveEntry]
    private var byParent: [String: [ArchiveEntry]] = [:]
    private var folderSizes: [String: (size: UInt64, csize: UInt64)] = [:]
    public private(set) var root: [FolderNode] = []

    public init(entries: [ArchiveEntry]) {
        self.entries = entries
        var dirs: [String: ArchiveEntry] = [:]
        var files: [ArchiveEntry] = []

        for e in entries {
            let comps = e.components
            guard !comps.isEmpty else { continue }
            var normalized = e
            normalized.name = comps.joined(separator: "/")
            // implied parent directories
            if comps.count > 1 {
                for i in 1..<comps.count {
                    let p = comps[0..<i].joined(separator: "/")
                    if dirs[p] == nil { dirs[p] = ArchiveEntry(name: p, isDirectory: true) }
                }
            }
            if e.isDirectory {
                var d = normalized
                d.isDirectory = true
                dirs[d.name] = d
            } else {
                files.append(normalized)
                var p = comps.dropLast()
                while !p.isEmpty {
                    let key = p.joined(separator: "/")
                    let cur = folderSizes[key] ?? (size: 0, csize: 0)
                    folderSizes[key] = (size: cur.size + e.size, csize: cur.csize + e.compressedSize)
                    p = p.dropLast()
                }
            }
        }

        for (path, var d) in dirs {
            if let agg = folderSizes[path] { d.size = agg.size; d.compressedSize = agg.csize }
            byParent[d.parentPath, default: []].append(d)
        }
        for f in files { byParent[f.parentPath, default: []].append(f) }

        let allDirPaths = Set(dirs.keys)
        func build(_ parent: String) -> [FolderNode] {
            (byParent[parent] ?? []).filter { $0.isDirectory && allDirPaths.contains($0.name) }
                .sorted { $0.displayName.localizedStandardCompare($1.displayName) == .orderedAscending }
                .map { d in
                    let kids = build(d.name)
                    return FolderNode(path: d.name, name: d.displayName, children: kids.isEmpty ? nil : kids)
                }
        }
        root = build("")
    }

    /// Direct children of a folder ("" = archive root).
    public func items(in folder: String) -> [ArchiveEntry] { byParent[folder] ?? [] }

    /// Case-insensitive substring search over full paths (optionally restricted to a folder).
    public func search(_ query: String, in folder: String = "", caseSensitive: Bool = false) -> [ArchiveEntry] {
        let q = query.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty else { return [] }
        let prefix = folder.isEmpty ? "" : folder + "/"
        var out: [ArchiveEntry] = []
        for list in byParent.values {
            for e in list where e.name.hasPrefix(prefix) {
                let hay = caseSensitive ? e.name : e.name.lowercased()
                let needle = caseSensitive ? q : q.lowercased()
                if hay.contains(needle) { out.append(e) }
            }
        }
        return out
    }

    public var totalFileCount: Int { entries.filter { !$0.isDirectory }.count }
    public var totalSize: UInt64 { entries.reduce(0) { $0 + ($1.isDirectory ? 0 : $1.size) } }

    /// Archive-relative paths to hand to `fz extract` for a selection (folders expand to their prefix).
    public static func extractionPaths(for selection: [ArchiveEntry]) -> [String] {
        selection.map { $0.isDirectory ? $0.name + "/" : $0.name }
    }
}
