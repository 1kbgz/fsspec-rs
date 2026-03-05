use std::io::{Read, Seek, Write};

use crate::error::FsResult;
use crate::types::FileInfo;

/// Trait for file-like objects returned by `FileSystem::open()`.
///
/// Mirrors fsspec's `AbstractBufferedFile` — backends implement
/// the raw I/O methods and get buffered read/write for free.
pub trait FsFile: Read + Write + Seek + Send {
    /// Return metadata about the open file.
    fn info(&self) -> FsResult<FileInfo>;

    /// The total size of the file (if known).
    fn size(&self) -> FsResult<Option<u64>>;

    /// Commit the file (for transaction support).
    /// Default: no-op.
    fn commit(&mut self) -> FsResult<()> {
        Ok(())
    }

    /// Discard the file (for transaction support).
    /// Default: no-op.
    fn discard(&mut self) -> FsResult<()> {
        Ok(())
    }
}
