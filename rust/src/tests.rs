//! Tests for the FileSystem trait's default method implementations
//! using a mock in-memory filesystem.

use std::collections::HashMap;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::sync::{Arc, Mutex};

use crate::error::{FsError, FsResult};
use crate::file::FsFile;
use crate::fs::FileSystem;
use crate::types::{FileInfo, OpenMode, OpenOptions};

// ---- Mock FsFile backed by a shared byte buffer ----

struct MockFile {
    name: String,
    cursor: Cursor<Vec<u8>>,
    store: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    mode: OpenMode,
}

impl Read for MockFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.cursor.read(buf)
    }
}

impl Write for MockFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.cursor.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // Persist data back to store
        let data = self.cursor.get_ref().clone();
        let mut store = self.store.lock().unwrap();
        store.insert(self.name.clone(), data);
        Ok(())
    }
}

impl Seek for MockFile {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.cursor.seek(pos)
    }
}

impl FsFile for MockFile {
    fn info(&self) -> FsResult<FileInfo> {
        let store = self.store.lock().unwrap();
        let size = store.get(&self.name).map(|d| d.len() as u64).unwrap_or(0);
        Ok(FileInfo::file(&self.name, size))
    }

    /// Files under `/stream/` report an unknown size, like data encoded while it is read.
    fn size(&self) -> FsResult<Option<u64>> {
        if self.name.starts_with("/stream/") {
            return Ok(None);
        }
        let store = self.store.lock().unwrap();
        Ok(store.get(&self.name).map(|d| d.len() as u64))
    }
}

impl Drop for MockFile {
    fn drop(&mut self) {
        if self.mode != OpenMode::Read {
            let data = self.cursor.get_ref().clone();
            let mut store = self.store.lock().unwrap();
            store.insert(self.name.clone(), data);
        }
    }
}

// ---- Mock Filesystem (in-memory) ----

struct MockFs {
    store: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    dirs: Arc<Mutex<Vec<String>>>,
}

impl MockFs {
    fn new() -> Self {
        Self {
            store: Arc::new(Mutex::new(HashMap::new())),
            dirs: Arc::new(Mutex::new(vec!["/".to_string()])),
        }
    }

    /// Populate some test data.
    fn with_files(self, files: &[(&str, &[u8])]) -> Self {
        {
            let mut store = self.store.lock().unwrap();
            let mut dirs = self.dirs.lock().unwrap();
            for (path, data) in files {
                store.insert(path.to_string(), data.to_vec());
                // auto-create parent dirs
                let mut parent = self.parent_path(path);
                while !parent.is_empty() && !dirs.contains(&parent) {
                    dirs.push(parent.clone());
                    parent = self.parent_path(&parent);
                }
            }
        }
        self
    }

    fn parent_path(&self, path: &str) -> String {
        let path = path.trim_end_matches('/');
        match path.rfind('/') {
            Some(0) => "/".to_string(),
            Some(idx) => path[..idx].to_string(),
            None => String::new(),
        }
    }
}

impl FileSystem for MockFs {
    fn protocol(&self) -> &[&str] {
        &["mock"]
    }

    fn root_marker(&self) -> &str {
        "/"
    }

    fn ls(&self, path: &str, _detail: bool) -> FsResult<Vec<FileInfo>> {
        let store = self.store.lock().unwrap();
        let dirs = self.dirs.lock().unwrap();
        let path = path.trim_end_matches('/');

        // Check path exists as a dir
        if !dirs.contains(&path.to_string()) && path != "/" && !path.is_empty() {
            return Err(FsError::NotFound(format!("directory not found: {path}")));
        }

        let prefix = if path == "/" || path.is_empty() {
            "/".to_string()
        } else {
            format!("{path}/")
        };

        let mut results = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // Find direct children (files)
        for key in store.keys() {
            if let Some(rest) = key.strip_prefix(&prefix) {
                if !rest.contains('/') {
                    let info = FileInfo::file(key.clone(), store[key].len() as u64);
                    if seen.insert(key.clone()) {
                        results.push(info);
                    }
                } else {
                    // This is a nested file; its first component is a direct child dir
                    let child_dir_name = rest.split('/').next().unwrap();
                    let child_dir_path = format!("{prefix}{child_dir_name}");
                    if seen.insert(child_dir_path.clone()) {
                        results.push(FileInfo::directory(child_dir_path));
                    }
                }
            }
        }

        // Find direct child dirs that are in the dirs list
        for dir in dirs.iter() {
            if let Some(rest) = dir.strip_prefix(&prefix) {
                if !rest.is_empty() && !rest.contains('/') && seen.insert(dir.clone()) {
                    results.push(FileInfo::directory(dir.clone()));
                }
            }
        }

        results.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(results)
    }

