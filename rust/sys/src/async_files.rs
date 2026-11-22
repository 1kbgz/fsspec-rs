//! Owned read-only async handles from the shared library.
use crate::{
    async_io::{check, Operation, Value},
    ffi,
    operations::operation_error,
    types::*,
};
pub use fsspec_rs::async_files::Limits;
use fsspec_rs::{AsyncFsFile, FileInfo, IoFuture, OperationError};
use std::io::SeekFrom;
use std::mem::size_of;
use std::ptr;
use std::time::Duration;

type Result<T> = std::result::Result<T, OperationError>;
fn options(timeout: Option<Duration>) -> Result<fsspec_async_options> {
    let timeout_ns = match timeout {
        None => u64::MAX,
        Some(duration) if duration.as_nanos() < u64::MAX as u128 => duration.as_nanos() as u64,
        Some(_) => return Err(operation_error(7, "timeout too large".into())),
    };
    Ok(fsspec_async_options {
        struct_size: size_of::<fsspec_async_options>(),
        timeout_ns,
    })
}
unsafe fn start(
    timeout: Option<Duration>,
    call: impl FnOnce(
        *const fsspec_async_options,
        *mut *mut fsspec_operation,
        *mut *mut fsspec_async_error,
    ) -> i32,
) -> Result<Operation> {
    let options = options(timeout)?;
    let mut operation = ptr::null_mut();
    let mut error = ptr::null_mut();
    let status = call(&options, &mut operation, &mut error);
    check(status, error)?;
    Operation::from_raw(operation)
}

impl AsyncContext {
    pub fn new(limits: Limits) -> Result<Self> {
        let options = fsspec_async_context_options {
            struct_size: size_of::<fsspec_async_context_options>(),
            max_operations: limits.max_operations,
            max_buffered_bytes: limits.max_buffered_bytes as u64,
            max_read_size: limits.max_read_size,
        };
        let mut handle = ptr::null_mut();
        let mut error = ptr::null_mut();
        unsafe {
            let status = ffi::fsspec_async_context_create(&options, &mut handle, &mut error);
            check(status, error)?;
            Ok(Self(handle))
        }
    }
}
impl AsyncFile {
    pub fn read(&self, max_bytes: usize, timeout: Option<Duration>) -> Result<Operation> {
        unsafe {
            start(timeout, |options, out, error| {
                ffi::fsspec_async_file_read(self.0, max_bytes, options, out, error)
            })
        }
    }
    pub fn seek(&self, position: SeekFrom, timeout: Option<Duration>) -> Result<Operation> {
        let (offset, whence) = match position {
            SeekFrom::Start(value) => (
                i64::try_from(value).map_err(|_| {
                    operation_error(7, "start offset exceeds C vtable range".into())
                })?,
                0,
            ),
            SeekFrom::Current(value) => (value, 1),
            SeekFrom::End(value) => (value, 2),
        };
        unsafe {
            start(timeout, |options, out, error| {
                ffi::fsspec_async_file_seek(self.0, offset, whence, options, out, error)
            })
        }
    }
    pub fn info(&self, timeout: Option<Duration>) -> Result<Operation> {
        unsafe {
            start(timeout, |options, out, error| {
                ffi::fsspec_async_file_info(self.0, options, out, error)
            })
        }
    }
}
impl AsyncFilesystem {
    /// Construct native read-only S3 handles using the shared managed runtime.
    /// None selects a new context with default limits.
    pub fn s3(
        bucket: &str,
        options: &fsspec_rs::FsOptions,
        context: Option<&AsyncContext>,
    ) -> Result<Self> {
        let options = crate::Options::new(options)?;
        let mut handle = ptr::null_mut();
        let mut error = ptr::null_mut();
        unsafe {
            let status = ffi::fsspec_async_s3_open(
                bucket.as_ptr().cast(),
                bucket.len(),
                options.0,
                context.map_or(ptr::null(), |context| context.0),
                &mut handle,
                &mut error,
            );
            check(status, error)?;
            Ok(Self::from_raw(handle))
        }
    }
    pub fn info(&self, path: &str, timeout: Option<Duration>) -> Result<Operation> {
        unsafe {
            start(timeout, |options, out, error| {
                ffi::fsspec_async_fs_info(
                    self.0,
                    path.as_ptr().cast(),
                    path.len(),
                    options,
                    out,
                    error,
                )
            })
        }
    }
    pub fn open_read(&self, path: &str, timeout: Option<Duration>) -> Result<Operation> {
        unsafe {
            start(timeout, |options, out, error| {
                ffi::fsspec_async_fs_open_read(
                    self.0,
                    path.as_ptr().cast(),
                    path.len(),
                    options,
                    out,
                    error,
                )
            })
        }
    }
}

