use std::collections::HashMap;

use crate::error::{FsError, FsResult};
use crate::file::FsFile;
use crate::types::{DuResult, FileInfo, FileType, OpenMode, OpenOptions, WalkEntry};

/// Core filesystem trait. Backends implement the primitive methods
/// and get higher-level operations for free via default implementations.
///
/// This mirrors fsspec's `AbstractFileSystem` template-method pattern.
pub trait FileSystem {
    /// Protocol name(s) this filesystem handles (e.g., `["file", "local"]`).
    fn protocol(&self) -> &[&str];

    /// Root marker (e.g., `"/"` for local, `""` for cloud).
    fn root_marker(&self) -> &str {
        ""
    }

    /// Path separator.
    fn sep(&self) -> &str {
        "/"
    }

    // ---------------------------------------------------------------
    // Primitives — backends MUST implement these
    // ---------------------------------------------------------------

    /// List directory contents.
    ///
    /// If `detail` is true, return full `FileInfo` for each entry.
    /// The returned vec always contains `FileInfo` structs; when the
    /// caller wants names only it can map over `.name`.
    fn ls(&self, path: &str, detail: bool) -> FsResult<Vec<FileInfo>>;

    /// Delete a single file.
    fn rm_file(&self, path: &str) -> FsResult<()>;

    /// Copy a single file within this filesystem.
    fn cp_file(&self, src: &str, dst: &str) -> FsResult<()>;

    /// Open a file and return a file-like object.
    fn open(
        &self,
        path: &str,
        mode: OpenMode,
        opts: Option<OpenOptions>,
    ) -> FsResult<Box<dyn FsFile>>;

    /// Return metadata about a single path.
    fn info(&self, path: &str) -> FsResult<FileInfo>;

    /// Create a directory. If `create_parents` is true, create
    /// intermediate directories as needed.
    fn mkdir(&self, path: &str, create_parents: bool) -> FsResult<()>;

    /// Remove an empty directory.
    fn rmdir(&self, path: &str) -> FsResult<()>;

    // ---------------------------------------------------------------
    // Concrete methods — default implementations built on primitives
    // ---------------------------------------------------------------

    /// Strip the protocol prefix from a path.
    /// Default: removes `"protocol://"` prefix for each protocol.
    fn strip_protocol(&self, path: &str) -> String {
        let mut p = path.to_string();
        for proto in self.protocol() {
            let prefix = format!("{proto}://");
            if let Some(stripped) = p.strip_prefix(&prefix) {
                p = stripped.to_string();
                break;
            }
        }
        if p.is_empty() {
            return self.root_marker().to_string();
        }
        p
    }

    /// Re-add the protocol prefix.
    fn unstrip_protocol(&self, path: &str) -> String {
        let proto = self.protocol().first().unwrap_or(&"abstract");
        format!("{proto}://{path}")
    }

    /// Return the parent directory of a path.
    fn parent(&self, path: &str) -> String {
        let sep = self.sep();
        let path = path.trim_end_matches(sep);
        if path.is_empty() || !path.contains(sep) {
            return self.root_marker().to_string();
        }
        match path.rfind(sep) {
            Some(idx) => {
                let parent = &path[..idx];
                if parent.is_empty() {
                    self.root_marker().to_string()
                } else {
                    parent.to_string()
                }
            }
            None => self.root_marker().to_string(),
        }
    }

