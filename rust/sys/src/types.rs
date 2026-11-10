//! The `#[repr(C)]` types of `fsspec.h`.
//!
//! Every buffer that crosses the ABI is allocated and freed with the platform
//! allocator ([`std::alloc::System`]), never a crate's global allocator, so
//! `libfsspec` and a backend living in another shared library agree on how to
//! free each other's memory. C code uses `malloc` on Unix and the process heap
//! on Windows, which is what `System` wraps.

use std::alloc::{GlobalAlloc, Layout, System};
use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::{c_char, c_void};
use std::ptr;
use std::slice;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fsspec_rs::{FileInfo, FileType};

/// The ABI version of this header and library. A caller built against a
/// different version must not use the library.
pub const FSSPEC_ABI_VERSION: u32 = 1;

/// Status returned by every entry point. Non-zero values mirror `FsError`.
#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum fsspec_status {
    FSSPEC_OK = 0,
    FSSPEC_NOT_FOUND = 1,
    FSSPEC_PERMISSION_DENIED = 2,
    FSSPEC_ALREADY_EXISTS = 3,
    FSSPEC_NOT_A_DIRECTORY = 4,
    FSSPEC_IS_A_DIRECTORY = 5,
    FSSPEC_IO = 6,
    FSSPEC_INVALID_ARGUMENT = 7,
    FSSPEC_NOT_SUPPORTED = 8,
    FSSPEC_OTHER = 9,
}

pub use fsspec_status::*;

/// The kind of a filesystem entry.
#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum fsspec_file_type {
    FSSPEC_FILE_TYPE_FILE = 0,
    FSSPEC_FILE_TYPE_DIRECTORY = 1,
    FSSPEC_FILE_TYPE_OTHER = 2,
}

pub use fsspec_file_type::*;

// Opaque handles. Without `#[repr(C)]`, cbindgen emits a forward declaration,
// and they only ever appear behind pointers.

/// Storage options for a filesystem: an opaque string map.
pub struct fsspec_options {
    _private: [u8; 0],
}

/// A filesystem resolved through the registry. Handles are reference counted:
/// `fsspec_fs_retain` returns another handle to the same filesystem and
/// `fsspec_fs_release` frees one.
pub struct fsspec_fs {
    _private: [u8; 0],
}

/// An open file. Close with `fsspec_file_close`.
pub struct fsspec_file {
    _private: [u8; 0],
}

/// Bytes allocated by libfsspec. Free with `fsspec_bytes_free`.
#[repr(C)]
pub struct fsspec_bytes {
    pub data: *mut u8,
    pub len: usize,
}

/// A UTF-8 string allocated by libfsspec. `data` is NUL-terminated and `len`
/// excludes the terminator. Free with `fsspec_string_free`.
#[repr(C)]
pub struct fsspec_string {
    pub data: *mut c_char,
    pub len: usize,
}

/// Strings allocated by libfsspec. Free with `fsspec_string_list_free`.
#[repr(C)]
pub struct fsspec_string_list {
    pub items: *mut fsspec_string,
    pub len: usize,
}

/// Metadata for one entry. Times are nanoseconds since the Unix epoch, or -1
/// when unknown. `extra` holds alternating key and value strings. Free with
/// `fsspec_file_info_free`.
#[repr(C)]
pub struct fsspec_file_info {
    pub name: fsspec_string,
    pub size: u64,
    pub file_type: fsspec_file_type,
    pub created_ns: i64,
    pub modified_ns: i64,
    pub extra: fsspec_string_list,
}

/// Entries allocated by libfsspec. Free with `fsspec_file_info_list_free`.
#[repr(C)]
pub struct fsspec_file_info_list {
    pub items: *mut fsspec_file_info,
    pub len: usize,
}

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
/// from them. Output values are allocated with the platform allocator and
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

/// Copies `values` into a platform-allocated array.
fn alloc_array<T>(values: Vec<T>) -> (*mut T, usize) {
    let len = values.len();
    if len == 0 {
        return (ptr::null_mut(), 0);
    }
    let layout = Layout::array::<T>(len).expect("array layout");
    let data = unsafe { System.alloc(layout) } as *mut T;
    assert!(!data.is_null(), "platform allocation failed");
    for (index, value) in values.into_iter().enumerate() {
        unsafe { data.add(index).write(value) };
    }
    (data, len)
}

/// Takes ownership of a platform-allocated array, moving the items out.
unsafe fn take_array<T>(data: *mut T, len: usize) -> Vec<T> {
    if data.is_null() || len == 0 {
        return Vec::new();
    }
    let values = (0..len).map(|index| data.add(index).read()).collect();
    System.dealloc(
        data as *mut u8,
        Layout::array::<T>(len).expect("array layout"),
    );
    values
}

impl fsspec_bytes {
    pub fn from_vec(bytes: Vec<u8>) -> Self {
        let (data, len) = alloc_array(bytes);
        Self { data, len }
    }

