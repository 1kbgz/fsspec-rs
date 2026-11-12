//! Tests run with `fsspec_capi` compiled in, so every call goes through the
//! C ABI and back without a shared library.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, MAIN_SEPARATOR};
use std::sync::Arc;

use fsspec_rs::{
    FileInfo, FileSystem, FileType, FsError, FsFile, FsOptions, LocalFs, OpenMode, OpenOptions,
    SharedFileSystem,
};

use crate::*;

// Pulls the implementation into the test binary.
#[allow(unused_imports)]
use fsspec as _;

fn join(base: &Path, name: &str) -> String {
    format!("{}{MAIN_SEPARATOR}{name}", base.display())
}

fn local_url(dir: &Path) -> String {
    format!("file://{}", dir.display())
}

#[test]
fn abi_version_matches() {
    assert_eq!(abi_version(), FSSPEC_ABI_VERSION);
}

#[test]
fn foreign_filesystem_matches_local_fs() {
    let dir = tempfile::tempdir().unwrap();
    let (foreign, root) = url_to_fs(&local_url(dir.path()), &FsOptions::new()).unwrap();
    let local = LocalFs::new();
    assert_eq!(root, dir.path().display().to_string());
    assert_eq!(foreign.protocol(), local.protocol());
    assert_eq!(foreign.root_marker(), local.root_marker());
    assert_eq!(foreign.sep(), local.sep());

    let file = join(dir.path(), "hello.txt");
    let copy = join(dir.path(), "copy.txt");
    let sub = join(dir.path(), "sub");
    foreign.pipe_file(&file, b"hello").unwrap();
    assert_eq!(foreign.cat_file(&file, None, None).unwrap(), b"hello");
    assert_eq!(foreign.cat_file(&file, Some(1), Some(3)).unwrap(), b"el");
    assert_eq!(local.cat_file(&file, None, None).unwrap(), b"hello");
    assert!(foreign.exists(&file).unwrap());
    assert!(foreign.isfile(&file).unwrap());
    assert_eq!(foreign.size(&file).unwrap(), 5);

    foreign.cp_file(&file, &copy).unwrap();
    foreign.mkdir(&sub, true).unwrap();
    assert!(foreign.isdir(&sub).unwrap());
    let mut names: Vec<String> = foreign
        .ls(&root, true)
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    names.sort();
    let mut local_names: Vec<String> = local
        .ls(&root, true)
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    local_names.sort();
    assert_eq!(names, local_names);
    assert_eq!(names, vec![copy.clone(), file.clone(), sub.clone()]);

    let info = foreign.info(&file).unwrap();
    assert_eq!(info.size, 5);
    assert_eq!(info.file_type, FileType::File);
    assert!(info.modified.is_some());

    // Trait defaults run over the primitives.
    let mut found = foreign.find(&root, None, false).unwrap();
    found.sort();
    assert_eq!(found, vec![copy.clone(), file.clone()]);

    foreign.rmdir(&sub).unwrap();
    foreign.rm_file(&copy).unwrap();
    assert!(!foreign.exists(&copy).unwrap());
    assert!(
        matches!(foreign.info(&copy), Err(FsError::NotFound(message)) if message.contains("copy.txt"))
    );
}

#[test]
fn foreign_file_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let (foreign, root) = url_to_fs(&local_url(dir.path()), &FsOptions::new()).unwrap();
    let path = join(Path::new(&root), "data.bin");

    let mut file = foreign.open(&path, OpenMode::Write, None).unwrap();
    file.write_all(b"abcdef").unwrap();
    file.flush().unwrap();
    drop(file);

    let opts = fsspec_rs::OpenOptions {
        cache_type: Some(fsspec_rs::CacheType::All),
        ..Default::default()
    };
    let mut file = foreign.open(&path, OpenMode::Read, Some(opts)).unwrap();
    assert_eq!(file.size().unwrap(), Some(6));
    assert_eq!(file.seek(SeekFrom::Start(2)).unwrap(), 2);
    let mut text = String::new();
    file.read_to_string(&mut text).unwrap();
    assert_eq!(text, "cdef");
    assert_eq!(file.info().unwrap().size, 6);
    file.commit().unwrap();
    file.discard().unwrap();
}

#[test]
fn url_errors_carry_messages() {
    let Err(error) = url_to_fs("nope://bucket/key", &FsOptions::new()) else {
        panic!("unknown protocol resolved")
    };
    assert!(matches!(error, FsError::NotSupported(message) if message.contains("nope")));
    let mut options = FsOptions::new();
    options.insert("bogus".to_string(), "1".to_string());
    let Err(error) = url_to_fs("file:///tmp", &options) else {
        panic!("bogus option accepted")
    };
    assert!(matches!(error, FsError::InvalidArgument(message) if message.contains("bogus")));
}

