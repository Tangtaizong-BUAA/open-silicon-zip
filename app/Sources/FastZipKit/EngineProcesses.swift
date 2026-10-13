import Foundation

/// Tracks only children launched by this app, including recovery outside the job queue.
public enum EngineProcesses {
    private static let lock = NSLock()
    private static var processes: [ObjectIdentifier: Process] = [:]
    private static var shuttingDown = false

    static func launch(_ process: Process) throws {
        lock.lock(); defer { lock.unlock() }
        guard !shuttingDown else { throw FZError(code: .cancelled) }
        try process.run()
        processes[ObjectIdentifier(process)] = process
    }

    static func unregister(_ process: Process) {
        lock.lock(); defer { lock.unlock() }
        processes.removeValue(forKey: ObjectIdentifier(process))
    }

    public static var isBusy: Bool {
        lock.lock(); defer { lock.unlock() }
        return processes.values.contains { $0.isRunning }
    }

    /// Prevents late-starting jobs from surviving the app's termination as well.
    public static func shutDown() {
        lock.lock()
        shuttingDown = true
        let children = Array(processes.values)
        lock.unlock()
        children.forEach(terminate)
        let deadline = Date().addingTimeInterval(2)
        while children.contains(where: \.isRunning) && Date() < deadline { Thread.sleep(forTimeInterval: 0.01) }
        for child in children where child.isRunning { kill(child.processIdentifier, SIGKILL) }
        for child in children where child.isRunning { child.waitUntilExit() }
    }

    private static func terminate(_ process: Process) {
        guard process.isRunning else { return }
        // SIGCONT is harmless for a running child and releases a paused child to handle SIGTERM.
        _ = process.resume()
        process.terminate()
    }
}
