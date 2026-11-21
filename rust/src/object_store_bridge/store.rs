use super::*;
use std::collections::{BTreeSet, VecDeque};
use std::io::{Read, Seek, SeekFrom};

use crate::{FileSystem, FileType, OpenMode};
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{stream, stream::BoxStream, StreamExt, TryStreamExt};
use object_store::{
    Attributes, CopyOptions, GetOptions, GetResult, GetResultPayload, ListResult, MultipartUpload,
    ObjectStore, PutMultipartOptions, PutOptions, PutPayload, PutResult,
};

/// Read-only object access to an immutable filesystem namespace.
///
/// The caller must keep contents, names, and metadata unchanged for the lifetime
/// of this adapter and all returned streams. The adapter does not freeze the
/// filesystem. Missing modification times and non-regular files are unsupported.
#[derive(Clone)]
pub struct FileSystemStore {
    fs: Arc<dyn FileSystem + Send + Sync>,
    root: String,
}

// A stream may be dropped on an async executor. Queue handle destruction on the
// blocking pool instead of calling an arbitrary backend's Drop on that thread.
struct StreamFile {
    file: Option<Box<dyn crate::FsFile>>,
    runtime: &'static Runtime,
}

impl Drop for StreamFile {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            self.runtime.spawn_blocking(move || drop(file));
        }
    }
}

impl FileSystemStore {
    /// Bind an immutable source to a filesystem root. The root is a namespace
    /// mapping, not a security sandbox; callers must exclude symlink escapes.
    pub fn from_snapshot(fs: Arc<dyn FileSystem + Send + Sync>, root: &str) -> FsResult<Self> {
        if fs.sep() != "/" {
            return Err(FsError::NotSupported(
                "bridge requires slash-separated paths".into(),
            ));
        }
        let root = fs.strip_protocol(root);
        Ok(Self {
            fs,
            root: root.trim_end_matches('/').to_owned(),
        })
    }

    fn path(&self, key: &Path) -> String {
        // Preserve the root slash of local/memory filesystems.
        if self.root.is_empty() {
            format!("{}{}", self.fs.root_marker(), key)
        } else if key.as_ref().is_empty() {
            self.root.clone()
        } else {
            format!("{}/{}", self.root, key)
        }
    }

    fn meta(&self, info: FileInfo, location: Path) -> object_store::Result<ObjectMeta> {
        if info.file_type == FileType::Directory {
            return Err(Error::NotFound {
                path: location.to_string(),
                source: "filesystem directory is not an object".into(),
            });
        }
        if info.file_type != FileType::File {
            return Err(unsupported("object metadata requires a regular file"));
        }
        Ok(ObjectMeta {
            location,
            size: info.size,
            last_modified: info
                .modified
                .ok_or_else(|| unsupported("filesystem does not provide a modification time"))?
                .into(),
            e_tag: info.extra.get("etag").cloned(),
            version: info.extra.get("version").cloned(),
        })
    }

    fn children(&self, dir: &Path) -> object_store::Result<Vec<(Path, FileInfo)>> {
        let path = self.path(dir);
        let prefix = format!("{}/", path.trim_end_matches('/'));
        let prefix = if path.is_empty() { "" } else { &prefix };
        self.fs
            .ls(&path, true)
            .map_err(|e| from_fs_error(&path, e))?
            .into_iter()
            .map(|info| {
                let child = info.name.strip_prefix(prefix).ok_or_else(|| {
                    unsupported("listing returned an entry outside its directory")
                })?;
                if child.is_empty() || child.contains('/') {
                    return Err(unsupported(
                        "listing must return immediate children with full paths",
                    ));
                }
                let name = if dir.as_ref().is_empty() {
                    child.to_owned()
                } else {
                    format!("{dir}/{child}")
                };
                let location = key(&name).map_err(|e| unsupported(&e.to_string()))?;
                Ok((location, info))
            })
            .collect()
    }
}

impl fmt::Debug for FileSystemStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FileSystemStore")
    }
}
impl fmt::Display for FileSystemStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FileSystemStore")
    }
}

