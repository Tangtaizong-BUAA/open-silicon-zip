import Foundation

/// A filename encoding offered in the Codepage menu (FR-801/803).
public struct CodePage: Identifiable, Hashable, Sendable {
    /// Value passed to `fz ... --codepage`. "auto" means engine-side auto-detect (05 section 5).
    public let id: String
    public let name: String
    public let region: String

    public var menuTitle: String { id == "auto" ? name : "\(name) (\(shortLabel))" }

    public var shortLabel: String {
        switch id {
        case "utf-8": return "UTF-8"
        case "tis-620": return "TIS-620"
        default: return id.uppercased()
        }
    }
}

public enum CodePageCatalog {
    public static let auto = CodePage(id: "auto", name: "Auto-detect", region: "Automatic")

    public static let all: [CodePage] = [
        CodePage(id: "utf-8", name: "Unicode", region: "Universal"),
        CodePage(id: "cp437", name: "US DOS", region: "Western"),
        CodePage(id: "cp850", name: "Western European DOS", region: "Western"),
        CodePage(id: "cp1252", name: "Western European", region: "Western"),
        CodePage(id: "cp1250", name: "Central European", region: "Central European"),
        CodePage(id: "cp1251", name: "Cyrillic", region: "Cyrillic"),
        CodePage(id: "cp1254", name: "Turkish", region: "Turkish"),
        CodePage(id: "cp1255", name: "Hebrew", region: "Hebrew"),
        CodePage(id: "cp1256", name: "Arabic", region: "Arabic"),
        CodePage(id: "cp737", name: "Greek DOS", region: "Greek"),
        CodePage(id: "cp1257", name: "Baltic", region: "Baltic"),
        CodePage(id: "tis-620", name: "Thai", region: "Thai"),
        CodePage(id: "cp932", name: "Japanese (Shift-JIS)", region: "East Asian"),
        CodePage(id: "cp936", name: "Simplified Chinese (GB)", region: "East Asian"),
        CodePage(id: "cp949", name: "Korean (EUC-KR)", region: "East Asian"),
        CodePage(id: "cp950", name: "Traditional Chinese (Big5)", region: "East Asian"),
    ]

    /// Auto first, then the catalog.
    public static var menuItems: [CodePage] { [auto] + all }

    public static func page(id: String) -> CodePage? {
        id == auto.id ? auto : all.first { $0.id == id }
    }

    /// Catalog grouped by region for sectioned menus, in stable order.
    public static var groupedByRegion: [(region: String, pages: [CodePage])] {
        var order: [String] = []
        var map: [String: [CodePage]] = [:]
        for p in all {
            if map[p.region] == nil { order.append(p.region) }
            map[p.region, default: []].append(p)
        }
        return order.map { (region: $0, pages: map[$0] ?? []) }
    }
}
