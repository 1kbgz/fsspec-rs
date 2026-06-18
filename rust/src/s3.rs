//! S3 filesystem backend using the `object_store` crate.
//!
//! Wraps an `object_store` S3 store to implement the [`FileSystem`] trait,
//! providing a sync API via an embedded Tokio runtime.

use std::collections::HashMap;
use std::fmt;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::sync::Arc;

use bytes::Bytes;
use object_store::aws::AmazonS3Builder;
use object_store::path::Path as ObjectPath;
use object_store::{MultipartUpload, ObjectStore, PutMode, PutPayload};
use tokio::runtime::Runtime;

use crate::buffered::BufferedFile;
use crate::caching::{CacheType, Fetcher};
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

const S3_MIN_MULTIPART_PART_SIZE: usize = 5 * 1024 * 1024;

// ---------------------------------------------------------------------------
// S3Fs
// ---------------------------------------------------------------------------

/// Configuration used to construct an [`S3Fs`].
#[derive(Clone)]
pub struct S3Config {
    /// S3 bucket name.
    pub bucket: String,
    /// AWS region (e.g. `"us-east-1"`).
    pub region: Option<String>,
    /// Custom endpoint URL (for MinIO / Backblaze B2 / LocalStack).
    ///
    /// Plain `http://` endpoints automatically enable object_store's
    /// `allow_http` setting.
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

impl fmt::Debug for S3Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Config")
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("endpoint_url", &self.endpoint_url)
            .field(
                "access_key_id",
                &self.access_key_id.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "secret_access_key",
                &self.secret_access_key.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "<redacted>"),
            )
            .field("anon", &self.anon)
            .field(
                "virtual_hosted_style_request",
                &self.virtual_hosted_style_request,
            )
            .finish()
    }
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

/// Sync S3 filesystem backed by an `object_store` S3 store.
///
/// All async `object_store` calls are executed via an embedded tokio runtime.
pub struct S3Fs {
    store: Arc<dyn ObjectStore>,
    rt: Arc<Runtime>,
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
            rt: Arc::new(rt),
            bucket: cfg.bucket,
        })
    }

    #[cfg(test)]
    fn from_object_store(bucket: impl Into<String>, store: Arc<dyn ObjectStore>) -> FsResult<Self> {
        let rt = Runtime::new().map_err(|e| FsError::Other(e.to_string()))?;
        Ok(Self {
            store,
            rt: Arc::new(rt),
            bucket: bucket.into(),
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
        put_object(&self.rt, self.store.as_ref(), &obj_path, payload, false)?;
        Ok(())
    }
}

fn put_object<S>(
    rt: &Runtime,
    store: &S,
    obj_path: &ObjectPath,
    payload: PutPayload,
    exclusive: bool,
) -> FsResult<()>
where
    S: ObjectStore + ?Sized,
{
    if exclusive {
        rt.block_on(store.put_opts(obj_path, payload, PutMode::Create.into()))
            .map_err(obj_err_to_fs)?;
    } else {
        rt.block_on(store.put(obj_path, payload))
            .map_err(obj_err_to_fs)?;
    }
    Ok(())
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

    fn ls(&self, path: &str, _detail: bool) -> FsResult<Vec<FileInfo>> {
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
                    let file_size = meta.size;

                    // Build a fetcher that captures an ObjectStore clone.
                    let store = Arc::clone(&self.store);
                    let rt = Arc::clone(&self.rt);
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

                let obj_path = to_object_path(&key);
                let meta = self
                    .rt
                    .block_on(self.store.head(&obj_path))
                    .map_err(obj_err_to_fs)?;
                let file_size = meta.size;

                let store = Arc::clone(&self.store);
                let rt = Arc::clone(&self.rt);
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

                    let result = rt
                        .block_on(store.get_opts(&obj_path, get_opts))
                        .map_err(obj_err_to_fs)?;
                    let bytes = rt.block_on(result.bytes()).map_err(obj_err_to_fs)?;
                    Ok(bytes.to_vec())
                });

                Ok(Box::new(BufferedFile::new_read(
                    key,
                    fetcher,
                    Some(file_size),
                    CacheType::ReadAhead,
                    opts.block_size as u64,
                    opts.max_blocks,
                )))
            }
            OpenMode::Write | OpenMode::Exclusive => {
                if mode == OpenMode::Exclusive {
                    return Ok(Box::new(S3File::new_exclusive(
                        key,
                        Arc::clone(&self.store),
                        Arc::clone(&self.rt),
                    )));
                }

                Ok(Box::new(S3File::new_write(
                    key,
                    Arc::clone(&self.store),
                    Arc::clone(&self.rt),
                    opts.block_size,
                )))
            }
            OpenMode::Append => {
                let obj_path = to_object_path(&key);
                let existing_size = match self.rt.block_on(self.store.head(&obj_path)) {
                    Ok(meta) => meta.size,
                    Err(object_store::Error::NotFound { .. }) => 0,
                    Err(e) => return Err(obj_err_to_fs(e)),
                };
                Ok(Box::new(S3File::new_append(
                    key,
                    Arc::clone(&self.store),
                    Arc::clone(&self.rt),
                    opts.block_size,
                    existing_size,
                )))
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

/// A write-mode file-like object for S3.
pub struct S3File {
    inner: S3FileInner,
}

enum S3FileInner {
    Buffered(Box<S3BufferedWriteFile>),
    Streaming(Box<S3StreamingWriteFile>),
}

impl S3File {
    fn new_exclusive(key: String, store: Arc<dyn ObjectStore>, rt: Arc<Runtime>) -> Self {
        Self {
            inner: S3FileInner::Buffered(Box::new(S3BufferedWriteFile::new(key, store, rt))),
        }
    }

    fn new_write(
        key: String,
        store: Arc<dyn ObjectStore>,
        rt: Arc<Runtime>,
        block_size: usize,
    ) -> Self {
        Self {
            inner: S3FileInner::Streaming(Box::new(S3StreamingWriteFile::new_write(
                key, store, rt, block_size,
            ))),
        }
    }

    fn new_append(
        key: String,
        store: Arc<dyn ObjectStore>,
        rt: Arc<Runtime>,
        block_size: usize,
        existing_size: u64,
    ) -> Self {
        Self {
            inner: S3FileInner::Streaming(Box::new(S3StreamingWriteFile::new_append(
                key,
                store,
                rt,
                block_size,
                existing_size,
            ))),
        }
    }
}

impl Read for S3File {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match &mut self.inner {
            S3FileInner::Buffered(file) => file.read(buf),
            S3FileInner::Streaming(file) => file.read(buf),
        }
    }
}

