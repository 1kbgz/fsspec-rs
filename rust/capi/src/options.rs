use std::borrow::Cow;
use std::ffi::c_char;

use fsspec_rs::FsOptions;
use fsspec_sys::types::{fsspec_options, fsspec_status, fsspec_string_list};

use crate::status::{guard, handle_arg, handle_arg_mut, str_arg, write_out};

/// The data behind an `fsspec_options` handle.
pub(crate) struct OptionsData {
    pub(crate) inner: FsOptions,
}

/// The options behind a possibly null pointer; null means no options.
pub(crate) unsafe fn options_arg<'a>(options: *const fsspec_options) -> Cow<'a, FsOptions> {
    match (options as *const OptionsData).as_ref() {
        Some(options) => Cow::Borrowed(&options.inner),
        None => Cow::Owned(FsOptions::new()),
    }
}

/// Creates an empty option set. Free with `fsspec_options_free`.
#[no_mangle]
pub extern "C" fn fsspec_options_new() -> *mut fsspec_options {
    Box::into_raw(Box::new(OptionsData {
        inner: FsOptions::new(),
    })) as *mut fsspec_options
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
        let options: &mut OptionsData = handle_arg_mut(options, "options")?;
        let key = str_arg(key, key_len)?;
        let value = str_arg(value, value_len)?;
        options.inner.insert(key.to_string(), value.to_string());
        Ok(())
    })
}

/// The options as alternating key and value strings, sorted by key. Free
/// with `fsspec_string_list_free`.
#[no_mangle]
pub unsafe extern "C" fn fsspec_options_items(
    options: *const fsspec_options,
    out: *mut fsspec_string_list,
) -> fsspec_status {
    guard(|| {
        let options: &OptionsData = handle_arg(options, "options")?;
        let items = options
            .inner
            .iter()
            .flat_map(|(key, value)| [key.clone(), value.clone()])
            .collect();
        write_out(out, fsspec_string_list::from_strings(items))
    })
}

/// Frees an option set. Null is ignored.
#[no_mangle]
pub unsafe extern "C" fn fsspec_options_free(options: *mut fsspec_options) {
    if !options.is_null() {
        drop(Box::from_raw(options as *mut OptionsData));
    }
}
