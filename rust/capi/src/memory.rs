use fsspec_sys::types::{
    fsspec_bytes, fsspec_file_info, fsspec_file_info_list, fsspec_string, fsspec_string_list,
};

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

/// Frees all buffers in a successful read result.
#[no_mangle]
pub unsafe extern "C" fn fsspec_read_result_free(result: fsspec_sys::types::fsspec_read_result) {
    result.free();
}

/// Frees all buffers in a successful write result.
#[no_mangle]
pub unsafe extern "C" fn fsspec_write_result_free(result: fsspec_sys::types::fsspec_write_result) {
    result.free();
}
