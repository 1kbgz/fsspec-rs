use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use crate::error::{FsError, FsResult};
use crate::file::FsFile;
use crate::fs::{strip_protocol_default, FileSystem};
use crate::types::{FileInfo, OpenMode, OpenOptions};

/// `SystemTime::now` panics on `wasm32-unknown-unknown`, which has no clock.
fn now() -> Option<SystemTime> {
    if cfg!(all(target_arch = "wasm32", target_os = "unknown")) {
        None
    } else {
        Some(SystemTime::now())
    }
}

/// `/`-rooted path without a trailing `/`.
fn normalize(path: &str) -> String {
    format!("/{}", path.trim_matches('/'))
}

/// The prefix every path below `dir` starts with.
fn child_prefix(dir: &str) -> String {
    if dir == "/" {
        "/".to_string()
    } else {
        format!("{dir}/")
    }
}

/// `path`'s ancestors, nearest first, excluding the root.
fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    let mut rest = path;
    std::iter::from_fn(move || {
        let (parent, _) = rest.rsplit_once('/')?;
        rest = parent;
        (!parent.is_empty()).then_some(parent)
    })
}

struct Entry {
    data: Vec<u8>,
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
}

/// Files by path, plus directories created with `mkdir`. Other directories exist
/// while a file below them does, as in fsspec's `MemoryFileSystem`.
#[derive(Default)]
struct Store {
    files: BTreeMap<String, Entry>,
    dirs: BTreeSet<String>,
}

impl Store {
    fn is_dir(&self, path: &str) -> bool {
        if path == "/" || self.dirs.contains(path) {
            return true;
        }
        let prefix = child_prefix(path);
        let below = |key: Option<&String>| key.is_some_and(|key| key.starts_with(&prefix));
        below(
            self.files
                .range(prefix.clone()..)
                .next()
                .map(|(key, _)| key),
        ) || below(self.dirs.range(prefix.clone()..).next())
    }

    fn info(&self, path: &str) -> FsResult<FileInfo> {
        if let Some(entry) = self.files.get(path) {
            return Ok(FileInfo {
                created: entry.created,
                modified: entry.modified,
                ..FileInfo::file(path, entry.data.len() as u64)
            });
        }
        if self.is_dir(path) {
            return Ok(FileInfo::directory(path));
        }
        Err(FsError::NotFound(path.to_string()))
    }

    fn children(&self, dir: &str) -> Vec<FileInfo> {
        let prefix = child_prefix(dir);
        let names = self
            .files
            .range(prefix.clone()..)
            .map(|(key, _)| key)
            .take_while(|key| key.starts_with(&prefix))
            .chain(
                self.dirs
                    .range(prefix.clone()..)
                    .take_while(|key| key.starts_with(&prefix)),
            )
            .map(|key| {
                let rest = &key[prefix.len()..];
                let name = rest.split('/').next().unwrap_or(rest);
                format!("{prefix}{name}")
            })
            .collect::<BTreeSet<_>>();
        names
            .into_iter()
            .map(|name| match self.files.get(&name) {
                Some(_) => self.info(&name).expect("listed file exists"),
                None => FileInfo::directory(name),
            })
            .collect()
    }

    fn write(&mut self, path: &str, data: Vec<u8>, exclusive: bool) -> FsResult<()> {
        if self.is_dir(path) {
            return Err(FsError::IsADirectory(path.to_string()));
        }
        if let Some(parent) = ancestors(path).find(|parent| self.files.contains_key(*parent)) {
            return Err(FsError::NotADirectory(parent.to_string()));
        }
        let created = match self.files.get(path) {
            Some(_) if exclusive => return Err(FsError::AlreadyExists(path.to_string())),
            Some(entry) => entry.created,
            None => now(),
        };
        self.files.insert(
            path.to_string(),
            Entry {
                data,
                created,
                modified: now(),
            },
        );
        Ok(())
    }
}

/// An open file. Reads see the contents at open; writes replace the file on commit
/// or drop, so readers never see a partial write.
pub struct MemoryFile {
    store: Arc<Mutex<Store>>,
    path: String,
    cursor: Cursor<Vec<u8>>,
    writable: bool,
    exclusive: bool,
    committed: bool,
}

impl MemoryFile {
    fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Read for MemoryFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.cursor.read(buf)
    }
}

impl Write for MemoryFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !self.writable {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("not open for writing: {}", self.path),
            ));
        }
        self.cursor.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        // Contents become visible only on commit.
        Ok(())
    }
}

impl Seek for MemoryFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.cursor.seek(pos)
    }
}

impl FsFile for MemoryFile {
    fn info(&self) -> FsResult<FileInfo> {
        self.store().info(&self.path)
    }

    fn size(&self) -> FsResult<Option<u64>> {
        Ok(Some(self.cursor.get_ref().len() as u64))
    }

    fn commit(&mut self) -> FsResult<()> {
        if self.writable && !self.committed {
            let data = self.cursor.get_ref().clone();
            self.store().write(&self.path, data, self.exclusive)?;
            self.committed = true;
        }
        Ok(())
    }

