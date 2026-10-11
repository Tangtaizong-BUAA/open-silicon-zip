import XCTest
@testable import FastZipKit

final class RegressionTests: XCTestCase {
    func testDefaultLanguageAndPrimaryFinderActions() {
        L10n.forceKeys = true
        let prefs = Preferences()
        XCTAssertEqual(prefs.language.code, "zh-Hans")
        let cfg = FinderConfig(from: prefs, presets: [])
        let file = URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("fixture.zip")
        let menu = FinderMenuBuilder.build(selection: [file], config: cfg)
        XCTAssertTrue(menu.contains { $0.link == .extract(paths: [file.path], mode: .named, deleteArchive: false) })
    }

    func testFormatLookupPreservesPlaceholdersAndPercentSigns() {
        let prior = L10n.languageCode
        defer { L10n.languageCode = prior; L10n.forceKeys = true }
        L10n.forceKeys = false
        L10n.languageCode = "zh-Hans"
        XCTAssertEqual(L10n.t("Compress to \"%@.%@\""), "压缩为“%@.%@”")
        XCTAssertEqual(L10n.f("Compress to \"%@.%@\"", "报告", "zip"), "压缩为“报告.zip”")
        XCTAssertEqual(L10n.t("100% ready"), "100% ready")
        L10n.languageCode = "en"
        XCTAssertEqual(L10n.t("Extract Here"), "Extract Here")
    }

    func testPasswordNeverAppearsInArguments() {
        let command = EngineCommand.test(archive: "/tmp/private.zip", password: "敏感\nsecret")
        XCTAssertFalse(command.arguments.contains("敏感\nsecret"))
        XCTAssertTrue(command.arguments.contains("--password-stdin"))
        XCTAssertEqual(command.passwordInput, Data("敏感\nsecret".utf8))
    }

    func testRejectMalformedExternalURLs() {
        for value in ["fastzip://extract?path=relative", "fastzip://extract?path=/a/../b", "fastzip://user@extract?path=/a", "fastzip://extract", "fastzip://extract?path=/a%00b"] {
            XCTAssertNil(DeepLink.parse(URL(string: value)!), value)
        }
    }

    func testPausedQueueIsBusy() {
        var summary = QueueSummary(); summary.paused = 1
        XCTAssertFalse(summary.isIdle)
    }

    func testCancelAllDoesNotStartQueuedJobs() async {
        let engine = MockEngine(); engine.set("one", .hold)
        let queue = JobQueue(engine: engine, maxConcurrent: 1)
        _ = await queue.enqueueBatch([.test(archive: "/one", password: nil), .test(archive: "/two", password: nil)])
        await queue.cancelAll()
        XCTAssertEqual(engine.startedCount(), 1)
    }

    func testErrorEventFailsQueueWithoutThrownError() async {
        let engine = MockEngine(); engine.set("one", .succeed(events: [.error(code: .crcMismatch, message: "bad")]))
        let queue = JobQueue(engine: engine)
        let id = await queue.enqueue(.test(archive: "/one", password: nil))
        await queue.waitUntilIdle()
        let job = await queue.job(id)
        XCTAssertEqual(job?.state, .failed)
    }

    func testUntrustedProgressCannotOverflowFormatting() {
        XCTAssertEqual(Format.rate(.infinity), "-")
        XCTAssertEqual(Format.duration(Double.greatestFiniteMagnitude, precise: true), "-")
        XCTAssertEqual(Format.percent(.nan), "")
        XCTAssertEqual(Format.percent(1e300), "100%")
    }

    @MainActor
    func testRecoveryRejectsFalseDoneAndDoesNotBlockMainActor() async throws {
        let (dir, _) = try fakeEngine("sleep 0.2\necho '{\"event\":\"done\",\"exhausted\":true}'\nexit 7\n")
        defer { try? FileManager.default.removeItem(at: dir) }
        let runner = RecoveryRunner(executableURL: dir.appendingPathComponent("fz"))
        runner.start(archive: "/synthetic.zip", charset: ["digits"], prefix: "", minLength: 1, maxLength: 1)
        try await Task.sleep(nanoseconds: 20_000_000)
        XCTAssertEqual(runner.state, .running)
        for _ in 0..<100 {
            if case .failed = runner.state { return }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        XCTFail("Nonzero recovery exit must fail even after done")
    }

    private func fakeEngine(_ body: String) throws -> (URL, ProcessEngineClient) {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let exe = dir.appendingPathComponent("fz")
        try ("#!/bin/sh\n" + body).write(to: exe, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: exe.path)
        return (dir, ProcessEngineClient(executableURL: exe))
    }

    func testDoneThenNonzeroExitIsFailure() async throws {
        let (dir, engine) = try fakeEngine("echo '{\"event\":\"done\",\"ok\":true}'\nexit 7\n")
        defer { try? FileManager.default.removeItem(at: dir) }
        do {
            for try await _ in engine.start(.test(archive: "/a", password: nil)).events {}
            XCTFail("Nonzero exit must fail even after done")
        } catch let e as FZError { XCTAssertEqual(e.code, .engineCrashed) }
    }

    func testEmptySuccessfulProcessIsNotCompletedJob() async throws {
        let (dir, engine) = try fakeEngine("exit 0\n")
        defer { try? FileManager.default.removeItem(at: dir) }
        do {
            for try await _ in engine.start(.test(archive: "/a", password: nil)).events {}
            XCTFail("Missing completion must fail")
        } catch let e as FZError { XCTAssertEqual(e.code, .engineCrashed) }
    }
}