impl Write for S3File {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match &mut self.inner {
            S3FileInner::Buffered(file) => file.write(buf),
            S3FileInner::Streaming(file) => file.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match &mut self.inner {
            S3FileInner::Buffered(file) => file.flush(),
            S3FileInner::Streaming(file) => file.flush(),
        }
    }
}

impl Seek for S3File {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match &mut self.inner {
            S3FileInner::Buffered(file) => file.seek(pos),
            S3FileInner::Streaming(file) => file.seek(pos),
        }
    }
}

impl FsFile for S3File {
    fn info(&self) -> FsResult<FileInfo> {
        match &self.inner {
            S3FileInner::Buffered(file) => file.info(),
            S3FileInner::Streaming(file) => file.info(),
        }
    }

    fn size(&self) -> FsResult<Option<u64>> {
        match &self.inner {
            S3FileInner::Buffered(file) => file.size(),
            S3FileInner::Streaming(file) => file.size(),
        }
    }

    fn commit(&mut self) -> FsResult<()> {
        match &mut self.inner {
            S3FileInner::Buffered(file) => file.commit(),
            S3FileInner::Streaming(file) => file.commit(),
        }
    }

    fn discard(&mut self) -> FsResult<()> {
        match &mut self.inner {
            S3FileInner::Buffered(file) => file.discard(),
            S3FileInner::Streaming(file) => file.discard(),
        }
    }
}

struct S3BufferedWriteFile {
    key: String,
    cursor: Cursor<Vec<u8>>,
    store: Arc<dyn ObjectStore>,
    rt: Arc<Runtime>,
    committed: bool,
    discarded: bool,
}

impl S3BufferedWriteFile {
    fn new(key: String, store: Arc<dyn ObjectStore>, rt: Arc<Runtime>) -> Self {
        Self {
            key,
            cursor: Cursor::new(Vec::new()),
            store,
            rt,
            committed: false,
            discarded: false,
        }
    }

    fn upload(&mut self) -> FsResult<()> {
        let data = self.cursor.get_ref().clone();
        let payload = PutPayload::from(Bytes::from(data));
        let obj_path = to_object_path(&self.key);

        put_object(&self.rt, self.store.as_ref(), &obj_path, payload, true)
    }
}

