//! Tests for the MemoryFs backend.

use std::io::{Read, Write};
use std::sync::Arc;

use crate::error::FsError;
use crate::fs::FileSystem;
use crate::memory::MemoryFs;
use crate::registry::{FilesystemRegistry, FsOptions};
use crate::types::{FileType, OpenMode};

fn names(fs: &MemoryFs, path: &str) -> Vec<String> {
    fs.ls(path, false)
        .unwrap()
        .into_iter()
        .map(|info| info.name)
        .collect()
}

#[test]
fn writes_imply_parent_directories() {
    let fs = MemoryFs::new();
    fs.pipe_file("a/b/c.txt", b"hello").unwrap();
    fs.pipe_file("/a/d.txt", b"hi").unwrap();

    assert_eq!(names(&fs, "/"), ["/a"]);
    assert_eq!(names(&fs, "a"), ["/a/b", "/a/d.txt"]);
    assert_eq!(fs.info("/a/b").unwrap().file_type, FileType::Directory);
    let file = fs.info("a/b/c.txt").unwrap();
    assert_eq!((file.name.as_str(), file.size), ("/a/b/c.txt", 5));
    assert!(file.modified.is_some());
    assert_eq!(fs.cat_file("/a/b/c.txt", Some(1), Some(3)).unwrap(), b"el");

    fs.rm_file("/a/b/c.txt").unwrap();
    assert!(!fs.exists("/a/b").unwrap());
}

#[test]
fn mkdir_creates_empty_directories() {
    let fs = MemoryFs::new();
    fs.mkdir("/x/y", true).unwrap();
    assert_eq!(names(&fs, "/x"), ["/x/y"]);
    assert!(fs.ls("/x/y", true).unwrap().is_empty());
    assert!(matches!(
        fs.mkdir("/x/y", true),
        Err(FsError::AlreadyExists(_))
    ));
    assert!(matches!(fs.mkdir("/p/q", false), Err(FsError::NotFound(p)) if p == "/p"));
    fs.makedirs("/x/y", true).unwrap();

    fs.rmdir("/x/y").unwrap();
    assert!(matches!(fs.rmdir("/x/y"), Err(FsError::NotFound(_))));
    fs.pipe_file("/x/f", b"").unwrap();
    assert!(matches!(fs.rmdir("/x"), Err(FsError::IoError(_))));
}

#[test]
fn files_and_directories_do_not_overlap() {
    let fs = MemoryFs::new();
    fs.pipe_file("/f", b"data").unwrap();
    assert!(matches!(fs.pipe_file("/f/g", b""), Err(FsError::NotADirectory(p)) if p == "/f"));
    assert!(matches!(
        fs.mkdir("/f/g", true),
        Err(FsError::NotADirectory(_))
    ));
    assert!(matches!(fs.ls("/f", false), Err(FsError::NotADirectory(_))));
    assert!(matches!(
        fs.ls("/missing", false),
        Err(FsError::NotFound(_))
    ));

    fs.mkdir("/d", false).unwrap();
    assert!(matches!(
        fs.pipe_file("/d", b""),
        Err(FsError::IsADirectory(_))
    ));
    assert!(matches!(
        fs.open("/d", OpenMode::Read, None),
        Err(FsError::IsADirectory(_))
    ));
    assert!(matches!(fs.rm_file("/d"), Err(FsError::IsADirectory(_))));
    assert!(matches!(fs.rm_file("/missing"), Err(FsError::NotFound(_))));
}

#[test]
fn writes_become_visible_on_commit_or_drop() {
    let fs = MemoryFs::new();
    let mut file = fs.open("/w", OpenMode::Write, None).unwrap();
    file.write_all(b"partial").unwrap();
    assert!(!fs.exists("/w").unwrap());
    file.commit().unwrap();
    assert_eq!(fs.cat_file("/w", None, None).unwrap(), b"partial");

    let mut discarded = fs.open("/w", OpenMode::Write, None).unwrap();
    discarded.write_all(b"lost").unwrap();
    discarded.discard().unwrap();
    drop(discarded);
    assert_eq!(fs.cat_file("/w", None, None).unwrap(), b"partial");

    {
        let mut dropped = fs.open("/d", OpenMode::Write, None).unwrap();
        dropped.write_all(b"kept").unwrap();
    }
    assert_eq!(fs.cat_file("/d", None, None).unwrap(), b"kept");
}