    fn rm_file(&self, path: &str) -> FsResult<()> {
        let mut store = self.store.lock().unwrap();
        store
            .remove(path)
            .ok_or_else(|| FsError::NotFound(path.to_string()))?;
        Ok(())
    }

    fn cp_file(&self, src: &str, dst: &str) -> FsResult<()> {
        let store = self.store.lock().unwrap();
        let data = store
            .get(src)
            .ok_or_else(|| FsError::NotFound(src.to_string()))?
            .clone();
        drop(store);
        let mut store = self.store.lock().unwrap();
        store.insert(dst.to_string(), data);
        Ok(())
    }

    fn open(
        &self,
        path: &str,
        mode: OpenMode,
        _opts: Option<OpenOptions>,
    ) -> FsResult<Box<dyn FsFile>> {
        let store_ref = self.store.clone();
        match mode {
            OpenMode::Read => {
                let store = self.store.lock().unwrap();
                let data = store
                    .get(path)
                    .ok_or_else(|| FsError::NotFound(path.to_string()))?
                    .clone();
                Ok(Box::new(MockFile {
                    name: path.to_string(),
                    cursor: Cursor::new(data),
                    store: store_ref,
                    mode,
                }))
            }
            OpenMode::Write | OpenMode::Exclusive => {
                if mode == OpenMode::Exclusive {
                    let store = self.store.lock().unwrap();
                    if store.contains_key(path) {
                        return Err(FsError::AlreadyExists(path.to_string()));
                    }
                }
                Ok(Box::new(MockFile {
                    name: path.to_string(),
                    cursor: Cursor::new(Vec::new()),
                    store: store_ref,
                    mode,
                }))
            }
            OpenMode::Append => {
                let store = self.store.lock().unwrap();
                let existing = store.get(path).cloned().unwrap_or_default();
                let len = existing.len();
                let mut cursor = Cursor::new(existing);
                cursor.set_position(len as u64);
                Ok(Box::new(MockFile {
                    name: path.to_string(),
                    cursor,
                    store: store_ref,
                    mode,
                }))
            }
        }
    }

    fn info(&self, path: &str) -> FsResult<FileInfo> {
        let store = self.store.lock().unwrap();
        if let Some(data) = store.get(path) {
            return Ok(FileInfo::file(path, data.len() as u64));
        }
        let dirs = self.dirs.lock().unwrap();
        let path_clean = path.trim_end_matches('/');
        if dirs.contains(&path_clean.to_string()) || path_clean == "/" || path_clean.is_empty() {
            return Ok(FileInfo::directory(path_clean));
        }
        // Check if any stored file has this as a prefix (implicit dir)
        let prefix = format!("{path_clean}/");
        for key in store.keys() {
            if key.starts_with(&prefix) {
                return Ok(FileInfo::directory(path_clean));
            }
        }
        Err(FsError::NotFound(path.to_string()))
    }

    fn mkdir(&self, path: &str, create_parents: bool) -> FsResult<()> {
        let path = path.trim_end_matches('/');
        let mut dirs = self.dirs.lock().unwrap();
        if dirs.contains(&path.to_string()) {
            return Err(FsError::AlreadyExists(path.to_string()));
        }
        if create_parents {
            let mut current = String::new();
            for component in path.split('/').filter(|c| !c.is_empty()) {
                current = format!("{current}/{component}");
                if !dirs.contains(&current) {
                    dirs.push(current.clone());
                }
            }
        } else {
            let parent = self.parent_path(path);
            if parent != "/" && !dirs.contains(&parent) {
                return Err(FsError::NotFound(format!(
                    "parent directory not found: {parent}"
                )));
            }
            dirs.push(path.to_string());
        }
        Ok(())
    }

    fn rmdir(&self, path: &str) -> FsResult<()> {
        let path = path.trim_end_matches('/');
        let mut dirs = self.dirs.lock().unwrap();
        if let Some(pos) = dirs.iter().position(|d| d == path) {
            dirs.remove(pos);
            Ok(())
        } else {
            Err(FsError::NotFound(path.to_string()))
        }
    }
}

// ======================================================================
// Tests
// ======================================================================

#[test]
fn test_protocol() {
    let fs = MockFs::new();
    assert_eq!(fs.protocol(), &["mock"]);
}

#[test]
fn test_root_marker() {
    let fs = MockFs::new();
    assert_eq!(fs.root_marker(), "/");
}

#[test]
fn test_sep() {
    let fs = MockFs::new();
    assert_eq!(fs.sep(), "/");
}

