import XCTest
@testable import FastZipKit

final class FinderMenuTests: XCTestCase {

    override func setUp() {
        super.setUp()
        L10n.forceKeys = true   // assertions are written against English
    }

    private func config(cascaded: Bool = false) -> FinderConfig {
        var c = FinderConfig()
        c.accessibleFolders = ["/Users/me"]
        c.cascaded = cascaded
        c.presetNames = ["Mail", "Backup"]
        c.defaultPresetName = "Backup"
        return c
    }

    private func titles(_ specs: [MenuSpec]) -> [String] { specs.filter { !$0.isSeparator }.map(\.title) }

    func testNothingOutsideAuthorisedFolders() {
        XCTAssertTrue(FinderMenuBuilder.build(selection: [URL(fileURLWithPath: "/etc/hosts")], config: config()).isEmpty)
    }

    func testCompressionMenuForFile() {
        let specs = FinderMenuBuilder.build(selection: [URL(fileURLWithPath: "/Users/me/report.pdf")], config: config())
        let t = titles(specs)
        XCTAssertTrue(t.contains("Compress with FastZip..."))
        XCTAssertTrue(t.contains("Compress to \"report.pdf.zip\""))
        XCTAssertTrue(t.contains("Compress to \"report.pdf.7z\""))
        XCTAssertEqual(t.filter { $0.hasPrefix("Compress with preset") }.first, "Compress with preset \"Backup\"")
        XCTAssertTrue(t.contains("More"))
    }

    func testExtractionMenuForArchives() {
        let specs = FinderMenuBuilder.build(selection: [URL(fileURLWithPath: "/Users/me/photos.tar.gz")], config: config())
        let t = titles(specs)
        XCTAssertTrue(t.contains("Extract Here (Smart)"))
        XCTAssertTrue(t.contains("Extract to \"photos/\""))
        XCTAssertTrue(t.contains("Test Archive"))
        XCTAssertFalse(t.contains("Compress with FastZip..."))
    }

    func testCascadedWrapsEverythingInOneSubmenu() {
        let specs = FinderMenuBuilder.build(selection: [URL(fileURLWithPath: "/Users/me/a.txt")], config: config(cascaded: true))
        XCTAssertEqual(specs.count, 1)
        XCTAssertEqual(specs[0].title, "Open Silicon Zip")
        XCTAssertFalse(specs[0].children.isEmpty)
    }

    func testExternalVolumesRespectSetting() {
        var c = config()
        c.accessibleFolders = ["/Volumes/USB"]
        let url = URL(fileURLWithPath: "/Volumes/USB/x.txt")
        XCTAssertTrue(FinderMenuBuilder.build(selection: [url], config: c).isEmpty)
        c.enableOnExternalVolumes = true
        XCTAssertFalse(FinderMenuBuilder.build(selection: [url], config: c).isEmpty)
    }
}
