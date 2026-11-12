use std::ffi::{c_char, c_void, CStr};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::mem::{size_of, MaybeUninit};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

use fsspec_rs::{FileSystem, FsError, FsFile, LocalFs, OpenMode};

use crate::file::FileData;
use crate::options::OptionsData;
use crate::status::{guard, invalid, str_arg, write_out};
use crate::*;

fn arg(text: &str) -> (*const c_char, usize) {
    (text.as_ptr() as *const c_char, text.len())
}

fn last_error() -> String {
    let message = fsspec_last_error_message();
    assert!(!message.is_null(), "no error message was set");
    unsafe { CStr::from_ptr(message) }
        .to_string_lossy()
        .into_owned()
}

unsafe fn string_value(string: &fsspec_string) -> String {
    string.as_str().into_owned()
}

struct Handle(*mut fsspec_fs);

impl Handle {
    fn open(url: &str) -> (Self, String) {
        let (url_ptr, url_len) = arg(url);
        let mut fs = ptr::null_mut();
        let mut path = fsspec_string::null();
        let status = unsafe { fsspec_url_to_fs(url_ptr, url_len, ptr::null(), &mut fs, &mut path) };
        assert_eq!(status, FSSPEC_OK, "{}", last_error());
        let resolved = unsafe { string_value(&path) };
        unsafe { fsspec_string_free(path) };
        (Self(fs), resolved)
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { fsspec_fs_release(self.0) };
    }
}

fn pipe(fs: &Handle, path: &str, data: &[u8]) -> fsspec_status {
    let (ptr, len) = arg(path);
    unsafe { fsspec_fs_pipe_file(fs.0, ptr, len, data.as_ptr(), data.len()) }
}

fn cat(fs: &Handle, path: &str, start: Option<i64>, end: Option<i64>) -> Vec<u8> {
    let (ptr, len) = arg(path);
    let mut out = fsspec_bytes {
        data: ptr::null_mut(),
        len: 0,
    };
    let status = unsafe {
        fsspec_fs_cat_file(
            fs.0,
            ptr,
            len,
            start.as_ref().map_or(ptr::null(), |value| value),
            end.as_ref().map_or(ptr::null(), |value| value),
            &mut out,
        )
    };
    assert_eq!(status, FSSPEC_OK, "{}", last_error());
    let data = unsafe { std::slice::from_raw_parts(out.data, out.len) }.to_vec();
    unsafe { fsspec_bytes_free(out) };
    data
}

fn ls_names(fs: &Handle, path: &str) -> Vec<String> {
    let (ptr, len) = arg(path);
    let mut out = fsspec_file_info_list {
        items: ptr::null_mut(),
        len: 0,
    };
    assert_eq!(
        unsafe { fsspec_fs_ls(fs.0, ptr, len, true, &mut out) },
        FSSPEC_OK,
        "{}",
        last_error()
    );
    let entries = unsafe { std::slice::from_raw_parts(out.items, out.len) };
    let mut names: Vec<String> = entries
        .iter()
        .map(|entry| unsafe { string_value(&entry.name) })
        .collect();
    unsafe { fsspec_file_info_list_free(out) };
    names.sort();
    names
}

fn file_url(dir: &std::path::Path) -> String {
    format!("file://{}", dir.display())
}

#[test]
fn abi_version_matches_header_constant() {
    assert_eq!(fsspec_abi_version(), FSSPEC_ABI_VERSION);
    assert_eq!(FSSPEC_ABI_VERSION, fsspec_sys::FSSPEC_ABI_VERSION);
    assert_eq!(FSSPEC_OK as i32, 0);
    assert_eq!(FSSPEC_OTHER as i32, 9);
}

#[test]
fn options_round_trip() {
    let options = fsspec_options_new();
    let (key, key_len) = arg("auto_mkdir");
    let (value, value_len) = arg("true");
    assert_eq!(
        unsafe { fsspec_options_set(options, key, key_len, value, value_len) },
        FSSPEC_OK
    );
    assert_eq!(
        unsafe { &(*(options as *const OptionsData)).inner }["auto_mkdir"],
        "true"
    );
    assert_eq!(
        unsafe { fsspec_options_set(ptr::null_mut(), key, key_len, value, value_len) },
        FSSPEC_INVALID_ARGUMENT
    );
    assert!(last_error().contains("options is null"));
    unsafe { fsspec_options_free(options) };
    unsafe { fsspec_options_free(ptr::null_mut()) };
}

