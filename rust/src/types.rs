use std::time::SystemTime;

/// The type of a filesystem entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileType {
    /// Regular file
    File,
    /// Directory
    Directory,
    /// Other (symlink, device, etc.)
    Other,
}

impl std::fmt::Display for FileType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileType::File => write!(f, "file"),
            FileType::Directory => write!(f, "directory"),
            FileType::Other => write!(f, "other"),
        }
    }
}

/// Metadata about a filesystem entry, analogous to fsspec's info dict.
#[derive(Clone, Debug)]
pub struct FileInfo {
    /// Full path of the entry
    pub name: String,
    /// Size in bytes (0 for directories)
    pub size: u64,
    /// Type of entry
    pub file_type: FileType,
    /// Creation time (if available)
    pub created: Option<SystemTime>,
    /// Last modification time (if available)
    pub modified: Option<SystemTime>,
    /// Additional metadata as key-value pairs
    pub extra: std::collections::HashMap<String, String>,
}

impl FileInfo {
    /// Create a new FileInfo for a file.
    pub fn file(name: impl Into<String>, size: u64) -> Self {
        Self {
            name: name.into(),
            size,
            file_type: FileType::File,
            created: None,
            modified: None,
            extra: std::collections::HashMap::new(),
        }
    }

    /// Create a new FileInfo for a directory.
    pub fn directory(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            size: 0,
            file_type: FileType::Directory,
            created: None,
            modified: None,
            extra: std::collections::HashMap::new(),
        }
    }

    /// Returns true if this entry is a file.
    pub fn is_file(&self) -> bool {
        self.file_type == FileType::File
    }

    /// Returns true if this entry is a directory.
    pub fn is_dir(&self) -> bool {
        self.file_type == FileType::Directory
    }
}

/// Mode for opening a file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpenMode {
    /// Read-only (binary)
    Read,
    /// Write (truncate, binary)
    Write,
    /// Append (binary)
    Append,
    /// Exclusive create (fail if exists, binary)
    Exclusive,
}

impl OpenMode {
    /// Parse from a Python-style mode string ("rb", "wb", "ab", "xb").
    pub fn from_str_mode(mode: &str) -> Option<Self> {
        match mode {
            "rb" | "r" => Some(OpenMode::Read),
            "wb" | "w" => Some(OpenMode::Write),
            "ab" | "a" => Some(OpenMode::Append),
            "xb" | "x" => Some(OpenMode::Exclusive),
            _ => None,
        }
    }

    /// Convert to a Python-style mode string.
    pub fn as_str(&self) -> &'static str {
        match self {
            OpenMode::Read => "rb",
            OpenMode::Write => "wb",
            OpenMode::Append => "ab",
            OpenMode::Exclusive => "xb",
        }
    }
}

/// Options for opening a file.
#[derive(Clone, Debug)]
pub struct OpenOptions {
    /// Block size for buffered I/O
    pub block_size: usize,
    /// Whether to auto-commit on close (vs. transaction mode)
    pub autocommit: bool,
    /// Cache strategy to use for reads (default: `None` meaning backend decides).
    pub cache_type: Option<crate::caching::CacheType>,
    /// Maximum number of cached blocks (only meaningful for `CacheType::Block`).
    pub max_blocks: usize,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            block_size: 4 * 1024 * 1024, // 4 MiB
            autocommit: true,
            cache_type: None,
            max_blocks: 32,
        }
    }
}

/// A single entry from walk().
#[derive(Clone, Debug)]
pub struct WalkEntry {
    /// Directory path being walked
    pub dirpath: String,
    /// Subdirectory names within dirpath
    pub dirnames: Vec<String>,
    /// File names within dirpath
    pub filenames: Vec<String>,
}

/// Result of a du() (disk usage) call
#[derive(Clone, Debug)]
pub enum DuResult {
    /// Total bytes used (when total=true)
    Total(u64),
    /// Per-path sizes (when total=false)
    PerPath(std::collections::HashMap<String, u64>),
}

#[cfg(test)]
mod types_tests {
    use super::*;

    #[test]
    fn test_file_info_file() {
        let info = FileInfo::file("/tmp/foo.txt", 1024);
        assert_eq!(info.name, "/tmp/foo.txt");
        assert_eq!(info.size, 1024);
        assert!(info.is_file());
        assert!(!info.is_dir());
        assert_eq!(info.file_type, FileType::File);
    }

    #[test]
    fn test_file_info_directory() {
        let info = FileInfo::directory("/tmp/bar");
        assert_eq!(info.name, "/tmp/bar");
        assert_eq!(info.size, 0);
        assert!(!info.is_file());
        assert!(info.is_dir());
    }

    #[test]
    fn test_file_type_display() {
        assert_eq!(format!("{}", FileType::File), "file");
        assert_eq!(format!("{}", FileType::Directory), "directory");
        assert_eq!(format!("{}", FileType::Other), "other");
    }

    #[test]
    fn test_open_mode_from_str() {
        assert_eq!(OpenMode::from_str_mode("rb"), Some(OpenMode::Read));
        assert_eq!(OpenMode::from_str_mode("r"), Some(OpenMode::Read));
        assert_eq!(OpenMode::from_str_mode("wb"), Some(OpenMode::Write));
        assert_eq!(OpenMode::from_str_mode("w"), Some(OpenMode::Write));
        assert_eq!(OpenMode::from_str_mode("ab"), Some(OpenMode::Append));
        assert_eq!(OpenMode::from_str_mode("a"), Some(OpenMode::Append));
        assert_eq!(OpenMode::from_str_mode("xb"), Some(OpenMode::Exclusive));
        assert_eq!(OpenMode::from_str_mode("x"), Some(OpenMode::Exclusive));
        assert_eq!(OpenMode::from_str_mode("rw"), None);
    }

    #[test]
    fn test_open_mode_as_str() {
        assert_eq!(OpenMode::Read.as_str(), "rb");
        assert_eq!(OpenMode::Write.as_str(), "wb");
        assert_eq!(OpenMode::Append.as_str(), "ab");
        assert_eq!(OpenMode::Exclusive.as_str(), "xb");
    }

    #[test]
    fn test_open_options_default() {
        let opts = OpenOptions::default();
        assert_eq!(opts.block_size, 4 * 1024 * 1024);
        assert!(opts.autocommit);
    }

    #[test]
    fn test_walk_entry() {
        let entry = WalkEntry {
            dirpath: "/tmp".into(),
            dirnames: vec!["sub1".into(), "sub2".into()],
            filenames: vec!["a.txt".into()],
        };
        assert_eq!(entry.dirpath, "/tmp");
        assert_eq!(entry.dirnames.len(), 2);
        assert_eq!(entry.filenames.len(), 1);
    }

    #[test]
    fn test_du_result_total() {
        let du = DuResult::Total(1024);
        assert!(matches!(du, DuResult::Total(1024)));
    }

    #[test]
    fn test_du_result_per_path() {
        let mut map = std::collections::HashMap::new();
        map.insert("/tmp/a".into(), 100);
        map.insert("/tmp/b".into(), 200);
        let du = DuResult::PerPath(map);
        if let DuResult::PerPath(ref m) = du {
            assert_eq!(m.len(), 2);
        } else {
            panic!("expected PerPath");
        }
    }

    #[test]
    fn test_file_info_extra_metadata() {
        let mut info = FileInfo::file("test", 0);
        info.extra.insert("etag".into(), "abc123".into());
        assert_eq!(info.extra.get("etag"), Some(&"abc123".to_string()));
    }
}
