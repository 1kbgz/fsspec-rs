//! Exporting Rust implementations through the vtables of `fsspec.h`, so a
//! backend compiled into another shared library can live in the registry of
//! `libfsspec`.

use std::ffi::{c_char, c_void, CString};
use std::io::{Read, Seek, SeekFrom, Write};
use std::mem::size_of;

use fsspec_rs::{FilesystemFactory, FsError, FsFile, FsResult, OpenMode, SharedFileSystem};

use crate::ffi;
use crate::options_from_handle;
use crate::types::*;
use crate::util::{arg, check, guard, invalid, str_arg, write_out};

struct FsData {
    fs: SharedFileSystem,
}

unsafe fn fs_data<'a>(data: *mut c_void) -> &'a SharedFileSystem {
    &(*(data as *const FsData)).fs
}

unsafe extern "C" fn fs_ls(
    data: *mut c_void,
    path: *const c_char,
    len: usize,
    detail: bool,
    out: *mut fsspec_file_info_list,
) -> fsspec_status {
    guard(|| {
        let entries = fs_data(data).ls(str_arg(path, len)?, detail)?;
        write_out(out, fsspec_file_info_list::from_infos(entries))
    })
}

unsafe extern "C" fn fs_info(
    data: *mut c_void,
    path: *const c_char,
    len: usize,
    out: *mut fsspec_file_info,
) -> fsspec_status {
    guard(|| {
        let info = fs_data(data).info(str_arg(path, len)?)?;
        write_out(out, fsspec_file_info::from_info(info))
    })
}

unsafe extern "C" fn fs_rm_file(
    data: *mut c_void,
    path: *const c_char,
    len: usize,
) -> fsspec_status {
    guard(|| fs_data(data).rm_file(str_arg(path, len)?))
}

unsafe extern "C" fn fs_cp_file(
    data: *mut c_void,
    src: *const c_char,
    src_len: usize,
    dst: *const c_char,
    dst_len: usize,
) -> fsspec_status {
    guard(|| fs_data(data).cp_file(str_arg(src, src_len)?, str_arg(dst, dst_len)?))
}

unsafe extern "C" fn fs_mkdir(
    data: *mut c_void,
    path: *const c_char,
    len: usize,
    create_parents: bool,
) -> fsspec_status {
    guard(|| fs_data(data).mkdir(str_arg(path, len)?, create_parents))
}

unsafe extern "C" fn fs_rmdir(data: *mut c_void, path: *const c_char, len: usize) -> fsspec_status {
    guard(|| fs_data(data).rmdir(str_arg(path, len)?))
}

unsafe extern "C" fn fs_open(
    data: *mut c_void,
    path: *const c_char,
    len: usize,
    mode: *const c_char,
    mode_len: usize,
    out: *mut *mut fsspec_file,
) -> fsspec_status {
    guard(|| {
        let mode = str_arg(mode, mode_len)?;
        let mode = OpenMode::from_str_mode(mode)
            .ok_or_else(|| FsError::InvalidArgument(format!("unsupported open mode: {mode}")))?;
        let file = fs_data(data).open(str_arg(path, len)?, mode, None)?;
        write_out(out, export_file(file)?)
    })
}

unsafe extern "C" fn fs_release(data: *mut c_void) {
    drop(Box::from_raw(data as *mut FsData));
}

/// Hands a filesystem to `libfsspec` as an `fsspec_fs` handle. The handle
/// owns a reference; `libfsspec` drops it with the last `fsspec_fs_release`.
pub fn export_filesystem(fs: SharedFileSystem) -> FsResult<*mut fsspec_fs> {
    let protocol = CString::new(fs.protocol().join(","))
        .map_err(|_| invalid("protocol contains a NUL byte"))?;
    let root_marker =
        CString::new(fs.root_marker()).map_err(|_| invalid("root marker contains a NUL byte"))?;
    let sep = CString::new(fs.sep()).map_err(|_| invalid("separator contains a NUL byte"))?;
    let vtable = fsspec_fs_vtable {
        struct_size: size_of::<fsspec_fs_vtable>(),
        abi_version: FSSPEC_ABI_VERSION,
        protocol: protocol.as_ptr(),
        root_marker: root_marker.as_ptr(),
        sep: sep.as_ptr(),
        ls: Some(fs_ls),
        info: Some(fs_info),
        rm_file: Some(fs_rm_file),
        cp_file: Some(fs_cp_file),
        mkdir: Some(fs_mkdir),
        rmdir: Some(fs_rmdir),
        open: Some(fs_open),
        release: Some(fs_release),
    };
    let data = Box::into_raw(Box::new(FsData { fs })) as *mut c_void;
    let handle = unsafe { ffi::fsspec_fs_from_vtable(&vtable, data) };
    if handle.is_null() {
        unsafe { fs_release(data) };
        return Err(check(FSSPEC_OTHER, "fsspec_fs_from_vtable").unwrap_err());
    }
    Ok(handle)
}

unsafe fn file_data<'a>(data: *mut c_void) -> &'a mut Box<dyn FsFile> {
    &mut *(data as *mut Box<dyn FsFile>)
}

