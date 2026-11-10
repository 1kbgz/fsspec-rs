//! Registration of foreign backends: factories and vtables implemented in
//! C, C++, or another Rust extension that must not compile the registry in.

use std::collections::HashSet;
use std::ffi::{c_char, c_void, CStr};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::mem::{size_of, MaybeUninit};
use std::ptr;
use std::sync::{Mutex, PoisonError};

use fsspec_rs::{
    FileInfo, FileSystem, FilesystemFactory, FsError, FsFile, FsOptions, FsResult, OpenMode,
    OpenOptions, SharedFileSystem, DEFAULT_FILESYSTEMS,
};

use crate::file::fsspec_file;
use crate::filesystem::fsspec_fs;
use crate::memory::{fsspec_file_info, fsspec_file_info_list, fsspec_string, fsspec_string_list};
use crate::options::fsspec_options;
use crate::status::{
    check, fsspec_status, guard, guard_ptr, invalid, io_error, str_arg, write_out,
    FSSPEC_ABI_VERSION,
};

/// Builds filesystems for one protocol. `struct_size` is
/// `sizeof(fsspec_fs_factory)` and `abi_version` is `FSSPEC_ABI_VERSION` as
/// seen by the caller; new members are appended, so older callers keep
/// working. `create` receives the URL path without `protocol://` and returns
/// a handle from `fsspec_fs_from_vtable` or another `fsspec_fs` function.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct fsspec_fs_factory {
    pub struct_size: usize,
    pub abi_version: u32,
    pub data: *mut c_void,
    pub create: Option<
        unsafe extern "C" fn(
            data: *mut c_void,
            path: *const c_char,
            path_len: usize,
            options: *const fsspec_options,
            out: *mut *mut fsspec_fs,
        ) -> fsspec_status,
    >,
    /// The part of the path that needs its own instance, such as a bucket.
    /// Optional; the registry caches one instance per protocol, scope, and
    /// option set.
    pub instance_scope: Option<
        unsafe extern "C" fn(
            data: *mut c_void,
            path: *const c_char,
            path_len: usize,
            out: *mut fsspec_string,
        ) -> fsspec_status,
    >,
    pub release: Option<unsafe extern "C" fn(data: *mut c_void)>,
}

/// A foreign filesystem. `protocol` is a NUL-terminated, comma-separated list
/// such as `"file,local"`; `root_marker` and `sep` may be null. The
/// operations mirror the `FileSystem` primitives; everything else is derived
/// from them. Output values are allocated with libfsspec's own functions and
/// freed by libfsspec.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct fsspec_fs_vtable {
    pub struct_size: usize,
    pub abi_version: u32,
    pub protocol: *const c_char,
    pub root_marker: *const c_char,
    pub sep: *const c_char,
    pub ls: Option<
        unsafe extern "C" fn(
            data: *mut c_void,
            path: *const c_char,
            path_len: usize,
            detail: bool,
            out: *mut fsspec_file_info_list,
        ) -> fsspec_status,
    >,
    pub info: Option<
        unsafe extern "C" fn(
            data: *mut c_void,
            path: *const c_char,
            path_len: usize,
            out: *mut fsspec_file_info,
        ) -> fsspec_status,
    >,
    pub rm_file: Option<
        unsafe extern "C" fn(
            data: *mut c_void,
            path: *const c_char,
            path_len: usize,
        ) -> fsspec_status,
    >,
    pub cp_file: Option<
        unsafe extern "C" fn(
            data: *mut c_void,
            src: *const c_char,
            src_len: usize,
            dst: *const c_char,
            dst_len: usize,
        ) -> fsspec_status,
    >,
    pub mkdir: Option<
        unsafe extern "C" fn(
            data: *mut c_void,
            path: *const c_char,
            path_len: usize,
            create_parents: bool,
        ) -> fsspec_status,
    >,
    pub rmdir: Option<
        unsafe extern "C" fn(
            data: *mut c_void,
            path: *const c_char,
            path_len: usize,
        ) -> fsspec_status,
    >,
    pub open: Option<
        unsafe extern "C" fn(
            data: *mut c_void,
            path: *const c_char,
            path_len: usize,
            mode: *const c_char,
            mode_len: usize,
            out: *mut *mut fsspec_file,
        ) -> fsspec_status,
    >,
    pub release: Option<unsafe extern "C" fn(data: *mut c_void)>,
}

