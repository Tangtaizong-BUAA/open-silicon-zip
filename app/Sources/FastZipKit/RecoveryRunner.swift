import Foundation

/// Drives `fz recover --json` for the recovery wizard (FR-703/705).
///
/// Deliberately not routed through the archive job queue: recovery runs for
/// minutes-to-hours with its own pause/resume (SIGSTOP/SIGCONT, mirroring
/// `ProcessEngineClient`) and reports domain stats (tested / per-second / ETA)
/// that the shared `ProgressSnapshot` does not model.
@MainActor
public final class RecoveryRunner: ObservableObject {
    public struct Stats: Sendable, Equatable {
        public var tested: Double
        public var perSecond: Double
        public var elapsed: Double
        public var eta: Double
    }

    public enum RunState: Equatable {
        case idle
        case running
        case paused
        case finished(password: String)
        case exhausted
        case cancelled(checkpoint: String?)
        case failed(message: String)

        public var isTerminal: Bool {
            switch self {
            case .idle, .running, .paused: return false
            default: return true
            }
        }
    }

    @Published public private(set) var state: RunState = .idle
    @Published public private(set) var stats: Stats?

    private var process: Process?
    private let executableURL: URL
    private var stdoutPipe: Pipe?

    public init(executableURL: URL) {
        self.executableURL = executableURL
    }

    /// Charset segments as understood by `fz recover --charset` (comma separated).
    public static func charsetArguments(digits: Bool, lower: Bool, upper: Bool, special: Bool, space: Bool, custom: String) -> [String] {
        var parts: [String] = []
        if digits { parts.append("digits") }
        if lower { parts.append("lower") }
        if upper { parts.append("upper") }
        if special { parts.append("special") }
        if space { parts.append("space") }
        let trimmed = custom.trimmingCharacters(in: .whitespaces)
        if !trimmed.isEmpty {
            if parts.isEmpty { parts.append("all") }
            parts.append(trimmed)
        }
        return parts.isEmpty ? ["digits"] : parts
    }

    public func start(archive: String, charset: [String], prefix: String, minLength: Int, maxLength: Int) {
        guard process == nil else { return }
        var args = ["recover", archive, "--charset", charset.joined(separator: ","), "--json"]
        if !prefix.isEmpty { args += ["--prefix", prefix] }
        args += ["--min", String(minLength), "--max", String(maxLength)]

        let p = Process()
        p.executableURL = executableURL
        p.arguments = args
        let pipe = Pipe()
        p.standardOutput = pipe
        p.standardError = FileHandle.nullDevice
        // Terminate the child when FastZip quits mid-run.
        p.qualityOfService = .userInitiated
        do { try EngineProcesses.launch(p) } catch {
            state = .failed(message: error.localizedDescription)
            return
        }
        process = p
        stdoutPipe = pipe
        state = .running
        stats = nil
        Task { [weak self] in await self?.drain(pipe.fileHandleForReading) }
    }

    public func pause() {
        guard state == .running, let p = process, p.isRunning else { return }
        p.suspend()
        state = .paused
    }

    public func resume() {
        guard state == .paused, let p = process, p.isRunning else { return }
        p.resume()
        state = .running
    }

    public func cancel() {
        guard let p = process else { return }
        if state == .paused && p.isRunning { p.resume() }
        p.terminate()
    }

    /// Reads line-delimited JSON until EOF, updating `stats` on progress events
    /// and settling `state` from the final `done` line (or a non-zero exit).
    private func drain(_ handle: FileHandle) async {
        var sawDone = false
        do {
            for try await line in handle.bytes.lines {
                guard let data = line.data(using: .utf8),
                      let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { continue }
                if obj["event"] as? String == "progress" {
                    stats = Stats(tested: obj["tested"] as? Double ?? 0,
                                  perSecond: obj["per_second"] as? Double ?? 0,
                                  elapsed: obj["elapsed"] as? Double ?? 0,
                                  eta: obj["eta"] as? Double ?? 0)
                } else if obj["event"] as? String == "done" {
                    sawDone = true
                    if let pw = obj["password"] as? String {
                        state = .finished(password: pw)
                    } else if (obj["exhausted"] as? Bool) == true {
                        state = .exhausted
                    } else if let cp = obj["checkpoint"] as? String {
                        state = .cancelled(checkpoint: cp)
                    } else {
                        state = .exhausted
                    }
                }
            }
        }
        catch { state = .failed(message: error.localizedDescription) }
        // EOF can precede process termination. Await off the main actor.
        if let child = process { await Task.detached { child.waitUntilExit() }.value }
        if sawDone, let child = process, child.terminationStatus != 0 {
            state = .failed(message: "engine exited with status \(child.terminationStatus)")
        }
        if !sawDone {
            switch state {
            case .running, .paused:
                let code = process?.terminationStatus ?? -1
                state = code == 15 || code == -15 || process?.terminationReason == .uncaughtSignal
                    ? .cancelled(checkpoint: nil)
                    : .failed(message: "engine exited with status \(code)")
            default: break
            }
        }
        if let child = process { EngineProcesses.unregister(child) }
        process = nil
        stdoutPipe = nil
    }
}
