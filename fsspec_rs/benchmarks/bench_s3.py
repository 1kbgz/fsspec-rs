"""Benchmarks: fsspec_rs.S3FileSystem vs s3fs.S3FileSystem against MinIO.

Covers ls, cat_file, find, pipe_file (put), and get_file.

Requires MinIO running locally — start with:
    make minio-start

Run with:
    pytest fsspec_rs/benchmarks/bench_s3.py -v --benchmark-columns=mean,stddev,rounds
"""

from __future__ import annotations

from .conftest import MINIO_BUCKET, requires_minio

pytestmark = requires_minio


# ── ls ─────────────────────────────────────────────────────────────────


class TestLs:
    """Benchmark S3 directory listing."""

    def test_ls_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        benchmark(rs_s3_fs.ls, s3_bench_data, detail=True)

    def test_ls_py(self, benchmark, py_s3_fs, s3_bench_data):
        benchmark(py_s3_fs.ls, f"{MINIO_BUCKET}/{s3_bench_data}", detail=True)

    def test_ls_many_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        benchmark(rs_s3_fs.ls, f"{s3_bench_data}/many", detail=True)

    def test_ls_many_py(self, benchmark, py_s3_fs, s3_bench_data):
        benchmark(py_s3_fs.ls, f"{MINIO_BUCKET}/{s3_bench_data}/many", detail=True)


# ── find ───────────────────────────────────────────────────────────────


class TestFind:
    """Benchmark recursive finding."""

    def test_find_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        benchmark(rs_s3_fs.find, s3_bench_data)

    def test_find_py(self, benchmark, py_s3_fs, s3_bench_data):
        benchmark(py_s3_fs.find, f"{MINIO_BUCKET}/{s3_bench_data}")


# ── cat_file ───────────────────────────────────────────────────────────


class TestCatFile:
    """Benchmark reading file contents from S3."""

    def test_cat_small_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        benchmark(rs_s3_fs.cat_file, f"{s3_bench_data}/small.bin")

    def test_cat_small_py(self, benchmark, py_s3_fs, s3_bench_data):
        benchmark(py_s3_fs.cat_file, f"{MINIO_BUCKET}/{s3_bench_data}/small.bin")

    def test_cat_medium_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        benchmark(rs_s3_fs.cat_file, f"{s3_bench_data}/medium.bin")

    def test_cat_medium_py(self, benchmark, py_s3_fs, s3_bench_data):
        benchmark(py_s3_fs.cat_file, f"{MINIO_BUCKET}/{s3_bench_data}/medium.bin")

    def test_cat_large_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        benchmark(rs_s3_fs.cat_file, f"{s3_bench_data}/large.bin")

    def test_cat_large_py(self, benchmark, py_s3_fs, s3_bench_data):
        benchmark(py_s3_fs.cat_file, f"{MINIO_BUCKET}/{s3_bench_data}/large.bin")


# ── pipe_file (write/put) ─────────────────────────────────────────────


class TestPipeFile:
    """Benchmark writing data to S3."""

    _data_small = bytes(4 * 1024)
    _data_medium = bytes(256 * 1024)

    def test_pipe_small_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        path = f"{s3_bench_data}/bench_write_small_rs.bin"
        benchmark(rs_s3_fs.pipe_file, path, self._data_small)

    def test_pipe_small_py(self, benchmark, py_s3_fs, s3_bench_data):
        path = f"{MINIO_BUCKET}/{s3_bench_data}/bench_write_small_py.bin"
        benchmark(py_s3_fs.pipe_file, path, self._data_small)

    def test_pipe_medium_rs(self, benchmark, rs_s3_fs, s3_bench_data):
        path = f"{s3_bench_data}/bench_write_medium_rs.bin"
        benchmark(rs_s3_fs.pipe_file, path, self._data_medium)

    def test_pipe_medium_py(self, benchmark, py_s3_fs, s3_bench_data):
        path = f"{MINIO_BUCKET}/{s3_bench_data}/bench_write_medium_py.bin"
        benchmark(py_s3_fs.pipe_file, path, self._data_medium)


# ── get_file (download) ───────────────────────────────────────────────


class TestGetFile:
    """Benchmark downloading files from S3 to local disk."""

    def test_get_small_rs(self, benchmark, rs_s3_fs, s3_bench_data, tmp_path):
        src = f"{s3_bench_data}/small.bin"
        dst = str(tmp_path / "dl_small_rs.bin")
        benchmark(rs_s3_fs.get_file, src, dst)

    def test_get_small_py(self, benchmark, py_s3_fs, s3_bench_data, tmp_path):
        src = f"{MINIO_BUCKET}/{s3_bench_data}/small.bin"
        dst = str(tmp_path / "dl_small_py.bin")
        benchmark(py_s3_fs.get_file, src, dst)

    def test_get_large_rs(self, benchmark, rs_s3_fs, s3_bench_data, tmp_path):
        src = f"{s3_bench_data}/large.bin"
        dst = str(tmp_path / "dl_large_rs.bin")
        benchmark(rs_s3_fs.get_file, src, dst)

    def test_get_large_py(self, benchmark, py_s3_fs, s3_bench_data, tmp_path):
        src = f"{MINIO_BUCKET}/{s3_bench_data}/large.bin"
        dst = str(tmp_path / "dl_large_py.bin")
        benchmark(py_s3_fs.get_file, src, dst)
