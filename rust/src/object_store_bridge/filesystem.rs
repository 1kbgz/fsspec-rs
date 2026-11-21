use super::*;
use std::io::{Read, Seek, SeekFrom, Write};

use crate::{FileSystem, FsFile, OpenMode, OpenOptions};
use object_store::{GetOptions, GetRange, ObjectStore, ObjectStoreExt};

/// Synchronous, read-only filesystem access to an object store.
///
/// Keys are relative to `prefix`. I/O is rejected inside a Tokio runtime; async
/// callers should use the native object store. Instances share one process-owned
/// runtime. Files use bounded range requests and pin a version or ETag if present.
#[derive(Clone)]
pub struct ObjectStoreFs {
    store: Arc<dyn ObjectStore>,
    prefix: Path,
}

impl ObjectStoreFs {
    pub fn new(store: Arc<dyn ObjectStore>, prefix: Path) -> Self {
        Self { store, prefix }
    }

    fn location(&self, path: &str) -> FsResult<Path> {
        let path = key(path)?;
        if self.prefix.as_ref().is_empty() {
            return Ok(path);
        }
        if path.as_ref().is_empty() {
            return Ok(self.prefix.clone());
        }
        key(&format!("{}/{}", self.prefix, path))
    }

    fn relative(&self, location: &Path) -> FsResult<String> {
        let name = if self.prefix.as_ref().is_empty() {
            location.as_ref()
        } else {
            location
                .as_ref()
                .strip_prefix(&format!("{}/", self.prefix))
                .ok_or_else(|| {
                    FsError::InvalidArgument("store returned a path outside the root".into())
                })?
        };
        key(name).map_err(|e| {
            FsError::NotSupported(format!("unrepresentable listed object key: {e}"))
        })?;
        Ok(name.to_owned())
    }

    fn entries(&self, path: &str) -> FsResult<Vec<FileInfo>> {
        let location = self.location(path)?;
        let result = wait(self.store.list_with_delimiter(Some(&location)))?;
        let mut entries = Vec::new();
        for meta in result.objects {
            entries.push(file_info(&meta, &self.relative(&meta.location)?));
        }
        for prefix in result.common_prefixes {
            let name = self.relative(&prefix)?;
            if entries.iter().any(|e| e.name == name) {
                return Err(FsError::NotSupported(format!(
                    "object and directory share a name: {name}"
                )));
            }
            entries.push(FileInfo::directory(name));
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }
}

impl FileSystem for ObjectStoreFs {
    fn protocol(&self) -> &[&str] {
        &["object"]
    }

    fn ls(&self, path: &str, _: bool) -> FsResult<Vec<FileInfo>> {
        outside_runtime()?;
        if !path.is_empty() && !self.info(path)?.is_dir() {
            return Err(FsError::NotADirectory(path.into()));
        }
        self.entries(path)
    }

    fn info(&self, path: &str) -> FsResult<FileInfo> {
        outside_runtime()?;
        let location = self.location(path)?;
        if path.is_empty() {
            return Ok(FileInfo::directory(""));
        }
        match wait(self.store.head(&location)) {
            Ok(meta) => {
                if !self.entries(path)?.is_empty() {
                    return Err(FsError::NotSupported(format!(
                        "object and directory share a name: {path}"
                    )));
                }
                Ok(file_info(&meta, path))
            }
            Err(FsError::NotFound(_)) => {
                if self.entries(path)?.is_empty() {
                    Err(FsError::NotFound(path.into()))
                } else {
                    Ok(FileInfo::directory(path))
                }
            }
            Err(e) => Err(e),
        }
    }

    fn open(
        &self,
        path: &str,
        mode: OpenMode,
        opts: Option<OpenOptions>,
    ) -> FsResult<Box<dyn FsFile>> {
        outside_runtime()?;
        if mode != OpenMode::Read {
            return Err(read_only());
        }
        if opts.is_some() {
            return Err(FsError::NotSupported(
                "bridge file options are not implemented".into(),
            ));
        }
        let info = self.info(path)?;
        if !info.is_file() {
            return Err(FsError::IsADirectory(path.into()));
        }
        Ok(Box::new(ObjectFile {
            fs: self.clone(),
            location: self.location(path)?,
            info,
            pos: 0,
        }))
    }

    fn rm_file(&self, _: &str) -> FsResult<()> {
        Err(read_only())
    }
    fn cp_file(&self, _: &str, _: &str) -> FsResult<()> {
        Err(read_only())
    }
    fn mkdir(&self, _: &str, _: bool) -> FsResult<()> {
        Err(read_only())
    }
    fn rmdir(&self, _: &str) -> FsResult<()> {
        Err(read_only())
    }
}

struct ObjectFile {
    fs: ObjectStoreFs,
    location: Path,
    info: FileInfo,
    pos: u64,
}

impl Read for ObjectFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        outside_runtime().map_err(to_io_error)?;
        let size = self.info.size;
        if self.pos >= size || buf.is_empty() {
            return Ok(0);
        }
        let count = (size - self.pos).min(buf.len().min(CHUNK_SIZE) as u64);
        let options = GetOptions {
            range: Some(GetRange::Bounded(self.pos..self.pos + count)),
            version: self.info.extra.get("version").cloned(),
            if_match: self.info.extra.get("etag").cloned(),
            ..Default::default()
        };
        let expected = self.pos..self.pos + count;
        let bytes = wait(async {
            let result = self.fs.store.get_opts(&self.location, options).await?;
            if result.range != expected || result.meta.size != size {
                return Err(generic(
                    "object changed or store returned an incorrect range",
                ));
            }
            result.bytes().await
        })
        .map_err(to_io_error)?;
        if bytes.len() as u64 != count {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "store returned an incomplete range",
            ));
        }
        buf[..bytes.len()].copy_from_slice(&bytes);
        self.pos += count;
        Ok(bytes.len())
    }
}

impl Seek for ObjectFile {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let next = match from {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::End(n) => i128::from(self.info.size) + i128::from(n),
            SeekFrom::Current(n) => i128::from(self.pos) + i128::from(n),
        };
        self.pos = u64::try_from(next).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "seek is outside the u64 range")
        })?;
        Ok(self.pos)
    }
}
impl Write for ObjectFile {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "bridge is read-only",
        ))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl FsFile for ObjectFile {
    fn info(&self) -> FsResult<FileInfo> {
        Ok(self.info.clone())
    }
    fn size(&self) -> FsResult<Option<u64>> {
        Ok(Some(self.info.size))
    }
}
