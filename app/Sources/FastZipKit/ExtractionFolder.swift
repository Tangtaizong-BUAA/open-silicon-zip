import Foundation
import Darwin

/// Reserve a fresh directory, including on case-insensitive volumes and when a
/// file or dangling symlink occupies the preferred name. Never merge or follow it.
public enum ExtractionFolder {
    public static func reserve(in parent: URL, archiveName: String) throws -> URL {
        let raw = SmartExtract.archiveBaseName(archiveName)
        let base = raw.isEmpty || raw == "." || raw == ".." || raw.contains("/") || raw.contains("\0") ? "Archive" : raw
        for index in 1...10_000 {
            let name = index == 1 ? base : "\(base) (\(index))"
            let url = parent.appendingPathComponent(name, isDirectory: true)
            if mkdir(url.path, 0o755) == 0 { return url }
            let code = errno
            if code != EEXIST { throw NSError(domain: NSPOSIXErrorDomain, code: Int(code)) }
        }
        throw CocoaError(.fileWriteFileExists)
    }
}
