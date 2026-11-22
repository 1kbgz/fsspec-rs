//! Rust futures over the shared library's async C ABI.
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::pin::Pin;
use std::ptr;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

pub use fsspec_rs::async_io::Kind;
pub type Outcome = Result<Value, OperationError>;
#[derive(Debug, PartialEq, Eq)]
pub enum Value {
    Unit,
    Bytes(Vec<u8>),
    Info(fsspec_rs::FileInfo),
    File(crate::async_files::AsyncFile),
    Position(u64),
}
use fsspec_rs::OperationError;

use crate::{
    ffi,
    operations::{operation_error, operation_status},
    types::*,
};

struct Notification(Mutex<Option<Waker>>);

unsafe extern "C" fn ready(data: *mut std::ffi::c_void, _: *const fsspec_operation) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let waker = (&*data.cast::<Notification>()).0.lock().unwrap().take();
        if let Some(waker) = waker {
            waker.wake();
        }
    }));
}

unsafe extern "C" fn release(data: *mut std::ffi::c_void) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        drop(Arc::from_raw(data.cast::<Notification>()));
    }));
}

pub(crate) unsafe fn check(
    status: i32,
    error: *mut fsspec_async_error,
) -> Result<(), OperationError> {
    let message = if error.is_null() {
        "async C operation failed".into()
    } else {
        let message = (*error).message.as_str().into_owned();
        ffi::fsspec_async_error_free(error);
        message
    };
    if status == 0 {
        Ok(())
    } else {
        Err(decode_error(status, message))
    }
}

fn decode_error(status: i32, message: String) -> OperationError {
    let descriptor = operation_error(status, String::new());
    OperationError::Detailed {
        kind: descriptor.kind(),
        retry_safe: descriptor.retry_safe(),
        message,
    }
}

/// A single consumer Future. Drop requests cancellation and releases its handle.
/// Producers still own cleanup leases; cancellation does not wait for cleanup.
pub struct Operation {
    handle: *mut fsspec_operation,
    notification: Arc<Notification>,
}

// libfsspec serializes operation state. Only this Future takes the result.
unsafe impl Send for Operation {}
unsafe impl Sync for Operation {}

impl Operation {
    pub fn new(
        kind: Kind,
        max_bytes: usize,
        timeout: Option<Duration>,
    ) -> Result<(Self, Completion), OperationError> {
        let timeout = match timeout {
            None => u64::MAX,
            Some(duration) if duration.as_nanos() < u64::MAX as u128 => duration.as_nanos() as u64,
            Some(_) => return Err(operation_error(7, "timeout is too large".into())),
        };
        unsafe {
            let mut handle = ptr::null_mut();
            let mut producer = ptr::null_mut();
            let mut error = ptr::null_mut();
            let status = ffi::fsspec_operation_create(
                match kind {
                    Kind::Unit => 0,
                    Kind::Bytes => 1,
                    Kind::Info => 2,
                    Kind::File => 3,
                    Kind::Position => 4,
                },
                max_bytes,
                timeout,
                &mut handle,
                &mut producer,
                &mut error,
            );
            check(status, error)?;
            let completion = Completion(producer);
            match Self::from_raw(handle) {
                Ok(operation) => Ok((operation, completion)),
                Err(error) => Err(error),
            }
        }
    }

    /// Takes one owned handle, including on error. It must have no subscriber or
    /// competing consumer. The library must remain loaded through all cleanup.
    pub unsafe fn from_raw(handle: *mut fsspec_operation) -> Result<Self, OperationError> {
        if handle.is_null() {
            return Err(operation_error(7, "null operation".into()));
        }
        let notification = Arc::new(Notification(Mutex::new(None)));
        let data = Arc::into_raw(notification.clone()).cast_mut().cast();
        let operation = Self {
            handle,
            notification,
        };
        let mut error = ptr::null_mut();
        let status = ffi::fsspec_operation_subscribe(
            handle,
            ptr::null(),
            Some(ready),
            data,
            Some(release),
            &mut error,
        );
        if status != 0 {
            release(data);
        }
        check(status, error)?;
        Ok(operation)
    }

    /// Wait for producer/callback/accounting cleanup without taking the result.
    pub fn drain(&self) -> Result<Self, OperationError> {
        unsafe {
            let mut handle = ptr::null_mut();
            let mut error = ptr::null_mut();
            let status = ffi::fsspec_operation_drain(self.handle, &mut handle, &mut error);
            check(status, error)?;
            Self::from_raw(handle)
        }
    }

    pub fn cancel(&self) {
        unsafe {
            ffi::fsspec_operation_cancel(self.handle);
        }
    }

    pub fn is_quiescent(&self) -> bool {
        unsafe { ffi::fsspec_operation_is_quiescent(self.handle) }
    }
}

