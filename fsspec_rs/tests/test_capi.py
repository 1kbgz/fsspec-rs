"""Smoke test of libfsspec through ctypes, the way a non-Python consumer sees it."""

from ctypes import (
    CDLL,
    POINTER,
    Structure,
    byref,
    c_bool,
    c_char_p,
    c_int32,
    c_int64,
    c_size_t,
    c_uint32,
    c_uint64,
    c_void_p,
    pointer,
    sizeof,
    string_at,
)
from pathlib import Path
from shutil import which
from subprocess import run
from sys import platform

import pytest

import fsspec_rs


class Bytes(Structure):
    _fields_ = [("data", c_void_p), ("len", c_size_t)]


class String(Structure):
    _fields_ = [("data", c_void_p), ("len", c_size_t)]

    def value(self) -> str:
        return string_at(self.data, self.len).decode() if self.data else ""


class StringList(Structure):
    _fields_ = [("items", POINTER(String)), ("len", c_size_t)]


class FileInfo(Structure):
    _fields_ = [
        ("name", String),
        ("size", c_uint64),
        ("file_type", c_int32),
        ("created_ns", c_int64),
        ("modified_ns", c_int64),
        ("extra", StringList),
    ]


class FileInfoList(Structure):
    _fields_ = [("items", POINTER(FileInfo)), ("len", c_size_t)]


def _library_path() -> Path:
    name = {"win32": "fsspec.dll", "darwin": "libfsspec.dylib"}.get(platform, "libfsspec.so")
    return Path(fsspec_rs.get_library_dirs()[0]) / name


@pytest.fixture(scope="module")
def lib():
    path = _library_path()
    if not path.exists():
        pytest.skip(f"libfsspec is not built: {path}")
    lib = CDLL(str(path))
    lib.fsspec_abi_version.restype = c_uint32
    lib.fsspec_last_error_message.restype = c_char_p
    lib.fsspec_options_new.restype = c_void_p
    lib.fsspec_options_free.argtypes = [c_void_p]
    lib.fsspec_url_to_fs.argtypes = [c_char_p, c_size_t, c_void_p, POINTER(c_void_p), POINTER(String)]
    lib.fsspec_fs_release.argtypes = [c_void_p]
    lib.fsspec_fs_pipe_file.argtypes = [c_void_p, c_char_p, c_size_t, c_char_p, c_size_t]
    lib.fsspec_fs_cat_file.argtypes = [c_void_p, c_char_p, c_size_t, POINTER(c_int64), POINTER(c_int64), POINTER(Bytes)]
    lib.fsspec_fs_exists.argtypes = [c_void_p, c_char_p, c_size_t, POINTER(c_bool)]
    lib.fsspec_fs_ls.argtypes = [c_void_p, c_char_p, c_size_t, c_bool, POINTER(FileInfoList)]
    lib.fsspec_fs_info.argtypes = [c_void_p, c_char_p, c_size_t, POINTER(FileInfo)]
    lib.fsspec_bytes_free.argtypes = [Bytes]
    lib.fsspec_string_free.argtypes = [String]
    lib.fsspec_file_info_list_free.argtypes = [FileInfoList]
    lib.fsspec_protocols.argtypes = [POINTER(StringList)]
    lib.fsspec_string_list_free.argtypes = [StringList]
    return lib


def _url_to_fs(lib, url: str):
    fs = c_void_p()
    path = String()
    assert lib.fsspec_url_to_fs(url.encode(), len(url), None, byref(fs), byref(path)) == 0, lib.fsspec_last_error_message()
    resolved = path.value()
    lib.fsspec_string_free(path)
    return fs, resolved


def test_header_is_installed():
    header = Path(fsspec_rs.get_include()) / "fsspec" / "fsspec.h"
    if not header.exists():
        pytest.skip(f"header is not installed: {header}")
    assert "#define FSSPEC_ABI_VERSION 1" in header.read_text()
    assert fsspec_rs.get_libraries() == ["fsspec"]
    assert Path(fsspec_rs.get_prefix()) == Path(fsspec_rs.__file__).parent


