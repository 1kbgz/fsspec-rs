//! Tests for the LocalFs backend, exercising all FileSystem trait
//! methods against the real filesystem using temporary directories.

use std::io::{Read, Seek, SeekFrom, Write};

use crate::error::FsError;
use crate::fs::FileSystem;
use crate::local::LocalFs;
use crate::types::{FileType, OpenMode};

/// Create a temporary directory and return its path string.
fn tmp_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("failed to create temp dir")
}

/// Helper: join a temp dir path with a relative path.
fn join(base: &std::path::Path, rel: &str) -> String {
    base.join(rel).to_str().expect("non-UTF-8 path").to_string()
}

// ======================================================================
// Construction
// ======================================================================

#[test]
fn test_local_fs_new() {
    let fs = LocalFs::new();
    assert!(!fs.auto_mkdir);
}

#[test]
fn test_local_fs_default() {
    let fs = LocalFs::default();
    assert!(!fs.auto_mkdir);
}

#[test]
fn test_local_fs_with_auto_mkdir() {
    let fs = LocalFs::with_auto_mkdir(true);
    assert!(fs.auto_mkdir);
}

// ======================================================================
// Protocol / metadata
// ======================================================================

#[test]
fn test_protocol() {
    let fs = LocalFs::new();
    let protos = fs.protocol();
    assert!(protos.contains(&"file"));
    assert!(protos.contains(&"local"));
}

#[test]
fn test_root_marker() {
    let fs = LocalFs::new();
    assert_eq!(fs.root_marker(), "/");
}

#[test]
fn test_sep() {
    let fs = LocalFs::new();
    assert_eq!(fs.sep(), std::path::MAIN_SEPARATOR_STR);
}

#[test]
fn test_strip_protocol() {
    let fs = LocalFs::new();
    assert_eq!(fs.strip_protocol("file:///tmp/foo"), "/tmp/foo");
    assert_eq!(fs.strip_protocol("local:///tmp/foo"), "/tmp/foo");
    assert_eq!(fs.strip_protocol("/tmp/foo"), "/tmp/foo");
}

#[test]
fn test_unstrip_protocol() {
    let fs = LocalFs::new();
    assert_eq!(fs.unstrip_protocol("/tmp/foo"), "file:///tmp/foo");
}

#[test]
fn test_parent() {
    let fs = LocalFs::new();
    let sep = std::path::MAIN_SEPARATOR_STR;
    let grandchild = format!("{sep}tmp{sep}foo{sep}bar");
    let parent_of_gc = format!("{sep}tmp{sep}foo");
    let child = format!("{sep}tmp");
    let root = fs.root_marker();
    assert_eq!(fs.parent(&grandchild), parent_of_gc);
    assert_eq!(fs.parent(&child), root);
    assert_eq!(fs.parent(root), root);
}

// ======================================================================
// info
// ======================================================================

#[test]
fn test_info_file() {
    let dir = tmp_dir();
    let path = join(dir.path(), "test.txt");
    std::fs::write(&path, "hello").unwrap();

    let fs = LocalFs::new();
    let info = fs.info(&path).unwrap();
    assert_eq!(info.name, path);
    assert_eq!(info.size, 5);
    assert_eq!(info.file_type, FileType::File);
    assert!(info.is_file());
    assert!(!info.is_dir());
    assert!(info.created.is_some() || info.modified.is_some());
}

#[test]
fn test_info_directory() {
    let dir = tmp_dir();
    let subdir = join(dir.path(), "subdir");
    std::fs::create_dir(&subdir).unwrap();

    let fs = LocalFs::new();
    let info = fs.info(&subdir).unwrap();
    assert_eq!(info.file_type, FileType::Directory);
    assert!(info.is_dir());
    assert!(!info.is_file());
}

#[test]
fn test_info_not_found() {
    let fs = LocalFs::new();
    let result = fs.info("/tmp/definitely_does_not_exist_fsspec_rs_test");
    assert!(matches!(result, Err(FsError::NotFound(_))));
}

// ======================================================================
// ls
// ======================================================================

