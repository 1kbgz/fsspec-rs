//! Shared plumbing for crossing the boundary in both directions.

use std::ffi::{c_char, CStr};
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::slice;

use fsspec_rs::{FsError, FsResult};

use crate::ffi;
use crate::types::*;

/// The status code a Rust error maps to.
pub fn status_code(error: &FsError) -> fsspec_status {
    match error {
        FsError::NotFound(_) => FSSPEC_NOT_FOUND,
        FsError::PermissionDenied(_) => FSSPEC_PERMISSION_DENIED,
        FsError::AlreadyExists(_) => FSSPEC_ALREADY_EXISTS,
        FsError::NotADirectory(_) => FSSPEC_NOT_A_DIRECTORY,
        FsError::IsADirectory(_) => FSSPEC_IS_A_DIRECTORY,
        FsError::IoError(_) => FSSPEC_IO,
        FsError::InvalidArgument(_) => FSSPEC_INVALID_ARGUMENT,
        FsError::NotSupported(_) => FSSPEC_NOT_SUPPORTED,
        FsError::Other(_) => FSSPEC_OTHER,
    }
}

/// The Rust error a status code and message map to.
pub fn error_from_status(status: fsspec_status, message: String) -> FsError {
    match status {
        FSSPEC_OK | FSSPEC_OTHER => FsError::Other(message),
        FSSPEC_NOT_FOUND => FsError::NotFound(message),
        FSSPEC_PERMISSION_DENIED => FsError::PermissionDenied(message),
        FSSPEC_ALREADY_EXISTS => FsError::AlreadyExists(message),
        FSSPEC_NOT_A_DIRECTORY => FsError::NotADirectory(message),
        FSSPEC_IS_A_DIRECTORY => FsError::IsADirectory(message),
        FSSPEC_IO => FsError::IoError(io::Error::other(message)),
        FSSPEC_INVALID_ARGUMENT => FsError::InvalidArgument(message),
        FSSPEC_NOT_SUPPORTED => FsError::NotSupported(message),
    }
}

/// An `io::Error` for a filesystem error, keeping the kind where one fits.
pub fn io_error(error: FsError) -> io::Error {
    let kind = match &error {
        FsError::NotFound(_) => io::ErrorKind::NotFound,
        FsError::PermissionDenied(_) => io::ErrorKind::PermissionDenied,
        FsError::AlreadyExists(_) => io::ErrorKind::AlreadyExists,
        FsError::InvalidArgument(_) => io::ErrorKind::InvalidInput,
        FsError::NotSupported(_) => io::ErrorKind::Unsupported,
        FsError::IoError(inner) => inner.kind(),
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, error.to_string())
}

/// The thread's last error message from libfsspec, cleared after reading.
pub fn take_last_error() -> Option<String> {
    unsafe {
        let message = ffi::fsspec_last_error_message();
        if message.is_null() {
            return None;
        }
        let text = CStr::from_ptr(message).to_string_lossy().into_owned();
        ffi::fsspec_clear_last_error();
        Some(text)
    }
}

/// Turns a status from libfsspec into a result carrying its message.
pub fn check(status: fsspec_status, context: &str) -> FsResult<()> {
    if status == FSSPEC_OK {
        return Ok(());
    }
    let message = take_last_error()
        .unwrap_or_else(|| format!("{context} failed with status {}", status as i32));
    Err(error_from_status(status, message))
}

pub(crate) fn set_last_error(message: &str) {
    unsafe { ffi::fsspec_set_last_error(message.as_ptr() as *const c_char, message.len()) }
}

/// Runs a callback body that libfsspec invoked, reporting errors and panics
/// as a status.
pub(crate) fn guard(body: impl FnOnce() -> FsResult<()>) -> fsspec_status {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(())) => FSSPEC_OK,
        Ok(Err(error)) => {
            set_last_error(&error.to_string());
            status_code(&error)
        }
        Err(_) => {
            set_last_error("fsspec_sys: panic in a foreign callback");
            FSSPEC_OTHER
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

/// Writes an output value; a null output pointer is an invalid argument.
pub(crate) unsafe fn write_out<T>(pointer: *mut T, value: T) -> FsResult<()> {
    if pointer.is_null() {
        drop(value);
        return Err(invalid("output pointer is null"));
    }
    pointer.write(value);
    Ok(())
}

pub(crate) fn arg(text: &str) -> (*const c_char, usize) {
    (text.as_ptr() as *const c_char, text.len())
}

pub(crate) fn null_string() -> fsspec_string {
    fsspec_string::null()
}

pub(crate) fn empty_bytes() -> fsspec_bytes {
    fsspec_bytes {
        data: ptr::null_mut(),
        len: 0,
    }
}

pub(crate) fn empty_string_list() -> fsspec_string_list {
    fsspec_string_list {
        items: ptr::null_mut(),
        len: 0,
    }
}

pub(crate) fn empty_info_list() -> fsspec_file_info_list {
    fsspec_file_info_list {
        items: ptr::null_mut(),
        len: 0,
    }
}
