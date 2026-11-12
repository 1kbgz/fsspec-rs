"""Tests for the Rust-backed MemoryFileSystem.

The store is shared by the whole process, so each test works under its own root.
"""

from __future__ import annotations

import uuid

import fsspec
import pytest
from fsspec.implementations.memory import MemoryFileSystem as PythonMemoryFileSystem

import fsspec_rs
from fsspec_rs import MemoryFile, MemoryFileSystem


@pytest.fixture()
def fs():
    return MemoryFileSystem()


@pytest.fixture()
def root(fs):
    root = f"/{uuid.uuid4().hex}"
    yield root
    if fs.exists(root):
        fs.rm(root, recursive=True)


def test_is_an_fsspec_filesystem(fs, root):
    assert isinstance(fs, fsspec.AbstractFileSystem)
    assert fs.protocol == ("memory-rs",)
    fs.pipe_file(f"{root}/a", b"x")
    with fs.open(f"{root}/a", "rb") as f:
        assert isinstance(f, MemoryFile)
        assert isinstance(f, fsspec.spec.AbstractBufferedFile)


def test_registered_with_fsspec(root):
    with fsspec.open(f"memory-rs://{root}/hello.txt", "wb") as f:
        f.write(b"hello")
    with fsspec.open(f"memory-rs://{root}/hello.txt", "rb") as f:
        assert f.read() == b"hello"
    assert isinstance(fsspec.filesystem("memory-rs"), MemoryFileSystem)


def test_strip_protocol():
    assert MemoryFileSystem._strip_protocol("memory-rs://a/b/") == "/a/b"
    assert MemoryFileSystem._strip_protocol("memory-rs://") == "/"
    assert MemoryFileSystem._strip_protocol("a") == "/a"


def test_files_and_implied_directories(fs, root):
    fs.pipe_file(f"{root}/d/a.txt", b"abc")
    fs.pipe_file(f"{root}/b.txt", b"hello")

    assert fs.ls(root, detail=False) == [f"{root}/b.txt", f"{root}/d"]
    entries = {entry["name"]: entry for entry in fs.ls(root)}
    assert entries[f"{root}/d"]["type"] == "directory"
    assert entries[f"{root}/b.txt"]["size"] == 5
    assert fs.cat_file(f"{root}/b.txt", start=1, end=3) == b"el"
    assert fs.find(root) == [f"{root}/b.txt", f"{root}/d/a.txt"]
    assert fs.du(root) == 8

    fs.rm_file(f"{root}/d/a.txt")
    assert not fs.exists(f"{root}/d")


def test_directories(fs, root):
    fs.makedirs(f"{root}/x/y")
    assert fs.isdir(f"{root}/x/y")
    assert fs.ls(f"{root}/x/y") == []
    with pytest.raises(FileExistsError):
        fs.mkdir(f"{root}/x/y")
    fs.rmdir(f"{root}/x/y")
    assert not fs.exists(f"{root}/x/y")


def test_errors(fs, root):
    with pytest.raises(FileNotFoundError):
        fs.cat_file(f"{root}/missing")
    with pytest.raises(FileNotFoundError):
        fs.ls(f"{root}/missing")
    fs.pipe_file(f"{root}/f", b"")
    with pytest.raises(NotADirectoryError):
        fs.pipe_file(f"{root}/f/g", b"")
    fs.mkdir(f"{root}/d")
    with pytest.raises(IsADirectoryError):
        fs.open(f"{root}/d", "rb")


def test_write_is_visible_after_close(fs, root):
    path = f"{root}/w"
    f = fs.open(path, "wb")
    f.write(b"data")
    assert not fs.exists(path)
    f.close()
    assert fs.cat_file(path) == b"data"

    with fs.open(path, "ab") as f:
        f.write(b"!")
    assert fs.cat_file(path) == b"data!"

    with pytest.raises(FileExistsError):
        fs.open(path, "xb")


def test_copy_move_and_remove(fs, root):
    fs.pipe_file(f"{root}/src/a", b"1")
    fs.pipe_file(f"{root}/src/sub/b", b"2")
    fs.copy(f"{root}/src", f"{root}/dst", recursive=True)
    assert fs.cat_file(f"{root}/dst/sub/b") == b"2"
    fs.mv(f"{root}/dst/a", f"{root}/moved")
    assert not fs.exists(f"{root}/dst/a")
    fs.rm(f"{root}/src", recursive=True)
    assert not fs.exists(f"{root}/src")


def test_get_and_put_files(fs, root, tmp_path):
    local = tmp_path / "local.txt"
    local.write_bytes(b"from disk")
    fs.put_file(str(local), f"{root}/remote")
    fs.get_file(f"{root}/remote", str(tmp_path / "back.txt"))
    assert (tmp_path / "back.txt").read_bytes() == b"from disk"


def test_instances_share_one_store(root):
    MemoryFileSystem().pipe_file(f"{root}/shared", b"x")
    assert MemoryFileSystem(skip_instance_cache=True).cat_file(f"{root}/shared") == b"x"


def test_python_memory_protocol_is_separate(fs, root):
    fs.pipe_file(f"{root}/rust-only", b"x")
    assert not PythonMemoryFileSystem().exists(f"{root}/rust-only")


def test_registry_has_native_memory():
    assert "memory" in fsspec_rs.fsspec_rs.protocols()
