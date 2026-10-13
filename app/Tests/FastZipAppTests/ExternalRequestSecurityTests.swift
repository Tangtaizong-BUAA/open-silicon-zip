import AppKit
import XCTest
import FastZipKit
@testable import FastZipApp

private final class RecordingEngine: EngineClient, @unchecked Sendable {
    private let lock = NSLock()
    private var commands: [EngineCommand] = []
    var failExtraction = false
    func recorded() -> [EngineCommand] { lock.lock(); defer { lock.unlock() }; return commands }

    func start(_ command: EngineCommand) -> EngineRun {
        lock.lock(); commands.append(command); lock.unlock()
        let stream = AsyncThrowingStream<EngineEvent, Error> { continuation in
            if case .extract(let request) = command {
                let file = URL(fileURLWithPath: request.destination).appendingPathComponent("file.txt")
                try? Data("decrypted secret".utf8).write(to: file)
            }
            if failExtraction { continuation.finish(throwing: FZError(code: .crcMismatch)) }
            else { continuation.yield(.done(ok: true)); continuation.finish() }
        }
        return EngineRun(events: stream, pause: {}, resume: {}, cancel: {})
    }
}

final class ExternalRequestSecurityTests: XCTestCase {
    @MainActor
    func testRejectedExternalRequestStartsNoReadOrWrite() async {
        _ = NSApplication.shared
        let engine = RecordingEngine()
        var prefs = Preferences()
        prefs.sandbox.accessibleFolders = ["/synthetic"]
        let app = AppModel(engine: engine, preferences: prefs, observe: false)
        var approvals = 0
        app.approveExternalRequest = { _ in approvals += 1; return false }
        app.handle(.test(paths: ["/synthetic/private.zip"]))
        app.handle(.extract(paths: ["/synthetic/private.zip"], mode: .named, deleteArchive: false))
        XCTAssertEqual(approvals, 2)
        await Task.yield()
        XCTAssertTrue(engine.recorded().isEmpty)
        let asked = expectation(description: "Compression must request approval")
        app.approveExternalRequest = { review in
            XCTAssertEqual(review.paths, ["/synthetic/first", "/synthetic/secret"])
            XCTAssertFalse(review.deletesOriginals)
            asked.fulfill()
            return false
        }
        app.handle(.compress(paths: ["/synthetic/first", "/synthetic/secret"], format: .zip,
                             deleteSources: false, each: false, preset: nil, rule: nil))
        await fulfillment(of: [asked], timeout: 3)
        XCTAssertTrue(engine.recorded().isEmpty)
        app.cleanUpTemporaryFiles()
    }

    @MainActor
    func testFailedAndReleasedPreviewsRemovePlaintext() async throws {
        let engine = RecordingEngine()
        let app = AppModel(engine: engine, preferences: Preferences(), observe: false)
        let archive = URL(fileURLWithPath: "/synthetic/private.zip")
        engine.failExtraction = true
        let failed = await app.extractToTemp(archive: archive, entry: ArchiveEntry(name: "file.txt"), password: nil, codepage: nil)
        XCTAssertNil(failed)
        guard case .extract(let request)? = engine.recorded().last else { return XCTFail("No extraction") }
        XCTAssertFalse(FileManager.default.fileExists(atPath: request.destination))
        engine.failExtraction = false
        let output = await app.extractToTemp(archive: archive, entry: ArchiveEntry(name: "file.txt"), password: nil, codepage: nil)
        let file = try XCTUnwrap(output)
        XCTAssertTrue(FileManager.default.fileExists(atPath: file.path))
        app.releaseTemporaryFile(file)
        XCTAssertFalse(FileManager.default.fileExists(atPath: file.deletingLastPathComponent().path))
        app.cleanUpTemporaryFiles()
    }
}
