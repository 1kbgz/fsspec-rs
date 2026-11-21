//! Safe ownership wrappers for the native object-read ABI.
use crate::{
    ffi,
    types::*,
    util::{arg, check, check_operation},
    ForeignFileSystem, Options,
};
use fsspec_rs::{
    ByteRange, FileInfo, FsOptions, FsResult, OperationError, RangeReadResult, SharedFileSystem,
};
use std::{mem::MaybeUninit, ptr, sync::Arc};

/// An owned native object view in libfsspec. Clone retains a handle to the same client.
pub struct ForeignObjectStore {
    handle: *mut fsspec_object_store,
}
// libfsspec's owned view holds an Arc to a Send + Sync backend.
unsafe impl Send for ForeignObjectStore {}
unsafe impl Sync for ForeignObjectStore {}
impl Clone for ForeignObjectStore {
    fn clone(&self) -> Self {
        Self {
            handle: unsafe { ffi::fsspec_object_store_retain(self.handle) },
        }
    }
}
impl Drop for ForeignObjectStore {
    fn drop(&mut self) {
        unsafe { ffi::fsspec_object_store_release(self.handle) };
    }
}

/// Construct a built-in S3 filesystem and native object view together, without
/// registry lookup or caching. The handles share a client and runtime.
pub fn s3_with_objects(
    bucket: &str,
    options: &FsOptions,
) -> FsResult<(SharedFileSystem, ForeignObjectStore)> {
    let options = Options::new(options)?;
    let mut fs = ptr::null_mut();
    let mut objects = ptr::null_mut();
    let (bucket, len) = arg(bucket);
    check(
        unsafe { ffi::fsspec_s3_open(bucket, len, options.0, &mut fs, &mut objects) },
        "s3_open",
    )?;
    let objects = ForeignObjectStore { handle: objects };
    let fs = unsafe { ForeignFileSystem::from_raw(fs) }?;
    Ok((Arc::new(fs), objects))
}

impl ForeignObjectStore {
    pub fn head(&self, key: &str) -> Result<FileInfo, OperationError> {
        let mut out = MaybeUninit::<fsspec_file_info>::uninit();
        let (key, len) = arg(key);
        check_operation(
            unsafe { ffi::fsspec_object_store_head(self.handle, key, len, out.as_mut_ptr()) },
            "object_head",
        )?;
        let out = unsafe { out.assume_init() };
        let info = unsafe { out.to_info() };
        unsafe { ffi::fsspec_file_info_free(out) };
        Ok(info)
    }
    pub fn read_ranges(
        &self,
        key: &str,
        ranges: &[ByteRange],
    ) -> Result<RangeReadResult, OperationError> {
        let mut out = MaybeUninit::<fsspec_range_read_result>::uninit();
        let status = crate::operations::with_ranges(ranges, |ranges, len| unsafe {
            ffi::fsspec_object_store_read_ranges(
                self.handle,
                key.as_ptr().cast(),
                key.len(),
                ranges,
                len,
                out.as_mut_ptr(),
            )
        });
        check_operation(status, "object_read_ranges")?;
        Ok(unsafe { out.assume_init().into_result()? })
    }
}