impl Future for Operation {
    type Output = Outcome;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let waker = cx.waker().clone();
        let previous = self.notification.0.lock().unwrap().replace(waker);
        drop(previous);
        unsafe {
            if ffi::fsspec_operation_state(self.handle) == 0 {
                return Poll::Pending;
            }
            let waker = self.notification.0.lock().unwrap().take();
            drop(waker);
            let mut result = ptr::null_mut();
            let mut error = ptr::null_mut();
            let status = ffi::fsspec_operation_take(self.handle, &mut result, &mut error);
            if let Err(error) = check(status, error) {
                return Poll::Ready(Err(error));
            }
            let borrowed_error = ffi::fsspec_async_result_error(result);
            let outcome = if !borrowed_error.is_null() {
                Err(decode_error(
                    (*borrowed_error).status,
                    (*borrowed_error).message.as_str().into_owned(),
                ))
            } else {
                match ffi::fsspec_async_result_kind(result) {
                    0 => Ok(Value::Unit),
                    2 => {
                        let mut info = std::mem::MaybeUninit::uninit();
                        let status = ffi::fsspec_async_result_take_info(
                            result,
                            info.as_mut_ptr(),
                            &mut error,
                        );
                        check(status, error).map(|()| {
                            let info = info.assume_init();
                            let copied = info.to_info();
                            ffi::fsspec_file_info_free(info);
                            Value::Info(copied)
                        })
                    }
                    3 => {
                        let mut file = ptr::null_mut();
                        let status =
                            ffi::fsspec_async_result_take_file(result, &mut file, &mut error);
                        check(status, error)
                            .map(|()| Value::File(crate::async_files::AsyncFile::from_raw(file)))
                    }
                    4 => {
                        let mut position = 0;
                        let status =
                            ffi::fsspec_async_result_position(result, &mut position, &mut error);
                        check(status, error).map(|()| Value::Position(position))
                    }
                    _ => {
                        let mut len = 0;
                        let data = ffi::fsspec_async_result_bytes(result, &mut len);
                        Ok(Value::Bytes(if len == 0 {
                            Vec::new()
                        } else {
                            std::slice::from_raw_parts(data, len).to_vec()
                        }))
                    }
                }
            };
            ffi::fsspec_async_result_free(result);
            Poll::Ready(outcome)
        }
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        self.cancel();
        unsafe {
            ffi::fsspec_operation_release(self.handle);
        }
    }
}

/// Owned producer reference. Drop without completion abandons pending work only
/// when the last producer reference is released.
pub struct Completion(*mut fsspec_completion);
unsafe impl Send for Completion {}
unsafe impl Sync for Completion {}

impl Completion {
    /// Takes one live owned producer reference, kept until backend cleanup ends.
    pub unsafe fn from_raw(handle: *mut fsspec_completion) -> Self {
        assert!(!handle.is_null());
        Self(handle)
    }

    pub fn into_raw(self) -> *mut fsspec_completion {
        let handle = self.0;
        std::mem::forget(self);
        handle
    }

    pub fn is_cancelled(&self) -> bool {
        unsafe { ffi::fsspec_completion_is_cancelled(self.0) }
    }

    pub fn complete(self, outcome: Outcome) {
        unsafe {
            match outcome {
                Ok(Value::Unit) => ffi::fsspec_completion_unit(self.into_raw()),
                Ok(Value::Bytes(bytes)) => {
                    ffi::fsspec_completion_bytes(self.into_raw(), bytes.as_ptr(), bytes.len())
                }
                Ok(Value::Info(info)) => {
                    let info = fsspec_file_info::from_info(info);
                    ffi::fsspec_completion_info(self.into_raw(), &info);
                    ffi::fsspec_file_info_free(info);
                }
                Ok(Value::File(file)) => {
                    ffi::fsspec_completion_file(self.into_raw(), file.into_raw())
                }
                Ok(Value::Position(position)) => {
                    ffi::fsspec_completion_position(self.into_raw(), position)
                }
                Err(error) => {
                    let message = error.to_string();
                    ffi::fsspec_completion_error(
                        self.into_raw(),
                        operation_status(&error),
                        message.as_ptr().cast(),
                        message.len(),
                    );
                }
            }
        }
    }
}

impl Clone for Completion {
    fn clone(&self) -> Self {
        unsafe { Self(ffi::fsspec_completion_retain(self.0)) }
    }
}

impl Drop for Completion {
    fn drop(&mut self) {
        unsafe {
            ffi::fsspec_completion_release(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fsspec as _;
    use fsspec_rs::ErrorKind;
    use std::task::Wake;

    struct Thread(std::thread::Thread);
    impl Wake for Thread {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    #[test]
    fn foreign_completion_wakes_a_rust_future() {
        let (mut operation, completion) = Operation::new(Kind::Bytes, 3, None).unwrap();
        let producer =
            std::thread::spawn(move || completion.complete(Ok(Value::Bytes(vec![1, 2, 3]))));
        let waker = Arc::new(Thread(std::thread::current())).into();
        let mut context = Context::from_waker(&waker);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "readiness notification lost"
            );
            if let Poll::Ready(result) = Pin::new(&mut operation).poll(&mut context) {
                assert_eq!(result.unwrap(), Value::Bytes(vec![1, 2, 3]));
                break;
            }
            std::thread::park_timeout(
                deadline.saturating_duration_since(std::time::Instant::now()),
            );
        }
        producer.join().unwrap();
        assert!(operation.is_quiescent());
    }

    #[test]
    fn dropping_future_cancels_while_producer_retains_cleanup() {
        let (operation, completion) = Operation::new(Kind::Unit, 0, None).unwrap();
        drop(operation);
        assert!(completion.is_cancelled());
        completion.complete(Err(OperationError::Detailed {
            kind: ErrorKind::Cancelled,
            message: "cleaned up".into(),
            retry_safe: false,
        }));
    }
}
