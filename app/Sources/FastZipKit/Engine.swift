import Foundation

// MARK: - Commands

public struct CreateCommand: Sendable, Equatable {
    public var output: String
    public var inputs: [String]
    public var format: ArchiveFormat
    public var level: CompressionLevel
    public var password: String?
    public var splitBytes: UInt64?
    public var threads: Int?
    public var excludeGlobs: [String] = []
    public init(output: String, inputs: [String], format: ArchiveFormat = .zip,
                level: CompressionLevel = .normal, password: String? = nil,
                splitBytes: UInt64? = nil, threads: Int? = nil) {
        self.output = output
        self.inputs = inputs
        self.format = format
        self.level = level
        self.password = password
        self.splitBytes = splitBytes
        self.threads = threads
    }
}

public struct ExtractCommand: Sendable, Equatable {
    public var archive: String
    public var destination: String
    public var smart: Bool
    public var password: String?
    public var codepage: String?
    public var entries: [String]
    /// PROPOSED CLI EXTENSION (not in the base contract): `--overwrite skip|overwrite|rename`.
    public var overwrite: ConflictPolicy?
    public init(archive: String, destination: String, smart: Bool = false, password: String? = nil,
                codepage: String? = nil, entries: [String] = [], overwrite: ConflictPolicy? = nil) {
        self.archive = archive
        self.destination = destination
        self.smart = smart
        self.password = password
        self.codepage = codepage
        self.entries = entries
        self.overwrite = overwrite
    }
}

/// One engine invocation. `arguments` is exactly what is passed to `fz`.
public enum EngineCommand: Sendable, Equatable {
    case create(CreateCommand)
    case extract(ExtractCommand)
    /// `--password` / `--codepage` on `list` are PROPOSED extensions (needed for FR-607 and FR-803).
    case list(archive: String, password: String?, codepage: String?)
    case test(archive: String, password: String?)
    case hash(files: [String])
    /// PROPOSED: `fz repair <archive> -o <out> --json` (not in the base contract).
    case repair(archive: String, output: String)
    case edit(archive: String, delete: [String], rename: [String], add: [String])

    public var kind: JobKind {
        switch self {
        case .create: return .create
        case .extract: return .extract
        case .list: return .list
        case .test: return .test
        case .hash: return .hash
        case .repair: return .repair
        case .edit: return .edit
        }
    }

    public var passwordInput: Data? {
        let password: String?
        switch self {
        case .create(let c): password = c.password
        case .extract(let c): password = c.password
        case .list(_, let p, _), .test(_, let p): password = p
        default: password = nil
        }
        guard let password, !password.isEmpty else { return nil }
        return Data(password.utf8)
    }

    public var arguments: [String] {
        switch self {
        case .create(let c):
            var a = ["create", c.output] + c.inputs
            a += ["--format", c.format.cliName, "--level", c.level.cliName]
            if let p = c.password, !p.isEmpty { a += ["--password-stdin"] }
            if let s = c.splitBytes { a += ["--split", String(s)] }
            if let t = c.threads { a += ["--threads", String(t)] }
            for pattern in c.excludeGlobs { a += ["--exclude", pattern] }
            a.append("--json")
            return a
        case .extract(let e):
            var a = ["extract", e.archive, "-o", e.destination]
            if e.smart { a.append("--smart") }
            if let p = e.password, !p.isEmpty { a += ["--password-stdin"] }
            if let cp = e.codepage, cp != "auto" { a += ["--codepage", cp] }
            if let o = e.overwrite, o != .ask { a += ["--overwrite", o.rawValue] }
            a.append("--json")
            if !e.entries.isEmpty { a.append("--"); a += e.entries }
            return a
        case .list(let archive, let password, let codepage):
            var a = ["list", archive]
            if let p = password, !p.isEmpty { a += ["--password-stdin"] }
            if let cp = codepage, cp != "auto" { a += ["--codepage", cp] }
            a.append("--json")
            return a
        case .test(let archive, let password):
            var a = ["test", archive]
            if let p = password, !p.isEmpty { a += ["--password-stdin"] }
            a.append("--json")
            return a
        case .hash(let files):
            return ["hash"] + files + ["--json"]
        case .edit(let archive, let delete, let rename, let add):
            return ["edit", archive, "--json"] + delete.flatMap { ["--delete", $0] }
                + rename.flatMap { ["--rename", $0] } + add.flatMap { ["--add", $0] }
        case .repair(let archive, let output):
            return ["repair", archive, "-o", output, "--json"]
        }
    }

    /// Short human description for queue rows.
    public var summary: String {
        switch self {
        case .create(let c): return URL(fileURLWithPath: c.output).lastPathComponent
        case .extract(let e): return URL(fileURLWithPath: e.archive).lastPathComponent
        case .list(let a, _, _), .test(let a, _), .repair(let a, _), .edit(let a, _, _, _):
            return URL(fileURLWithPath: a).lastPathComponent
        case .hash(let f): return f.count == 1 ? URL(fileURLWithPath: f[0]).lastPathComponent : "\(f.count) files"
        }
    }
}

// MARK: - Events

public enum EngineEvent: Sendable, Equatable {
    case progress(ProgressSnapshot)
    case entry(ArchiveEntry)
    case hash(HashResult)
    case done(ok: Bool)
    case error(code: ErrorCode, message: String)
    case log(String)
    case unknown(String)
}

public enum EventParseError: Error, Equatable {
    case malformed(String)
}

/// Parses the line-delimited JSON protocol of `fz --json`.
public enum EventParser {

    /// Returns nil for blank lines. Throws `EventParseError.malformed` for non-JSON lines.
    public static func parse(line: String) throws -> EngineEvent? {
        let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty { return nil }
        guard let data = trimmed.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data),
              let dict = obj as? [String: Any] else {
            throw EventParseError.malformed(trimmed)
        }
        guard let kind = dict["event"] as? String else { return .unknown(trimmed) }

