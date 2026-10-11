//! Stable error taxonomy (FR-1201 / FR-1202).
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    WrongPassword,
    PasswordRequired,
    UnsupportedFormat,
    UnsupportedMethod,
    UnsupportedEncryption,
    CrcMismatch,
    BrokenArchive,
    MissingVolume,
    DiskFull,
    PathTooLong,
    PermissionDenied,
    Locked,
    DictionaryTooLarge,
    FourGbLimit,
    InvalidSplitSize,
    UnsafePath,
    Cancelled,
    InvalidInput,
    Io,
}

impl ErrorKind {
    /// Stable machine-readable code.
    pub fn code(self) -> &'static str {
        use ErrorKind::*;
        match self {
            WrongPassword => "E_WRONG_PASSWORD",
            PasswordRequired => "E_PASSWORD_REQUIRED",
            UnsupportedFormat => "E_UNSUPPORTED_FORMAT",
            UnsupportedMethod => "E_UNSUPPORTED_METHOD",
            UnsupportedEncryption => "E_UNSUPPORTED_ENCRYPTION",
            CrcMismatch => "E_CRC_MISMATCH",
            BrokenArchive => "E_BROKEN_ARCHIVE",
            MissingVolume => "E_MISSING_VOLUME",
            DiskFull => "E_DISK_FULL",
            PathTooLong => "E_PATH_TOO_LONG",
            PermissionDenied => "E_PERMISSION_DENIED",
            Locked => "E_LOCKED",
            DictionaryTooLarge => "E_DICT_TOO_LARGE",
            FourGbLimit => "E_4GB_LIMIT",
            InvalidSplitSize => "E_INVALID_SPLIT_SIZE",
            UnsafePath => "E_UNSAFE_PATH",
            Cancelled => "E_CANCELLED",
            InvalidInput => "E_INVALID_INPUT",
            Io => "E_IO",
        }
    }

    /// Human readable guidance, distinguishing broken / needs-volume / wrong-password.
    pub fn guidance(self) -> &'static str {
        use ErrorKind::*;
        match self {
            WrongPassword => "The password is wrong, or the archive is damaged.",
            PasswordRequired => "This archive is encrypted. Enter its password.",
            UnsupportedFormat => "FastZip does not recognise this file as an archive.",
            UnsupportedMethod => "The archive uses a compression method that is not supported.",
            UnsupportedEncryption => "The archive uses an encryption method that is not supported.",
            CrcMismatch => "A file failed its integrity check; the archive is damaged.",
            BrokenArchive => "The archive is damaged. Try Repair.",
            MissingVolume => "Another volume of this archive set is needed.",
            DiskFull => "The destination volume is full.",
            PathTooLong => "A destination path is too long for this file system.",
            PermissionDenied => "Permission denied. Choose an accessible folder.",
            Locked => "The file is in use by another process.",
            DictionaryTooLarge => "The archive needs more memory than allowed. Increase the memory budget.",
            FourGbLimit => "This format cannot hold files or volumes over 4 GB. Choose 7Z.",
            InvalidSplitSize => "The volume size is invalid (minimum 100 KB).",
            UnsafePath => "An entry tried to write outside the destination and was refused.",
            Cancelled => "The operation was cancelled.",
            InvalidInput => "Invalid input.",
            Io => "An I/O error occurred.",
        }
    }
}

#[derive(Debug)]
pub struct Error {
    pub kind: ErrorKind,
    pub detail: String,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn new(kind: ErrorKind, detail: impl Into<String>) -> Self {
        Error { kind, detail: detail.into() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.kind.code(), self.detail)
    }
}
impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        use std::io::ErrorKind as K;
        let kind = match e.kind() {
            K::Other if e.to_string() == crate::cancel::CANCEL_MSG => ErrorKind::Cancelled,
            K::PermissionDenied => ErrorKind::PermissionDenied,
            K::StorageFull | K::QuotaExceeded => ErrorKind::DiskFull,
            K::InvalidFilename => ErrorKind::PathTooLong,
            K::UnexpectedEof | K::InvalidData => ErrorKind::BrokenArchive,
            _ => match e.raw_os_error() {
                Some(36) => ErrorKind::PathTooLong,
                Some(28) => ErrorKind::DiskFull,
                _ => ErrorKind::Io,
            },
        };
        Error::new(kind, e.to_string())
    }
}
