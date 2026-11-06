use std::collections::HashMap;
use std::fs;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use crate::error::{FsError, FsResult};
use crate::file::FsFile;
use crate::fs::FileSystem;
use crate::types::{FileInfo, FileType, OpenMode, OpenOptions};

// ============================================================================
// LocalFile
// ============================================================================

/// A file-like object for the local filesystem.
///
/// Wraps either a real `std::fs::File` (for read/append) or a
/// `Cursor<Vec<u8>>` that is written atomically on commit/drop (for write/exclusive).
pub enum LocalFile {
    /// Directly wraps a standard file handle.
    Direct {
        path: String,
        inner: fs::File,
        mode: OpenMode,
    },
    /// Buffered write: accumulates in memory and replaces the target atomically on
    /// commit/drop, so readers see either the old contents or the complete new contents.
    Buffered {
        path: String,
        cursor: Cursor<Vec<u8>>,
        committed: bool,
        /// Fail instead of replacing an existing file.
        exclusive: bool,
    },
}

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// Writes `data` to a synced temporary file next to `path`, then renames it over `path`.
/// With `exclusive`, the temporary file is hard-linked instead, which fails if `path` exists.
fn write_atomic(path: &str, data: &[u8], exclusive: bool) -> FsResult<()> {
    let target = Path::new(path);
    let name = target
        .file_name()
        .ok_or_else(|| FsError::InvalidArgument(format!("not a file path: {path}")))?;
    let dir = target
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temp = dir.join(format!(
        ".{}.{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(data)?;
        file.sync_all()?;
        if exclusive {
            fs::hard_link(&temp, target)?;
        } else {
            fs::rename(&temp, target)?;
        }
        sync_dir(dir)
    })();
    if exclusive || result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(FsError::from)
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

impl Read for LocalFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            LocalFile::Direct { inner, .. } => inner.read(buf),
            LocalFile::Buffered { cursor, .. } => cursor.read(buf),
        }
    }
}

impl Write for LocalFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            LocalFile::Direct { inner, .. } => inner.write(buf),
            LocalFile::Buffered { cursor, .. } => cursor.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            LocalFile::Direct { inner, .. } => inner.flush(),
            // Buffered contents become visible only on commit.
            LocalFile::Buffered { .. } => Ok(()),
        }
    }
}

impl Seek for LocalFile {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match self {
            LocalFile::Direct { inner, .. } => inner.seek(pos),
            LocalFile::Buffered { cursor, .. } => cursor.seek(pos),
        }
    }
}

impl FsFile for LocalFile {
    fn info(&self) -> FsResult<FileInfo> {
        let path = match self {
            LocalFile::Direct { path, .. } | LocalFile::Buffered { path, .. } => path,
        };
        let meta = fs::metadata(path)?;
        Ok(metadata_to_info(path, &meta))
    }

    fn size(&self) -> FsResult<Option<u64>> {
        let path = match self {
            LocalFile::Direct { path, .. } | LocalFile::Buffered { path, .. } => path,
        };
        let meta = fs::metadata(path)?;
        Ok(Some(meta.len()))
    }

    fn commit(&mut self) -> FsResult<()> {
        if let LocalFile::Buffered {
            path,
            cursor,
            committed,
            exclusive,
        } = self
        {
            if !*committed {
                write_atomic(path, cursor.get_ref(), *exclusive)?;
                *committed = true;
            }
        }
        Ok(())
    }

    fn discard(&mut self) -> FsResult<()> {
        if let LocalFile::Buffered { committed, .. } = self {
            *committed = true; // prevent Drop from writing
        }
        Ok(())
    }
}

impl Drop for LocalFile {
    fn drop(&mut self) {
        if let LocalFile::Buffered {
            path,
            cursor,
            committed,
            exclusive,
        } = self
        {
            if !*committed {
                let _ = write_atomic(path, cursor.get_ref(), *exclusive);
            }
        }
    }
}

// ============================================================================
// LocalFs
// ============================================================================

/// Local filesystem backend, wrapping `std::fs`.
///
/// Implements the [`FileSystem`] trait using standard library filesystem
/// operations. This is the Rust equivalent of fsspec's
/// `LocalFileSystem`.
pub struct LocalFs {
    /// If true, auto-mkdir parent directories on write operations.
    pub auto_mkdir: bool,
}

impl LocalFs {
    /// Create a new `LocalFs`.
    pub fn new() -> Self {
        Self { auto_mkdir: false }
    }

    /// Create a new `LocalFs` with auto_mkdir enabled.
    pub fn with_auto_mkdir(auto_mkdir: bool) -> Self {
        Self { auto_mkdir }
    }
}

impl Default for LocalFs {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert `std::fs::Metadata` into `FileInfo`.
fn metadata_to_info(path: &str, meta: &fs::Metadata) -> FileInfo {
    let file_type = if meta.is_dir() {
        FileType::Directory
    } else if meta.is_file() {
        FileType::File
    } else {
        FileType::Other
    };

    let size = if meta.is_file() { meta.len() } else { 0 };

    FileInfo {
        name: path.to_string(),
        size,
        file_type,
        created: meta.created().ok(),
        modified: meta.modified().ok(),
        extra: HashMap::new(),
    }
}

/// Convert a `SystemTime` to epoch seconds (for extra metadata).
#[allow(dead_code)]
fn system_time_to_epoch(t: SystemTime) -> f64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

impl FileSystem for LocalFs {
    fn protocol(&self) -> &[&str] {
        &["file", "local"]
    }

