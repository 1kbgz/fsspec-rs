//! Native callers retain the thread guarantees used by runtimes and the C ABI.
#![cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]

use fsspec_rs::{AsyncFsFile, FilesystemRegistry, FsFile, IoFuture, SharedFileSystem};

#[test]
fn native_files_futures_and_registries_keep_thread_bounds() {
    fn send<T: Send + ?Sized>() {}
    fn send_sync<T: Send + Sync + ?Sized>() {}
    send::<dyn FsFile>();
    send::<dyn fsspec_rs::caching::Cache>();
    send::<fsspec_rs::caching::Fetcher>();
    send::<fsspec_rs::buffered::Uploader>();
    send::<IoFuture<'static, Vec<u8>>>();
    send_sync::<dyn AsyncFsFile>();
    send_sync::<SharedFileSystem>();
    send_sync::<FilesystemRegistry>();
}
