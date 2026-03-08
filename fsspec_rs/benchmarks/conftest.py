from __future__ import annotations

import os
import socket

import pytest


MINIO_ENDPOINT = "http://localhost:9000"
MINIO_KEY = "minioadmin"
MINIO_SECRET = "minioadmin"
MINIO_BUCKET = "benchmark"


def _minio_is_reachable() -> bool:
    """Return True if something is listening on localhost:9000."""
    try:
        with socket.create_connection(("localhost", 9000), timeout=1):
            return True
    except OSError:
        return False


_has_minio = _minio_is_reachable()

# Sizes for test payloads
SMALL = 4 * 1024  # 4 KiB
MEDIUM = 256 * 1024  # 256 KiB
LARGE = 4 * 1024 * 1024  # 4 MiB


def _make_data(size: int) -> bytes:
    """Deterministic test payload of *size* bytes."""
    return bytes(range(256)) * (size // 256) + bytes(range(size % 256))


@pytest.fixture(scope="session")
def local_bench_dir(tmp_path_factory):
    """Session-scoped temp directory populated with benchmark data.

    Layout::
        <tmpdir>/
            small.bin        (4 KiB)
            medium.bin       (256 KiB)
            large.bin        (4 MiB)
            many/0000 .. many/0099   (100 x 4 KiB files)
            nested/a/b/c/d/e/f.bin   (deep nesting)
    """
    base = tmp_path_factory.mktemp("bench_local")

    # Single files at different sizes
    (base / "small.bin").write_bytes(_make_data(SMALL))
    (base / "medium.bin").write_bytes(_make_data(MEDIUM))
    (base / "large.bin").write_bytes(_make_data(LARGE))

    # Many small files
    many = base / "many"
    many.mkdir()
    for i in range(100):
        (many / f"{i:04d}").write_bytes(_make_data(SMALL))

    # Deeply nested path
    nested = base / "nested" / "a" / "b" / "c" / "d" / "e"
    nested.mkdir(parents=True)
    (nested / "f.bin").write_bytes(_make_data(SMALL))

    return str(base)


@pytest.fixture()
def rs_local_fs():
    """Return a Rust-backed LocalFileSystem."""
    from fsspec_rs import LocalFileSystem

    return LocalFileSystem()


@pytest.fixture()
def py_local_fs():
    """Return the pure-Python fsspec LocalFileSystem."""
    from fsspec.implementations.local import LocalFileSystem

    return LocalFileSystem()


requires_minio = pytest.mark.skipif(
    not _has_minio,
    reason="MinIO not reachable on localhost:9000 (run `make minio-start`)",
)


@pytest.fixture(scope="session")
def s3_bench_prefix():
    """Session-unique prefix so parallel runs don't collide."""
    import time

    return f"bench/{os.getpid()}/{int(time.time())}"


@pytest.fixture(scope="session")
def s3_bench_data(s3_bench_prefix):
    """Populate MinIO with benchmark data (mirrors local_bench_dir layout).

    Returns the S3 prefix string (e.g. ``bench/12345/1700000000``).
    """
    if not _has_minio:
        pytest.skip("MinIO not reachable")

    import s3fs

    fs = s3fs.S3FileSystem(
        key=MINIO_KEY,
        secret=MINIO_SECRET,
        endpoint_url=MINIO_ENDPOINT,
        client_kwargs={"endpoint_url": MINIO_ENDPOINT},
    )

    prefix = f"{MINIO_BUCKET}/{s3_bench_prefix}"

    # Upload test data
    fs.pipe_file(f"{prefix}/small.bin", _make_data(SMALL))
    fs.pipe_file(f"{prefix}/medium.bin", _make_data(MEDIUM))
    fs.pipe_file(f"{prefix}/large.bin", _make_data(LARGE))

    for i in range(100):
        fs.pipe_file(f"{prefix}/many/{i:04d}", _make_data(SMALL))

    fs.pipe_file(f"{prefix}/nested/a/b/c/d/e/f.bin", _make_data(SMALL))

    yield s3_bench_prefix

    # Cleanup
    try:
        fs.rm(prefix, recursive=True)
    except Exception:
        pass


@pytest.fixture()
def rs_s3_fs():
    """Return a Rust-backed S3FileSystem pointed at MinIO."""
    if not _has_minio:
        pytest.skip("MinIO not reachable")

    from fsspec_rs import S3FileSystem

    return S3FileSystem(
        bucket=MINIO_BUCKET,
        key=MINIO_KEY,
        secret=MINIO_SECRET,
        endpoint_url=MINIO_ENDPOINT,
        anon=False,
    )


@pytest.fixture()
def py_s3_fs():
    """Return the pure-Python s3fs S3FileSystem pointed at MinIO."""
    if not _has_minio:
        pytest.skip("MinIO not reachable")

    import s3fs

    return s3fs.S3FileSystem(
        key=MINIO_KEY,
        secret=MINIO_SECRET,
        endpoint_url=MINIO_ENDPOINT,
        client_kwargs={"endpoint_url": MINIO_ENDPOINT},
    )
