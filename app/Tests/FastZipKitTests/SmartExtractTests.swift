import XCTest
@testable import FastZipKit

final class SmartExtractTests: XCTestCase {
    private func p(_ paths: [String], name: String = "Photos.zip", wrap: Bool = false) -> SmartExtractPlan {
        SmartExtract.plan(inputs: paths.map {
            PlanInput(path: $0, isDirectory: $0.hasSuffix("/") || $0.hasSuffix("\\"))
        }, archiveName: name, wrapSingleFiles: wrap)
    }

    // 1. single root
    func testSingleRootFolderIsStripped() {
        let plan = p(["Proj/", "Proj/a.txt", "Proj/sub/b.txt"])
        XCTAssertEqual(plan.kind, .stripSingleRoot("Proj"))
        XCTAssertNil(plan.wrapperFolder)
        XCTAssertEqual(plan.stripComponents, 1)
        XCTAssertEqual(Set(plan.collisionCandidates), ["a.txt", "sub"])
    }

    func testImpliedRootWithoutExplicitDirEntry() {
        XCTAssertEqual(p(["Proj/a.txt"]).kind, .stripSingleRoot("Proj"))
    }

    // 2. single file
    func testSingleFileExtractsDirectly() {
        let plan = p(["readme.txt"])
        XCTAssertEqual(plan.kind, .singleFile("readme.txt"))
        XCTAssertNil(plan.wrapperFolder)
        XCTAssertEqual(plan.stripComponents, 0)
    }

    func testSingleFileWrappedWhenSettingOn() {
        let plan = p(["readme.txt"], wrap: true)
        XCTAssertEqual(plan.kind, .wrapped("Photos"))
        XCTAssertEqual(plan.wrapperFolder, "Photos")
    }

    // 3. multiple roots
    func testMultiRootWrapsInArchiveName() {
        let plan = p(["a.txt", "b/", "b/c.txt"])
        XCTAssertEqual(plan.kind, .wrapped("Photos"))
        XCTAssertEqual(plan.wrapperFolder, "Photos")
        XCTAssertEqual(plan.stripComponents, 0)
    }

    // 4. root + stray
    func testRootPlusStrayFileIsMultiRoot() {
        let plan = p(["Proj/", "Proj/a.txt", "stray.txt"])
        XCTAssertEqual(plan.kind, .wrapped("Photos"))
    }

    // 5. empty
    func testEmptyArchive() {
        let plan = p([])
        XCTAssertEqual(plan.kind, .nothing)
        XCTAssertTrue(plan.collisionCandidates.isEmpty)
    }

    // 6. ./ prefixed
    func testDotSlashPrefixIsNormalised() {
        let plan = p(["./Proj/", "./Proj/a.txt"])
        XCTAssertEqual(plan.kind, .stripSingleRoot("Proj"))
    }

    // 7. backslash separators
    func testBackslashPathsAreNormalised() {
        let plan = p(["Proj\\", "Proj\\a.txt", "Proj\\sub\\b.txt"])
        XCTAssertEqual(plan.kind, .stripSingleRoot("Proj"))
        XCTAssertEqual(Set(plan.collisionCandidates), ["a.txt", "sub"])
    }

    // 8. CJK names, plus NFC/NFD and case folding
    func testCJKAndUnicodeNormalisation() {
        XCTAssertEqual(p(["\u{6587}\u{4EF6}/", "\u{6587}\u{4EF6}/\u{4F60}\u{597D}.txt"]).kind,
                       .stripSingleRoot("\u{6587}\u{4EF6}"))
        // "é" precomposed vs decomposed + case difference are the same root
        let plan = p(["Caf\u{E9}/a", "cafe\u{301}/b"])
        XCTAssertEqual(plan.kind, .stripSingleRoot("Caf\u{E9}"))
    }

    // 9. symlinked root is a file, not a directory
    func testSymlinkedRootCountsAsSingleFile() {
        let plan = SmartExtract.plan(inputs: [PlanInput(path: "current", isDirectory: true, isSymlink: true)],
                                     archiveName: "rel.tar.gz")
        XCTAssertEqual(plan.kind, .singleFile("current"))
    }

    // 10. huge depth
    func testVeryDeepPathIsLinear() {
        let deep = (0..<2000).map { "d\($0)" }.joined(separator: "/") + "/f.txt"
        let plan = p(["root/" + deep, "root/"])
        XCTAssertEqual(plan.kind, .stripSingleRoot("root"))
        XCTAssertEqual(plan.collisionCandidates, ["d0"])
    }

    // 11. name collision at the destination
    func testCollisionDetectionIsCaseInsensitiveAndNeverMerges() {
        let wrapped = p(["a", "b"])
        XCTAssertEqual(SmartExtract.collisions(for: wrapped, existingNames: ["photos", "x"]), ["Photos"])
        XCTAssertEqual(SmartExtract.collisions(for: wrapped, existingNames: ["photos"], caseInsensitive: false), [])
        let stripped = p(["Proj/a.txt", "Proj/b.txt"])
        XCTAssertEqual(SmartExtract.collisions(for: stripped, existingNames: ["a.txt"]), ["a.txt"])
    }

    // 12. read-only destination
    func testReadOnlyDestinationThrowsPermissionDenied() {
        XCTAssertThrowsError(try SmartExtract.validateDestination("/ro", isWritable: { _ in false })) { error in
            XCTAssertEqual((error as? FZError)?.code, .permissionDenied)
        }
        XCTAssertNoThrow(try SmartExtract.validateDestination("/rw", isWritable: { _ in true }))
    }

    // Extras: junk filtering and unsafe entries
    func testMacOSJunkIsIgnoredSoSingleRootStillDetected() {
        let plan = p(["Proj/a.txt", "__MACOSX/Proj/._a.txt", ".DS_Store", "Proj/._b"])
        XCTAssertEqual(plan.kind, .stripSingleRoot("Proj"))
    }

    func testUnsafeEntriesAreReported() {
        let plan = p(["Proj/a", "Proj/../../evil", "/abs/path", "C:\\win"])
        XCTAssertEqual(plan.unsafeEntries.count, 3)
    }

    func testArchiveBaseName() {
        XCTAssertEqual(SmartExtract.archiveBaseName("Photos.zip"), "Photos")
        XCTAssertEqual(SmartExtract.archiveBaseName("rel.tar.gz"), "rel")
        XCTAssertEqual(SmartExtract.archiveBaseName("big.7z.001"), "big")
        XCTAssertEqual(SmartExtract.archiveBaseName("old.z01"), "old")
        XCTAssertEqual(SmartExtract.archiveBaseName("c.part1.rar"), "c")
        XCTAssertEqual(SmartExtract.archiveBaseName("noext"), "noext")
    }
}