#[test]
fn read_append_and_exclusive_modes() {
    let fs = MemoryFs::new();
    assert!(matches!(
        fs.open("/r", OpenMode::Read, None),
        Err(FsError::NotFound(_))
    ));
    fs.pipe_file("/r", b"abc").unwrap();

    let mut reader = fs.open("/r", OpenMode::Read, None).unwrap();
    fs.pipe_file("/r", b"replaced").unwrap();
    let mut seen = String::new();
    reader.read_to_string(&mut seen).unwrap();
    assert_eq!(seen, "abc");
    assert!(reader.write(b"x").is_err());

    let mut append = fs.open("/r", OpenMode::Append, None).unwrap();
    append.write_all(b"!").unwrap();
    append.commit().unwrap();
    assert_eq!(fs.cat_file("/r", None, None).unwrap(), b"replaced!");

    assert!(matches!(
        fs.open("/r", OpenMode::Exclusive, None),
        Err(FsError::AlreadyExists(_))
    ));
    let mut first = fs.open("/x", OpenMode::Exclusive, None).unwrap();
    fs.pipe_file("/x", b"raced").unwrap();
    first.write_all(b"mine").unwrap();
    assert!(matches!(first.commit(), Err(FsError::AlreadyExists(_))));
    first.discard().unwrap();
}

#[test]
fn copy_move_and_recursive_remove() {
    let fs = MemoryFs::new();
    fs.pipe_file("/src/a", b"1").unwrap();
    fs.pipe_file("/src/sub/b", b"2").unwrap();
    fs.mkdir("/src/empty", false).unwrap();

    fs.copy("/src", "/dst", true).unwrap();
    assert_eq!(
        fs.find("/dst", None, false).unwrap(),
        ["/dst/a", "/dst/sub/b"]
    );
    fs.mv("/dst/a", "/moved").unwrap();
    assert_eq!(fs.cat_file("/moved", None, None).unwrap(), b"1");
    assert!(!fs.exists("/dst/a").unwrap());

    assert!(matches!(
        fs.rm("/src", false),
        Err(FsError::IsADirectory(_))
    ));
    fs.rm("/src", true).unwrap();
    assert!(!fs.exists("/src").unwrap());
    assert!(!fs.exists("/src/empty").unwrap());
    assert_eq!(names(&fs, "/"), ["/dst", "/moved"]);
}

#[test]
fn clones_share_a_store() {
    let fs = MemoryFs::new();
    let other = fs.clone();
    fs.pipe_file("/shared", b"x").unwrap();
    assert!(other.exists("/shared").unwrap());
    assert!(!MemoryFs::new().exists("/shared").unwrap());
}

#[test]
fn strip_protocol_normalizes_paths() {
    let fs = MemoryFs::new();
    assert_eq!(fs.strip_protocol("memory://a/b/"), "/a/b");
    assert_eq!(fs.strip_protocol("memory://"), "/");
    assert_eq!(fs.strip_protocol("/a"), "/a");
}

#[test]
fn default_registry_shares_one_memory_store() {
    let registry = FilesystemRegistry::default();
    let (first, path) = registry
        .url_to_fs("memory://a/b", &FsOptions::new())
        .unwrap();
    assert_eq!(path, "/a/b");
    first.pipe_file(&path, b"x").unwrap();
    let (second, _) = registry
        .url_to_fs("memory://other", &FsOptions::new())
        .unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert!(second.exists("/a/b").unwrap());

    let mut options = FsOptions::new();
    options.insert("auto_mkdir".to_string(), "true".to_string());
    assert!(matches!(
        registry.url_to_fs("memory://a", &options),
        Err(FsError::InvalidArgument(_))
    ));
}