    /// Check if a path exists.
    fn exists(&self, path: &str) -> FsResult<bool> {
        match self.info(path) {
            Ok(_) => Ok(true),
            Err(FsError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Check if a path is a directory.
    fn isdir(&self, path: &str) -> FsResult<bool> {
        match self.info(path) {
            Ok(info) => Ok(info.file_type == FileType::Directory),
            Err(FsError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Check if a path is a file.
    fn isfile(&self, path: &str) -> FsResult<bool> {
        match self.info(path) {
            Ok(info) => Ok(info.file_type == FileType::File),
            Err(FsError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Return the size of a file in bytes.
    fn size(&self, path: &str) -> FsResult<u64> {
        Ok(self.info(path)?.size)
    }

    /// Return sizes of multiple paths.
    fn sizes(&self, paths: &[&str]) -> FsResult<Vec<u64>> {
        paths.iter().map(|p| self.size(p)).collect()
    }

    /// Read the entire contents of a file.
    fn cat_file(&self, path: &str, start: Option<i64>, end: Option<i64>) -> FsResult<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = self.open(path, OpenMode::Read, None)?;
        let file_size = f.size()?.unwrap_or(0) as i64;

        let actual_start = match start {
            Some(s) if s < 0 => (file_size + s).max(0) as u64,
            Some(s) => s as u64,
            None => 0,
        };
        let actual_end = match end {
            Some(e) if e < 0 => (file_size + e).max(0) as u64,
            Some(e) => e as u64,
            None => file_size as u64,
        };

        if actual_start > 0 {
            f.seek(SeekFrom::Start(actual_start))?;
        }

        let to_read = (actual_end - actual_start) as usize;
        let mut buf = vec![0u8; to_read];
        let mut total = 0;
        while total < to_read {
            match f.read(&mut buf[total..]) {
                Ok(0) => break,
                Ok(n) => total += n,
                Err(e) => return Err(e.into()),
            }
        }
        buf.truncate(total);
        Ok(buf)
    }

    /// Write bytes directly to a file.
    fn pipe_file(&self, path: &str, data: &[u8]) -> FsResult<()> {
        use std::io::Write;
        let mut f = self.open(path, OpenMode::Write, None)?;
        f.write_all(data)?;
        f.flush()?;
        Ok(())
    }

    /// Read the first `size` bytes of a file.
    fn head(&self, path: &str, size: usize) -> FsResult<Vec<u8>> {
        self.cat_file(path, Some(0), Some(size as i64))
    }

    /// Read the last `size` bytes of a file.
    fn tail(&self, path: &str, size: usize) -> FsResult<Vec<u8>> {
        self.cat_file(path, Some(-(size as i64)), None)
    }

    /// Create an empty file, or truncate an existing one.
    fn touch(&self, path: &str, truncate: bool) -> FsResult<()> {
        if truncate || !self.exists(path)? {
            self.pipe_file(path, b"")?;
        }
        Ok(())
    }

    /// Walk a directory tree, yielding (dirpath, dirnames, filenames).
    fn walk(
        &self,
        path: &str,
        max_depth: Option<usize>,
        topdown: bool,
    ) -> FsResult<Vec<WalkEntry>> {
        self.walk_recursive(path, max_depth, topdown, 0)
    }

    /// Internal recursive walk helper.
    fn walk_recursive(
        &self,
        path: &str,
        max_depth: Option<usize>,
        topdown: bool,
        current_depth: usize,
    ) -> FsResult<Vec<WalkEntry>> {
        if let Some(max) = max_depth {
            if current_depth >= max {
                return Ok(vec![]);
            }
        }

        let entries = self.ls(path, true)?;
        let mut dirnames = Vec::new();
        let mut filenames = Vec::new();

        for entry in &entries {
            let basename = entry
                .name
                .rsplit(self.sep())
                .next()
                .unwrap_or(&entry.name)
                .to_string();
            match entry.file_type {
                FileType::Directory => dirnames.push(basename),
                _ => filenames.push(basename),
            }
        }

        let this_entry = WalkEntry {
            dirpath: path.to_string(),
            dirnames: dirnames.clone(),
            filenames,
        };

        let mut results = Vec::new();

        if topdown {
            results.push(this_entry.clone());
        }

        for dirinfo in entries.iter().filter(|e| e.is_dir()) {
            let sub = self.walk_recursive(&dirinfo.name, max_depth, topdown, current_depth + 1)?;
            results.extend(sub);
        }

        if !topdown {
            results.push(this_entry);
        }

        Ok(results)
    }

    /// Find all files below a path.
    fn find(&self, path: &str, max_depth: Option<usize>, with_dirs: bool) -> FsResult<Vec<String>> {
        let walk = self.walk(path, max_depth, true)?;
        let mut out = Vec::new();
        for entry in &walk {
            if with_dirs && entry.dirpath != path {
                out.push(entry.dirpath.clone());
            }
            for fname in &entry.filenames {
                let sep = self.sep();
                let full = if entry.dirpath.ends_with(sep) {
                    format!("{}{fname}", entry.dirpath)
                } else {
                    format!("{}{sep}{fname}", entry.dirpath)
                };
                out.push(full);
            }
        }
        Ok(out)
    }

    /// Copy files, potentially recursively.
    fn copy(&self, src: &str, dst: &str, recursive: bool) -> FsResult<()> {
        if !recursive {
            return self.cp_file(src, dst);
        }

        let src_info = self.info(src)?;
        if src_info.is_file() {
            return self.cp_file(src, dst);
        }

        // Recursive directory copy
        let files = self.find(src, None, false)?;
        for file_path in &files {
            let relative = file_path
                .strip_prefix(src)
                .unwrap_or(file_path)
                .trim_start_matches(self.sep());
            let sep = self.sep();
            let dst_path = if dst.ends_with(sep) {
                format!("{dst}{relative}")
            } else {
                format!("{dst}{sep}{relative}")
            };
            // Ensure parent directory exists
            let parent = self.parent(&dst_path);
            if !parent.is_empty() && !self.exists(&parent)? {
                self.mkdir(&parent, true)?;
            }
            self.cp_file(file_path, &dst_path)?;
        }
        Ok(())
    }

    /// Move a file or directory.
    fn mv(&self, src: &str, dst: &str) -> FsResult<()> {
        self.copy(src, dst, true)?;
        self.rm(src, true)?;
        Ok(())
    }

    /// Remove file(s), optionally recursively.
    fn rm(&self, path: &str, recursive: bool) -> FsResult<()> {
        let info = self.info(path)?;
        if info.is_file() {
            return self.rm_file(path);
        }
        if !recursive {
            return Err(FsError::IsADirectory(format!(
                "cannot remove directory without recursive=true: {path}"
            )));
        }
        // Remove files first, then directories bottom-up
        let walk = self.walk(path, None, false)?;
        for entry in &walk {
            for fname in &entry.filenames {
                let sep = self.sep();
                let full = if entry.dirpath.ends_with(sep) {
                    format!("{}{fname}", entry.dirpath)
                } else {
                    format!("{}{sep}{fname}", entry.dirpath)
                };
                self.rm_file(&full)?;
            }
        }
        // Remove directories bottom-up (walk with topdown=false gives us this order)
        for entry in &walk {
            if entry.dirpath != path {
                self.rmdir(&entry.dirpath)?;
            }
        }
        self.rmdir(path)?;
        Ok(())
    }

    /// Disk usage.
    fn du(&self, path: &str, total: bool) -> FsResult<DuResult> {
        let files = self.find(path, None, false)?;
        if total {
            let mut sum = 0u64;
            for f in &files {
                sum += self.size(f)?;
            }
            Ok(DuResult::Total(sum))
        } else {
            let mut map = HashMap::new();
            for f in &files {
                map.insert(f.clone(), self.size(f)?);
            }
            Ok(DuResult::PerPath(map))
        }
    }

    /// Create parent directories as needed (like `makedirs`).
    fn makedirs(&self, path: &str, exist_ok: bool) -> FsResult<()> {
        match self.mkdir(path, true) {
            Ok(()) => Ok(()),
            Err(FsError::AlreadyExists(_)) if exist_ok => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Read file contents as a UTF-8 string.
    fn read_text(&self, path: &str) -> FsResult<String> {
        let data = self.cat_file(path, None, None)?;
        String::from_utf8(data).map_err(|e| FsError::Other(format!("UTF-8 decode error: {e}")))
    }

    /// Write a UTF-8 string to a file.
    fn write_text(&self, path: &str, data: &str) -> FsResult<()> {
        self.pipe_file(path, data.as_bytes())
    }

    /// Download a remote file to a local path.
    fn get_file(&self, remote: &str, local: &str) -> FsResult<()> {
        use std::io::{Read, Write};
        let mut src = self.open(remote, OpenMode::Read, None)?;
        let mut dst = std::fs::File::create(local)?;
        let mut buf = vec![0u8; 128 * 1024];
        loop {
            let n = src.read(&mut buf)?;
            if n == 0 {
                break;
            }
            dst.write_all(&buf[..n])?;
        }
        Ok(())
    }

    /// Upload a local file to a remote path.
    fn put_file(&self, local: &str, remote: &str) -> FsResult<()> {
        use std::io::{Read, Write};
        let mut src = std::fs::File::open(local)?;
        let mut dst = self.open(remote, OpenMode::Write, None)?;
        let mut buf = vec![0u8; 128 * 1024];
        loop {
            let n = src.read(&mut buf)?;
            if n == 0 {
                break;
            }
            dst.write_all(&buf[..n])?;
        }
        dst.flush()?;
        Ok(())
    }
}
