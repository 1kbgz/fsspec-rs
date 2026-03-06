"""Tests for the Rust-backed LocalFileSystem.

Every test that creates a ``LocalFileSystem`` asserts it is an instance
of ``fsspec.AbstractFileSystem`` so we **enforce** the subclass invariant
across the whole suite.
"""

from __future__ import annotations

import os

import fsspec
import pytest

from fsspec_rs import LocalFile, LocalFileSystem, RustLocalFile, RustLocalFs


@pytest.fixture()
def tmp(tmp_path):
    """Return a temporary directory path as a string."""
    return str(tmp_path)


@pytest.fixture()
def fs():
    """Return a fresh ``LocalFileSystem``."""
    return LocalFileSystem()


@pytest.fixture()
def fs_auto(tmp):
    """Return a ``LocalFileSystem`` with auto_mkdir=True."""
    return LocalFileSystem(auto_mkdir=True)


class TestIsInstance:
    """Ensure our classes are subclasses of fsspec base classes."""

    def test_local_filesystem_is_abstract_fs(self, fs):
        assert isinstance(fs, fsspec.AbstractFileSystem)

    def test_local_filesystem_class_hierarchy(self):
        assert issubclass(LocalFileSystem, fsspec.AbstractFileSystem)

    def test_local_file_is_abstract_buffered_file(self, fs, tmp):
        path = os.path.join(tmp, "test.txt")
        with open(path, "w") as f:
            f.write("hello")
        lf = fs.open(path, "rb")
        try:
            assert isinstance(lf, fsspec.spec.AbstractBufferedFile)
        finally:
            lf.close()

    def test_local_file_class_hierarchy(self):
        assert issubclass(LocalFile, fsspec.spec.AbstractBufferedFile)


class TestConstruction:
    def test_default_construction(self, fs):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        assert fs.auto_mkdir is False

    def test_auto_mkdir_construction(self, fs_auto):
        assert isinstance(fs_auto, fsspec.AbstractFileSystem)
        assert fs_auto.auto_mkdir is True

    def test_protocol(self, fs):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        assert fs.protocol == ("file-rs", "local-rs")


class TestRustInner:
    def test_rust_local_fs_available(self, tmp):
        rust_fs = RustLocalFs()
        assert rust_fs.exists(tmp)

    def test_rust_local_file_closes(self, tmp):
        rust_fs = RustLocalFs()
        path = os.path.join(tmp, "inner.txt")
        rust_fs.pipe_file(path, b"data")
        rf = rust_fs.open(path, "rb")
        assert isinstance(rf, RustLocalFile)
        assert not rf.closed
        rf.close()
        assert rf.closed


