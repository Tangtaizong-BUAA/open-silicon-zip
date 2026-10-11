import XCTest
@testable import FastZipKit

/// Verifies the zh-Hans / zh-Hant tables ship in the module bundle and translate
/// the load-bearing keys (Finder menus, error taxonomy, enum display names).
final class LocalizationTests: XCTestCase {
    override func setUp() {
        super.setUp()
        L10n.forceKeys = true
    }

    /// SwiftPM/Xcode lowercase `.lproj` directory names when copying into the
    /// bundle (zh-Hans -> zh-hans); runtime lookup is case-insensitive, but
    /// explicit path resolution is not, so discover directories tolerantly.
    private func lproj(_ lang: String) -> Bundle? {
        let dirs = Bundle.module.paths(forResourcesOfType: "lproj", inDirectory: nil)
        guard let p = dirs.first(where: {
            URL(fileURLWithPath: $0).deletingPathExtension().lastPathComponent.lowercased() == lang.lowercased()
        }) else { return nil }
        return Bundle(path: p)
    }

    private func lookup(_ bundle: Bundle, _ key: String) -> String {
        bundle.localizedString(forKey: key, value: "␦missing", table: nil)
    }

    func testBothChineseTablesExist() {
        XCTAssertNotNil(lproj("zh-Hans"), "zh-Hans.lproj missing from FastZipKit bundle")
        XCTAssertNotNil(lproj("zh-Hant"), "zh-Hant.lproj missing from FastZipKit bundle")
    }

    func testFinderMenuTitlesTranslate() throws {
        let hans = try XCTUnwrap(lproj("zh-Hans"))
        XCTAssertEqual(lookup(hans, "Extract Here (Smart)"), "解压到此处（智能）")
        XCTAssertEqual(lookup(hans, "Compress with FastZip..."), "使用 Open Silicon Zip 压缩…")
        XCTAssertEqual(lookup(hans, "Test Archive"), "测试压缩包")
        let hant = try XCTUnwrap(lproj("zh-Hant"))
        XCTAssertEqual(lookup(hant, "Extract Here (Smart)"), "解壓縮到此處（智慧）")
    }

    func testFormatKeysTranslate() throws {
        let hans = try XCTUnwrap(lproj("zh-Hans"))
        let title = String(format: lookup(hans, "Compress to \"%@.%@\""), locale: Locale(identifier: "zh_Hans"), "报告", "zip")
        XCTAssertEqual(title, "压缩为“报告.zip”")
    }

    func testErrorTaxonomyTranslates() throws {
        let hans = try XCTUnwrap(lproj("zh-Hans"))
        XCTAssertEqual(lookup(hans, "Wrong password"), "密码错误")
        XCTAssertNotEqual(lookup(hans, "Another volume is needed"), "␦missing")
    }

    func testForceKeysReturnsEnglishRegardlessOfSystemLanguage() {
        XCTAssertEqual(L10n.t("Extract Here (Smart)"), "Extract Here (Smart)")
    }
}
