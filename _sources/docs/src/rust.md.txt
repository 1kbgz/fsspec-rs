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

### Object I/O and conditions

`FileSystem::read(path, ReadOptions)` returns `ReadResult { data, validator }`.
`FileSystem::write(path, data, WriteOptions)` returns `WriteResult { validator }`
after committing the write. A validator, when available, describes the object
read or committed by that operation. Obtain it from the result and pass it back
unchanged; it contains an opaque scope and value, not a general content hash.

Both options structs accept `condition: Option<Condition>`:

| Condition          | Read                           | Write                             |
| ------------------ | ------------------------------ | --------------------------------- |
| `None`             | Read current bytes.            | Replace or create.                |
| `Match(validator)` | Read only the matching object. | Replace only the matching object. |
| `Missing`          | Invalid argument.              | Create only if absent.            |

```rust
use fsspec_rs::{Condition, FileSystem, OperationError, ReadOptions, WriteOptions};

fn replace(fs: &dyn FileSystem, path: &str, data: &[u8]) -> Result<(), OperationError> {
    let current = fs.read(path, ReadOptions::default())?;
    let validator = current.validator.ok_or_else(|| {
        fsspec_rs::FsError::NotSupported("backend returned no validator".into())
    })?;
    fs.write(path, data, WriteOptions {
        condition: Some(Condition::Match(validator)),
    })?;
    Ok(())
}
```

`ReadOptions.start` and `.end` follow `cat_file` slicing semantics: negative
offsets count from the end, bounds are clamped, and reversed ranges are empty.
Empty reads still check the condition. S3 pins a ranged read to the validator
returned by its metadata request, even without a caller-supplied condition.
A concurrent replacement between metadata and data requests returns a conflict.

`OperationError::Conflict` means a condition failed, including a Match against a
deleted object. `OperationError::FileSystem` wraps existing filesystem errors;
unconditional reads of missing objects return `FsError::NotFound`. Unsupported
conditions return `FsError::NotSupported` before I/O. `FsError` itself is unchanged.

`OperationError::kind()` returns a stable `ErrorKind` category and
`retry_safe()` reports whether the operation can safely be repeated.
`OperationError::Detailed { kind, message, retry_safe }` lets backends preserve
these fields. `UnknownOutcome` always reports false for retry safety. The
existing exhaustive `FsError` variants remain unchanged; `FsError::kind()` also
classifies underlying timeout, connection and corrupt-data I/O errors.

S3 typed writes disable automatic PUT retries. Uncertain Missing writes resolve
by reading and comparing SHA-256 content checksums; local exclusive writes also
verify content after I/O errors. Matching content acknowledges publication,
different content is a conflict, and failed verification is `UnknownOutcome`.
This acknowledgement does not prove writer ownership. Definitive precondition
failures stay conflicts. Other uncertain S3 typed writes return `UnknownOutcome`
without automatic recovery.

`io_capabilities()` reports `read_match`, `write_match`, and `write_missing`:

| Backend                                               | Match read | Match write | Missing write | Result validator                                                 |
| ----------------------------------------------------- | ---------- | ----------- | ------------- | ---------------------------------------------------------------- |
| S3                                                    | Yes        | Yes         | Yes           | ETag scoped to endpoint, addressing mode, bucket and object path |
| Local, memory                                         | No         | No          | Yes           | None                                                             |
| Default implementation, Python bridge, old C backends | No         | No          | No            | None                                                             |

S3 enforces conditions in the backend request. Validators from another endpoint, addressing mode,
bucket or path are invalid arguments, as are empty ETags, wildcards and ETag lists.
ETags are equality tokens: writing identical content can retain an ETag, so Match
does not promise detection of every intervening write.

