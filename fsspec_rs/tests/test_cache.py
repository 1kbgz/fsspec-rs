"""Tests for the buffered file / caching layer."""

import os

import pytest

from fsspec_rs import LocalFileSystem

# ============================================================================
# Fixtures
# ============================================================================


@pytest.fixture
def tmp(tmp_path):
    """Return a temporary directory path as a string."""
    return str(tmp_path)


@pytest.fixture
def sample_file(tmp):
    """Create a sample 1000-byte file for read tests."""
    path = os.path.join(tmp, "sample.bin")
    data = bytes(range(256)) * 4  # 1024 bytes, repeating 0-255
    data = data[:1000]  # exactly 1000 bytes
    with open(path, "wb") as f:
        f.write(data)
    return path, data


# ============================================================================
# Test RustLocalFs.open() with cache_type parameter
# ============================================================================


class TestLocalOpenCacheType:
    """Verify that the cache_type parameter is accepted by the Rust open()."""

    def test_open_no_cache_type(self, sample_file):
        """Default open works without cache_type."""
        path, expected = sample_file
        fs = LocalFileSystem()
        with fs.open(path, "rb") as f:
            data = f.read()
        assert data == expected

    def test_open_cache_type_none(self, sample_file):
        """cache_type='none' is accepted."""
        path, expected = sample_file
        fs = LocalFileSystem()
        with fs.open(path, "rb", cache_type="none") as f:
            data = f.read()
        # For local FS, cache_type is passed through but local.rs
        # ignores it (local files are already efficient). The data
        # should be unchanged.
        assert data == expected

    def test_open_cache_type_all(self, sample_file):
        """cache_type='all' is accepted."""
        path, expected = sample_file
        fs = LocalFileSystem()
        with fs.open(path, "rb", cache_type="all") as f:
            data = f.read()
        assert data == expected

    def test_open_cache_type_readahead(self, sample_file):
        """cache_type='readahead' is accepted."""
        path, expected = sample_file
        fs = LocalFileSystem()
        with fs.open(path, "rb", cache_type="readahead") as f:
            data = f.read()
        assert data == expected

    def test_open_cache_type_block(self, sample_file):
        """cache_type='block' is accepted."""
        path, expected = sample_file
        fs = LocalFileSystem()
        with fs.open(path, "rb", cache_type="block") as f:
            data = f.read()
        assert data == expected

    def test_open_invalid_cache_type(self, sample_file):
        """Invalid cache_type raises ValueError."""
        path, _ = sample_file
        fs = LocalFileSystem()
        with pytest.raises(ValueError, match="unknown cache type"):
            fs.open(path, "rb", cache_type="bogus")

    def test_open_block_size(self, sample_file):
        """block_size parameter is accepted."""
        path, expected = sample_file
        fs = LocalFileSystem()
        with fs.open(path, "rb", block_size=256) as f:
            data = f.read()
        assert data == expected


# ============================================================================
# Test Rust-level RustLocalFs.open() directly
# ============================================================================


class TestRustOpenDirect:
    """Call the Rust inner open() directly to verify parameter passing."""

    def test_rust_open_with_cache_type(self, sample_file):
        from fsspec_rs import RustLocalFs

        path, expected = sample_file
        rfs = RustLocalFs()
        f = rfs.open(path, "rb", cache_type="all")
        data = bytes(f.read())
        f.close()
        assert data == expected

    def test_rust_open_with_block_size(self, sample_file):
        from fsspec_rs import RustLocalFs

        path, expected = sample_file
        rfs = RustLocalFs()
        f = rfs.open(path, "rb", cache_type="block", block_size=128)
        data = bytes(f.read())
        f.close()
        assert data == expected

    def test_rust_open_with_max_blocks(self, sample_file):
        from fsspec_rs import RustLocalFs

        path, expected = sample_file
        rfs = RustLocalFs()
        f = rfs.open(path, "rb", cache_type="block", block_size=128, max_blocks=2)
        data = bytes(f.read())
        f.close()
        assert data == expected


# ============================================================================
# Test write path with cache_type parameter
# ============================================================================


class TestWriteWithCacheType:
    """Write-mode files with cache_type should still work normally."""

    def test_write_with_cache_type_none(self, tmp):
        path = os.path.join(tmp, "out.bin")
        fs = LocalFileSystem()
        with fs.open(path, "wb", cache_type="none") as f:
            f.write(b"hello")
        with open(path, "rb") as f:
            assert f.read() == b"hello"

    def test_write_default(self, tmp):
        path = os.path.join(tmp, "out2.bin")
        fs = LocalFileSystem()
        with fs.open(path, "wb") as f:
            f.write(b"world")
        with open(path, "rb") as f:
            assert f.read() == b"world"


# ============================================================================
# Test _open() signature (called by fsspec.open())
# ============================================================================


class TestFsspecOpenSignature:
    """Verify that the _open() method passes cache_type through."""

    def test_open_passes_cache_type(self, sample_file):
        """Calling fs.open() with cache_type= works through _open()."""
        path, expected = sample_file
        fs = LocalFileSystem()
        # fsspec.AbstractFileSystem.open() calls _open()
        with fs.open(path, "rb", cache_type="readahead") as f:
            data = f.read()
        assert data == expected

    def test_open_passes_block_size(self, sample_file):
        """Calling fs.open() with block_size= works through _open()."""
        path, expected = sample_file
        fs = LocalFileSystem()
        with fs.open(path, "rb", block_size=512) as f:
            data = f.read()
        assert data == expected


# ============================================================================
# Test sequential and random read patterns
# ============================================================================


class TestReadPatterns:
    """Test different read patterns to exercise cache strategies."""

    def test_sequential_small_reads(self, sample_file):
        path, expected = sample_file
        fs = LocalFileSystem()
        with fs.open(path, "rb") as f:
            chunks = []
            while True:
                chunk = f.read(100)
                if not chunk:
                    break
                chunks.append(chunk)
        assert b"".join(chunks) == expected

    def test_seek_and_read(self, sample_file):
        path, expected = sample_file
        fs = LocalFileSystem()
        with fs.open(path, "rb") as f:
            f.seek(500)
            data = f.read(100)
        assert data == expected[500:600]

    def test_seek_from_end(self, sample_file):
        path, expected = sample_file
        fs = LocalFileSystem()
        with fs.open(path, "rb") as f:
            f.seek(-100, 2)
            data = f.read()
        assert data == expected[-100:]
