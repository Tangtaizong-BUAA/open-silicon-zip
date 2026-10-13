import Foundation
import Darwin

/// Owns decrypted preview/open-in-place files. A lease protects another running
/// app instance; abandoned sessions are reclaimed at the next launch.
@MainActor
public final class TemporaryExtractionStore {
    public let root: URL
    private var lease: Int32
    private var directories = Set<URL>()
    private var closed = false
    private static let prefix = "OpenSiliconZip-Plaintext-"

    public init(parent: URL = FileManager.default.temporaryDirectory) throws {
        Self.reclaimAbandonedSessions(in: parent)
        var template = Array(parent.appendingPathComponent(Self.prefix + "XXXXXX").path.utf8CString)
        guard let created = mkdtemp(&template) else { throw POSIXError(.EIO) }
        root = URL(fileURLWithPath: String(cString: created), isDirectory: true).resolvingSymlinksInPath().standardizedFileURL
        lease = Darwin.open(root.appendingPathComponent(".lease").path, O_CREAT | O_EXCL | O_RDWR | O_NOFOLLOW | O_CLOEXEC, 0o600)
        guard lease >= 0, flock(lease, LOCK_EX | LOCK_NB) == 0 else {
            if lease >= 0 { Darwin.close(lease); lease = -1 }
            try? FileManager.default.removeItem(at: root)
            throw POSIXError(.EIO)
        }
    }

    public func createDirectory() throws -> URL {
        guard !closed else { throw FZError(code: .cancelled) }
        let directory = root.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false,
                                               attributes: [.posixPermissions: 0o700])
        directories.insert(directory)
        return directory
    }

    public func remove(_ directory: URL) {
        guard directories.remove(directory) != nil else { return }
        try? FileManager.default.removeItem(at: directory)
    }

    public func removeFile(_ file: URL) {
        let path = file.standardizedFileURL.path
        guard let directory = directories.first(where: { path.hasPrefix($0.path + "/") }) else { return }
        remove(directory)
    }

    /// Call after engine processes stop so a late writer cannot recreate files.
    public func cleanUp() {
        guard !closed else { return }
        closed = true
        directories.removeAll()
        try? FileManager.default.removeItem(at: root)
        Darwin.close(lease)
        lease = -1
    }

    deinit {
        try? FileManager.default.removeItem(at: root)
        if lease >= 0 { Darwin.close(lease) }
    }

    private static func reclaimAbandonedSessions(in parent: URL) {
        let fm = FileManager.default
        guard let candidates = try? fm.contentsOfDirectory(at: parent, includingPropertiesForKeys: nil) else { return }
        for directory in candidates {
            let name = directory.lastPathComponent
            guard name.hasPrefix(prefix), name.count == prefix.count + 6 else { continue }
            var info = stat()
            guard lstat(directory.path, &info) == 0, (info.st_mode & S_IFMT) == S_IFDIR,
                  info.st_uid == geteuid() else { continue }
            let fd = Darwin.open(directory.appendingPathComponent(".lease").path, O_RDWR | O_NOFOLLOW | O_CLOEXEC)
            guard fd >= 0 else { continue }
            defer { Darwin.close(fd) }
            var leaseInfo = stat()
            guard fstat(fd, &leaseInfo) == 0, (leaseInfo.st_mode & S_IFMT) == S_IFREG,
                  leaseInfo.st_uid == geteuid(), flock(fd, LOCK_EX | LOCK_NB) == 0 else { continue }
            try? fm.removeItem(at: directory)
        }
    }
}
