use std::ffi::{c_char, c_void};
use std::io::SeekFrom;
use std::mem::size_of;
use std::ptr;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, UNIX_EPOCH};

use crate::async_io::{bytes, dispatcher, guard, invalid, ResultValue};
use fsspec_rs::async_files::{
    AsyncContext, AsyncFile, AsyncFilesystem, FileProvider, FilesystemProvider, Limits,
};
use fsspec_rs::async_io::{Completion, Operation, Value};
use fsspec_rs::{FileInfo, FileType, OperationError};
use fsspec_sys::types::*;

type Result<T> = std::result::Result<T, OperationError>;
unsafe fn options(options: *const fsspec_async_options) -> Result<Option<Duration>> {
    if options.is_null() {
        return Ok(None);
    }
    if ptr::read(options.cast::<usize>()) < size_of::<fsspec_async_options>() {
        return Err(invalid("async options struct_size too small"));
    }
    Ok(((*options).timeout_ns != u64::MAX).then(|| Duration::from_nanos((*options).timeout_ns)))
}
unsafe fn string<'a>(data: *const c_char, len: usize) -> Result<&'a str> {
    std::str::from_utf8(bytes(data.cast(), len)?).map_err(|_| invalid("string is not UTF-8"))
}
unsafe fn start(
    out: *mut *mut fsspec_operation,
    out_error: *mut *mut fsspec_async_error,
    body: impl FnOnce() -> Result<Operation>,
) -> i32 {
    guard(out_error, || {
        if out.is_null() {
            return Err(invalid("null operation output"));
        }
        *out = Box::into_raw(Box::new(body()?)).cast();
        Ok(())
    })
}
unsafe fn handle<'a, T, H>(handle: *const H) -> Result<&'a T> {
    handle
        .cast::<T>()
        .as_ref()
        .ok_or_else(|| invalid("null async handle"))
}
fn token(completion: Completion) -> *mut fsspec_completion {
    Box::into_raw(Box::new(completion)).cast()
}

/// Creates a context sharing libfsspec's managed runtime. Outputs unchanged on failure.
#[no_mangle]
pub unsafe extern "C" fn fsspec_async_context_create(
    options: *const fsspec_async_context_options,
    out: *mut *mut fsspec_async_context,
    out_error: *mut *mut fsspec_async_error,
) -> i32 {
    guard(out_error, || {
        if out.is_null() {
            return Err(invalid("null context output"));
        }
        let limits = if options.is_null() {
            Limits::default()
        } else {
            if ptr::read(options.cast::<usize>()) < size_of::<fsspec_async_context_options>() {
                return Err(invalid("context options struct_size too small"));
            }
            Limits {
                max_operations: (*options).max_operations,
                max_buffered_bytes: usize::try_from((*options).max_buffered_bytes)
                    .map_err(|_| invalid("byte limit too large"))?,
                max_read_size: (*options).max_read_size,
            }
        };
        *out = Box::into_raw(Box::new(AsyncContext::new(limits)?)).cast();
        Ok(())
    })
}

