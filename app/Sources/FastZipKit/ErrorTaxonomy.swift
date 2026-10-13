import Foundation

/// Stable error taxonomy (FR-1201/1202). Raw values are the wire codes emitted by `fz`.
public enum ErrorCode: String, Codable, Sendable, CaseIterable {
    case wrongPassword = "E_WRONG_PASSWORD"
    case passwordRequired = "E_PASSWORD_REQUIRED"
    case unsupportedFormat = "E_UNSUPPORTED_FORMAT"
    case unsupportedMethod = "E_UNSUPPORTED_METHOD"
    case unsupportedEncryption = "E_UNSUPPORTED_ENCRYPTION"
    case crcMismatch = "E_CRC_MISMATCH"
    case brokenArchive = "E_BROKEN_ARCHIVE"
    case missingVolume = "E_MISSING_VOLUME"
    case diskFull = "E_DISK_FULL"
    case pathTooLong = "E_PATH_TOO_LONG"
    case permissionDenied = "E_PERMISSION_DENIED"
    case locked = "E_LOCKED"
    case dictionaryTooLarge = "E_DICT_TOO_LARGE"
    case resourceLimit = "E_RESOURCE_LIMIT"
    case fourGBLimit = "E_4GB_LIMIT"
    case invalidSplitSize = "E_INVALID_SPLIT_SIZE"
    case unsafePath = "E_UNSAFE_PATH"
    case cancelled = "E_CANCELLED"
    case invalidInput = "E_INVALID_INPUT"
    case io = "E_IO"
    /// Client-side codes (never emitted by the engine).
    case engineCrashed = "E_ENGINE_CRASHED"
    case engineMissing = "E_ENGINE_MISSING"
    case unknown = "E_UNKNOWN"

    public init(wire: String) {
        self = ErrorCode(rawValue: wire) ?? .unknown
    }

    /// FR-1202: distinct guidance for distinct situations.
    public enum Kind: Sendable {
        case password, broken, volumeNeeded, unsupported, environment, limit, cancelled, internalFault
    }

    public var kind: Kind {
        switch self {
        case .wrongPassword, .passwordRequired: return .password
        case .crcMismatch, .brokenArchive: return .broken
        case .missingVolume: return .volumeNeeded
        case .unsupportedFormat, .unsupportedMethod, .unsupportedEncryption: return .unsupported
        case .diskFull, .pathTooLong, .permissionDenied, .locked, .unsafePath, .io: return .environment
        case .dictionaryTooLarge, .resourceLimit, .fourGBLimit, .invalidSplitSize, .invalidInput: return .limit
        case .cancelled: return .cancelled
        case .engineCrashed, .engineMissing, .unknown: return .internalFault
        }
    }

    public var title: String {
        switch self {
        case .wrongPassword: return L10n.t("Wrong password")
        case .passwordRequired: return L10n.t("Password required")
        case .unsupportedFormat: return L10n.t("Unsupported format")
        case .unsupportedMethod: return L10n.t("Unsupported compression method")
        case .unsupportedEncryption: return L10n.t("Unsupported encryption")
        case .crcMismatch: return L10n.t("Checksum mismatch")
        case .brokenArchive: return L10n.t("Archive is damaged")
        case .missingVolume: return L10n.t("Another volume is needed")
        case .diskFull: return L10n.t("Not enough disk space")
        case .pathTooLong: return L10n.t("Path is too long")
        case .permissionDenied: return L10n.t("Permission denied")
        case .locked: return L10n.t("File is in use")
        case .dictionaryTooLarge: return L10n.t("Not enough memory for this archive")
        case .resourceLimit: return L10n.t("Archive safety limit reached")
        case .fourGBLimit: return L10n.t("Format size limit reached")
        case .invalidSplitSize: return L10n.t("Invalid volume size")
        case .unsafePath: return L10n.t("Unsafe path blocked")
        case .cancelled: return L10n.t("Cancelled")
        case .invalidInput: return L10n.t("Invalid input")
        case .io: return L10n.t("Read/write error")
        case .engineCrashed: return L10n.t("The compression engine stopped unexpectedly")
        case .engineMissing: return L10n.t("Compression engine not found")
        case .unknown: return L10n.t("Something went wrong")
        }
    }

