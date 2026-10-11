import Foundation

/// Expands output-name rules (FR-503): `${NAME}` plus strftime-style `%Y %m %d %H %M %S`.
/// `%%` yields a literal percent sign; unknown `%x` sequences are kept verbatim.
public enum FilenameRule {

    public static let helpText = """
    ${NAME} source name   %Y year   %m month   %d day   %H hour   %M minute   %S second   %% percent sign
    Example: ${NAME}-%Y%m%d  ->  Report-20261010
    """

    public static func expand(_ rule: String, name: String, date: Date = Date(),
                              timeZone: TimeZone = .current) -> String {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = timeZone
        let c = calendar.dateComponents([.year, .month, .day, .hour, .minute, .second], from: date)

        func two(_ v: Int?) -> String { String(format: "%02d", v ?? 0) }

        var out = ""
        let chars = Array(rule)
        var i = 0
        let nameToken = Array("${NAME}")
        while i < chars.count {
            if chars[i] == "$", i + nameToken.count <= chars.count,
               Array(chars[i..<(i + nameToken.count)]) == nameToken {
                out += name
                i += nameToken.count
            } else if chars[i] == "%", i + 1 < chars.count {
                switch chars[i + 1] {
                case "Y": out += String(format: "%04d", c.year ?? 0)
                case "m": out += two(c.month)
                case "d": out += two(c.day)
                case "H": out += two(c.hour)
                case "M": out += two(c.minute)
                case "S": out += two(c.second)
                case "%": out += "%"
                default: out += "%" + String(chars[i + 1])
                }
                i += 2
            } else {
                out.append(chars[i])
                i += 1
            }
        }
        return sanitize(out, fallback: name)
    }

    /// Path separators are not allowed in a single file name.
    public static func sanitize(_ s: String, fallback: String) -> String {
        let cleaned = s.replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: ":", with: "_")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return cleaned.isEmpty ? fallback : cleaned
    }

    /// Appends `.ext` unless the name already ends with it (case-insensitive).
    public static func ensureExtension(_ name: String, _ ext: String) -> String {
        name.lowercased().hasSuffix("." + ext.lowercased()) ? name : name + "." + ext
    }

    /// Finder-style "name 2.ext" uniquing.
    public static func uniqueName(_ name: String, exists: (String) -> Bool) -> String {
        if !exists(name) { return name }
        let ns = name as NSString
        let ext = ns.pathExtension
        let stem = ns.deletingPathExtension
        var n = 2
        while true {
            let candidate = ext.isEmpty ? "\(stem) \(n)" : "\(stem) \(n).\(ext)"
            if !exists(candidate) { return candidate }
            n += 1
        }
    }
}
