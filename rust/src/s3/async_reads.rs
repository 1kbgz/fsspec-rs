//! Native read-only S3 providers for managed async handles.
use super::*;
use crate::async_files::{
    info_bytes, AsyncContext, AsyncFile, AsyncFilesystem, FileProvider, FilesystemProvider,
};
use crate::async_io::{error, Completion, Job, ManagedDispatcher, Value};
use futures_util::{
    future::{AbortHandle, Abortable},
    TryStreamExt,
};
use object_store::{GetOptions, GetRange, ObjectMeta};
use std::future::Future;

type Result<T> = std::result::Result<T, OperationError>;

impl AsyncFilesystem {
    /// Open a read-only native S3 client on the shared managed runtime.
    /// Options match S3Fs::from_options. Metadata addresses objects, not inferred directories.
    pub fn s3(bucket: &str, options: &crate::FsOptions, context: &AsyncContext) -> Result<Self> {
        let (config, options) = crate::registry::s3_options(bucket, options)?;
        let (config, builder, endpoint) = s3_builder(config, options)?;
        let store = builder
            .build()
            .map_err(|e| FsError::InvalidArgument(e.to_string()))?;
        Self::new(
            context,
            Arc::new(ManagedDispatcher),
            Arc::new(NativeFilesystem {
                store: Arc::new(store),
                bucket: config.bucket,
                permits: endpoint_range_permits(&endpoint),
                context: context.clone(),
            }),
        )
    }
}

fn spawn(completion: Completion, work: impl Future<Output = Result<Value>> + Send + 'static) {
    let runtime = match crate::runtime::runtime() {
        Ok(runtime) => runtime,
        Err(error) => return completion.complete(Err(error.into())),
    };
    let (abort, registration) = AbortHandle::new_pair();
    if let Err(error) =
        completion.on_cancel(Arc::new(ManagedDispatcher), Job::new(move || abort.abort()))
    {
        return completion.complete(Err(error));
    }
    runtime.spawn(async move {
        let outcome = Abortable::new(work, registration)
            .await
            .unwrap_or_else(|_| Err(error(ErrorKind::Cancelled, "S3 request cancelled")));
        completion.complete(outcome);
    });
}

fn metadata(meta: &ObjectMeta, name: &str) -> FileInfo {
    let mut extra = HashMap::new();
    if let Some(etag) = &meta.e_tag {
        extra.insert("etag".into(), etag.clone());
    }
    if let Some(version) = &meta.version {
        extra.insert("version".into(), version.clone());
    }
    FileInfo {
        name: name.into(),
        size: meta.size,
        file_type: FileType::File,
        created: None,
        modified: Some(meta.last_modified.into()),
        extra,
    }
}

struct NativeFilesystem {
    store: Arc<dyn ObjectStore>,
    bucket: String,
    permits: Arc<Semaphore>,
    context: AsyncContext,
}
impl NativeFilesystem {
    fn path(&self, path: &str) -> Result<ObjectPath> {
        let prefix = format!("{}/", self.bucket);
        let key = if let Some(path) = path.strip_prefix("s3://") {
            path.strip_prefix(&prefix)
                .ok_or_else(|| error(ErrorKind::Invalid, "S3 URL belongs to another bucket"))?
        } else {
            path.strip_prefix(&prefix).unwrap_or(path)
        };
        if normalise_key(key).is_empty() || path == self.bucket {
            return Err(error(ErrorKind::Invalid, "expected an S3 object path"));
        }
        Ok(to_object_path(key))
    }
}
impl FilesystemProvider for NativeFilesystem {
    fn info(&self, path: &str, completion: Completion) {
        let path = match self.path(path) {
            Ok(path) => path,
            Err(e) => return completion.complete(Err(e)),
        };
        let store = self.store.clone();
        let permits = self.permits.clone();
        spawn(completion, async move {
            let _permit = permits
                .acquire()
                .await
                .expect("endpoint semaphore remains open");
            let meta = store
                .head(&path)
                .await
                .map_err(|e| operation_error(e, false))?;
            Ok(Value::Info(metadata(&meta, &from_object_path(&path))))
        });
    }
    fn open_read(&self, path: &str, completion: Completion) {
        let path = match self.path(path) {
            Ok(path) => path,
            Err(e) => return completion.complete(Err(e)),
        };
        let store = self.store.clone();
        let permits = self.permits.clone();
        let context = self.context.clone();
        spawn(completion, async move {
            let _permit = permits
                .acquire()
                .await
                .expect("endpoint semaphore remains open");
            let meta = store
                .head(&path)
                .await
                .map_err(|e| operation_error(e, false))?;
            let etag = meta
                .e_tag
                .clone()
                .ok_or_else(|| error(ErrorKind::NotSupported, "async S3 reads require an ETag"))?;
            validate_expected_etag(&etag)?;
            let info = metadata(&meta, &from_object_path(&path));
            if info_bytes(&info) > context.limits().max_read_size {
                return Err(error(
                    ErrorKind::Invalid,
                    "object metadata exceeds context read limit",
                ));
            }
            let file = NativeFile {
                store,
                path,
                etag,
                info,
                permits: permits.clone(),
                position: Arc::new(Mutex::new(0)),
            };
            Ok(Value::File(AsyncFile::new(
                &context,
                Arc::new(ManagedDispatcher),
                Arc::new(file),
            )?))
        });
    }
}