unsafe extern "C" fn file_read(
    data: *mut c_void,
    buf: *mut u8,
    len: usize,
    out: *mut usize,
) -> fsspec_status {
    guard(|| {
        let buf = if len == 0 {
            &mut [][..]
        } else {
            std::slice::from_raw_parts_mut(buf, len)
        };
        let read = file_data(data).read(buf)?;
        write_out(out, read)
    })
}

unsafe extern "C" fn file_write(
    data: *mut c_void,
    buf: *const u8,
    len: usize,
    out: *mut usize,
) -> fsspec_status {
    guard(|| {
        let buf = if len == 0 {
            &[][..]
        } else {
            std::slice::from_raw_parts(buf, len)
        };
        let written = file_data(data).write(buf)?;
        write_out(out, written)
    })
}

unsafe extern "C" fn file_seek(
    data: *mut c_void,
    offset: i64,
    whence: i32,
    out: *mut u64,
) -> fsspec_status {
    guard(|| {
        let target = match whence {
            0 => SeekFrom::Start(
                u64::try_from(offset).map_err(|_| invalid("negative offset from start"))?,
            ),
            1 => SeekFrom::Current(offset),
            2 => SeekFrom::End(offset),
            _ => return Err(invalid("whence must be 0, 1, or 2")),
        };
        let position = file_data(data).seek(target)?;
        write_out(out, position)
    })
}

unsafe extern "C" fn file_flush(data: *mut c_void) -> fsspec_status {
    guard(|| Ok(file_data(data).flush()?))
}

unsafe extern "C" fn file_size(data: *mut c_void, out: *mut i64) -> fsspec_status {
    guard(|| {
        let size = file_data(data)
            .size()?
            .and_then(|size| i64::try_from(size).ok())
            .unwrap_or(-1);
        write_out(out, size)
    })
}

unsafe extern "C" fn file_info(data: *mut c_void, out: *mut fsspec_file_info) -> fsspec_status {
    guard(|| {
        let info = file_data(data).info()?;
        write_out(out, fsspec_file_info::from_info(info))
    })
}

unsafe extern "C" fn file_commit(data: *mut c_void) -> fsspec_status {
    guard(|| file_data(data).commit())
}

unsafe extern "C" fn file_discard(data: *mut c_void) -> fsspec_status {
    guard(|| file_data(data).discard())
}

unsafe extern "C" fn file_release(data: *mut c_void) {
    drop(Box::from_raw(data as *mut Box<dyn FsFile>));
}

/// Hands an open file to `libfsspec` as an `fsspec_file` handle, closed by
/// `fsspec_file_close`.
pub fn export_file(file: Box<dyn FsFile>) -> FsResult<*mut fsspec_file> {
    let vtable = fsspec_file_vtable {
        struct_size: size_of::<fsspec_file_vtable>(),
        abi_version: FSSPEC_ABI_VERSION,
        read: Some(file_read),
        write: Some(file_write),
        seek: Some(file_seek),
        flush: Some(file_flush),
        size: Some(file_size),
        info: Some(file_info),
        commit: Some(file_commit),
        discard: Some(file_discard),
        release: Some(file_release),
    };
    let data = Box::into_raw(Box::new(file)) as *mut c_void;
    let handle = unsafe { ffi::fsspec_file_from_vtable(&vtable, data) };
    if handle.is_null() {
        unsafe { file_release(data) };
        return Err(check(FSSPEC_OTHER, "fsspec_file_from_vtable").unwrap_err());
    }
    Ok(handle)
}

type BoxedFactory = Box<dyn FilesystemFactory>;

unsafe fn factory_data<'a>(data: *mut c_void) -> &'a BoxedFactory {
    &*(data as *const BoxedFactory)
}

unsafe extern "C" fn factory_create(
    data: *mut c_void,
    path: *const c_char,
    len: usize,
    options: *const fsspec_options,
    out: *mut *mut fsspec_fs,
) -> fsspec_status {
    guard(|| {
        let options = options_from_handle(options)?;
        let fs = factory_data(data).create(str_arg(path, len)?, &options)?;
        write_out(out, export_filesystem(fs)?)
    })
}

unsafe extern "C" fn factory_scope(
    data: *mut c_void,
    path: *const c_char,
    len: usize,
    out: *mut fsspec_string,
) -> fsspec_status {
    guard(|| {
        let scope = factory_data(data).instance_scope(str_arg(path, len)?);
        write_out(out, fsspec_string::from_string(scope))
    })
}

unsafe extern "C" fn factory_release(data: *mut c_void) {
    drop(Box::from_raw(data as *mut BoxedFactory));
}

pub(crate) fn register_factory(protocol: &str, factory: BoxedFactory) -> FsResult<()> {
    let data = Box::into_raw(Box::new(factory)) as *mut c_void;
    let descriptor = fsspec_fs_factory {
        struct_size: size_of::<fsspec_fs_factory>(),
        abi_version: FSSPEC_ABI_VERSION,
        data,
        create: Some(factory_create),
        instance_scope: Some(factory_scope),
        release: Some(factory_release),
    };
    let (protocol_ptr, protocol_len) = arg(protocol);
    let status =
        unsafe { ffi::fsspec_register_filesystem(protocol_ptr, protocol_len, &descriptor) };
    if status != FSSPEC_OK {
        unsafe { factory_release(data) };
    }
    check(status, "fsspec_register_filesystem")
}