    fn root_marker(&self) -> &str {
        "/"
    }

    fn sep(&self) -> &str {
        std::path::MAIN_SEPARATOR_STR
    }

    fn ls(&self, path: &str, _detail: bool) -> FsResult<Vec<FileInfo>> {
        let p = Path::new(path);
        if !p.exists() {
            return Err(FsError::NotFound(path.to_string()));
        }
        if !p.is_dir() {
            return Err(FsError::NotADirectory(path.to_string()));
        }

        let mut results = Vec::new();
        for entry in fs::read_dir(p)? {
            let entry = entry?;
            let entry_path = entry.path();
            let name = entry_path
                .to_str()
                .ok_or_else(|| FsError::Other("non-UTF-8 path".into()))?
                .to_string();
            let meta = entry.metadata()?;
            results.push(metadata_to_info(&name, &meta));
        }
        results.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(results)
    }

    fn rm_file(&self, path: &str) -> FsResult<()> {
        let p = Path::new(path);
        if !p.exists() {
            return Err(FsError::NotFound(path.to_string()));
        }
        if p.is_dir() {
            return Err(FsError::IsADirectory(path.to_string()));
        }
        fs::remove_file(p)?;
        Ok(())
    }

    fn cp_file(&self, src: &str, dst: &str) -> FsResult<()> {
        let src_p = Path::new(src);
        if !src_p.exists() {
            return Err(FsError::NotFound(src.to_string()));
        }
        if src_p.is_dir() {
            return Err(FsError::IsADirectory(format!(
                "source is a directory: {src}"
            )));
        }
        // Ensure parent of dst exists
        if let Some(parent) = Path::new(dst).parent() {
            if !parent.exists() && self.auto_mkdir {
                fs::create_dir_all(parent)?;
            }
        }
        fs::copy(src, dst)?;
        Ok(())
    }

    fn open(
        &self,
        path: &str,
        mode: OpenMode,
        _opts: Option<OpenOptions>,
    ) -> FsResult<Box<dyn FsFile>> {
        let p = Path::new(path);

        match mode {
            OpenMode::Read => {
                if !p.exists() {
                    return Err(FsError::NotFound(path.to_string()));
                }
                let f = fs::File::open(p)?;
                Ok(Box::new(LocalFile::Direct {
                    path: path.to_string(),
                    inner: f,
                    mode: OpenMode::Read,
                }))
            }
            OpenMode::Write => {
                if self.auto_mkdir {
                    if let Some(parent) = p.parent() {
                        if !parent.exists() {
                            fs::create_dir_all(parent)?;
                        }
                    }
                }
                Ok(Box::new(LocalFile::Buffered {
                    path: path.to_string(),
                    cursor: Cursor::new(Vec::new()),
                    committed: false,
                    exclusive: false,
                }))
            }
            OpenMode::Append => {
                if self.auto_mkdir {
                    if let Some(parent) = p.parent() {
                        if !parent.exists() {
                            fs::create_dir_all(parent)?;
                        }
                    }
                }
                let f = fs::OpenOptions::new().append(true).create(true).open(p)?;
                Ok(Box::new(LocalFile::Direct {
                    path: path.to_string(),
                    inner: f,
                    mode: OpenMode::Append,
                }))
            }
            OpenMode::Exclusive => {
                if p.exists() {
                    return Err(FsError::AlreadyExists(path.to_string()));
                }
                if self.auto_mkdir {
                    if let Some(parent) = p.parent() {
                        if !parent.exists() {
                            fs::create_dir_all(parent)?;
                        }
                    }
                }
                Ok(Box::new(LocalFile::Buffered {
                    path: path.to_string(),
                    cursor: Cursor::new(Vec::new()),
                    committed: false,
                    exclusive: true,
                }))
            }
        }
    }

    fn info(&self, path: &str) -> FsResult<FileInfo> {
        let p = Path::new(path);
        if !p.exists() {
            return Err(FsError::NotFound(path.to_string()));
        }
        let meta = fs::metadata(p)?;
        Ok(metadata_to_info(path, &meta))
    }

    fn mkdir(&self, path: &str, create_parents: bool) -> FsResult<()> {
        let p = Path::new(path);
        if p.exists() {
            return Err(FsError::AlreadyExists(path.to_string()));
        }
        if create_parents {
            fs::create_dir_all(p)?;
        } else {
            fs::create_dir(p)?;
        }
        Ok(())
    }

    fn rmdir(&self, path: &str) -> FsResult<()> {
        let p = Path::new(path);
        if !p.exists() {
            return Err(FsError::NotFound(path.to_string()));
        }
        if !p.is_dir() {
            return Err(FsError::NotADirectory(path.to_string()));
        }
        fs::remove_dir(p)?;
        Ok(())
    }
}
