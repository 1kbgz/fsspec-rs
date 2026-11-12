//! Declarations of every `libfsspec` entry point. No `#[link]` attribute:
//! the build links the library (hatch-rs for a bundled or external copy) or
//! compiles `fsspec_capi` in (the static route and this crate's tests).

use std::ffi::{c_char, c_void};

use crate::types::*;

#[allow(improper_ctypes)]
extern "C" {
    pub fn fsspec_abi_version() -> u32;
    pub fn fsspec_last_error_message() -> *const c_char;
    pub fn fsspec_set_last_error(message: *const c_char, len: usize);
    pub fn fsspec_clear_last_error();

    pub fn fsspec_bytes_free(bytes: fsspec_bytes);
    pub fn fsspec_string_free(string: fsspec_string);
    pub fn fsspec_string_list_free(list: fsspec_string_list);
    pub fn fsspec_file_info_free(info: fsspec_file_info);
    pub fn fsspec_file_info_list_free(list: fsspec_file_info_list);

    pub fn fsspec_options_new() -> *mut fsspec_options;
    pub fn fsspec_options_set(
        options: *mut fsspec_options,
        key: *const c_char,
        key_len: usize,
        value: *const c_char,
        value_len: usize,
    ) -> fsspec_status;
    pub fn fsspec_options_items(
        options: *const fsspec_options,
        out: *mut fsspec_string_list,
    ) -> fsspec_status;
    pub fn fsspec_options_free(options: *mut fsspec_options);

    pub fn fsspec_url_to_fs(
        url: *const c_char,
        url_len: usize,
        options: *const fsspec_options,
        out_fs: *mut *mut fsspec_fs,
        out_path: *mut fsspec_string,
    ) -> fsspec_status;
    pub fn fsspec_fs_retain(fs: *const fsspec_fs) -> *mut fsspec_fs;
    pub fn fsspec_fs_release(fs: *mut fsspec_fs);
    pub fn fsspec_fs_protocols(fs: *const fsspec_fs, out: *mut fsspec_string_list)
        -> fsspec_status;
    pub fn fsspec_fs_root_marker(fs: *const fsspec_fs, out: *mut fsspec_string) -> fsspec_status;
    pub fn fsspec_fs_strip_protocol(
        fs: *const fsspec_fs,
        path: *const c_char,
        path_len: usize,
        out: *mut fsspec_string,
    ) -> fsspec_status;
    pub fn fsspec_fs_unstrip_protocol(
        fs: *const fsspec_fs,
        path: *const c_char,
        path_len: usize,
        out: *mut fsspec_string,
    ) -> fsspec_status;
    pub fn fsspec_fs_sep(fs: *const fsspec_fs, out: *mut fsspec_string) -> fsspec_status;
    pub fn fsspec_fs_ls(
        fs: *const fsspec_fs,
        path: *const c_char,
        path_len: usize,
        detail: bool,
        out: *mut fsspec_file_info_list,
    ) -> fsspec_status;
    pub fn fsspec_fs_info(
        fs: *const fsspec_fs,
        path: *const c_char,
        path_len: usize,
        out: *mut fsspec_file_info,
    ) -> fsspec_status;
    pub fn fsspec_fs_exists(
        fs: *const fsspec_fs,
        path: *const c_char,
        path_len: usize,
        out: *mut bool,
    ) -> fsspec_status;
    pub fn fsspec_fs_cat_file(
        fs: *const fsspec_fs,
        path: *const c_char,
        path_len: usize,
        start: *const i64,
        end: *const i64,
        out: *mut fsspec_bytes,
    ) -> fsspec_status;
    pub fn fsspec_fs_pipe_file(
        fs: *const fsspec_fs,
        path: *const c_char,
        path_len: usize,
        data: *const u8,
        data_len: usize,
    ) -> fsspec_status;
    pub fn fsspec_fs_rm_file(
        fs: *const fsspec_fs,
        path: *const c_char,
        path_len: usize,
    ) -> fsspec_status;
    pub fn fsspec_fs_cp_file(
        fs: *const fsspec_fs,
        src: *const c_char,
        src_len: usize,
        dst: *const c_char,
        dst_len: usize,
    ) -> fsspec_status;
    pub fn fsspec_fs_mkdir(
        fs: *const fsspec_fs,
        path: *const c_char,
        path_len: usize,
        create_parents: bool,
    ) -> fsspec_status;
    pub fn fsspec_fs_rmdir(
        fs: *const fsspec_fs,
        path: *const c_char,
        path_len: usize,
    ) -> fsspec_status;
    pub fn fsspec_fs_open(
        fs: *const fsspec_fs,
        path: *const c_char,
        path_len: usize,
        mode: *const c_char,
        mode_len: usize,
        out: *mut *mut fsspec_file,
    ) -> fsspec_status;

    pub fn fsspec_fs_open_ex(
        fs: *const fsspec_fs,
        path: *const c_char,
        path_len: usize,
        mode: *const c_char,
        mode_len: usize,
        options: *const fsspec_options,
        out: *mut *mut fsspec_file,
    ) -> fsspec_status;

    pub fn fsspec_file_read(
        file: *mut fsspec_file,
        buf: *mut u8,
        len: usize,
        out_read: *mut usize,
    ) -> fsspec_status;
    pub fn fsspec_file_write(
        file: *mut fsspec_file,
        data: *const u8,
        len: usize,
        out_written: *mut usize,
    ) -> fsspec_status;
    pub fn fsspec_file_seek(
        file: *mut fsspec_file,
        offset: i64,
        whence: i32,
        out_position: *mut u64,
    ) -> fsspec_status;
    pub fn fsspec_file_tell(file: *mut fsspec_file, out_position: *mut u64) -> fsspec_status;
    pub fn fsspec_file_flush(file: *mut fsspec_file) -> fsspec_status;
    pub fn fsspec_file_size(file: *mut fsspec_file, out_size: *mut i64) -> fsspec_status;
    pub fn fsspec_file_get_info(
        file: *mut fsspec_file,
        out: *mut fsspec_file_info,
    ) -> fsspec_status;
    pub fn fsspec_file_commit(file: *mut fsspec_file) -> fsspec_status;
    pub fn fsspec_file_discard(file: *mut fsspec_file) -> fsspec_status;
    pub fn fsspec_file_close(file: *mut fsspec_file) -> fsspec_status;

    pub fn fsspec_register_filesystem(
        protocol: *const c_char,
        protocol_len: usize,
        factory: *const fsspec_fs_factory,
    ) -> fsspec_status;
    pub fn fsspec_protocols(out: *mut fsspec_string_list) -> fsspec_status;
    pub fn fsspec_fs_from_vtable(
        vtable: *const fsspec_fs_vtable,
        data: *mut c_void,
    ) -> *mut fsspec_fs;
    pub fn fsspec_file_from_vtable(
        vtable: *const fsspec_file_vtable,
        data: *mut c_void,
    ) -> *mut fsspec_file;
}
