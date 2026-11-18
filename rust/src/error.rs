use std::fmt;
use std::io;

/// Error type for filesystem operations, mapping to common OS error semantics.
#[derive(Debug)]
pub enum FsError {
    /// File or directory not found
    NotFound(String),
    /// Permission denied
    PermissionDenied(String),
    /// File or directory already exists
    AlreadyExists(String),
    /// Target is not a directory
    NotADirectory(String),
    /// Target is a directory (expected a file)
    IsADirectory(String),
    /// Generic I/O error
    IoError(io::Error),
    /// Invalid argument or configuration
    InvalidArgument(String),
    /// Operation not supported by this backend
    NotSupported(String),
    /// Other error with description
    Other(String),
}

/// Stable error categories shared by typed Rust, Python, and C operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
#[non_exhaustive]
pub enum ErrorKind {
    NotFound = 1,
    PermissionDenied = 2,
    AlreadyExists = 3,
    NotADirectory = 4,
    IsADirectory = 5,
    Io = 6,
    Invalid = 7,
    NotSupported = 8,
    Other = 9,
    Conflict = 10,
    Gap = 11,
    Unavailable = 12,
    Timeout = 13,
    Corrupt = 14,
    Cancelled = 15,
    UnknownOutcome = 16,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::PermissionDenied => "permission_denied",
            Self::AlreadyExists => "already_exists",
            Self::NotADirectory => "not_a_directory",
            Self::IsADirectory => "is_a_directory",
            Self::Io => "io",
            Self::Invalid => "invalid",
            Self::NotSupported => "not_supported",
            Self::Other => "other",
            Self::Conflict => "conflict",
            Self::Gap => "gap",
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
            Self::Corrupt => "corrupt",
            Self::Cancelled => "cancelled",
            Self::UnknownOutcome => "unknown_outcome",
        }
    }
}

impl FsError {
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::NotFound(_) => ErrorKind::NotFound,
            Self::PermissionDenied(_) => ErrorKind::PermissionDenied,
            Self::AlreadyExists(_) => ErrorKind::AlreadyExists,
            Self::NotADirectory(_) => ErrorKind::NotADirectory,
            Self::IsADirectory(_) => ErrorKind::IsADirectory,
            Self::InvalidArgument(_) => ErrorKind::Invalid,
            Self::NotSupported(_) => ErrorKind::NotSupported,
            Self::Other(_) => ErrorKind::Other,
            Self::IoError(error) => match error.kind() {
                io::ErrorKind::TimedOut => ErrorKind::Timeout,
                io::ErrorKind::InvalidData => ErrorKind::Corrupt,
                io::ErrorKind::ConnectionRefused
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::ConnectionAborted
                | io::ErrorKind::NotConnected
                | io::ErrorKind::BrokenPipe => ErrorKind::Unavailable,
                _ => ErrorKind::Io,
            },
        }
    }
}

/// Failure from a byte read or write, including a failed condition.
///
/// Kept separate from [`FsError`] so existing backend error matches remain valid.
#[derive(Debug)]
#[non_exhaustive]
pub enum OperationError {
    /// A read or write condition was not satisfied.
    Conflict(String),
    /// An operation failure with explicit retry safety. Unknown outcomes must
    /// be resolved before retrying a write.
    Detailed {
        kind: ErrorKind,
        message: String,
        retry_safe: bool,
    },
    /// A filesystem failure, including unsupported conditional operations.
    FileSystem(FsError),
}

impl OperationError {
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Conflict(_) => ErrorKind::Conflict,
            Self::Detailed { kind, .. } => *kind,
            Self::FileSystem(error) => error.kind(),
        }
    }

    /// Whether repeating this operation is safe. False also covers cases
    /// where the backend cannot establish retry safety.
    pub fn retry_safe(&self) -> bool {
        matches!(self, Self::Detailed { retry_safe: true, kind, .. } if *kind != ErrorKind::UnknownOutcome)
    }
}

impl fmt::Display for OperationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(message) => write!(f, "condition conflict: {message}"),
            Self::FileSystem(error) => error.fmt(f),
            Self::Detailed { message, .. } => f.write_str(message),
        }
    }
}

impl std::error::Error for OperationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Conflict(_) | Self::Detailed { .. } => None,
            Self::FileSystem(error) => Some(error),
        }
    }
}

impl From<FsError> for OperationError {
    fn from(error: FsError) -> Self {
        Self::FileSystem(error)
    }
}

/// Result type alias for filesystem operations
pub type FsResult<T> = Result<T, FsError>;

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FsError::NotFound(msg) => write!(f, "not found: {msg}"),
            FsError::PermissionDenied(msg) => write!(f, "permission denied: {msg}"),
            FsError::AlreadyExists(msg) => write!(f, "already exists: {msg}"),
            FsError::NotADirectory(msg) => write!(f, "not a directory: {msg}"),
            FsError::IsADirectory(msg) => write!(f, "is a directory: {msg}"),
            FsError::IoError(err) => write!(f, "I/O error: {err}"),
            FsError::InvalidArgument(msg) => write!(f, "invalid argument: {msg}"),
            FsError::NotSupported(msg) => write!(f, "not supported: {msg}"),
            FsError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for FsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FsError::IoError(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for FsError {
    fn from(err: io::Error) -> Self {
        match err.kind() {
            io::ErrorKind::NotFound => FsError::NotFound(err.to_string()),
            io::ErrorKind::PermissionDenied => FsError::PermissionDenied(err.to_string()),
            io::ErrorKind::AlreadyExists => FsError::AlreadyExists(err.to_string()),
            _ => FsError::IoError(err),
        }
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;

    #[test]
    fn test_display() {
        let e = FsError::NotFound("foo.txt".into());
        assert_eq!(format!("{e}"), "not found: foo.txt");
    }

    #[test]
    fn test_from_io_error_not_found() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "gone");
        let fs_err: FsError = io_err.into();
        assert!(matches!(fs_err, FsError::NotFound(_)));
    }

    #[test]
    fn test_from_io_error_permission() {
        let io_err = io::Error::new(io::ErrorKind::PermissionDenied, "nope");
        let fs_err: FsError = io_err.into();
        assert!(matches!(fs_err, FsError::PermissionDenied(_)));
    }

    #[test]
    fn test_from_io_error_already_exists() {
        let io_err = io::Error::new(io::ErrorKind::AlreadyExists, "dup");
        let fs_err: FsError = io_err.into();
        assert!(matches!(fs_err, FsError::AlreadyExists(_)));
    }

    #[test]
    fn test_from_io_error_other() {
        let io_err = io::Error::other("misc");
        let fs_err: FsError = io_err.into();
        assert!(matches!(fs_err, FsError::IoError(_)));
    }

    #[test]
    fn test_source() {
        let io_err = io::Error::other("misc");
        let fs_err = FsError::IoError(io_err);
        assert!(std::error::Error::source(&fs_err).is_some());

        let fs_err2 = FsError::NotFound("x".into());
        assert!(std::error::Error::source(&fs_err2).is_none());
    }
}