class TestInfo:
    def test_info_file(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        path = os.path.join(tmp, "info.txt")
        fs.pipe_file(path, b"hello")
        info = fs.info(path)
        assert info["name"] == path
        assert info["type"] == "file"
        assert info["size"] == 5

    def test_info_directory(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        info = fs.info(tmp)
        assert info["type"] == "directory"

    def test_info_not_found(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        with pytest.raises(Exception):
            fs.info(os.path.join(tmp, "nope"))


class TestLs:
    def test_ls_detail(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        path = os.path.join(tmp, "a.txt")
        fs.pipe_file(path, b"data")
        entries = fs.ls(tmp, detail=True)
        assert isinstance(entries, list)
        assert len(entries) >= 1
        assert any(e["name"] == path for e in entries)

    def test_ls_names_only(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        fs.pipe_file(os.path.join(tmp, "b.txt"), b"x")
        entries = fs.ls(tmp, detail=False)
        assert isinstance(entries, list)
        assert any("b.txt" in e for e in entries)


class TestExistence:
    def test_exists(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        assert fs.exists(tmp)
        assert not fs.exists(os.path.join(tmp, "missing"))

    def test_isdir(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        assert fs.isdir(tmp)
        path = os.path.join(tmp, "f.txt")
        fs.pipe_file(path, b"x")
        assert not fs.isdir(path)

    def test_isfile(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        path = os.path.join(tmp, "g.txt")
        fs.pipe_file(path, b"x")
        assert fs.isfile(path)
        assert not fs.isfile(tmp)


class TestSize:
    def test_size(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        path = os.path.join(tmp, "sz.txt")
        fs.pipe_file(path, b"12345")
        assert fs.size(path) == 5


class TestDirs:
    def test_mkdir(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        d = os.path.join(tmp, "newdir")
        fs.mkdir(d)
        assert fs.isdir(d)

    def test_makedirs(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        d = os.path.join(tmp, "a", "b", "c")
        fs.makedirs(d, exist_ok=True)
        assert fs.isdir(d)

    def test_rmdir(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        d = os.path.join(tmp, "todelete")
        fs.mkdir(d)
        assert fs.isdir(d)
        fs.rmdir(d)
        assert not fs.exists(d)


class TestRemove:
    def test_rm_file(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        path = os.path.join(tmp, "del.txt")
        fs.pipe_file(path, b"x")
        fs.rm_file(path)
        assert not fs.exists(path)

    def test_rm_recursive(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        d = os.path.join(tmp, "tree")
        fs.mkdir(d)
        fs.pipe_file(os.path.join(d, "a.txt"), b"a")
        fs.rm(d, recursive=True)
        assert not fs.exists(d)

    def test_rm_list(self, fs, tmp):
        """rm() should accept a list of paths."""
        assert isinstance(fs, fsspec.AbstractFileSystem)
        p1 = os.path.join(tmp, "l1.txt")
        p2 = os.path.join(tmp, "l2.txt")
        fs.pipe_file(p1, b"1")
        fs.pipe_file(p2, b"2")
        fs.rm([p1, p2])
        assert not fs.exists(p1)
        assert not fs.exists(p2)


class TestCopyMove:
    def test_cp_file(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        src = os.path.join(tmp, "src.txt")
        dst = os.path.join(tmp, "dst.txt")
        fs.pipe_file(src, b"content")
        fs.cp_file(src, dst)
        assert fs.cat_file(dst) == b"content"

    def test_copy_recursive(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        src_dir = os.path.join(tmp, "srcdir")
        fs.mkdir(src_dir)
        fs.pipe_file(os.path.join(src_dir, "a.txt"), b"a")
        dst_dir = os.path.join(tmp, "dstdir")
        fs.copy(src_dir, dst_dir, recursive=True)
        assert fs.cat_file(os.path.join(dst_dir, "a.txt")) == b"a"

    def test_mv(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        src = os.path.join(tmp, "old.txt")
        dst = os.path.join(tmp, "new.txt")
        fs.pipe_file(src, b"move")
        fs.mv(src, dst)
        assert not fs.exists(src)
        assert fs.cat_file(dst) == b"move"


class TestReadWrite:
    def test_cat_pipe(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        path = os.path.join(tmp, "rw.txt")
        fs.pipe_file(path, b"hello world")
        assert fs.cat_file(path) == b"hello world"

    def test_cat_range(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        path = os.path.join(tmp, "range.txt")
        fs.pipe_file(path, b"0123456789")
        assert fs.cat_file(path, start=2, end=5) == b"234"

    def test_head(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        path = os.path.join(tmp, "head.txt")
        fs.pipe_file(path, b"abcdefghij")
        assert fs.head(path, 3) == b"abc"

    def test_tail(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        path = os.path.join(tmp, "tail.txt")
        fs.pipe_file(path, b"abcdefghij")
        assert fs.tail(path, 3) == b"hij"


class TestTouch:
    def test_touch_create(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        path = os.path.join(tmp, "touched.txt")
        fs.touch(path)
        assert fs.exists(path)

    def test_touch_truncate(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        path = os.path.join(tmp, "trunc.txt")
        fs.pipe_file(path, b"data")
        fs.touch(path, truncate=True)
        assert fs.size(path) == 0


class TestText:
    def test_write_read_text(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        path = os.path.join(tmp, "text.txt")
        fs.write_text(path, "hello text")
        assert fs.read_text(path) == "hello text"


class TestOpen:
    def test_open_write_read(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        path = os.path.join(tmp, "open.txt")
        with fs.open(path, "wb") as f:
            f.write(b"via open")
        with fs.open(path, "rb") as f:
            assert f.read() == b"via open"

    def test_open_returns_local_file(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        path = os.path.join(tmp, "lf.txt")
        fs.pipe_file(path, b"x")
        f = fs.open(path, "rb")
        assert isinstance(f, LocalFile)
        f.close()

    def test_open_context_manager(self, fs, tmp):
        path = os.path.join(tmp, "ctx.txt")
        fs.pipe_file(path, b"ctx")
        with fs.open(path, "rb") as f:
            data = f.read()
        assert data == b"ctx"
        assert f.closed

    def test_open_seek_tell(self, fs, tmp):
        path = os.path.join(tmp, "seek.txt")
        fs.pipe_file(path, b"0123456789")
        with fs.open(path, "rb") as f:
            f.seek(5)
            assert f.tell() == 5
            assert f.read() == b"56789"


class TestWalkFind:
    def _make_tree(self, fs, tmp):
        """Create sub/a.txt and sub/deep/b.txt."""
        sub = os.path.join(tmp, "sub")
        deep = os.path.join(sub, "deep")
        fs.makedirs(deep, exist_ok=True)
        fs.pipe_file(os.path.join(sub, "a.txt"), b"a")
        fs.pipe_file(os.path.join(deep, "b.txt"), b"b")
        return sub

    def test_walk(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        sub = self._make_tree(fs, tmp)
        entries = list(fs.walk(sub))
        # Should yield at least 2 entries (sub, sub/deep)
        assert len(entries) >= 2

    def test_find(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        sub = self._make_tree(fs, tmp)
        files = fs.find(sub)
        assert len(files) >= 2
        assert any("a.txt" in f for f in files)
        assert any("b.txt" in f for f in files)

    def test_find_with_dirs(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        sub = self._make_tree(fs, tmp)
        all_items = fs.find(sub, withdirs=True)
        assert len(all_items) >= 3  # deep dir + 2 files

    def test_walk_max_depth(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        sub = self._make_tree(fs, tmp)
        entries = list(fs.walk(sub, maxdepth=1))
        # maxdepth=1 → only the top-level directory
        dirs = [e for e in entries if e[0] == sub]
        assert len(dirs) == 1


class TestDu:
    def test_du_total(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        path = os.path.join(tmp, "du.txt")
        fs.pipe_file(path, b"12345")
        total = fs.du(tmp, total=True)
        assert total >= 5

    def test_du_per_path(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        fs.pipe_file(os.path.join(tmp, "x.txt"), b"123")
        fs.pipe_file(os.path.join(tmp, "y.txt"), b"45")
        result = fs.du(tmp, total=False)
        assert isinstance(result, dict)
        assert len(result) >= 2


class TestGetPut:
    def test_get_file(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        src = os.path.join(tmp, "remote.txt")
        dst = os.path.join(tmp, "local.txt")
        fs.pipe_file(src, b"getme")
        fs.get_file(src, dst)
        assert fs.cat_file(dst) == b"getme"

    def test_put_file(self, fs, tmp):
        assert isinstance(fs, fsspec.AbstractFileSystem)
        local = os.path.join(tmp, "upload_src.txt")
        remote = os.path.join(tmp, "upload_dst.txt")
        fs.pipe_file(local, b"putme")
        fs.put_file(local, remote)
        assert fs.cat_file(remote) == b"putme"


class TestAutoMkdir:
    def test_auto_mkdir_pipe(self, fs_auto, tmp):
        assert isinstance(fs_auto, fsspec.AbstractFileSystem)
        path = os.path.join(tmp, "deep", "nested", "auto.txt")
        fs_auto.pipe_file(path, b"auto")
        assert fs_auto.cat_file(path) == b"auto"
