import XCTest
@testable import FastZipKit

final class ExtractionFolderTests: XCTestCase {
    func testReservesDistinctDirectoriesWithoutMergingOrFollowingLinks() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createSymbolicLink(at: root.appendingPathComponent("photos"), withDestinationURL: root.appendingPathComponent("missing"))
        try Data("keep".utf8).write(to: root.appendingPathComponent("photos (2)"))
        let first = try ExtractionFolder.reserve(in: root, archiveName: "photos.tar.gz")
        let second = try ExtractionFolder.reserve(in: root, archiveName: "photos.zip")
        XCTAssertEqual(first.lastPathComponent, "photos (3)")
        XCTAssertEqual(second.lastPathComponent, "photos (4)")
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: first.path), [])
        XCTAssertEqual(try String(contentsOf: root.appendingPathComponent("photos (2)"), encoding: .utf8), "keep")
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent("missing").path))
    }

    func testDotArchiveCannotSelectParentDirectory() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let folder = try ExtractionFolder.reserve(in: root, archiveName: "..")
        XCTAssertEqual(folder.lastPathComponent, "Archive")
        XCTAssertEqual(folder.deletingLastPathComponent().standardizedFileURL, root.standardizedFileURL)
    }
}