#[test]
fn test_strip_protocol() {
    let fs = MockFs::new();
    assert_eq!(fs.strip_protocol("mock:///tmp/foo"), "/tmp/foo");
    assert_eq!(fs.strip_protocol("/tmp/foo"), "/tmp/foo");
}

#[test]
fn test_unstrip_protocol() {
    let fs = MockFs::new();
    assert_eq!(fs.unstrip_protocol("/tmp/foo"), "mock:///tmp/foo");
}

#[test]
fn test_parent() {
    let fs = MockFs::new();
    assert_eq!(fs.parent("/tmp/foo/bar"), "/tmp/foo");
    assert_eq!(fs.parent("/tmp"), "/");
    assert_eq!(fs.parent("/"), "/");
}

#[test]
fn test_ls() {
    let fs = MockFs::new().with_files(&[
        ("/tmp/a.txt", b"hello"),
        ("/tmp/b.txt", b"world"),
        ("/tmp/sub/c.txt", b"nested"),
    ]);
    let entries = fs.ls("/tmp", true).unwrap();
    assert_eq!(entries.len(), 3); // a.txt, b.txt, sub/
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"/tmp/a.txt"));
    assert!(names.contains(&"/tmp/b.txt"));
    assert!(names.contains(&"/tmp/sub"));
}

#[test]
fn test_ls_not_found() {
    let fs = MockFs::new();
    let result = fs.ls("/nonexistent", true);
    assert!(result.is_err());
}

#[test]
fn test_info_file() {
    let fs = MockFs::new().with_files(&[("/tmp/a.txt", b"hello")]);
    let info = fs.info("/tmp/a.txt").unwrap();
    assert_eq!(info.name, "/tmp/a.txt");
    assert_eq!(info.size, 5);
    assert!(info.is_file());
}

#[test]
fn test_info_dir() {
    let fs = MockFs::new().with_files(&[("/tmp/sub/a.txt", b"data")]);
    let info = fs.info("/tmp/sub").unwrap();
    assert!(info.is_dir());
}

#[test]
fn test_info_not_found() {
    let fs = MockFs::new();
    let result = fs.info("/nonexistent");
    assert!(matches!(result, Err(FsError::NotFound(_))));
}

#[test]
fn test_exists() {
    let fs = MockFs::new().with_files(&[("/tmp/a.txt", b"hello")]);
    assert!(fs.exists("/tmp/a.txt").unwrap());
    assert!(!fs.exists("/tmp/nope.txt").unwrap());
}

#[test]
fn test_isdir() {
    let fs = MockFs::new().with_files(&[("/tmp/sub/a.txt", b"data")]);
    assert!(fs.isdir("/tmp/sub").unwrap());
    assert!(!fs.isdir("/tmp/sub/a.txt").unwrap());
    assert!(!fs.isdir("/nonexistent").unwrap());
}

#[test]
fn test_isfile() {
    let fs = MockFs::new().with_files(&[("/tmp/a.txt", b"hello")]);
    assert!(fs.isfile("/tmp/a.txt").unwrap());
    assert!(!fs.isfile("/tmp").unwrap());
    assert!(!fs.isfile("/nonexistent").unwrap());
}

#[test]
fn test_size() {
    let fs = MockFs::new().with_files(&[("/tmp/a.txt", b"hello")]);
    assert_eq!(fs.size("/tmp/a.txt").unwrap(), 5);
}

#[test]
fn test_sizes() {
    let fs = MockFs::new().with_files(&[("/tmp/a.txt", b"hi"), ("/tmp/b.txt", b"hello")]);
    let sizes = fs.sizes(&["/tmp/a.txt", "/tmp/b.txt"]).unwrap();
    assert_eq!(sizes, vec![2, 5]);
}

#[test]
fn test_cat_file_full() {
    let fs = MockFs::new().with_files(&[("/tmp/a.txt", b"hello world")]);
    let data = fs.cat_file("/tmp/a.txt", None, None).unwrap();
    assert_eq!(data, b"hello world");
}

#[test]
fn test_cat_file_range() {
    let fs = MockFs::new().with_files(&[("/tmp/a.txt", b"hello world")]);
    let data = fs.cat_file("/tmp/a.txt", Some(0), Some(5)).unwrap();
    assert_eq!(data, b"hello");
}

#[test]
fn test_cat_file_negative_range() {
    let fs = MockFs::new().with_files(&[("/tmp/a.txt", b"hello world")]);
    let data = fs.cat_file("/tmp/a.txt", Some(-5), None).unwrap();
    assert_eq!(data, b"world");
}