        switch kind {
        case "progress":
            var s = ProgressSnapshot()
            s.phase = dict["phase"] as? String ?? ""
            s.current = dict["current"] as? String ?? ""
            s.bytesRead = uint(dict, "bytes_read") ?? 0
            s.bytesWritten = uint(dict, "bytes_written") ?? 0
            s.bytesTotal = uint(dict, "bytes_total") ?? 0
            s.filesDone = Int(uint(dict, "files_done") ?? 0)
            s.filesTotal = Int(uint(dict, "files_total") ?? 0)
            return .progress(s)

        case "entry":
            return .entry(entry(from: dict))

        case "hash":
            var digests: [String: String] = [:]
            if let nested = dict["digests"] as? [String: Any] {
                for (k, v) in nested { if let s = v as? String { digests[k.lowercased()] = s } }
            }
            for (k, v) in dict where !["event", "path", "name", "digests", "file"].contains(k) {
                if let s = v as? String { digests[k.lowercased()] = s }
            }
            let path = (dict["path"] as? String) ?? (dict["file"] as? String) ?? (dict["name"] as? String) ?? ""
            return .hash(HashResult(path: path, digests: digests))

        case "done":
            let ok = (dict["ok"] as? Bool) ?? true
            return .done(ok: ok)

        case "error":
            let code = ErrorCode(wire: dict["code"] as? String ?? "")
            return .error(code: code, message: dict["message"] as? String ?? "")

        case "log":
            return .log(dict["message"] as? String ?? trimmed)

        default:
            return .unknown(trimmed)
        }
    }

    static func uint(_ d: [String: Any], _ key: String) -> UInt64? {
        guard let n = d[key] as? NSNumber else { return nil }
        let v = n.int64Value
        return v < 0 ? 0 : UInt64(v)
    }

    static func entry(from d: [String: Any]) -> ArchiveEntry {
        let name = (d["name"] as? String) ?? (d["path"] as? String) ?? ""
        var e = ArchiveEntry(name: name)
        e.size = uint(d, "size") ?? 0
        e.compressedSize = uint(d, "csize") ?? 0
        e.method = d["method"] as? String ?? ""
        e.comment = d["comment"] as? String
        if let b = d["encrypted"] as? Bool {
            e.encrypted = b
        } else if let s = d["encrypted"] as? String {
            e.encrypted = !s.isEmpty && s.lowercased() != "none"
            e.encryptionMethod = s
        }
        if let s = d["encryption"] as? String { e.encryptionMethod = s }
        e.attributes = uint(d, "attrs").map { UInt32(truncatingIfNeeded: $0) }
        e.isDirectory = (d["is_dir"] as? Bool) ?? (name.hasSuffix("/") || name.hasSuffix("\\"))
        e.isSymlink = (d["is_symlink"] as? Bool) ?? ArchiveEntry.symlinkFromAttributes(e.attributes)
        if let n = d["crc"] as? NSNumber, !(d["crc"] is String) {
            e.crc = UInt32(truncatingIfNeeded: n.int64Value)
        } else if let s = d["crc"] as? String {
            let hex = s.lowercased().hasPrefix("0x") ? String(s.dropFirst(2)) : s
            e.crc = UInt32(hex, radix: 16)
        }
        if let n = d["mtime"] as? NSNumber {
            e.modified = Date(timeIntervalSince1970: n.doubleValue)
        } else if let s = d["mtime"] as? String {
            e.modified = ISO8601DateFormatter().date(from: s)
        }
        return e
    }
}

// MARK: - Transport abstraction

/// Handle to one running engine invocation.
public struct EngineRun: Sendable {
    public let events: AsyncThrowingStream<EngineEvent, Error>
    public let pause: @Sendable () -> Void
    public let resume: @Sendable () -> Void
    public let cancel: @Sendable () -> Void
    /// Completes after the child exits, even when the event consumer is cancelled.
    public let waitForTermination: @Sendable () async -> Void

    public init(events: AsyncThrowingStream<EngineEvent, Error>,
                pause: @escaping @Sendable () -> Void,
                resume: @escaping @Sendable () -> Void,
                cancel: @escaping @Sendable () -> Void,
                waitForTermination: @escaping @Sendable () async -> Void = {}) {
        self.events = events
        self.pause = pause
        self.resume = resume
        self.cancel = cancel
        self.waitForTermination = waitForTermination
    }
}

/// Transport-agnostic engine interface.
///
/// The stream yields `progress`/`entry`/`hash`/`log` events and finishes normally
/// after `done`. An engine `error` event, a crash or a missing binary finishes
/// the stream by throwing `FZError`.
public protocol EngineClient: Sendable {
    func start(_ command: EngineCommand) -> EngineRun
}

public extension EngineClient {
    /// Convenience for browsing: collect all entries of an archive.
    func listEntries(archive: String, password: String? = nil, codepage: String? = nil) async throws -> [ArchiveEntry] {
        let run = start(.list(archive: archive, password: password, codepage: codepage))
        var result: [ArchiveEntry] = []
        var budget = ArchiveListingBudget()
        do {
            for try await event in run.events {
                try Task.checkCancellation()
                if case .entry(let e) = event {
                    try budget.admit(e)
                    result.append(e)
                }
            }
        } catch {
            run.cancel()
            throw error
        }
        return result
    }

    /// Convenience: run to completion, ignoring progress. Throws `FZError`.
    func runToCompletion(_ command: EngineCommand) async throws -> [EngineEvent] {
        let run = start(command)
        var all: [EngineEvent] = []
        for try await e in run.events { all.append(e) }
        return all
    }
}