impl AsyncFsFile for AsyncFile {
    fn read(&mut self, max_bytes: usize) -> IoFuture<'_, Vec<u8>> {
        Box::pin(async move {
            let mut operation = AsyncFile::read(self, max_bytes, None)?;
            let value = (&mut operation).await?;
            operation.drain()?.await?;
            match value {
                Value::Bytes(bytes) => Ok(bytes),
                _ => unreachable!(),
            }
        })
    }
    fn seek(&mut self, position: SeekFrom) -> IoFuture<'_, u64> {
        Box::pin(async move {
            let mut operation = AsyncFile::seek(self, position, None)?;
            let value = (&mut operation).await?;
            operation.drain()?.await?;
            match value {
                Value::Position(position) => Ok(position),
                _ => unreachable!(),
            }
        })
    }
    fn info(&self) -> IoFuture<'_, FileInfo> {
        Box::pin(async move {
            let mut operation = AsyncFile::info(self, None)?;
            let value = (&mut operation).await?;
            operation.drain()?.await?;
            match value {
                Value::Info(info) => Ok(info),
                _ => unreachable!(),
            }
        })
    }
    fn close(&mut self) -> IoFuture<'_, ()> {
        Box::pin(async move { AsyncFile::close(self, None)?.await.map(|_| ()) })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct AsyncContext(*mut fsspec_async_context);
