//! S3 filesystem backend using the `object_store` crate.
//!
//! Wraps [`object_store::aws::AmazonS3`] to implement the [`FileSystem`] trait,
//! providing a sync API via an embedded tokio runtime.

use std::collections::HashMap;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::sync::Arc;

use bytes::Bytes;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, PutPayload};
use tokio::runtime::Runtime;

use crate::buffered::{BufferedFile, Uploader};
use crate::caching::Fetcher;
use crate::error::{FsError, FsResult};
use crate::file::FsFile;
use crate::fs::FileSystem;
use crate::types::{FileInfo, FileType, OpenMode, OpenOptions};

// ---------------------------------------------------------------------------
// Error conversion
// ---------------------------------------------------------------------------

fn obj_err_to_fs(err: object_store::Error) -> FsError {
    match &err {
        object_store::Error::NotFound { .. } => FsError::NotFound(err.to_string()),
        object_store::Error::AlreadyExists { .. } => FsError::AlreadyExists(err.to_string()),
        object_store::Error::Precondition { .. } => FsError::AlreadyExists(err.to_string()),
        object_store::Error::PermissionDenied { .. } => FsError::PermissionDenied(err.to_string()),
        _ => FsError::Other(err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Normalise a key so it never starts with a leading `/`.
fn normalise_key(key: &str) -> &str {
    key.strip_prefix('/').unwrap_or(key)
}

/// Build an `ObjectPath` from a key string.
fn to_object_path(key: &str) -> ObjectPath {
    ObjectPath::from(normalise_key(key))
}

// ---------------------------------------------------------------------------
// S3Fs
// ---------------------------------------------------------------------------

/// Configuration used to construct an [`S3Fs`].
#[derive(Clone, Debug)]
pub struct S3Config {
    /// S3 bucket name.
    pub bucket: String,
    /// AWS region (e.g. `"us-east-1"`).
    pub region: Option<String>,
    /// Custom endpoint URL (for MinIO / Backblaze B2 / LocalStack).
    pub endpoint_url: Option<String>,
    /// Access key ID.
    pub access_key_id: Option<String>,
    /// Secret access key.
    pub secret_access_key: Option<String>,
    /// Session token (for temporary credentials).
    pub session_token: Option<String>,
    /// Use anonymous access (no credentials).
    pub anon: bool,
    /// Use virtual-hosted style URLs.
    pub virtual_hosted_style_request: bool,
}

impl S3Config {
    /// Minimal config for a given bucket.
    pub fn new(bucket: impl Into<String>) -> Self {
        Self {
            bucket: bucket.into(),
            region: None,
            endpoint_url: None,
            access_key_id: None,
            secret_access_key: None,
            session_token: None,
            anon: false,
            virtual_hosted_style_request: false,
        }
    }
}

/// Sync S3 filesystem backed by `object_store::aws::AmazonS3`.
///
/// All async `object_store` calls are executed via an embedded tokio runtime.
pub struct S3Fs {
    store: Arc<AmazonS3>,
    rt: Runtime,
    pub bucket: String,
}

impl S3Fs {
    /// Build from an [`S3Config`].
    pub fn new(cfg: S3Config) -> FsResult<Self> {
        let mut builder = AmazonS3Builder::new().with_bucket_name(&cfg.bucket);

        if let Some(ref region) = cfg.region {
            builder = builder.with_region(region);
        }
        if let Some(ref endpoint) = cfg.endpoint_url {
            builder = builder.with_endpoint(endpoint);
            // Allow plain HTTP (needed for MinIO / LocalStack / other local S3)
            if endpoint.starts_with("http://") {
                builder = builder.with_allow_http(true);
            }
        }
        if let Some(ref key) = cfg.access_key_id {
            builder = builder.with_access_key_id(key);
        }
        if let Some(ref secret) = cfg.secret_access_key {
            builder = builder.with_secret_access_key(secret);
        }
        if let Some(ref token) = cfg.session_token {
            builder = builder.with_token(token);
        }
        if cfg.anon {
            builder = builder.with_skip_signature(true);
        }
        if cfg.virtual_hosted_style_request {
            builder = builder.with_virtual_hosted_style_request(true);
        }

        let store = builder
            .build()
            .map_err(|e| FsError::InvalidArgument(e.to_string()))?;

        let rt = Runtime::new().map_err(|e| FsError::Other(e.to_string()))?;

        Ok(Self {
            store: Arc::new(store),
            rt,
            bucket: cfg.bucket,
        })
    }

    /// Fetch the byte range `[start, end)` from an S3 object.
    /// This performs a range-GET using `object_store`.
    pub fn fetch_range(&self, key: &str, start: u64, end: u64) -> FsResult<Vec<u8>> {
        use object_store::GetOptions;
        use object_store::GetRange;

        let obj_path = to_object_path(key);
        let range = GetRange::Bounded(start..end);
        let opts = GetOptions {
            range: Some(range),
            ..Default::default()
        };

        let result = self
            .rt
            .block_on(self.store.get_opts(&obj_path, opts))
            .map_err(obj_err_to_fs)?;
        let bytes = self.rt.block_on(result.bytes()).map_err(obj_err_to_fs)?;
        Ok(bytes.to_vec())
    }

    /// Upload data to an S3 object.
    pub fn upload_data(&self, key: &str, data: &[u8]) -> FsResult<()> {
        let obj_path = to_object_path(key);
        let payload = PutPayload::from(Bytes::from(data.to_vec()));
        self.rt
            .block_on(self.store.put(&obj_path, payload))
            .map_err(obj_err_to_fs)?;
        Ok(())
    }
}

impl FileSystem for S3Fs {
    fn protocol(&self) -> &[&str] {
        &["s3"]
    }

    fn root_marker(&self) -> &str {
        ""
    }

    fn sep(&self) -> &str {
        "/"
    }

    /// Strip `s3://bucket/` prefix from a path.
    fn strip_protocol(&self, path: &str) -> String {
        let mut p = path.to_string();
        // Remove protocol prefix
        for proto in self.protocol() {
            let prefix = format!("{proto}://");
            if let Some(stripped) = p.strip_prefix(&prefix) {
                p = stripped.to_string();
                break;
            }
        }
        // Remove bucket name prefix
        let bucket_prefix = format!("{}/", self.bucket);
        if let Some(stripped) = p.strip_prefix(&bucket_prefix) {
            return stripped.to_string();
        }
        if p == self.bucket {
            return String::new();
        }
        p
    }

    fn unstrip_protocol(&self, path: &str) -> String {
        let proto = self.protocol().first().unwrap_or(&"s3");
        let key = normalise_key(path);
        if key.is_empty() {
            format!("{proto}://{}", self.bucket)
        } else {
            format!("{proto}://{}/{key}", self.bucket)
        }
    }

    // ---------------------------------------------------------------
    // Primitives
    // ---------------------------------------------------------------

    fn ls(&self, path: &str, detail: bool) -> FsResult<Vec<FileInfo>> {
        let key = self.strip_protocol(path);
        let prefix = if key.is_empty() {
            None
        } else {
            let k = normalise_key(&key);
            // Ensure prefix ends with "/" so we list the contents of the directory.
            let with_slash = if k.ends_with('/') {
                k.to_string()
            } else {
                format!("{k}/")
            };
            Some(ObjectPath::from(with_slash.as_str()))
        };

        let result = self
            .rt
            .block_on(async {
                object_store::ObjectStore::list_with_delimiter(&*self.store, prefix.as_ref()).await
            })
            .map_err(obj_err_to_fs)?;

        let mut entries = Vec::new();

        // Subdirectories (common prefixes)
        for prefix in &result.common_prefixes {
            let dir_name = prefix.as_ref().trim_end_matches('/');
            entries.push(FileInfo {
                name: format!("{}/{dir_name}", self.bucket),
                size: 0,
                file_type: FileType::Directory,
                created: None,
                modified: None,
                extra: HashMap::new(),
            });
        }

        // Files (objects)
        for obj in &result.objects {
            let obj_key = obj.location.as_ref();
            entries.push(FileInfo {
                name: format!("{}/{obj_key}", self.bucket),
                size: obj.size,
                file_type: FileType::File,
                created: None,
                modified: Some(obj.last_modified.into()),
                extra: {
                    let mut m = HashMap::new();
                    if let Some(ref etag) = obj.e_tag {
                        m.insert("etag".into(), etag.clone());
                    }
                    m
                },
            });
        }

        entries.sort_by(|a, b| a.name.cmp(&b.name));

        if !detail && !entries.is_empty() {
            // The trait always returns FileInfo; "detail=false" still works,
            // the caller just uses .name. We keep full info for consistency.
        }

        // If prefix + objects are both empty, the "directory" may not exist.
        // S3 doesn't have real directories, so this is fine — return empty.
        Ok(entries)
    }

    fn info(&self, path: &str) -> FsResult<FileInfo> {
        let key = self.strip_protocol(path);
        let key = normalise_key(&key);

        if key.is_empty() {
            // Root of bucket
            return Ok(FileInfo::directory(self.bucket.clone()));
        }

        // Try to HEAD the object first (it's a file)
        let obj_path = to_object_path(key);
        match self.rt.block_on(self.store.head(&obj_path)) {
            Ok(meta) => {
                return Ok(FileInfo {
                    name: format!("{}/{key}", self.bucket),
                    size: meta.size,
                    file_type: FileType::File,
                    created: None,
                    modified: Some(meta.last_modified.into()),
                    extra: {
                        let mut m = HashMap::new();
                        if let Some(ref etag) = meta.e_tag {
                            m.insert("etag".into(), etag.clone());
                        }
                        m
                    },
                });
            }
            Err(object_store::Error::NotFound { .. }) => {
                // Fall through — might be a "directory" prefix
            }
            Err(e) => return Err(obj_err_to_fs(e)),
        }

        // Check if it's a directory prefix (any objects exist under it)
        let prefix_str = if key.ends_with('/') {
            key.to_string()
        } else {
            format!("{key}/")
        };
        let prefix = ObjectPath::from(prefix_str.as_str());
        let result = self
            .rt
            .block_on(async {
                object_store::ObjectStore::list_with_delimiter(&*self.store, Some(&prefix)).await
            })
            .map_err(obj_err_to_fs)?;

        if !result.objects.is_empty() || !result.common_prefixes.is_empty() {
            return Ok(FileInfo::directory(format!("{}/{key}", self.bucket)));
        }

        Err(FsError::NotFound(format!("{}/{key}", self.bucket)))
    }

    fn rm_file(&self, path: &str) -> FsResult<()> {
        let key = self.strip_protocol(path);
        let obj_path = to_object_path(&key);
        self.rt
            .block_on(self.store.delete(&obj_path))
            .map_err(obj_err_to_fs)
    }

    fn cp_file(&self, src: &str, dst: &str) -> FsResult<()> {
        let src_key = self.strip_protocol(src);
        let dst_key = self.strip_protocol(dst);
        let src_path = to_object_path(&src_key);
        let dst_path = to_object_path(&dst_key);
        self.rt
            .block_on(self.store.copy(&src_path, &dst_path))
            .map_err(obj_err_to_fs)
    }

    fn open(
        &self,
        path: &str,
        mode: OpenMode,
        opts: Option<OpenOptions>,
    ) -> FsResult<Box<dyn FsFile>> {
        let key = self.strip_protocol(path);
        let key = normalise_key(&key).to_string();
        let opts = opts.unwrap_or_default();

        match mode {
            OpenMode::Read => {
                // If a cache_type is specified, use BufferedFile with lazy range-fetch.
                if let Some(cache_type) = opts.cache_type {
                    // Get file size via HEAD
                    let obj_path = to_object_path(&key);
                    let meta = self
                        .rt
                        .block_on(self.store.head(&obj_path))
                        .map_err(obj_err_to_fs)?;
                    let file_size = meta.size as u64;

                    // Build a fetcher that captures an Arc<AmazonS3> clone
                    let store = Arc::clone(&self.store);
                    let key_clone = key.clone();
                    let fetcher: Fetcher = Box::new(move |start, end| {
                        use object_store::GetOptions;
                        use object_store::GetRange;

                        let obj_path = to_object_path(&key_clone);
                        let range = GetRange::Bounded(start..end);
                        let get_opts = GetOptions {
                            range: Some(range),
                            ..Default::default()
                        };

                        // Build a Runtime per fetch — we are inside a closure
                        // that will be called from sync context.
                        let rt = Runtime::new().map_err(|e| FsError::Other(e.to_string()))?;
                        let result = rt
                            .block_on(store.get_opts(&obj_path, get_opts))
                            .map_err(obj_err_to_fs)?;
                        let bytes = rt.block_on(result.bytes()).map_err(obj_err_to_fs)?;
                        Ok(bytes.to_vec())
                    });

                    let blocksize = opts.block_size as u64;
                    let max_blocks = opts.max_blocks;

                    return Ok(Box::new(BufferedFile::new_read(
                        key,
                        fetcher,
                        Some(file_size),
                        cache_type,
                        blocksize,
                        max_blocks,
                    )));
                }

                // Default: eagerly load entire file (backwards compatible)
                let obj_path = to_object_path(&key);
                let result = self
                    .rt
                    .block_on(self.store.get(&obj_path))
                    .map_err(obj_err_to_fs)?;
                let bytes = self.rt.block_on(result.bytes()).map_err(obj_err_to_fs)?;
                Ok(Box::new(S3File::new_read(
                    key,
                    bytes,
                    Arc::clone(&self.store),
                )))
            }
            OpenMode::Write | OpenMode::Exclusive => {
                // If cache_type is set, use BufferedFile for writes
                if opts.cache_type.is_some() {
                    let store = Arc::clone(&self.store);
                    let key_clone = key.clone();
                    let uploader: Uploader = Box::new(move |data: &[u8]| {
                        let obj_path = to_object_path(&key_clone);
                        let payload = PutPayload::from(Bytes::from(data.to_vec()));
                        let rt = Runtime::new().map_err(|e| FsError::Other(e.to_string()))?;
                        rt.block_on(store.put(&obj_path, payload))
                            .map_err(obj_err_to_fs)?;
                        Ok(())
                    });
                    return Ok(Box::new(BufferedFile::new_write(
                        key,
                        uploader,
                        opts.autocommit,
                    )));
                }

                Ok(Box::new(S3File::new_write(
                    key,
                    Arc::clone(&self.store),
                    mode == OpenMode::Exclusive,
                )))
            }
            OpenMode::Append => {
                // Download existing content (if any), then open for write at end
                let obj_path = to_object_path(&key);
                let existing = match self.rt.block_on(self.store.get(&obj_path)) {
                    Ok(result) => {
                        let bytes = self.rt.block_on(result.bytes()).map_err(obj_err_to_fs)?;
                        bytes.to_vec()
                    }
                    Err(object_store::Error::NotFound { .. }) => Vec::new(),
                    Err(e) => return Err(obj_err_to_fs(e)),
                };
                let mut f = S3File::new_write(key, Arc::clone(&self.store), false);
                f.buffer = existing.clone();
                f.cursor = Cursor::new(existing);
                // Seek to end
                let len = f.cursor.get_ref().len() as u64;
                f.cursor.set_position(len);
                Ok(Box::new(f))
            }
        }
    }

    fn mkdir(&self, _path: &str, _create_parents: bool) -> FsResult<()> {
        // S3 doesn't have real directories. No-op.
        Ok(())
    }

    fn rmdir(&self, _path: &str) -> FsResult<()> {
        // S3 doesn't have real directories. No-op.
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// S3File
// ---------------------------------------------------------------------------

/// A file-like object for S3. Data is fully buffered in memory.
///
/// - **Read mode:** data fetched eagerly via GET.
/// - **Write mode:** data collected in a buffer, uploaded on `commit()` / drop.
pub struct S3File {
    key: String,
    cursor: Cursor<Vec<u8>>,
    /// The write buffer (only meaningful in write mode).
    buffer: Vec<u8>,
    store: Arc<AmazonS3>,
    mode: S3FileMode,
    committed: bool,
    discarded: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum S3FileMode {
    Read,
    Write,
    WriteExclusive,
}

impl S3File {
    fn new_read(key: String, data: Bytes, store: Arc<AmazonS3>) -> Self {
        Self {
            key,
            cursor: Cursor::new(data.to_vec()),
            buffer: Vec::new(),
            store,
            mode: S3FileMode::Read,
            committed: false,
            discarded: false,
        }
    }

    fn new_write(key: String, store: Arc<AmazonS3>, exclusive: bool) -> Self {
        Self {
            key,
            cursor: Cursor::new(Vec::new()),
            buffer: Vec::new(),
            store,
            mode: if exclusive {
                S3FileMode::WriteExclusive
            } else {
                S3FileMode::Write
            },
            committed: false,
            discarded: false,
        }
    }

    /// Upload the buffer contents to S3.
    fn upload(&mut self) -> FsResult<()> {
        let data = self.cursor.get_ref().clone();
        let payload = PutPayload::from(Bytes::from(data));
        let obj_path = to_object_path(&self.key);

        let rt = Runtime::new().map_err(|e| FsError::Other(e.to_string()))?;
        rt.block_on(self.store.put(&obj_path, payload))
            .map_err(obj_err_to_fs)?;
        Ok(())
    }
}

impl Read for S3File {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.cursor.read(buf)
    }
}

impl Write for S3File {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.mode == S3FileMode::Read {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "file not opened for writing",
            ));
        }
        self.cursor.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.cursor.flush()
    }
}

impl Seek for S3File {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.cursor.seek(pos)
    }
}

impl FsFile for S3File {
    fn info(&self) -> FsResult<FileInfo> {
        Ok(FileInfo::file(
            self.key.clone(),
            self.cursor.get_ref().len() as u64,
        ))
    }

    fn size(&self) -> FsResult<Option<u64>> {
        Ok(Some(self.cursor.get_ref().len() as u64))
    }

    fn commit(&mut self) -> FsResult<()> {
        if self.mode != S3FileMode::Read && !self.committed && !self.discarded {
            self.upload()?;
            self.committed = true;
        }
        Ok(())
    }

    fn discard(&mut self) -> FsResult<()> {
        self.discarded = true;
        Ok(())
    }
}

impl Drop for S3File {
    fn drop(&mut self) {
        if self.mode != S3FileMode::Read && !self.committed && !self.discarded {
            // Auto-commit on drop (best-effort)
            let _ = self.upload();
            self.committed = true;
        }
    }
}
