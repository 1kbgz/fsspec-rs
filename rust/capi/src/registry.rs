//! Registration of foreign backends: factories and vtables implemented in
//! C, C++, or another Rust extension that must not compile the registry in.

use std::collections::HashSet;
use std::ffi::{c_char, c_void, CStr};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::mem::{size_of, MaybeUninit};
use std::ptr;
use std::sync::{Arc, Mutex, PoisonError};

use fsspec_rs::{
    cat_file_default, pipe_file_default, strip_protocol_default, unstrip_protocol_default,
    FileInfo, FileSystem, FilesystemFactory, FsError, FsFile, FsOptions, FsResult, OpenMode,
    OpenOptions, SharedFileSystem, DEFAULT_FILESYSTEMS,
};
use fsspec_sys::types::{
    fsspec_bytes, fsspec_file, fsspec_file_info, fsspec_file_info_list, fsspec_file_vtable,
    fsspec_fs, fsspec_fs_factory, fsspec_fs_vtable, fsspec_options, fsspec_status, fsspec_string,
    fsspec_string_list,
};

use crate::file::FileData;
use crate::filesystem::FsData;
use crate::options::OptionsData;
use crate::status::{check, guard, guard_ptr, invalid, str_arg, write_out, FSSPEC_ABI_VERSION};

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
        let options = OptionsData {
            inner: options.clone(),
        };
        let mut out: *mut fsspec_fs = ptr::null_mut();
        let status = unsafe {
            create(
                self.factory.data,
                path.as_ptr() as *const c_char,
                path.len(),
                &options as *const OptionsData as *const fsspec_options,
                &mut out,
            )
        };
        check(status, "foreign factory create")?;
        if out.is_null() {
            return Err(FsError::Other(
                "foreign factory returned no filesystem".to_string(),
            ));
        }
        Ok(unsafe { FsData::from_raw(out) }.inner)
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

type PathCall =
    unsafe extern "C" fn(*mut c_void, *const c_char, usize, *mut fsspec_string) -> fsspec_status;

impl ForeignFileSystem {
    /// Calls a vtable member that maps a path to a string.
    fn path_call(&self, call: PathCall, path: &str, operation: &str) -> FsResult<String> {
        let mut out = fsspec_string::null();
        check(
            unsafe {
                call(
                    self.data,
                    path.as_ptr() as *const c_char,
                    path.len(),
                    &mut out,
                )
            },
            operation,
        )?;
        let text = unsafe { out.as_str().into_owned() };
        unsafe { out.free() };
        Ok(text)
    }

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

    fn strip_protocol(&self, path: &str) -> String {
        match self.vtable.strip_protocol {
            Some(strip) => self
                .path_call(strip, path, "strip_protocol")
                .unwrap_or_else(|_| {
                    strip_protocol_default(&self.protocols, &self.root_marker, path)
                }),
            None => strip_protocol_default(&self.protocols, &self.root_marker, path),
        }
    }

    fn unstrip_protocol(&self, path: &str) -> String {
        match self.vtable.unstrip_protocol {
            Some(unstrip) => self
                .path_call(unstrip, path, "unstrip_protocol")
                .unwrap_or_else(|_| unstrip_protocol_default(&self.protocols, path)),
            None => unstrip_protocol_default(&self.protocols, path),
        }
    }

    fn cat_file(&self, path: &str, start: Option<i64>, end: Option<i64>) -> FsResult<Vec<u8>> {
        let Some(cat_file) = self.vtable.cat_file else {
            return cat_file_default(self, path, start, end);
        };
        let mut out = fsspec_bytes {
            data: ptr::null_mut(),
            len: 0,
        };
        check(
            unsafe {
                cat_file(
                    self.data,
                    path.as_ptr() as *const c_char,
                    path.len(),
                    start
                        .as_ref()
                        .map_or(ptr::null(), |start| start as *const i64),
                    end.as_ref().map_or(ptr::null(), |end| end as *const i64),
                    &mut out,
                )
            },
            "cat_file",
        )?;
        let bytes = unsafe { out.as_slice().to_vec() };
        unsafe { out.free() };
        Ok(bytes)
    }

    fn pipe_file(&self, path: &str, data: &[u8]) -> FsResult<()> {
        let Some(pipe_file) = self.vtable.pipe_file else {
            return pipe_file_default(self, path, data);
        };
        check(
            unsafe {
                pipe_file(
                    self.data,
                    path.as_ptr() as *const c_char,
                    path.len(),
                    data.as_ptr(),
                    data.len(),
                )
            },
            "pipe_file",
        )
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
        let entries = unsafe { out.to_infos() };
        unsafe { out.free() };
        Ok(entries)
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
        Ok(unsafe { FileData::from_raw(out) }.inner)
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
        let out = unsafe { out.assume_init() };
        let entry = unsafe { out.to_info() };
        unsafe { out.free() };
        Ok(entry)
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
        .map_err(fsspec_sys::io_error)?;
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
        .map_err(fsspec_sys::io_error)?;
        Ok(count.min(buf.len()))
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.vtable.flush {
            Some(flush) => {
                check(unsafe { flush(self.data) }, "flush").map_err(fsspec_sys::io_error)
            }
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
        .map_err(fsspec_sys::io_error)?;
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
        let out = unsafe { out.assume_init() };
        let entry = unsafe { out.to_info() };
        unsafe { out.free() };
        Ok(entry)
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
        DEFAULT_FILESYSTEMS
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .register(protocol, ForeignFactory { factory });
        Ok(())
    })
}

/// Installs the factory used for protocols nothing registered, replacing an
/// earlier one; null clears it. The factory is given the whole URL. The
/// factory struct is copied.
#[no_mangle]
pub unsafe extern "C" fn fsspec_register_fallback(
    factory: *const fsspec_fs_factory,
) -> fsspec_status {
    guard(|| {
        let factory = if factory.is_null() {
            None
        } else {
            Some(ForeignFactory {
                factory: copy_versioned(factory, "factory")?,
            })
        };
        DEFAULT_FILESYSTEMS
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .set_fallback(factory);
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
        Ok(FsData::into_raw(Arc::new(fs)))
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
        Ok(FileData::into_raw(Box::new(ForeignFile { vtable, data })))
    })
}
