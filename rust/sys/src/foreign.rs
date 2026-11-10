//! `FileSystem` and `FsFile` over `libfsspec` handles.

use std::collections::HashSet;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::mem::MaybeUninit;
use std::ptr;
use std::sync::{Mutex, PoisonError};

use fsspec_rs::{
    CacheType, FileInfo, FileSystem, FsError, FsFile, FsOptions, FsResult, OpenMode, OpenOptions,
};

use crate::ffi;
use crate::types::*;
use crate::util::{
    arg, check, empty_bytes, empty_info_list, empty_string_list, io_error, null_string,
};
use crate::Options;

/// Reads and frees a string returned by libfsspec.
pub(crate) unsafe fn take_string(string: fsspec_string) -> String {
    let value = string.as_str().into_owned();
    ffi::fsspec_string_free(string);
    value
}

pub(crate) unsafe fn take_strings(list: fsspec_string_list) -> Vec<String> {
    let values = list.to_strings();
    ffi::fsspec_string_list_free(list);
    values
}

unsafe fn take_bytes(bytes: fsspec_bytes) -> Vec<u8> {
    let value = bytes.as_slice().to_vec();
    ffi::fsspec_bytes_free(bytes);
    value
}

unsafe fn take_info(info: fsspec_file_info) -> FileInfo {
    let value = info.to_info();
    ffi::fsspec_file_info_free(info);
    value
}

unsafe fn take_infos(list: fsspec_file_info_list) -> Vec<FileInfo> {
    let values = list.to_infos();
    ffi::fsspec_file_info_list_free(list);
    values
}

/// Protocol names live for the process, since `FileSystem::protocol` hands
/// out borrowed slices. Each distinct name is leaked once.
fn intern(protocol: &str) -> &'static str {
    static INTERNED: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut guard = INTERNED.lock().unwrap_or_else(PoisonError::into_inner);
    let set = guard.get_or_insert_with(HashSet::new);
    if let Some(existing) = set.get(protocol) {
        return existing;
    }
    let leaked: &'static str = Box::leak(protocol.to_owned().into_boxed_str());
    set.insert(leaked);
    leaked
}

/// The string map `fsspec_fs_open_ex` reads for an `OpenOptions`.
pub fn open_options_map(opts: &OpenOptions) -> FsOptions {
    let mut options = FsOptions::new();
    options.insert("block_size".to_string(), opts.block_size.to_string());
    options.insert("max_blocks".to_string(), opts.max_blocks.to_string());
    options.insert("autocommit".to_string(), opts.autocommit.to_string());
    if let Some(cache_type) = &opts.cache_type {
        let name = match cache_type {
            CacheType::None => "none",
            CacheType::ReadAhead => "readahead",
            CacheType::Block => "block",
            CacheType::All => "all",
        };
        options.insert("cache_type".to_string(), name.to_string());
    }
    options
}

/// A filesystem owned by `libfsspec`, reached through its C ABI. Dropping it
/// releases the handle; cloning retains another.
pub struct ForeignFileSystem {
    handle: *mut fsspec_fs,
    protocols: Vec<&'static str>,
    root_marker: String,
    sep: String,
}

unsafe impl Send for ForeignFileSystem {}
unsafe impl Sync for ForeignFileSystem {}

impl ForeignFileSystem {
    /// Takes ownership of a handle returned by libfsspec.
    pub unsafe fn from_raw(handle: *mut fsspec_fs) -> FsResult<Self> {
        if handle.is_null() {
            return Err(FsError::Other(
                "libfsspec returned a null filesystem".to_string(),
            ));
        }
        let mut protocols = empty_string_list();
        check(
            ffi::fsspec_fs_protocols(handle, &mut protocols),
            "fsspec_fs_protocols",
        )?;
        let protocols = take_strings(protocols)
            .iter()
            .map(|protocol| intern(protocol))
            .collect();
        let mut root_marker = null_string();
        check(
            ffi::fsspec_fs_root_marker(handle, &mut root_marker),
            "fsspec_fs_root_marker",
        )?;
        let mut sep = null_string();
        check(ffi::fsspec_fs_sep(handle, &mut sep), "fsspec_fs_sep")?;
        Ok(Self {
            handle,
            protocols,
            root_marker: take_string(root_marker),
            sep: take_string(sep),
        })
    }

