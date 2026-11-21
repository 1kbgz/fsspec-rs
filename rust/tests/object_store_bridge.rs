#![cfg(feature = "object-store")]

use std::io::{Read, Seek, SeekFrom};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use async_trait::async_trait;
use bytes::Bytes;
use fsspec_rs::{
    object_store_bridge::{FileSystemStore, ObjectStoreFs},
    FileInfo, FileSystem, FsError, FsFile, FsResult, MemoryFs, OpenMode, OpenOptions,
};
use futures_util::{stream::BoxStream, TryStreamExt};
use object_store::{
    memory::InMemory, path::Path, CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload,
    ObjectMeta, ObjectStore, ObjectStoreExt, PutMultipartOptions, PutOptions, PutPayload,
    PutResult,
};

fn run<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

fn snapshot() -> FileSystemStore {
    let fs = Arc::new(MemoryFs::new());
    fs.pipe_file("/root/a", b"abcdef").unwrap();
    fs.pipe_file("/root/dir/b", b"nested").unwrap();
    fs.pipe_file("/root/dir_baz/c", b"neighbor").unwrap();
    fs.pipe_file("/root/space %2F λ", b"literal").unwrap();
    fs.pipe_file("/root/empty", b"").unwrap();
    fs.mkdir("/root/unused", false).unwrap();
    FileSystemStore::from_snapshot(fs, "/root").unwrap()
}

#[test]
fn filesystem_only_reads_and_lists_as_objects() {
    run(async {
        let store = snapshot();
        let path = Path::parse("a").unwrap();
        assert_eq!(store.head(&path).await.unwrap().size, 6);
        assert_eq!(
            store.get(&path).await.unwrap().bytes().await.unwrap(),
            "abcdef"
        );
        assert_eq!(store.get_range(&path, 2..99).await.unwrap(), "cdef");
        let ranges = store.get_ranges(&path, &[4..6, 0..2, 4..6]).await.unwrap();
        assert_eq!(
            ranges,
            [Bytes::from("ef"), Bytes::from("ab"), Bytes::from("ef")]
        );
        assert!(store.get_range(&path, 0..0).await.is_err());
        assert!(store.get_range(&path, 6..7).await.is_err());
        assert!(store
            .get(&Path::parse("empty").unwrap())
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .get(&Path::parse("space %2F λ").unwrap())
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap(),
            "literal"
        );
        let nested: Vec<_> = store
            .list(Some(&Path::parse("dir").unwrap()))
            .try_collect()
            .await
            .unwrap();
        assert_eq!(nested.len(), 1);
        assert_eq!(nested[0].location.as_ref(), "dir/b");
        let root = store.list_with_delimiter(None).await.unwrap();
        assert_eq!(root.objects.len(), 3);
        assert_eq!(
            root.common_prefixes,
            [Path::parse("dir").unwrap(), Path::parse("dir_baz").unwrap()]
        );
        assert!(store
            .list(Some(&Path::parse("missing").unwrap()))
            .try_collect::<Vec<_>>()
            .await
            .unwrap()
            .is_empty());
        assert!(matches!(
            store.head(&Path::parse("missing").unwrap()).await,
            Err(object_store::Error::NotFound { .. })
        ));
    });
}

#[test]
fn snapshot_preconditions_and_mutations_are_explicit() {
    run(async {
        let store = snapshot();
        let p = Path::parse("a").unwrap();
        assert!(matches!(
            store
                .get_opts(
                    &p,
                    GetOptions {
                        if_match: Some("unknown".into()),
                        ..Default::default()
                    }
                )
                .await,
            Err(object_store::Error::Precondition { .. })
        ));
        assert!(matches!(
            store
                .get_opts(
                    &p,
                    GetOptions {
                        if_none_match: Some("*".into()),
                        ..Default::default()
                    }
                )
                .await,
            Err(object_store::Error::NotModified { .. })
        ));
        assert!(matches!(
            store
                .get_opts(
                    &p,
                    GetOptions {
                        version: Some("v1".into()),
                        ..Default::default()
                    }
                )
                .await,
            Err(object_store::Error::NotSupported { .. })
        ));
        assert!(matches!(
            store.put(&p, Bytes::from("overwrite").into()).await,
            Err(object_store::Error::NotSupported { .. })
        ));
        assert!(matches!(
            store.put_multipart(&p).await,
            Err(object_store::Error::NotSupported { .. })
        ));
        assert!(matches!(
            store.delete(&p).await,
            Err(object_store::Error::NotSupported { .. })
        ));
        assert!(matches!(
            store.copy(&p, &Path::parse("copy").unwrap()).await,
            Err(object_store::Error::NotSupported { .. })
        ));
        assert_eq!(
            store.get(&p).await.unwrap().bytes().await.unwrap(),
            "abcdef"
        );
    });
}

