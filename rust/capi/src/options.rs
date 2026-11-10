use std::borrow::Cow;
use std::ffi::c_char;

use fsspec_rs::FsOptions;

use crate::status::{fsspec_status, guard, handle_arg_mut, str_arg};

/// Storage options for a filesystem, the string map of `FsOptions`.
pub struct fsspec_options {
    pub(crate) inner: FsOptions,
}

/// The options behind a possibly null pointer; null means no options.
pub(crate) unsafe fn options_arg<'a>(options: *const fsspec_options) -> Cow<'a, FsOptions> {
    match options.as_ref() {
        Some(options) => Cow::Borrowed(&options.inner),
        None => Cow::Owned(FsOptions::new()),
    }
}

/// Creates an empty option set. Free with `fsspec_options_free`.
#[no_mangle]
pub extern "C" fn fsspec_options_new() -> *mut fsspec_options {
    Box::into_raw(Box::new(fsspec_options {
        inner: FsOptions::new(),
    }))
}

/// Sets `key` to `value`, replacing an earlier value.
#[no_mangle]
pub unsafe extern "C" fn fsspec_options_set(
    options: *mut fsspec_options,
    key: *const c_char,
    key_len: usize,
    value: *const c_char,
    value_len: usize,
) -> fsspec_status {
    guard(|| {
        let options = handle_arg_mut(options, "options")?;
        let key = str_arg(key, key_len)?;
        let value = str_arg(value, value_len)?;
        options.inner.insert(key.to_string(), value.to_string());
        Ok(())
    })
}

/// Frees an option set. Null is ignored.
#[no_mangle]
pub unsafe extern "C" fn fsspec_options_free(options: *mut fsspec_options) {
    if !options.is_null() {
        drop(Box::from_raw(options));
    }
}
