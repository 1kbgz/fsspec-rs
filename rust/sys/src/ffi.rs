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
    pub fn fsspec_s3_open(
        bucket: *const c_char,
        bucket_len: usize,
        options: *const fsspec_options,
        out_fs: *mut *mut fsspec_fs,
        out_objects: *mut *mut fsspec_object_store,
    ) -> fsspec_status;
    pub fn fsspec_object_store_retain(
        store: *const fsspec_object_store,
    ) -> *mut fsspec_object_store;
    pub fn fsspec_object_store_release(store: *mut fsspec_object_store);
    pub fn fsspec_object_store_head(
        store: *const fsspec_object_store,
        key: *const c_char,
        key_len: usize,
        out: *mut fsspec_file_info,
    ) -> fsspec_operation_status;
    pub fn fsspec_object_store_read_ranges(
        store: *const fsspec_object_store,
        key: *const c_char,
        key_len: usize,
        ranges: *const fsspec_byte_range,
        ranges_len: usize,
        out: *mut fsspec_range_read_result,
    ) -> fsspec_operation_status;
    pub fn fsspec_fs_retain(fs: *const fsspec_fs) -> *mut fsspec_fs;
    pub fn fsspec_fs_release(fs: *mut fsspec_fs);
    pub fn fsspec_fs_protocols(fs: *const fsspec_fs, out: *mut fsspec_string_list)
        -> fsspec_status;
    pub fn fsspec_register_fallback(factory: *const fsspec_fs_factory) -> fsspec_status;
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
    pub fn fsspec_fs_read_ranges(
        fs: *const fsspec_fs,
        path: *const c_char,
        path_len: usize,
        ranges: *const fsspec_byte_range,
        ranges_len: usize,
        options: *const fsspec_range_read_options,
        out: *mut fsspec_range_read_result,
    ) -> fsspec_operation_status;
    pub fn fsspec_range_read_result_free(result: fsspec_range_read_result);
    pub fn fsspec_fs_read(
        fs: *const fsspec_fs,
        path: *const c_char,
        path_len: usize,
        options: *const fsspec_read_options,
        out: *mut fsspec_read_result,
    ) -> fsspec_operation_status;
    pub fn fsspec_fs_write(
        fs: *const fsspec_fs,
        path: *const c_char,
        path_len: usize,
        bytes: *const u8,
        bytes_len: usize,
        options: *const fsspec_write_options,
        out: *mut fsspec_write_result,
    ) -> fsspec_operation_status;
    pub fn fsspec_fs_io_capabilities(
        fs: *const fsspec_fs,
        out: *mut fsspec_io_capabilities,
    ) -> fsspec_status;
    pub fn fsspec_read_result_free(result: fsspec_read_result);
    pub fn fsspec_write_result_free(result: fsspec_write_result);
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