fn objects() -> Arc<InMemory> {
    let store = Arc::new(InMemory::new());
    run(async {
        store
            .put(
                &Path::parse("root/a").unwrap(),
                Bytes::from("abcdef").into(),
            )
            .await
            .unwrap();
        store
            .put(
                &Path::parse("root/dir/b").unwrap(),
                Bytes::from("nested").into(),
            )
            .await
            .unwrap();
        store
            .put(
                &Path::parse("outside").unwrap(),
                Bytes::from("hidden").into(),
            )
            .await
            .unwrap();
    });
    store
}

#[test]
fn object_store_only_supports_seekable_filesystem_reads() {
    let fs = ObjectStoreFs::new(objects(), Path::parse("root").unwrap());
    assert_eq!(
        fs.ls("", true)
            .unwrap()
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "dir"]
    );
    assert!(fs.info("dir").unwrap().is_dir());
    assert_eq!(fs.cat_file("a", Some(-3), None).unwrap(), b"def");
    assert!(matches!(fs.info("outside"), Err(FsError::NotFound(_))));
    let mut file = fs.open("a", OpenMode::Read, None).unwrap();
    assert_eq!(file.seek(SeekFrom::End(-2)).unwrap(), 4);
    let mut data = Vec::new();
    file.read_to_end(&mut data).unwrap();
    assert_eq!(data, b"ef");
    assert_eq!(file.seek(SeekFrom::Start(100)).unwrap(), 100);
    assert_eq!(file.read(&mut [0; 1]).unwrap(), 0);
    assert!(file.seek(SeekFrom::Start(0)).is_ok());
    assert!(file.seek(SeekFrom::Current(-1)).is_err());
    assert!(matches!(
        fs.open("a", OpenMode::Write, None),
        Err(FsError::NotSupported(_))
    ));
    for path in ["/a", "../a", "dir//b", "s3://a", "a/"] {
        assert!(fs.info(path).is_err(), "{path}");
    }
}

#[test]
fn opened_object_detects_replacement() {
    let store = objects();
    let fs = ObjectStoreFs::new(store.clone(), Path::parse("root").unwrap());
    let mut file = fs.open("a", OpenMode::Read, None).unwrap();
    run(store.put(
        &Path::parse("root/a").unwrap(),
        Bytes::from("ghijkl").into(),
    ))
    .unwrap();
    let error = file.read(&mut [0; 1]).unwrap_err();
    assert!(matches!(
        error
            .get_ref()
            .and_then(|cause| cause.downcast_ref::<object_store::Error>()),
        Some(object_store::Error::Precondition { .. })
    ));
    assert_eq!(fs.cat_file("a", None, None).unwrap(), b"ghijkl");
}

#[test]
fn collisions_are_not_hidden() {
    let store = objects();
    run(store.put(
        &Path::parse("root/dir").unwrap(),
        Bytes::from("collision").into(),
    ))
    .unwrap();
    let fs = ObjectStoreFs::new(store, Path::parse("root").unwrap());
    assert!(matches!(fs.info("dir"), Err(FsError::NotSupported(_))));
    assert!(matches!(fs.ls("", true), Err(FsError::NotSupported(_))));
}

#[test]
fn synchronous_bridge_rejects_async_context_and_stacking() {
    let fs = Arc::new(ObjectStoreFs::new(objects(), Path::parse("root").unwrap()));
    let mut file = fs.open("a", OpenMode::Read, None).unwrap();
    run(async {
        assert!(matches!(fs.info("a"), Err(FsError::NotSupported(_))));
        assert!(file.read(&mut [0; 1]).is_err());
        let stacked = FileSystemStore::from_snapshot(fs, "").unwrap();
        assert!(matches!(
            stacked.get(&Path::parse("a").unwrap()).await,
            Err(object_store::Error::NotSupported { .. })
        ));
    });
}

