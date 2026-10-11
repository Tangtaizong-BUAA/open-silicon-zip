import Foundation

public enum Format {
    public static func bytes(_ n: UInt64, exact: Bool = false) -> String {
        if exact { return L10n.f("%@ bytes", Self.grouped(n)) }
        return ByteCountFormatter.string(fromByteCount: Int64(clamping: n), countStyle: .file)
    }

    private static func grouped(_ n: UInt64) -> String {
        let f = NumberFormatter()
        f.numberStyle = .decimal
        return f.string(from: NSNumber(value: n)) ?? String(n)
    }

    public static func rate(_ bytesPerSecond: Double) -> String {
        guard bytesPerSecond.isFinite, bytesPerSecond > 0, bytesPerSecond < Double(Int64.max) else { return "-" }
        return ByteCountFormatter.string(fromByteCount: Int64(bytesPerSecond), countStyle: .file) + "/s"
    }

    /// Coarse ("about 2 min") or precise ("00:02:13") durations (FR-905 precise progress time).
    public static func duration(_ seconds: TimeInterval?, precise: Bool) -> String {
        guard let seconds, seconds.isFinite, seconds >= 0, seconds < Double(Int.max) / 2 else { return "-" }
        let total = Int(seconds.rounded())
        if precise {
            return String(format: "%02ld:%02ld:%02ld", total / 3600, (total % 3600) / 60, total % 60)
        }
        switch total {
        case 0..<5: return L10n.t("a few seconds")
        case 5..<60: return L10n.f("%ld sec", total / 5 * 5)
        case 60..<3600: return L10n.f("about %ld min", max(1, Int((Double(total) / 60).rounded())))
        default:
            let h = total / 3600
            let m = (total % 3600) / 60
            return m == 0 ? L10n.f("%ld h", h) : L10n.f("%ld h %ld min", h, m)
        }
    }

    public static func percent(_ fraction: Double?) -> String {
        guard let f = fraction, f.isFinite else { return "" }
        return "\(Int((min(1, max(0, f)) * 100).rounded()))%"
    }
}

/// Password strength hint for the New Archive sheet (FR-105). Heuristic only.
public enum PasswordStrength: Int, Sendable, Comparable {
    case empty, weak, fair, good, strong

    public static func < (a: PasswordStrength, b: PasswordStrength) -> Bool { a.rawValue < b.rawValue }

    public static func evaluate(_ p: String) -> PasswordStrength {
        if p.isEmpty { return .empty }
        var classes = 0
        if p.contains(where: { $0.isLowercase }) { classes += 1 }
        if p.contains(where: { $0.isUppercase }) { classes += 1 }
        if p.contains(where: { $0.isNumber }) { classes += 1 }
        if p.contains(where: { !$0.isLetter && !$0.isNumber }) { classes += 1 }
        let score = (p.count >= 8 ? 1 : 0) + (p.count >= 12 ? 1 : 0) + (p.count >= 16 ? 1 : 0) + classes
        switch score {
        case ..<3: return .weak
        case 3: return .fair
        case 4...5: return .good
        default: return .strong
        }
    }

    public var hint: String {
        switch self {
        case .empty: return L10n.t("Leave empty for no encryption")
        case .weak: return L10n.t("Weak. Use at least 8 characters")
        case .fair: return L10n.t("Fair. Add length or symbols")
        case .good: return L10n.t("Good")
        case .strong: return L10n.t("Strong")
        }
    }
}
