use std::ffi::c_char;
use std::ptr;
use std::slice;

use fsspec_rs::{CacheType, FsError, FsOptions, FsResult, OpenMode, OpenOptions, SharedFileSystem};
use fsspec_sys::types::{
    fsspec_bytes, fsspec_file, fsspec_file_info, fsspec_file_info_list, fsspec_fs, fsspec_options,
    fsspec_status, fsspec_string, fsspec_string_list,
};

use crate::file::FileData;
use crate::options::options_arg;
use crate::status::{guard, handle_arg, invalid, str_arg, write_out};

/// The data behind an `fsspec_fs` handle.
pub(crate) struct FsData {
    pub(crate) inner: SharedFileSystem,
}

impl FsData {
    pub(crate) fn into_raw(inner: SharedFileSystem) -> *mut fsspec_fs {
        Box::into_raw(Box::new(Self { inner })) as *mut fsspec_fs
    }

    pub(crate) unsafe fn from_raw(handle: *mut fsspec_fs) -> Box<Self> {
        Box::from_raw(handle as *mut Self)
    }
}

pub(crate) unsafe fn fs_arg<'a>(fs: *const fsspec_fs) -> FsResult<&'a SharedFileSystem> {
    let data: &FsData = handle_arg(fs, "filesystem")?;
    Ok(&data.inner)
}

/// Resolves `url` through the process-wide registry. On success `out_fs`
/// receives a handle and `out_path`, when not null, the path to use with it.
#[no_mangle]
pub unsafe extern "C" fn fsspec_url_to_fs(
    url: *const c_char,
    url_len: usize,
    options: *const fsspec_options,
    out_fs: *mut *mut fsspec_fs,
    out_path: *mut fsspec_string,
) -> fsspec_status {
    guard(|| {
        let url = str_arg(url, url_len)?;
        if out_fs.is_null() {
            return Err(invalid("output filesystem pointer is null"));
        }
        let (fs, path) = fsspec_rs::url_to_fs(url, &options_arg(options))?;
        if !out_path.is_null() {
            out_path.write(fsspec_string::from_string(path));
        }
        out_fs.write(FsData::into_raw(fs));
        Ok(())
    })
}

/// Returns another handle to the same filesystem, or null for null.
#[no_mangle]
pub unsafe extern "C" fn fsspec_fs_retain(fs: *const fsspec_fs) -> *mut fsspec_fs {
    match (fs as *const FsData).as_ref() {
        Some(fs) => FsData::into_raw(fs.inner.clone()),
        None => ptr::null_mut(),
    }
}

/// Frees a handle. The filesystem is dropped with its last handle. Null is
/// ignored.
#[no_mangle]
pub unsafe extern "C" fn fsspec_fs_release(fs: *mut fsspec_fs) {
    if !fs.is_null() {
        drop(FsData::from_raw(fs));
    }
}

/// The protocols the filesystem answers to.
#[no_mangle]
pub unsafe extern "C" fn fsspec_fs_protocols(
    fs: *const fsspec_fs,
    out: *mut fsspec_string_list,
) -> fsspec_status {
    guard(|| {
        let fs = fs_arg(fs)?;
        let protocols = fs
            .protocol()
            .iter()
            .map(|protocol| protocol.to_string())
            .collect();
        write_out(out, fsspec_string_list::from_strings(protocols))
    })
}

/// The filesystem's root marker, such as `/` for local paths.
#[no_mangle]
pub unsafe extern "C" fn fsspec_fs_root_marker(
    fs: *const fsspec_fs,
    out: *mut fsspec_string,
) -> fsspec_status {
    guard(|| {
        let fs = fs_arg(fs)?;
        write_out(
            out,
            fsspec_string::from_string(fs.root_marker().to_string()),
        )
    })
}

/// The filesystem's path separator.
#[no_mangle]
pub unsafe extern "C" fn fsspec_fs_sep(
    fs: *const fsspec_fs,
    out: *mut fsspec_string,
) -> fsspec_status {
    guard(|| {
        let fs = fs_arg(fs)?;
        write_out(out, fsspec_string::from_string(fs.sep().to_string()))
    })
}

/// Lists a directory. With `detail` false, backends may fill only names.
#[no_mangle]
pub unsafe extern "C" fn fsspec_fs_ls(
    fs: *const fsspec_fs,
    path: *const c_char,
    path_len: usize,
    detail: bool,
    out: *mut fsspec_file_info_list,
) -> fsspec_status {
    guard(|| {
        let fs = fs_arg(fs)?;
        let entries = fs.ls(str_arg(path, path_len)?, detail)?;
        write_out(out, fsspec_file_info_list::from_infos(entries))
    })
}

/// Metadata for one path.
#[no_mangle]
pub unsafe extern "C" fn fsspec_fs_info(
    fs: *const fsspec_fs,
    path: *const c_char,
    path_len: usize,
    out: *mut fsspec_file_info,
) -> fsspec_status {
    guard(|| {
        let fs = fs_arg(fs)?;
        let info = fs.info(str_arg(path, path_len)?)?;
        write_out(out, fsspec_file_info::from_info(info))
    })
}

/// Whether a path exists.
#[no_mangle]
pub unsafe extern "C" fn fsspec_fs_exists(
    fs: *const fsspec_fs,
    path: *const c_char,
    path_len: usize,
    out: *mut bool,
) -> fsspec_status {
    guard(|| {
        let fs = fs_arg(fs)?;
        let exists = fs.exists(str_arg(path, path_len)?)?;
        write_out(out, exists)
    })
}

