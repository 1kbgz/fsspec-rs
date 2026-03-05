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