    fn discard(&mut self) -> FsResult<()> {
        self.committed = true;
        Ok(())
    }
}

impl Drop for MemoryFile {
    fn drop(&mut self) {
        let _ = self.commit();
    }
}

/// In-memory filesystem, the Rust counterpart of fsspec's `MemoryFileSystem`.
///
/// Clones share one store. Through the registry, `memory://` URLs share one instance
/// per process.
#[derive(Clone, Default)]
pub struct MemoryFs {
    store: Arc<Mutex<Store>>,
}

impl MemoryFs {
    /// An empty filesystem.
    pub fn new() -> Self {
        Self::default()
    }

    fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl FileSystem for MemoryFs {
    fn protocol(&self) -> &[&str] {
        &["memory"]
    }

    fn root_marker(&self) -> &str {
        "/"
    }

    fn strip_protocol(&self, path: &str) -> String {
        normalize(&strip_protocol_default(self.protocol(), "", path))
    }

    fn ls(&self, path: &str, _detail: bool) -> FsResult<Vec<FileInfo>> {
        let path = normalize(path);
        let store = self.store();
        if store.files.contains_key(&path) {
            return Err(FsError::NotADirectory(path));
        }
        if !store.is_dir(&path) {
            return Err(FsError::NotFound(path));
        }
        Ok(store.children(&path))
    }

    fn rm_file(&self, path: &str) -> FsResult<()> {
        let path = normalize(path);
        let mut store = self.store();
        if store.files.remove(&path).is_some() {
            return Ok(());
        }
        if store.is_dir(&path) {
            return Err(FsError::IsADirectory(path));
        }
        Err(FsError::NotFound(path))
    }

    fn cp_file(&self, src: &str, dst: &str) -> FsResult<()> {
        let (src, dst) = (normalize(src), normalize(dst));
        let mut store = self.store();
        let data = match store.files.get(&src) {
            Some(entry) => entry.data.clone(),
            None if store.is_dir(&src) => return Err(FsError::IsADirectory(src)),
            None => return Err(FsError::NotFound(src)),
        };
        store.write(&dst, data, false)
    }

    fn open(
        &self,
        path: &str,
        mode: OpenMode,
        _opts: Option<OpenOptions>,
    ) -> FsResult<Box<dyn FsFile>> {
        let path = normalize(path);
        let store = self.store();
        if store.is_dir(&path) {
            return Err(FsError::IsADirectory(path));
        }
        let existing = store.files.get(&path).map(|entry| entry.data.clone());
        drop(store);
        let (data, writable, exclusive) = match mode {
            OpenMode::Read => (
                existing.ok_or_else(|| FsError::NotFound(path.clone()))?,
                false,
                false,
            ),
            OpenMode::Write => (Vec::new(), true, false),
            OpenMode::Append => (existing.unwrap_or_default(), true, false),
            OpenMode::Exclusive if existing.is_some() => return Err(FsError::AlreadyExists(path)),
            OpenMode::Exclusive => (Vec::new(), true, true),
        };
        let mut cursor = Cursor::new(data);
        if mode == OpenMode::Append {
            cursor.seek(SeekFrom::End(0))?;
        }
        Ok(Box::new(MemoryFile {
            store: Arc::clone(&self.store),
            path,
            cursor,
            writable,
            exclusive,
            committed: false,
        }))
    }

    fn info(&self, path: &str) -> FsResult<FileInfo> {
        self.store().info(&normalize(path))
    }

    fn mkdir(&self, path: &str, create_parents: bool) -> FsResult<()> {
        let path = normalize(path);
        let mut store = self.store();
        if store.files.contains_key(&path) || store.is_dir(&path) {
            return Err(FsError::AlreadyExists(path));
        }
        let missing: Vec<String> = ancestors(&path)
            .take_while(|parent| !store.is_dir(parent))
            .map(str::to_string)
            .collect();
        if let Some(parent) = missing
            .iter()
            .find(|parent| store.files.contains_key(*parent))
        {
            return Err(FsError::NotADirectory(parent.clone()));
        }
        if !create_parents {
            if let Some(parent) = missing.first() {
                return Err(FsError::NotFound(parent.clone()));
            }
        }
        store.dirs.extend(missing);
        store.dirs.insert(path);
        Ok(())
    }

    fn rmdir(&self, path: &str) -> FsResult<()> {
        let path = normalize(path);
        let mut store = self.store();
        if store.files.contains_key(&path) {
            return Err(FsError::NotADirectory(path));
        }
        if !store.is_dir(&path) {
            return Err(FsError::NotFound(path));
        }
        if !store.children(&path).is_empty() {
            return Err(FsError::IoError(io::Error::new(
                io::ErrorKind::DirectoryNotEmpty,
                format!("directory not empty: {path}"),
            )));
        }
        store.dirs.remove(&path);
        Ok(())
    }
}