impl Read for S3BufferedWriteFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.cursor.read(buf)
    }
}

impl Write for S3BufferedWriteFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.cursor.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.cursor.flush()
    }
}

impl Seek for S3BufferedWriteFile {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.cursor.seek(pos)
    }
}

impl FsFile for S3BufferedWriteFile {
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
        if !self.committed && !self.discarded {
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

impl Drop for S3BufferedWriteFile {
    fn drop(&mut self) {
        if !self.committed && !self.discarded {
            // Auto-commit on drop (best-effort)
            let _ = self.upload();
            self.committed = true;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum S3StreamingWriteMode {
    Write,
    Append,
}

struct S3StreamingWriteFile {
    key: String,
    store: Arc<dyn ObjectStore>,
    rt: Arc<Runtime>,
    mode: S3StreamingWriteMode,
    part_size: usize,
    buffer: Vec<u8>,
    upload: Option<Box<dyn MultipartUpload>>,
    existing_size: u64,
    copied_existing: bool,
    position: u64,
    size: u64,
    dirty: bool,
    committed: bool,
    discarded: bool,
}

impl S3StreamingWriteFile {
    fn new_write(
        key: String,
        store: Arc<dyn ObjectStore>,
        rt: Arc<Runtime>,
        block_size: usize,
    ) -> Self {
        Self {
            key,
            store,
            rt,
            mode: S3StreamingWriteMode::Write,
            part_size: block_size.max(S3_MIN_MULTIPART_PART_SIZE),
            buffer: Vec::new(),
            upload: None,
            existing_size: 0,
            copied_existing: true,
            position: 0,
            size: 0,
            dirty: false,
            committed: false,
            discarded: false,
        }
    }

    fn new_append(
        key: String,
        store: Arc<dyn ObjectStore>,
        rt: Arc<Runtime>,
        block_size: usize,
        existing_size: u64,
    ) -> Self {
        Self {
            key,
            store,
            rt,
            mode: S3StreamingWriteMode::Append,
            part_size: block_size.max(S3_MIN_MULTIPART_PART_SIZE),
            buffer: Vec::new(),
            upload: None,
            existing_size,
            copied_existing: existing_size == 0,
            position: existing_size,
            size: existing_size,
            dirty: false,
            committed: false,
            discarded: false,
        }
    }

    fn io_error(err: FsError) -> std::io::Error {
        std::io::Error::other(err.to_string())
    }

    fn object_path(&self) -> ObjectPath {
        to_object_path(&self.key)
    }

    fn fetch_range(&self, start: u64, end: u64) -> FsResult<Vec<u8>> {
        use object_store::GetOptions;
        use object_store::GetRange;

        let opts = GetOptions {
            range: Some(GetRange::Bounded(start..end)),
            ..Default::default()
        };
        let result = self
            .rt
            .block_on(self.store.get_opts(&self.object_path(), opts))
            .map_err(obj_err_to_fs)?;
        let bytes = self.rt.block_on(result.bytes()).map_err(obj_err_to_fs)?;
        Ok(bytes.to_vec())
    }

    fn ensure_upload(&mut self) -> FsResult<&mut Box<dyn MultipartUpload>> {
        if self.upload.is_none() {
            let upload = self
                .rt
                .block_on(self.store.put_multipart(&self.object_path()))
                .map_err(obj_err_to_fs)?;
            self.upload = Some(upload);
        }
        Ok(self.upload.as_mut().expect("upload initialized"))
    }

    fn upload_part(&mut self, data: Vec<u8>) -> FsResult<()> {
        let rt = Arc::clone(&self.rt);
        let upload = self.ensure_upload()?;
        rt.block_on(upload.put_part(PutPayload::from(Bytes::from(data))))
            .map_err(obj_err_to_fs)
    }

    fn flush_buffer_part(&mut self) -> FsResult<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let part = std::mem::take(&mut self.buffer);
        self.upload_part(part)
    }

    fn append_bytes(&mut self, mut data: &[u8]) -> FsResult<()> {
        while !data.is_empty() {
            if self.buffer.len() == self.part_size {
                self.flush_buffer_part()?;
            }
            let room = self.part_size - self.buffer.len();
            let take = data.len().min(room);
            self.buffer.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buffer.len() == self.part_size && !data.is_empty() {
                self.flush_buffer_part()?;
            }
        }
        Ok(())
    }

    fn copy_existing_for_append(&mut self) -> FsResult<()> {
        if self.copied_existing {
            return Ok(());
        }

        let mut offset = 0;
        while self.existing_size - offset > self.part_size as u64 {
            let end = offset + self.part_size as u64;
            let data = self.fetch_range(offset, end)?;
            self.upload_part(data)?;
            offset = end;
        }

        if offset < self.existing_size {
            let data = self.fetch_range(offset, self.existing_size)?;
            self.append_bytes(&data)?;
        }

        self.copied_existing = true;
        Ok(())
    }

    fn abort_upload(&mut self) -> FsResult<()> {
        if let Some(mut upload) = self.upload.take() {
            self.rt.block_on(upload.abort()).map_err(obj_err_to_fs)?;
        }
        Ok(())
    }
}

impl Read for S3StreamingWriteFile {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "file not opened for reading",
        ))
    }
}

impl Write for S3StreamingWriteFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.position != self.size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "S3 streaming writes only support writing at the end of the file",
            ));
        }
        if self.mode == S3StreamingWriteMode::Append && !self.copied_existing {
            self.copy_existing_for_append().map_err(Self::io_error)?;
        }
        self.append_bytes(buf).map_err(Self::io_error)?;
        self.position += buf.len() as u64;
        self.size += buf.len() as u64;
        self.dirty = true;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Seek for S3StreamingWriteFile {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let next = match pos {
            SeekFrom::Start(pos) => pos,
            SeekFrom::Current(0) => self.position,
            SeekFrom::End(0) => self.size,
            SeekFrom::Current(offset) if offset > 0 => self.position + offset as u64,
            SeekFrom::End(offset) if offset > 0 => self.size + offset as u64,
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "S3 streaming writes do not support seeking before the current end",
                ))
            }
        };

        if next != self.size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "S3 streaming writes only support seeking to the current end",
            ));
        }
        self.position = next;
        Ok(self.position)
    }
}