#[test]
fn test_ls_empty_dir() {
    let dir = tmp_dir();
    let fs = LocalFs::new();
    let entries = fs.ls(dir.path().to_str().unwrap(), true).unwrap();
    assert!(entries.is_empty());
}

#[test]
fn test_ls_with_files() {
    let dir = tmp_dir();
    std::fs::write(dir.path().join("a.txt"), "aaa").unwrap();
    std::fs::write(dir.path().join("b.txt"), "bbbbb").unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();

    let fs = LocalFs::new();
    let entries = fs.ls(dir.path().to_str().unwrap(), true).unwrap();
    assert_eq!(entries.len(), 3);

    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.iter().any(|n| n.ends_with("a.txt")));
    assert!(names.iter().any(|n| n.ends_with("b.txt")));
    assert!(names.iter().any(|n| n.ends_with("sub")));
}

#[test]
fn test_ls_sorted() {
    let dir = tmp_dir();
    std::fs::write(dir.path().join("c.txt"), "c").unwrap();
    std::fs::write(dir.path().join("a.txt"), "a").unwrap();
    std::fs::write(dir.path().join("b.txt"), "b").unwrap();

    let fs = LocalFs::new();
    let entries = fs.ls(dir.path().to_str().unwrap(), true).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    // Verify they are sorted
    for window in names.windows(2) {
        assert!(window[0] <= window[1]);
    }
}

#[test]
fn test_ls_not_found() {
    let fs = LocalFs::new();
    let result = fs.ls("/tmp/definitely_does_not_exist_fsspec_rs_test", true);
    assert!(matches!(result, Err(FsError::NotFound(_))));
}

#[test]
fn test_ls_not_a_directory() {
    let dir = tmp_dir();
    let path = join(dir.path(), "file.txt");
    std::fs::write(&path, "data").unwrap();

    let fs = LocalFs::new();
    let result = fs.ls(&path, true);
    assert!(matches!(result, Err(FsError::NotADirectory(_))));
}

// ======================================================================
// exists / isdir / isfile
// ======================================================================

#[test]
fn test_exists() {
    let dir = tmp_dir();
    let path = join(dir.path(), "exists.txt");
    std::fs::write(&path, "data").unwrap();

    let fs = LocalFs::new();
    assert!(fs.exists(&path).unwrap());
    assert!(!fs.exists(&join(dir.path(), "nope.txt")).unwrap());
}

#[test]
fn test_isdir() {
    let dir = tmp_dir();
    let subdir = join(dir.path(), "sub");
    std::fs::create_dir(&subdir).unwrap();
    let file = join(dir.path(), "file.txt");
    std::fs::write(&file, "d").unwrap();

    let fs = LocalFs::new();
    assert!(fs.isdir(&subdir).unwrap());
    assert!(!fs.isdir(&file).unwrap());
    assert!(!fs.isdir(&join(dir.path(), "nope")).unwrap());
}

#[test]
fn test_isfile() {
    let dir = tmp_dir();
    let file = join(dir.path(), "file.txt");
    std::fs::write(&file, "d").unwrap();
    let subdir = join(dir.path(), "sub");
    std::fs::create_dir(&subdir).unwrap();

    let fs = LocalFs::new();
    assert!(fs.isfile(&file).unwrap());
    assert!(!fs.isfile(&subdir).unwrap());
    assert!(!fs.isfile(&join(dir.path(), "nope")).unwrap());
}

// ======================================================================
// size / sizes
// ======================================================================

#[test]
fn test_size() {
    let dir = tmp_dir();
    let path = join(dir.path(), "sized.txt");
    std::fs::write(&path, "hello").unwrap();

    let fs = LocalFs::new();
    assert_eq!(fs.size(&path).unwrap(), 5);
}

#[test]
fn test_sizes() {
    let dir = tmp_dir();
    let a = join(dir.path(), "a.txt");
    let b = join(dir.path(), "b.txt");
    std::fs::write(&a, "hi").unwrap();
    std::fs::write(&b, "hello").unwrap();

    let fs = LocalFs::new();
    let sizes = fs.sizes(&[a.as_str(), b.as_str()]).unwrap();
    assert_eq!(sizes, vec![2, 5]);
}

