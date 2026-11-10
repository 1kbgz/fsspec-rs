//! Tests run with `fsspec_capi` compiled in, so every call goes through the
//! C ABI and back without a shared library.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, MAIN_SEPARATOR};
use std::sync::Arc;

use fsspec_rs::{
    FileSystem, FileType, FsError, FsFile, FsOptions, LocalFs, OpenMode, SharedFileSystem,
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
