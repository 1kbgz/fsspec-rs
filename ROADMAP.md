# fsspec-rs Roadmap

## Project Summary

**fsspec-rs** aims to provide a Rust-native filesystem abstraction framework inspired by Python's [filesystem_spec (fsspec)](https://filesystem-spec.readthedocs.io/en/latest/), along with Python bindings (via PyO3) that are fully compatible with the real `fsspec.spec.AbstractFileSystem` base class. The end result is:

1. **Pure Rust traits and structs** — a standalone `fsspec`-like framework usable from any Rust project, with no Python dependency.
2. **Python bindings** — PyO3-based wrappers that fuse the fast Rust implementations with actual `fsspec` base classes, so they participate in fsspec's registry, caching, transaction system, and are drop-in replacements for existing Python fsspec backends.
3. **Concrete backends** — starting with Local Filesystem and S3, each available as both a pure-Rust trait implementation and a full fsspec-compatible Python class.

---

## Current State

| Component | Status |
|---|---|
| **Build infrastructure** | **Complete.** Workspace Cargo.toml (cdylib via PyO3 0.28), `rust/Cargo.toml` (pure rlib), `pyproject.toml` (hatchling + hatch-rs), Makefiles, cibuildwheel, bumpversion, ruff, clippy, coverage — all wired up. |
| **Rust core** (`rust/src/`) | **Complete.** `FileSystem` trait (8 primitives + 20 default methods), `AsyncFileSystem` trait (async mirror), `FsFile` trait, `LocalFs` + `LocalFile`, `S3Fs` + `S3File` implementations. `FsError` enum, `FileType`, `FileInfo`, `OpenMode`, `OpenOptions`, `WalkEntry`, `DuResult`. 137 unit + 20 S3 integration tests passing. |
| **PyO3 bridge** (`rust/python/`) | **Complete.** `PyFileType`, `PyFileInfo`, `RustLocalFs`, `RustLocalFile`, `RustS3Fs`, `RustS3File` pyclasses. Error-to-exception conversion. |
| **Python package** (`fsspec_rs/`) | **Complete.** `LocalFileSystem` with `protocol = ("file-rs", "local-rs")`, `S3FileSystem` with `protocol = ("s3-rs",)`, both with file wrappers. 97 Python tests passing including isinstance enforcement. |
| **Tests** | **Complete.** 137 Rust unit tests + 20 S3 integration tests + 97 Python tests = 254 total. All passing. |

---

## Research Findings: fsspec Architecture

### Core Abstractions

fsspec is built around a small number of abstractions with a "template method" design: backends implement a handful of primitive operations, and the base class builds higher-level operations on top.

#### `AbstractFileSystem` (sync base class)

**Primitives that backends MUST implement:**

| Method | Signature | Purpose |
|---|---|---|
| `ls` | `(path, detail=True) → list[dict] \| list[str]` | List directory contents. Each dict has `name`, `size`, `type`. |
| `rm_file` | `(path)` | Delete a single file. |
| `cp_file` | `(path1, path2)` | Copy a single file within the filesystem. |

**Primitives that backends SHOULD implement:**

| Method | Signature | Purpose |
|---|---|---|
| `_open` | `(path, mode, block_size, ...) → file-like` | Return a file-like object. |
| `info` | `(path) → dict` | Return metadata dict (`name`, `size`, `type`, ...). Default: calls `ls(parent)`. |
| `mkdir` / `makedirs` | `(path, ...)` | Create directories. Default: no-op. |
| `rmdir` | `(path)` | Remove directory. Default: no-op. |
| `_strip_protocol` | `(cls, path) → str` | Normalize a URL to a bare path. |
| `created` / `modified` | `(path) → datetime` | Timestamps. Default: raises `NotImplementedError`. |

**Concrete methods provided by the base (built on primitives):**

