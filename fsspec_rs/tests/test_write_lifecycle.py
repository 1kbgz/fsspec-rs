"""Pending writes require an explicit successful close or commit."""

import gc
import uuid

import pytest

from fsspec_rs import LocalFileSystem, MemoryFileSystem, RustLocalFs, RustMemoryFs, RustS3Fs, S3FileSystem


@pytest.fixture(params=[LocalFileSystem, MemoryFileSystem, RustLocalFs, RustMemoryFs, S3FileSystem, RustS3Fs])
def writer(request, tmp_path, s3_endpoint):
    cls = request.param
    if cls in (S3FileSystem, RustS3Fs):
        fs = cls(bucket="test", region="us-east-1", anon=True, endpoint_url=s3_endpoint)
        path = "test/value"
    else:
        fs = cls()
        path = str(tmp_path / "value") if cls in (LocalFileSystem, RustLocalFs) else f"/lifecycle-{uuid.uuid4().hex}"
    return fs, path


@pytest.mark.parametrize("mode", ["wb", "xb", "w", "wt"])
def test_exception_discards_pending_write(writer, mode):
    fs, path = writer
    if mode == "wt" and isinstance(fs, (RustLocalFs, RustMemoryFs, RustS3Fs)):
        pytest.skip("native handles accept binary mode aliases, not text wrappers")
    with pytest.raises(RuntimeError, match="abandon"), fs.open(path, mode) as file:
        file.write(b"pending" if "b" in mode or isinstance(fs, (RustLocalFs, RustMemoryFs, RustS3Fs)) else "pending")
        raise RuntimeError("abandon")
    with pytest.raises(FileNotFoundError):
        fs.read(path)


@pytest.mark.parametrize("mode", ["wb", "w"])
def test_abandoned_handle_discards_pending_write(writer, mode):
    fs, path = writer
    file = fs.open(path, mode)
    file.write(b"pending" if "b" in mode or isinstance(fs, (RustLocalFs, RustMemoryFs, RustS3Fs)) else "pending")
    del file
    gc.collect()
    with pytest.raises(FileNotFoundError):
        fs.read(path)


def test_explicit_close_publishes_and_discard_does_not(writer):
    fs, path = writer
    file = fs.open(path, "wb")
    file.write(b"committed")
    file.close()
    assert bytes(fs.cat_file(path)) == b"committed"
    file = fs.open(path, "wb")
    file.write(b"discarded")
    file.discard()
    file.close()
    assert bytes(fs.cat_file(path)) == b"committed"
