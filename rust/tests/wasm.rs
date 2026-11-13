//! The core crate built without the `s3` feature, running in WebAssembly under
//! wasm-bindgen-test. `make -C rust test-wasm` runs it in Node.
#![cfg(target_arch = "wasm32")]

use std::io::{Read, Write};
use std::sync::Arc;

use fsspec_rs::{url_to_fs, FileSystem, FsError, FsOptions, LocalFs, MemoryFs, OpenMode};
use wasm_bindgen_test::wasm_bindgen_test;

#[wasm_bindgen_test]
fn memory_filesystem_reads_writes_and_lists() {
    let fs = MemoryFs::new();
    fs.pipe_file("/data/a.txt", b"hello").unwrap();
    let mut file = fs.open("/data/b.txt", OpenMode::Write, None).unwrap();
    file.write_all(b"from wasm").unwrap();
    file.commit().unwrap();

    let names: Vec<_> = fs
        .ls("/data", true)
        .unwrap()
        .into_iter()
        .map(|info| info.name)
        .collect();
    assert_eq!(names, ["/data/a.txt", "/data/b.txt"]);
    assert_eq!(
        fs.cat_file("/data/a.txt", Some(1), Some(4)).unwrap(),
        b"ell"
    );

    let mut text = String::new();
    fs.open("/data/b.txt", OpenMode::Read, None)
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "from wasm");

    // wasm32-unknown-unknown has no clock, so files carry no timestamps.
    assert_eq!(fs.info("/data/a.txt").unwrap().modified, None);

    fs.rm("/data", true).unwrap();
    assert!(!fs.exists("/data").unwrap());
}

#[wasm_bindgen_test]
fn registry_serves_memory_urls_without_s3() {
    let (first, path) = url_to_fs("memory://shared/a", &FsOptions::new()).unwrap();
    assert_eq!(path, "/shared/a");
    first.pipe_file(&path, b"x").unwrap();
    let (second, _) = url_to_fs("memory://other", &FsOptions::new()).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert!(second.exists("/shared/a").unwrap());

    assert!(matches!(
        url_to_fs("s3://bucket/key", &FsOptions::new()),
        Err(FsError::NotSupported(_))
    ));
}

#[wasm_bindgen_test]
fn local_filesystem_reports_errors_instead_of_panicking() {
    assert!(LocalFs::new().info("/").is_err());
}