#[test]
fn exported_backend_is_reachable_from_both_registries() {
    let factory = |_path: &str, options: &FsOptions| -> fsspec_rs::FsResult<SharedFileSystem> {
        let auto_mkdir = options
            .get("auto_mkdir")
            .is_some_and(|value| value == "true");
        Ok(Arc::new(LocalFs::with_auto_mkdir(auto_mkdir)))
    };
    register_filesystem("systest", factory).unwrap();
    assert!(protocols().unwrap().contains(&"systest".to_string()));

    let dir = tempfile::tempdir().unwrap();
    let mut options = FsOptions::new();
    options.insert("auto_mkdir".to_string(), "true".to_string());

    // Through libfsspec: the C API resolves the protocol and calls back into the exported LocalFs.
    let (foreign, root) =
        url_to_fs(&format!("systest://{}", dir.path().display()), &options).unwrap();
    assert_eq!(root, dir.path().display().to_string());
    assert_eq!(foreign.protocol(), ["file", "local"]);
    let nested = join(&dir.path().join("deep"), "file.txt");
    foreign.pipe_file(&nested, b"through the vtable").unwrap();
    assert_eq!(
        foreign.cat_file(&nested, Some(12), None).unwrap(),
        b"vtable"
    );
    assert!(dir.path().join("deep").join("file.txt").is_file());
    let mut file = foreign.open(&nested, OpenMode::Append, None).unwrap();
    file.write_all(b"!").unwrap();
    drop(file);
    assert_eq!(foreign.cat_file(&nested, Some(-1), None).unwrap(), b"!");

    // The Rust registry in the same process sees the registration too.
    let (direct, _) =
        fsspec_rs::url_to_fs(&format!("systest://{}", dir.path().display()), &options).unwrap();
    assert!(direct.exists(&nested).unwrap());
}

#[test]
fn exported_file_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let local = LocalFs::new();
    let path = join(dir.path(), "exported.txt");
    local.pipe_file(&path, b"exported").unwrap();
    let handle = export_file(local.open(&path, OpenMode::Read, None).unwrap()).unwrap();
    let mut file = unsafe { ForeignFile::from_raw(handle) }.unwrap();
    let mut text = String::new();
    file.read_to_string(&mut text).unwrap();
    assert_eq!(text, "exported");
    assert_eq!(file.size().unwrap(), Some(8));
}

#[test]
fn exported_filesystem_handle_is_reference_counted() {
    let dir = tempfile::tempdir().unwrap();
    let handle = export_filesystem(Arc::new(LocalFs::new())).unwrap();
    let foreign = unsafe { ForeignFileSystem::from_raw(handle) }.unwrap();
    let second = foreign.clone();
    drop(foreign);
    let path = join(dir.path(), "counted.txt");
    second.pipe_file(&path, b"still open").unwrap();
    assert_eq!(second.cat_file(&path, None, None).unwrap(), b"still open");
}

#[test]
fn shared_registry_hooks_route_fsspec_rs_functions_through_libfsspec() {
    use_shared_registry();
    use_shared_registry();
    let dir = tempfile::tempdir().unwrap();

    fsspec_rs::register_filesystem("hooked", |_: &str, _: &FsOptions| {
        Ok(Arc::new(LocalFs::new()) as SharedFileSystem)
    });
    assert!(protocols().unwrap().contains(&"hooked".to_string()));

    // "hooked" exists only in libfsspec's registry, so resolving it proves the route.
    let (fs, path) = fsspec_rs::url_to_fs(
        &format!("hooked://{}", dir.path().display()),
        &FsOptions::new(),
    )
    .unwrap();
    fs.pipe_file(&join(Path::new(&path), "via-hook.txt"), b"shared")
        .unwrap();
    assert!(dir.path().join("via-hook.txt").exists());
}

/// A filesystem that overrides the methods real backends override, each counting
/// its calls, so a method the vtable does not carry shows up as a zero count.
struct Prefixed(LocalFs);

static OVERRIDE_CALLS: [std::sync::atomic::AtomicUsize; 4] = [
    std::sync::atomic::AtomicUsize::new(0),
    std::sync::atomic::AtomicUsize::new(0),
    std::sync::atomic::AtomicUsize::new(0),
    std::sync::atomic::AtomicUsize::new(0),
];
const STRIP: usize = 0;
const UNSTRIP: usize = 1;
const CAT: usize = 2;
const PIPE: usize = 3;

