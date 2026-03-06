//! A generic buffered file that layers a [`Cache`] over any range-fetch
//! backend.  Implements [`FsFile`] so it can be used transparently in
//! place of a raw file object.
//!
//! Write path: data accumulates in a `Vec<u8>` buffer.  The caller
//! supplies an *uploader* closure that is called with the full buffer
//! on `commit()` (or on `Drop` if `autocommit` is set).

use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};

use crate::caching::{make_cache, Cache, CacheType, Fetcher};
use crate::error::FsResult;
use crate::file::FsFile;
use crate::types::{FileInfo, FileType, OpenMode};

// ============================================================================
// Uploader type
// ============================================================================

/// A function that writes the complete buffer to the backend.
/// Called with `(data)`.
pub type Uploader = Box<dyn FnMut(&[u8]) -> FsResult<()> + Send>;

// ============================================================================
// BufferedFile
// ============================================================================

/// A generic cached & buffered file.
///
/// - **Read** path: delegates to a [`Cache`] which wraps a range-fetch
///   `Fetcher`.
/// - **Write** path: accumulates data in an in-memory `Cursor<Vec<u8>>`
///   and flushes via an `Uploader` on `commit()`.
pub struct BufferedFile {
    /// The path of the file in the backend.
    path: String,
    /// Open mode.
    mode: OpenMode,
    /// Total size (for reads; may grow for writes).
    file_size: Option<u64>,

    // ── read state ──
    cache: Option<Box<dyn Cache>>,
    read_pos: u64,

    // ── write state ──
    write_buf: Cursor<Vec<u8>>,
    uploader: Option<Uploader>,
    autocommit: bool,
    committed: bool,
    discarded: bool,
}

impl BufferedFile {
    /// Create a new read-mode `BufferedFile`.
    pub fn new_read(
        path: String,
        fetcher: Fetcher,
        file_size: Option<u64>,
        cache_type: CacheType,
        blocksize: u64,
        max_blocks: usize,
    ) -> Self {
        let cache = make_cache(cache_type, fetcher, file_size, blocksize, max_blocks);
        Self {
            path,
            mode: OpenMode::Read,
            file_size,
            cache: Some(cache),
            read_pos: 0,
            write_buf: Cursor::new(Vec::new()),
            uploader: None,
            autocommit: true,
            committed: false,
            discarded: false,
        }
    }

    /// Create a new write-mode `BufferedFile`.
    pub fn new_write(path: String, uploader: Uploader, autocommit: bool) -> Self {
        Self {
            path,
            mode: OpenMode::Write,
            file_size: None,
            cache: None,
            read_pos: 0,
            write_buf: Cursor::new(Vec::new()),
            uploader: Some(uploader),
            autocommit,
            committed: false,
            discarded: false,
        }
    }

    /// The path this file was opened with.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Whether the file is writable.
    pub fn writable(&self) -> bool {
        matches!(
            self.mode,
            OpenMode::Write | OpenMode::Append | OpenMode::Exclusive
        )
    }

    /// Whether the file is readable.
    pub fn readable(&self) -> bool {
        matches!(self.mode, OpenMode::Read)
    }
}

// ============================================================================
// Read
// ============================================================================

impl Read for BufferedFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !self.readable() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "file not opened for reading",
            ));
        }

        let cache = self
            .cache
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "no cache available"))?;

        let file_sz = cache.size().unwrap_or(u64::MAX);
        if self.read_pos >= file_sz {
            return Ok(0); // EOF
        }

        let want = buf.len() as u64;
        let end = (self.read_pos + want).min(file_sz);

        let data = cache
            .fetch(self.read_pos, end)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        let n = data.len().min(buf.len());
        buf[..n].copy_from_slice(&data[..n]);
        self.read_pos += n as u64;
        Ok(n)
    }
}

// ============================================================================
// Write
// ============================================================================

impl Write for BufferedFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !self.writable() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "file not opened for writing",
            ));
        }
        self.write_buf.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.write_buf.flush()
    }
}

// ============================================================================
// Seek
// ============================================================================

impl Seek for BufferedFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        if self.readable() {
            let file_sz = self.cache.as_ref().and_then(|c| c.size()).unwrap_or(0) as i64;

            let new_pos = match pos {
                SeekFrom::Start(n) => n as i64,
                SeekFrom::Current(n) => self.read_pos as i64 + n,
                SeekFrom::End(n) => file_sz + n,
            };

            if new_pos < 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "seek to negative position",
                ));
            }
            self.read_pos = new_pos as u64;
            Ok(self.read_pos)
        } else {
            self.write_buf.seek(pos)
        }
    }
}

// ============================================================================
// FsFile
// ============================================================================

impl FsFile for BufferedFile {
    fn info(&self) -> FsResult<FileInfo> {
        let size = if self.readable() {
            self.file_size.unwrap_or(0)
        } else {
            self.write_buf.get_ref().len() as u64
        };
        Ok(FileInfo {
            name: self.path.clone(),
            size,
            file_type: FileType::File,
            created: None,
            modified: None,
            extra: std::collections::HashMap::new(),
        })
    }

    fn size(&self) -> FsResult<Option<u64>> {
        if self.readable() {
            Ok(self.file_size)
        } else {
            Ok(Some(self.write_buf.get_ref().len() as u64))
        }
    }

    fn commit(&mut self) -> FsResult<()> {
        if self.committed || self.discarded {
            return Ok(());
        }
        if self.writable() {
            if let Some(ref mut uploader) = self.uploader {
                let data = self.write_buf.get_ref();
                uploader(data)?;
            }
        }
        self.committed = true;
        Ok(())
    }

    fn discard(&mut self) -> FsResult<()> {
        self.discarded = true;
        Ok(())
    }
}

// ============================================================================
// Drop — auto-commit if requested
// ============================================================================

impl Drop for BufferedFile {
    fn drop(&mut self) {
        if self.autocommit && self.writable() && !self.committed && !self.discarded {
            let _ = self.commit();
        }
    }
}
