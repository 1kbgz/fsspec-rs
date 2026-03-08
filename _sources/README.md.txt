# fsspec-rs

fsspec-compatible filesystem backends with Rust acceleration

[![Build Status](https://github.com/1kbgz/fsspec-rs/actions/workflows/build.yaml/badge.svg?branch=main&event=push)](https://github.com/1kbgz/fsspec-rs/actions/workflows/build.yaml)
[![codecov](https://codecov.io/gh/1kbgz/fsspec-rs/branch/main/graph/badge.svg)](https://codecov.io/gh/1kbgz/fsspec-rs)
[![License](https://img.shields.io/github/license/1kbgz/fsspec-rs)](https://github.com/1kbgz/fsspec-rs)
[![PyPI](https://img.shields.io/pypi/v/fsspec-rs.svg)](https://pypi.python.org/pypi/fsspec-rs)

## Overview

**fsspec-rs** provides drop-in replacements for [fsspec](https://filesystem-spec.readthedocs.io/) filesystem backends, with performance-critical operations implemented in Rust. The Python classes inherit from fsspec's real base classes (`AbstractFileSystem`, `AbstractBufferedFile`), so they work everywhere fsspec filesystems are accepted — pandas, dask, xarray, and the broader PyData ecosystem.

### Backends

| Backend          | Protocol              | Python class                | Replaces                                       |
| ---------------- | --------------------- | --------------------------- | ---------------------------------------------- |
| Local filesystem | `file-rs`, `local-rs` | `fsspec_rs.LocalFileSystem` | `fsspec.implementations.local.LocalFileSystem` |
| Amazon S3        | `s3-rs`               | `fsspec_rs.S3FileSystem`    | `s3fs.S3FileSystem`                            |

### Features

- **Pure Rust core** — standalone `FileSystem` and `AsyncFileSystem` traits usable from any Rust project, with no Python dependency
- **Full fsspec compatibility** — inherits from real fsspec base classes, participates in the registry, and passes isinstance checks
- **Pluggable read caching** — readahead, block, and all-bytes cache strategies for buffered S3 reads
- **S3 via `object_store`** — uses the battle-tested [object_store](https://crates.io/crates/object_store) crate (from Apache Arrow) for S3 access with retries, streaming, and standard AWS credential resolution

## Quick start

```python
# Local filesystem — drop-in replacement
from fsspec_rs import LocalFileSystem

fs = LocalFileSystem()
fs.ls("/tmp")
fs.cat_file("/tmp/example.txt")

# S3 filesystem
from fsspec_rs import S3FileSystem

fs = S3FileSystem(bucket="my-bucket", region="us-east-1")
data = fs.cat_file("path/to/object.parquet")
fs.pipe_file("path/to/output.txt", b"hello")

# Works with fsspec's open() and registry
import fsspec

with fsspec.open("local-rs:///tmp/example.txt", "rb") as f:
    print(f.read())
```

## Performance

Benchmarks comparing fsspec-rs against the pure-Python fsspec/s3fs implementations (measured with `pytest-benchmark`; S3 benchmarks run against a local MinIO instance):

### Local filesystem

| Operation           | fsspec-rs | fsspec (Python) | Speedup  |
| ------------------- | --------- | --------------- | -------- |
| `ls` (small dir)    | 6.5 µs    | 19.3 µs         | **3.0x** |
| `ls` (100 files)    | 73.5 µs   | 234.3 µs        | **3.2x** |
| `find` (recursive)  | 97.4 µs   | 326.3 µs        | **3.4x** |
| `walk` (recursive)  | 86.2 µs   | 299.6 µs        | **3.5x** |
| `cat_file` (4 KiB)  | 2.0 µs    | 5.7 µs          | **2.8x** |
| `get` (100 × 4 KiB) | 846 µs    | 1,840 µs        | **2.2x** |

### S3 (vs s3fs, against MinIO)

| Operation            | fsspec-rs | s3fs (Python) | Speedup  |
| -------------------- | --------- | ------------- | -------- |
| `cat_file` (4 KiB)   | 387 µs    | 1,190 µs      | **3.1x** |
| `cat_file` (256 KiB) | 958 µs    | 3,511 µs      | **3.7x** |
| `cat_file` (4 MiB)   | 6,156 µs  | 8,884 µs      | **1.4x** |
| `find` (recursive)   | 5,454 µs  | 11,803 µs     | **2.2x** |
| `pipe_file` (4 KiB)  | 4,093 µs  | 5,250 µs      | **1.3x** |
| `get_file` (4 KiB)   | 1,743 µs  | 2,816 µs      | **1.6x** |

> [!NOTE]
> This library was generated using [copier](https://copier.readthedocs.io/en/stable/) from the [Base Python Project Template repository](https://github.com/python-project-templates/base).