fn count(which: usize) {
    OVERRIDE_CALLS[which].fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

fn counted(which: usize) -> usize {
    OVERRIDE_CALLS[which].load(std::sync::atomic::Ordering::SeqCst)
}

impl FileSystem for Prefixed {
    fn protocol(&self) -> &[&str] {
        &["prefixed"]
    }

    fn strip_protocol(&self, path: &str) -> String {
        count(STRIP);
        path.trim_start_matches("prefixed://")
            .trim_start_matches("mount/")
            .to_string()
    }

    fn unstrip_protocol(&self, path: &str) -> String {
        count(UNSTRIP);
        format!("prefixed://mount/{path}")
    }

    fn cat_file(&self, path: &str, start: Option<i64>, end: Option<i64>) -> FsResult<Vec<u8>> {
        count(CAT);
        self.0.cat_file(path, start, end)
    }

    fn pipe_file(&self, path: &str, data: &[u8]) -> FsResult<()> {
        count(PIPE);
        self.0.pipe_file(path, data)
    }

    fn ls(&self, path: &str, detail: bool) -> FsResult<Vec<FileInfo>> {
        self.0.ls(path, detail)
    }

    fn info(&self, path: &str) -> FsResult<FileInfo> {
        self.0.info(path)
    }

    fn rm_file(&self, path: &str) -> FsResult<()> {
        self.0.rm_file(path)
    }

    fn cp_file(&self, src: &str, dst: &str) -> FsResult<()> {
        self.0.cp_file(src, dst)
    }

    fn mkdir(&self, path: &str, create_parents: bool) -> FsResult<()> {
        self.0.mkdir(path, create_parents)
    }

    fn rmdir(&self, path: &str) -> FsResult<()> {
        self.0.rmdir(path)
    }

    fn open(
        &self,
        path: &str,
        mode: OpenMode,
        options: Option<OpenOptions>,
    ) -> FsResult<Box<dyn FsFile>> {
        self.0.open(path, mode, options)
    }
}

#[test]
fn path_normalisation_crosses_the_abi() {
    let handle = export_filesystem(Arc::new(Prefixed(LocalFs::new()))).unwrap();
    let foreign = unsafe { ForeignFileSystem::from_raw(handle) }.unwrap();
    assert_eq!(foreign.strip_protocol("prefixed://mount/a/b"), "a/b");
    assert_eq!(foreign.unstrip_protocol("a/b"), "prefixed://mount/a/b");

    // The registry strips through the same route, so url_to_fs gives the backend's path.
    register_filesystem("prefixed", |_: &str, _: &FsOptions| {
        Ok(Arc::new(Prefixed(LocalFs::new())) as SharedFileSystem)
    })
    .unwrap();
    let (_, path) = url_to_fs("prefixed://mount/a/b", &FsOptions::new()).unwrap();
    assert_eq!(path, "a/b");
}

#[test]
fn fallback_crosses_the_abi_with_the_whole_url() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = seen.clone();
    // Other tests expect unknown protocols to fail, so this fallback only takes its own.
    register_fallback(Some(move |url: &str, _: &FsOptions| {
        if !url.starts_with("nobody-registered-this://") {
            return Err(FsError::NotSupported(format!(
                "no filesystem registered for: {url}"
            )));
        }
        recorded.lock().unwrap().push(url.to_string());
        Ok(Arc::new(LocalFs::new()) as SharedFileSystem)
    }))
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let url = format!("nobody-registered-this://{}", dir.path().display());
    let (fs, path) = url_to_fs(&url, &FsOptions::new()).unwrap();
    assert_eq!(*seen.lock().unwrap(), std::slice::from_ref(&url));
    assert_eq!(path, url);
    assert!(fs.exists(&join(dir.path(), "")).unwrap_or(true));

    assert!(url_to_fs("nope://bucket/key", &FsOptions::new()).is_err());
}

#[test]
fn overridden_methods_reach_the_backend_through_the_abi() {
    let dir = tempfile::tempdir().unwrap();
    let handle = export_filesystem(Arc::new(Prefixed(LocalFs::new()))).unwrap();
    let foreign = unsafe { ForeignFileSystem::from_raw(handle) }.unwrap();
    let path = join(dir.path(), "counted.txt");

    let before = [
        counted(STRIP),
        counted(UNSTRIP),
        counted(CAT),
        counted(PIPE),
    ];
    foreign.strip_protocol("prefixed://mount/x");
    foreign.unstrip_protocol("x");
    foreign.pipe_file(&path, b"0123456789").unwrap();
    assert_eq!(foreign.cat_file(&path, Some(2), Some(5)).unwrap(), b"234");
    let after = [
        counted(STRIP),
        counted(UNSTRIP),
        counted(CAT),
        counted(PIPE),
    ];
    for (name, (before, after)) in [
        "strip_protocol",
        "unstrip_protocol",
        "cat_file",
        "pipe_file",
    ]
    .iter()
    .zip(before.iter().zip(after.iter()))
    {
        assert!(
            after > before,
            "{name} did not reach the backend through the ABI"
        );
    }
}