// Library handles synchronize state; each wrapper owns one independent reference.
unsafe impl Send for AsyncContext {}
unsafe impl Sync for AsyncContext {}
impl AsyncContext {
    /// Takes one live owned reference. Library and provider code must stay loaded
    /// until close drains and all callback/handle references are released.
    pub unsafe fn from_raw(handle: *mut fsspec_async_context) -> Self {
        assert!(!handle.is_null());
        Self(handle)
    }
    pub fn as_raw(&self) -> *mut fsspec_async_context {
        self.0
    }
    pub fn into_raw(self) -> *mut fsspec_async_context {
        let handle = self.0;
        std::mem::forget(self);
        handle
    }
    pub fn close(&self, timeout: Option<Duration>) -> Result<Operation> {
        unsafe {
            start(timeout, |options, out, error| {
                ffi::fsspec_async_context_close(self.0, options, out, error)
            })
        }
    }
}
impl Clone for AsyncContext {
    fn clone(&self) -> Self {
        unsafe { Self(ffi::fsspec_async_context_retain(self.0)) }
    }
}
impl Drop for AsyncContext {
    fn drop(&mut self) {
        unsafe { ffi::fsspec_async_context_release(self.0) }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct AsyncFile(*mut fsspec_async_file);
// Library handles synchronize state; each wrapper owns one independent reference.
unsafe impl Send for AsyncFile {}
unsafe impl Sync for AsyncFile {}
impl AsyncFile {
    /// Takes one live owned reference. Library and provider code must stay loaded
    /// until close drains and all callback/handle references are released.
    pub unsafe fn from_raw(handle: *mut fsspec_async_file) -> Self {
        assert!(!handle.is_null());
        Self(handle)
    }
    pub fn as_raw(&self) -> *mut fsspec_async_file {
        self.0
    }
    pub fn into_raw(self) -> *mut fsspec_async_file {
        let handle = self.0;
        std::mem::forget(self);
        handle
    }
    pub fn close(&self, timeout: Option<Duration>) -> Result<Operation> {
        unsafe {
            start(timeout, |options, out, error| {
                ffi::fsspec_async_file_close(self.0, options, out, error)
            })
        }
    }
}
impl Clone for AsyncFile {
    fn clone(&self) -> Self {
        unsafe { Self(ffi::fsspec_async_file_retain(self.0)) }
    }
}
impl Drop for AsyncFile {
    fn drop(&mut self) {
        unsafe { ffi::fsspec_async_file_release(self.0) }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct AsyncFilesystem(*mut fsspec_async_fs);
// Library handles synchronize state; each wrapper owns one independent reference.
unsafe impl Send for AsyncFilesystem {}
unsafe impl Sync for AsyncFilesystem {}
impl AsyncFilesystem {
    /// Takes one live owned reference. Library and provider code must stay loaded
    /// until close drains and all callback/handle references are released.
    pub unsafe fn from_raw(handle: *mut fsspec_async_fs) -> Self {
        assert!(!handle.is_null());
        Self(handle)
    }
    pub fn as_raw(&self) -> *mut fsspec_async_fs {
        self.0
    }
    pub fn into_raw(self) -> *mut fsspec_async_fs {
        let handle = self.0;
        std::mem::forget(self);
        handle
    }
    pub fn close(&self, timeout: Option<Duration>) -> Result<Operation> {
        unsafe {
            start(timeout, |options, out, error| {
                ffi::fsspec_async_fs_close(self.0, options, out, error)
            })
        }
    }
}
impl Clone for AsyncFilesystem {
    fn clone(&self) -> Self {
        unsafe { Self(ffi::fsspec_async_fs_retain(self.0)) }
    }
}
impl Drop for AsyncFilesystem {
    fn drop(&mut self) {
        unsafe { ffi::fsspec_async_fs_release(self.0) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::async_io::Completion;
    use fsspec as _;
    use std::future::Future;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Wake};
    use std::time::Instant;

    #[derive(Default)]
    struct Provider {
        lease: Mutex<Option<Completion>>,
    }
    unsafe extern "C" fn read(
        data: *mut std::ffi::c_void,
        _: usize,
        completion: *mut fsspec_completion,
    ) {
        let state = &*data.cast::<Arc<Provider>>();
        let completion = Completion::from_raw(completion);
        *state.lease.lock().unwrap() = Some(completion.clone());
        completion.complete(Ok(Value::Bytes(vec![42])));
    }
    unsafe extern "C" fn info(_: *mut std::ffi::c_void, completion: *mut fsspec_completion) {
        Completion::from_raw(completion).complete(Ok(Value::Info(FileInfo::file("", 1))));
    }
    unsafe extern "C" fn close(_: *mut std::ffi::c_void, completion: *mut fsspec_completion) {
        Completion::from_raw(completion).complete(Ok(Value::Unit));
    }
    unsafe extern "C" fn release(data: *mut std::ffi::c_void) {
        drop(Box::from_raw(data.cast::<Arc<Provider>>()));
    }
    struct Thread(std::thread::Thread);
    impl Wake for Thread {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    fn wait<T>(future: impl Future<Output = T>) -> T {
        let mut future = std::pin::pin!(future);
        let waker = Arc::new(Thread(std::thread::current())).into();
        let mut context = Context::from_waker(&waker);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(Instant::now() < deadline);
            if let Poll::Ready(result) = future.as_mut().poll(&mut context) {
                return result;
            }
            std::thread::park_timeout(deadline.saturating_duration_since(Instant::now()));
        }
    }
    #[test]
    fn trait_reads_across_c_wait_for_cleanup_before_next_read() {
        let ctx = AsyncContext::new(Limits {
            max_operations: 1,
            max_buffered_bytes: 1,
            max_read_size: 1,
        })
        .unwrap();
        let state = Arc::new(Provider::default());
        let data = Box::into_raw(Box::new(state.clone())).cast();
        let vtable = fsspec_async_file_vtable {
            struct_size: size_of::<fsspec_async_file_vtable>(),
            abi_version: 1,
            read: Some(read),
            seek: None,
            info: Some(info),
            close: Some(close),
            release: Some(release),
        };
        let mut handle = ptr::null_mut();
        unsafe {
            assert_eq!(
                ffi::fsspec_async_file_from_vtable(
                    ctx.as_raw(),
                    ptr::null(),
                    &vtable,
                    data,
                    &mut handle,
                    ptr::null_mut()
                ),
                0
            );
        }
        let mut file = unsafe { AsyncFile::from_raw(handle) };
        for _ in 0..2 {
            let state = state.clone();
            let cleanup = std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    assert!(Instant::now() < deadline);
                    let lease = state.lease.lock().unwrap().take();
                    if let Some(lease) = lease {
                        std::thread::sleep(Duration::from_millis(10));
                        drop(lease);
                        break;
                    }
                    std::thread::yield_now();
                }
            });
            assert_eq!(wait(AsyncFsFile::read(&mut file, 1)).unwrap(), vec![42]);
            cleanup.join().unwrap();
        }
        wait(ctx.close(None).unwrap()).unwrap();
    }
    #[test]
    fn native_s3_constructor_uses_owned_errors_and_contexts() {
        let options = fsspec_rs::FsOptions::from([("anon".into(), "true".into())]);
        assert_eq!(
            AsyncFilesystem::s3("bad/bucket", &options, None)
                .unwrap_err()
                .kind(),
            fsspec_rs::ErrorKind::Invalid
        );
        let context = AsyncContext::new(Limits::default()).unwrap();
        let fs = AsyncFilesystem::s3("bucket", &options, Some(&context)).unwrap();
        drop(context);
        wait(fs.close(None).unwrap()).unwrap();
        let fs = AsyncFilesystem::s3("bucket", &options, None).unwrap();
        wait(fs.close(None).unwrap()).unwrap();
    }
}