#[async_trait]
impl ObjectStore for FileSystemStore {
    async fn get_opts(
        &self,
        location: &Path,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        key(location.as_ref()).map_err(|e| from_fs_error(location.as_ref(), e))?;
        if location.as_ref().is_empty() {
            return Err(Error::NotFound {
                path: String::new(),
                source: "root is not an object".into(),
            });
        }
        if options.version.is_some() {
            return Err(unsupported(
                "version selection is not supported by the snapshot bridge",
            ));
        }
        let this = self.clone();
        let location = location.clone();
        let (file, meta, range) = blocking(move || {
            let path = this.path(&location);
            let info = this.fs.info(&path).map_err(|e| from_fs_error(&path, e))?;
            let meta = this.meta(info, location)?;
            options.check_preconditions(&meta)?;
            let range = match &options.range {
                Some(r) => r.as_range(meta.size).map_err(generic)?,
                None => 0..meta.size,
            };
            let file = if options.head {
                None
            } else {
                let mut file = this
                    .fs
                    .open(&path, OpenMode::Read, None)
                    .map_err(|e| from_fs_error(&path, e))?;
                file.seek(SeekFrom::Start(range.start)).map_err(generic)?;
                Some(StreamFile {
                    file: Some(file),
                    runtime: runtime().map_err(|e| from_fs_error(&path, e))?,
                })
            };
            Ok((file, meta, range))
        })
        .await?;
        let remaining = if file.is_some() {
            range.end - range.start
        } else {
            0
        };
        let chunks = stream::try_unfold((file, remaining), |(file, remaining)| async move {
            if remaining == 0 {
                return Ok(None);
            }
            blocking(move || {
                let mut file = file.expect("body stream owns a file");
                let mut data = vec![0; remaining.min(CHUNK_SIZE as u64) as usize];
                file.file
                    .as_mut()
                    .expect("stream owns its handle")
                    .read_exact(&mut data)
                    .map_err(generic)?;
                let remaining = remaining - data.len() as u64;
                let file = if remaining == 0 {
                    // Already on a blocking worker; close before returning the final chunk.
                    drop(file.file.take());
                    None
                } else {
                    Some(file)
                };
                Ok(Some((Bytes::from(data), (file, remaining))))
            })
            .await
        })
        .boxed();
        Ok(GetResult {
            payload: GetResultPayload::Stream(chunks),
            meta,
            range,
            attributes: Attributes::default(),
            extensions: Default::default(),
        })
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
        let this = self.clone();
        let prefix = prefix.cloned().unwrap_or_default();
        if let Err(e) = key(prefix.as_ref()) {
            return stream::once(std::future::ready(Err(from_fs_error(prefix.as_ref(), e))))
                .boxed();
        }
        stream::try_unfold(
            (
                this,
                prefix,
                VecDeque::from([Path::default()]),
                VecDeque::<(Path, FileInfo)>::new(),
            ),
            |(this, prefix, mut dirs, mut entries)| async move {
                loop {
                    if let Some((path, info)) = entries.pop_front() {
                        match info.file_type {
                            FileType::Directory => {
                                // Only visit directories overlapping the requested prefix.
                                if path.prefix_matches(&prefix) || prefix.prefix_matches(&path) {
                                    dirs.push_back(path);
                                }
                            }
                            FileType::File if path != prefix && path.prefix_matches(&prefix) => {
                                let meta = this.meta(info, path)?;
                                return Ok(Some((meta, (this, prefix, dirs, entries))));
                            }
                            FileType::File => {}
                            FileType::Other
                                if path.prefix_matches(&prefix) || prefix.prefix_matches(&path) =>
                            {
                                return Err(unsupported("listing contains a non-regular entry"))
                            }
                            FileType::Other => {}
                        }
                    } else if let Some(dir) = dirs.pop_front() {
                        let cloned = this.clone();
                        entries = blocking(move || cloned.children(&dir)).await?.into();
                    } else {
                        return Ok(None);
                    }
                }
            },
        )
        .boxed()
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> object_store::Result<ListResult> {
        let prefix = prefix.cloned().unwrap_or_default();
        let mut objects = Vec::new();
        let mut common = BTreeSet::new();
        let mut stream = self.list(Some(&prefix));
        while let Some(meta) = stream.try_next().await? {
            let rest = meta
                .location
                .as_ref()
                .strip_prefix(prefix.as_ref())
                .unwrap()
                .trim_start_matches('/');
            if rest.is_empty() {
                continue;
            }
            if let Some((first, _)) = rest.split_once('/') {
                let name = if prefix.as_ref().is_empty() {
                    first.to_owned()
                } else {
                    format!("{prefix}/{first}")
                };
                common.insert(Path::parse(name)?);
            } else {
                objects.push(meta);
            }
        }
        Ok(ListResult {
            objects,
            common_prefixes: common.into_iter().collect(),
            extensions: Default::default(),
        })
    }

    async fn put_opts(
        &self,
        _: &Path,
        _: PutPayload,
        _: PutOptions,
    ) -> object_store::Result<PutResult> {
        Err(unsupported("bridge is read-only"))
    }
    async fn put_multipart_opts(
        &self,
        _: &Path,
        _: PutMultipartOptions,
    ) -> object_store::Result<Box<dyn MultipartUpload>> {
        Err(unsupported("bridge is read-only"))
    }
    fn delete_stream(
        &self,
        locations: BoxStream<'static, object_store::Result<Path>>,
    ) -> BoxStream<'static, object_store::Result<Path>> {
        locations
            .map(|location| location.and_then(|_| Err(unsupported("bridge is read-only"))))
            .boxed()
    }
    async fn copy_opts(&self, _: &Path, _: &Path, _: CopyOptions) -> object_store::Result<()> {
        Err(unsupported("bridge is read-only"))
    }
}