#[test]
fn local_filesystem_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let (fs, root) = Handle::open(&file_url(dir.path()));
    assert_eq!(root, dir.path().display().to_string());
    let sep = std::path::MAIN_SEPARATOR;
    let file = format!("{root}{sep}hello.txt");
    let copy = format!("{root}{sep}copy.txt");
    let sub = format!("{root}{sep}sub");

    assert_eq!(pipe(&fs, &file, b"hello"), FSSPEC_OK, "{}", last_error());
    assert_eq!(cat(&fs, &file, None, None), b"hello");
    assert_eq!(cat(&fs, &file, Some(1), Some(3)), b"el");
    assert_eq!(cat(&fs, &file, Some(-2), None), b"lo");

    let (ptr, len) = arg(&file);
    let mut exists = false;
    assert_eq!(
        unsafe { fsspec_fs_exists(fs.0, ptr, len, &mut exists) },
        FSSPEC_OK
    );
    assert!(exists);

    let mut info = MaybeUninit::<fsspec_file_info>::zeroed();
    assert_eq!(
        unsafe { fsspec_fs_info(fs.0, ptr, len, info.as_mut_ptr()) },
        FSSPEC_OK
    );
    let info = unsafe { info.assume_init() };
    assert_eq!(info.size, 5);
    assert_eq!(info.file_type, FSSPEC_FILE_TYPE_FILE);
    assert!(info.modified_ns > 0);
    unsafe { fsspec_file_info_free(info) };

    let (copy_ptr, copy_len) = arg(&copy);
    assert_eq!(
        unsafe { fsspec_fs_cp_file(fs.0, ptr, len, copy_ptr, copy_len) },
        FSSPEC_OK
    );
    let (sub_ptr, sub_len) = arg(&sub);
    assert_eq!(
        unsafe { fsspec_fs_mkdir(fs.0, sub_ptr, sub_len, true) },
        FSSPEC_OK
    );
    assert_eq!(
        ls_names(&fs, &root),
        vec![copy.clone(), file.clone(), sub.clone()]
    );
    assert_eq!(
        unsafe { fsspec_fs_rmdir(fs.0, sub_ptr, sub_len) },
        FSSPEC_OK
    );
    assert_eq!(
        unsafe { fsspec_fs_rm_file(fs.0, copy_ptr, copy_len) },
        FSSPEC_OK
    );
    assert_eq!(ls_names(&fs, &root), vec![file.clone()]);

    let mut protocols = fsspec_string_list {
        items: ptr::null_mut(),
        len: 0,
    };
    assert_eq!(
        unsafe { fsspec_fs_protocols(fs.0, &mut protocols) },
        FSSPEC_OK
    );
    assert_eq!(unsafe { protocols.to_strings() }, vec!["file", "local"]);
    unsafe { fsspec_string_list_free(protocols) };

    let retained = unsafe { fsspec_fs_retain(fs.0) };
    assert!(!retained.is_null());
    unsafe { fsspec_fs_release(retained) };
    assert!(unsafe { fsspec_fs_retain(ptr::null()) }.is_null());
}