#[test]
fn test_cat_file_unknown_size() {
    let fs = MockFs::new().with_files(&[("/stream/a.txt", b"hello world")]);
    let cat = |start, end| fs.cat_file("/stream/a.txt", start, end).unwrap();
    assert_eq!(cat(None, None), b"hello world");
    assert_eq!(cat(Some(-5), None), b"world");
    assert_eq!(cat(Some(2), Some(-6)), b"llo");
}

#[test]
fn test_cat_file_end_before_start() {
    let fs = MockFs::new().with_files(&[
        ("/tmp/a.txt", b"hello world"),
        ("/stream/a.txt", b"hello world"),
    ]);
    for path in ["/tmp/a.txt", "/stream/a.txt"] {
        assert!(fs.cat_file(path, Some(5), Some(2)).unwrap().is_empty());
        assert!(fs.cat_file(path, Some(20), None).unwrap().is_empty());
    }
}

#[test]
fn test_pipe_file() {
    let fs = MockFs::new();
    fs.mkdir("/tmp", true).unwrap();
    fs.pipe_file("/tmp/out.txt", b"written data").unwrap();
    let data = fs.cat_file("/tmp/out.txt", None, None).unwrap();
    assert_eq!(data, b"written data");
}

#[test]
fn test_head() {
    let fs = MockFs::new().with_files(&[("/tmp/a.txt", b"hello world")]);
    let data = fs.head("/tmp/a.txt", 5).unwrap();
    assert_eq!(data, b"hello");
}

#[test]
fn test_tail() {
    let fs = MockFs::new().with_files(&[("/tmp/a.txt", b"hello world")]);
    let data = fs.tail("/tmp/a.txt", 5).unwrap();
    assert_eq!(data, b"world");
}

#[test]
fn test_touch_creates_file() {
    let fs = MockFs::new();
    fs.mkdir("/tmp", true).unwrap();
    fs.touch("/tmp/new.txt", false).unwrap();
    assert!(fs.exists("/tmp/new.txt").unwrap());
    assert_eq!(fs.size("/tmp/new.txt").unwrap(), 0);
}

#[test]
fn test_rm_file() {
    let fs = MockFs::new().with_files(&[("/tmp/a.txt", b"data")]);
    assert!(fs.exists("/tmp/a.txt").unwrap());
    fs.rm_file("/tmp/a.txt").unwrap();
    assert!(!fs.exists("/tmp/a.txt").unwrap());
}

#[test]
fn test_rm_file_not_found() {
    let fs = MockFs::new();
    let result = fs.rm_file("/nonexistent");
    assert!(matches!(result, Err(FsError::NotFound(_))));
}

#[test]
fn test_cp_file() {
    let fs = MockFs::new().with_files(&[("/tmp/a.txt", b"copied data")]);
    fs.cp_file("/tmp/a.txt", "/tmp/b.txt").unwrap();
    let data = fs.cat_file("/tmp/b.txt", None, None).unwrap();
    assert_eq!(data, b"copied data");
}

#[test]
fn test_cp_file_not_found() {
    let fs = MockFs::new();
    let result = fs.cp_file("/nonexistent", "/dst");
    assert!(matches!(result, Err(FsError::NotFound(_))));
}

#[test]
fn test_mkdir() {
    let fs = MockFs::new();
    fs.mkdir("/tmp/newdir", true).unwrap();
    assert!(fs.isdir("/tmp/newdir").unwrap());
}

#[test]
fn test_mkdir_already_exists() {
    let fs = MockFs::new();
    fs.mkdir("/tmp/d", true).unwrap();
    let result = fs.mkdir("/tmp/d", true);
    assert!(matches!(result, Err(FsError::AlreadyExists(_))));
}

#[test]
fn test_makedirs_exist_ok() {
    let fs = MockFs::new();
    fs.mkdir("/tmp/d", true).unwrap();
    fs.makedirs("/tmp/d", true).unwrap(); // should not error
}

#[test]
fn test_rmdir() {
    let fs = MockFs::new();
    fs.mkdir("/tmp/d", true).unwrap();
    fs.rmdir("/tmp/d").unwrap();
    assert!(!fs.isdir("/tmp/d").unwrap());
}

#[test]
fn test_walk() {
    let fs = MockFs::new().with_files(&[
        ("/root/a.txt", b"a"),
        ("/root/sub1/b.txt", b"b"),
        ("/root/sub1/sub2/c.txt", b"c"),
    ]);
    let walk = fs.walk("/root", None, true).unwrap();

    // root, sub1, sub2
    assert_eq!(walk.len(), 3);
    assert_eq!(walk[0].dirpath, "/root");
    assert!(walk[0].filenames.contains(&"a.txt".to_string()));
    assert!(walk[0].dirnames.contains(&"sub1".to_string()));
}