/// Native dual-trait fixture. Neither interface contains a bridge.
#[derive(Debug, Default)]
struct Dual {
    store: InMemory,
    file_reads: AtomicUsize,
    object_reads: AtomicUsize,
    ranges: Mutex<Vec<std::ops::Range<u64>>>,
}
impl std::fmt::Display for Dual {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Dual")
    }
}
impl FileSystem for Dual {
    fn protocol(&self) -> &[&str] {
        &["dual"]
    }
    fn cat_file(&self, path: &str, start: Option<i64>, end: Option<i64>) -> FsResult<Vec<u8>> {
        assert!(start.is_none() && end.is_none());
        self.file_reads.fetch_add(1, Ordering::SeqCst);
        Ok(run(async {
            self.store
                .get(&Path::parse(path).unwrap())
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap()
                .to_vec()
        }))
    }
    fn info(&self, _: &str) -> FsResult<FileInfo> {
        Err(FsError::NotSupported("fixture".into()))
    }
    fn ls(&self, _: &str, _: bool) -> FsResult<Vec<FileInfo>> {
        Err(FsError::NotSupported("fixture".into()))
    }
    fn open(&self, _: &str, _: OpenMode, _: Option<OpenOptions>) -> FsResult<Box<dyn FsFile>> {
        Err(FsError::NotSupported("fixture".into()))
    }
    fn mkdir(&self, _: &str, _: bool) -> FsResult<()> {
        Err(FsError::NotSupported("fixture".into()))
    }
    fn rmdir(&self, _: &str) -> FsResult<()> {
        Err(FsError::NotSupported("fixture".into()))
    }
    fn rm_file(&self, _: &str) -> FsResult<()> {
        Err(FsError::NotSupported("fixture".into()))
    }
    fn cp_file(&self, _: &str, _: &str) -> FsResult<()> {
        Err(FsError::NotSupported("fixture".into()))
    }
}
#[async_trait]
impl ObjectStore for Dual {
    async fn get_opts(&self, p: &Path, o: GetOptions) -> object_store::Result<GetResult> {
        self.object_reads.fetch_add(1, Ordering::SeqCst);
        if let Some(object_store::GetRange::Bounded(range)) = &o.range {
            self.ranges.lock().unwrap().push(range.clone());
        }
        self.store.get_opts(p, o).await
    }
    fn list(&self, p: Option<&Path>) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
        self.store.list(p)
    }
    async fn list_with_delimiter(&self, p: Option<&Path>) -> object_store::Result<ListResult> {
        self.store.list_with_delimiter(p).await
    }
    async fn put_opts(
        &self,
        p: &Path,
        b: PutPayload,
        o: PutOptions,
    ) -> object_store::Result<PutResult> {
        self.store.put_opts(p, b, o).await
    }
    async fn put_multipart_opts(
        &self,
        p: &Path,
        o: PutMultipartOptions,
    ) -> object_store::Result<Box<dyn MultipartUpload>> {
        self.store.put_multipart_opts(p, o).await
    }
    fn delete_stream(
        &self,
        paths: BoxStream<'static, object_store::Result<Path>>,
    ) -> BoxStream<'static, object_store::Result<Path>> {
        self.store.delete_stream(paths)
    }
    async fn copy_opts(&self, a: &Path, b: &Path, o: CopyOptions) -> object_store::Result<()> {
        self.store.copy_opts(a, b, o).await
    }
}

#[test]
fn dual_trait_backend_uses_native_views_of_the_same_data() {
    let backend = Arc::new(Dual::default());
    let fs: Arc<dyn FileSystem + Send + Sync> = backend.clone();
    let store: Arc<dyn ObjectStore> = backend.clone();
    let p = Path::parse("a").unwrap();
    run(store.put(&p, Bytes::from("first").into())).unwrap();
    assert_eq!(fs.cat_file("a", None, None).unwrap(), b"first");
    assert_eq!(backend.object_reads.load(Ordering::SeqCst), 0);
    assert_eq!(
        run(async { store.get(&p).await.unwrap().bytes().await.unwrap() }),
        "first"
    );
    assert_eq!(backend.file_reads.load(Ordering::SeqCst), 1);
    run(store.put(&p, Bytes::from("second").into())).unwrap();
    assert_eq!(fs.cat_file("a", None, None).unwrap(), b"second");
    assert_eq!(backend.object_reads.load(Ordering::SeqCst), 1);
}

