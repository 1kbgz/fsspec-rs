# Rust traits and types

The `fsspec_rs` crate is independent of Python. The `fsspec_rs_bridge` crate
adapts Python fsspec objects to its synchronous filesystem trait.

## Core model

Most public Rust types are re-exported from the crate root:

```rust
use fsspec_rs::{
    CacheType, FileInfo, FileSystem, FsFile, FsResult, LocalFs, OpenMode,
    OpenOptions, S3Config, S3Fs,
};
```

| Type                         | Purpose                                                                          |
| ---------------------------- | -------------------------------------------------------------------------------- |
| `FileSystem`                 | Synchronous filesystem trait with primitive methods and default helpers.         |
| `AsyncFileSystem`            | Experimental async mirror of `FileSystem`; no shipped backend implements it yet. |
| `FsFile`                     | File-like trait returned by `FileSystem::open()`.                                |
| `FileInfo`                   | Metadata for files, directories, and other entries.                              |
| `OpenMode` and `OpenOptions` | Open mode and buffered I/O options.                                              |
| `FsError` and `FsResult<T>`  | Shared error and result types.                                                   |
| `Cache` and `CacheType`      | Read cache strategy interface and selector.                                      |

## FileSystem

`LocalFs` and `S3Fs` implement the synchronous `FileSystem` trait. Backends must
implement seven primitive methods and `protocol`:

```rust
use fsspec_rs::{FileInfo, FileSystem, FsFile, FsResult, OpenMode, OpenOptions};

struct MyFs;

impl FileSystem for MyFs {
    fn protocol(&self) -> &[&str] {
        &["myfs"]
    }

    fn ls(&self, path: &str, detail: bool) -> FsResult<Vec<FileInfo>> {
        todo!()
    }

    fn rm_file(&self, path: &str) -> FsResult<()> {
        todo!()
    }

    fn cp_file(&self, src: &str, dst: &str) -> FsResult<()> {
        todo!()
    }

    fn open(
        &self,
        path: &str,
        mode: OpenMode,
        opts: Option<OpenOptions>,
    ) -> FsResult<Box<dyn FsFile>> {
        todo!()
    }

    fn info(&self, path: &str) -> FsResult<FileInfo> {
        todo!()
    }

    fn mkdir(&self, path: &str, create_parents: bool) -> FsResult<()> {
        todo!()
    }

    fn rmdir(&self, path: &str) -> FsResult<()> {
        todo!()
    }
}
```

`root_marker()` defaults to `""` and `sep()` defaults to `"/"`. Backends can override them
for local or platform-specific behavior. `strip_protocol()` and
`unstrip_protocol()` also have defaults and can be overridden when a backend
needs bucket-aware or authority-aware paths.

The default methods built on those primitives include:

- Path helpers: `strip_protocol`, `unstrip_protocol`, `parent`
- Metadata checks: `exists`, `isdir`, `isfile`, `size`, `sizes`
- File I/O helpers: `cat_file`, `pipe_file`, `head`, `tail`, `touch`,
  `read_text`, `write_text`
- Tree operations: `walk`, `find`, `copy`, `mv`, `rm`, `du`, `makedirs`
- Local transfers: `get_file`, `put_file`

## AsyncFileSystem

`AsyncFileSystem` is the experimental async version of `FileSystem`. Its
primitive methods return futures, and its default helpers are `async fn`s.
`LocalFs` and `S3Fs` implement only the synchronous `FileSystem` trait.

The default async implementation avoids recursive async futures in `walk()` by
using an explicit stack. Method semantics otherwise match `FileSystem`.

## FsFile

`FileSystem::open()` returns a file implementing `FsFile`, which requires
`Read + Write + Seek + Send`. `commit()` and `discard()` default to no-ops and
are available for buffered writes or future transaction support.

## Metadata and open options

`FileInfo` is the common metadata value. It stores:

- `name`: full path or backend path
- `size`: bytes, usually `0` for directories
- `file_type`: `FileType::File`, `FileType::Directory`, or `FileType::Other`
- `created` and `modified`: optional `SystemTime` values
- `extra`: backend-specific string metadata such as S3 ETags

Convenience constructors are available:

```rust
let file = FileInfo::file("/tmp/data.bin", 1024);
let dir = FileInfo::directory("/tmp");
```

`OpenMode::from_str_mode()` accepts `rb`, `wb`, `ab`, `xb`, and the equivalent
single-letter forms. `OpenOptions::default()` uses a 4 MiB block size,
`autocommit = true`, no explicit cache strategy, and `max_blocks = 32`.

## LocalFs

`LocalFs` uses `std::fs` and implements `FileSystem`.

`LocalFs::new()` starts with `auto_mkdir = false`. `LocalFs::with_auto_mkdir(true)` creates missing parent directories during
write, append, exclusive-create, and copy operations.

## S3Fs

`S3Fs` wraps `object_store::aws::AmazonS3` and runs its async calls through an
embedded Tokio runtime to implement the synchronous `FileSystem` trait.

`S3Config` supports:

- `bucket`
- `region`
- `endpoint_url`
- `access_key_id`
- `secret_access_key`
- `session_token`
- `anon`
- `virtual_hosted_style_request`

Plain HTTP is allowed automatically when `endpoint_url` starts with
`http://`, which is useful for MinIO and LocalStack.

## Python fsspec bridge

`fsspec_rs_bridge` exports `PyFsspecFs`, `url_to_fs`, and `StorageOptions`. It is an
`rlib` using PyO3 and Python fsspec at runtime, with no `#[pyclass]` exports.

`url_to_fs(url, options)` returns a filesystem adapter and start path.
`PyFsspecFs::from_protocol(protocol, options)` constructs an adapter by protocol.
The adapter's trait protocol is `python`; `source_protocol()` reports the
wrapped protocol. The bridge initializes and attaches to the Python interpreter
internally.

`PyFsspecFs` implements `FileSystem` and delegates `ls`, `info`, `open`, `cat_file`,
`pipe_file`, `cp_file`, `rm_file`, `mkdir`, `rmdir`, `get_file`, and `put_file` to
Python. Common Python exceptions map to `FsError` categories.

`StorageOptions` values support nulls, booleans, signed and unsigned integers,
floats, strings, lists, and nested maps. Nested maps keyed by protocol configure
chained URLs. The [Rust guide](how-to-rust.md) includes dependency and invocation
examples.

## Caching

The caching layer is used by buffered reads. `CacheType::from_str()` accepts:

| Value                       | Strategy                                           |
| --------------------------- | -------------------------------------------------- |
| `none`                      | `NoCache`: every fetch goes to the backend.        |
| `readahead` or `read_ahead` | `ReadAheadCache`: one lookahead window.            |
| `block` or `blockcache`     | `BlockCache`: fixed-size blocks with LRU eviction. |
| `all` or `bytes`            | `AllBytesCache`: whole-file cache on first read.   |

Each cache wraps a `Fetcher`, a `FnMut(u64, u64) -> FsResult<Vec<u8>>` that
retrieves the half-open byte range `[start, end)` from the backend.

## Error handling

All trait methods return `FsResult<T> = Result<T, FsError>`. `FsError` represents
filesystem errors with these variants:

- `NotFound`
- `PermissionDenied`
- `AlreadyExists`
- `NotADirectory`
- `IsADirectory`
- `IoError`
- `InvalidArgument`
- `NotSupported`
- `Other`

The PyO3 layer maps those variants to Python exceptions such as
`FileNotFoundError`, `PermissionError`, `FileExistsError`,
`NotADirectoryError`, `IsADirectoryError`, `ValueError`,
`NotImplementedError`, and `OSError`.