/// A foreign open file. `read` and `write` move at most `len` bytes and
/// report the count; `seek` takes `whence` as in `fseek`. `size` reports -1
/// when unknown. Optional members may be null.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct fsspec_file_vtable {
    pub struct_size: usize,
    pub abi_version: u32,
    pub read: Option<
        unsafe extern "C" fn(
            data: *mut c_void,
            buf: *mut u8,
            len: usize,
            out_read: *mut usize,
        ) -> fsspec_status,
    >,
    pub write: Option<
        unsafe extern "C" fn(
            data: *mut c_void,
            buf: *const u8,
            len: usize,
            out_written: *mut usize,
        ) -> fsspec_status,
    >,
    pub seek: Option<
        unsafe extern "C" fn(
            data: *mut c_void,
            offset: i64,
            whence: i32,
            out_position: *mut u64,
        ) -> fsspec_status,
    >,
    pub flush: Option<unsafe extern "C" fn(data: *mut c_void) -> fsspec_status>,
    pub size: Option<unsafe extern "C" fn(data: *mut c_void, out_size: *mut i64) -> fsspec_status>,
    pub info: Option<
        unsafe extern "C" fn(data: *mut c_void, out: *mut fsspec_file_info) -> fsspec_status,
    >,
    pub commit: Option<unsafe extern "C" fn(data: *mut c_void) -> fsspec_status>,
    pub discard: Option<unsafe extern "C" fn(data: *mut c_void) -> fsspec_status>,
    pub release: Option<unsafe extern "C" fn(data: *mut c_void)>,
}

