import Foundation

/// `EngineClient` that spawns the Rust `fz` CLI with `Process` and reads
/// line-delimited JSON events from its stdout.
///
/// Passwords use an anonymous stdin pipe; they never appear in argv or environment.
public final class ProcessEngineClient: EngineClient, @unchecked Sendable {
    public let executableURL: URL
    public let environment: [String: String]?

    public init(executableURL: URL, environment: [String: String]? = nil) {
        self.executableURL = executableURL
        self.environment = environment
    }

    /// Looks for `fz`: $FZ_BINARY, the app bundle's auxiliary executables,
    /// common install prefixes, then $PATH.
    public static func locate(bundle: Bundle = .main) -> URL? {
        let fm = FileManager.default
        if let p = ProcessInfo.processInfo.environment["FZ_BINARY"], fm.isExecutableFile(atPath: p) {
            return URL(fileURLWithPath: p)
        }
        if let u = bundle.url(forAuxiliaryExecutable: "fz") { return u }
        var dirs = ["/opt/homebrew/bin", "/usr/local/bin"]
        if let path = ProcessInfo.processInfo.environment["PATH"] {
            dirs += path.split(separator: ":").map(String.init)
        }
        for d in dirs {
            let candidate = (d as NSString).appendingPathComponent("fz")
            if fm.isExecutableFile(atPath: candidate) { return URL(fileURLWithPath: candidate) }
        }
        return nil
    }

    public func start(_ command: EngineCommand) -> EngineRun {
        let control = ProcessControl()
        let exe = executableURL
        let args = command.arguments
        let input = command.passwordInput
        let env = environment

        let stream = AsyncThrowingStream<EngineEvent, Error> { continuation in
            let task = Task.detached {
                await ProcessEngineClient.run(executable: exe, arguments: args, input: input, environment: env,
                                              control: control, continuation: continuation)
            }
            continuation.onTermination = { _ in
                task.cancel()
                control.cancel()
            }
        }
        return EngineRun(events: stream,
                         pause: { control.pause() },
                         resume: { control.resume() },
                         cancel: { control.cancel() })
    }

    private static func run(executable: URL, arguments: [String], input: Data?, environment: [String: String]?,
                            control: ProcessControl,
                            continuation: AsyncThrowingStream<EngineEvent, Error>.Continuation) async {
        let process = Process()
        process.executableURL = executable
        process.arguments = arguments
        if let environment { process.environment = environment }
        let outPipe = Pipe()
        let errPipe = Pipe()
        process.standardOutput = outPipe
        process.standardError = errPipe
        let inputPipe = input == nil ? nil : Pipe()
        process.standardInput = inputPipe.map { $0 as Any } ?? FileHandle.nullDevice

        let (exitStream, exitContinuation) = AsyncStream<Int32>.makeStream()
        process.terminationHandler = { p in
            exitContinuation.yield(p.terminationStatus)
            exitContinuation.finish()
        }

        do {
            try process.run()
        } catch {
            continuation.finish(throwing: FZError(code: .engineMissing,
                                                  message: "\(executable.path): \(error.localizedDescription)"))
            return
        }
        EngineProcesses.register(process)
        defer { EngineProcesses.unregister(process) }
        control.attach(process)
        if let inputPipe, let input {
            Task.detached {
                try? inputPipe.fileHandleForWriting.write(contentsOf: input)
                try? inputPipe.fileHandleForWriting.close()
            }
        }

        let stderrLines = LockedLines()
        let errTask = Task.detached {
            do {
                for try await line in errPipe.fileHandleForReading.bytes.lines {
                    stderrLines.append(line)
                }
            } catch {}
        }

        var failure: FZError?
        var sawDone = false

        do {
            readLoop: for try await line in outPipe.fileHandleForReading.bytes.lines {
                let parsed: EngineEvent?
                do {
                    parsed = try EventParser.parse(line: line)
                } catch {
                    parsed = .log(line)      // stray non-JSON output is kept as a log line
                }
                guard let event = parsed else { continue }
                switch event {
                case .error(let code, let message):
                    failure = FZError(code: code, message: message)
                    // Keep draining stdout: stopping here can deadlock a child whose pipe fills.
                case .done:
                    sawDone = true
                    continuation.yield(event)
                default:
                    continuation.yield(event)
                }
            }
        } catch {
            failure = FZError(code: .io, message: error.localizedDescription)
        }

        var status: Int32 = 0
        for await s in exitStream { status = s }
        await errTask.value

        if control.isCancelled {
            continuation.finish(throwing: FZError(code: .cancelled, message: "Cancelled"))
        } else if let failure {
            continuation.finish(throwing: failure)
        } else if status != 0 {
            let tail = stderrLines.tail(5)
            continuation.finish(throwing: FZError(code: .engineCrashed,
                                                  message: "exit status \(status). \(tail)"))
        } else if !sawDone {
            continuation.finish(throwing: FZError(code: .engineCrashed, message: "Engine exited without a completion event."))
        } else {
            continuation.finish()
        }
    }
}

/// Thread-safe control surface for a spawned process (pause = SIGSTOP, resume = SIGCONT).
final class ProcessControl: @unchecked Sendable {
    private let lock = NSLock()
    private var process: Process?
    private var paused = false
    private var cancelled = false

    var isCancelled: Bool {
        lock.lock(); defer { lock.unlock() }
        return cancelled
    }

    func attach(_ p: Process) {
        lock.lock()
        process = p
        let shouldKill = cancelled
        lock.unlock()
        if shouldKill, p.isRunning { p.terminate() }
    }

    func pause() {
        lock.lock(); defer { lock.unlock() }
        guard let p = process, p.isRunning, !paused else { return }
        kill(p.processIdentifier, SIGSTOP)
        paused = true
    }

    func resume() {
        lock.lock(); defer { lock.unlock() }
        guard let p = process, p.isRunning, paused else { return }
        kill(p.processIdentifier, SIGCONT)
        paused = false
    }

    func cancel() {
        lock.lock()
        cancelled = true
        let p = process
        let wasPaused = paused
        paused = false
        lock.unlock()
        guard let p, p.isRunning else { return }
        if wasPaused { kill(p.processIdentifier, SIGCONT) }
        p.terminate()
    }
}

final class LockedLines: @unchecked Sendable {
    private let lock = NSLock()
    private var lines: [String] = []

    func append(_ s: String) {
        lock.lock(); defer { lock.unlock() }
        lines.append(s)
        if lines.count > 200 { lines.removeFirst(lines.count - 200) }
    }

    func tail(_ n: Int) -> String {
        lock.lock(); defer { lock.unlock() }
        return lines.suffix(n).joined(separator: " | ")
    }
}
