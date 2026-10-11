import Foundation

public typealias JobID = UUID

/// One queue item (FR-402). A batch is simply several jobs sharing a `batchID`.
public struct Job: Identifiable, Sendable, Equatable {
    public let id: JobID
    public var title: String
    public var kind: JobKind
    public var command: EngineCommand
    public var batchID: UUID?
    public var state: JobState = .queued
    public var progress = ProgressSnapshot()
    public var error: FZError?
    public var entries: [ArchiveEntry] = []
    public var hashes: [HashResult] = []
    public var log: [String] = []
    public var attempt: Int = 1
    public let createdAt: Date
    public var startedAt: Date?
    public var finishedAt: Date?

    public init(id: JobID = UUID(), title: String, command: EngineCommand, batchID: UUID? = nil, createdAt: Date = Date()) {
        self.id = id
        self.title = title
        self.kind = command.kind
        self.command = command
        self.batchID = batchID
        self.createdAt = createdAt
    }
}

public struct QueueSummary: Sendable, Equatable {
    public var total = 0
    public var queued = 0
    public var running = 0
    public var paused = 0
    public var success = 0
    public var failed = 0
    public var skipped = 0
    public var cancelled = 0
    public var isIdle: Bool { queued == 0 && running == 0 && paused == 0 }
    public init() {}
}

