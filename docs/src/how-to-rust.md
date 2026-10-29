# How to use the Rust crates

## Add the native crate

Add the core crate to a Rust project's `Cargo.toml`:

```toml
[dependencies]
fsspec_rs = "0.1.5"
```

Read and write through `FileSystem`:

```rust
use fsspec_rs::{FileSystem, LocalFs};

fn main() -> fsspec_rs::FsResult<()> {
    let fs = LocalFs::with_auto_mkdir(true);
    let path = std::env::temp_dir().join("fsspec-rs-example.txt");
    let path = path.to_string_lossy();
    fs.pipe_file(&path, b"hello")?;
    assert_eq!(fs.cat_file(&path, None, None)?, b"hello");
    fs.rm_file(&path)?;
    Ok(())
}
```

For an S3 bucket, construct `S3Fs` with `S3Config` and use the same trait methods:

```rust
use fsspec_rs::{FileSystem, S3Config, S3Fs};

let mut config = S3Config::new("my-bucket");
config.region = Some("us-east-1".to_string());
let fs = S3Fs::new(config)?;
let objects = fs.ls("", true)?;
# Ok::<(), fsspec_rs::FsError>(())
```

## Call a Python fsspec backend from Rust

Use the bridge crate in a Python extension or embedded-Python application. It
requires Python and fsspec at runtime:

```toml
[dependencies]
fsspec_rs = "0.1.5"
fsspec_rs_bridge = "0.1.5"
```

Resolve a URL and call the filesystem trait:

```rust
use fsspec_rs::FileSystem;
use fsspec_rs_bridge::{url_to_fs, StorageOptions};

let (fs, _) = url_to_fs("memory://", &StorageOptions::new())?;
fs.pipe_file("/example.txt", b"hello from Python fsspec")?;
assert_eq!(fs.cat_file("/example.txt", None, None)?, b"hello from Python fsspec");
# Ok::<(), fsspec_rs::FsError>(())
```

For chained URLs, pass options in nested maps keyed by protocol:

```rust
use fsspec_rs_bridge::{url_to_fs, StorageOptions};

let mut memory = StorageOptions::new();
memory.insert("skip_instance_cache".to_string(), true.into());
let mut options = StorageOptions::new();
options.insert("memory".to_string(), memory.into());
let (fs, path) = url_to_fs("simplecache::memory://bucket/data.csv", &options)?;
# Ok::<(), fsspec_rs::FsError>(())
```

Within this repository's workspace, use path dependencies `./rust` and
`./rust/bridge` instead of registry versions. See [Rust traits](rust.md) for backend
primitives and bridge value types.
