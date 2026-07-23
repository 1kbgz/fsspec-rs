from __future__ import annotations

import os

import fsspec
import pytest

from fsspec_rs import RustS3File, RustS3Fs, S3FileSystem

_SKIP_REASON = "FSSPEC_S3_ENDPOINT_URL not set — no S3 credentials available"
_has_s3 = os.environ.get("FSSPEC_S3_ENDPOINT_URL") is not None

pytestmark = pytest.mark.skipif(not _has_s3, reason=_SKIP_REASON)

# Test data lives at s3://timkpaine-public/projects/organizeit2
# with 4 subdirs and 64 total files (all 0-byte placeholders).
BUCKET = os.environ.get("FSSPEC_S3_BUCKET", "timkpaine-public")
PREFIX = os.environ.get("FSSPEC_S3_PREFIX", "projects/organizeit2")
EXPECTED_FILE_COUNT = int(os.environ.get("FSSPEC_S3_EXPECTED_FILE_COUNT", "64"))
S3_ROOT = f"s3://{BUCKET}/{PREFIX}"
SUBDIR1 = f"s3://{BUCKET}/{PREFIX}/subdir1"
FILE1 = f"s3://{BUCKET}/{PREFIX}/subdir1/file1.txt"


@pytest.fixture()
def fs():
    """Return a fresh ``S3FileSystem`` backed by Rust."""
    return S3FileSystem(bucket=BUCKET)


class TestIsInstance:
    """Ensure our classes are subclasses of fsspec base classes."""

    def test_s3_filesystem_is_abstract_fs(self, fs):
        assert isinstance(fs, fsspec.AbstractFileSystem)

    def test_s3_file_is_buffered_file(self, fs):
        f = fs.open(FILE1, "rb")
        assert isinstance(f, fsspec.spec.AbstractBufferedFile)
        f.close()

    def test_rust_s3_fs_class_exists(self):
        """RustS3Fs should be importable from fsspec_rs."""
        assert RustS3Fs is not None

    def test_rust_s3_file_class_exists(self):
        """RustS3File should be importable from fsspec_rs."""
        assert RustS3File is not None


class TestProtocol:
    def test_protocol(self, fs):
        assert "s3-rs" in fs.protocol

    def test_bucket_attr(self, fs):
        assert fs.bucket == BUCKET


class TestLs:
    def test_ls_detail_false(self, fs):
        entries = fs.ls(S3_ROOT, detail=False)
        assert len(entries) == 4
        # All entries should be directory-like names
        basenames = sorted(e.split("/")[-1] for e in entries)
        assert basenames == ["subdir1", "subdir2", "subdir3", "subdir4"]

    def test_ls_detail_true(self, fs):
        entries = fs.ls(S3_ROOT, detail=True)
        assert len(entries) == 4
        for entry in entries:
            assert "name" in entry
            assert "type" in entry

    def test_ls_subdir(self, fs):
        entries = fs.ls(SUBDIR1, detail=False)
        assert len(entries) > 0
        for name in entries:
            assert "subdir1" in name


class TestInfo:
    def test_info_directory(self, fs):
        info = fs.info(SUBDIR1)
        assert info["type"] == "directory"

    def test_info_file(self, fs):
        info = fs.info(FILE1)
        assert info["type"] == "file"
        assert info["size"] == 0  # 0-byte placeholder

    def test_info_not_found(self, fs):
        with pytest.raises(FileNotFoundError):
            fs.info(f"s3://{BUCKET}/nonexistent/path/xyz")


class TestExistence:
    def test_exists_file(self, fs):
        assert fs.exists(FILE1) is True

    def test_exists_dir(self, fs):
        assert fs.exists(SUBDIR1) is True

    def test_exists_missing(self, fs):
        assert fs.exists(f"s3://{BUCKET}/nope/nope/nope") is False

    def test_isdir(self, fs):
        assert fs.isdir(SUBDIR1) is True
        assert fs.isdir(FILE1) is False

    def test_isfile(self, fs):
        assert fs.isfile(FILE1) is True
        assert fs.isfile(SUBDIR1) is False


class TestContent:
    def test_cat_file(self, fs):
        data = fs.cat_file(FILE1)
        assert isinstance(data, bytes)
        assert len(data) == 0  # 0-byte placeholder

    def test_head(self, fs):
        data = fs.head(FILE1, size=5)
        assert isinstance(data, bytes)
        assert len(data) == 0

    def test_tail(self, fs):
        data = fs.tail(FILE1, size=3)
        assert isinstance(data, bytes)
        assert len(data) == 0

    def test_size(self, fs):
        sz = fs.size(FILE1)
        assert sz == 0

    def test_read_text(self, fs):
        text = fs.read_text(FILE1)
        assert isinstance(text, str)
        assert text == ""


class TestOpen:
    def test_open_read(self, fs):
        with fs.open(FILE1, "rb") as f:
            data = f.read()
            assert isinstance(data, bytes)
            assert len(data) == 0

    def test_open_context_manager(self, fs):
        with fs.open(FILE1, "rb") as f:
            assert not f.closed
        assert f.closed


class TestFindWalk:
    def test_find(self, fs):
        files = fs.find(S3_ROOT)
        assert len(files) == EXPECTED_FILE_COUNT

    def test_walk(self, fs):
        entries = list(fs.walk(S3_ROOT))
        assert len(entries) > 0
        dirpath, dirnames, _filenames = entries[0]
        assert dirpath == S3_ROOT
        assert len(dirnames) >= 4