#[allow(improper_ctypes)]
extern "C" {
    pub fn fsspec_async_error_new(
        status: i32,
        data: *const c_char,
        len: usize,
    ) -> *mut fsspec_async_error;
    pub fn fsspec_async_error_free(error: *mut fsspec_async_error);
    pub fn fsspec_dispatcher_from_vtable(
        vtable: *const fsspec_dispatcher_vtable,
        data: *mut c_void,
        out: *mut *mut fsspec_dispatcher,
        out_error: *mut *mut fsspec_async_error,
    ) -> i32;
    pub fn fsspec_dispatcher_retain(handle: *const fsspec_dispatcher) -> *mut fsspec_dispatcher;
    pub fn fsspec_dispatcher_release(handle: *mut fsspec_dispatcher);
    pub fn fsspec_dispatch_job_run(job: *mut fsspec_dispatch_job);
    pub fn fsspec_dispatch_job_discard(job: *mut fsspec_dispatch_job);
    pub fn fsspec_operation_create(
        kind: u32,
        max_bytes: usize,
        timeout_ns: u64,
        out: *mut *mut fsspec_operation,
        producer: *mut *mut fsspec_completion,
        out_error: *mut *mut fsspec_async_error,
    ) -> i32;
    pub fn fsspec_operation_retain(operation: *const fsspec_operation) -> *mut fsspec_operation;
    pub fn fsspec_operation_release(operation: *mut fsspec_operation);
    pub fn fsspec_operation_state(operation: *const fsspec_operation) -> u32;
    pub fn fsspec_operation_is_quiescent(operation: *const fsspec_operation) -> bool;
    pub fn fsspec_operation_cancel(operation: *mut fsspec_operation);
    pub fn fsspec_operation_take(
        operation: *mut fsspec_operation,
        out: *mut *mut fsspec_async_result,
        out_error: *mut *mut fsspec_async_error,
    ) -> i32;
    pub fn fsspec_async_result_free(result: *mut fsspec_async_result);
    pub fn fsspec_async_result_error(
        result: *const fsspec_async_result,
    ) -> *const fsspec_async_error;
    pub fn fsspec_async_result_kind(result: *const fsspec_async_result) -> u32;
    pub fn fsspec_async_result_bytes(
        result: *const fsspec_async_result,
        out_len: *mut usize,
    ) -> *const u8;
    pub fn fsspec_operation_subscribe(
        operation: *mut fsspec_operation,
        dispatch: *const fsspec_dispatcher,
        ready: Option<unsafe extern "C" fn(*mut c_void, *const fsspec_operation)>,
        data: *mut c_void,
        release: Option<unsafe extern "C" fn(*mut c_void)>,
        out_error: *mut *mut fsspec_async_error,
    ) -> i32;
    pub fn fsspec_completion_retain(completion: *const fsspec_completion)
        -> *mut fsspec_completion;
    pub fn fsspec_completion_release(completion: *mut fsspec_completion);
    pub fn fsspec_completion_is_cancelled(completion: *const fsspec_completion) -> bool;
    pub fn fsspec_completion_remaining_ns(completion: *const fsspec_completion) -> u64;
    pub fn fsspec_completion_on_cancel(
        completion: *mut fsspec_completion,
        dispatch: *const fsspec_dispatcher,
        cancel: Option<unsafe extern "C" fn(*mut c_void)>,
        data: *mut c_void,
        release: Option<unsafe extern "C" fn(*mut c_void)>,
        out_error: *mut *mut fsspec_async_error,
    ) -> i32;
    pub fn fsspec_completion_bytes(completion: *mut fsspec_completion, data: *const u8, len: usize);
    pub fn fsspec_completion_unit(completion: *mut fsspec_completion);
    pub fn fsspec_completion_error(
        completion: *mut fsspec_completion,
        status: i32,
        data: *const c_char,
        len: usize,
    );
}

