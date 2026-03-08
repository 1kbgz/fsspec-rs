"""Benchmarks: cached vs uncached S3 reads — sequential and random access.

Compares different cache strategies (none, readahead, bytes, all) for
sequential reads and random-offset reads against MinIO.

Requires MinIO running locally — start with:
    make minio-start

Run with:
    pytest fsspec_rs/benchmarks/bench_cache.py -v --benchmark-columns=mean,stddev,rounds
"""

from __future__ import annotations

from .conftest import MINIO_BUCKET, requires_minio

pytestmark = requires_minio


# ── Helpers ────────────────────────────────────────────────────────────


def _sequential_read(fs, path, chunk_size=8192, **open_kwargs):
    """Read a file sequentially in fixed-size chunks."""
    with fs.open(path, "rb", **open_kwargs) as f:
        while True:
            data = f.read(chunk_size)
            if not data:
                break


def _random_read(fs, path, offsets, read_size=4096, **open_kwargs):
    """Seek to each offset and read *read_size* bytes."""
    with fs.open(path, "rb", **open_kwargs) as f:
        for off in offsets:
            f.seek(off)
            f.read(read_size)


# Fixed random-ish offsets within a 4 MiB file (deterministic, no randomness)
_RANDOM_OFFSETS_LARGE = [
    0,
    1000000,
    500000,
    3500000,
    2000000,
    100000,
    3900000,
    750000,
    2500000,
    1500000,
    3000000,
    250000,
    1750000,
    3250000,
    50000,
    4000000,
]

# Offsets within a 256 KiB file
_RANDOM_OFFSETS_MEDIUM = [
    0,
    100000,
    50000,
    200000,
    150000,
    25000,
    175000,
    75000,
    125000,
    10000,
    225000,
    60000,
    180000,
    30000,
    250000,
    5000,
]


# ── Sequential reads — large file ─────────────────────────────────────


class TestSequentialLarge:
    """Sequential read of 4 MiB file with different cache strategies."""

    def test_no_cache_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        path = f"{s3_bench_data}/large.bin"
        benchmark(_sequential_read, rs_s3_fs, path)

    def test_readahead_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        path = f"{s3_bench_data}/large.bin"
        benchmark(_sequential_read, rs_s3_fs, path, cache_type="readahead")

    def test_bytes_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        path = f"{s3_bench_data}/large.bin"
        benchmark(_sequential_read, rs_s3_fs, path, cache_type="bytes")

    def test_all_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        path = f"{s3_bench_data}/large.bin"
        benchmark(_sequential_read, rs_s3_fs, path, cache_type="all")

    def test_no_cache_py(self, benchmark, py_s3_fs, s3_bench_data):
        path = f"{MINIO_BUCKET}/{s3_bench_data}/large.bin"
        benchmark(_sequential_read, py_s3_fs, path)

    def test_readahead_py(self, benchmark, py_s3_fs, s3_bench_data):
        path = f"{MINIO_BUCKET}/{s3_bench_data}/large.bin"
        benchmark(_sequential_read, py_s3_fs, path, cache_type="readahead")

    def test_bytes_py(self, benchmark, py_s3_fs, s3_bench_data):
        path = f"{MINIO_BUCKET}/{s3_bench_data}/large.bin"
        benchmark(_sequential_read, py_s3_fs, path, cache_type="bytes")

    def test_all_py(self, benchmark, py_s3_fs, s3_bench_data):
        path = f"{MINIO_BUCKET}/{s3_bench_data}/large.bin"
        benchmark(_sequential_read, py_s3_fs, path, cache_type="all")


# ── Sequential reads — medium file ────────────────────────────────────


class TestSequentialMedium:
    """Sequential read of 256 KiB file with different cache strategies."""

    def test_no_cache_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        path = f"{s3_bench_data}/medium.bin"
        benchmark(_sequential_read, rs_s3_fs, path)

    def test_readahead_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        path = f"{s3_bench_data}/medium.bin"
        benchmark(_sequential_read, rs_s3_fs, path, cache_type="readahead")

    def test_all_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        path = f"{s3_bench_data}/medium.bin"
        benchmark(_sequential_read, rs_s3_fs, path, cache_type="all")

    def test_no_cache_py(self, benchmark, py_s3_fs, s3_bench_data):
        path = f"{MINIO_BUCKET}/{s3_bench_data}/medium.bin"
        benchmark(_sequential_read, py_s3_fs, path)

    def test_readahead_py(self, benchmark, py_s3_fs, s3_bench_data):
        path = f"{MINIO_BUCKET}/{s3_bench_data}/medium.bin"
        benchmark(_sequential_read, py_s3_fs, path, cache_type="readahead")

    def test_all_py(self, benchmark, py_s3_fs, s3_bench_data):
        path = f"{MINIO_BUCKET}/{s3_bench_data}/medium.bin"
        benchmark(_sequential_read, py_s3_fs, path, cache_type="all")