#[test]
fn file_handle_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let (fs, root) = Handle::open(&file_url(dir.path()));
    let path = format!("{root}{}data.bin", std::path::MAIN_SEPARATOR);
    let (path_ptr, path_len) = arg(&path);

    let (mode, mode_len) = arg("wb");
    let mut file = ptr::null_mut();
    assert_eq!(
        unsafe { fsspec_fs_open(fs.0, path_ptr, path_len, mode, mode_len, &mut file) },
        FSSPEC_OK,
        "{}",
        last_error()
    );
    let mut written = 0;
    assert_eq!(
        unsafe { fsspec_file_write(file, b"abcdef".as_ptr(), 6, &mut written) },
        FSSPEC_OK
    );
    assert_eq!(written, 6);
    assert_eq!(unsafe { fsspec_file_flush(file) }, FSSPEC_OK);
    assert_eq!(unsafe { fsspec_file_close(file) }, FSSPEC_OK);

    let (mode, mode_len) = arg("rb");
    assert_eq!(
        unsafe { fsspec_fs_open(fs.0, path_ptr, path_len, mode, mode_len, &mut file) },
        FSSPEC_OK
    );
    let mut position = 0;
    assert_eq!(
        unsafe { fsspec_file_seek(file, 2, 0, &mut position) },
        FSSPEC_OK
    );
    assert_eq!(position, 2);
    assert_eq!(unsafe { fsspec_file_tell(file, &mut position) }, FSSPEC_OK);
    assert_eq!(position, 2);
    let mut buf = [0u8; 16];
    let mut read = 0;
    assert_eq!(
        unsafe { fsspec_file_read(file, buf.as_mut_ptr(), buf.len(), &mut read) },
        FSSPEC_OK
    );
    assert_eq!(&buf[..read], b"cdef");
    let mut size = 0;
    assert_eq!(unsafe { fsspec_file_size(file, &mut size) }, FSSPEC_OK);
    assert_eq!(size, 6);
    let mut info = MaybeUninit::<fsspec_file_info>::zeroed();
    assert_eq!(
        unsafe { fsspec_file_get_info(file, info.as_mut_ptr()) },
        FSSPEC_OK
    );
    unsafe { fsspec_file_info_free(info.assume_init()) };
    assert_eq!(
        unsafe { fsspec_file_seek(file, 0, 7, &mut position) },
        FSSPEC_INVALID_ARGUMENT
    );
    assert_eq!(unsafe { fsspec_file_commit(file) }, FSSPEC_OK);
    assert_eq!(unsafe { fsspec_file_discard(file) }, FSSPEC_OK);
    assert_eq!(unsafe { fsspec_file_close(file) }, FSSPEC_OK);
    assert_eq!(unsafe { fsspec_file_close(ptr::null_mut()) }, FSSPEC_OK);

    let (mode, mode_len) = arg("zz");
    assert_eq!(
        unsafe { fsspec_fs_open(fs.0, path_ptr, path_len, mode, mode_len, &mut file) },
        FSSPEC_INVALID_ARGUMENT
    );
    assert!(last_error().contains("unsupported open mode"));
}

#[test]
fn errors_carry_status_and_message() {
    let dir = tempfile::tempdir().unwrap();
    let (fs, root) = Handle::open(&file_url(dir.path()));
    let missing = format!("{root}{}missing.txt", std::path::MAIN_SEPARATOR);
    let (ptr_, len) = arg(&missing);
    let mut info = MaybeUninit::<fsspec_file_info>::zeroed();
    assert_eq!(
        unsafe { fsspec_fs_info(fs.0, ptr_, len, info.as_mut_ptr()) },
        FSSPEC_NOT_FOUND
    );
    assert!(last_error().contains("missing.txt"), "{}", last_error());

    assert_eq!(
        unsafe { fsspec_fs_info(ptr::null(), ptr_, len, info.as_mut_ptr()) },
        FSSPEC_INVALID_ARGUMENT
    );
    assert_eq!(
        unsafe { fsspec_fs_info(fs.0, ptr::null(), 3, info.as_mut_ptr()) },
        FSSPEC_INVALID_ARGUMENT
    );
    assert_eq!(
        unsafe { fsspec_fs_info(fs.0, ptr_, len, ptr::null_mut()) },
        FSSPEC_NOT_FOUND
    );

    let (url, url_len) = arg("nope://bucket/key");
    let mut out = ptr::null_mut();
    assert_eq!(
        unsafe { fsspec_url_to_fs(url, url_len, ptr::null(), &mut out, ptr::null_mut()) },
        FSSPEC_NOT_SUPPORTED
    );
    assert!(last_error().contains("nope"));

    fsspec_clear_last_error();
    assert!(fsspec_last_error_message().is_null());
    let (message, message_len) = arg("custom");
    unsafe { fsspec_set_last_error(message, message_len) };
    assert_eq!(last_error(), "custom");
}

// A vtable backend over LocalFs, standing in for a backend written in C.

static RELEASED: AtomicUsize = AtomicUsize::new(0);

unsafe fn local(data: *mut c_void) -> &'static LocalFs {
    &*(data as *const LocalFs)
}

unsafe extern "C" fn vt_ls(
    data: *mut c_void,
    path: *const c_char,
    len: usize,
    detail: bool,
    out: *mut fsspec_file_info_list,
) -> fsspec_status {
    guard(|| {
        let entries = local(data).ls(str_arg(path, len)?, detail)?;
        write_out(out, fsspec_file_info_list::from_infos(entries))
    })
}