impl FsFile for S3StreamingWriteFile {
    fn info(&self) -> FsResult<FileInfo> {
        Ok(FileInfo::file(self.key.clone(), self.size))
    }

    fn size(&self) -> FsResult<Option<u64>> {
        Ok(Some(self.size))
    }

    fn commit(&mut self) -> FsResult<()> {
        if self.committed || self.discarded {
            return Ok(());
        }
        if self.mode == S3StreamingWriteMode::Append && !self.dirty {
            self.committed = true;
            return Ok(());
        }
        if self.mode == S3StreamingWriteMode::Append && !self.copied_existing {
            self.copy_existing_for_append()?;
        }

        if self.upload.is_none() {
            put_object(
                &self.rt,
                self.store.as_ref(),
                &self.object_path(),
                PutPayload::from(Bytes::from(self.buffer.clone())),
                false,
            )?;
        } else {
            if !self.buffer.is_empty() {
                self.flush_buffer_part()?;
            }
            if let Some(upload) = self.upload.as_mut() {
                self.rt.block_on(upload.complete()).map_err(obj_err_to_fs)?;
            }
            self.upload = None;
        }

        self.buffer.clear();
        self.committed = true;
        Ok(())
    }

    fn discard(&mut self) -> FsResult<()> {
        self.abort_upload()?;
        self.buffer.clear();
        self.discarded = true;
        Ok(())
    }
}

