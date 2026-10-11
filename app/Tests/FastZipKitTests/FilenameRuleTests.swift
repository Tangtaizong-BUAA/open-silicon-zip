import XCTest
@testable import FastZipKit

final class FilenameRuleTests: XCTestCase {
    private let utc = TimeZone(identifier: "UTC")!
    // 2026-03-07 09:05:02 UTC
    private var date: Date {
        var c = DateComponents()
        c.year = 2026; c.month = 3; c.day = 7; c.hour = 9; c.minute = 5; c.second = 2
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = utc
        return cal.date(from: c)!
    }

    func testNameToken() {
        XCTAssertEqual(FilenameRule.expand("${NAME}", name: "Report", date: date, timeZone: utc), "Report")
    }

    func testDateTokens() {
        XCTAssertEqual(FilenameRule.expand("${NAME}-%Y%m%d_%H%M%S", name: "Report", date: date, timeZone: utc),
                       "Report-20260307_090502")
    }

    func testLiteralPercentAndUnknownTokens() {
        XCTAssertEqual(FilenameRule.expand("100%%-%Q", name: "x", date: date, timeZone: utc), "100%-%Q")
        XCTAssertEqual(FilenameRule.expand("trailing%", name: "x", date: date, timeZone: utc), "trailing%")
    }

    func testSeparatorsAreSanitised() {
        XCTAssertEqual(FilenameRule.expand("a/b:%H", name: "x", date: date, timeZone: utc), "a_b_09")
    }

    func testEmptyResultFallsBackToName() {
        XCTAssertEqual(FilenameRule.expand("  ", name: "Fallback", date: date, timeZone: utc), "Fallback")
    }

    func testEnsureExtension() {
        XCTAssertEqual(FilenameRule.ensureExtension("a", "zip"), "a.zip")
        XCTAssertEqual(FilenameRule.ensureExtension("a.ZIP", "zip"), "a.ZIP")
    }

    func testUniqueName() {
        let taken: Set<String> = ["a.zip", "a 2.zip"]
        XCTAssertEqual(FilenameRule.uniqueName("a.zip", exists: { taken.contains($0) }), "a 3.zip")
        XCTAssertEqual(FilenameRule.uniqueName("b.zip", exists: { taken.contains($0) }), "b.zip")
    }
}