unsafe extern "C" fn vt_info(
    data: *mut c_void,
    path: *const c_char,
    len: usize,
    out: *mut fsspec_file_info,
) -> fsspec_status {
    guard(|| {
        let info = local(data).info(str_arg(path, len)?)?;
        write_out(out, fsspec_file_info::from_info(info))
    })
}

unsafe extern "C" fn vt_rm_file(
    data: *mut c_void,
    path: *const c_char,
    len: usize,
) -> fsspec_status {
    guard(|| local(data).rm_file(str_arg(path, len)?))
}

unsafe extern "C" fn vt_cp_file(
    data: *mut c_void,
    src: *const c_char,
    src_len: usize,
    dst: *const c_char,
    dst_len: usize,
) -> fsspec_status {
    guard(|| local(data).cp_file(str_arg(src, src_len)?, str_arg(dst, dst_len)?))
}

unsafe extern "C" fn vt_mkdir(
    data: *mut c_void,
    path: *const c_char,
    len: usize,
    create_parents: bool,
) -> fsspec_status {
    guard(|| local(data).mkdir(str_arg(path, len)?, create_parents))
}

unsafe extern "C" fn vt_rmdir(data: *mut c_void, path: *const c_char, len: usize) -> fsspec_status {
    guard(|| local(data).rmdir(str_arg(path, len)?))
}

unsafe extern "C" fn vt_open(
    data: *mut c_void,
    path: *const c_char,
    len: usize,
    mode: *const c_char,
    mode_len: usize,
    out: *mut *mut fsspec_file,
) -> fsspec_status {
    guard(|| {
        let mode =
            OpenMode::from_str_mode(str_arg(mode, mode_len)?).ok_or_else(|| invalid("bad mode"))?;
        let file = local(data).open(str_arg(path, len)?, mode, None)?;
        write_out(out, FileData::into_raw(file))
    })
}

unsafe extern "C" fn vt_release(data: *mut c_void) {
    drop(Box::from_raw(data as *mut LocalFs));
    RELEASED.fetch_add(1, Ordering::SeqCst);
}

fn local_vtable() -> fsspec_fs_vtable {
    fsspec_fs_vtable {
        struct_size: size_of::<fsspec_fs_vtable>(),
        abi_version: FSSPEC_ABI_VERSION,
        protocol: c"vtest,vtest-alias".as_ptr(),
        root_marker: c"/".as_ptr(),
        sep: ptr::null(),
        ls: Some(vt_ls),
        info: Some(vt_info),
        rm_file: Some(vt_rm_file),
        cp_file: Some(vt_cp_file),
        mkdir: Some(vt_mkdir),
        rmdir: Some(vt_rmdir),
        open: Some(vt_open),
        release: Some(vt_release),
        strip_protocol: None,
        unstrip_protocol: None,
    }
}

unsafe extern "C" fn factory_create(
    _data: *mut c_void,
    _path: *const c_char,
    _path_len: usize,
    options: *const fsspec_options,
    out: *mut *mut fsspec_fs,
) -> fsspec_status {
    guard(|| {
        let auto_mkdir = (*(options as *const OptionsData))
            .inner
            .get("auto_mkdir")
            .is_some_and(|value| value == "true");
        let data = Box::into_raw(Box::new(LocalFs::with_auto_mkdir(auto_mkdir))) as *mut c_void;
        let fs = fsspec_fs_from_vtable(&local_vtable(), data);
        if fs.is_null() {
            return Err(FsError::Other(last_error()));
        }
        write_out(out, fs)
    })
}

unsafe extern "C" fn factory_scope(
    _data: *mut c_void,
    path: *const c_char,
    len: usize,
    out: *mut fsspec_string,
) -> fsspec_status {
    guard(|| {
        let first = str_arg(path, len)?
            .split('/')
            .next()
            .unwrap_or_default()
            .to_string();
        write_out(out, fsspec_string::from_string(first))
    })
}