struct ForeignFile {
    vtable: fsspec_async_file_vtable,
    data: *mut c_void,
    owned: AtomicBool,
}
// The vtable contract requires thread-safe final release; starts use its dispatcher.
unsafe impl Send for ForeignFile {}
unsafe impl Sync for ForeignFile {}
impl Drop for ForeignFile {
    fn drop(&mut self) {
        if self.owned.load(Ordering::Acquire) {
            unsafe { self.vtable.release.unwrap()(self.data) }
        }
    }
}
impl FileProvider for ForeignFile {
    fn read(&self, bytes: usize, completion: Completion) {
        unsafe { self.vtable.read.unwrap()(self.data, bytes, token(completion)) }
    }
    fn info(&self, completion: Completion) {
        unsafe { self.vtable.info.unwrap()(self.data, token(completion)) }
    }
    fn close(&self, completion: Completion) {
        unsafe { self.vtable.close.unwrap()(self.data, token(completion)) }
    }
    fn seek(&self, position: SeekFrom, completion: Completion) {
        let (offset, whence) = match position {
            SeekFrom::Start(offset) => match i64::try_from(offset) {
                Ok(offset) => (offset, 0),
                Err(_) => {
                    completion.complete(Err(invalid("start offset exceeds C vtable range")));
                    return;
                }
            },
            SeekFrom::Current(offset) => (offset, 1),
            SeekFrom::End(offset) => (offset, 2),
        };
        if let Some(seek) = self.vtable.seek {
            unsafe { seek(self.data, offset, whence, token(completion)) }
        } else {
            completion.complete(Err(fsspec_rs::FsError::NotSupported(
                "file is not seekable".into(),
            )
            .into()));
        }
    }
}
struct ForeignFilesystem {
    vtable: fsspec_async_fs_vtable,
    data: *mut c_void,
    owned: AtomicBool,
}
unsafe impl Send for ForeignFilesystem {}
unsafe impl Sync for ForeignFilesystem {}
impl Drop for ForeignFilesystem {
    fn drop(&mut self) {
        if self.owned.load(Ordering::Acquire) {
            unsafe { self.vtable.release.unwrap()(self.data) }
        }
    }
}
impl FilesystemProvider for ForeignFilesystem {
    fn info(&self, path: &str, completion: Completion) {
        unsafe {
            self.vtable.info.unwrap()(
                self.data,
                path.as_ptr().cast(),
                path.len(),
                token(completion),
            )
        }
    }
    fn open_read(&self, path: &str, completion: Completion) {
        unsafe {
            self.vtable.open_read.unwrap()(
                self.data,
                path.as_ptr().cast(),
                path.len(),
                token(completion),
            )
        }
    }
    fn close(&self, completion: Completion) {
        if let Some(close) = self.vtable.close {
            unsafe { close(self.data, token(completion)) }
        } else {
            completion.complete(Ok(Value::Unit));
        }
    }
}