// ======================================================================
// mkdir / rmdir / makedirs
// ======================================================================

#[test]
fn test_mkdir() {
    let dir = tmp_dir();
    let subdir = join(dir.path(), "newdir");

    let fs = LocalFs::new();
    fs.mkdir(&subdir, false).unwrap();
    assert!(std::path::Path::new(&subdir).is_dir());
}

#[test]
fn test_mkdir_create_parents() {
    let dir = tmp_dir();
    let deep = join(dir.path(), "a/b/c");

    let fs = LocalFs::new();
    fs.mkdir(&deep, true).unwrap();
    assert!(std::path::Path::new(&deep).is_dir());
}

#[test]
fn test_mkdir_already_exists() {
    let dir = tmp_dir();
    let subdir = join(dir.path(), "existing");
    std::fs::create_dir(&subdir).unwrap();

    let fs = LocalFs::new();
    let result = fs.mkdir(&subdir, false);
    assert!(matches!(result, Err(FsError::AlreadyExists(_))));
}

#[test]
fn test_rmdir() {
    let dir = tmp_dir();
    let subdir = join(dir.path(), "todelete");
    std::fs::create_dir(&subdir).unwrap();

    let fs = LocalFs::new();
    fs.rmdir(&subdir).unwrap();
    assert!(!std::path::Path::new(&subdir).exists());
}

#[test]
fn test_rmdir_not_found() {
    let fs = LocalFs::new();
    let result = fs.rmdir("/tmp/definitely_does_not_exist_fsspec_rs_test");
    assert!(matches!(result, Err(FsError::NotFound(_))));
}

#[test]
fn test_rmdir_not_a_directory() {
    let dir = tmp_dir();
    let path = join(dir.path(), "file.txt");
    std::fs::write(&path, "d").unwrap();

    let fs = LocalFs::new();
    let result = fs.rmdir(&path);
    assert!(matches!(result, Err(FsError::NotADirectory(_))));
}

#[test]
fn test_makedirs_exist_ok() {
    let dir = tmp_dir();
    let subdir = join(dir.path(), "existing");
    std::fs::create_dir(&subdir).unwrap();

    let fs = LocalFs::new();
    // Should not error with exist_ok=true
    fs.makedirs(&subdir, true).unwrap();
}

// ======================================================================
// rm_file / rm
// ======================================================================

#[test]
fn test_rm_file() {
    let dir = tmp_dir();
    let path = join(dir.path(), "todelete.txt");
    std::fs::write(&path, "data").unwrap();

    let fs = LocalFs::new();
    fs.rm_file(&path).unwrap();
    assert!(!std::path::Path::new(&path).exists());
}

#[test]
fn test_rm_file_not_found() {
    let fs = LocalFs::new();
    let result = fs.rm_file("/tmp/definitely_not_here_fsspec_rs");
    assert!(matches!(result, Err(FsError::NotFound(_))));
}

#[test]
fn test_rm_file_is_directory() {
    let dir = tmp_dir();
    let subdir = join(dir.path(), "subdir");
    std::fs::create_dir(&subdir).unwrap();

    let fs = LocalFs::new();
    let result = fs.rm_file(&subdir);
    assert!(matches!(result, Err(FsError::IsADirectory(_))));
}

#[test]
fn test_rm_recursive() {
    let dir = tmp_dir();
    let root = join(dir.path(), "root");
    std::fs::create_dir_all(format!("{root}/sub")).unwrap();
    std::fs::write(format!("{root}/a.txt"), "a").unwrap();
    std::fs::write(format!("{root}/sub/b.txt"), "b").unwrap();

    let fs = LocalFs::new();
    fs.rm(&root, true).unwrap();
    assert!(!std::path::Path::new(&root).exists());
}

#[test]
fn test_rm_non_recursive_dir_errors() {
    let dir = tmp_dir();
    let subdir = join(dir.path(), "sub");
    std::fs::create_dir(&subdir).unwrap();
    std::fs::write(format!("{subdir}/file.txt"), "d").unwrap();

    let fs = LocalFs::new();
    let result = fs.rm(&subdir, false);
    assert!(matches!(result, Err(FsError::IsADirectory(_))));
}

