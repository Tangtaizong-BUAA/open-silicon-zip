import XCTest
@testable import FastZipKit

/// Scriptable engine. Each command is answered by `script`; "gated" scripts wait for `release()`.
final class MockEngine: EngineClient, @unchecked Sendable {
    enum Script {
        case succeed(events: [EngineEvent])
        case fail(FZError)
        case hold                       // runs until cancelled or released
    }

    private let lock = NSLock()
    private var scripts: [String: Script] = [:]
    private var continuations: [String: AsyncThrowingStream<EngineEvent, Error>.Continuation] = [:]
    private(set) var started: [String] = []
    private(set) var pauseCalls = 0
    private(set) var resumeCalls = 0
    private(set) var cancelCalls = 0

    func set(_ key: String, _ script: Script) { lock.lock(); scripts[key] = script; lock.unlock() }

    func release(_ key: String) {
        lock.lock(); let c = continuations[key]; lock.unlock()
        c?.yield(.done(ok: true))
        c?.finish()
    }

    func startedCount() -> Int { lock.lock(); defer { lock.unlock() }; return started.count }

    func start(_ command: EngineCommand) -> EngineRun {
        let key = command.summary
        lock.lock()
        started.append(key)
        let script = scripts[key] ?? .succeed(events: [.done(ok: true)])
        lock.unlock()

        let stream = AsyncThrowingStream<EngineEvent, Error> { cont in
            switch script {
            case .succeed(let events):
                for e in events { cont.yield(e) }
                cont.finish()
            case .fail(let err):
                cont.finish(throwing: err)
            case .hold:
                self.lock.lock(); self.continuations[key] = cont; self.lock.unlock()
            }
        }
        return EngineRun(
            events: stream,
            pause: { [self] in lock.lock(); pauseCalls += 1; lock.unlock() },
            resume: { [self] in lock.lock(); resumeCalls += 1; lock.unlock() },
            cancel: { [self] in
                lock.lock(); cancelCalls += 1; let c = continuations[key]; lock.unlock()
                c?.finish(throwing: FZError(code: .cancelled))
            })
    }
}

final class JobQueueTests: XCTestCase {
    private func test(_ name: String) -> EngineCommand { .test(archive: "/tmp/\(name)", password: nil) }

