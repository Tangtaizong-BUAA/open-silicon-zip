import Foundation

/// Admission limits apply before building the browser's expanded directory tree.
/// They also protect clients used with an older or separately configured engine.
public struct ArchiveListingBudget: Sendable {
    public static let maximumEntries = 100_000
    public static let maximumPathBytes = 4_096
    public static let maximumDepth = 64
    public static let maximumMetadataBytes = 16 * 1024 * 1024
    public static let maximumDirectoryNodes = 100_000
    public static let maximumExpandedPathBytes = 32 * 1024 * 1024

    private var count = 0
    private var metadataBytes = 0

    public init() {}

    public mutating func admit(_ entry: ArchiveEntry) throws {
        guard count < Self.maximumEntries,
              entry.name.utf8.count <= Self.maximumPathBytes else { throw Self.limitError() }
        let bytes = entry.name.utf8.count + entry.method.utf8.count
            + (entry.comment?.utf8.count ?? 0) + (entry.encryptionMethod?.utf8.count ?? 0)
        guard bytes <= Self.maximumMetadataBytes - metadataBytes,
              entry.components.count <= Self.maximumDepth else { throw Self.limitError() }
        count += 1
        metadataBytes += bytes
    }

    static func limitError() -> FZError {
        FZError(code: .resourceLimit, message: L10n.t("The archive listing exceeds the safe metadata or path-depth limit."))
    }

    static func adding(_ lhs: UInt64, _ rhs: UInt64) throws -> UInt64 {
        let (value, overflow) = lhs.addingReportingOverflow(rhs)
        guard !overflow else {
            throw FZError(code: .brokenArchive, message: L10n.t("The archive's combined sizes exceed the supported range."))
        }
        return value
    }
}