#[derive(Default)]
struct ObservedFs {
    fs: MemoryFs,
    missing_time: bool,
    other: bool,
    opened: Arc<AtomicUsize>,
    read_bytes: Arc<AtomicUsize>,
    closed: Arc<AtomicUsize>,
    closed_on: Arc<Mutex<Option<std::thread::ThreadId>>>,
    closed_notify: Arc<tokio::sync::Notify>,
}
impl FileSystem for ObservedFs {
    fn protocol(&self) -> &[&str] {
        &["observed"]
    }
    fn root_marker(&self) -> &str {
        "/"
    }
    fn info(&self, path: &str) -> FsResult<FileInfo> {
        let mut info = self.fs.info(path)?;
        if self.missing_time {
            info.modified = None;
        }
        if self.other {
            info.file_type = fsspec_rs::FileType::Other;
        }
        Ok(info)
    }
    fn ls(&self, path: &str, detail: bool) -> FsResult<Vec<FileInfo>> {
        let mut entries = self.fs.ls(path, detail)?;
        if self.missing_time {
            for entry in &mut entries {
                entry.modified = None;
            }
        }
        if self.other {
            for entry in &mut entries {
                entry.file_type = fsspec_rs::FileType::Other;
            }
        }
        Ok(entries)
    }
    fn open(
        &self,
        path: &str,
        mode: OpenMode,
        opts: Option<OpenOptions>,
    ) -> FsResult<Box<dyn FsFile>> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(ObservedFile {
            file: self.fs.open(path, mode, opts)?,
            read_bytes: self.read_bytes.clone(),
            closed: self.closed.clone(),
            closed_on: self.closed_on.clone(),
            closed_notify: self.closed_notify.clone(),
        }))
    }
    fn mkdir(&self, _: &str, _: bool) -> FsResult<()> {
        panic!("unexpected mutation")
    }
    fn rmdir(&self, _: &str) -> FsResult<()> {
        panic!("unexpected mutation")
    }
    fn rm_file(&self, _: &str) -> FsResult<()> {
        panic!("unexpected mutation")
    }
    fn cp_file(&self, _: &str, _: &str) -> FsResult<()> {
        panic!("unexpected mutation")
    }
}
struct ObservedFile {
    file: Box<dyn FsFile>,
    read_bytes: Arc<AtomicUsize>,
    closed: Arc<AtomicUsize>,
    closed_on: Arc<Mutex<Option<std::thread::ThreadId>>>,
    closed_notify: Arc<tokio::sync::Notify>,
}
impl Drop for ObservedFile {
    fn drop(&mut self) {
        self.closed.fetch_add(1, Ordering::SeqCst);
        *self.closed_on.lock().unwrap() = Some(std::thread::current().id());
        self.closed_notify.notify_one();
    }
}
impl Read for ObservedFile {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        let count = self.file.read(b)?;
        self.read_bytes.fetch_add(count, Ordering::SeqCst);
        Ok(count)
    }
}
impl Seek for ObservedFile {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(p)
    }
}
impl std::io::Write for ObservedFile {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        panic!("unexpected mutation")
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl FsFile for ObservedFile {
    fn info(&self) -> FsResult<FileInfo> {
        self.file.info()
    }
    fn size(&self) -> FsResult<Option<u64>> {
        self.file.size()
    }
}

#[test]
fn snapshot_stream_is_lazy_bounded_and_closes_on_early_exit() {
    let fs = Arc::new(ObservedFs::default());
    fs.fs.pipe_file("/a", &vec![42; 200_000]).unwrap();
    let store = FileSystemStore::from_snapshot(fs.clone(), "/").unwrap();
    run(async {
        let p = Path::parse("a").unwrap();
        store.head(&p).await.unwrap();
        assert_eq!(fs.opened.load(Ordering::SeqCst), 0);
        let result = store.get(&p).await.unwrap();
        assert_eq!(fs.read_bytes.load(Ordering::SeqCst), 0);
        let mut stream = result.into_stream();
        let first = stream.try_next().await.unwrap().unwrap();
        assert_eq!(first.len(), 64 * 1024);
        assert_eq!(fs.read_bytes.load(Ordering::SeqCst), first.len());
        drop(stream);
        fs.closed_notify.notified().await;
        assert_eq!(fs.closed.load(Ordering::SeqCst), 1);
        assert_ne!(
            *fs.closed_on.lock().unwrap(),
            Some(std::thread::current().id())
        );
    });
}

#[test]
fn snapshot_rejects_missing_metadata_before_opening() {
    let fs = Arc::new(ObservedFs {
        missing_time: true,
        ..Default::default()
    });
    fs.fs.pipe_file("/a", b"data").unwrap();
    let store = FileSystemStore::from_snapshot(fs.clone(), "/").unwrap();
    run(async {
        assert!(matches!(
            store.get(&Path::parse("a").unwrap()).await,
            Err(object_store::Error::NotSupported { .. })
        ));
        assert!(matches!(
            store.list(None).try_collect::<Vec<_>>().await,
            Err(object_store::Error::NotSupported { .. })
        ));
    });
    assert_eq!(fs.opened.load(Ordering::SeqCst), 0);
}

#[test]
fn snapshot_listing_matches_native_object_store_prefix_semantics() {
    run(async {
        let snapshot = snapshot();
        let native = InMemory::new();
        for name in ["a", "dir/b", "dir_baz/c", "space %2F λ", "empty"] {
            native
                .put(&Path::parse(name).unwrap(), Bytes::new().into())
                .await
                .unwrap();
        }
        for prefix in ["", "a", "dir", "dir_baz", "missing"] {
            let prefix = Path::parse(prefix).unwrap();
            let mut actual: Vec<_> = snapshot
                .list(Some(&prefix))
                .map_ok(|m| m.location)
                .try_collect()
                .await
                .unwrap();
            let mut expected: Vec<_> = native
                .list(Some(&prefix))
                .map_ok(|m| m.location)
                .try_collect()
                .await
                .unwrap();
            actual.sort();
            expected.sort();
            assert_eq!(actual, expected, "prefix {prefix}");
            let actual = snapshot.list_with_delimiter(Some(&prefix)).await.unwrap();
            let expected = native.list_with_delimiter(Some(&prefix)).await.unwrap();
            let mut actual_objects: Vec<_> =
                actual.objects.into_iter().map(|m| m.location).collect();
            let mut expected_objects: Vec<_> =
                expected.objects.into_iter().map(|m| m.location).collect();
            actual_objects.sort();
            expected_objects.sort();
            assert_eq!(
                actual_objects, expected_objects,
                "delimiter prefix {prefix}"
            );
            assert_eq!(actual.common_prefixes, expected.common_prefixes);
        }
    });
}

#[test]
fn snapshot_directories_are_missing_objects() {
    run(async {
        let store = snapshot();
        for name in ["", "dir", "unused"] {
            let path = Path::parse(name).unwrap();
            assert!(
                matches!(
                    store.head(&path).await,
                    Err(object_store::Error::NotFound { .. })
                ),
                "{name}"
            );
            assert!(
                matches!(
                    store.get(&path).await,
                    Err(object_store::Error::NotFound { .. })
                ),
                "{name}"
            );
        }
        let fs = Arc::new(ObservedFs {
            other: true,
            ..Default::default()
        });
        fs.fs.pipe_file("/a", b"data").unwrap();
        let store = FileSystemStore::from_snapshot(fs, "/").unwrap();
        assert!(matches!(
            store.get(&Path::parse("a").unwrap()).await,
            Err(object_store::Error::NotSupported { .. })
        ));
        assert!(matches!(
            store.list(None).try_collect::<Vec<_>>().await,
            Err(object_store::Error::NotSupported { .. })
        ));
    });
}

#[test]
fn listings_reject_unrepresentable_names_instead_of_returning_unusable_paths() {
    let fs = Arc::new(MemoryFs::new());
    fs.pipe_file("/a\\b", b"data").unwrap();
    let snapshot = FileSystemStore::from_snapshot(fs, "/").unwrap();
    run(async {
        assert!(matches!(
            snapshot.list(None).try_collect::<Vec<_>>().await,
            Err(object_store::Error::NotSupported { .. })
        ));
        assert!(matches!(
            snapshot.list_with_delimiter(None).await,
            Err(object_store::Error::NotSupported { .. })
        ));
    });
    let objects = objects();
    run(objects.put(
        &Path::parse("root/a\\b").unwrap(),
        Bytes::from_static(b"data").into(),
    ))
    .unwrap();
    let fs = ObjectStoreFs::new(objects, Path::parse("root").unwrap());
    assert!(matches!(fs.ls("", true), Err(FsError::NotSupported(_))));
    assert!(matches!(fs.info("a\\b"), Err(FsError::InvalidArgument(_))));
}

#[test]
fn snapshot_range_crosses_chunks_and_closes_before_final_chunk_returns() {
    let fs = Arc::new(ObservedFs::default());
    let bytes: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
    fs.fs.pipe_file("/a", &bytes).unwrap();
    let store = FileSystemStore::from_snapshot(fs.clone(), "/").unwrap();
    run(async {
        let result = store
            .get_opts(
                &Path::parse("a").unwrap(),
                GetOptions {
                    range: Some(object_store::GetRange::Bounded(13..150_013)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let mut chunks = result.into_stream();
        let mut collected = Vec::new();
        for expected_len in [65_536, 65_536, 18_928] {
            let chunk = chunks.try_next().await.unwrap().unwrap();
            assert_eq!(chunk.len(), expected_len);
            collected.extend_from_slice(&chunk);
        }
        assert_eq!(collected, bytes[13..150_013]);
        // Cleanup must not depend on polling for None after the last data chunk.
        assert_eq!(fs.closed.load(Ordering::SeqCst), 1);
        assert_ne!(
            *fs.closed_on.lock().unwrap(),
            Some(std::thread::current().id())
        );
        assert!(chunks.try_next().await.unwrap().is_none());
    });
}

#[test]
fn cancelling_a_response_closes_its_unpolled_file_on_a_worker() {
    let fs = Arc::new(ObservedFs::default());
    fs.fs.pipe_file("/a", b"data").unwrap();
    let store = FileSystemStore::from_snapshot(fs.clone(), "/").unwrap();
    run(async {
        let (ready, started) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let result = store.get(&Path::parse("a").unwrap()).await.unwrap();
            ready.send(()).unwrap();
            std::future::pending::<()>().await;
            drop(result);
        });
        started.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        fs.closed_notify.notified().await;
        assert_eq!(fs.closed.load(Ordering::SeqCst), 1);
        assert_eq!(fs.read_bytes.load(Ordering::SeqCst), 0);
        assert_ne!(
            *fs.closed_on.lock().unwrap(),
            Some(std::thread::current().id())
        );
    });
}

#[test]
fn object_file_limits_native_requests_and_preserves_missing_error_kind() {
    let native = Arc::new(Dual::default());
    let bytes: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
    let p = Path::parse("a").unwrap();
    run(native.put(&p, Bytes::from(bytes.clone()).into())).unwrap();
    let fs = ObjectStoreFs::new(native.clone(), Path::default());
    let mut file = fs.open("a", OpenMode::Read, None).unwrap();
    assert!(native.ranges.lock().unwrap().is_empty());
    file.seek(SeekFrom::Start(13)).unwrap();
    let mut read = Vec::new();
    let mut buf = [0; 100_000];
    loop {
        let n = file.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        read.extend_from_slice(&buf[..n]);
    }
    assert_eq!(read, bytes[13..]);
    assert_eq!(
        *native.ranges.lock().unwrap(),
        [
            13..65_549,
            65_549..131_085,
            131_085..196_621,
            196_621..200_000
        ]
    );
    run(native.delete(&p)).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    assert_eq!(
        file.read(&mut buf).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    assert!(matches!(fs.info("a"), Err(FsError::NotFound(path)) if path == "a"));
}