#[test]
fn test_walk_max_depth() {
    let fs = MockFs::new().with_files(&[
        ("/root/a.txt", b"a"),
        ("/root/sub1/b.txt", b"b"),
        ("/root/sub1/sub2/c.txt", b"c"),
    ]);
    let walk = fs.walk("/root", Some(1), true).unwrap();
    assert_eq!(walk.len(), 1); // only the root level
    assert_eq!(walk[0].dirpath, "/root");
}

#[test]
fn test_find() {
    let fs = MockFs::new().with_files(&[
        ("/root/a.txt", b"a"),
        ("/root/sub/b.txt", b"b"),
        ("/root/sub/c.txt", b"c"),
    ]);
    let found = fs.find("/root", None, false).unwrap();
    assert_eq!(found.len(), 3);
    assert!(found.contains(&"/root/a.txt".to_string()));
    assert!(found.contains(&"/root/sub/b.txt".to_string()));
    assert!(found.contains(&"/root/sub/c.txt".to_string()));
}

#[test]
fn test_find_with_dirs() {
    let fs = MockFs::new().with_files(&[("/root/a.txt", b"a"), ("/root/sub/b.txt", b"b")]);
    let found = fs.find("/root", None, true).unwrap();
    assert!(found.contains(&"/root/sub".to_string()));
}

#[test]
fn test_copy_recursive() {
    let fs = MockFs::new().with_files(&[("/src/a.txt", b"aa"), ("/src/sub/b.txt", b"bb")]);
    fs.mkdir("/dst", true).unwrap();
    fs.copy("/src", "/dst", true).unwrap();
    assert_eq!(fs.cat_file("/dst/a.txt", None, None).unwrap(), b"aa");
    assert_eq!(fs.cat_file("/dst/sub/b.txt", None, None).unwrap(), b"bb");
}

#[test]
fn test_mv() {
    let fs = MockFs::new().with_files(&[("/tmp/a.txt", b"data")]);
    fs.mv("/tmp/a.txt", "/tmp/b.txt").unwrap();
    assert!(!fs.exists("/tmp/a.txt").unwrap());
    assert_eq!(fs.cat_file("/tmp/b.txt", None, None).unwrap(), b"data");
}

#[test]
fn test_rm_recursive() {
    let fs = MockFs::new().with_files(&[("/rm_test/a.txt", b"a"), ("/rm_test/sub/b.txt", b"b")]);
    fs.rm("/rm_test", true).unwrap();
    assert!(!fs.exists("/rm_test").unwrap());
}

#[test]
fn test_rm_non_recursive_file() {
    let fs = MockFs::new().with_files(&[("/tmp/a.txt", b"data")]);
    fs.rm("/tmp/a.txt", false).unwrap();
    assert!(!fs.exists("/tmp/a.txt").unwrap());
}

#[test]
fn test_rm_non_recursive_dir_errors() {
    let fs = MockFs::new().with_files(&[("/tmp/sub/a.txt", b"data")]);
    let result = fs.rm("/tmp/sub", false);
    assert!(matches!(result, Err(FsError::IsADirectory(_))));
}

#[test]
fn test_du_total() {
    let fs = MockFs::new().with_files(&[("/du/a.txt", b"aaa"), ("/du/b.txt", b"bb")]);
    match fs.du("/du", true).unwrap() {
        crate::types::DuResult::Total(total) => assert_eq!(total, 5),
        _ => panic!("expected Total"),
    }
}

#[test]
fn test_du_per_path() {
    let fs = MockFs::new().with_files(&[("/du/a.txt", b"aaa"), ("/du/b.txt", b"bb")]);
    match fs.du("/du", false).unwrap() {
        crate::types::DuResult::PerPath(map) => {
            assert_eq!(map["/du/a.txt"], 3);
            assert_eq!(map["/du/b.txt"], 2);
        }
        _ => panic!("expected PerPath"),
    }
}

#[test]
fn test_read_text() {
    let fs = MockFs::new().with_files(&[("/tmp/hello.txt", b"hello world")]);
    let text = fs.read_text("/tmp/hello.txt").unwrap();
    assert_eq!(text, "hello world");
}

#[test]
fn test_write_text() {
    let fs = MockFs::new();
    fs.mkdir("/tmp", true).unwrap();
    fs.write_text("/tmp/out.txt", "hello world").unwrap();
    let text = fs.read_text("/tmp/out.txt").unwrap();
    assert_eq!(text, "hello world");
}

#[test]
fn test_open_read() {
    let fs = MockFs::new().with_files(&[("/tmp/a.txt", b"hello")]);
    let mut f = fs.open("/tmp/a.txt", OpenMode::Read, None).unwrap();
    let mut buf = String::new();
    f.read_to_string(&mut buf).unwrap();
    assert_eq!(buf, "hello");
}