struct NativeFile {
    store: Arc<dyn ObjectStore>,
    path: ObjectPath,
    etag: String,
    info: FileInfo,
    permits: Arc<Semaphore>,
    position: Arc<Mutex<u64>>,
}
impl FileProvider for NativeFile {
    fn read_buffer_bytes(&self, bytes: usize) -> Result<usize> {
        bytes
            .checked_mul(2)
            .ok_or_else(|| error(ErrorKind::Invalid, "read size overflow"))
    }
    fn read(&self, max_bytes: usize, completion: Completion) {
        let start = *self.position.lock().unwrap();
        let size = self.info.size;
        if max_bytes == 0 || start >= size {
            return completion.complete(Ok(Value::Bytes(Vec::new())));
        }
        let end = start.saturating_add(max_bytes as u64).min(size);
        let store = self.store.clone();
        let path = self.path.clone();
        let etag = self.etag.clone();
        let permits = self.permits.clone();
        let position = self.position.clone();
        spawn(completion, async move {
            let _permit = permits
                .acquire()
                .await
                .expect("endpoint semaphore remains open");
            let result = store
                .get_opts(
                    &path,
                    GetOptions {
                        if_match: Some(etag.clone()),
                        range: Some(GetRange::Bounded(start..end)),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|e| operation_error(e, true))?;
            if result.meta.e_tag.as_ref() != Some(&etag) || result.meta.size != size {
                return Err(OperationError::Conflict(
                    "S3 object changed since open".into(),
                ));
            }
            if result.range != (start..end) {
                return Err(error(
                    ErrorKind::Corrupt,
                    "S3 response range did not match request",
                ));
            }
            let expected = (end - start) as usize;
            let mut data = Vec::with_capacity(expected);
            let mut stream = result.into_stream();
            while let Some(chunk) = stream
                .try_next()
                .await
                .map_err(|e| operation_error(e, true))?
            {
                if chunk.len() > expected - data.len() {
                    return Err(error(
                        ErrorKind::Corrupt,
                        "S3 response exceeded requested range",
                    ));
                }
                data.extend_from_slice(&chunk);
            }
            if data.len() != expected {
                return Err(error(
                    ErrorKind::Corrupt,
                    "S3 response ended before requested range",
                ));
            }
            *position.lock().unwrap() = end;
            Ok(Value::Bytes(data))
        });
    }
    fn seek(&self, position: SeekFrom, completion: Completion) {
        let mut cursor = self.position.lock().unwrap();
        let position = match position {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::Current(delta) => cursor.checked_add_signed(delta),
            SeekFrom::End(delta) => self.info.size.checked_add_signed(delta),
        };
        completion.complete(match position {
            Some(position) => {
                *cursor = position;
                Ok(Value::Position(position))
            }
            None => Err(error(
                ErrorKind::Invalid,
                "seek outside unsigned file offsets",
            )),
        });
    }
    fn info(&self, completion: Completion) {
        completion.complete(Ok(Value::Info(self.info.clone())));
    }
    fn close(&self, completion: Completion) {
        completion.complete(Ok(Value::Unit));
    }
}