- **Traversal:** `walk`, `find`, `glob`, `expand_path`, `du`, `tree`
- **Reading:** `cat_file`, `cat`, `cat_ranges`, `head`, `tail`, `read_text`, `read_block`
- **Writing:** `pipe_file`, `pipe`, `write_text`, `touch`
- **Copy/Move:** `copy`, `mv`, `get_file`, `get`, `put_file`, `put`
- **Delete:** `rm` (expands path, calls `rm_file` in reverse order)
- **Info:** `exists`, `isdir`, `isfile`, `size`, `sizes`, `checksum`
- **Mapping:** `get_mapper` → `FSMap`
- **Opening:** `open` (wraps `_open` + compression + text mode + transactions)
- **Serialization:** `to_json` / `from_json`, pickle support
- **Aliases:** `cp`→`copy`, `mv`→`move`, `stat`→`info`, `listdir`→`ls`, etc.

#### `AsyncFileSystem(AbstractFileSystem)` (async base class)

Sets `async_impl = True`, `mirror_sync_methods = True`. Backends implement async versions of the primitives (`_ls`, `_cat_file`, `_get_file`, `_put_file`, `_rm_file`, `_cp_file`, `_pipe_file`, `_info`). Sync wrappers are auto-generated via `mirror_sync_methods()` which dispatches through a background `asyncio` event loop.

#### `AbstractBufferedFile(io.IOBase)` (file objects)

Returned by `_open()`. Provides buffered read (with pluggable cache strategy) and buffered write (with multipart upload). Backends override:
- `_fetch_range(start, end)` — fetch remote bytes
- `_upload_chunk(final)` — upload a write buffer chunk
- `_initiate_upload()` — begin a multipart upload session

#### Supporting Systems

- **Registry:** Protocol → class mapping with lazy imports. `known_implementations` dict + `fsspec.specs` entry-point discovery.
- **Instance caching:** `_Cached` metaclass deduplicates filesystem instances by tokenized constructor args.
- **Read caching:** Pluggable strategies (`readahead`, `blockcache`, `bytes`, `all`, `mmap`, `background`, etc.) in `AbstractBufferedFile`.
- **Directory cache:** `DirCache` (time-expiring `MutableMapping`) in every filesystem instance.
- **Transactions:** `Transaction` context manager batches file commits/discards.
- **Callbacks:** `Callback` with branching for nested progress tracking.

### Reference Implementations

- **`LocalFileSystem`** — sync, wraps `os.*` / `shutil.*`, returns `LocalFileOpener` (thin wrapper around `builtins.open`).
- **`MemoryFileSystem`** — sync, dict-backed, returns `MemoryFile` (BytesIO subclass).
- **`S3FileSystem`** (in s3fs) — async-native via `AsyncFileSystem`, uses `aiobotocore` for all S3 API calls, returns `S3File(AbstractBufferedFile)` for buffered I/O and `S3AsyncStreamedFile` for streaming.

---

## Architecture Design

### Layer Diagram

```
┌─────────────────────────────────────────────────────────────────────┐
│                     Python User Code                                │
│         from fsspec_rs import LocalFileSystem, S3FileSystem         │
│                    (or via fsspec registry)                         │
├─────────────────────────────────────────────────────────────────────┤
│                   Python Binding Layer (fsspec_rs/)                  │
│                                                                     │
│   LocalFileSystem(AbstractFileSystem)   S3FileSystem(AsyncFS)       │
│   ┌─────────────────────────────┐   ┌───────────────────────────┐  │
│   │  Inherits real fsspec base  │   │ Inherits real fsspec base │  │
│   │  Delegates primitives to    │   │ Delegates _async methods  │  │
│   │  Rust via PyO3 calls        │   │ to Rust via PyO3 calls    │  │
│   └─────────────────────────────┘   └───────────────────────────┘  │
├─────────────────────────────────────────────────────────────────────┤
│                   PyO3 Bridge (rust/python/)                        │
│                                                                     │
│   #[pyclass] RustLocalFs        #[pyclass] RustS3Fs                │
│   Exposes trait methods as      Exposes async trait methods as     │
│   Python-callable functions     Python-callable functions          │
├─────────────────────────────────────────────────────────────────────┤
│                   Pure Rust Library (rust/src/)                      │
│                                                                     │
│   trait FileSystem {             trait AsyncFileSystem {            │
│     fn ls(...)                     async fn ls(...)                 │
│     fn rm_file(...)                async fn rm_file(...)            │
│     fn cp_file(...)                async fn cp_file(...)            │
│     fn open(...)                   async fn open(...)               │
│     fn info(...)                   async fn info(...)               │
│     fn mkdir(...)                  ...                              │
│     ...                          }                                  │
│   }                                                                 │
│                                                                     │
│   // Concrete methods via extension traits / default impls:         │
│   fn cat_file(...)   built on open()                               │
│   fn walk(...)       built on ls()                                  │
│   fn find(...)       built on walk()                                │
│   fn copy(...)       built on cp_file()                             │
│   fn rm(...)         built on rm_file()                             │
│   fn get/put(...)    built on open()                                │
│   ...                                                               │
│                                                                     │
│   struct LocalFs { ... }          impl FileSystem for LocalFs      │
│   struct S3Fs { ... }             impl AsyncFileSystem for S3Fs    │
└─────────────────────────────────────────────────────────────────────┘
```