#[test]
fn foreign_backend_registers_and_resolves_through_the_registry() {
    let factory = fsspec_fs_factory {
        struct_size: size_of::<fsspec_fs_factory>(),
        abi_version: FSSPEC_ABI_VERSION,
        data: ptr::null_mut(),
        create: Some(factory_create),
        instance_scope: Some(factory_scope),
        release: None,
    };
    let (protocol, protocol_len) = arg("vtest");
    assert_eq!(
        unsafe { fsspec_register_filesystem(protocol, protocol_len, &factory) },
        FSSPEC_OK,
        "{}",
        last_error()
    );

    let mut protocols = fsspec_string_list {
        items: ptr::null_mut(),
        len: 0,
    };
    assert_eq!(unsafe { fsspec_protocols(&mut protocols) }, FSSPEC_OK);
    assert!(unsafe { protocols.to_strings() }.contains(&"vtest".to_string()));
    unsafe { fsspec_string_list_free(protocols) };

    let dir = tempfile::tempdir().unwrap();
    let (fs, root) = Handle::open(&format!("vtest://{}", dir.path().display()));
    assert_eq!(root, dir.path().display().to_string());
    let file = format!("{root}{}via-vtable.txt", std::path::MAIN_SEPARATOR);
    assert_eq!(
        pipe(&fs, &file, b"through the vtable"),
        FSSPEC_OK,
        "{}",
        last_error()
    );
    assert_eq!(cat(&fs, &file, Some(12), None), b"vtable");
    assert_eq!(ls_names(&fs, &root), vec![file.clone()]);
    assert!(dir.path().join("via-vtable.txt").is_file());

    let mut protocols = fsspec_string_list {
        items: ptr::null_mut(),
        len: 0,
    };
    assert_eq!(
        unsafe { fsspec_fs_protocols(fs.0, &mut protocols) },
        FSSPEC_OK
    );
    assert_eq!(
        unsafe { protocols.to_strings() },
        vec!["vtest", "vtest-alias"]
    );
    unsafe { fsspec_string_list_free(protocols) };

    // Replacing the factory drops the cached instance, which releases the vtable data.
    let before = RELEASED.load(Ordering::SeqCst);
    drop(fs);
    assert_eq!(
        unsafe { fsspec_register_filesystem(protocol, protocol_len, &factory) },
        FSSPEC_OK
    );
    assert_eq!(RELEASED.load(Ordering::SeqCst), before + 1);
}

#[test]
fn versioned_structs_are_checked() {
    let mut factory = fsspec_fs_factory {
        struct_size: size_of::<fsspec_fs_factory>(),
        abi_version: FSSPEC_ABI_VERSION + 1,
        data: ptr::null_mut(),
        create: Some(factory_create),
        instance_scope: None,
        release: None,
    };
    let (protocol, protocol_len) = arg("vbad");
    assert_eq!(
        unsafe { fsspec_register_filesystem(protocol, protocol_len, &factory) },
        FSSPEC_INVALID_ARGUMENT
    );
    assert!(last_error().contains("ABI version"), "{}", last_error());
    factory.abi_version = FSSPEC_ABI_VERSION;
    factory.struct_size = 4;
    assert_eq!(
        unsafe { fsspec_register_filesystem(protocol, protocol_len, &factory) },
        FSSPEC_INVALID_ARGUMENT
    );
    assert!(last_error().contains("struct_size"));
    assert_eq!(
        unsafe { fsspec_register_filesystem(protocol, protocol_len, ptr::null()) },
        FSSPEC_INVALID_ARGUMENT
    );
    assert_eq!(
        unsafe { fsspec_register_filesystem(ptr::null(), 0, &factory) },
        FSSPEC_INVALID_ARGUMENT
    );
    assert!(unsafe { fsspec_fs_from_vtable(ptr::null(), ptr::null_mut()) }.is_null());

    // An older, shorter factory struct still registers.
    #[repr(C)]
    struct OldFactory {
        struct_size: usize,
        abi_version: u32,
        data: *mut c_void,
        create: Option<
            unsafe extern "C" fn(
                *mut c_void,
                *const c_char,
                usize,
                *const fsspec_options,
                *mut *mut fsspec_fs,
            ) -> fsspec_status,
        >,
    }
    let old = OldFactory {
        struct_size: size_of::<OldFactory>(),
        abi_version: FSSPEC_ABI_VERSION,
        data: ptr::null_mut(),
        create: Some(factory_create),
    };
    let (protocol, protocol_len) = arg("vold");
    assert_eq!(
        unsafe {
            fsspec_register_filesystem(
                protocol,
                protocol_len,
                &old as *const OldFactory as *const fsspec_fs_factory,
            )
        },
        FSSPEC_OK
    );
}

// A foreign in-memory file, standing in for a file object written in C.

struct MemFile {
    cursor: Cursor<Vec<u8>>,
    released: *mut bool,
}