#[test]
fn test_rm_single_file() {
    let dir = tmp_dir();
    let path = join(dir.path(), "file.txt");
    std::fs::write(&path, "data").unwrap();

    let fs = LocalFs::new();
    fs.rm(&path, false).unwrap();
    assert!(!std::path::Path::new(&path).exists());
}

// ======================================================================
// cp_file / copy
// ======================================================================

#[test]
fn test_cp_file() {
    let dir = tmp_dir();
    let src = join(dir.path(), "src.txt");
    let dst = join(dir.path(), "dst.txt");
    std::fs::write(&src, "copied data").unwrap();

    let fs = LocalFs::new();
    fs.cp_file(&src, &dst).unwrap();
    assert_eq!(std::fs::read_to_string(&dst).unwrap(), "copied data");
}

#[test]
fn test_cp_file_not_found() {
    let dir = tmp_dir();
    let fs = LocalFs::new();
    let result = fs.cp_file(&join(dir.path(), "nope"), &join(dir.path(), "dst"));
    assert!(matches!(result, Err(FsError::NotFound(_))));
}

#[test]
fn test_copy_recursive() {
    let dir = tmp_dir();
    let src = join(dir.path(), "src");
    let dst = join(dir.path(), "dst");
    std::fs::create_dir_all(format!("{src}/sub")).unwrap();
    std::fs::write(format!("{src}/a.txt"), "aa").unwrap();
    std::fs::write(format!("{src}/sub/b.txt"), "bb").unwrap();

    let fs = LocalFs::with_auto_mkdir(true);
    fs.mkdir(&dst, true).unwrap();
    fs.copy(&src, &dst, true).unwrap();

    assert_eq!(
        std::fs::read_to_string(format!("{dst}/a.txt")).unwrap(),
        "aa"
    );
    assert_eq!(
        std::fs::read_to_string(format!("{dst}/sub/b.txt")).unwrap(),
        "bb"
    );
}

// ======================================================================
// mv
// ======================================================================

#[test]
fn test_mv_file() {
    let dir = tmp_dir();
    let src = join(dir.path(), "src.txt");
    let dst = join(dir.path(), "dst.txt");
    std::fs::write(&src, "moved data").unwrap();

    let fs = LocalFs::new();
    fs.mv(&src, &dst).unwrap();
    assert!(!std::path::Path::new(&src).exists());
    assert_eq!(std::fs::read_to_string(&dst).unwrap(), "moved data");
}

// ======================================================================
// open / read / write / seek
// ======================================================================

#[test]
fn test_open_read() {
    let dir = tmp_dir();
    let path = join(dir.path(), "read.txt");
    std::fs::write(&path, "hello world").unwrap();

    let fs = LocalFs::new();
    let mut f = fs.open(&path, OpenMode::Read, None).unwrap();
    let mut buf = String::new();
    f.read_to_string(&mut buf).unwrap();
    assert_eq!(buf, "hello world");
}

#[test]
fn test_open_read_not_found() {
    let dir = tmp_dir();
    let fs = LocalFs::new();
    let result = fs.open(&join(dir.path(), "nope"), OpenMode::Read, None);
    assert!(matches!(result, Err(FsError::NotFound(_))));
}

#[test]
fn test_open_write() {
    let dir = tmp_dir();
    let path = join(dir.path(), "write.txt");

    let fs = LocalFs::new();
    {
        let mut f = fs.open(&path, OpenMode::Write, None).unwrap();
        f.write_all(b"written").unwrap();
        f.flush().unwrap();
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "written");
}

#[test]
fn test_open_write_commit() {
    let dir = tmp_dir();
    let path = join(dir.path(), "commit.txt");

    let fs = LocalFs::new();
    {
        let mut f = fs.open(&path, OpenMode::Write, None).unwrap();
        f.write_all(b"committed").unwrap();
        f.commit().unwrap();
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "committed");
}

