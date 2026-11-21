//! Owned native object views, separate from filesystem vtables.
use std::ffi::c_char;
use std::ptr;
use std::sync::Arc;

use fsspec_rs::{S3Fs, S3ObjectReader};
use fsspec_sys::types::*;

use crate::filesystem::FsData;
use crate::options::options_arg;
use crate::status::{guard, guard_operation, handle_arg, invalid, str_arg};

struct ObjectData {
    inner: S3ObjectReader,
}
impl ObjectData {
    fn into_raw(inner: S3ObjectReader) -> *mut fsspec_object_store {
        Box::into_raw(Box::new(Self { inner })).cast()
    }
}

/// Construct the built-in S3 backend and its native object view together.
/// Both handles share one client/runtime and can be released independently.
/// This explicit constructor does not use the protocol registry or its cache.
/// Options match the registered S3 backend. Outputs are unchanged on failure.
#[no_mangle]
pub unsafe extern "C" fn fsspec_s3_open(
    bucket: *const c_char,
    bucket_len: usize,
    options: *const fsspec_options,
    out_fs: *mut *mut fsspec_fs,
    out_objects: *mut *mut fsspec_object_store,
) -> fsspec_status {
    guard(|| {
        if out_fs.is_null()
            || out_objects.is_null()
            || out_fs.cast::<()>() == out_objects.cast::<()>()
        {
            return Err(invalid(
                "distinct nonnull filesystem and object outputs are required",
            ));
        }
        let fs = Arc::new(S3Fs::from_options(
            str_arg(bucket, bucket_len)?,
            &options_arg(options),
        )?);
        let objects = ObjectData::into_raw(fs.objects());
        let fs = FsData::into_raw(fs);
        out_fs.write(fs);
        out_objects.write(objects);
        Ok(())
    })
}

/// Retain the native object view; null returns null.
#[no_mangle]
pub unsafe extern "C" fn fsspec_object_store_retain(
    store: *const fsspec_object_store,
) -> *mut fsspec_object_store {
    match (store as *const ObjectData).as_ref() {
        Some(store) => ObjectData::into_raw(store.inner.clone()),
        None => ptr::null_mut(),
    }
}

/// Release one native object view handle. Null is ignored.
#[no_mangle]
pub unsafe extern "C" fn fsspec_object_store_release(store: *mut fsspec_object_store) {
    if !store.is_null() {
        drop(Box::from_raw(store as *mut ObjectData));
    }
}

/// Native metadata for a bucket-relative object key. Never infers directories.
/// On success out is always a file with modified_ns and optional extra keys
/// "etag" and "version". Free it with fsspec_file_info_free. Failure leaves out unchanged.
#[no_mangle]
pub unsafe extern "C" fn fsspec_object_store_head(
    store: *const fsspec_object_store,
    key: *const c_char,
    key_len: usize,
    out: *mut fsspec_file_info,
) -> fsspec_operation_status {
    guard_operation(|| {
        if out.is_null() {
            return Err(invalid("object metadata output is null").into());
        }
        let store: &ObjectData = handle_arg(store, "object store")?;
        let info = store.inner.head(str_arg(key, key_len)?)?;
        out.write(fsspec_file_info::from_info(info));
        Ok(())
    })
}

