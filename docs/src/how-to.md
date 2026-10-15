# How to use Rust-backed filesystems

Use these recipes when replacing a local or S3 filesystem with its `fsspec-rs`
equivalent.

## Select a backend by protocol

Construct filesystems through fsspec when an application accepts a protocol:

```python
import fsspec
import fsspec_rs

local = fsspec.filesystem("file-rs", auto_mkdir=True)
s3 = fsspec.filesystem("s3-rs", bucket="reports", region="us-east-1")
```

Use direct classes when an application accepts an `AbstractFileSystem` instance:

```python
from fsspec_rs import LocalFileSystem, S3FileSystem

local = LocalFileSystem(auto_mkdir=True)
s3 = S3FileSystem(bucket="reports", region="us-east-1")
```

## Use fsspec URLs

Import `fsspec_rs` before resolving a protocol in environments that do not discover
package entry points automatically:

```python
import fsspec
import fsspec_rs

with fsspec.open("file-rs:///tmp/report.txt", "wb") as file:
    file.write(b"complete\n")

with fsspec.open(
    "s3-rs://reports/2026/summary.txt",
    "rb",
    region="us-east-1",
) as file:
    summary = file.read()
```

Keep `file`, `local`, and `s3` URLs when you want the existing Python implementations.
Use the `-rs` protocols to select the Rust-backed implementations explicitly.

## Configure an S3-compatible service

Pass the endpoint and credentials accepted by `S3FileSystem`:

```python
from fsspec_rs import S3FileSystem

fs = S3FileSystem(
    bucket="warehouse",
    endpoint_url="http://localhost:9000",
    region="us-east-1",
    key="minioadmin",
    secret="minioadmin",
)
```

Use `anon=True` for anonymous access. For AWS, omit explicit credentials to use the
credential chain provided by the underlying object-store implementation.

## Bound S3 read caching

Choose an explicit cache strategy when opening large objects. Omitting
`cache_type` eagerly reads the whole S3 object:

```python
with fs.open(
    "tables/orders.parquet",
    "rb",
    cache_type="block",
    block_size=8 * 1024 * 1024,
) as file:
    header = file.read(64 * 1024)
```

The Python wrapper uses the Rust default of 32 cached blocks; it does not forward
`max_blocks`. Use `none` for direct range fetches, `readahead` for sequential access, `block` for
repeated random access, or `all` to fetch the complete object on first read. See the
[filesystem reference](filesystems.md) for exact behavior and supported operations.
