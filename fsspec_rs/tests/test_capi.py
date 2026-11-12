"""Smoke test of libfsspec through ctypes, the way a non-Python consumer sees it."""

from ctypes import CDLL, POINTER, Structure, byref, c_bool, c_char_p, c_int32, c_int64, c_size_t, c_uint32, c_uint64, c_void_p, sizeof, string_at
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

int main(int argc, char **argv) {
    if (argc != 2 || fsspec_abi_version() != FSSPEC_ABI_VERSION) return 2;
    fsspec_fs *fs = NULL;
    fsspec_string path = {0};
    if (fsspec_url_to_fs(argv[1], strlen(argv[1]), NULL, &fs, &path) != FSSPEC_OK) return 3;
    fsspec_file *file = NULL;
    fsspec_file_info info;
    if (fsspec_fs_open(fs, path.data, path.len, "rb", 2, &file) != FSSPEC_OK) return 4;
    if (fsspec_file_get_info(file, &info) != FSSPEC_OK) return 5;
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