    /// The underlying handle, still owned by this value.
    pub fn as_ptr(&self) -> *const fsspec_fs {
        self.handle
    }
}

impl Clone for ForeignFileSystem {
    fn clone(&self) -> Self {
        Self {
            handle: unsafe { ffi::fsspec_fs_retain(self.handle) },
            protocols: self.protocols.clone(),
            root_marker: self.root_marker.clone(),
            sep: self.sep.clone(),
        }
    }
}

impl Drop for ForeignFileSystem {
    fn drop(&mut self) {
        unsafe { ffi::fsspec_fs_release(self.handle) }
    }
}

impl FileSystem for ForeignFileSystem {
    fn protocol(&self) -> &[&str] {
        &self.protocols
    }

    fn root_marker(&self) -> &str {
        &self.root_marker
    }

    fn sep(&self) -> &str {
        &self.sep
    }

    fn ls(&self, path: &str, detail: bool) -> FsResult<Vec<FileInfo>> {
        let (path_ptr, path_len) = arg(path);
        let mut out = empty_info_list();
        check(
            unsafe { ffi::fsspec_fs_ls(self.handle, path_ptr, path_len, detail, &mut out) },
            "fsspec_fs_ls",
        )?;
        Ok(unsafe { take_infos(out) })
    }

    fn rm_file(&self, path: &str) -> FsResult<()> {
        let (path_ptr, path_len) = arg(path);
        check(
            unsafe { ffi::fsspec_fs_rm_file(self.handle, path_ptr, path_len) },
            "fsspec_fs_rm_file",
        )
    }

    fn cp_file(&self, src: &str, dst: &str) -> FsResult<()> {
        let (src_ptr, src_len) = arg(src);
        let (dst_ptr, dst_len) = arg(dst);
        check(
            unsafe { ffi::fsspec_fs_cp_file(self.handle, src_ptr, src_len, dst_ptr, dst_len) },
            "fsspec_fs_cp_file",
        )
    }

    fn open(
        &self,
        path: &str,
        mode: OpenMode,
        opts: Option<OpenOptions>,
    ) -> FsResult<Box<dyn FsFile>> {
        let (path_ptr, path_len) = arg(path);
        let (mode_ptr, mode_len) = arg(mode.as_str());
        let mut out = ptr::null_mut();
        let status = match opts {
            Some(opts) => {
                let options = Options::new(&open_options_map(&opts))?;
                unsafe {
                    ffi::fsspec_fs_open_ex(
                        self.handle,
                        path_ptr,
                        path_len,
                        mode_ptr,
                        mode_len,
                        options.0,
                        &mut out,
                    )
                }
            }
            None => unsafe {
                ffi::fsspec_fs_open(
                    self.handle,
                    path_ptr,
                    path_len,
                    mode_ptr,
                    mode_len,
                    &mut out,
                )
            },
        };
        check(status, "fsspec_fs_open")?;
        if out.is_null() {
            return Err(FsError::Other("libfsspec returned a null file".to_string()));
        }
        Ok(Box::new(ForeignFile { handle: out }))
    }

    fn info(&self, path: &str) -> FsResult<FileInfo> {
        let (path_ptr, path_len) = arg(path);
        let mut out = MaybeUninit::<fsspec_file_info>::zeroed();
        check(
            unsafe { ffi::fsspec_fs_info(self.handle, path_ptr, path_len, out.as_mut_ptr()) },
            "fsspec_fs_info",
        )?;
        Ok(unsafe { take_info(out.assume_init()) })
    }

    fn mkdir(&self, path: &str, create_parents: bool) -> FsResult<()> {
        let (path_ptr, path_len) = arg(path);
        check(
            unsafe { ffi::fsspec_fs_mkdir(self.handle, path_ptr, path_len, create_parents) },
            "fsspec_fs_mkdir",
        )
    }

    fn rmdir(&self, path: &str) -> FsResult<()> {
        let (path_ptr, path_len) = arg(path);
        check(
            unsafe { ffi::fsspec_fs_rmdir(self.handle, path_ptr, path_len) },
            "fsspec_fs_rmdir",
        )
    }