def test_abi_version(lib):
    assert lib.fsspec_abi_version() == 1


def test_local_filesystem_through_c_api(lib, tmp_path):
    fs, root = _url_to_fs(lib, f"file://{tmp_path}")
    assert Path(root) == tmp_path
    try:
        path = str(tmp_path / "hello.txt").encode()
        assert lib.fsspec_fs_pipe_file(fs, path, len(path), b"hello", 5) == 0, lib.fsspec_last_error_message()

        exists = c_bool()
        assert lib.fsspec_fs_exists(fs, path, len(path), byref(exists)) == 0
        assert exists.value

        data = Bytes()
        assert lib.fsspec_fs_cat_file(fs, path, len(path), byref(c_int64(1)), byref(c_int64(3)), byref(data)) == 0
        assert string_at(data.data, data.len) == b"el"
        lib.fsspec_bytes_free(data)

        info = FileInfo()
        assert lib.fsspec_fs_info(fs, path, len(path), byref(info)) == 0
        assert info.size == 5
        assert info.file_type == 0
        assert info.name.value() == str(tmp_path / "hello.txt")
        lib.fsspec_string_free(info.name)
        lib.fsspec_string_list_free(info.extra)

        root_bytes = str(tmp_path).encode()
        entries = FileInfoList()
        assert lib.fsspec_fs_ls(fs, root_bytes, len(root_bytes), True, byref(entries)) == 0
        assert [entries.items[index].name.value() for index in range(entries.len)] == [str(tmp_path / "hello.txt")]
        lib.fsspec_file_info_list_free(entries)
    finally:
        lib.fsspec_fs_release(fs)


def test_errors_report_status_and_message(lib, tmp_path):
    fs, _ = _url_to_fs(lib, f"file://{tmp_path}")
    try:
        path = str(tmp_path / "missing.txt").encode()
        info = FileInfo()
        assert lib.fsspec_fs_info(fs, path, len(path), byref(info)) == 1
        assert b"missing.txt" in lib.fsspec_last_error_message()
    finally:
        lib.fsspec_fs_release(fs)


def test_registry_lists_builtin_protocols(lib):
    protocols = StringList()
    assert lib.fsspec_protocols(byref(protocols)) == 0
    names = {protocols.items[index].value() for index in range(protocols.len)}
    lib.fsspec_string_list_free(protocols)
    assert {"file", "local", "s3"} <= names


class Factory(Structure):
    _fields_ = [
        ("struct_size", c_size_t),
        ("abi_version", c_uint32),
        ("data", c_void_p),
        ("create", c_void_p),
        ("instance_scope", c_void_p),
        ("release", c_void_p),
    ]


def test_extension_shares_the_registry_with_libfsspec(lib):
    from fsspec_rs.fsspec_rs import linked_to_libfsspec, protocols

    if not linked_to_libfsspec():
        pytest.skip("the extension compiled the registry in")
    lib.fsspec_register_filesystem.argtypes = [c_char_p, c_size_t, POINTER(Factory)]
    factory = Factory(sizeof(Factory), 1, None, None, None, None)
    assert lib.fsspec_register_filesystem(b"probe", 5, byref(factory)) == 0, lib.fsspec_last_error_message()
    assert "probe" in protocols()


