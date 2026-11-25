//! The core crate built without the `s3` feature, running in WebAssembly under
//! wasm-bindgen-test. `make -C rust test-wasm` runs it in Node.
#![cfg(all(target_arch = "wasm32", target_os = "unknown"))]

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

#[wasm_bindgen_test]
fn browser_local_factories_and_buffers_need_no_send_wrappers() {
    use fsspec_rs::{register_filesystem, BufferedFile, CacheType, FsFile, SharedFileSystem};
    use std::cell::RefCell;
    use std::rc::Rc;

    let bytes = Rc::new(RefCell::new(Vec::new()));
    let output = bytes.clone();
    let mut writer: Box<dyn FsFile> = Box::new(BufferedFile::new_write(
        "file".into(),
        Box::new(move |data| {
            *output.borrow_mut() = data.to_vec();
            Ok(())
        }),
        false,
    ));
    writer.write_all(b"browser-local").unwrap();
    writer.commit().unwrap();
    let input = bytes.clone();
    let mut reader: Box<dyn FsFile> = Box::new(BufferedFile::new_read(
        "file".into(),
        Box::new(move |start, end| Ok(input.borrow()[start as usize..end as usize].to_vec())),
        Some(13),
        CacheType::Block,
        4,
        2,
    ));
    let mut result = Vec::new();
    reader.read_to_end(&mut result).unwrap();
    assert_eq!(result, b"browser-local");

    register_filesystem("browser-local", move |_: &str, _: &FsOptions| {
        // Registration from inside the factory must not deadlock the default registry.
        register_filesystem("browser-inner", |_: &str, _: &FsOptions| {
            Ok(Arc::new(MemoryFs::new()) as SharedFileSystem)
        });
        let fs = MemoryFs::new();
        fs.pipe_file("/file", &bytes.borrow())?;
        Ok(Arc::new(BrowserLocalFs(Rc::new(fs))) as SharedFileSystem)
    });
    let (first, path) = url_to_fs("browser-local://file", &FsOptions::new()).unwrap();
    let (second, _) = url_to_fs("browser-local://file", &FsOptions::new()).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(first.cat_file(&path, None, None).unwrap(), b"browser-local");
    assert!(url_to_fs("browser-inner://file", &FsOptions::new()).is_ok());
}

// An Rc-owned filesystem proves that the registry accepts a backend itself that
// cannot cross threads, in addition to factories and buffered callbacks.
struct BrowserLocalFs(std::rc::Rc<MemoryFs>);
impl FileSystem for BrowserLocalFs {
    fn protocol(&self) -> &[&str] {
        &["browser-local"]
    }
    fn ls(&self, p: &str, d: bool) -> fsspec_rs::FsResult<Vec<fsspec_rs::FileInfo>> {
        self.0.ls(p, d)
    }
    fn info(&self, p: &str) -> fsspec_rs::FsResult<fsspec_rs::FileInfo> {
        self.0.info(p)
    }
    fn open(
        &self,
        p: &str,
        m: OpenMode,
        o: Option<fsspec_rs::OpenOptions>,
    ) -> fsspec_rs::FsResult<Box<dyn fsspec_rs::FsFile>> {
        self.0.open(p, m, o)
    }
    fn rm_file(&self, p: &str) -> fsspec_rs::FsResult<()> {
        self.0.rm_file(p)
    }
    fn cp_file(&self, a: &str, b: &str) -> fsspec_rs::FsResult<()> {
        self.0.cp_file(a, b)
    }
    fn mkdir(&self, p: &str, parents: bool) -> fsspec_rs::FsResult<()> {
        self.0.mkdir(p, parents)
    }
    fn rmdir(&self, p: &str) -> fsspec_rs::FsResult<()> {
        self.0.rmdir(p)
    }
}