### Pure Rust Design (`rust/src/`)

The Rust library mirrors fsspec's template-method pattern using **traits with default method implementations**:

```rust
// Simplified sketch
pub trait FileSystem {
    fn protocol(&self) -> &[&str];
    fn root_marker(&self) -> &str { "" }
    fn sep(&self) -> &str { "/" }

    // --- Primitives (backends must implement) ---
    fn ls(&self, path: &str, detail: bool) -> Result<Vec<FileInfo>>;
    fn rm_file(&self, path: &str) -> Result<()>;
    fn cp_file(&self, src: &str, dst: &str) -> Result<()>;
    fn open(&self, path: &str, mode: OpenMode, opts: &OpenOptions) -> Result<Box<dyn FsFile>>;
    fn info(&self, path: &str) -> Result<FileInfo>;
    fn mkdir(&self, path: &str, create_parents: bool) -> Result<()>;
    fn rmdir(&self, path: &str) -> Result<()>;
    fn strip_protocol(&self, path: &str) -> String;

    // --- Concrete methods (default impls built on primitives) ---
    fn exists(&self, path: &str) -> Result<bool> { ... }
    fn isdir(&self, path: &str) -> Result<bool> { ... }
    fn isfile(&self, path: &str) -> Result<bool> { ... }
    fn size(&self, path: &str) -> Result<u64> { ... }
    fn cat_file(&self, path: &str, start: Option<i64>, end: Option<i64>) -> Result<Vec<u8>> { ... }
    fn pipe_file(&self, path: &str, data: &[u8]) -> Result<()> { ... }
    fn head(&self, path: &str, size: usize) -> Result<Vec<u8>> { ... }
    fn tail(&self, path: &str, size: usize) -> Result<Vec<u8>> { ... }
    fn walk(&self, path: &str, max_depth: Option<usize>) -> Result<Vec<WalkEntry>> { ... }
    fn find(&self, path: &str, max_depth: Option<usize>, with_dirs: bool) -> Result<Vec<String>> { ... }
    fn glob(&self, pattern: &str, max_depth: Option<usize>) -> Result<Vec<String>> { ... }
    fn copy(&self, src: &str, dst: &str, recursive: bool) -> Result<()> { ... }
    fn mv(&self, src: &str, dst: &str) -> Result<()> { ... }
    fn rm(&self, path: &str, recursive: bool) -> Result<()> { ... }
    fn get_file(&self, remote: &str, local: &str) -> Result<()> { ... }
    fn put_file(&self, local: &str, remote: &str) -> Result<()> { ... }
    fn touch(&self, path: &str, truncate: bool) -> Result<()> { ... }
    fn du(&self, path: &str, total: bool) -> Result<DuResult> { ... }
    // ... etc.
}
```