PROGRAM = """
#include <stdio.h>
#include <string.h>
#include <fsspec/fsspec.h>
#if FSSPEC_CONFLICT != 10 || FSSPEC_CONDITION_MATCH != 1 || FSSPEC_CONDITION_MISSING != 2
#error missing object operation constants
#endif

int main(int argc, char **argv) {
    if (argc != 2 || fsspec_abi_version() != FSSPEC_ABI_VERSION) return 2;
    fsspec_fs *fs = NULL;
    fsspec_string path = {0};
    if (fsspec_url_to_fs(argv[1], strlen(argv[1]), NULL, &fs, &path) != FSSPEC_OK) return 3;
    fsspec_file *file = NULL;
    fsspec_file_info info;
    if (fsspec_fs_open(fs, path.data, path.len, "rb", 2, &file) != FSSPEC_OK) return 4;
    if (fsspec_file_get_info(file, &info) != FSSPEC_OK) return 5;
    const int64_t start = 1, end = 4;
    fsspec_byte_range range = {&start, &end};
    fsspec_range_read_result ranges;
    if (fsspec_fs_read_ranges(fs, path.data, path.len, &range, 1, NULL, &ranges) != FSSPEC_OK) return 6;
    if (ranges.len != 1 || ranges.data[0].len != 3 || memcmp(ranges.data[0].data, "ell", 3)) return 7;
    fsspec_range_read_result_free(ranges);
    printf("%llu", (unsigned long long)info.size);
    fsspec_file_info_free(info);
    fsspec_file_close(file);
    fsspec_string_free(path);
    fsspec_fs_release(fs);
    return 0;
}
"""


@pytest.mark.parametrize("compiler,suffix", [("cc", ".c"), ("c++", ".cpp")])
def test_header_compiles_and_links(lib, tmp_path, compiler, suffix):
    if platform == "win32" or which(compiler) is None:
        pytest.skip(f"no {compiler} compiler")
    source = tmp_path / f"program{suffix}"
    source.write_text(PROGRAM)
    program = tmp_path / "program"
    library_dir = fsspec_rs.get_library_dirs()[0]
    command = [
        compiler,
        "-Wall",
        "-Werror",
        str(source),
        "-I",
        fsspec_rs.get_include(),
        "-L",
        library_dir,
        "-lfsspec",
        f"-Wl,-rpath,{library_dir}",
        "-o",
        str(program),
    ]
    run(command, check=True)
    target = tmp_path / "hello.txt"
    target.write_bytes(b"hello")
    result = run([str(program), f"file://{target}"], check=False, capture_output=True, text=True)
    assert (result.returncode, result.stdout) == (0, "5")


def test_python_backends_are_reachable_through_the_c_api(lib):
    """In a Python process the registry falls back to fsspec's own backends."""
    import fsspec
    from fsspec.implementations.memory import MemoryFileSystem

    # The native registry has its own memory://, so use a protocol only Python knows.
    class PythonMemory(MemoryFileSystem):
        protocol = ("python-memory",)

        @classmethod
        def _strip_protocol(cls, path):
            return super()._strip_protocol(path.replace("python-memory://", "memory://", 1))

    fsspec.register_implementation("python-memory", PythonMemory, clobber=True)
    fs, path = _url_to_fs(lib, "python-memory://shared/hello.txt")
    try:
        assert path == "/shared/hello.txt"
        encoded = path.encode()
        assert lib.fsspec_fs_pipe_file(fs, encoded, len(encoded), b"from c", 6) == 0, lib.fsspec_last_error_message()
    finally:
        lib.fsspec_fs_release(fs)
    assert fsspec.filesystem("python-memory").cat("/shared/hello.txt") == b"from c"


def test_native_memory_is_the_memory_rs_store(lib):
    """memory:// in the native registry and memory-rs:// in Python share one store."""
    import uuid

    from fsspec_rs import MemoryFileSystem

    url = f"memory://{uuid.uuid4().hex}/hello.txt"
    fs, path = _url_to_fs(lib, url)
    try:
        encoded = path.encode()
        assert lib.fsspec_fs_pipe_file(fs, encoded, len(encoded), b"from c", 6) == 0, lib.fsspec_last_error_message()
    finally:
        lib.fsspec_fs_release(fs)
    memory = MemoryFileSystem()
    assert memory.cat_file(path) == b"from c"
    memory.rm_file(path)


