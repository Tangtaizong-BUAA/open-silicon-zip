import XCTest
@testable import FastZipKit

final class EventParsingTests: XCTestCase {

    override func setUp() {
        super.setUp()
        L10n.forceKeys = true   // assertions are written against English
    }

    func testProgress() throws {
        let line = #"{"event":"progress","phase":"compress","current":"a/b.txt","bytes_read":100,"bytes_written":40,"bytes_total":1000,"files_done":2,"files_total":10}"#
        guard case .progress(let s)? = try EventParser.parse(line: line) else { return XCTFail("not progress") }
        XCTAssertEqual(s.phase, "compress")
        XCTAssertEqual(s.current, "a/b.txt")
        XCTAssertEqual(s.bytesRead, 100)
        XCTAssertEqual(s.bytesWritten, 40)
        XCTAssertEqual(s.bytesTotal, 1000)
        XCTAssertEqual(s.filesDone, 2)
        XCTAssertEqual(s.filesTotal, 10)
        XCTAssertEqual(s.fraction ?? 0, 0.1, accuracy: 0.0001)
    }

    func testEntry() throws {
        let line = #"{"event":"entry","name":"dir/f.txt","size":12,"csize":5,"mtime":1700000000,"method":"deflate","encrypted":true,"crc":"0xDEADBEEF","attrs":2175008768,"is_dir":false}"#
        guard case .entry(let e)? = try EventParser.parse(line: line) else { return XCTFail("not entry") }
        XCTAssertEqual(e.name, "dir/f.txt")
        XCTAssertEqual(e.size, 12)
        XCTAssertEqual(e.compressedSize, 5)
        XCTAssertEqual(e.method, "deflate")
        XCTAssertTrue(e.encrypted)
        XCTAssertEqual(e.crc, 0xDEADBEEF)
        XCTAssertEqual(e.modified, Date(timeIntervalSince1970: 1_700_000_000))
        XCTAssertFalse(e.isDirectory)
        XCTAssertEqual(e.displayName, "f.txt")
        XCTAssertEqual(e.parentPath, "dir")
    }

    func testEntryNumericCRCAndDirectoryBySuffix() throws {
        let line = #"{"event":"entry","name":"d/","size":0,"csize":0,"crc":305419896}"#
        guard case .entry(let e)? = try EventParser.parse(line: line) else { return XCTFail("not entry") }
        XCTAssertEqual(e.crc, 0x12345678)
        XCTAssertTrue(e.isDirectory)
    }