/// Reads a file, or the byte range `[*start, *end)` when the pointers are not
/// null. Negative offsets count from the end.
#[no_mangle]
pub unsafe extern "C" fn fsspec_fs_cat_file(
    fs: *const fsspec_fs,
    path: *const c_char,
    path_len: usize,
    start: *const i64,
    end: *const i64,
    out: *mut fsspec_bytes,
) -> fsspec_status {
    guard(|| {
        let fs = fs_arg(fs)?;
        let data = fs.cat_file(
            str_arg(path, path_len)?,
            start.as_ref().copied(),
            end.as_ref().copied(),
        )?;
        write_out(out, fsspec_bytes::from_vec(data))
    })
}

/// Writes `data` to a file, replacing it.
#[no_mangle]
pub unsafe extern "C" fn fsspec_fs_pipe_file(
    fs: *const fsspec_fs,
    path: *const c_char,
    path_len: usize,
    data: *const u8,
    data_len: usize,
) -> fsspec_status {
    guard(|| {
        let fs = fs_arg(fs)?;
        let path = str_arg(path, path_len)?;
        let data = if data.is_null() {
            if data_len != 0 {
                return Err(invalid("data pointer is null"));
            }
            &[][..]
        } else {
            slice::from_raw_parts(data, data_len)
        };
        fs.pipe_file(path, data)
    })
}

/// Deletes one file.
#[no_mangle]
pub unsafe extern "C" fn fsspec_fs_rm_file(
    fs: *const fsspec_fs,
    path: *const c_char,
    path_len: usize,
) -> fsspec_status {
    guard(|| fs_arg(fs)?.rm_file(str_arg(path, path_len)?))
}

/// Copies one file within the filesystem.
#[no_mangle]
pub unsafe extern "C" fn fsspec_fs_cp_file(
    fs: *const fsspec_fs,
    src: *const c_char,
    src_len: usize,
    dst: *const c_char,
    dst_len: usize,
) -> fsspec_status {
    guard(|| fs_arg(fs)?.cp_file(str_arg(src, src_len)?, str_arg(dst, dst_len)?))
}

/// Creates a directory, and its parents when `create_parents` is set.
#[no_mangle]
pub unsafe extern "C" fn fsspec_fs_mkdir(
    fs: *const fsspec_fs,
    path: *const c_char,
    path_len: usize,
    create_parents: bool,
) -> fsspec_status {
    guard(|| fs_arg(fs)?.mkdir(str_arg(path, path_len)?, create_parents))
}

/// Removes an empty directory.
#[no_mangle]
pub unsafe extern "C" fn fsspec_fs_rmdir(
    fs: *const fsspec_fs,
    path: *const c_char,
    path_len: usize,
) -> fsspec_status {
    guard(|| fs_arg(fs)?.rmdir(str_arg(path, path_len)?))
}

/// Opens a file with a Python-style mode (`rb`, `wb`, `ab`, `xb`). Close the
/// result with `fsspec_file_close`.
#[no_mangle]
pub unsafe extern "C" fn fsspec_fs_open(
    fs: *const fsspec_fs,
    path: *const c_char,
    path_len: usize,
    mode: *const c_char,
    mode_len: usize,
    out: *mut *mut fsspec_file,
) -> fsspec_status {
    guard(|| {
        let fs = fs_arg(fs)?;
        let path = str_arg(path, path_len)?;
        let mode = str_arg(mode, mode_len)?;
        let mode = OpenMode::from_str_mode(mode)
            .ok_or_else(|| FsError::InvalidArgument(format!("unsupported open mode: {mode}")))?;
        let file = fs.open(path, mode, None)?;
        write_out(out, FileData::into_raw(file))
    })
}

fn parse_open_options(options: &FsOptions) -> FsResult<OpenOptions> {
    let mut opts = OpenOptions::default();
    for (key, value) in options {
        let invalid_value = || {
            FsError::InvalidArgument(format!(
                "{key} must be a valid open option value, got {value:?}"
            ))
        };
        match key.as_str() {
            "block_size" => opts.block_size = value.parse().map_err(|_| invalid_value())?,
            "max_blocks" => opts.max_blocks = value.parse().map_err(|_| invalid_value())?,
            "autocommit" => opts.autocommit = value.parse().map_err(|_| invalid_value())?,
            "cache_type" => {
                opts.cache_type = Some(
                    value
                        .parse::<CacheType>()
                        .map_err(|error| FsError::InvalidArgument(error.to_string()))?,
                )
            }
            _ => {
                return Err(FsError::InvalidArgument(format!(
                    "unknown open option: {key}"
                )))
            }
        }
    }
    Ok(opts)
}

/// Opens a file like `fsspec_fs_open`, with open options from a string map:
/// `block_size`, `max_blocks`, `autocommit`, and `cache_type` (`none`,
/// `readahead`, `block`, `all`). Backends registered through a vtable
/// receive no options.
#[no_mangle]
pub unsafe extern "C" fn fsspec_fs_open_ex(
    fs: *const fsspec_fs,
    path: *const c_char,
    path_len: usize,
    mode: *const c_char,
    mode_len: usize,
    options: *const fsspec_options,
    out: *mut *mut fsspec_file,
) -> fsspec_status {
    guard(|| {
        let fs = fs_arg(fs)?;
        let path = str_arg(path, path_len)?;
        let mode = str_arg(mode, mode_len)?;
        let mode = OpenMode::from_str_mode(mode)
            .ok_or_else(|| FsError::InvalidArgument(format!("unsupported open mode: {mode}")))?;
        let opts = parse_open_options(&options_arg(options))?;
        let file = fs.open(path, mode, Some(opts))?;
        write_out(out, FileData::into_raw(file))
    })
}
