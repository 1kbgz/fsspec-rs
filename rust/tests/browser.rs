//! Browser integration proving that an OPFS-backed crate can own JavaScript handles
//! and await promises through fsspec-rs's public async traits without unsafe Send impls.
#![cfg(all(target_arch = "wasm32", target_os = "unknown"))]

use fsspec_rs::{
    AsyncFileSystem, AsyncFsFile, FileInfo, FsError, FsFile, FsResult, IoFuture, OpenMode,
    OpenOptions, OperationError,
};
use js_sys::{Promise, Uint8Array};
use std::io::SeekFrom;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen(inline_js = "
export async function prepare() {
    const root = await navigator.storage.getDirectory();
    const name = 'fsspec-test-' + crypto.randomUUID();
    const directory = await root.getDirectoryHandle(name, {create: true});
    const handle = await directory.getFileHandle('value', {create: true});
    const writer = await handle.createWritable();
    await writer.write(new Uint8Array([10, 20, 30, 40]));
    await writer.close();
    return {root, name, directory};
}
export async function get_file(store, name) {
    return (await store.directory.getFileHandle(name)).getFile();
}
export async function read_bytes(file, start, length) {
    return file.slice(start, start + length).arrayBuffer();
}
export async function remove_fixture(store) {
    await store.root.removeEntry(store.name, {recursive: true});
}
")]
extern "C" {
    fn prepare() -> Promise;
    fn get_file(store: &JsValue, name: &str) -> Promise;
    fn read_bytes(file: &JsValue, start: u32, length: u32) -> Promise;
    fn remove_fixture(store: &JsValue) -> Promise;
}

fn js_error(error: JsValue) -> OperationError {
    FsError::Other(format!("{error:?}")).into()
}
fn unsupported<T>() -> FsResult<T> {
    Err(FsError::NotSupported(
        "fixture implements async reading only".into(),
    ))
}
struct BrowserStore(JsValue);
struct BrowserFile {
    file: Option<JsValue>,
    position: u32,
}
impl AsyncFsFile for BrowserFile {
    fn read(&mut self, max_bytes: usize) -> IoFuture<'_, Vec<u8>> {
        Box::pin(async move {
            let file = self
                .file
                .as_ref()
                .ok_or_else(|| FsError::InvalidArgument("closed".into()))?;
            let buffer = JsFuture::from(read_bytes(
                file,
                self.position,
                max_bytes.min(u32::MAX as usize) as u32,
            ))
            .await
            .map_err(js_error)?;
            let bytes = Uint8Array::new(&buffer).to_vec();
            self.position += bytes.len() as u32;
            Ok(bytes)
        })
    }
    fn seek(&mut self, position: SeekFrom) -> IoFuture<'_, u64> {
        Box::pin(async move {
            let SeekFrom::Start(position) = position else {
                return Err(FsError::NotSupported("fixture supports absolute seeks".into()).into());
            };
            self.position = position
                .try_into()
                .map_err(|_| FsError::InvalidArgument("offset overflow".into()))?;
            Ok(position)
        })
    }
    fn info(&self) -> IoFuture<'_, FileInfo> {
        Box::pin(async { Ok(FileInfo::file("value", 4)) })
    }
    fn close(&mut self) -> IoFuture<'_, ()> {
        Box::pin(async {
            self.file = None;
            Ok(())
        })
    }
}
impl AsyncFileSystem for BrowserStore {
    fn protocol(&self) -> &[&str] {
        &["opfs-fixture"]
    }
    async fn ls(&self, _: &str, _: bool) -> FsResult<Vec<FileInfo>> {
        unsupported()
    }
    async fn rm_file(&self, _: &str) -> FsResult<()> {
        unsupported()
    }
    async fn cp_file(&self, _: &str, _: &str) -> FsResult<()> {
        unsupported()
    }
    async fn open(
        &self,
        _: &str,
        _: OpenMode,
        _: Option<OpenOptions>,
    ) -> FsResult<Box<dyn FsFile>> {
        unsupported()
    }
    async fn info(&self, _: &str) -> FsResult<FileInfo> {
        unsupported()
    }
    async fn mkdir(&self, _: &str, _: bool) -> FsResult<()> {
        unsupported()
    }
    async fn rmdir(&self, _: &str) -> FsResult<()> {
        unsupported()
    }
    async fn open_async(&self, path: &str) -> Result<Box<dyn AsyncFsFile>, OperationError> {
        let file = JsFuture::from(get_file(&self.0, path))
            .await
            .map_err(js_error)?;
        Ok(Box::new(BrowserFile {
            file: Some(file),
            position: 0,
        }))
    }
}

#[wasm_bindgen_test]
async fn opfs_handles_and_promises_work_through_async_traits() {
    let fixture = JsFuture::from(prepare()).await.unwrap();
    let store = BrowserStore(fixture.clone());
    let mut file = store.open_async("value").await.unwrap();
    assert_eq!(file.info().await.unwrap().size, 4);
    assert_eq!(file.read(2).await.unwrap(), [10, 20]);
    assert_eq!(file.seek(SeekFrom::Start(1)).await.unwrap(), 1);
    assert_eq!(file.read(10).await.unwrap(), [20, 30, 40]);
    assert!(file.read(1).await.unwrap().is_empty());
    file.close().await.unwrap();
    assert!(file.read(1).await.is_err());
    store.close_async().await.unwrap();
    JsFuture::from(remove_fixture(&fixture)).await.unwrap();
}