Local and memory Missing writes use exclusive creation. Local writes stage and
sync a sibling temporary file, then publish it with a hard link that fails when
the destination exists. Readers see the complete file. Filesystems must support
hard links and the required synchronization operations; NFS guarantees depend
on the server and mount configuration. Deployment checks are described in the
[create-if-absent guide](how-to.md#create-conformance).

The Rust bindings, C ABI and Python wrappers expose the same operations. Older C
backends reject conditions and retain unconditional I/O through their existing
callbacks. Conditions currently apply to whole-object writes and whole-object or
ranged byte reads. Open handles, append, copy, delete and async APIs do not accept
conditions. Table row predicates remain a separate concern in `fsspec-data`.

### Coalesced range reads

`FileSystem::read_ranges(path, &[ByteRange], RangeReadOptions)` returns a
`RangeReadResult` containing `data: Vec<Vec<u8>>` in input order and an optional
shared validator. Each `ByteRange { start, end }` uses the same signed, optional
bounds as `read`: ends are exclusive, negative offsets count from the end,
bounds are clamped, and reversed ranges are empty. Duplicates and overlapping
ranges each receive their own output buffer.

`RangeReadOptions` accepts:

| Field              | Default         | Meaning                                                                                |
| ------------------ | --------------- | -------------------------------------------------------------------------------------- |
| `condition`        | `None`          | Optional `Match(validator)`; `Missing` is invalid.                                     |
| `max_gap`          | 65,536 bytes    | Largest gap eligible for coalescing. Zero still merges adjacent or overlapping ranges. |
| `max_request_size` | 8,388,608 bytes | Maximum data span per request; larger input ranges are split. Must be positive.        |
| `max_concurrency`  | 8               | Maximum concurrent requests within this call. Must be positive.                        |

S3 reads metadata once, then pins data requests to its validator, including
requests made without a caller-supplied Match. It rejects changed validators,
response bounds or response lengths instead of returning partial results. All
range-read calls and S3 instances using the same canonical endpoint share an
additional cap of eight active requests, including metadata requests. Data-request
permits remain held until their response bodies finish. The first request failure
drops the operation's remaining futures.

The default implementation coalesces requests and reads them sequentially through
`cat_file`. Local, memory and older C backends use this fallback, return no validator,
and reject Match. Such results have no snapshot guarantee during concurrent writes.
Existing single-read capability flags do not promise conditional range-read support.

An empty range list performs no I/O when unconditional. A conditional empty list
still validates its condition on S3. Empty or out-of-bounds individual ranges still
require metadata and preserve the corresponding empty output buffers.

Request size and concurrency bound in-flight fetching, not the total result size:
all requested output buffers remain in memory. This API does not add async trait
methods or integrate Parquet decoding.

```rust
use fsspec_rs::{ByteRange, FileSystem, OperationError, RangeReadOptions};

fn chunks(fs: &dyn FileSystem, path: &str) -> Result<Vec<Vec<u8>>, OperationError> {
    let result = fs.read_ranges(path, &[
        ByteRange { start: Some(0), end: Some(16) },
        ByteRange { start: Some(32), end: Some(48) },
    ], RangeReadOptions::default())?;
    Ok(result.data)
}
```

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

S3 writes require explicit `commit()`. Dropping an unfinished S3 writer discards
its buffer and attempts to abort any multipart upload. This includes exclusive
and append handles; Drop never publishes S3 content. Foreign handles reached
through the C ABI also discard on Drop. Direct local append writes remain
immediate, and generic `BufferedFile` retains its configured autocommit behavior.

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

`S3Fs` and the registry's `s3` protocol come from the `s3` feature, which is on by
default. With `default-features = false` the crate does not depend on `object_store` or
Tokio and builds for `wasm32-unknown-unknown`:

```toml
[dependencies]
fsspec_rs = { version = "0.2", default-features = false }
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

Region precedence is explicit `region`, `AWS_REGION`, then `AWS_DEFAULT_REGION`;
without any value the backend uses `us-east-1`. When no explicit credential field
is supplied, static credentials come from `AWS_ACCESS_KEY_ID`,
`AWS_SECRET_ACCESS_KEY`, and optional `AWS_SESSION_TOKEN`. Key and secret are
required together. Explicit credentials never inherit an environment token.
Anonymous access ignores credential fields. Without static credentials, the
existing instance-metadata provider remains available. Profiles, SSO and
`credential_process` are not implemented. AWS endpoint environment variables
are not imported; custom endpoints require `endpoint_url`.

`S3Fs::with_client_options(config, S3ClientOptions)` accepts transport overrides;
`S3Fs::new(config)` retains defaults. `S3Config` fields are unchanged.

| Field             | Rust type          | Default    | Meaning                                                                            |
| ----------------- | ------------------ | ---------- | ---------------------------------------------------------------------------------- |
| `connect_timeout` | `Option<Duration>` | 5 seconds  | Connection timeout.                                                                |
| `request_timeout` | `Option<Duration>` | 30 seconds | Timeout for one HTTP request, including its body.                                  |
| `max_retries`     | `Option<usize>`    | 10         | Additional attempts for the general client, including reads and legacy operations. |

Omitted values use `object_store` defaults. Durations must be positive. Retries
retain the backend's existing backoff and retry-time budget; request timeout is
not a deadline for an operation containing several requests. Typed `write`
requests always use zero retries, including when `max_retries` is set. Uncertain
creates retain checksum recovery.

Environment defaults are captured when an instance is constructed. Registry
caches include explicit options, but not subsequent environment changes. Use
explicit credentials and region for independently configured clients, or
`FilesystemRegistry::url_to_fs_uncached` to capture updated environment values.

## Filesystem registry

`FilesystemRegistry` maps URL protocols to `FilesystemFactory` implementations,
the Rust counterpart of fsspec's `fsspec.specs` entry points. Rust has no runtime
plugin discovery, so a backend crate exports a factory and the application
registers it. Any `Fn(&str, &FsOptions) -> FsResult<SharedFileSystem>` closure is
a factory.

`url_to_fs(url, options)` returns a `SharedFileSystem` and the path to pass to it.
A URL without `protocol://` is a local path. Options are a
`FsOptions` (`BTreeMap<String, String>`). Chained URLs such as
`simplecache::s3://...` are not supported.

Filesystems are cached by protocol, options, and the factory's
`instance_scope` for the path, so equal calls share one instance.
`url_to_fs_uncached` always builds a new one. Registering a protocol again
replaces its factory and drops its cached instances.

`FilesystemRegistry::default()` registers:

| Protocol        | Filesystem                      | Options                                                                                                                                         |
| --------------- | ------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| `file`, `local` | `LocalFs`                       | `auto_mkdir`                                                                                                                                    |
| `memory`        | `MemoryFs`, one per process     | none                                                                                                                                            |
| `s3`            | `S3Fs`, one instance per bucket | `key`, `secret`, `token`, `endpoint_url`, `region`, `anon`, `virtual_hosted_style_request`, `connect_timeout`, `request_timeout`, `max_retries` |

Registry and C options encode timeouts as finite positive seconds and `max_retries`
as a nonnegative integer, all supplied as strings. Unknown options are rejected. Boolean options accept `true`, `false`, `1`, and `0`.
`FilesystemRegistry::empty()` starts with no protocols.

`DEFAULT_FILESYSTEMS` is a process-wide registry. The `register_filesystem` and
`url_to_fs` functions use it.

`FilesystemRegistry::url_to_fs_shared(&lock, url, options)` resolves through a
registry behind an `RwLock` and releases the lock while a factory runs, so a
factory may register other protocols in the same registry. The `url_to_fs`
function and `libfsspec` resolve this way.

## Object-store bridges

The optional `object-store` feature exports `object_store_bridge::FileSystemStore`
and `object_store_bridge::ObjectStoreFs`. It can be enabled without S3:
`default-features = false, features = ["object-store"]`. The default `s3` feature
also enables it. Neither adapter requires Python, registration, or a new trait.

| Backend implements               | Filesystem access | Object access     |
| -------------------------------- | ----------------- | ----------------- |
| `FileSystem` only                | Native            | `FileSystemStore` |
| `object_store::ObjectStore` only | `ObjectStoreFs`   | Native            |
| Both                             | Native            | Native            |

Construction is explicit. No blanket trait implementations, automatic discovery,
or adapter chaining are provided. A backend implementing both traits exposes its
native interfaces directly over the same data. The compiling example is
[`rust/examples/object_store_bridge.rs`](../../rust/examples/object_store_bridge.rs).

### Filesystem to object store

`FileSystemStore::from_snapshot(Arc<dyn FileSystem + Send + Sync>, root)` binds a
read-only object view to an **immutable** filesystem namespace. The caller must
keep names, metadata, and contents unchanged until the adapter and its streams
are dropped. The constructor does not create a snapshot or prevent other handles
from writing. Generic mutable-filesystem adaptation remains unsupported because
`FileSystem` cannot atomically return both metadata and bytes.

| ObjectStore operation                | Behavior                                                                                                               |
| ------------------------------------ | ---------------------------------------------------------------------------------------------------------------------- |
| `get_opts`                           | Read metadata and open a regular file within the immutable source; return lazy chunks. `head` avoids opening the body. |
| Read conditions                      | Evaluate ETag/time conditions against snapshot metadata. Version selection is unsupported.                             |
| `get_ranges`                         | Use the trait's coalescing implementation over range reads.                                                            |
| `list`                               | Walk directories lazily and yield regular-file metadata under a segment-aware prefix.                                  |
| `list_with_delimiter`                | Return immediate objects and prefixes containing objects; empty filesystem directories disappear.                      |
| Put, multipart, delete, copy, rename | Unsupported; no mutation reaches the source.                                                                           |

`FileInfo.modified` is required for object metadata. Missing timestamps and
reported special files return `NotSupported`; timestamps are never invented.
Directories, including the root, return `NotFound` for object reads and metadata.
`extra["etag"]` and `extra["version"]` are copied when supplied, without interpreting
ETags as checksums. Other attributes are not mapped. `ls` must return full paths
of immediate children. Slash-separated roots map to relative object keys without
percent decoding. Unrepresentable entries, including backslash names, fail the
listing with `NotSupported`; they are not silently skipped. Validation covers the
visited directory before prefix filtering, so an unrelated entry there can fail
a filtered listing. Delimiter listing currently traverses all descendants to
exclude empty directories; its cost grows with the entire selected subtree. The root is not a security sandbox: callers must exclude
symlinks that escape it or change the namespace during a read.

Filesystem work runs on a shared blocking executor with eight admitted calls.
Cancelling a future does not interrupt a running blocking call or release its
slot early. Body chunks are at most 64 KiB and are fetched only when polled.
The final chunk closes its file on the blocking worker. Dropping an unfinished
stream schedules closure on that same pool after any in-progress call completes;
it does not wait for closure. Cleanup jobs share the pool's eight-thread limit
but can queue independently of the eight-call admission limit. These limits
cover the bridge, not allocations inside a backend's `open` or `ls`:
listings retain one directory's entries plus pending directory names, and the
source may itself buffer entire files. Range collection allocates its output.

### Object store to filesystem

`ObjectStoreFs::new(Arc<dyn ObjectStore>, prefix)` binds relative filesystem paths
to one object-store prefix. The adapter owns a shared client handle, not its
credentials or a separate client. Root is `""`; paths are parsed without percent
encoding or decoding. URLs, absolute paths, backslashes and noncanonical paths
are rejected. Listings also fail with `NotSupported` when returned names cannot
be reopened through this mapping. No `object://` protocol is registered automatically.

| FileSystem operation                | Behavior                                                                                    |
| ----------------------------------- | ------------------------------------------------------------------------------------------- |
| `info`                              | Use native object metadata; infer directories from delimiter listings; root is a directory. |
| `ls`                                | Return objects and synthetic directories; reject object/prefix name collisions.             |
| `open(Read, None)`                  | Return a seekable handle after metadata checks, without downloading the body.               |
| Handle reads                        | Issue at most 64 KiB per request; use the opening version/ETag when available.              |
| Handle seeks                        | Support absolute, relative and end-relative offsets; seeking past EOF is allowed.           |
| Mutations and explicit open options | Unsupported.                                                                                |

Metadata and listings are required, including for collision checks on open. Empty
object-store prefixes have no persistent directory identity. Handles without a
version or ETag provide no snapshot guarantee; a size mismatch still fails.
A file handle retains its opening metadata. Reopening obtains current metadata.

Synchronous I/O uses a process-owned runtime with two worker threads and rejects
calls from any Tokio runtime context, including blocking tasks entered through
Tokio. Async callers use `ObjectStore` directly. This also rejects bridge stacking
before nested runtime execution. Closing an adapter does not shut down the shared
runtime or a caller-owned client. Other async runtimes cannot be detected; callers
must keep this synchronous interface off their event-loop threads.

Object-store missing, permission and unsupported errors map to filesystem error
categories. Other object errors retain their cause inside `FsError::IoError`.
Filesystem errors become corresponding object-store categories where available,
with their causes retained. File-handle errors preserve the I/O kind for missing,
permission-denied, invalid-input and unsupported failures. Object precondition
errors remain available as the cause of an `Other` I/O error. No bridge-specific
retry or deadline policy is added.

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

Except for typed object I/O, fallible filesystem methods return
`FsResult<T> = Result<T, FsError>`. `FsError` represents
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