    public var guidance: String {
        switch self {
        case .wrongPassword:
            return L10n.t("The password does not match, or the archive is damaged. Try again or pick one from your saved passwords.")
        case .passwordRequired:
            return L10n.t("This archive is encrypted. Enter its password to continue.")
        case .unsupportedFormat:
            return L10n.t("FastZip cannot read this file type. Check that it is really an archive and not a partial download.")
        case .unsupportedMethod:
            return L10n.t("The archive uses a compression method this version does not support.")
        case .unsupportedEncryption:
            return L10n.t("The archive uses an encryption method this version does not support.")
        case .crcMismatch:
            return L10n.t("Some data does not match its stored checksum. The file may be corrupted; try Repair or download it again.")
        case .brokenArchive:
            return L10n.t("The archive structure is damaged. Try Repair to salvage readable files into a new archive.")
        case .missingVolume:
            return L10n.t("This archive is split across several files. Select the missing volume (all parts must be in the same folder and keep their names).")
        case .diskFull:
            return L10n.t("Free up space on the destination volume or choose another destination, then retry.")
        case .pathTooLong:
            return L10n.t("A file name or folder path exceeds the filesystem limit. Choose a shorter destination.")
        case .permissionDenied:
            return L10n.t("FastZip is not allowed to write there. Choose another destination or grant folder access in Preferences > Sandbox.")
        case .locked:
            return L10n.t("Another process is using this file. Close it and retry.")
        case .dictionaryTooLarge:
            return L10n.t("The archive needs more memory than the configured budget. Raise the limit in Preferences > Advanced or close other apps.")
        case .resourceLimit:
            return L10n.t("The archive exceeds the safe listing or buffering budget. Use a smaller archive or fewer entries.")
        case .fourGBLimit:
            return L10n.t("This format cannot hold that much data. Use 7Z, or split into volumes under 4 GB.")
        case .invalidSplitSize:
            return L10n.t("Volume size must be at least 100 KB.")
        case .unsafePath:
            return L10n.t("An entry tried to write outside the destination and was blocked for your safety.")
        case .cancelled:
            return L10n.t("The operation was cancelled.")
        case .invalidInput:
            return L10n.t("One of the inputs could not be used. Check the selected files.")
        case .io:
            return L10n.t("A file could not be read or written. Check the disk and permissions, then retry.")
        case .engineCrashed:
            return L10n.t("Your files were not modified by the failed step. Retry, and copy diagnostics if it keeps happening.")
        case .engineMissing:
            return L10n.t("Reinstall FastZip or set the engine path in Preferences > Advanced.")
        case .unknown:
            return L10n.t("Copy the diagnostics and report this if it keeps happening.")
        }
    }

    /// Offered actions (FR-1203).
    public enum Recovery: Sendable, Equatable {
        case retry, skip, abort, enterPassword, locateVolume, chooseDestination
        case repair, copyDiagnostics, openLog
    }

    public var recoveries: [Recovery] {
        switch kind {
        case .password: return [.enterPassword, .skip, .abort]
        case .broken: return [.repair, .skip, .copyDiagnostics]
        case .volumeNeeded: return [.locateVolume, .skip, .abort]
        case .unsupported: return [.skip, .copyDiagnostics]
        case .environment:
            return self == .permissionDenied || self == .diskFull
                ? [.chooseDestination, .retry, .skip, .abort]
                : [.retry, .skip, .abort, .openLog]
        case .limit: return [.abort, .copyDiagnostics]
        case .cancelled: return [.retry]
        case .internalFault: return [.retry, .copyDiagnostics, .openLog]
        }
    }
}

public struct FZError: Error, Sendable, Equatable, LocalizedError {
    public var code: ErrorCode
    public var message: String

    public init(code: ErrorCode, message: String = "") {
        self.code = code
        self.message = message
    }

    public var errorDescription: String? {
        message.isEmpty ? code.title : "\(code.title): \(message)"
    }

    /// Plain text suitable for "Copy diagnostics".
    public var diagnostics: String {
        "code=\(code.rawValue)\ntitle=\(code.title)\nmessage=\(message)"
    }
}
