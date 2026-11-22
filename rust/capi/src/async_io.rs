use std::ffi::{c_char, c_void};
use std::mem::size_of;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::slice;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use fsspec_rs::async_io::{
    Completion, Dispatcher, Job, Kind, ManagedDispatcher, Operation, Outcome, Value,
};
use fsspec_rs::{ErrorKind, OperationError};
use fsspec_sys::{
    operations::{operation_error, operation_status},
    types::*,
};

pub(crate) fn invalid(message: &str) -> OperationError {
    OperationError::Detailed {
        kind: ErrorKind::Invalid,
        message: message.into(),
        retry_safe: false,
    }
}

fn owned_error(error: OperationError) -> *mut fsspec_async_error {
    Box::into_raw(Box::new(error_value(&error)))
}

fn error_value(error: &OperationError) -> fsspec_async_error {
    fsspec_async_error {
        status: operation_status(error),
        message: fsspec_string::from_string(error.to_string()),
    }
}

pub(crate) unsafe fn guard(
    out: *mut *mut fsspec_async_error,
    body: impl FnOnce() -> Result<(), OperationError>,
) -> i32 {
    if !out.is_null() {
        *out = ptr::null_mut();
    }
    match catch_unwind(AssertUnwindSafe(body))
        .unwrap_or_else(|_| Err(operation_error(9, "panic in async operation".into())))
    {
        Ok(()) => 0,
        Err(error) => {
            let status = operation_status(&error);
            if !out.is_null() {
                *out = owned_error(error);
            }
            status
        }
    }
}

pub(crate) unsafe fn bytes<'a>(data: *const u8, len: usize) -> Result<&'a [u8], OperationError> {
    if len == 0 {
        return Ok(&[]);
    }
    if data.is_null() || len > isize::MAX as usize {
        return Err(invalid("invalid byte pointer or length"));
    }
    Ok(slice::from_raw_parts(data, len))
}

