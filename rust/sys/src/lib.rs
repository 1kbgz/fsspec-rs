//! Bindings to `libfsspec`, the shared fsspec-rs registry, for code that
//! must not compile the registry in.
//!
//! A Python extension or another shared library that opens filesystems calls
//! [`url_to_fs`] and gets a [`ForeignFileSystem`], which implements
//! `FileSystem` over a `libfsspec` handle. A backend written in Rust joins
//! the shared registry with [`register_filesystem`], which exports its
//! `FileSystem` implementation through the vtables in `fsspec.h`.
//!
//! The crate declares the C entry points without a `#[link]` attribute. The
//! build decides where they come from: hatch-rs links a bundled or external
//! `libfsspec`, and compiling `fsspec_capi` in resolves them statically.

#![allow(clippy::missing_safety_doc)]
#![allow(non_camel_case_types)]

mod export;
pub mod ffi;
mod foreign;
pub mod types;
mod util;

#[cfg(test)]
mod tests;

use std::ptr;
use std::sync::Arc;

use fsspec_rs::{FilesystemFactory, FsOptions, FsResult, SharedFileSystem};

pub use export::{export_file, export_filesystem};
pub use foreign::{ForeignFile, ForeignFileSystem};
pub use types::*;
pub use util::{check, error_from_status, io_error, status_code, take_last_error};

use util::{arg, empty_string_list};

/// The ABI version of the loaded `libfsspec`.
pub fn abi_version() -> u32 {
    unsafe { ffi::fsspec_abi_version() }
}

/// An owned `fsspec_options` handle.
pub(crate) struct Options(pub(crate) *mut fsspec_options);

impl Options {
    pub(crate) fn new(options: &FsOptions) -> FsResult<Self> {
        let handle = unsafe { ffi::fsspec_options_new() };
        if handle.is_null() {
            return Err(fsspec_rs::FsError::Other(
                "libfsspec could not allocate options".to_string(),
            ));
        }
        let owned = Self(handle);
        for (key, value) in options {
            let (key_ptr, key_len) = arg(key);
            let (value_ptr, value_len) = arg(value);
            check(
                unsafe { ffi::fsspec_options_set(handle, key_ptr, key_len, value_ptr, value_len) },
                "fsspec_options_set",
            )?;
        }
        Ok(owned)
    }
}

impl Drop for Options {
    fn drop(&mut self) {
        unsafe { ffi::fsspec_options_free(self.0) }
    }
}

/// Reads the string map behind an options handle; null means no options.
pub(crate) unsafe fn options_from_handle(options: *const fsspec_options) -> FsResult<FsOptions> {
    if options.is_null() {
        return Ok(FsOptions::new());
    }
    let mut items = empty_string_list();
    check(
        ffi::fsspec_options_items(options, &mut items),
        "fsspec_options_items",
    )?;
    let strings = items.to_strings();
    ffi::fsspec_string_list_free(items);
    Ok(strings
        .chunks(2)
        .map(|pair| (pair[0].clone(), pair.get(1).cloned().unwrap_or_default()))
        .collect())
}

/// Resolves `url` through the shared registry in `libfsspec`. Returns the
/// filesystem and the path to use with it, like `fsspec_rs::url_to_fs`.
pub fn url_to_fs(url: &str, options: &FsOptions) -> FsResult<(SharedFileSystem, String)> {
    let options = Options::new(options)?;
    let (url_ptr, url_len) = arg(url);
    let mut handle = ptr::null_mut();
    let mut path = fsspec_string::null();
    check(
        unsafe { ffi::fsspec_url_to_fs(url_ptr, url_len, options.0, &mut handle, &mut path) },
        "fsspec_url_to_fs",
    )?;
    let path = unsafe { foreign::take_string(path) };
    let fs = unsafe { ForeignFileSystem::from_raw(handle) }?;
    Ok((Arc::new(fs), path))
}

/// The protocols registered in `libfsspec`, sorted.
pub fn protocols() -> FsResult<Vec<String>> {
    let mut list = empty_string_list();
    check(
        unsafe { ffi::fsspec_protocols(&mut list) },
        "fsspec_protocols",
    )?;
    Ok(unsafe { foreign::take_strings(list) })
}

/// Registers `factory` for `protocol` in the shared registry, replacing an
/// earlier factory. The filesystems it builds are exported through the C
/// ABI, so any consumer of `libfsspec` can open them.
pub fn register_filesystem<F: FilesystemFactory + 'static>(
    protocol: &str,
    factory: F,
) -> FsResult<()> {
    export::register_factory(protocol, Box::new(factory))
}
