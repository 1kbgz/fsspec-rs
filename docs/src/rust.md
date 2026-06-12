# Rust traits

The Rust crate is named `fsspec_rs`. It can be used independently from the
Python package and exposes the same template-method shape as fsspec:
implement a small set of backend primitives, then get higher-level filesystem
methods from default trait implementations.

```toml
[dependencies]
fsspec_rs = "0.1.1"
```

For local development in this repository, the Python extension depends on the
crate by path:

```toml
fsspec_rs = { path = "./rust", version = "*" }
```

## Core model

Most public Rust types are re-exported from the crate root:

```rust
use fsspec_rs::{
    CacheType, FileInfo, FileSystem, FsFile, FsResult, LocalFs, OpenMode,
    OpenOptions, S3Config, S3Fs,
};
```

The main pieces are:

| Type                         | Purpose                                                                  |
| ---------------------------- | ------------------------------------------------------------------------ |
| `FileSystem`                 | Synchronous filesystem trait with primitive methods and default helpers. |
| `AsyncFileSystem`            | Async mirror of `FileSystem` for async-native backends.                  |
| `FsFile`                     | File-like trait returned by `FileSystem::open()`.                        |
| `FileInfo`                   | Metadata for files, directories, and other entries.                      |
| `OpenMode` and `OpenOptions` | Open mode and buffered I/O options.                                      |
| `FsError` and `FsResult<T>`  | Shared error and result types.                                           |
| `Cache` and `CacheType`      | Read cache strategy interface and selector.                              |

## FileSystem

`FileSystem` is the sync trait implemented by `LocalFs` and `S3Fs`.
Backends must provide seven core primitives plus protocol metadata:

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

`root_marker()` defaults to `""` and `sep()` defaults to `"/"`. Override them
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

`AsyncFileSystem` mirrors `FileSystem`, but primitive methods return futures
and default helpers are `async fn`s. It is intended for backends whose native
clients are async, such as object stores or HTTP services.

The default async implementation avoids recursive async futures in `walk()` by
using an explicit stack. Method semantics otherwise match `FileSystem`.

## FsFile

Opened files implement `FsFile`:

```rust
use std::io::{Read, Seek, Write};
use fsspec_rs::{FileInfo, FsFile, FsResult};

struct MyFile;

impl Read for MyFile {
    /* ... */
}

impl Write for MyFile {
    /* ... */
}

impl Seek for MyFile {
    /* ... */
}

impl FsFile for MyFile {
    fn info(&self) -> FsResult<FileInfo> {
        todo!()
    }

    fn size(&self) -> FsResult<Option<u64>> {
        todo!()
    }
}
```

`FsFile` requires `Read + Write + Seek + Send`. `commit()` and `discard()`
default to no-ops and are available for buffered writes or future transaction
support.

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

```rust
use fsspec_rs::{FileSystem, LocalFs};

fn main() -> fsspec_rs::FsResult<()> {
    let fs = LocalFs::with_auto_mkdir(true);
    fs.pipe_file("/tmp/fsspec-rs.txt", b"hello")?;
    let _data = fs.cat_file("/tmp/fsspec-rs.txt", None, None)?;
    Ok(())
}
```

`LocalFs::new()` starts with `auto_mkdir = false`. Use
`LocalFs::with_auto_mkdir(true)` to create missing parent directories during
write, append, exclusive-create, and copy operations.

## S3Fs

`S3Fs` wraps `object_store::aws::AmazonS3` and presents a sync `FileSystem`
API through an embedded Tokio runtime.

```rust
use fsspec_rs::{FileSystem, S3Config, S3Fs};

fn main() -> fsspec_rs::FsResult<()> {
    let mut cfg = S3Config::new("my-bucket");
    cfg.region = Some("us-east-1".to_string());

    let fs = S3Fs::new(cfg)?;
    let _objects = fs.ls("", true)?;
    Ok(())
}
```

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

All trait methods return `FsResult<T> = Result<T, FsError>`. `FsError` maps
common filesystem semantics into stable variants:

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