class Validator(Structure):
    _fields_ = [("scope", String), ("value", String)]


class Condition(Structure):
    _fields_ = [("kind", c_uint32), ("validator", POINTER(Validator))]


class WriteOptions(Structure):
    _fields_ = [("struct_size", c_size_t), ("condition", POINTER(Condition))]


class ReadOptions(Structure):
    _fields_ = [("struct_size", c_size_t), ("start", POINTER(c_int64)), ("end", POINTER(c_int64)), ("condition", POINTER(Condition))]


class ReadResult(Structure):
    _fields_ = [("data", Bytes), ("validator", Validator)]


class WriteResult(Structure):
    _fields_ = [("validator", Validator)]


def test_conditional_object_io_through_c_api(lib, tmp_path):
    lib.fsspec_fs_read.argtypes = [c_void_p, c_char_p, c_size_t, POINTER(ReadOptions), POINTER(ReadResult)]
    lib.fsspec_fs_write.argtypes = [c_void_p, c_char_p, c_size_t, c_char_p, c_size_t, POINTER(WriteOptions), POINTER(WriteResult)]
    lib.fsspec_read_result_free.argtypes = [ReadResult]
    lib.fsspec_write_result_free.argtypes = [WriteResult]
    fs, _ = _url_to_fs(lib, f"file://{tmp_path}")
    try:
        path = str(tmp_path / "exclusive").encode()
        missing = Condition(2, None)
        options = WriteOptions(sizeof(WriteOptions), pointer(missing))
        written = WriteResult()
        assert lib.fsspec_fs_write(fs, path, len(path), b"first", 5, byref(options), byref(written)) == 0
        assert not written.validator.scope.data
        lib.fsspec_write_result_free(written)
        assert lib.fsspec_fs_write(fs, path, len(path), b"second", 6, byref(options), byref(written)) == 10
        read = ReadResult()
        assert lib.fsspec_fs_read(fs, path, len(path), None, byref(read)) == 0
        assert string_at(read.data.data, read.data.len) == b"first"
        assert not read.validator.value.data
        lib.fsspec_read_result_free(read)
    finally:
        lib.fsspec_fs_release(fs)


class ByteRange(Structure):
    _fields_ = [("start", POINTER(c_int64)), ("end", POINTER(c_int64))]


class RangeReadOptions(Structure):
    _fields_ = [
        ("struct_size", c_size_t),
        ("condition", POINTER(Condition)),
        ("max_gap", c_uint64),
        ("max_request_size", c_uint64),
        ("max_concurrency", c_size_t),
    ]


class RangeReadResult(Structure):
    _fields_ = [("data", POINTER(Bytes)), ("len", c_size_t), ("validator", Validator)]


def test_coalesced_ranges_through_c_api(lib, tmp_path):
    lib.fsspec_fs_read_ranges.argtypes = [
        c_void_p,
        c_char_p,
        c_size_t,
        POINTER(ByteRange),
        c_size_t,
        POINTER(RangeReadOptions),
        POINTER(RangeReadResult),
    ]
    lib.fsspec_range_read_result_free.argtypes = [RangeReadResult]
    fs, _ = _url_to_fs(lib, f"file://{tmp_path}")
    try:
        path = str(tmp_path / "ranges").encode()
        assert lib.fsspec_fs_pipe_file(fs, path, len(path), b"0123456789", 10) == 0
        ranges = (ByteRange * 3)(
            ByteRange(pointer(c_int64(-3)), None),
            ByteRange(pointer(c_int64(1)), pointer(c_int64(5))),
            ByteRange(pointer(c_int64(5)), pointer(c_int64(3))),
        )
        options = RangeReadOptions(sizeof(RangeReadOptions), None, 1, 2, 2)
        result = RangeReadResult()
        assert lib.fsspec_fs_read_ranges(fs, path, len(path), ranges, len(ranges), byref(options), byref(result)) == 0
        try:
            assert result.len == 3
            assert [string_at(result.data[index].data, result.data[index].len) for index in range(result.len)] == [b"789", b"1234", b""]
            assert not result.validator.scope.data
        finally:
            lib.fsspec_range_read_result_free(result)
        assert lib.fsspec_fs_read_ranges(fs, path, len(path), None, 1, None, byref(result)) == 7
        options.max_concurrency = 0
        assert lib.fsspec_fs_read_ranges(fs, path, len(path), ranges, len(ranges), byref(options), byref(result)) == 7
    finally:
        lib.fsspec_fs_release(fs)


