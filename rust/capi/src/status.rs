use std::cell::RefCell;
use std::ffi::{c_char, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::slice;

use fsspec_rs::{FsError, FsResult};
use fsspec_sys::types::fsspec_status;
use fsspec_sys::types::fsspec_status::*;

/// The ABI version of this header and library. A caller built against a
/// different version must not use the library.
pub const FSSPEC_ABI_VERSION: u32 = 1;

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

pub(crate) fn set_last_error(message: &str) {
    let bytes: Vec<u8> = message.bytes().filter(|byte| *byte != 0).collect();
    let message = CString::new(bytes).expect("interior NUL bytes were removed");
    LAST_ERROR.with(|slot| *slot.borrow_mut() = Some(message));
}

fn take_last_error() -> Option<String> {
    LAST_ERROR.with(|slot| {
        slot.borrow_mut()
            .take()
            .map(|message| message.to_string_lossy().into_owned())
    })
}

/// Turns a foreign status into a result, consuming the thread's error message.
pub(crate) fn check(status: fsspec_status, context: &str) -> FsResult<()> {
    if status == FSSPEC_OK {
        return Ok(());
    }
    let message = take_last_error()
        .unwrap_or_else(|| format!("{context} failed with status {}", status as i32));
    Err(fsspec_sys::error_from_status(status, message))
}

/// Runs an entry point, converting errors and panics into a status.
pub(crate) fn guard(body: impl FnOnce() -> FsResult<()>) -> fsspec_status {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(())) => FSSPEC_OK,
        Ok(Err(error)) => {
            set_last_error(&error.to_string());
            fsspec_sys::status_code(&error)
        }
        Err(_) => {
            set_last_error("libfsspec: panic across the C boundary");
            FSSPEC_OTHER
        }
    }
}

/// Like [`guard`] for entry points that return a pointer; null means failure.
pub(crate) fn guard_ptr<T>(body: impl FnOnce() -> FsResult<*mut T>) -> *mut T {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(pointer)) => pointer,
        Ok(Err(error)) => {
            set_last_error(&error.to_string());
            ptr::null_mut()
        }
        Err(_) => {
            set_last_error("libfsspec: panic across the C boundary");
            ptr::null_mut()
        }
    }
}

pub(crate) fn invalid(message: &str) -> FsError {
    FsError::InvalidArgument(message.to_string())
}

/// A UTF-8 input string. Null with zero length is the empty string.
pub(crate) unsafe fn str_arg<'a>(pointer: *const c_char, len: usize) -> FsResult<&'a str> {
    if pointer.is_null() {
        if len == 0 {
            return Ok("");
        }
        return Err(invalid("string pointer is null"));
    }
    std::str::from_utf8(slice::from_raw_parts(pointer as *const u8, len))
        .map_err(|_| invalid("string is not UTF-8"))
}

/// The data behind an opaque handle pointer.
pub(crate) unsafe fn handle_arg<'a, H, T>(pointer: *const H, what: &str) -> FsResult<&'a T> {
    (pointer as *const T)
        .as_ref()
        .ok_or_else(|| invalid(&format!("{what} is null")))
}

pub(crate) unsafe fn handle_arg_mut<'a, H, T>(pointer: *mut H, what: &str) -> FsResult<&'a mut T> {
    (pointer as *mut T)
        .as_mut()
        .ok_or_else(|| invalid(&format!("{what} is null")))
}

/// Writes an output value; a null output pointer is an invalid argument.
pub(crate) unsafe fn write_out<T>(pointer: *mut T, value: T) -> FsResult<()> {
    if pointer.is_null() {
        drop(value);
        return Err(invalid("output pointer is null"));
    }
    pointer.write(value);
    Ok(())
}

/// The ABI version this library was built with.
#[no_mangle]
pub extern "C" fn fsspec_abi_version() -> u32 {
    FSSPEC_ABI_VERSION
}

/// The message for the last failure on the calling thread, or null. The
/// pointer stays valid until the next failure on this thread.
#[no_mangle]
pub extern "C" fn fsspec_last_error_message() -> *const c_char {
    LAST_ERROR.with(|slot| {
        slot.borrow()
            .as_ref()
            .map_or(ptr::null(), |message| message.as_ptr())
    })
}

/// Sets the thread's error message. Foreign backends call this before
/// returning a failing status so callers see a description.
#[no_mangle]
pub unsafe extern "C" fn fsspec_set_last_error(message: *const c_char, len: usize) {
    match str_arg(message, len) {
        Ok(message) => set_last_error(message),
        Err(_) => set_last_error("foreign backend reported a non-UTF-8 error message"),
    }
}

/// Clears the thread's error message.
#[no_mangle]
pub extern "C" fn fsspec_clear_last_error() {
    LAST_ERROR.with(|slot| *slot.borrow_mut() = None);
}