#[allow(improper_ctypes)]
extern "C" {
    pub fn fsspec_async_s3_open(
        bucket: *const c_char,
        bucket_len: usize,
        options: *const fsspec_options,
        context: *const fsspec_async_context,
        out: *mut *mut fsspec_async_fs,
        out_error: *mut *mut fsspec_async_error,
    ) -> i32;
    pub fn fsspec_async_context_create(
        options: *const fsspec_async_context_options,
        out: *mut *mut fsspec_async_context,
        out_error: *mut *mut fsspec_async_error,
    ) -> i32;
    pub fn fsspec_async_file_from_vtable(
        context: *const fsspec_async_context,
        dispatch: *const fsspec_dispatcher,
        vtable: *const fsspec_async_file_vtable,
        data: *mut c_void,
        out: *mut *mut fsspec_async_file,
        out_error: *mut *mut fsspec_async_error,
    ) -> i32;
    pub fn fsspec_async_fs_from_vtable(
        context: *const fsspec_async_context,
        dispatch: *const fsspec_dispatcher,
        vtable: *const fsspec_async_fs_vtable,
        data: *mut c_void,
        out: *mut *mut fsspec_async_fs,
        out_error: *mut *mut fsspec_async_error,
    ) -> i32;
    pub fn fsspec_completion_info(
        completion: *mut fsspec_completion,
        info: *const fsspec_file_info,
    );
    pub fn fsspec_completion_file(completion: *mut fsspec_completion, file: *mut fsspec_async_file);
    pub fn fsspec_completion_position(completion: *mut fsspec_completion, position: u64);
    pub fn fsspec_async_result_take_info(
        result: *mut fsspec_async_result,
        out: *mut fsspec_file_info,
        out_error: *mut *mut fsspec_async_error,
    ) -> i32;
    pub fn fsspec_async_result_take_file(
        result: *mut fsspec_async_result,
        out: *mut *mut fsspec_async_file,
        out_error: *mut *mut fsspec_async_error,
    ) -> i32;
    pub fn fsspec_async_result_position(
        result: *const fsspec_async_result,
        out: *mut u64,
        out_error: *mut *mut fsspec_async_error,
    ) -> i32;
    pub fn fsspec_async_context_retain(
        value: *const fsspec_async_context,
    ) -> *mut fsspec_async_context;
    pub fn fsspec_async_context_release(value: *mut fsspec_async_context);
    pub fn fsspec_async_context_close(
        value: *mut fsspec_async_context,
        opts: *const fsspec_async_options,
        out: *mut *mut fsspec_operation,
        out_error: *mut *mut fsspec_async_error,
    ) -> i32;
    pub fn fsspec_async_file_retain(value: *const fsspec_async_file) -> *mut fsspec_async_file;
    pub fn fsspec_async_file_release(value: *mut fsspec_async_file);
    pub fn fsspec_async_file_close(
        value: *mut fsspec_async_file,
        opts: *const fsspec_async_options,
        out: *mut *mut fsspec_operation,
        out_error: *mut *mut fsspec_async_error,
    ) -> i32;
    pub fn fsspec_async_fs_retain(value: *const fsspec_async_fs) -> *mut fsspec_async_fs;
    pub fn fsspec_async_fs_release(value: *mut fsspec_async_fs);
    pub fn fsspec_async_fs_close(
        value: *mut fsspec_async_fs,
        opts: *const fsspec_async_options,
        out: *mut *mut fsspec_operation,
        out_error: *mut *mut fsspec_async_error,
    ) -> i32;
    pub fn fsspec_async_fs_info(
        value: *const fsspec_async_fs,
        path: *const c_char,
        path_len: usize,
        opts: *const fsspec_async_options,
        out: *mut *mut fsspec_operation,
        out_error: *mut *mut fsspec_async_error,
    ) -> i32;
    pub fn fsspec_async_fs_open_read(
        value: *const fsspec_async_fs,
        path: *const c_char,
        path_len: usize,
        opts: *const fsspec_async_options,
        out: *mut *mut fsspec_operation,
        out_error: *mut *mut fsspec_async_error,
    ) -> i32;
    pub fn fsspec_async_file_read(
        value: *const fsspec_async_file,
        max_bytes: usize,
        opts: *const fsspec_async_options,
        out: *mut *mut fsspec_operation,
        out_error: *mut *mut fsspec_async_error,
    ) -> i32;
    pub fn fsspec_async_file_info(
        value: *const fsspec_async_file,
        opts: *const fsspec_async_options,
        out: *mut *mut fsspec_operation,
        out_error: *mut *mut fsspec_async_error,
    ) -> i32;
    pub fn fsspec_async_file_seek(
        value: *const fsspec_async_file,
        offset: i64,
        whence: u32,
        opts: *const fsspec_async_options,
        out: *mut *mut fsspec_operation,
        out_error: *mut *mut fsspec_async_error,
    ) -> i32;
}

#[allow(improper_ctypes)]
extern "C" {
    pub fn fsspec_operation_drain(
        operation: *const fsspec_operation,
        out: *mut *mut fsspec_operation,
        out_error: *mut *mut fsspec_async_error,
    ) -> i32;
}