#[test]
fn test_open_write() {
    let fs = MockFs::new();
    fs.mkdir("/tmp", true).unwrap();
    {
        let mut f = fs.open("/tmp/out.txt", OpenMode::Write, None).unwrap();
        f.write_all(b"written").unwrap();
        f.flush().unwrap();
    }
    let data = fs.cat_file("/tmp/out.txt", None, None).unwrap();
    assert_eq!(data, b"written");
}

#[test]
fn test_open_exclusive() {
    let fs = MockFs::new().with_files(&[("/tmp/exist.txt", b"data")]);
    let result = fs.open("/tmp/exist.txt", OpenMode::Exclusive, None);
    assert!(matches!(result, Err(FsError::AlreadyExists(_))));
}

#[test]
fn test_open_append() {
    let fs = MockFs::new().with_files(&[("/tmp/a.txt", b"hello")]);
    {
        let mut f = fs.open("/tmp/a.txt", OpenMode::Append, None).unwrap();
        f.write_all(b" world").unwrap();
        f.flush().unwrap();
    }
    let data = fs.cat_file("/tmp/a.txt", None, None).unwrap();
    assert_eq!(data, b"hello world");
}

#[test]
fn test_file_info_from_open() {
    let fs = MockFs::new().with_files(&[("/tmp/a.txt", b"hello")]);
    let f = fs.open("/tmp/a.txt", OpenMode::Read, None).unwrap();
    let info = f.info().unwrap();
    assert_eq!(info.name, "/tmp/a.txt");
    assert_eq!(info.size, 5);
}

#[test]
fn test_file_size_from_open() {
    let fs = MockFs::new().with_files(&[("/tmp/a.txt", b"hello")]);
    let f = fs.open("/tmp/a.txt", OpenMode::Read, None).unwrap();
    assert_eq!(f.size().unwrap(), Some(5));
}

#[test]
fn test_file_seek() {
    let fs = MockFs::new().with_files(&[("/tmp/a.txt", b"hello world")]);
    let mut f = fs.open("/tmp/a.txt", OpenMode::Read, None).unwrap();
    f.seek(SeekFrom::Start(6)).unwrap();
    let mut buf = String::new();
    f.read_to_string(&mut buf).unwrap();
    assert_eq!(buf, "world");
}

// ---- Commit failures ----

/// A file whose commit always fails, like an upload rejected by the server.
#[derive(Default)]
struct CommitState {
    attempts: usize,
    discarded: bool,
    published: bool,
}

struct FailingCommitFile {
    buffer: Cursor<Vec<u8>>,
    state: Option<Arc<Mutex<CommitState>>>,
    fail_on_write: bool,
}

impl Read for FailingCommitFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.buffer.read(buf)
    }
}

impl Write for FailingCommitFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.fail_on_write {
            return Err(std::io::Error::other("write rejected"));
        }
        self.buffer.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Seek for FailingCommitFile {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.buffer.seek(pos)
    }
}

impl FsFile for FailingCommitFile {
    fn info(&self) -> FsResult<FileInfo> {
        Ok(FileInfo::file("failing", 0))
    }

    fn size(&self) -> FsResult<Option<u64>> {
        Ok(None)
    }

    fn commit(&mut self) -> FsResult<()> {
        if let Some(state) = &self.state {
            let mut state = state.lock().unwrap();
            if state.discarded {
                return Ok(());
            }
            state.attempts += 1;
            if state.attempts > 1 {
                state.published = true;
                return Ok(());
            }
            return Err(FsError::AlreadyExists("competing write".into()));
        }
        Err(FsError::Other("upload rejected".into()))
    }

    fn discard(&mut self) -> FsResult<()> {
        if let Some(state) = &self.state {
            state.lock().unwrap().discarded = true;
        }
        Ok(())
    }
}

impl Drop for FailingCommitFile {
    fn drop(&mut self) {
        if self.state.is_some() {
            let _ = self.commit();
        }
    }
}

#[derive(Default)]
struct FailingCommitFs {
    state: Option<Arc<Mutex<CommitState>>>,
    fail_on_write: bool,
}

impl FileSystem for FailingCommitFs {
    fn protocol(&self) -> &[&str] {
        &["failing"]
    }

    fn ls(&self, _path: &str, _detail: bool) -> FsResult<Vec<FileInfo>> {
        Ok(Vec::new())
    }

    fn rm_file(&self, _path: &str) -> FsResult<()> {
        Ok(())
    }

