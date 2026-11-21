//! Run with `cargo run --example object_store_bridge --features object-store`.
use std::sync::Arc;

use bytes::Bytes;
use fsspec_rs::{
    object_store_bridge::{FileSystemStore, ObjectStoreFs},
    FileSystem, MemoryFs,
};
use object_store::{memory::InMemory, path::Path, ObjectStoreExt};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Runtime::new()?;

    // FileSystem-only backend: populate it, then keep the namespace immutable.
    let filesystem = Arc::new(MemoryFs::new());
    filesystem.pipe_file("/snapshot/orders", b"from filesystem")?;
    let objects = FileSystemStore::from_snapshot(filesystem, "/snapshot")?;
    let data =
        runtime.block_on(async { objects.get(&Path::parse("orders")?).await?.bytes().await })?;
    assert_eq!(data, "from filesystem");

    // ObjectStore-only backend: expose a filesystem rooted at a prefix.
    let native = Arc::new(InMemory::new());
    runtime.block_on(native.put(
        &Path::parse("tables/orders")?,
        Bytes::from_static(b"from object store").into(),
    ))?;
    let filesystem = ObjectStoreFs::new(native, Path::parse("tables")?);
    // Synchronous access must be outside a Tokio runtime context.
    assert_eq!(
        filesystem.cat_file("orders", None, None)?,
        b"from object store"
    );
    Ok(())
}