unsafe fn mem(data: *mut c_void) -> &'static mut MemFile {
    &mut *(data as *mut MemFile)
}

unsafe extern "C" fn mf_read(
    data: *mut c_void,
    buf: *mut u8,
    len: usize,
    out: *mut usize,
) -> fsspec_status {
    guard(|| {
        let read = mem(data)
            .cursor
            .read(std::slice::from_raw_parts_mut(buf, len))?;
        write_out(out, read)
    })
}

unsafe extern "C" fn mf_write(
    data: *mut c_void,
    buf: *const u8,
    len: usize,
    out: *mut usize,
) -> fsspec_status {
    guard(|| {
        let written = mem(data)
            .cursor
            .write(std::slice::from_raw_parts(buf, len))?;
        write_out(out, written)
    })
}

unsafe extern "C" fn mf_seek(
    data: *mut c_void,
    offset: i64,
    whence: i32,
    out: *mut u64,
) -> fsspec_status {
    guard(|| {
        let target = match whence {
            0 => SeekFrom::Start(offset as u64),
            1 => SeekFrom::Current(offset),
            _ => SeekFrom::End(offset),
        };
        let position = mem(data).cursor.seek(target)?;
        write_out(out, position)
    })
}

unsafe extern "C" fn mf_size(data: *mut c_void, out: *mut i64) -> fsspec_status {
    guard(|| write_out(out, mem(data).cursor.get_ref().len() as i64))
}

unsafe extern "C" fn mf_release(data: *mut c_void) {
    let file = Box::from_raw(data as *mut MemFile);
    *file.released = true;
}

#[test]
fn foreign_file_round_trip() {
    let mut released = false;
    let data = Box::into_raw(Box::new(MemFile {
        cursor: Cursor::new(Vec::new()),
        released: &mut released,
    })) as *mut c_void;
    let vtable = fsspec_file_vtable {
        struct_size: size_of::<fsspec_file_vtable>(),
        abi_version: FSSPEC_ABI_VERSION,
        read: Some(mf_read),
        write: Some(mf_write),
        seek: Some(mf_seek),
        flush: None,
        size: Some(mf_size),
        info: None,
        commit: None,
        discard: None,
        release: Some(mf_release),
    };
    let file = unsafe { fsspec_file_from_vtable(&vtable, data) };
    assert!(!file.is_null(), "{}", last_error());

    let mut written = 0;
    assert_eq!(
        unsafe { fsspec_file_write(file, b"memory".as_ptr(), 6, &mut written) },
        FSSPEC_OK
    );
    assert_eq!(written, 6);
    let mut position = 0;
    assert_eq!(
        unsafe { fsspec_file_seek(file, -3, 2, &mut position) },
        FSSPEC_OK
    );
    assert_eq!(position, 3);
    let mut buf = [0u8; 8];
    let mut read = 0;
    assert_eq!(
        unsafe { fsspec_file_read(file, buf.as_mut_ptr(), buf.len(), &mut read) },
        FSSPEC_OK
    );
    assert_eq!(&buf[..read], b"ory");
    let mut size = 0;
    assert_eq!(unsafe { fsspec_file_size(file, &mut size) }, FSSPEC_OK);
    assert_eq!(size, 6);
    let mut info = MaybeUninit::<fsspec_file_info>::zeroed();
    assert_eq!(
        unsafe { fsspec_file_get_info(file, info.as_mut_ptr()) },
        FSSPEC_NOT_SUPPORTED
    );
    assert_eq!(unsafe { fsspec_file_commit(file) }, FSSPEC_OK);
    assert_eq!(unsafe { fsspec_file_close(file) }, FSSPEC_OK);
    assert!(released);

    // The Rust side sees a foreign file as an FsFile.
    let mut released = false;
    let data = Box::into_raw(Box::new(MemFile {
        cursor: Cursor::new(b"rust".to_vec()),
        released: &mut released,
    })) as *mut c_void;
    let file = unsafe { FileData::from_raw(fsspec_file_from_vtable(&vtable, data)) };
    let mut inner: Box<dyn FsFile> = file.inner;
    let mut text = String::new();
    inner.read_to_string(&mut text).unwrap();
    assert_eq!(text, "rust");
    assert_eq!(inner.size().unwrap(), Some(4));
    assert!(inner.info().is_err());
    drop(inner);
    assert!(released);
}