    fn cp_file(&self, _src: &str, _dst: &str) -> FsResult<()> {
        Ok(())
    }

    fn open(
        &self,
        _path: &str,
        _mode: OpenMode,
        _opts: Option<OpenOptions>,
    ) -> FsResult<Box<dyn FsFile>> {
        if _mode == OpenMode::Read {
            return Err(FsError::NotFound(_path.into()));
        }
        Ok(Box::new(FailingCommitFile {
            buffer: Cursor::new(Vec::new()),
            state: self.state.clone(),
            fail_on_write: self.fail_on_write,
        }))
    }

    fn info(&self, path: &str) -> FsResult<FileInfo> {
        Err(FsError::NotFound(path.to_string()))
    }

    fn mkdir(&self, _path: &str, _create_parents: bool) -> FsResult<()> {
        Ok(())
    }

    fn rmdir(&self, _path: &str) -> FsResult<()> {
        Ok(())
    }
}

#[test]
fn pipe_file_returns_commit_errors() {
    let error = FailingCommitFs::default()
        .pipe_file("key", b"value")
        .unwrap_err();
    assert!(error.to_string().contains("upload rejected"));
}

#[test]
fn put_file_returns_commit_errors() {
    let source = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(source.path(), b"value").unwrap();
    let error = FailingCommitFs::default()
        .put_file(source.path().to_str().unwrap(), "key")
        .unwrap_err();
    assert!(error.to_string().contains("upload rejected"));
}

#[test]
fn byte_operation_defaults_reject_conditions_without_modifying_files() {
    use crate::{Condition, OperationError, ReadOptions, Validator, WriteOptions};

    let fs = MockFs::new();
    let fs: &dyn FileSystem = &fs;
    assert_eq!(fs.io_capabilities(), crate::IoCapabilities::default());
    assert_eq!(
        fs.write("/existing", b"original", WriteOptions::default())
            .unwrap()
            .validator,
        None
    );
    let read = fs.read("/existing", ReadOptions::default()).unwrap();
    assert_eq!(read.data, b"original");
    assert_eq!(read.validator, None);
    let condition = Condition::Match(Validator {
        scope: "object".into(),
        value: "etag".into(),
    });
    for path in ["/existing", "/missing"] {
        assert!(matches!(
            fs.read(
                path,
                ReadOptions {
                    condition: Some(condition.clone()),
                    ..Default::default()
                }
            ),
            Err(OperationError::FileSystem(FsError::NotSupported(_)))
        ));
        assert!(matches!(
            fs.read(
                path,
                ReadOptions {
                    condition: Some(Condition::Missing),
                    ..Default::default()
                }
            ),
            Err(OperationError::FileSystem(FsError::InvalidArgument(_)))
        ));
        for condition in [condition.clone(), Condition::Missing] {
            assert!(matches!(
                fs.write(
                    path,
                    b"replacement",
                    WriteOptions {
                        condition: Some(condition)
                    }
                ),
                Err(OperationError::FileSystem(FsError::NotSupported(_)))
            ));
        }
    }
    assert_eq!(fs.cat_file("/existing", None, None).unwrap(), b"original");
    assert!(!fs.exists("/missing").unwrap());
}

#[test]
fn local_and_memory_byte_writes_support_exclusive_create_only() {
    use crate::{Condition, IoCapabilities, OperationError, Validator, WriteOptions};

    let dir = tempfile::tempdir().unwrap();
    let local = crate::LocalFs::new();
    let memory = crate::MemoryFs::new();
    let path = dir.path().join("created").to_string_lossy().to_string();
    for fs in [&local as &dyn FileSystem, &memory] {
        assert_eq!(
            fs.io_capabilities(),
            IoCapabilities {
                write_missing: true,
                ..Default::default()
            }
        );
        let options = WriteOptions {
            condition: Some(Condition::Missing),
        };
        let result = fs.write(&path, b"first", options.clone()).unwrap();
        assert_eq!(result.validator, None);
        assert!(matches!(
            fs.write(&path, b"second", options),
            Err(OperationError::Conflict(_))
        ));
        let options = WriteOptions {
            condition: Some(Condition::Match(Validator {
                scope: path.clone(),
                value: "value".into(),
            })),
        };
        assert!(matches!(
            fs.write(&path, b"second", options),
            Err(OperationError::FileSystem(FsError::NotSupported(_)))
        ));
        assert_eq!(fs.cat_file(&path, None, None).unwrap(), b"first");
    }
}