An analogous `AsyncFileSystem` trait will use `async fn` (via `async-trait` or Rust's native async-in-traits), with the same template-method pattern.

A `FsFile` trait mirrors `AbstractBufferedFile`:

```rust
pub trait FsFile: Read + Write + Seek {
    fn info(&self) -> Result<FileInfo>;
    fn flush_upload(&mut self) -> Result<()>;
    fn commit(&mut self) -> Result<()>;
    fn discard(&mut self) -> Result<()>;
}
```

### PyO3 Bridge Design (`rust/python/`)

Each Rust backend gets a `#[pyclass]` wrapper that:
1. Holds the Rust struct (e.g., `LocalFs`, `S3Fs`).
2. Exposes each trait method as a `#[pymethods]` function.
3. Converts between Rust types and Python types (e.g., `FileInfo` ↔ `dict`, `Vec<u8>` ↔ `bytes`).

### Python Integration Design (`fsspec_rs/`)

Each backend has a Python class that:
1. **Inherits** from the real `fsspec.spec.AbstractFileSystem` (or `fsspec.asyn.AsyncFileSystem`).
2. **Holds** an instance of the PyO3-exposed Rust struct.
3. **Overrides** the fsspec primitives (`ls`, `info`, `_open`, `rm_file`, `cp_file`, etc.) to delegate to the Rust backend.
4. **Registers** itself with fsspec's registry so it can be discovered by protocol name.
5. Inherits all concrete methods (`walk`, `find`, `glob`, `cat`, `get`, `put`, `copy`, `rm`, etc.) from the fsspec base class for free — OR optionally overrides them to call Rust-accelerated versions.

Example (simplified):

```python
import fsspec
from fsspec_rs._native import RustLocalFs  # PyO3 class

class LocalFileSystem(fsspec.AbstractFileSystem):
    protocol = ("file-rs", "local-rs")  # distinct from built-in "file"

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self._rust = RustLocalFs()

    def ls(self, path, detail=True, **kwargs):
        return self._rust.ls(path, detail)

    def info(self, path, **kwargs):
        return self._rust.info(path)

    def _open(self, path, mode="rb", block_size=None, **kwargs):
        return self._rust.open(path, mode, block_size)

    def rm_file(self, path):
        self._rust.rm_file(path)

    def cp_file(self, path1, path2, **kwargs):
        self._rust.cp_file(path1, path2)

    def mkdir(self, path, create_parents=True, **kwargs):
        self._rust.mkdir(path, create_parents)

    def rmdir(self, path):
        self._rust.rmdir(path)
```

---

## Phased Roadmap

### Phase 0: Foundation & Core Types ✦ _Milestone: compilable trait with no backends_ ✅

**Rust (`rust/src/`)**

- [x] **0.1** Define core data types: `FileInfo` struct (name, size, file_type, created, modified, mode, etc.), `FileType` enum (File, Directory, Other), `OpenMode` enum (Read, Write, Append, Exclusive), `OpenOptions` struct, `WalkEntry`, `DuResult`
- [x] **0.2** Define error types: `FsError` enum (NotFound, PermissionDenied, AlreadyExists, NotADirectory, IsADirectory, IoError, Other), `Result<T>` alias
- [x] **0.3** Define `FsFile` trait: `Read + Write + Seek + info() + commit() + discard()`
- [x] **0.4** Define `FileSystem` trait with all primitive methods (abstract) and all concrete methods (default impls)
- [x] **0.5** Define `AsyncFileSystem` trait (async mirror of `FileSystem`) — iterative stack-based walk to avoid recursive-async issues
- [x] **0.6** Unit tests for default method logic using a mock/stub `FileSystem` implementation (67 tests)

**PyO3 (`rust/python/`)**

- [x] **0.7** Remove `Example` scaffolding
- [x] **0.8** Set up PyO3 wrappers for `FileInfo`, `FileType`, `FsError` → Python `dict` / exception conversion utilities

**Python (`fsspec_rs/`)**

- [x] **0.9** Remove stub code
- [x] **0.10** Add `fsspec` as a dependency (for the Python binding layer; not required for pure-Rust usage)

### Phase 1: Local Filesystem Backend ✦ _Milestone: working local FS in Rust and Python_ ✅

**Rust (`rust/src/`)**

- [x] **1.1** Implement `LocalFs` struct
- [x] **1.2** Implement `FileSystem for LocalFs`: `ls` (via `std::fs::read_dir`), `info` (via `std::fs::metadata`), `rm_file`, `cp_file` (via `std::fs::copy`), `mkdir`, `makedirs`, `rmdir`, `_strip_protocol`
- [x] **1.3** Implement `LocalFile` struct implementing `FsFile` (wraps `std::fs::File`)
- [x] **1.4** Implement `_open` for `LocalFs` → returns `LocalFile`
- [x] **1.5** Tests: ls, info, read/write files, mkdir, rm, cp, mv, walk, find, cat, pipe, get_file, put_file (70 tests)

**PyO3 (`rust/python/`)**

- [x] **1.6** Create `#[pyclass] RustLocalFs` wrapping `LocalFs`, exposing all trait methods
- [x] **1.7** Create `#[pyclass] RustLocalFile` wrapping `LocalFile`, implementing Python file protocol (`read`, `write`, `seek`, `tell`, `close`, `__enter__`, `__exit__`)
- [x] **1.8** Register in the PyO3 module

**Python (`fsspec_rs/`)**

- [x] **1.9** Create `fsspec_rs.local` module with `LocalFileSystem(fsspec.AbstractFileSystem)` class delegating to `RustLocalFs`
- [x] **1.10** Create `fsspec_rs.local.LocalFile(AbstractBufferedFile)` wrapping `RustLocalFile` as a Python file-like object
- [x] **1.11** Register with fsspec: protocol = ("file-rs", "local-rs")
- [x] **1.12** Tests: all fsspec-compatible operations with isinstance enforcement (47 tests)
- [ ] **1.13** Benchmark: compare `fsspec_rs.LocalFileSystem` vs `fsspec.implementations.local.LocalFileSystem` for `ls`, `find`, `walk`, `cat`, batch `get`/`put`

### Phase 2: S3 Backend (via `object_store` crate) ✦ _Milestone: working S3 FS in Rust and Python_ ✅

The [`object_store`](https://crates.io/crates/object_store) crate (from the Apache Arrow ecosystem) provides a unified, production-grade Rust interface for S3, GCS, Azure Blob, and local storage. Rather than implementing raw S3 API calls with `aws-sdk-s3`, we built our S3 backend as an adapter over `object_store`, gaining:

- Battle-tested S3 client with retries, multipart upload, streaming, and conditional requests
- Credential resolution via the standard AWS chain (env vars, config files, IMDS, ECS)
- Built-in support for GCS and Azure with the same interface (enabling easy Phase 5 backends)
- Active maintenance by the Arrow/DataFusion community

**Rust (`rust/src/`)**

- [x] **2.1** Add `object_store` (with `aws` feature), `tokio`, `bytes`, and `url` dependencies to `rust/Cargo.toml`
- [x] **2.2** Implement `S3Fs` struct wrapping `AmazonS3` from `object_store`, with embedded tokio runtime for sync API
- [x] **2.3** Implement `FileSystem for S3Fs`: `ls` (list_with_delimiter), `info` (HEAD + fallback listing), `rm_file`, `cp_file`, `mkdir`/`rmdir` (no-ops), `open`, `strip_protocol`, `unstrip_protocol`
- [x] **2.4** Map `object_store::Error` variants → `FsError` variants
- [x] **2.5** Implement `S3File` struct implementing `FsFile` using in-memory `Cursor<Vec<u8>>` buffering with upload-on-drop
- [x] **2.6** Expose S3-specific configuration via `S3Config`: region, endpoint URL, anonymous access, virtual-hosted style, credentials
- [x] **2.7** Tests: 20 integration tests against Backblaze B2 (all passing with `#[ignore]` + env var gating)

**PyO3 (`rust/python/`)**

- [x] **2.8** Create `#[pyclass] RustS3Fs` wrapping `S3Fs`, exposing all trait methods with `client_kwargs` support
- [x] **2.9** Create `#[pyclass] RustS3File` wrapping `S3File` with `Mutex<Option<Box<dyn FsFile>>>` pattern
- [x] **2.10** Register in the PyO3 module

**Python (`fsspec_rs/`)**

- [x] **2.11** Create `fsspec_rs.s3` module with `S3FileSystem(fsspec.AbstractFileSystem)` class delegating to `RustS3Fs`
- [x] **2.12** Handle credential passthrough: merge from `fsspec.config.conf["s3"]` (populated by `FSSPEC_S3_*` env vars), accept same kwargs as s3fs (`key`, `secret`, `token`, `anon`, `endpoint_url`, `client_kwargs`)
- [x] **2.13** Register with fsspec: protocol = `("s3-rs",)`
- [x] **2.14** Tests: 26 Python tests with isinstance enforcement against Backblaze B2 (all passing, skipped when no credentials)
- [ ] **2.15** Benchmark: compare `fsspec_rs.S3FileSystem` vs `s3fs.S3FileSystem` for ls, cat, get, put, find

### Phase 3: Buffered File & Caching ✦ _Milestone: feature parity with fsspec file objects_

- [ ] **3.1** Implement Rust `BufferedFile` struct with pluggable read cache (readahead, block, all-bytes)
- [ ] **3.2** Implement Rust write buffering with configurable block size and auto-flush
- [ ] **3.3** Expose cache strategies to Python via configuration
- [ ] **3.4** Integrate with fsspec's `AbstractBufferedFile` for Python-side compatibility
- [ ] **3.5** Tests and benchmarks for buffered read/write patterns

### Phase 4: Advanced Features ✦ _Milestone: production-ready_

- [ ] **4.1** Directory listing cache (`DirCache` equivalent in Rust, with TTL and LRU eviction)
- [ ] **4.2** Transaction support (Rust-side `Transaction` struct, Python integration with `fsspec.Transaction`)
- [ ] **4.3** Callback/progress support (Rust callback trait, Python integration with `fsspec.Callback`)
- [ ] **4.4** `FSMap` / `get_mapper()` support (likely just works via fsspec base class, but verify)
- [ ] **4.5** Signed URL support for S3 (presigned GET/PUT)
- [ ] **4.6** Multipart concurrent transfers for S3 (parallel chunk upload/download)
- [ ] **4.7** Entry-point registration so `fsspec_rs` backends are auto-discovered by fsspec

### Phase 5: Additional Backends (Future) ✦ _Milestone: expanded ecosystem_

- [ ] **5.1** Memory filesystem (Rust `MemoryFs` + Python binding) — useful for testing
- [ ] **5.2** HTTP/HTTPS filesystem (Rust `HttpFs` via `reqwest` + Python binding)
- [ ] **5.3** GCS filesystem — reuse `ObjectStoreFs` adapter with `object_store::gcp::GoogleCloudStorage` + Python binding
- [ ] **5.4** Azure Blob filesystem — reuse `ObjectStoreFs` adapter with `object_store::azure::MicrosoftAzure` + Python binding
- [ ] **5.5** SFTP filesystem (Rust `SftpFs` via `ssh2` crate + Python binding)
- [ ] **5.6** Chained filesystem (Rust `ChainedFs` + Python `ChainedFileSystem(fsspec.AbstractFileSystem)`) — layered/caching filesystem composition (analogous to `fsspec.implementations.chained.CachingFileSystem`). Allows wrapping one filesystem with another (e.g., adding a local disk cache in front of an S3 backend). The Rust implementation will hold a `Vec<Box<dyn FileSystem>>` chain and delegate operations through the layers.

---

## Key Design Decisions

### 1. Trait vs. Struct Dispatch

Use **trait objects** (`Box<dyn FileSystem>`) for runtime polymorphism in the Rust library, mirroring fsspec's class hierarchy. This allows generic code to operate on any filesystem backend. For performance-critical paths, also support **static dispatch** via generics (`fn do_thing<F: FileSystem>(fs: &F)`).

### 2. Async Runtime

Use **tokio** as the async runtime in Rust. For the sync `FileSystem` trait, provide a `block_on()` helper that runs async operations synchronously (similar to fsspec's `sync()` function). For the PyO3 bridge, use `pyo3-asyncio-0.21` (or newer) to expose Rust futures as Python awaitables.

### 3. Error Handling

Define a unified `FsError` enum in Rust that maps cleanly to both:
- Rust's `std::io::Error` (for interop with std)
- Python's exception hierarchy (`FileNotFoundError`, `PermissionError`, `FileExistsError`, `IsADirectoryError`, `NotADirectoryError`, `OSError`)

### 4. Python Compatibility Strategy

The Python classes inherit from real fsspec base classes and delegate primitives to Rust. This means:
- All of fsspec's concrete methods (walk, find, glob, cat, get, put, etc.) work out of the box via the base class.
- fsspec's registry, caching, transaction, callback, and serialization systems all work.
- Users can pass `fsspec_rs` filesystems anywhere fsspec filesystems are accepted (pandas, dask, xarray, etc.).
- Over time, we can selectively override concrete methods (e.g., `find`, `walk`) to call Rust-accelerated versions if the Python overhead of per-file `ls` calls becomes a bottleneck.

### 5. Protocol Naming

Use distinct protocol names (e.g., `"file-rs"`, `"s3-rs"`) to avoid conflicting with the built-in fsspec implementations. Users can opt-in to the Rust-accelerated versions explicitly. A future option could allow overriding the default protocols.

### 6. Feature Flags

Use Cargo feature flags to make backends optional:
- `default = ["local"]`
- `s3` — enables S3 backend (pulls in `object_store` with `aws` feature, `tokio`)
- `gcs` — enables GCS backend (pulls in `object_store` with `gcp` feature)
- `azure` — enables Azure backend (pulls in `object_store` with `azure` feature)
- `all` — enables all backends

The PyO3 cdylib always compiles with all features enabled.

### 7. The `object_store` Crate Strategy

Rather than implementing raw cloud API calls, we use the [`object_store`](https://crates.io/crates/object_store) crate from the Apache Arrow ecosystem as the underlying cloud storage client. This gives us:
- Production-grade S3/GCS/Azure clients with retries, multipart, streaming
- A single `ObjectStore` trait that we adapt to our `FileSystem` trait
- Easy addition of new cloud backends (GCS, Azure) by swapping the `ObjectStore` implementation
- Active maintenance by the Arrow/DataFusion community

The `ObjectStoreFs` adapter struct bridges `object_store::ObjectStore` → our `FileSystem` trait, translating path types, error types, and listing formats.

---

## Dependencies

### Rust

| Crate | Purpose | Phase |
|---|---|---|
| `pyo3` (0.28+) | Python bindings | 0 |
| `object_store` | Cloud storage client (S3, GCS, Azure) | 2 |
| `tokio` | Async runtime (for object_store) | 2 |
| `bytes` | Byte buffer types (for object_store interop) | 2 |
| `glob` or `globset` | Glob pattern matching | 1 |
| `chrono` | Timestamps | 0 |
| `serde` / `serde_json` | Serialization (for config, FileInfo) | 0 |

### Python

| Package | Purpose | Phase |
|---|---|---|
| `fsspec` | Base classes for Python binding layer | 0 |
| `pytest` | Testing | 0 |
| `minio` / `localstack` | S3 integration testing | 2 |

---

## Testing Strategy

1. **Rust unit tests** (`cargo test`): test each trait method's default impl with mock filesystems, test each backend's primitives.
2. **Rust integration tests**: test LocalFs against real filesystem, test S3Fs against MinIO.
3. **Python unit tests** (`pytest`): test each Python class's fsspec compatibility, ensuring all inherited methods work.
4. **Python integration tests**: test against real/mock services, test interop with pandas/dask/xarray.
5. **Benchmarks**: comparative benchmarks of fsspec_rs vs. pure-Python fsspec for key operations (ls, find, walk, cat, get, put) on both local and S3.

---

## Success Criteria

- [ ] `fsspec_rs.LocalFileSystem` passes fsspec's own test suite patterns for local filesystem operations
- [ ] `fsspec_rs.S3FileSystem` can be used as a drop-in replacement for `s3fs.S3FileSystem` in common workflows
- [ ] Measurable speedup (>2x) for filesystem-traversal-heavy operations (find, walk, glob) on local filesystem
- [ ] Measurable speedup for S3 batch operations (cat many files, list large directories)
- [ ] Pure-Rust `LocalFs` and `S3Fs` usable from Rust projects with no Python dependency
- [ ] Clean `cargo doc` documentation for the Rust library
- [ ] Published to PyPI (`fsspec-rs`) and crates.io (`fsspec_rs`)
