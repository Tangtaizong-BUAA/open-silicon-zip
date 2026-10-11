import Foundation

/// Tracks only children launched by this app, including recovery outside the job queue.
public enum EngineProcesses {
    private static let lock = NSLock()
    private static var processes: [ObjectIdentifier: Process] = [:]
    private static var shuttingDown = false

    static func register(_ process: Process) {
        lock.lock()
        processes[ObjectIdentifier(process)] = process
        let stop = shuttingDown
        lock.unlock()
        if stop { terminate(process) }
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
    }

    private static func terminate(_ process: Process) {
        guard process.isRunning else { return }
        // SIGCONT is harmless for a running child and releases a paused child to handle SIGTERM.
        _ = process.resume()
        process.terminate()
    }
}