    private func waitFor(_ queue: JobQueue, _ id: JobID, _ state: JobState, timeout: TimeInterval = 3) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if await queue.job(id)?.state == state { return }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        let actual = await queue.job(id)?.state
        XCTFail("expected \(state), got \(String(describing: actual))")
    }

    func testTransitionTable() {
        XCTAssertTrue(JobState.queued.canTransition(to: .running))
        XCTAssertTrue(JobState.running.canTransition(to: .paused))
        XCTAssertTrue(JobState.paused.canTransition(to: .running))
        XCTAssertTrue(JobState.failed.canTransition(to: .queued))
        XCTAssertFalse(JobState.success.canTransition(to: .running))
        XCTAssertFalse(JobState.queued.canTransition(to: .success))
        XCTAssertFalse(JobState.skipped.canTransition(to: .queued))
    }

    func testSuccessPath() async throws {
        let engine = MockEngine()
        var snap = ProgressSnapshot(); snap.bytesTotal = 10; snap.bytesRead = 5
        engine.set("a.zip", .succeed(events: [.progress(snap), .done(ok: true)]))
        let queue = JobQueue(engine: engine)
        let id = await queue.enqueue(test("a.zip"))
        await queue.waitUntilIdle()
        let job = await queue.job(id)
        XCTAssertEqual(job?.state, .success)
        XCTAssertEqual(job?.progress.bytesRead, 5)
        XCTAssertNil(job?.error)
        XCTAssertNotNil(job?.finishedAt)
    }

    func testFailureRecordsErrorAndBatchContinues() async throws {
        let engine = MockEngine()
        engine.set("bad.zip", .fail(FZError(code: .wrongPassword, message: "nope")))
        let queue = JobQueue(engine: engine, maxConcurrent: 1)
        let (_, ids) = await queue.enqueueBatch([test("bad.zip"), test("good.zip")])
        await queue.waitUntilIdle()
        let bad = await queue.job(ids[0])
        let good = await queue.job(ids[1])
        XCTAssertEqual(bad?.state, .failed)
        XCTAssertEqual(bad?.error?.code, .wrongPassword)
        XCTAssertEqual(good?.state, .success)
        let summary = await queue.summary()
        XCTAssertEqual(summary.failed, 1)
        XCTAssertEqual(summary.success, 1)
    }

    func testRetryFailedRequeuesAndSucceeds() async throws {
        let engine = MockEngine()
        engine.set("flaky.zip", .fail(FZError(code: .io)))
        let queue = JobQueue(engine: engine)
        let id = await queue.enqueue(test("flaky.zip"))
        await queue.waitUntilIdle()
        let failedState = await queue.job(id)?.state
        XCTAssertEqual(failedState, .failed)

        engine.set("flaky.zip", .succeed(events: [.done(ok: true)]))
        let n = await queue.retryFailed()
        XCTAssertEqual(n, 1)
        await queue.waitUntilIdle()
        let job = await queue.job(id)
        XCTAssertEqual(job?.state, .success)
        XCTAssertEqual(job?.attempt, 2)
    }

    func testConcurrencyLimitAndQueuedToRunning() async throws {
        let engine = MockEngine()
        for k in ["1", "2", "3"] { engine.set(k, .hold) }
        let queue = JobQueue(engine: engine, maxConcurrent: 2)
        let (_, ids) = await queue.enqueueBatch([test("1"), test("2"), test("3")])
        try await waitFor(queue, ids[0], .running)
        try await waitFor(queue, ids[1], .running)
        let third = await queue.job(ids[2])?.state
        XCTAssertEqual(third, .queued)

        engine.release("1")
        try await waitFor(queue, ids[0], .success)
        try await waitFor(queue, ids[2], .running)
        engine.release("2"); engine.release("3")
        await queue.waitUntilIdle()
        let s = await queue.summary()
        XCTAssertEqual(s.success, 3)
    }

    func testPauseResumeCancel() async throws {
        let engine = MockEngine()
        engine.set("p", .hold)
        let queue = JobQueue(engine: engine)
        let id = await queue.enqueue(test("p"))
        try await waitFor(queue, id, .running)

        await queue.pause(id)
        var state = await queue.job(id)?.state
        XCTAssertEqual(state, .paused)
        XCTAssertEqual(engine.pauseCalls, 1)

        await queue.resume(id)
        state = await queue.job(id)?.state
        XCTAssertEqual(state, .running)
        XCTAssertEqual(engine.resumeCalls, 1)

        await queue.cancel(id)
        state = await queue.job(id)?.state
        XCTAssertEqual(state, .cancelled)
        XCTAssertEqual(engine.cancelCalls, 1)
        await queue.waitUntilIdle()
    }

    func testCancelQueuedJobNeverStartsAndSkipWorks() async throws {
        let engine = MockEngine()
        engine.set("run", .hold)
        let queue = JobQueue(engine: engine, maxConcurrent: 1)
        let (_, ids) = await queue.enqueueBatch([test("run"), test("q1"), test("q2")])
        try await waitFor(queue, ids[0], .running)
        await queue.cancel(ids[1])
        await queue.skip(ids[2])
        let s1 = await queue.job(ids[1])?.state
        let s2 = await queue.job(ids[2])?.state
        XCTAssertEqual(s1, .cancelled)
        XCTAssertEqual(s2, .skipped)
        engine.release("run")
        await queue.waitUntilIdle()
        XCTAssertEqual(engine.startedCount(), 1)
    }

    func testPauseAllPreventsNewStartsUntilResumeAll() async throws {
        let engine = MockEngine()
        engine.set("x", .hold)
        let queue = JobQueue(engine: engine, maxConcurrent: 1)
        let id = await queue.enqueue(test("x"))
        try await waitFor(queue, id, .running)
        await queue.pauseAll()
        let second = await queue.enqueue(test("y"))
        let sState = await queue.job(second)?.state
        XCTAssertEqual(sState, .queued)
        await queue.resumeAll()
        try await waitFor(queue, id, .running)
        engine.release("x")
        await queue.waitUntilIdle()
        let final = await queue.job(second)?.state
        XCTAssertEqual(final, .success)
    }

    func testDoneNotOkFailsJob() async throws {
        let engine = MockEngine()
        engine.set("n", .succeed(events: [.done(ok: false)]))
        let queue = JobQueue(engine: engine)
        let id = await queue.enqueue(test("n"))
        await queue.waitUntilIdle()
        let state = await queue.job(id)?.state
        XCTAssertEqual(state, .failed)
    }

    func testEntriesAreCollectedForListJobs() async throws {
        let engine = MockEngine()
        engine.set("l.zip", .succeed(events: [.entry(ArchiveEntry(name: "a")), .entry(ArchiveEntry(name: "b")), .done(ok: true)]))
        let queue = JobQueue(engine: engine)
        let id = await queue.enqueue(.list(archive: "/tmp/l.zip", password: nil, codepage: nil))
        await queue.waitUntilIdle()
        let count = await queue.job(id)?.entries.count
        XCTAssertEqual(count, 2)
    }
}