/// Copies a versioned struct, honoring the caller's `struct_size` so a
/// shorter struct from an older header reads as zero-filled.
unsafe fn copy_versioned<T: Copy>(pointer: *const T, what: &str) -> FsResult<T> {
    if pointer.is_null() {
        return Err(invalid(&format!("{what} is null")));
    }
    let struct_size = ptr::read_unaligned(pointer as *const usize);
    let header = size_of::<usize>() + size_of::<u32>();
    if struct_size < header || struct_size > 1 << 16 {
        return Err(invalid(&format!(
            "{what} has an invalid struct_size: {struct_size}"
        )));
    }
    let mut value = MaybeUninit::<T>::zeroed();
    ptr::copy_nonoverlapping(
        pointer as *const u8,
        value.as_mut_ptr() as *mut u8,
        struct_size.min(size_of::<T>()),
    );
    let value = value.assume_init();
    let abi_version =
        ptr::read_unaligned((pointer as *const u8).add(size_of::<usize>()) as *const u32);
    if abi_version != FSSPEC_ABI_VERSION {
        return Err(invalid(&format!(
            "{what} was built for ABI version {abi_version}, this library is version {FSSPEC_ABI_VERSION}"
        )));
    }
    Ok(value)
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

unsafe fn cstr_field(pointer: *const c_char, default: &str) -> String {
    if pointer.is_null() {
        default.to_string()
    } else {
        CStr::from_ptr(pointer).to_string_lossy().into_owned()
    }
}

struct ForeignFactory {
    factory: fsspec_fs_factory,
}

unsafe impl Send for ForeignFactory {}
unsafe impl Sync for ForeignFactory {}

impl FilesystemFactory for ForeignFactory {
    fn create(&self, path: &str, options: &FsOptions) -> FsResult<SharedFileSystem> {
        let create = self.factory.create.ok_or_else(|| {
            FsError::NotSupported("foreign factory has no create function".to_string())
        })?;
        let options = fsspec_options {
            inner: options.clone(),
        };
        let mut out: *mut fsspec_fs = ptr::null_mut();
        let status = unsafe {
            create(
                self.factory.data,
                path.as_ptr() as *const c_char,
                path.len(),
                &options,
                &mut out,
            )
        };
        check(status, "foreign factory create")?;
        if out.is_null() {
            return Err(FsError::Other(
                "foreign factory returned no filesystem".to_string(),
            ));
        }
        Ok(unsafe { Box::from_raw(out) }.inner)
    }

    fn instance_scope(&self, path: &str) -> String {
        let Some(instance_scope) = self.factory.instance_scope else {
            return String::new();
        };
        let mut out = fsspec_string::null();
        let status = unsafe {
            instance_scope(
                self.factory.data,
                path.as_ptr() as *const c_char,
                path.len(),
                &mut out,
            )
        };
        if status != fsspec_status::FSSPEC_OK {
            return String::new();
        }
        let scope = unsafe { out.as_str().into_owned() };
        unsafe { out.free() };
        scope
    }
}

impl Drop for ForeignFactory {
    fn drop(&mut self) {
        if let Some(release) = self.factory.release {
            unsafe { release(self.factory.data) };
        }
    }
}

struct ForeignFileSystem {
    vtable: fsspec_fs_vtable,
    data: *mut c_void,
    protocols: Vec<&'static str>,
    root_marker: String,
    sep: String,
}

unsafe impl Send for ForeignFileSystem {}
unsafe impl Sync for ForeignFileSystem {}

impl ForeignFileSystem {
    fn missing(&self, operation: &str) -> FsError {
        FsError::NotSupported(format!(
            "{} backend does not implement {operation}",
            self.protocols[0]
        ))
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
        let ls = self.vtable.ls.ok_or_else(|| self.missing("ls"))?;
        let mut out = fsspec_file_info_list {
            items: ptr::null_mut(),
            len: 0,
        };
        check(
            unsafe {
                ls(
                    self.data,
                    path.as_ptr() as *const c_char,
                    path.len(),
                    detail,
                    &mut out,
                )
            },
            "ls",
        )?;
        Ok(unsafe { out.into_infos() })
    }

    fn rm_file(&self, path: &str) -> FsResult<()> {
        let rm_file = self.vtable.rm_file.ok_or_else(|| self.missing("rm_file"))?;
        check(
            unsafe { rm_file(self.data, path.as_ptr() as *const c_char, path.len()) },
            "rm_file",
        )
    }

    fn cp_file(&self, src: &str, dst: &str) -> FsResult<()> {
        let cp_file = self.vtable.cp_file.ok_or_else(|| self.missing("cp_file"))?;
        check(
            unsafe {
                cp_file(
                    self.data,
                    src.as_ptr() as *const c_char,
                    src.len(),
                    dst.as_ptr() as *const c_char,
                    dst.len(),
                )
            },
            "cp_file",
        )
    }

    fn open(
        &self,
        path: &str,
        mode: OpenMode,
        _opts: Option<OpenOptions>,
    ) -> FsResult<Box<dyn FsFile>> {
        let open = self.vtable.open.ok_or_else(|| self.missing("open"))?;
        let mode = mode.as_str();
        let mut out: *mut fsspec_file = ptr::null_mut();
        check(
            unsafe {
                open(
                    self.data,
                    path.as_ptr() as *const c_char,
                    path.len(),
                    mode.as_ptr() as *const c_char,
                    mode.len(),
                    &mut out,
                )
            },
            "open",
        )?;
        if out.is_null() {
            return Err(FsError::Other(
                "foreign backend returned no file".to_string(),
            ));
        }
        Ok(unsafe { Box::from_raw(out) }.inner)
    }

    fn info(&self, path: &str) -> FsResult<FileInfo> {
        let info = self.vtable.info.ok_or_else(|| self.missing("info"))?;
        let mut out = MaybeUninit::<fsspec_file_info>::zeroed();
        check(
            unsafe {
                info(
                    self.data,
                    path.as_ptr() as *const c_char,
                    path.len(),
                    out.as_mut_ptr(),
                )
            },
            "info",
        )?;
        Ok(unsafe { out.assume_init().into_info() })
    }

    fn mkdir(&self, path: &str, create_parents: bool) -> FsResult<()> {
        let mkdir = self.vtable.mkdir.ok_or_else(|| self.missing("mkdir"))?;
        check(
            unsafe {
                mkdir(
                    self.data,
                    path.as_ptr() as *const c_char,
                    path.len(),
                    create_parents,
                )
            },
            "mkdir",
        )
    }

    fn rmdir(&self, path: &str) -> FsResult<()> {
        let rmdir = self.vtable.rmdir.ok_or_else(|| self.missing("rmdir"))?;
        check(
            unsafe { rmdir(self.data, path.as_ptr() as *const c_char, path.len()) },
            "rmdir",
        )
    }
}

impl Drop for ForeignFileSystem {
    fn drop(&mut self) {
        if let Some(release) = self.vtable.release {
            unsafe { release(self.data) };
        }
    }
}

struct ForeignFile {
    vtable: fsspec_file_vtable,
    data: *mut c_void,
}

unsafe impl Send for ForeignFile {}

impl ForeignFile {
    fn unsupported(operation: &str) -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            format!("foreign file does not implement {operation}"),
        )
    }
}

impl Read for ForeignFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.vtable.read.ok_or_else(|| Self::unsupported("read"))?;
        let mut count = 0;
        check(
            unsafe { read(self.data, buf.as_mut_ptr(), buf.len(), &mut count) },
            "read",
        )
        .map_err(io_error)?;
        Ok(count.min(buf.len()))
    }
}

impl Write for ForeignFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let write = self
            .vtable
            .write
            .ok_or_else(|| Self::unsupported("write"))?;
        let mut count = 0;
        check(
            unsafe { write(self.data, buf.as_ptr(), buf.len(), &mut count) },
            "write",
        )
        .map_err(io_error)?;
        Ok(count.min(buf.len()))
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.vtable.flush {
            Some(flush) => check(unsafe { flush(self.data) }, "flush").map_err(io_error),
            None => Ok(()),
        }
    }
}