# ── Random access reads — large file ──────────────────────────────────


class TestRandomLarge:
    """Random-offset reads of 4 MiB file with different cache strategies."""

    def test_no_cache_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        path = f"{s3_bench_data}/large.bin"
        benchmark(_random_read, rs_s3_fs, path, _RANDOM_OFFSETS_LARGE)

    def test_readahead_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        path = f"{s3_bench_data}/large.bin"
        benchmark(
            _random_read,
            rs_s3_fs,
            path,
            _RANDOM_OFFSETS_LARGE,
            cache_type="readahead",
        )

    def test_bytes_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        path = f"{s3_bench_data}/large.bin"
        benchmark(
            _random_read,
            rs_s3_fs,
            path,
            _RANDOM_OFFSETS_LARGE,
            cache_type="bytes",
        )

    def test_all_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        path = f"{s3_bench_data}/large.bin"
        benchmark(
            _random_read,
            rs_s3_fs,
            path,
            _RANDOM_OFFSETS_LARGE,
            cache_type="all",
        )

    def test_no_cache_py(self, benchmark, py_s3_fs, s3_bench_data):
        path = f"{MINIO_BUCKET}/{s3_bench_data}/large.bin"
        benchmark(_random_read, py_s3_fs, path, _RANDOM_OFFSETS_LARGE)

    def test_readahead_py(self, benchmark, py_s3_fs, s3_bench_data):
        path = f"{MINIO_BUCKET}/{s3_bench_data}/large.bin"
        benchmark(
            _random_read,
            py_s3_fs,
            path,
            _RANDOM_OFFSETS_LARGE,
            cache_type="readahead",
        )

    def test_bytes_py(self, benchmark, py_s3_fs, s3_bench_data):
        path = f"{MINIO_BUCKET}/{s3_bench_data}/large.bin"
        benchmark(
            _random_read,
            py_s3_fs,
            path,
            _RANDOM_OFFSETS_LARGE,
            cache_type="bytes",
        )

    def test_all_py(self, benchmark, py_s3_fs, s3_bench_data):
        path = f"{MINIO_BUCKET}/{s3_bench_data}/large.bin"
        benchmark(
            _random_read,
            py_s3_fs,
            path,
            _RANDOM_OFFSETS_LARGE,
            cache_type="all",
        )


# ── Random access reads — medium file ─────────────────────────────────


class TestRandomMedium:
    """Random-offset reads of 256 KiB file with different cache strategies."""

    def test_no_cache_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        path = f"{s3_bench_data}/medium.bin"
        benchmark(_random_read, rs_s3_fs, path, _RANDOM_OFFSETS_MEDIUM)

    def test_bytes_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        path = f"{s3_bench_data}/medium.bin"
        benchmark(
            _random_read,
            rs_s3_fs,
            path,
            _RANDOM_OFFSETS_MEDIUM,
            cache_type="bytes",
        )

    def test_all_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        path = f"{s3_bench_data}/medium.bin"
        benchmark(
            _random_read,
            rs_s3_fs,
            path,
            _RANDOM_OFFSETS_MEDIUM,
            cache_type="all",
        )

    def test_no_cache_py(self, benchmark, py_s3_fs, s3_bench_data):
        path = f"{MINIO_BUCKET}/{s3_bench_data}/medium.bin"
        benchmark(_random_read, py_s3_fs, path, _RANDOM_OFFSETS_MEDIUM)

    def test_bytes_py(self, benchmark, py_s3_fs, s3_bench_data):
        path = f"{MINIO_BUCKET}/{s3_bench_data}/medium.bin"
        benchmark(
            _random_read,
            py_s3_fs,
            path,
            _RANDOM_OFFSETS_MEDIUM,
            cache_type="bytes",
        )

    def test_all_py(self, benchmark, py_s3_fs, s3_bench_data):
        path = f"{MINIO_BUCKET}/{s3_bench_data}/medium.bin"
        benchmark(
            _random_read,
            py_s3_fs,
            path,
            _RANDOM_OFFSETS_MEDIUM,
            cache_type="all",
        )
