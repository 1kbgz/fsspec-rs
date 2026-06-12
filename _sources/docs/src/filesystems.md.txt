# fsspec-rs filesystems

`fsspec-rs` exposes Rust-backed filesystems through normal fsspec classes.
Importing `fsspec_rs` registers the protocols with fsspec:

| Filesystem       | Protocols             | Class                       |
| ---------------- | --------------------- | --------------------------- |
| Local filesystem | `file-rs`, `local-rs` | `fsspec_rs.LocalFileSystem` |
| Amazon S3        | `s3-rs`               | `fsspec_rs.S3FileSystem`    |

Use these protocols when you want the Rust-backed implementation without
changing the standard `file`, `local`, or `s3` protocol names owned by fsspec
and s3fs.

```python
import fsspec
import fsspec_rs

with fsspec.open("file-rs:///tmp/example.txt", "rb") as f:
    data = f.read()
```

## Local filesystem

`LocalFileSystem` wraps the Rust `LocalFs` backend, which uses `std::fs` for
local paths. It is marked as a local filesystem (`local_file = True`) and
supports the common fsspec primitives.

```python
from fsspec_rs import LocalFileSystem

fs = LocalFileSystem(auto_mkdir=True)
fs.pipe_file("/tmp/fsspec-rs/example.txt", b"hello")

assert fs.exists("/tmp/fsspec-rs/example.txt")
assert fs.isfile("/tmp/fsspec-rs/example.txt")
print(fs.info("/tmp/fsspec-rs/example.txt"))
```

### Paths

Direct method calls use normal local paths:

```python
fs.cat_file("/tmp/fsspec-rs/example.txt")
```

fsspec URLs use `file-rs://` or `local-rs://`:

```python
with fsspec.open("local-rs:///tmp/fsspec-rs/example.txt", "rb") as f:
    print(f.read())
```

`LocalFileSystem._strip_protocol()` removes only the Rust-specific local
protocols. The regular `file://` and `local://` protocols remain owned by
fsspec's Python implementation.

### Opening files

Supported modes are `rb`, `wb`, `ab`, and `xb`, plus their textless aliases
`r`, `w`, `a`, and `x` at the Rust layer. Python wrappers expose binary file
objects that implement `read`, `write`, `seek`, `tell`, `flush`, `close`,
`readable`, `writable`, and `seekable`.

```python
with fs.open("/tmp/fsspec-rs/out.bin", "wb") as f:
    f.write(b"payload")

with fs.open("/tmp/fsspec-rs/out.bin", "rb") as f:
    f.seek(2)
    print(f.read())
```

`auto_mkdir=True` creates missing parent directories for write, append,
exclusive-create, and copy operations. With the default `auto_mkdir=False`,
missing parents are reported by the operating system.

### Supported operations

Local filesystem methods delegated to Rust include:

- Listing and metadata: `ls`, `info`, `exists`, `isdir`, `isfile`, `size`
- Reads and writes: `open`, `cat_file`, `pipe_file`, `head`, `tail`,
  `read_text`, `write_text`, `touch`
- Directory operations: `mkdir`, `makedirs`, `rmdir`, `walk`, `find`, `du`
- Mutation and transfer: `rm_file`, `rm`, `cp_file`, `copy`, `mv`, `get_file`,
  `put_file`

The `cache_type`, `block_size`, and `max_blocks` options are accepted by
`open()` for API compatibility. Local reads are already direct local file
operations, so those cache options do not change local read behavior.

## S3

`S3FileSystem` wraps the Rust `S3Fs` backend, which uses Apache Arrow's
`object_store` crate and runs its async S3 calls through an embedded Tokio
runtime. The Python protocol is `s3-rs`, so it can coexist with `s3fs`.

```python
from fsspec_rs import S3FileSystem

fs = S3FileSystem(bucket="my-bucket", region="us-east-1")
fs.pipe_file("reports/hello.txt", b"hello")
print(fs.cat_file("reports/hello.txt"))
```

### Paths

`S3FileSystem` is constructed for one bucket. Method calls can use keys within
that bucket:

```python
fs.ls("reports")
fs.info("reports/hello.txt")
```

fsspec URLs include the bucket and can infer constructor options through
`_get_kwargs_from_urls()`:

```python
with fsspec.open(
    "s3-rs://my-bucket/reports/hello.txt",
    "rb",
    region="us-east-1",
) as f:
    print(f.read())
```

Internally, object names are normalized without a leading slash. Directory
entries are S3 prefixes, not real directories; `mkdir()` and `rmdir()` are
no-ops.

### Credentials and endpoints

Credential resolution follows this order:

1. Explicit constructor arguments: `key`, `secret`, `token`, `region`,
   `endpoint_url`, and `anon`
1. `fsspec.config.conf["s3"]`, including values populated from fsspec config
   environment variables
1. `object_store`'s own AWS credential chain, such as environment variables,
   profiles, and instance or task roles

Use `client_kwargs={"endpoint_url": ...}` when porting code from s3fs. If the
endpoint starts with `http://`, the Rust backend allows plain HTTP
automatically for local S3-compatible services.

```python
fs = S3FileSystem(
    bucket="benchmark",
    key="minioadmin",
    secret="minioadmin",
    endpoint_url="http://localhost:9000",
    region="us-east-1",
)
```

Anonymous access is available with `anon=True`.

### Caching reads

S3 reads support optional Rust-side cache strategies through `cache_type`:

| `cache_type` | Behavior                                        |
| ------------ | ----------------------------------------------- |
| `none`       | Fetch each requested byte range directly.       |
| `readahead`  | Keep one lookahead window for sequential reads. |
| `block`      | Cache fixed-size blocks with LRU eviction.      |
| `all`        | Fetch the whole object on first read.           |

```python
with fs.open(
    "large/table.parquet",
    "rb",
    cache_type="block",
    block_size=8 * 1024 * 1024,
) as f:
    header = f.read(4096)
```

`block_size` defaults to 4 MiB. `max_blocks` defaults to 32 and only affects
`cache_type="block"`.

Without an explicit `cache_type`, S3 read mode eagerly loads the whole object
into memory. Use `readahead`, `block`, or `none` when reading selected ranges
from large objects.

### Supported operations

S3 methods delegated to Rust include:

- Listing and metadata: `ls`, `info`, `exists`, `isdir`, `isfile`, `size`
- Reads and writes: `open`, `cat_file`, `pipe_file`, `head`, `tail`,
  `read_text`
- Prefix traversal: `walk`, `find`
- Object mutation: `rm_file`, `rm`, `cp_file`

Current S3 writes buffer object contents in memory before upload. Multipart
concurrent uploads and signed URLs are not yet part of the public API.