impl Seek for ForeignFile {
    fn seek(&mut self, target: SeekFrom) -> io::Result<u64> {
        let seek = self.vtable.seek.ok_or_else(|| Self::unsupported("seek"))?;
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
            unsafe { seek(self.data, offset, whence, &mut position) },
            "seek",
        )
        .map_err(io_error)?;
        Ok(position)
    }
}

impl FsFile for ForeignFile {
    fn info(&self) -> FsResult<FileInfo> {
        let info = self.vtable.info.ok_or_else(|| {
            FsError::NotSupported("foreign file does not implement info".to_string())
        })?;
        let mut out = MaybeUninit::<fsspec_file_info>::zeroed();
        check(unsafe { info(self.data, out.as_mut_ptr()) }, "info")?;
        Ok(unsafe { out.assume_init().into_info() })
    }

    fn size(&self) -> FsResult<Option<u64>> {
        let Some(size) = self.vtable.size else {
            return Ok(None);
        };
        let mut out = -1;
        check(unsafe { size(self.data, &mut out) }, "size")?;
        Ok(u64::try_from(out).ok())
    }

    fn commit(&mut self) -> FsResult<()> {
        match self.vtable.commit {
            Some(commit) => check(unsafe { commit(self.data) }, "commit"),
            None => Ok(()),
        }
    }

    fn discard(&mut self) -> FsResult<()> {
        match self.vtable.discard {
            Some(discard) => check(unsafe { discard(self.data) }, "discard"),
            None => Ok(()),
        }
    }
}

impl Drop for ForeignFile {
    fn drop(&mut self) {
        if let Some(release) = self.vtable.release {
            unsafe { release(self.data) };
        }
    }
}

/// Registers a factory for `protocol` in the process-wide registry,
/// replacing an earlier one. The factory struct is copied.
#[no_mangle]
pub unsafe extern "C" fn fsspec_register_filesystem(
    protocol: *const c_char,
    protocol_len: usize,
    factory: *const fsspec_fs_factory,
) -> fsspec_status {
    guard(|| {
        let protocol = str_arg(protocol, protocol_len)?;
        if protocol.is_empty() {
            return Err(invalid("protocol is empty"));
        }
        let factory = copy_versioned(factory, "factory")?;
        fsspec_rs::register_filesystem(protocol, ForeignFactory { factory });
        Ok(())
    })
}

/// The registered protocols in sorted order.
#[no_mangle]
pub unsafe extern "C" fn fsspec_protocols(out: *mut fsspec_string_list) -> fsspec_status {
    guard(|| {
        let protocols = DEFAULT_FILESYSTEMS
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .protocols();
        write_out(out, fsspec_string_list::from_strings(protocols))
    })
}

/// Wraps a foreign implementation as a filesystem handle. The vtable is
/// copied; `data` is passed to every callback and to `release` when the last
/// handle goes away. Returns null and sets the error message on failure.
#[no_mangle]
pub unsafe extern "C" fn fsspec_fs_from_vtable(
    vtable: *const fsspec_fs_vtable,
    data: *mut c_void,
) -> *mut fsspec_fs {
    guard_ptr(|| {
        let vtable = copy_versioned(vtable, "filesystem vtable")?;
        if vtable.protocol.is_null() {
            return Err(invalid("filesystem vtable has no protocol"));
        }
        let protocols: Vec<&'static str> = cstr_field(vtable.protocol, "")
            .split(',')
            .map(str::trim)
            .filter(|protocol| !protocol.is_empty())
            .map(intern)
            .collect();
        if protocols.is_empty() {
            return Err(invalid("filesystem vtable has an empty protocol"));
        }
        let fs = ForeignFileSystem {
            root_marker: cstr_field(vtable.root_marker, ""),
            sep: cstr_field(vtable.sep, "/"),
            vtable,
            data,
            protocols,
        };
        Ok(fsspec_fs::into_raw(std::sync::Arc::new(fs)))
    })
}

/// Wraps a foreign open file. The vtable is copied; `data` is passed to every
/// callback and to `release` on close. Returns null and sets the error
/// message on failure.
#[no_mangle]
pub unsafe extern "C" fn fsspec_file_from_vtable(
    vtable: *const fsspec_file_vtable,
    data: *mut c_void,
) -> *mut fsspec_file {
    guard_ptr(|| {
        let vtable = copy_versioned(vtable, "file vtable")?;
        Ok(fsspec_file::into_raw(Box::new(ForeignFile {
            vtable,
            data,
        })))
    })
}