unsafe fn message(
    status: i32,
    data: *const c_char,
    len: usize,
) -> Result<OperationError, OperationError> {
    if !(1..=16).contains(&(status & FSSPEC_ERROR_KIND_MASK))
        || status & !(FSSPEC_ERROR_KIND_MASK | FSSPEC_RETRY_SAFE) != 0
    {
        return Err(invalid("invalid error status"));
    }
    let text = std::str::from_utf8(bytes(data.cast(), len)?)
        .map_err(|_| invalid("error message is not UTF-8"))?;
    let kind = operation_error(status, String::new()).kind();
    let mut end = text.len().min(fsspec_rs::async_io::MAX_ERROR_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Ok(OperationError::Detailed {
        kind,
        message: text[..end].into(),
        retry_safe: status & FSSPEC_RETRY_SAFE != 0 && kind != ErrorKind::UnknownOutcome,
    })
}

/// Copies a nonzero status and UTF-8 message, truncated at 4096 bytes on a
/// character boundary. Null means invalid arguments.
#[no_mangle]
pub unsafe extern "C" fn fsspec_async_error_new(
    status: i32,
    data: *const c_char,
    len: usize,
) -> *mut fsspec_async_error {
    message(status, data, len)
        .map(owned_error)
        .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn fsspec_async_error_free(error: *mut fsspec_async_error) {
    if !error.is_null() {
        Box::from_raw(error).message.free();
    }
}

struct ForeignDispatcher {
    vtable: fsspec_dispatcher_vtable,
    data: *mut c_void,
}
// The C contract requires post and final release to be safe on any thread.
unsafe impl Send for ForeignDispatcher {}
unsafe impl Sync for ForeignDispatcher {}

impl Drop for ForeignDispatcher {
    fn drop(&mut self) {
        unsafe { (self.vtable.release.unwrap())(self.data) }
    }
}

impl Dispatcher for ForeignDispatcher {
    fn post(&self, job: Job) -> Result<(), (OperationError, Job)> {
        unsafe {
            let job = Box::into_raw(Box::new(job)).cast();
            let mut error = ptr::null_mut();
            let status = (self.vtable.post.unwrap())(self.data, job, &mut error);
            let detail = if error.is_null() {
                "dispatcher rejected job".into()
            } else {
                let text = (*error).message.as_str().into_owned();
                fsspec_async_error_free(error);
                text
            };
            if status == 0 {
                Ok(())
            } else {
                Err((
                    operation_error(status, detail),
                    *Box::from_raw(job.cast::<Job>()),
                ))
            }
        }
    }
}

pub(crate) unsafe fn dispatcher(handle: *const fsspec_dispatcher) -> Arc<dyn Dispatcher> {
    if handle.is_null() {
        Arc::new(ManagedDispatcher)
    } else {
        (&*handle.cast::<Arc<dyn Dispatcher>>()).clone()
    }
}

/// Copies the vtable and takes data only on success. Outputs remain untouched on
/// failure, except out_error (optional), which receives an owned error or null.
#[no_mangle]
pub unsafe extern "C" fn fsspec_dispatcher_from_vtable(
    vtable: *const fsspec_dispatcher_vtable,
    data: *mut c_void,
    out: *mut *mut fsspec_dispatcher,
    out_error: *mut *mut fsspec_async_error,
) -> i32 {
    guard(out_error, || {
        if vtable.is_null() || out.is_null() {
            return Err(invalid("null vtable or output"));
        }
        let size = ptr::read(vtable.cast::<usize>());
        if !(size_of::<fsspec_dispatcher_vtable>()..=65536).contains(&size) {
            return Err(invalid("invalid dispatcher struct_size"));
        }
        let vtable = *vtable;
        if vtable.abi_version != FSSPEC_ABI_VERSION
            || vtable.post.is_none()
            || vtable.release.is_none()
        {
            return Err(invalid("invalid dispatcher ABI version or callbacks"));
        }
        let dispatcher: Arc<dyn Dispatcher> = Arc::new(ForeignDispatcher { vtable, data });
        *out = Box::into_raw(Box::new(dispatcher)).cast();
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn fsspec_dispatcher_retain(
    handle: *const fsspec_dispatcher,
) -> *mut fsspec_dispatcher {
    if handle.is_null() {
        return ptr::null_mut();
    }
    Box::into_raw(Box::new(dispatcher(handle))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn fsspec_dispatcher_release(handle: *mut fsspec_dispatcher) {
    if !handle.is_null() {
        drop(Box::from_raw(handle.cast::<Arc<dyn Dispatcher>>()));
    }
}

/// Consumes one accepted job. Never call run and discard on the same job.
#[no_mangle]
pub unsafe extern "C" fn fsspec_dispatch_job_run(job: *mut fsspec_dispatch_job) {
    if !job.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| Box::from_raw(job.cast::<Job>()).run()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn fsspec_dispatch_job_discard(job: *mut fsspec_dispatch_job) {
    if !job.is_null() {
        drop(Box::from_raw(job.cast::<Job>()));
    }
}

/// Creates an observer/producer pair. kind: 0 unit, 1 bytes, 2 info, 3 file, 4 position. max_bytes bounds a
/// byte result (including the copy into libfsspec), not foreign allocations.
/// timeout_ns is relative monotonic time; UINT64_MAX disables the deadline.
/// Zero times out immediately. Keep a producer reference until cleanup stops.
#[no_mangle]
pub unsafe extern "C" fn fsspec_operation_create(
    kind: u32,
    max_bytes: usize,
    timeout_ns: u64,
    out: *mut *mut fsspec_operation,
    producer: *mut *mut fsspec_completion,
    out_error: *mut *mut fsspec_async_error,
) -> i32 {
    guard(out_error, || {
        if out.is_null() || producer.is_null() {
            return Err(invalid("null operation output"));
        }
        let kind = match kind {
            0 => Kind::Unit,
            1 => Kind::Bytes,
            2 => Kind::Info,
            3 => Kind::File,
            4 => Kind::Position,
            _ => return Err(invalid("unknown result kind")),
        };
        let timeout = (timeout_ns != u64::MAX).then(|| Duration::from_nanos(timeout_ns));
        let (operation, completion) = Operation::new(kind, max_bytes, timeout)?;
        *out = Box::into_raw(Box::new(operation)).cast();
        *producer = Box::into_raw(Box::new(completion)).cast();
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn fsspec_operation_retain(
    operation: *const fsspec_operation,
) -> *mut fsspec_operation {
    if operation.is_null() {
        return ptr::null_mut();
    }
    Box::into_raw(Box::new((&*operation.cast::<Operation>()).clone())).cast()
}

/// Releasing a raw observer does not cancel. Future wrappers should cancel first.
#[no_mangle]
pub unsafe extern "C" fn fsspec_operation_release(operation: *mut fsspec_operation) {
    if !operation.is_null() {
        drop(Box::from_raw(operation.cast::<Operation>()));
    }
}

/// 0 pending, 1 ready, 2 taken. Requires a live, nonnull handle.
#[no_mangle]
pub unsafe extern "C" fn fsspec_operation_state(operation: *const fsspec_operation) -> u32 {
    (&*operation.cast::<Operation>()).state()
}

/// True only after the last producer reference is consumed/released.
#[no_mangle]
pub unsafe extern "C" fn fsspec_operation_is_quiescent(operation: *const fsspec_operation) -> bool {
    (&*operation.cast::<Operation>()).is_quiescent()
}

#[no_mangle]
pub unsafe extern "C" fn fsspec_operation_cancel(operation: *mut fsspec_operation) {
    if !operation.is_null() {
        (&*operation.cast::<Operation>()).cancel();
    }
}

pub(crate) struct ResultValue {
    pub(crate) outcome: Outcome,
    error: *mut fsspec_async_error,
}
impl Drop for ResultValue {
    fn drop(&mut self) {
        unsafe { fsspec_async_error_free(self.error) }
    }
}

/// Transfers a ready result exactly once. Pending/taken returns Invalid.
#[no_mangle]
pub unsafe extern "C" fn fsspec_operation_take(
    operation: *mut fsspec_operation,
    out: *mut *mut fsspec_async_result,
    out_error: *mut *mut fsspec_async_error,
) -> i32 {
    guard(out_error, || {
        if operation.is_null() || out.is_null() {
            return Err(invalid("null operation or output"));
        }
        let outcome = (&*operation.cast::<Operation>()).take()?;
        let error = match &outcome {
            Ok(_) => ptr::null_mut(),
            Err(error) => Box::into_raw(Box::new(error_value(error))),
        };
        *out = Box::into_raw(Box::new(ResultValue { outcome, error })).cast();
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn fsspec_async_result_free(result: *mut fsspec_async_result) {
    if !result.is_null() {
        drop(Box::from_raw(result.cast::<ResultValue>()));
    }
}

/// Borrowed until result_free. Null on success. Do not free this borrowed error.
#[no_mangle]
pub unsafe extern "C" fn fsspec_async_result_error(
    result: *const fsspec_async_result,
) -> *const fsspec_async_error {
    (&*result.cast::<ResultValue>()).error
}

/// 0 unit, 1 bytes, 2 info, 3 file, 4 position, UINT32_MAX error. Requires a live, nonnull result.
#[no_mangle]
pub unsafe extern "C" fn fsspec_async_result_kind(result: *const fsspec_async_result) -> u32 {
    match &(&*result.cast::<ResultValue>()).outcome {
        Ok(Value::Unit) => 0,
        Ok(Value::Bytes(_)) => 1,
        Ok(Value::Info(_)) => 2,
        Ok(Value::File(_)) => 3,
        Ok(Value::Position(_)) => 4,
        Err(_) => u32::MAX,
    }
}

/// Bytes borrowed until result_free. Null with len=0 for unit/error; an empty
/// bytes result has kind=1. out_len must be nonnull. Never free this pointer.
#[no_mangle]
pub unsafe extern "C" fn fsspec_async_result_bytes(
    result: *const fsspec_async_result,
    out_len: *mut usize,
) -> *const u8 {
    match &(&*result.cast::<ResultValue>()).outcome {
        Ok(Value::Bytes(bytes)) => {
            *out_len = bytes.len();
            bytes.as_ptr()
        }
        _ => {
            *out_len = 0;
            ptr::null()
        }
    }
}

struct CallbackOwner {
    data: *mut c_void,
    release: Option<unsafe extern "C" fn(*mut c_void)>,
    armed: AtomicBool,
}
// Callback data is opaque; the foreign contract requires thread-safe release.
unsafe impl Send for CallbackOwner {}
unsafe impl Sync for CallbackOwner {}
impl Drop for CallbackOwner {
    fn drop(&mut self) {
        if self.armed.load(Ordering::Acquire) {
            if let Some(release) = self.release {
                unsafe {
                    release(self.data);
                }
            }
        }
    }
}

/// One subscriber, including after completion. Null dispatcher uses managed
/// dispatch. Takes data only on success. ready borrows its operation handle;
/// retain it inside ready to keep it. release may run on any thread.
#[no_mangle]
pub unsafe extern "C" fn fsspec_operation_subscribe(
    operation: *mut fsspec_operation,
    dispatch: *const fsspec_dispatcher,
    ready: Option<unsafe extern "C" fn(*mut c_void, *const fsspec_operation)>,
    data: *mut c_void,
    release: Option<unsafe extern "C" fn(*mut c_void)>,
    out_error: *mut *mut fsspec_async_error,
) -> i32 {
    guard(out_error, || {
        if operation.is_null() || ready.is_none() {
            return Err(invalid("null operation or ready callback"));
        }
        let operation = (&*operation.cast::<Operation>()).clone();
        let observer = operation.clone();
        let owner = Arc::new(CallbackOwner {
            data,
            release,
            armed: AtomicBool::new(true),
        });
        let capture = owner.clone();
        let result = operation.subscribe(
            dispatcher(dispatch),
            Job::new(move || {
                ready.unwrap()(capture.data, (&observer as *const Operation).cast());
                drop(capture);
            }),
        );
        if result.is_err() {
            owner.armed.store(false, Ordering::Release);
        }
        result
    })
}

#[no_mangle]
pub unsafe extern "C" fn fsspec_completion_retain(
    completion: *const fsspec_completion,
) -> *mut fsspec_completion {
    if completion.is_null() {
        return ptr::null_mut();
    }
    Box::into_raw(Box::new((&*completion.cast::<Completion>()).clone())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn fsspec_completion_release(completion: *mut fsspec_completion) {
    if !completion.is_null() {
        drop(Box::from_raw(completion.cast::<Completion>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn fsspec_completion_is_cancelled(
    completion: *const fsspec_completion,
) -> bool {
    (&*completion.cast::<Completion>()).is_cancelled()
}

/// UINT64_MAX means no deadline. Requires a live, nonnull producer.
#[no_mangle]
pub unsafe extern "C" fn fsspec_completion_remaining_ns(
    completion: *const fsspec_completion,
) -> u64 {
    (&*completion.cast::<Completion>())
        .remaining()
        .map(|d| d.as_nanos().min(u64::MAX as u128 - 1) as u64)
        .unwrap_or(u64::MAX)
}

/// Registers once; catches cancellation before registration. Data transfers on
/// success. Registration after successful completion releases data without calling cancel.
#[no_mangle]
pub unsafe extern "C" fn fsspec_completion_on_cancel(
    completion: *mut fsspec_completion,
    dispatch: *const fsspec_dispatcher,
    cancel: Option<unsafe extern "C" fn(*mut c_void)>,
    data: *mut c_void,
    release: Option<unsafe extern "C" fn(*mut c_void)>,
    out_error: *mut *mut fsspec_async_error,
) -> i32 {
    guard(out_error, || {
        if completion.is_null() || cancel.is_none() {
            return Err(invalid("null producer or cancellation callback"));
        }
        let owner = Arc::new(CallbackOwner {
            data,
            release,
            armed: AtomicBool::new(true),
        });
        let capture = owner.clone();
        let result = (&*completion.cast::<Completion>()).on_cancel(
            dispatcher(dispatch),
            Job::new(move || {
                cancel.unwrap()(capture.data);
                drop(capture);
            }),
        );
        if result.is_err() {
            owner.armed.store(false, Ordering::Release);
        }
        result
    })
}

/// Consumes one producer reference, including on invalid or late completion.
/// Copies borrowed bytes only after checking the operation's byte limit.
#[no_mangle]
pub unsafe extern "C" fn fsspec_completion_bytes(
    completion: *mut fsspec_completion,
    data: *const u8,
    len: usize,
) {
    if completion.is_null() {
        return;
    }
    let completion = *Box::from_raw(completion.cast::<Completion>());
    let outcome = if len > completion.max_bytes() {
        Err(invalid("byte result exceeds operation limit"))
    } else {
        bytes(data, len).map(|bytes| Value::Bytes(bytes.to_vec()))
    };
    completion.complete(outcome);
}

/// Consumes one producer reference.
#[no_mangle]
pub unsafe extern "C" fn fsspec_completion_unit(completion: *mut fsspec_completion) {
    if !completion.is_null() {
        Box::from_raw(completion.cast::<Completion>()).complete(Ok(Value::Unit));
    }
}

/// Copies up to 4096 UTF-8 bytes of error text and consumes one producer reference. Invalid inputs
/// complete with Invalid. No thread-local error state is read or written.
#[no_mangle]
pub unsafe extern "C" fn fsspec_completion_error(
    completion: *mut fsspec_completion,
    status: i32,
    data: *const c_char,
    len: usize,
) {
    if !completion.is_null() {
        Box::from_raw(completion.cast::<Completion>())
            .complete(Err(message(status, data, len).unwrap_or_else(|error| error)));
    }
}

/// Returns a unit operation after producer cleanup, accepted callback jobs, and
/// handle accounting finish. Does not take the source result. Cancelling this
/// waiter does not cancel the source operation or stop its cleanup.
#[no_mangle]
pub unsafe extern "C" fn fsspec_operation_drain(
    operation: *const fsspec_operation,
    out: *mut *mut fsspec_operation,
    out_error: *mut *mut fsspec_async_error,
) -> i32 {
    guard(out_error, || {
        if operation.is_null() || out.is_null() {
            return Err(invalid("null drain operation/output"));
        }
        let source = (&*operation.cast::<Operation>()).clone();
        let waiter = source.drain_operation()?;
        *out = Box::into_raw(Box::new(waiter)).cast();
        Ok(())
    })
}
