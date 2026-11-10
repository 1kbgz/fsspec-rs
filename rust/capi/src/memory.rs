use std::collections::HashMap;
use std::ffi::c_char;
use std::ptr;
use std::slice;
use std::time::{SystemTime, UNIX_EPOCH};

use fsspec_rs::{FileInfo, FileType};

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

/// The kind of a filesystem entry.
#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum fsspec_file_type {
    FSSPEC_FILE_TYPE_FILE = 0,
    FSSPEC_FILE_TYPE_DIRECTORY = 1,
    FSSPEC_FILE_TYPE_OTHER = 2,
}

pub use fsspec_file_type::*;

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

fn into_raw_slice<T>(values: Vec<T>) -> (*mut T, usize) {
    let boxed = values.into_boxed_slice();
    let len = boxed.len();
    (Box::into_raw(boxed) as *mut T, len)
}

unsafe fn from_raw_slice<T>(pointer: *mut T, len: usize) -> Vec<T> {
    if pointer.is_null() {
        return Vec::new();
    }
    Box::from_raw(ptr::slice_from_raw_parts_mut(pointer, len)).into_vec()
}

impl fsspec_bytes {
    pub(crate) fn from_vec(bytes: Vec<u8>) -> Self {
        let (data, len) = into_raw_slice(bytes);
        Self { data, len }
    }

    pub(crate) unsafe fn free(self) {
        drop(from_raw_slice(self.data, self.len));
    }
}

impl fsspec_string {
    pub(crate) fn from_string(string: String) -> Self {
        let mut bytes = string.into_bytes();
        bytes.push(0);
        let (data, len) = into_raw_slice(bytes);
        Self {
            data: data as *mut c_char,
            len: len - 1,
        }
    }

    pub(crate) fn null() -> Self {
        Self {
            data: ptr::null_mut(),
            len: 0,
        }
    }

    /// The string's text; a null string reads as empty.
    pub(crate) unsafe fn as_str(&self) -> std::borrow::Cow<'_, str> {
        if self.data.is_null() {
            return "".into();
        }
        String::from_utf8_lossy(slice::from_raw_parts(self.data as *const u8, self.len))
    }

    pub(crate) unsafe fn free(self) {
        if !self.data.is_null() {
            drop(from_raw_slice(self.data as *mut u8, self.len + 1));
        }
    }
}

impl fsspec_string_list {
    pub(crate) fn from_strings(strings: Vec<String>) -> Self {
        let (items, len) = into_raw_slice(
            strings
                .into_iter()
                .map(fsspec_string::from_string)
                .collect(),
        );
        Self { items, len }
    }

    pub(crate) unsafe fn to_strings(&self) -> Vec<String> {
        if self.items.is_null() {
            return Vec::new();
        }
        slice::from_raw_parts(self.items, self.len)
            .iter()
            .map(|item| item.as_str().into_owned())
            .collect()
    }

    pub(crate) unsafe fn free(self) {
        for item in from_raw_slice(self.items, self.len) {
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
        .map(|nanos| UNIX_EPOCH + std::time::Duration::from_nanos(nanos))
}

impl fsspec_file_info {
    pub(crate) fn from_info(info: FileInfo) -> Self {
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

    /// Converts and frees an entry produced by a foreign backend.
    pub(crate) unsafe fn into_info(self) -> FileInfo {
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
        let info = FileInfo {
            name: self.name.as_str().into_owned(),
            size: self.size,
            file_type,
            created: time_from_nanos(self.created_ns),
            modified: time_from_nanos(self.modified_ns),
            extra: pairs,
        };
        self.free();
        info
    }

    pub(crate) unsafe fn free(self) {
        self.name.free();
        self.extra.free();
    }
}

impl fsspec_file_info_list {
    pub(crate) fn from_infos(infos: Vec<FileInfo>) -> Self {
        let (items, len) =
            into_raw_slice(infos.into_iter().map(fsspec_file_info::from_info).collect());
        Self { items, len }
    }

    /// Converts and frees a list produced by a foreign backend.
    pub(crate) unsafe fn into_infos(self) -> Vec<FileInfo> {
        from_raw_slice(self.items, self.len)
            .into_iter()
            .map(|item| item.into_info())
            .collect()
    }

    pub(crate) unsafe fn free(self) {
        for item in from_raw_slice(self.items, self.len) {
            item.free();
        }
    }
}

/// Frees bytes returned by libfsspec.
#[no_mangle]
pub unsafe extern "C" fn fsspec_bytes_free(bytes: fsspec_bytes) {
    bytes.free();
}

/// Frees a string returned by libfsspec.
#[no_mangle]
pub unsafe extern "C" fn fsspec_string_free(string: fsspec_string) {
    string.free();
}

/// Frees a string list returned by libfsspec.
#[no_mangle]
pub unsafe extern "C" fn fsspec_string_list_free(list: fsspec_string_list) {
    list.free();
}

/// Frees one entry returned by libfsspec.
#[no_mangle]
pub unsafe extern "C" fn fsspec_file_info_free(info: fsspec_file_info) {
    info.free();
}

/// Frees an entry list returned by libfsspec.
#[no_mangle]
pub unsafe extern "C" fn fsspec_file_info_list_free(list: fsspec_file_info_list) {
    list.free();
}
