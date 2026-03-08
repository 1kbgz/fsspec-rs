"""Benchmarks: fsspec_rs.LocalFileSystem vs fsspec.implementations.local.LocalFileSystem.

Covers ls, find, walk, cat_file, and batch get/put.

Run with:
    pytest fsspec_rs/benchmarks/bench_local.py -v --benchmark-columns=mean,stddev,rounds
"""

from __future__ import annotations

import os
import tempfile

# ── ls ─────────────────────────────────────────────────────────────────


class TestLs:
    """Benchmark directory listing."""

    def test_ls_rs(self, benchmark, rs_local_fs, local_bench_dir):
        benchmark(rs_local_fs.ls, local_bench_dir, detail=True)

    def test_ls_py(self, benchmark, py_local_fs, local_bench_dir):
        benchmark(py_local_fs.ls, local_bench_dir, detail=True)

    def test_ls_many_rs(self, benchmark, rs_local_fs, local_bench_dir):
        benchmark(rs_local_fs.ls, os.path.join(local_bench_dir, "many"), detail=True)

    def test_ls_many_py(self, benchmark, py_local_fs, local_bench_dir):
        benchmark(py_local_fs.ls, os.path.join(local_bench_dir, "many"), detail=True)


# ── find ───────────────────────────────────────────────────────────────


class TestFind:
    """Benchmark recursive file finding."""

    def test_find_rs(self, benchmark, rs_local_fs, local_bench_dir):
        benchmark(rs_local_fs.find, local_bench_dir)

    def test_find_py(self, benchmark, py_local_fs, local_bench_dir):
        benchmark(py_local_fs.find, local_bench_dir)


# ── walk ───────────────────────────────────────────────────────────────


class TestWalk:
    """Benchmark recursive walk."""

    def test_walk_rs(self, benchmark, rs_local_fs, local_bench_dir):
        benchmark(lambda: list(rs_local_fs.walk(local_bench_dir)))

    def test_walk_py(self, benchmark, py_local_fs, local_bench_dir):
        benchmark(lambda: list(py_local_fs.walk(local_bench_dir)))


# ── cat_file ───────────────────────────────────────────────────────────


class TestCatFile:
    """Benchmark reading file contents."""

    def test_cat_small_rs(self, benchmark, rs_local_fs, local_bench_dir):
        path = os.path.join(local_bench_dir, "small.bin")
        benchmark(rs_local_fs.cat_file, path)

    def test_cat_small_py(self, benchmark, py_local_fs, local_bench_dir):
        path = os.path.join(local_bench_dir, "small.bin")
        benchmark(py_local_fs.cat_file, path)

    def test_cat_medium_rs(self, benchmark, rs_local_fs, local_bench_dir):
        path = os.path.join(local_bench_dir, "medium.bin")
        benchmark(rs_local_fs.cat_file, path)

    def test_cat_medium_py(self, benchmark, py_local_fs, local_bench_dir):
        path = os.path.join(local_bench_dir, "medium.bin")
        benchmark(py_local_fs.cat_file, path)

    def test_cat_large_rs(self, benchmark, rs_local_fs, local_bench_dir):
        path = os.path.join(local_bench_dir, "large.bin")
        benchmark(rs_local_fs.cat_file, path)

    def test_cat_large_py(self, benchmark, py_local_fs, local_bench_dir):
        path = os.path.join(local_bench_dir, "large.bin")
        benchmark(py_local_fs.cat_file, path)


# ── batch get ──────────────────────────────────────────────────────────


class TestBatchGet:
    """Benchmark getting multiple files to local disk."""

    def test_get_batch_rs(self, benchmark, rs_local_fs, local_bench_dir, tmp_path):
        src = os.path.join(local_bench_dir, "many")
        files = rs_local_fs.ls(src, detail=False)

        def do_get():
            dst = tempfile.mkdtemp(dir=str(tmp_path))
            for f in files:
                rs_local_fs.get_file(f, os.path.join(dst, os.path.basename(f)))

        benchmark(do_get)

    def test_get_batch_py(self, benchmark, py_local_fs, local_bench_dir, tmp_path):
        src = os.path.join(local_bench_dir, "many")
        files = py_local_fs.ls(src, detail=False)

        def do_get():
            dst = tempfile.mkdtemp(dir=str(tmp_path))
            for f in files:
                py_local_fs.get_file(f, os.path.join(dst, os.path.basename(f)))

        benchmark(do_get)


# ── batch put ──────────────────────────────────────────────────────────


class TestBatchPut:
    """Benchmark putting multiple files."""

    def test_put_batch_rs(self, benchmark, rs_local_fs, local_bench_dir, tmp_path):
        src_dir = os.path.join(local_bench_dir, "many")
        files = rs_local_fs.ls(src_dir, detail=False)

        def do_put():
            dst = tempfile.mkdtemp(dir=str(tmp_path))
            for f in files:
                rs_local_fs.put_file(f, os.path.join(dst, os.path.basename(f)))

        benchmark(do_put)

    def test_put_batch_py(self, benchmark, py_local_fs, local_bench_dir, tmp_path):
        src_dir = os.path.join(local_bench_dir, "many")
        files = py_local_fs.ls(src_dir, detail=False)

        def do_put():
            dst = tempfile.mkdtemp(dir=str(tmp_path))
            for f in files:
                py_local_fs.put_file(f, os.path.join(dst, os.path.basename(f)))

        benchmark(do_put)