    fn exists(&self, path: &str) -> FsResult<bool> {
        let (path_ptr, path_len) = arg(path);
        let mut out = false;
        check(
            unsafe { ffi::fsspec_fs_exists(self.handle, path_ptr, path_len, &mut out) },
            "fsspec_fs_exists",
        )?;
        Ok(out)
    }

    fn cat_file(&self, path: &str, start: Option<i64>, end: Option<i64>) -> FsResult<Vec<u8>> {
        let (path_ptr, path_len) = arg(path);
        let mut out = empty_bytes();
        let status = unsafe {
            ffi::fsspec_fs_cat_file(
                self.handle,
                path_ptr,
                path_len,
                start.as_ref().map_or(ptr::null(), |value| value),
                end.as_ref().map_or(ptr::null(), |value| value),
                &mut out,
            )
        };
        check(status, "fsspec_fs_cat_file")?;
        Ok(unsafe { take_bytes(out) })
    }

    fn pipe_file(&self, path: &str, data: &[u8]) -> FsResult<()> {
        let (path_ptr, path_len) = arg(path);
        check(
            unsafe {
                ffi::fsspec_fs_pipe_file(self.handle, path_ptr, path_len, data.as_ptr(), data.len())
            },
            "fsspec_fs_pipe_file",
        )
    }
}

/// An open file owned by `libfsspec`. Dropping it closes the file.
pub struct ForeignFile {
    handle: *mut fsspec_file,
}

unsafe impl Send for ForeignFile {}

impl ForeignFile {
    /// Takes ownership of a file handle returned by libfsspec.
    pub unsafe fn from_raw(handle: *mut fsspec_file) -> FsResult<Self> {
        if handle.is_null() {
            return Err(FsError::Other("libfsspec returned a null file".to_string()));
        }
        Ok(Self { handle })
    }
}

impl Read for ForeignFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut count = 0;
        check(
            unsafe { ffi::fsspec_file_read(self.handle, buf.as_mut_ptr(), buf.len(), &mut count) },
            "fsspec_file_read",
        )
        .map_err(io_error)?;
        Ok(count.min(buf.len()))
    }
}

impl Write for ForeignFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut count = 0;
        check(
            unsafe { ffi::fsspec_file_write(self.handle, buf.as_ptr(), buf.len(), &mut count) },
            "fsspec_file_write",
        )
        .map_err(io_error)?;
        Ok(count.min(buf.len()))
    }

    fn flush(&mut self) -> io::Result<()> {
        check(
            unsafe { ffi::fsspec_file_flush(self.handle) },
            "fsspec_file_flush",
        )
        .map_err(io_error)
    }
}

impl Seek for ForeignFile {
    fn seek(&mut self, target: SeekFrom) -> io::Result<u64> {
        let (offset, whence) = match target {
            SeekFrom::Start(offset) => (
                i64::try_from(offset)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "offset too large"))?,
                0,
            ),
            SeekFrom::Current(offset) => (offset, 1),
            SeekFrom::End(offset) => (offset, 2),
        };
        let mut position = 0;
        check(
            unsafe { ffi::fsspec_file_seek(self.handle, offset, whence, &mut position) },
            "fsspec_file_seek",
        )
        .map_err(io_error)?;
        Ok(position)
    }
}

impl FsFile for ForeignFile {
    fn info(&self) -> FsResult<FileInfo> {
        let mut out = MaybeUninit::<fsspec_file_info>::zeroed();
        check(
            unsafe { ffi::fsspec_file_info(self.handle, out.as_mut_ptr()) },
            "fsspec_file_info",
        )?;
        Ok(unsafe { take_info(out.assume_init()) })
    }

    fn size(&self) -> FsResult<Option<u64>> {
        let mut out = -1;
        check(
            unsafe { ffi::fsspec_file_size(self.handle, &mut out) },
            "fsspec_file_size",
        )?;
        Ok(u64::try_from(out).ok())
    }

    fn commit(&mut self) -> FsResult<()> {
        check(
            unsafe { ffi::fsspec_file_commit(self.handle) },
            "fsspec_file_commit",
        )
    }

    fn discard(&mut self) -> FsResult<()> {
        check(
            unsafe { ffi::fsspec_file_discard(self.handle) },
            "fsspec_file_discard",
        )
    }
}

impl Drop for ForeignFile {
    fn drop(&mut self) {
        unsafe { ffi::fsspec_file_close(self.handle) };
    }
}