/// Concurrent job scheduler. All mutation happens on the actor; the UI observes
/// immutable `[Job]` snapshots through `updates()`.
public actor JobQueue {
    private let engine: EngineClient
    private let clock: @Sendable () -> Date
    public private(set) var maxConcurrent: Int

    private var jobs: [JobID: Job] = [:]
    private var order: [JobID] = []
    private var runs: [JobID: EngineRun] = [:]
    private var trackers: [JobID: ProgressTracker] = [:]
    private var globallyPaused = false
    private var observers: [UUID: AsyncStream<[Job]>.Continuation] = [:]
    private var idleWaiters: [CheckedContinuation<Void, Never>] = []

    public init(engine: EngineClient, maxConcurrent: Int = 2, clock: @escaping @Sendable () -> Date = { Date() }) {
        self.engine = engine
        self.maxConcurrent = max(1, maxConcurrent)
        self.clock = clock
    }

    // MARK: Enqueue

    @discardableResult
    public func enqueue(_ command: EngineCommand, title: String? = nil, batchID: UUID? = nil) -> JobID {
        let job = Job(title: title ?? command.summary, command: command, batchID: batchID, createdAt: clock())
        jobs[job.id] = job
        order.append(job.id)
        publish()
        pump()
        return job.id
    }

    @discardableResult
    public func enqueueBatch(_ commands: [EngineCommand]) -> (batch: UUID, ids: [JobID]) {
        let batch = UUID()
        var ids: [JobID] = []
        for c in commands {
            let job = Job(title: c.summary, command: c, batchID: batch, createdAt: clock())
            jobs[job.id] = job
            order.append(job.id)
            ids.append(job.id)
        }
        publish()
        pump()
        return (batch, ids)
    }

    public func setMaxConcurrent(_ n: Int) {
        maxConcurrent = max(1, n)
        pump()
    }

    // MARK: Control

    public func pause(_ id: JobID) {
        guard let job = jobs[id], job.state == .running else { return }
        runs[id]?.pause()
        let now = clock()
        trackers[id]?.markPaused(at: now)
        _ = transition(id, to: .paused)
    }

    public func resume(_ id: JobID) {
        guard let job = jobs[id], job.state == .paused else { return }
        runs[id]?.resume()
        trackers[id]?.markResumed(at: clock())
        _ = transition(id, to: .running)
    }

    public func cancel(_ id: JobID) {
        guard let job = jobs[id], !job.state.isTerminal else { return }
        if let run = runs[id] {
            if job.state == .paused { run.resume() }
            run.cancel()
        }
        runs[id] = nil
        trackers[id] = nil
        jobs[id]?.error = FZError(code: .cancelled, message: "Cancelled")
        _ = transition(id, to: .cancelled)
        pump()
    }

    /// Queued items can be skipped (e.g. from the failure summary).
    public func skip(_ id: JobID) {
        guard jobs[id]?.state == .queued else { return }
        _ = transition(id, to: .skipped)
        pump()
    }

    public func pauseAll() {
        globallyPaused = true
        for id in order where jobs[id]?.state == .running { pause(id) }
        publish()
    }

    public func resumeAll() {
        globallyPaused = false
        for id in order where jobs[id]?.state == .paused { resume(id) }
        pump()
    }

    public func cancelAll() {
        let wasPaused = globallyPaused
        globallyPaused = true
        defer { globallyPaused = wasPaused; publish() }
        for id in order where !(jobs[id]?.state.isTerminal ?? true) { cancel(id) }
    }

    /// FR-404: re-queues every failed job (optionally only one batch). Returns the count.
    @discardableResult
    public func retryFailed(batchID: UUID? = nil) -> Int {
        var n = 0
        for id in order {
            guard let job = jobs[id], job.state == .failed else { continue }
            if let b = batchID, job.batchID != b { continue }
            jobs[id]?.error = nil
            jobs[id]?.progress = ProgressSnapshot()
            jobs[id]?.attempt += 1
            jobs[id]?.startedAt = nil
            jobs[id]?.finishedAt = nil
            if transition(id, to: .queued, publishNow: false) { n += 1 }
        }
        publish()
        pump()
        return n
    }

    /// Retry one job (failed or cancelled), e.g. after supplying a password.
    public func retry(_ id: JobID, replacing command: EngineCommand? = nil) {
        guard let job = jobs[id], job.state == .failed || job.state == .cancelled else { return }
        if let command { jobs[id]?.command = command }
        jobs[id]?.error = nil
        jobs[id]?.progress = ProgressSnapshot()
        jobs[id]?.attempt += 1
        _ = transition(id, to: .queued)
        pump()
    }

    public func clearFinished() {
        let finished = order.filter { jobs[$0]?.state.isTerminal ?? false }
        for id in finished { jobs[id] = nil }
        order.removeAll { jobs[$0] == nil }
        publish()
    }

    // MARK: Queries

    public func snapshot() -> [Job] { order.compactMap { jobs[$0] } }

    public func job(_ id: JobID) -> Job? { jobs[id] }

    public func summary() -> QueueSummary {
        var s = QueueSummary()
        for j in jobs.values {
            s.total += 1
            switch j.state {
            case .queued: s.queued += 1
            case .running: s.running += 1
            case .paused: s.paused += 1
            case .success: s.success += 1
            case .failed: s.failed += 1
            case .skipped: s.skipped += 1
            case .cancelled: s.cancelled += 1
            }
        }
        return s
    }

    /// Sum of smoothed per-job throughput of running jobs, bytes/s.
    public func aggregateThroughput() -> Double {
        jobs.values.filter { $0.state == .running }.reduce(0) { $0 + $1.progress.throughput }
    }

    /// Emits the full job list now and after every change.
    public func updates() -> AsyncStream<[Job]> {
        let (stream, continuation) = AsyncStream<[Job]>.makeStream(bufferingPolicy: .bufferingNewest(1))
        let token = UUID()
        observers[token] = continuation
        continuation.yield(snapshot())
        continuation.onTermination = { [weak self] _ in
            Task { await self?.removeObserver(token) }
        }
        return stream
    }

    /// Suspends until nothing is queued, running or paused.
    public func waitUntilIdle() async {
        if isIdle() { return }
        await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
            idleWaiters.append(c)
        }
    }

    // MARK: Internals

    private func removeObserver(_ token: UUID) { observers[token] = nil }

    private func isIdle() -> Bool {
        !jobs.values.contains { $0.state == .queued || $0.state == .running || $0.state == .paused }
    }

    private func publish() {
        let snap = snapshot()
        for c in observers.values { c.yield(snap) }
        if isIdle() && !idleWaiters.isEmpty {
            let waiters = idleWaiters
            idleWaiters.removeAll()
            for w in waiters { w.resume() }
        }
    }

    @discardableResult
    private func transition(_ id: JobID, to next: JobState, publishNow: Bool = true) -> Bool {
        guard var job = jobs[id], job.state.canTransition(to: next) else { return false }
        job.state = next
        if next == .running, job.startedAt == nil { job.startedAt = clock() }
        if next.isTerminal { job.finishedAt = clock() }
        jobs[id] = job
        if publishNow { publish() }
        return true
    }

    private func pump() {
        guard !globallyPaused else { publish(); return }
        while runs.count < maxConcurrent,
              let next = order.first(where: { jobs[$0]?.state == .queued }) {
            start(next)
        }
        publish()
    }

    private func start(_ id: JobID) {
        guard let job = jobs[id], transition(id, to: .running, publishNow: false) else { return }
        trackers[id] = ProgressTracker(startedAt: clock())
        let run = engine.start(job.command)
        runs[id] = run
        Task { await self.consume(id: id, attempt: job.attempt, run: run) }
    }

    private func consume(id: JobID, attempt: Int, run: EngineRun) async {
        var failure: FZError?
        var doneOK = true
        do {
            for try await event in run.events {
                guard jobs[id]?.attempt == attempt, jobs[id]?.state.isTerminal == false else { continue }
                if case .error(let code, let message) = event { failure = FZError(code: code, message: message) }
                if case .done(let ok) = event { doneOK = ok }
                apply(event, to: id)
            }
        } catch let e as FZError {
            failure = e
        } catch {
            failure = FZError(code: .io, message: error.localizedDescription)
        }

        // A cancel() may already have moved the job to a terminal state.
        guard let job = jobs[id], job.attempt == attempt, job.state == .running || job.state == .paused else {
            return
        }
        runs[id] = nil
        trackers[id] = nil

        if let failure {
            jobs[id]?.error = failure
            if failure.code == .cancelled {
                _ = transition(id, to: .cancelled, publishNow: false)
            } else {
                _ = transition(id, to: .failed, publishNow: false)
            }
        } else if !doneOK {
            jobs[id]?.error = FZError(code: .io, message: "The engine reported a failure.")
            _ = transition(id, to: .failed, publishNow: false)
        } else {
            if job.state == .paused { jobs[id]?.state = .running }   // stream ended while paused
            _ = transition(id, to: .success, publishNow: false)
        }
        pump()
    }

    private func apply(_ event: EngineEvent, to id: JobID) {
        guard jobs[id] != nil else { return }
        switch event {
        case .progress(let snap):
            if var tracker = trackers[id] {
                jobs[id]?.progress = tracker.ingest(snap, at: clock())
                trackers[id] = tracker
            } else {
                jobs[id]?.progress = snap
            }
        case .entry(let e):
            jobs[id]?.entries.append(e)
            return      // no per-entry publish: listings can have 100k+ entries
        case .hash(let h):
            jobs[id]?.hashes.append(h)
        case .log(let line):
            jobs[id]?.log.append(line)
        case .unknown(let line):
            jobs[id]?.log.append(line)
        case .done, .error:
            break
        }
        publish()
    }
}
