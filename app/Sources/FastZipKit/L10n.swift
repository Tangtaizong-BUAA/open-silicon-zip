import Foundation

/// Localized strings for code-built (non-SwiftUI-literal) text in the kit:
/// Finder menu titles, error taxonomy, enum display names, notifications.
///
/// The key is always the English source string. When no table matches the
/// language (including tests running under `en` without an `en` table), the
/// key itself is returned, so English tests assert the exact same strings.
public enum L10n {
    #if SWIFT_PACKAGE
    static var bundle: Bundle { .module }
    #else
    // Xcode builds: resources live in the FastZipKit framework bundle.
    private final class BundleToken {}
    static var bundle: Bundle { Bundle(for: BundleToken.self) }
    #endif

    /// Test hook: return the key unchanged, ignoring the process language, so
    /// English assertions are environment-independent. Never set in production.
    public static var forceKeys = false

    public static var languageCode: String {
        get { UserDefaults.standard.string(forKey: "OpenSiliconZip.language") ?? "zh-Hans" }
        set { UserDefaults.standard.set(newValue, forKey: "OpenSiliconZip.language") }
    }

    public static func configure(language: String) {
        let code = ["en", "zh-Hans", "zh-Hant"].contains(language) ? language : "zh-Hans"
        languageCode = code
        UserDefaults.standard.set([code], forKey: "AppleLanguages")
    }

    /// Plain lookup without arguments. Never interpret percent signs until f().
    public static func t(_ key: String) -> String {
        if forceKeys { return key }
        let code = languageCode
        if code == "en" { return key.replacingOccurrences(of: "FastZip", with: "Open Silicon Zip") }
        let dirs = bundle.paths(forResourcesOfType: "lproj", inDirectory: nil)
        let path = dirs.first { URL(fileURLWithPath: $0).deletingPathExtension().lastPathComponent.lowercased() == code.lowercased() }
        let localized = path.flatMap(Bundle.init(path:)) ?? bundle
        return localized.localizedString(forKey: key, value: key, table: nil)
    }

    /// Format lookup: the key is a format string, arguments are substituted.
    public static func f(_ key: String, _ args: CVarArg...) -> String {
        String(format: t(key), locale: Locale.current, arguments: args)
    }
}
