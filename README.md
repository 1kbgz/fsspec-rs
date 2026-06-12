# fsspec-rs

fsspec-compatible filesystem backends with Rust acceleration.

[![Build Status](https://github.com/1kbgz/fsspec-rs/actions/workflows/build.yaml/badge.svg?branch=main&event=push)](https://github.com/1kbgz/fsspec-rs/actions/workflows/build.yaml)
[![codecov](https://codecov.io/gh/1kbgz/fsspec-rs/branch/main/graph/badge.svg)](https://codecov.io/gh/1kbgz/fsspec-rs)
[![License](https://img.shields.io/github/license/1kbgz/fsspec-rs)](https://github.com/1kbgz/fsspec-rs)
[![PyPI](https://img.shields.io/pypi/v/fsspec-rs.svg)](https://pypi.python.org/pypi/fsspec-rs)

## Overview

`fsspec-rs` provides Python filesystem classes that inherit from
`fsspec.spec.AbstractFileSystem` while delegating core operations to Rust.
They can be used anywhere fsspec filesystems are accepted, including pandas,
dask, xarray, and direct `fsspec.open()` calls.

The package currently includes:

| Backend          | Protocol              | Python class                | Replaces                                       |
| ---------------- | --------------------- | --------------------------- | ---------------------------------------------- |
| Local filesystem | `file-rs`, `local-rs` | `fsspec_rs.LocalFileSystem` | `fsspec.implementations.local.LocalFileSystem` |
| Amazon S3        | `s3-rs`               | `fsspec_rs.S3FileSystem`    | `s3fs.S3FileSystem`                            |

## Install

```bash
pip install fsspec-rs
```

## Quick start

```python
import fsspec
from fsspec_rs import LocalFileSystem
from fsspec_rs import S3FileSystem

fs = LocalFileSystem()
fs.pipe_file("/tmp/example.txt", b"hello")
print(fs.cat_file("/tmp/example.txt"))

s3 = S3FileSystem(bucket="my-bucket", region="us-east-1")
s3.pipe_file("path/to/output.txt", b"hello")

with fsspec.open("file-rs:///tmp/example.txt", "rb") as f:
    print(f.read())
```

## Documentation

- [Filesystems](docs/src/filesystems.md): local and S3 usage, configuration, caching, and path conventions.
- [API reference](docs/src/api.md): Python API generated with yardang/Sphinx.
- [Rust traits](docs/src/rust.md): Rust `FileSystem`, `AsyncFileSystem`, `FsFile`, and cache traits.

Build docs locally with:

```bash
yardang build
```