    func testDoneAndError() throws {
        XCTAssertEqual(try EventParser.parse(line: #"{"event":"done","ok":true}"#), .done(ok: true))
        XCTAssertEqual(try EventParser.parse(line: #"{"event":"done","ok":false}"#), .done(ok: false))
        XCTAssertEqual(try EventParser.parse(line: #"{"event":"error","code":"E_WRONG_PASSWORD","message":"bad"}"#),
                       .error(code: .wrongPassword, message: "bad"))
        XCTAssertEqual(try EventParser.parse(line: #"{"event":"error","code":"E_NEW_THING","message":"x"}"#),
                       .error(code: .unknown, message: "x"))
    }

    func testHashEvent() throws {
        let line = #"{"event":"hash","path":"/tmp/a","crc32":"abcd1234","sha256":"ff"}"#
        guard case .hash(let h)? = try EventParser.parse(line: line) else { return XCTFail("not hash") }
        XCTAssertEqual(h.path, "/tmp/a")
        XCTAssertEqual(h.digests["crc32"], "abcd1234")
        XCTAssertEqual(h.digests["sha256"], "ff")
    }

    func testBlankMalformedAndUnknown() throws {
        XCTAssertNil(try EventParser.parse(line: "   "))
        XCTAssertThrowsError(try EventParser.parse(line: "not json"))
        XCTAssertEqual(try EventParser.parse(line: #"{"event":"future","x":1}"#), .unknown(#"{"event":"future","x":1}"#))
    }

    func testAllWireCodesMapToKnownCases() {
        for code in ErrorCode.allCases where code != .unknown {
            XCTAssertEqual(ErrorCode(wire: code.rawValue), code)
            XCTAssertFalse(code.title.isEmpty)
            XCTAssertFalse(code.guidance.isEmpty)
        }
    }

    func testCommandArguments() {
        let create = EngineCommand.create(CreateCommand(output: "/o.zip", inputs: ["/a", "/b"], format: .sevenZ,
                                                        level: .max, password: "pw", splitBytes: 1000, threads: 4))
        XCTAssertEqual(create.arguments,
                       ["create", "/o.zip", "/a", "/b", "--format", "7z", "--level", "max",
                        "--password-stdin", "--split", "1000", "--threads", "4", "--json"])
        let extract = EngineCommand.extract(ExtractCommand(archive: "/a.zip", destination: "/d", smart: true,
                                                           password: nil, codepage: "cp936", entries: ["x/y"]))
        XCTAssertEqual(extract.arguments,
                       ["extract", "/a.zip", "-o", "/d", "--smart", "--codepage", "cp936", "--json", "--", "x/y"])
        XCTAssertEqual(EngineCommand.list(archive: "/a.zip", password: nil, codepage: "auto").arguments,
                       ["list", "/a.zip", "--json"])
        XCTAssertEqual(EngineCommand.test(archive: "/a.zip", password: nil).arguments, ["test", "/a.zip", "--json"])
        XCTAssertEqual(EngineCommand.hash(files: ["/f"]).arguments, ["hash", "/f", "--json"])
    }

    func testProgressTrackerComputesThroughputAndETA() {
        var t = ProgressTracker(startedAt: Date(timeIntervalSince1970: 0))
        var s = ProgressSnapshot()
        s.bytesTotal = 1000
        s.bytesRead = 0
        _ = t.ingest(s, at: Date(timeIntervalSince1970: 0))
        s.bytesRead = 100
        let out = t.ingest(s, at: Date(timeIntervalSince1970: 1))
        XCTAssertEqual(out.throughput, 100, accuracy: 0.001)
        XCTAssertEqual(out.eta ?? 0, 9, accuracy: 0.01)
        XCTAssertEqual(out.elapsed, 1, accuracy: 0.001)
    }

    func testDeepLinkRoundTrip() {
        let links: [DeepLink] = [
            .open(paths: ["/a b/c.zip"]),
            .extract(paths: ["/x.zip", "/y.zip"], mode: .named, deleteArchive: true),
            .compress(paths: ["/f"], format: .sevenZ, deleteSources: false, each: true, preset: "Mail", rule: "${NAME}-%Y"),
        ]
        for l in links { XCTAssertEqual(DeepLink.parse(l.url), l) }
        XCTAssertNil(DeepLink.parse(URL(string: "https://example.com")!))
    }

    func testPreferencesMergeKeepsDefaultsForMissingKeys() throws {
        let json = #"{"version":1,"general":{"escToClose":false}}"#.data(using: .utf8)!
        let p = try PreferencesStore.decode(json)
        XCTAssertFalse(p.general.escToClose)
        XCTAssertEqual(p.general.maxConcurrentJobs, 2)
        XCTAssertEqual(p.compression.format, .zip)
        let roundTrip = try PreferencesStore.decode(PreferencesStore.encode(p))
        XCTAssertEqual(roundTrip, p)
        XCTAssertThrowsError(try PreferencesStore.decode(#"{"version":99}"#.data(using: .utf8)!))
    }

    func testSplitSizeParsing() {
        XCTAssertEqual(SplitSize.parse("700"), 700 * 1_048_576)
        XCTAssertEqual(SplitSize.parse("1 GB"), 1_073_741_824)
        XCTAssertEqual(SplitSize.parse("512k"), 524_288)
        XCTAssertNil(SplitSize.parse("abc"))
        XCTAssertNotNil(SplitSize.validate(1000))
        XCTAssertNil(SplitSize.validate(100 * 1024))
    }
}