/// Copies vtable and takes data only on success. Context required; null dispatcher
/// uses managed dispatch. Close must finish loop-affine cleanup before completion.
#[no_mangle]
pub unsafe extern "C" fn fsspec_async_file_from_vtable(
    context: *const fsspec_async_context,
    dispatch: *const fsspec_dispatcher,
    vtable: *const fsspec_async_file_vtable,
    data: *mut c_void,
    out: *mut *mut fsspec_async_file,
    out_error: *mut *mut fsspec_async_error,
) -> i32 {
    guard(out_error, || {
        let context = handle::<AsyncContext, _>(context)?;
        if out.is_null() || vtable.is_null() {
            return Err(invalid("null file vtable or output"));
        }
        if !(size_of::<fsspec_async_file_vtable>()..=65536)
            .contains(&ptr::read(vtable.cast::<usize>()))
        {
            return Err(invalid("invalid file vtable size"));
        }
        let vtable = *vtable;
        if vtable.abi_version != FSSPEC_ABI_VERSION
            || vtable.read.is_none()
            || vtable.info.is_none()
            || vtable.close.is_none()
            || vtable.release.is_none()
        {
            return Err(invalid("invalid file vtable callbacks/version"));
        }
        let provider = Arc::new(ForeignFile {
            vtable,
            data,
            owned: AtomicBool::new(false),
        });
        let file = AsyncFile::new(context, dispatcher(dispatch), provider.clone())?;
        provider.owned.store(true, Ordering::Release);
        *out = Box::into_raw(Box::new(file)).cast();
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn fsspec_async_fs_from_vtable(
    context: *const fsspec_async_context,
    dispatch: *const fsspec_dispatcher,
    vtable: *const fsspec_async_fs_vtable,
    data: *mut c_void,
    out: *mut *mut fsspec_async_fs,
    out_error: *mut *mut fsspec_async_error,
) -> i32 {
    guard(out_error, || {
        let context = handle::<AsyncContext, _>(context)?;
        if out.is_null() || vtable.is_null() {
            return Err(invalid("null filesystem vtable or output"));
        }
        if !(size_of::<fsspec_async_fs_vtable>()..=65536)
            .contains(&ptr::read(vtable.cast::<usize>()))
        {
            return Err(invalid("invalid filesystem vtable size"));
        }
        let vtable = *vtable;
        if vtable.abi_version != FSSPEC_ABI_VERSION
            || vtable.info.is_none()
            || vtable.open_read.is_none()
            || vtable.release.is_none()
        {
            return Err(invalid("invalid filesystem vtable callbacks/version"));
        }
        let provider = Arc::new(ForeignFilesystem {
            vtable,
            data,
            owned: AtomicBool::new(false),
        });
        let fs = AsyncFilesystem::new(context, dispatcher(dispatch), provider.clone())?;
        provider.owned.store(true, Ordering::Release);
        *out = Box::into_raw(Box::new(fs)).cast();
        Ok(())
    })
}

/// Copies bounded metadata and consumes a producer reference, even on invalid input.
/// UTF-8 string bytes are bounded; map/struct overhead is outside the byte budget.
#[no_mangle]
pub unsafe extern "C" fn fsspec_completion_info(
    completion: *mut fsspec_completion,
    info: *const fsspec_file_info,
) {
    if completion.is_null() {
        return;
    }
    let completion = *Box::from_raw(completion.cast::<Completion>());
    let read = || -> Result<Value> {
        if info.is_null() {
            return Err(invalid("null metadata"));
        }
        let kind = ptr::read(ptr::addr_of!((*info).file_type).cast::<i32>());
        let file_type = match kind {
            0 => FileType::File,
            1 => FileType::Directory,
            2 => FileType::Other,
            _ => return Err(invalid("invalid file type")),
        };
        let info = &*info;
        if !info.extra.len.is_multiple_of(2)
            || info.extra.len > completion.max_bytes()
            || info.extra.len > isize::MAX as usize / size_of::<fsspec_string>()
            || (info.extra.len != 0 && info.extra.items.is_null())
        {
            return Err(invalid("invalid metadata map"));
        }
        let extras = if info.extra.len == 0 {
            &[][..]
        } else {
            std::slice::from_raw_parts(info.extra.items, info.extra.len)
        };
        let size = extras.iter().try_fold(info.name.len, |size, value| {
            size.checked_add(value.len)
                .ok_or_else(|| invalid("metadata too large"))
        })?;
        if size > completion.max_bytes() {
            return Err(invalid("metadata exceeds operation limit"));
        }
        let name = string(info.name.data, info.name.len)?;
        let mut output = FileInfo::file(name, info.size);
        output.file_type = file_type;
        output.created = (info.created_ns >= 0)
            .then(|| UNIX_EPOCH + Duration::from_nanos(info.created_ns as u64));
        output.modified = (info.modified_ns >= 0)
            .then(|| UNIX_EPOCH + Duration::from_nanos(info.modified_ns as u64));
        for pair in extras.as_chunks::<2>().0 {
            output.extra.insert(
                string(pair[0].data, pair[0].len)?.into(),
                string(pair[1].data, pair[1].len)?.into(),
            );
        }
        Ok(Value::Info(output))
    };
    let result = read();
    completion.complete(result);
}

/// Consumes one file and producer reference, including on late/invalid completion.
#[no_mangle]
pub unsafe extern "C" fn fsspec_completion_file(
    completion: *mut fsspec_completion,
    file: *mut fsspec_async_file,
) {
    let file = if file.is_null() {
        None
    } else {
        Some(*Box::from_raw(file.cast::<AsyncFile>()))
    };
    if !completion.is_null() {
        Box::from_raw(completion.cast::<Completion>()).complete(
            file.map(Value::File)
                .ok_or_else(|| invalid("null file completion")),
        );
    }
}
#[no_mangle]
pub unsafe extern "C" fn fsspec_completion_position(
    completion: *mut fsspec_completion,
    position: u64,
) {
    if !completion.is_null() {
        Box::from_raw(completion.cast::<Completion>()).complete(Ok(Value::Position(position)));
    }
}

/// Moves metadata out once. Free with fsspec_file_info_free. Result becomes unit.
#[no_mangle]
pub unsafe extern "C" fn fsspec_async_result_take_info(
    result: *mut fsspec_async_result,
    out: *mut fsspec_file_info,
    out_error: *mut *mut fsspec_async_error,
) -> i32 {
    guard(out_error, || {
        if out.is_null() || result.is_null() {
            return Err(invalid("null metadata output/result"));
        }
        let result = &mut *result.cast::<ResultValue>();
        if !matches!(result.outcome, Ok(Value::Info(_))) {
            return Err(invalid("result is not metadata"));
        }
        if let Ok(Value::Info(info)) = std::mem::replace(&mut result.outcome, Ok(Value::Unit)) {
            *out = fsspec_file_info::from_info(info);
        }
        Ok(())
    })
}
/// Moves one file handle out once. Result becomes unit. Release with async_file_release.
#[no_mangle]
pub unsafe extern "C" fn fsspec_async_result_take_file(
    result: *mut fsspec_async_result,
    out: *mut *mut fsspec_async_file,
    out_error: *mut *mut fsspec_async_error,
) -> i32 {
    guard(out_error, || {
        if out.is_null() || result.is_null() {
            return Err(invalid("null file output/result"));
        }
        let result = &mut *result.cast::<ResultValue>();
        if !matches!(result.outcome, Ok(Value::File(_))) {
            return Err(invalid("result is not a file"));
        }
        if let Ok(Value::File(file)) = std::mem::replace(&mut result.outcome, Ok(Value::Unit)) {
            *out = Box::into_raw(Box::new(file)).cast();
        }
        Ok(())
    })
}
#[no_mangle]
pub unsafe extern "C" fn fsspec_async_result_position(
    result: *const fsspec_async_result,
    out: *mut u64,
    out_error: *mut *mut fsspec_async_error,
) -> i32 {
    guard(out_error, || {
        if out.is_null() || result.is_null() {
            return Err(invalid("null position output/result"));
        }
        match (&*result.cast::<ResultValue>()).outcome {
            Ok(Value::Position(value)) => {
                *out = value;
                Ok(())
            }
            _ => Err(invalid("result is not a position")),
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn fsspec_async_context_retain(
    value: *const fsspec_async_context,
) -> *mut fsspec_async_context {
    if value.is_null() {
        return ptr::null_mut();
    }
    Box::into_raw(Box::new((&*value.cast::<AsyncContext>()).clone())).cast()
}
/// Final release schedules best-effort close; explicitly close to observe cleanup.
#[no_mangle]
pub unsafe extern "C" fn fsspec_async_context_release(value: *mut fsspec_async_context) {
    if !value.is_null() {
        drop(Box::from_raw(value.cast::<AsyncContext>()));
    }
}
/// Joins cleanup without consuming the handle. Timeout/cancellation stops only
/// this waiter; a later close can await cleanup. Control work bypasses data limits.
#[no_mangle]
pub unsafe extern "C" fn fsspec_async_context_close(
    value: *mut fsspec_async_context,
    opts: *const fsspec_async_options,
    out: *mut *mut fsspec_operation,
    out_error: *mut *mut fsspec_async_error,
) -> i32 {
    start(out, out_error, || {
        handle::<AsyncContext, _>(value)?.close(options(opts)?)
    })
}

#[no_mangle]
pub unsafe extern "C" fn fsspec_async_file_retain(
    value: *const fsspec_async_file,
) -> *mut fsspec_async_file {
    if value.is_null() {
        return ptr::null_mut();
    }
    Box::into_raw(Box::new((&*value.cast::<AsyncFile>()).clone())).cast()
}
/// Final release schedules best-effort close; explicitly close to observe cleanup.
#[no_mangle]
pub unsafe extern "C" fn fsspec_async_file_release(value: *mut fsspec_async_file) {
    if !value.is_null() {
        drop(Box::from_raw(value.cast::<AsyncFile>()));
    }
}
/// Joins cleanup without consuming the handle. Timeout/cancellation stops only
/// this waiter; a later close can await cleanup. Control work bypasses data limits.
#[no_mangle]
pub unsafe extern "C" fn fsspec_async_file_close(
    value: *mut fsspec_async_file,
    opts: *const fsspec_async_options,
    out: *mut *mut fsspec_operation,
    out_error: *mut *mut fsspec_async_error,
) -> i32 {
    start(out, out_error, || {
        handle::<AsyncFile, _>(value)?.close(options(opts)?)
    })
}

#[no_mangle]
pub unsafe extern "C" fn fsspec_async_fs_retain(
    value: *const fsspec_async_fs,
) -> *mut fsspec_async_fs {
    if value.is_null() {
        return ptr::null_mut();
    }
    Box::into_raw(Box::new((&*value.cast::<AsyncFilesystem>()).clone())).cast()
}
/// Final release schedules best-effort close; explicitly close to observe cleanup.
#[no_mangle]
pub unsafe extern "C" fn fsspec_async_fs_release(value: *mut fsspec_async_fs) {
    if !value.is_null() {
        drop(Box::from_raw(value.cast::<AsyncFilesystem>()));
    }
}
/// Joins cleanup without consuming the handle. Timeout/cancellation stops only
/// this waiter; a later close can await cleanup. Control work bypasses data limits.
#[no_mangle]
pub unsafe extern "C" fn fsspec_async_fs_close(
    value: *mut fsspec_async_fs,
    opts: *const fsspec_async_options,
    out: *mut *mut fsspec_operation,
    out_error: *mut *mut fsspec_async_error,
) -> i32 {
    start(out, out_error, || {
        handle::<AsyncFilesystem, _>(value)?.close(options(opts)?)
    })
}

#[no_mangle]
pub unsafe extern "C" fn fsspec_async_fs_info(
    value: *const fsspec_async_fs,
    path: *const c_char,
    path_len: usize,
    opts: *const fsspec_async_options,
    out: *mut *mut fsspec_operation,
    out_error: *mut *mut fsspec_async_error,
) -> i32 {
    start(out, out_error, || {
        handle::<AsyncFilesystem, _>(value)?.info(string(path, path_len)?, options(opts)?)
    })
}

#[no_mangle]
pub unsafe extern "C" fn fsspec_async_fs_open_read(
    value: *const fsspec_async_fs,
    path: *const c_char,
    path_len: usize,
    opts: *const fsspec_async_options,
    out: *mut *mut fsspec_operation,
    out_error: *mut *mut fsspec_async_error,
) -> i32 {
    start(out, out_error, || {
        handle::<AsyncFilesystem, _>(value)?.open_read(string(path, path_len)?, options(opts)?)
    })
}

#[no_mangle]
pub unsafe extern "C" fn fsspec_async_file_read(
    value: *const fsspec_async_file,
    max_bytes: usize,
    opts: *const fsspec_async_options,
    out: *mut *mut fsspec_operation,
    out_error: *mut *mut fsspec_async_error,
) -> i32 {
    start(out, out_error, || {
        handle::<AsyncFile, _>(value)?.read(max_bytes, options(opts)?)
    })
}

#[no_mangle]
pub unsafe extern "C" fn fsspec_async_file_info(
    value: *const fsspec_async_file,
    opts: *const fsspec_async_options,
    out: *mut *mut fsspec_operation,
    out_error: *mut *mut fsspec_async_error,
) -> i32 {
    start(out, out_error, || {
        handle::<AsyncFile, _>(value)?.info(options(opts)?)
    })
}

#[no_mangle]
pub unsafe extern "C" fn fsspec_async_file_seek(
    value: *const fsspec_async_file,
    offset: i64,
    whence: u32,
    opts: *const fsspec_async_options,
    out: *mut *mut fsspec_operation,
    out_error: *mut *mut fsspec_async_error,
) -> i32 {
    start(out, out_error, || {
        let position = match whence {
            0 if offset >= 0 => SeekFrom::Start(offset as u64),
            1 => SeekFrom::Current(offset),
            2 => SeekFrom::End(offset),
            _ => return Err(invalid("invalid seek whence or negative start offset")),
        };
        handle::<AsyncFile, _>(value)?.seek(position, options(opts)?)
    })
}