/// Read ordered nonnegative, nonempty bounded ranges from a bucket-relative key.
/// Starts at/past EOF fail; ends past EOF are truncated. Empty input does no I/O.
/// Reuses fsspec_byte_range, but both bounds must be present and 0 <= start < end.
/// Free a successful result with fsspec_range_read_result_free. Failure leaves out unchanged.
#[no_mangle]
pub unsafe extern "C" fn fsspec_object_store_read_ranges(
    store: *const fsspec_object_store,
    key: *const c_char,
    key_len: usize,
    ranges: *const fsspec_byte_range,
    ranges_len: usize,
    out: *mut fsspec_range_read_result,
) -> fsspec_operation_status {
    guard_operation(|| {
        if out.is_null() {
            return Err(invalid("object ranges output is null").into());
        }
        let store: &ObjectData = handle_arg(store, "object store")?;
        let ranges = fsspec_sys::operations::ranges_arg(ranges, ranges_len)?;
        let result = store.inner.read_ranges(str_arg(key, key_len)?, &ranges)?;
        out.write(fsspec_range_read_result::from_result(result));
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        fsspec_fs_release, fsspec_options_free, fsspec_options_new, fsspec_options_set,
        fsspec_range_read_result_free,
    };
    use std::mem::MaybeUninit;

    #[test]
    fn constructor_failures_leave_outputs_untouched() {
        unsafe {
            let mut fs = ptr::null_mut();
            let mut objects = ptr::null_mut();
            assert_eq!(
                fsspec_s3_open(
                    b"bad/bucket".as_ptr().cast(),
                    10,
                    ptr::null(),
                    &mut fs,
                    &mut objects
                ),
                FSSPEC_INVALID_ARGUMENT
            );
            assert!(fs.is_null() && objects.is_null());
            assert_eq!(
                fsspec_s3_open(ptr::null(), 0, ptr::null(), &mut fs, ptr::null_mut()),
                FSSPEC_INVALID_ARGUMENT
            );
            assert!(fs.is_null());
            assert!(fsspec_object_store_retain(ptr::null()).is_null());
            fsspec_object_store_release(ptr::null_mut());
        }
    }

    #[test]
    fn invalid_options_match_the_registry_factory() {
        for (key, value) in [
            ("max_retries", "x"),
            ("request_timeout", "0"),
            ("unknown", "true"),
        ] {
            unsafe {
                let options = fsspec_options_new();
                assert_eq!(
                    fsspec_options_set(
                        options,
                        key.as_ptr().cast(),
                        key.len(),
                        value.as_ptr().cast(),
                        value.len()
                    ),
                    FSSPEC_OK
                );
                let mut fs = ptr::null_mut();
                let mut objects = ptr::null_mut();
                assert_eq!(
                    fsspec_s3_open(b"bucket".as_ptr().cast(), 6, options, &mut fs, &mut objects),
                    FSSPEC_INVALID_ARGUMENT
                );
                assert!(fs.is_null() && objects.is_null());
                let mut path = fsspec_string::null();
                assert_eq!(
                    crate::fsspec_url_to_fs(
                        b"s3://bucket".as_ptr().cast(),
                        11,
                        options,
                        &mut fs,
                        &mut path
                    ),
                    FSSPEC_INVALID_ARGUMENT
                );
                assert!(fs.is_null() && path.data.is_null());
                fsspec_options_free(options);
            }
        }
    }

    #[test]
    fn retained_object_handle_is_independent_and_validates_before_io() {
        unsafe {
            let options = fsspec_options_new();
            assert_eq!(
                fsspec_options_set(
                    options,
                    b"anon".as_ptr().cast(),
                    4,
                    b"true".as_ptr().cast(),
                    4
                ),
                FSSPEC_OK
            );
            let mut fs = ptr::null_mut();
            let mut objects = ptr::null_mut();
            assert_eq!(
                fsspec_s3_open(b"bucket".as_ptr().cast(), 6, options, &mut fs, &mut objects),
                FSSPEC_OK
            );
            fsspec_options_free(options);
            let retained = fsspec_object_store_retain(objects);
            fsspec_object_store_release(objects);
            fsspec_fs_release(fs);
            let mut info = MaybeUninit::<fsspec_file_info>::zeroed().assume_init();
            info.size = 42;
            assert_eq!(
                fsspec_object_store_head(retained, b"/invalid".as_ptr().cast(), 8, &mut info),
                FSSPEC_INVALID_ARGUMENT as i32
            );
            assert_eq!(info.size, 42);
            assert_eq!(
                fsspec_object_store_head(ptr::null(), b"key".as_ptr().cast(), 3, &mut info),
                FSSPEC_INVALID_ARGUMENT as i32
            );
            let mut result = MaybeUninit::<fsspec_range_read_result>::zeroed().assume_init();
            result.len = 42;
            let start = -1;
            let end = 1;
            let range = fsspec_byte_range {
                start: &start,
                end: &end,
            };
            assert_eq!(
                fsspec_object_store_read_ranges(
                    retained,
                    b"key".as_ptr().cast(),
                    3,
                    &range,
                    1,
                    &mut result
                ),
                FSSPEC_INVALID_ARGUMENT as i32
            );
            assert_eq!(result.len, 42);
            assert_eq!(
                fsspec_object_store_read_ranges(
                    retained,
                    b"key".as_ptr().cast(),
                    3,
                    ptr::null(),
                    0,
                    &mut result
                ),
                FSSPEC_OK as i32
            );
            assert_eq!(result.len, 0);
            fsspec_range_read_result_free(result);
            fsspec_object_store_release(retained);
        }
    }
}