    /// The bytes; a null buffer reads as empty.
    pub unsafe fn as_slice(&self) -> &[u8] {
        if self.data.is_null() {
            &[]
        } else {
            slice::from_raw_parts(self.data, self.len)
        }
    }

    /// Frees a platform-allocated buffer.
    pub unsafe fn free(self) {
        drop(take_array(self.data, self.len));
    }
}

impl fsspec_string {
    pub fn from_string(string: String) -> Self {
        let mut bytes = string.into_bytes();
        bytes.push(0);
        let (data, len) = alloc_array(bytes);
        Self {
            data: data as *mut c_char,
            len: len - 1,
        }
    }

    pub fn null() -> Self {
        Self {
            data: ptr::null_mut(),
            len: 0,
        }
    }

    /// The string's text; a null string reads as empty.
    pub unsafe fn as_str(&self) -> Cow<'_, str> {
        if self.data.is_null() {
            return "".into();
        }
        String::from_utf8_lossy(slice::from_raw_parts(self.data as *const u8, self.len))
    }

    /// Frees a platform-allocated string.
    pub unsafe fn free(self) {
        if !self.data.is_null() {
            drop(take_array(self.data as *mut u8, self.len + 1));
        }
    }
}

impl fsspec_string_list {
    pub fn from_strings(strings: Vec<String>) -> Self {
        let (items, len) = alloc_array(
            strings
                .into_iter()
                .map(fsspec_string::from_string)
                .collect(),
        );
        Self { items, len }
    }

    pub unsafe fn as_slice(&self) -> &[fsspec_string] {
        if self.items.is_null() {
            &[]
        } else {
            slice::from_raw_parts(self.items, self.len)
        }
    }

    pub unsafe fn to_strings(&self) -> Vec<String> {
        self.as_slice()
            .iter()
            .map(|item| item.as_str().into_owned())
            .collect()
    }

    /// Frees a platform-allocated list and its strings.
    pub unsafe fn free(self) {
        for item in take_array(self.items, self.len) {
            item.free();
        }
    }
}

fn nanos_since_epoch(time: Option<SystemTime>) -> i64 {
    time.and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_nanos()).ok())
        .unwrap_or(-1)
}

fn time_from_nanos(nanos: i64) -> Option<SystemTime> {
    u64::try_from(nanos)
        .ok()
        .map(|nanos| UNIX_EPOCH + Duration::from_nanos(nanos))
}

impl fsspec_file_info {
    pub fn from_info(info: FileInfo) -> Self {
        let file_type = match info.file_type {
            FileType::File => FSSPEC_FILE_TYPE_FILE,
            FileType::Directory => FSSPEC_FILE_TYPE_DIRECTORY,
            FileType::Other => FSSPEC_FILE_TYPE_OTHER,
        };
        let mut extra: Vec<(String, String)> = info.extra.into_iter().collect();
        extra.sort();
        Self {
            name: fsspec_string::from_string(info.name),
            size: info.size,
            file_type,
            created_ns: nanos_since_epoch(info.created),
            modified_ns: nanos_since_epoch(info.modified),
            extra: fsspec_string_list::from_strings(
                extra
                    .into_iter()
                    .flat_map(|(key, value)| [key, value])
                    .collect(),
            ),
        }
    }

    /// Reads the entry without freeing it.
    pub unsafe fn to_info(&self) -> FileInfo {
        let file_type = match self.file_type {
            FSSPEC_FILE_TYPE_FILE => FileType::File,
            FSSPEC_FILE_TYPE_DIRECTORY => FileType::Directory,
            FSSPEC_FILE_TYPE_OTHER => FileType::Other,
        };
        let extra = self.extra.to_strings();
        let mut pairs = HashMap::new();
        for pair in extra.chunks(2) {
            pairs.insert(pair[0].clone(), pair.get(1).cloned().unwrap_or_default());
        }
        FileInfo {
            name: self.name.as_str().into_owned(),
            size: self.size,
            file_type,
            created: time_from_nanos(self.created_ns),
            modified: time_from_nanos(self.modified_ns),
            extra: pairs,
        }
    }

    /// Frees a platform-allocated entry.
    pub unsafe fn free(self) {
        self.name.free();
        self.extra.free();
    }
}

impl fsspec_file_info_list {
    pub fn from_infos(infos: Vec<FileInfo>) -> Self {
        let (items, len) =
            alloc_array(infos.into_iter().map(fsspec_file_info::from_info).collect());
        Self { items, len }
    }

    pub unsafe fn as_slice(&self) -> &[fsspec_file_info] {
        if self.items.is_null() {
            &[]
        } else {
            slice::from_raw_parts(self.items, self.len)
        }
    }

    pub unsafe fn to_infos(&self) -> Vec<FileInfo> {
        self.as_slice().iter().map(|item| item.to_info()).collect()
    }

    /// Frees a platform-allocated list and its entries.
    pub unsafe fn free(self) {
        for item in take_array(self.items, self.len) {
            item.free();
        }
    }
}
