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

    fn size(&self) -> FsResult<Option<u64>> {
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