#[test]
fn test_open_write_discard() {
    let dir = tmp_dir();
    let path = join(dir.path(), "discard.txt");
    std::fs::write(&path, "original").unwrap();

    let fs = LocalFs::new();
    {
        let mut f = fs.open(&path, OpenMode::Write, None).unwrap();
        f.write_all(b"REPLACED").unwrap();
        f.discard().unwrap();
        // On drop, committed=true so nothing is written
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
}

#[test]
fn test_open_append() {
    let dir = tmp_dir();
    let path = join(dir.path(), "append.txt");
    std::fs::write(&path, "hello").unwrap();

    let fs = LocalFs::new();
    {
        let mut f = fs.open(&path, OpenMode::Append, None).unwrap();
        f.write_all(b" world").unwrap();
        f.flush().unwrap();
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world");
}

#[test]
fn test_open_exclusive() {
    let dir = tmp_dir();
    let path = join(dir.path(), "exclusive.txt");

    let fs = LocalFs::new();
    {
        let mut f = fs.open(&path, OpenMode::Exclusive, None).unwrap();
        f.write_all(b"exclusive").unwrap();
        f.flush().unwrap();
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "exclusive");
}

#[test]
fn test_open_exclusive_already_exists() {
    let dir = tmp_dir();
    let path = join(dir.path(), "exists.txt");
    std::fs::write(&path, "data").unwrap();

    let fs = LocalFs::new();
    let result = fs.open(&path, OpenMode::Exclusive, None);
    assert!(matches!(result, Err(FsError::AlreadyExists(_))));
}

#[test]
fn test_open_seek() {
    let dir = tmp_dir();
    let path = join(dir.path(), "seek.txt");
    std::fs::write(&path, "hello world").unwrap();

    let fs = LocalFs::new();
    let mut f = fs.open(&path, OpenMode::Read, None).unwrap();
    f.seek(SeekFrom::Start(6)).unwrap();
    let mut buf = String::new();
    f.read_to_string(&mut buf).unwrap();
    assert_eq!(buf, "world");
}

// ======================================================================
// file info / size from open file
// ======================================================================

#[test]
fn test_file_info_from_open() {
    let dir = tmp_dir();
    let path = join(dir.path(), "info.txt");
    std::fs::write(&path, "hello").unwrap();

    let fs = LocalFs::new();
    let f = fs.open(&path, OpenMode::Read, None).unwrap();
    let info = f.info().unwrap();
    assert_eq!(info.name, path);
    assert_eq!(info.size, 5);
}

#[test]
fn test_file_size_from_open() {
    let dir = tmp_dir();
    let path = join(dir.path(), "size.txt");
    std::fs::write(&path, "hello").unwrap();

    let fs = LocalFs::new();
    let f = fs.open(&path, OpenMode::Read, None).unwrap();
    assert_eq!(f.size().unwrap(), Some(5));
}

// ======================================================================
// cat_file / pipe_file / head / tail
// ======================================================================

#[test]
fn test_cat_file_full() {
    let dir = tmp_dir();
    let path = join(dir.path(), "cat.txt");
    std::fs::write(&path, "hello world").unwrap();

    let fs = LocalFs::new();
    let data = fs.cat_file(&path, None, None).unwrap();
    assert_eq!(data, b"hello world");
}

#[test]
fn test_cat_file_range() {
    let dir = tmp_dir();
    let path = join(dir.path(), "cat_range.txt");
    std::fs::write(&path, "hello world").unwrap();

    let fs = LocalFs::new();
    let data = fs.cat_file(&path, Some(0), Some(5)).unwrap();
    assert_eq!(data, b"hello");
}

#[test]
fn test_cat_file_negative() {
    let dir = tmp_dir();
    let path = join(dir.path(), "cat_neg.txt");
    std::fs::write(&path, "hello world").unwrap();

    let fs = LocalFs::new();
    let data = fs.cat_file(&path, Some(-5), None).unwrap();
    assert_eq!(data, b"world");
}

#[test]
fn test_pipe_file() {
    let dir = tmp_dir();
    let path = join(dir.path(), "piped.txt");

    let fs = LocalFs::new();
    fs.pipe_file(&path, b"piped data").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "piped data");
}

#[test]
fn test_head() {
    let dir = tmp_dir();
    let path = join(dir.path(), "head.txt");
    std::fs::write(&path, "hello world").unwrap();

    let fs = LocalFs::new();
    let data = fs.head(&path, 5).unwrap();
    assert_eq!(data, b"hello");
}

#[test]
fn test_tail() {
    let dir = tmp_dir();
    let path = join(dir.path(), "tail.txt");
    std::fs::write(&path, "hello world").unwrap();

    let fs = LocalFs::new();
    let data = fs.tail(&path, 5).unwrap();
    assert_eq!(data, b"world");
}

// ======================================================================
// touch
// ======================================================================

#[test]
fn test_touch_create() {
    let dir = tmp_dir();
    let path = join(dir.path(), "touched.txt");

    let fs = LocalFs::new();
    fs.touch(&path, false).unwrap();
    assert!(std::path::Path::new(&path).exists());
    assert_eq!(fs.size(&path).unwrap(), 0);
}

#[test]
fn test_touch_truncate() {
    let dir = tmp_dir();
    let path = join(dir.path(), "touch_trunc.txt");
    std::fs::write(&path, "some data").unwrap();

    let fs = LocalFs::new();
    fs.touch(&path, true).unwrap();
    assert_eq!(fs.size(&path).unwrap(), 0);
}

// ======================================================================
// read_text / write_text
// ======================================================================

#[test]
fn test_read_text() {
    let dir = tmp_dir();
    let path = join(dir.path(), "read.txt");
    std::fs::write(&path, "hello world").unwrap();

    let fs = LocalFs::new();
    assert_eq!(fs.read_text(&path).unwrap(), "hello world");
}

#[test]
fn test_write_text() {
    let dir = tmp_dir();
    let path = join(dir.path(), "write.txt");

    let fs = LocalFs::new();
    fs.write_text(&path, "hello from rust").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello from rust");
}

// ======================================================================
// walk / find
// ======================================================================

#[test]
fn test_walk() {
    let dir = tmp_dir();
    let root = join(dir.path(), "walkroot");
    std::fs::create_dir_all(format!("{root}/sub1/sub2")).unwrap();
    std::fs::write(format!("{root}/a.txt"), "a").unwrap();
    std::fs::write(format!("{root}/sub1/b.txt"), "b").unwrap();
    std::fs::write(format!("{root}/sub1/sub2/c.txt"), "c").unwrap();

    let fs = LocalFs::new();
    let walk = fs.walk(&root, None, true).unwrap();

    // Should have 3 entries: root, sub1, sub2
    assert_eq!(walk.len(), 3);
    assert_eq!(walk[0].dirpath, root);
    assert!(walk[0].filenames.contains(&"a.txt".to_string()));
    assert!(walk[0].dirnames.contains(&"sub1".to_string()));
}

#[test]
fn test_walk_max_depth() {
    let dir = tmp_dir();
    let root = join(dir.path(), "depth");
    std::fs::create_dir_all(format!("{root}/sub1/sub2")).unwrap();
    std::fs::write(format!("{root}/a.txt"), "a").unwrap();
    std::fs::write(format!("{root}/sub1/b.txt"), "b").unwrap();
    std::fs::write(format!("{root}/sub1/sub2/c.txt"), "c").unwrap();

    let fs = LocalFs::new();
    let walk = fs.walk(&root, Some(1), true).unwrap();
    assert_eq!(walk.len(), 1);
    assert_eq!(walk[0].dirpath, root);
}

#[test]
fn test_find() {
    let dir = tmp_dir();
    let root = join(dir.path(), "findroot");
    std::fs::create_dir_all(format!("{root}/sub")).unwrap();
    std::fs::write(format!("{root}/a.txt"), "a").unwrap();
    std::fs::write(format!("{root}/sub/b.txt"), "b").unwrap();
    std::fs::write(format!("{root}/sub/c.txt"), "c").unwrap();

    let fs = LocalFs::new();
    let found = fs.find(&root, None, false).unwrap();
    assert_eq!(found.len(), 3);
    assert!(found.iter().any(|f| f.ends_with("a.txt")));
    assert!(found.iter().any(|f| f.ends_with("b.txt")));
    assert!(found.iter().any(|f| f.ends_with("c.txt")));
}

#[test]
fn test_find_with_dirs() {
    let dir = tmp_dir();
    let root = join(dir.path(), "findwd");
    std::fs::create_dir_all(format!("{root}/sub")).unwrap();
    std::fs::write(format!("{root}/a.txt"), "a").unwrap();
    std::fs::write(format!("{root}/sub/b.txt"), "b").unwrap();

    let fs = LocalFs::new();
    let found = fs.find(&root, None, true).unwrap();
    assert!(found.iter().any(|f| f.ends_with("sub")));
}

// ======================================================================
// du
// ======================================================================

#[test]
fn test_du_total() {
    let dir = tmp_dir();
    let root = join(dir.path(), "du_total");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(format!("{root}/a.txt"), "aaa").unwrap();
    std::fs::write(format!("{root}/b.txt"), "bb").unwrap();

    let fs = LocalFs::new();
    match fs.du(&root, true).unwrap() {
        crate::types::DuResult::Total(total) => assert_eq!(total, 5),
        _ => panic!("expected Total"),
    }
}

#[test]
fn test_du_per_path() {
    let dir = tmp_dir();
    let root = join(dir.path(), "du_per");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(format!("{root}/a.txt"), "aaa").unwrap();
    std::fs::write(format!("{root}/b.txt"), "bb").unwrap();

    let fs = LocalFs::new();
    match fs.du(&root, false).unwrap() {
        crate::types::DuResult::PerPath(map) => {
            let a_val = map.iter().find(|(k, _)| k.ends_with("a.txt")).unwrap().1;
            let b_val = map.iter().find(|(k, _)| k.ends_with("b.txt")).unwrap().1;
            assert_eq!(*a_val, 3);
            assert_eq!(*b_val, 2);
        }
        _ => panic!("expected PerPath"),
    }
}

// ======================================================================
// get_file / put_file
// ======================================================================

#[test]
fn test_get_file() {
    let dir = tmp_dir();
    let remote = join(dir.path(), "remote.txt");
    let local_dst = join(dir.path(), "local_copy.txt");
    std::fs::write(&remote, "remote data").unwrap();

    let fs = LocalFs::new();
    fs.get_file(&remote, &local_dst).unwrap();
    assert_eq!(std::fs::read_to_string(&local_dst).unwrap(), "remote data");
}

#[test]
fn test_put_file() {
    let dir = tmp_dir();
    let local_src = join(dir.path(), "local_src.txt");
    let remote_dst = join(dir.path(), "remote_dst.txt");
    std::fs::write(&local_src, "local data").unwrap();

    let fs = LocalFs::new();
    fs.put_file(&local_src, &remote_dst).unwrap();
    assert_eq!(std::fs::read_to_string(&remote_dst).unwrap(), "local data");
}

// ======================================================================
// auto_mkdir behavior
// ======================================================================

#[test]
fn test_auto_mkdir_on_write() {
    let dir = tmp_dir();
    let path = join(dir.path(), "deep/nested/write.txt");

    let fs = LocalFs::with_auto_mkdir(true);
    {
        let mut f = fs.open(&path, OpenMode::Write, None).unwrap();
        f.write_all(b"deep write").unwrap();
        f.flush().unwrap();
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "deep write");
}

#[test]
fn test_auto_mkdir_on_cp_file() {
    let dir = tmp_dir();
    let src = join(dir.path(), "src.txt");
    let dst = join(dir.path(), "deep/nested/dst.txt");
    std::fs::write(&src, "data").unwrap();

    let fs = LocalFs::with_auto_mkdir(true);
    fs.cp_file(&src, &dst).unwrap();
    assert_eq!(std::fs::read_to_string(&dst).unwrap(), "data");
}