#[test]
fn local_and_memory_missing_writes_allow_one_concurrent_writer() {
    use crate::{Condition, OperationError, WriteOptions};

    let dir = tempfile::tempdir().unwrap();
    let local = crate::LocalFs::new();
    let memory = crate::MemoryFs::new();
    let path = dir.path().join("race").to_string_lossy().to_string();
    for fs in [&local as &(dyn FileSystem + Sync), &memory] {
        let barrier = std::sync::Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let handles: Vec<_> = [b"first".as_slice(), b"second".as_slice()]
                .into_iter()
                .map(|data| {
                    let path = &path;
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        (
                            data,
                            fs.write(
                                path,
                                data,
                                WriteOptions {
                                    condition: Some(Condition::Missing),
                                },
                            ),
                        )
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            results.iter().filter(|(_, result)| result.is_ok()).count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|(_, result)| matches!(result, Err(OperationError::Conflict(_))))
                .count(),
            1
        );
        let winner = results.iter().find(|(_, result)| result.is_ok()).unwrap().0;
        assert_eq!(fs.cat_file(&path, None, None).unwrap(), winner);
    }
}

#[test]
fn conditional_create_discards_files_after_write_or_commit_failure() {
    for fail_on_write in [false, true] {
        let state = Arc::new(Mutex::new(CommitState::default()));
        let fs = FailingCommitFs {
            state: Some(state.clone()),
            fail_on_write,
        };
        let result = crate::fs::write_with_exclusive_create(
            &fs,
            "key",
            b"value",
            crate::WriteOptions {
                condition: Some(crate::Condition::Missing),
            },
        );
        if fail_on_write {
            let error = result.unwrap_err();
            assert_eq!(error.kind(), crate::ErrorKind::UnknownOutcome);
            assert!(!error.retry_safe());
        } else {
            assert!(matches!(result, Err(crate::OperationError::Conflict(_))));
        }
        let state = state.lock().unwrap();
        assert!(state.discarded);
        assert!(!state.published);
        assert_eq!(state.attempts, usize::from(!fail_on_write));
    }
}

#[test]
fn default_range_reads_preserve_order_and_fail_closed() {
    use crate::{ByteRange, Condition, OperationError, RangeReadOptions, Validator};

    let fs = MockFs::new().with_files(&[("/data", b"0123456789abcdef")]);
    let ranges = [
        ByteRange {
            start: Some(8),
            end: Some(12),
        },
        ByteRange {
            start: Some(0),
            end: Some(4),
        },
        ByteRange {
            start: Some(2),
            end: Some(6),
        },
        ByteRange {
            start: Some(-4),
            end: None,
        },
        ByteRange {
            start: Some(5),
            end: Some(1),
        },
        ByteRange {
            start: Some(0),
            end: Some(4),
        },
    ];
    let result = fs
        .read_ranges(
            "/data",
            &ranges,
            RangeReadOptions {
                max_gap: 2,
                max_request_size: 5,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(result.validator, None);
    for (range, data) in ranges.iter().zip(result.data) {
        assert_eq!(data, fs.cat_file("/data", range.start, range.end).unwrap());
    }
    assert_eq!(
        fs.read_ranges("/missing", &[], RangeReadOptions::default())
            .unwrap()
            .data,
        Vec::<Vec<u8>>::new()
    );
    for options in [
        RangeReadOptions {
            max_request_size: 0,
            ..Default::default()
        },
        RangeReadOptions {
            max_concurrency: 0,
            ..Default::default()
        },
        RangeReadOptions {
            condition: Some(Condition::Missing),
            ..Default::default()
        },
    ] {
        assert!(matches!(
            fs.read_ranges("/missing", &[], options),
            Err(OperationError::FileSystem(FsError::InvalidArgument(_)))
        ));
    }
    let options = RangeReadOptions {
        condition: Some(Condition::Match(Validator {
            scope: "object".into(),
            value: "v".into(),
        })),
        ..Default::default()
    };
    assert!(matches!(
        fs.read_ranges("/missing", &[], options),
        Err(OperationError::FileSystem(FsError::NotSupported(_)))
    ));
}

#[test]
fn default_range_reads_reject_directories_even_for_empty_ranges() {
    use crate::{ByteRange, OperationError, RangeReadOptions};

    let dir = tempfile::tempdir().unwrap();
    let local = crate::LocalFs::new();
    let memory = crate::MemoryFs::new();
    let path = dir.path().to_str().unwrap();
    memory.mkdir(path, true).unwrap();
    for fs in [&local as &dyn FileSystem, &memory] {
        for range in [
            ByteRange::default(),
            ByteRange {
                start: Some(0),
                end: Some(0),
            },
        ] {
            assert!(matches!(
                fs.read_ranges(path, &[range], RangeReadOptions::default()),
                Err(OperationError::FileSystem(FsError::IsADirectory(_)))
            ));
        }
    }
}