impl Drop for S3StreamingWriteFile {
    fn drop(&mut self) {
        if self.committed || self.discarded {
            return;
        }
        if self.upload.is_some() {
            // Multipart uploads cannot report completion errors from Drop. Abort
            // unfinished parts instead; explicit close()/commit() is the
            // supported path for durable writes and surfaced upload errors.
            let _ = self.abort_upload();
        } else if self.dirty && !self.buffer.is_empty() {
            let _ = put_object(
                &self.rt,
                self.store.as_ref(),
                &self.object_path(),
                PutPayload::from(Bytes::from(self.buffer.clone())),
                false,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::memory::InMemory;
    use std::io::{Read, Write};

    #[test]
    fn put_object_honors_exclusive_create() {
        let rt = Runtime::new().unwrap();
        let store = InMemory::new();
        let path = ObjectPath::from("existing.txt");

        put_object(
            &rt,
            &store,
            &path,
            PutPayload::from(Bytes::from_static(b"first")),
            false,
        )
        .unwrap();

        let err = put_object(
            &rt,
            &store,
            &path,
            PutPayload::from(Bytes::from_static(b"second")),
            true,
        )
        .unwrap_err();
        assert!(matches!(err, FsError::AlreadyExists(_)));

        put_object(
            &rt,
            &store,
            &ObjectPath::from("new.txt"),
            PutPayload::from(Bytes::from_static(b"new")),
            true,
        )
        .unwrap();
    }

    fn memory_fs() -> S3Fs {
        S3Fs::from_object_store("bucket", Arc::new(InMemory::new())).unwrap()
    }

    fn seed_object(fs: &S3Fs, key: &str, data: &[u8]) {
        let path = to_object_path(key);
        put_object(
            &fs.rt,
            fs.store.as_ref(),
            &path,
            PutPayload::from(Bytes::from(data.to_vec())),
            false,
        )
        .unwrap();
    }

    #[test]
    fn in_memory_s3fs_lists_and_infos_objects() {
        let fs = memory_fs();
        seed_object(&fs, "dir/file.txt", b"hello");
        seed_object(&fs, "dir/nested/file.txt", b"world");

        let info = fs.info("s3://bucket/dir/file.txt").unwrap();
        assert!(info.is_file());
        assert_eq!(info.size, 5);

        let names: Vec<String> = fs
            .ls("s3://bucket/dir", true)
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(names, vec!["bucket/dir/file.txt", "bucket/dir/nested"]);
    }

    #[test]
    fn default_read_open_uses_object_store_without_eager_s3file() {
        let fs = memory_fs();
        seed_object(&fs, "data.txt", b"abcdef");

        let mut file = fs
            .open("s3://bucket/data.txt", OpenMode::Read, None)
            .unwrap();
        assert_eq!(file.size().unwrap(), Some(6));

        let mut first = [0; 3];
        file.read_exact(&mut first).unwrap();
        assert_eq!(&first, b"abc");

        let mut rest = Vec::new();
        file.read_to_end(&mut rest).unwrap();
        assert_eq!(rest, b"def");
    }

    #[test]
    fn in_memory_s3fs_write_commit_uploads_object() {
        let fs = memory_fs();
        let mut file = fs.open("out.txt", OpenMode::Write, None).unwrap();
        file.write_all(b"written").unwrap();
        file.commit().unwrap();

        assert_eq!(fs.cat_file("out.txt", None, None).unwrap(), b"written");
    }

    #[test]
    fn in_memory_s3fs_large_write_uses_multipart_path() {
        let fs = memory_fs();
        let mut data = vec![b'a'; S3_MIN_MULTIPART_PART_SIZE + 17];
        data[S3_MIN_MULTIPART_PART_SIZE] = b'b';

        let mut file = fs.open("large.txt", OpenMode::Write, None).unwrap();
        file.write_all(&data).unwrap();
        file.commit().unwrap();

        let written = fs.cat_file("large.txt", None, None).unwrap();
        assert_eq!(written.len(), data.len());
        assert_eq!(written[0], b'a');
        assert_eq!(written[S3_MIN_MULTIPART_PART_SIZE], b'b');
        assert_eq!(written.last(), Some(&b'a'));
    }

    #[test]
    fn in_memory_s3fs_append_preserves_existing_bytes() {
        let fs = memory_fs();
        seed_object(&fs, "append.txt", b"hello");

        let mut file = fs.open("append.txt", OpenMode::Append, None).unwrap();
        file.write_all(b" world").unwrap();
        file.commit().unwrap();

        assert_eq!(
            fs.cat_file("append.txt", None, None).unwrap(),
            b"hello world"
        );
    }

    #[test]
    fn in_memory_s3fs_large_append_rewrites_without_whole_object_buffer() {
        let fs = memory_fs();
        let mut existing = vec![b'x'; S3_MIN_MULTIPART_PART_SIZE + 9];
        existing[S3_MIN_MULTIPART_PART_SIZE] = b'y';
        seed_object(&fs, "large-append.txt", &existing);

        let mut file = fs.open("large-append.txt", OpenMode::Append, None).unwrap();
        file.write_all(b" tail").unwrap();
        file.commit().unwrap();

        let written = fs.cat_file("large-append.txt", None, None).unwrap();
        assert_eq!(written.len(), existing.len() + 5);
        assert_eq!(&written[..existing.len()], existing.as_slice());
        assert_eq!(&written[existing.len()..], b" tail");
    }
}