def test_native_object_handle_shares_s3_and_outlives_filesystem(lib, s3_endpoint):
    lib.fsspec_options_new.restype = c_void_p
    lib.fsspec_options_set.argtypes = [c_void_p, c_char_p, c_size_t, c_char_p, c_size_t]
    lib.fsspec_options_free.argtypes = [c_void_p]
    lib.fsspec_s3_open.argtypes = [c_char_p, c_size_t, c_void_p, POINTER(c_void_p), POINTER(c_void_p)]
    lib.fsspec_object_store_retain.argtypes = [c_void_p]
    lib.fsspec_object_store_retain.restype = c_void_p
    lib.fsspec_object_store_release.argtypes = [c_void_p]
    lib.fsspec_object_store_head.argtypes = [c_void_p, c_char_p, c_size_t, POINTER(FileInfo)]
    lib.fsspec_object_store_read_ranges.argtypes = [c_void_p, c_char_p, c_size_t, POINTER(ByteRange), c_size_t, POINTER(RangeReadResult)]
    lib.fsspec_file_info_free.argtypes = [FileInfo]
    lib.fsspec_range_read_result_free.argtypes = [RangeReadResult]
    options = lib.fsspec_options_new()
    fs, objects = c_void_p(), c_void_p()
    try:
        for key, value in [(b"anon", b"true"), (b"endpoint_url", s3_endpoint.encode())]:
            assert lib.fsspec_options_set(options, key, len(key), value, len(value)) == 0
        assert lib.fsspec_s3_open(b"test", 4, options, byref(fs), byref(objects)) == 0
    finally:
        lib.fsspec_options_free(options)
    try:
        assert lib.fsspec_fs_pipe_file(fs, b"test/data", 9, b"shared", 6) == 0
    finally:
        lib.fsspec_fs_release(fs)
    retained = lib.fsspec_object_store_retain(objects)
    lib.fsspec_object_store_release(objects)
    try:
        info = FileInfo()
        assert lib.fsspec_object_store_head(retained, b"data", 4, byref(info)) == 0
        try:
            assert info.name.value() == "data"
            assert info.size == 6
            assert info.file_type == 0
            assert info.modified_ns > 0
            extra = [info.extra.items[i].value() for i in range(info.extra.len)]
            assert extra == ["etag", '"version-1"']
        finally:
            lib.fsspec_file_info_free(info)
        ranges = (ByteRange * 2)(ByteRange(pointer(c_int64(3)), pointer(c_int64(99))), ByteRange(pointer(c_int64(0)), pointer(c_int64(2))))
        result = RangeReadResult()
        assert lib.fsspec_object_store_read_ranges(retained, b"data", 4, ranges, 2, byref(result)) == 0
        try:
            assert [string_at(result.data[i].data, result.data[i].len) for i in range(result.len)] == [b"red", b"sh"]
        finally:
            lib.fsspec_range_read_result_free(result)
        for start, end in [(6, 7), (7, 8), (0, 0)]:
            invalid = ByteRange(pointer(c_int64(start)), pointer(c_int64(end)))
            result = RangeReadResult(len=42)
            assert lib.fsspec_object_store_read_ranges(retained, b"data", 4, byref(invalid), 1, byref(result)) == 7
            assert result.len == 42
    finally:
        lib.fsspec_object_store_release(retained)
