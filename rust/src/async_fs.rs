use std::collections::HashMap;

use crate::error::{FsError, FsResult};
use crate::file::FsFile;
use crate::types::{DuResult, FileInfo, FileType, OpenMode, OpenOptions, WalkEntry};

/// Async filesystem trait. Mirrors [`super::fs::FileSystem`] but with
/// `async fn` methods, suitable for backends that are inherently async
/// (e.g. S3 via `object_store`, HTTP).
///
/// Backends implement the async primitive methods and get higher-level
/// async operations for free via default implementations, exactly like
/// fsspec's `AsyncFileSystem`.
#[allow(async_fn_in_trait)]
pub trait AsyncFileSystem: Send + Sync {
    /// Protocol name(s) this filesystem handles.
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
    fn ls(
        &self,
        path: &str,
        detail: bool,
    ) -> impl std::future::Future<Output = FsResult<Vec<FileInfo>>> + Send;

    /// Delete a single file.
    fn rm_file(&self, path: &str) -> impl std::future::Future<Output = FsResult<()>> + Send;

    /// Copy a single file within this filesystem.
    fn cp_file(
        &self,
        src: &str,
        dst: &str,
    ) -> impl std::future::Future<Output = FsResult<()>> + Send;

    /// Open a file and return a file-like object.
    fn open(
        &self,
        path: &str,
        mode: OpenMode,
        opts: Option<OpenOptions>,
    ) -> impl std::future::Future<Output = FsResult<Box<dyn FsFile>>> + Send;

    /// Return metadata about a single path.
    fn info(&self, path: &str) -> impl std::future::Future<Output = FsResult<FileInfo>> + Send;

    /// Create a directory.
    fn mkdir(
        &self,
        path: &str,
        create_parents: bool,
    ) -> impl std::future::Future<Output = FsResult<()>> + Send;

    /// Remove an empty directory.
    fn rmdir(&self, path: &str) -> impl std::future::Future<Output = FsResult<()>> + Send;

    // ---------------------------------------------------------------
    // Concrete methods — default implementations built on primitives
    // ---------------------------------------------------------------

    /// Strip the protocol prefix from a path.
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
    async fn exists(&self, path: &str) -> FsResult<bool> {
        match self.info(path).await {
            Ok(_) => Ok(true),
            Err(FsError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Check if a path is a directory.
    async fn isdir(&self, path: &str) -> FsResult<bool> {
        match self.info(path).await {
            Ok(info) => Ok(info.file_type == FileType::Directory),
            Err(FsError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Check if a path is a file.
    async fn isfile(&self, path: &str) -> FsResult<bool> {
        match self.info(path).await {
            Ok(info) => Ok(info.file_type == FileType::File),
            Err(FsError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Return the size of a file in bytes.
    async fn size(&self, path: &str) -> FsResult<u64> {
        Ok(self.info(path).await?.size)
    }

    /// Return sizes of multiple paths.
    async fn sizes(&self, paths: &[&str]) -> FsResult<Vec<u64>> {
        let mut result = Vec::with_capacity(paths.len());
        for p in paths {
            result.push(self.size(p).await?);
        }
        Ok(result)
    }

    /// Read the entire contents of a file.
    async fn cat_file(
        &self,
        path: &str,
        start: Option<i64>,
        end: Option<i64>,
    ) -> FsResult<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = self.open(path, OpenMode::Read, None).await?;
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
    async fn pipe_file(&self, path: &str, data: &[u8]) -> FsResult<()> {
        use std::io::Write;
        let mut f = self.open(path, OpenMode::Write, None).await?;
        f.write_all(data)?;
        f.flush()?;
        Ok(())
    }

    /// Read the first `size` bytes of a file.
    async fn head(&self, path: &str, size: usize) -> FsResult<Vec<u8>> {
        self.cat_file(path, Some(0), Some(size as i64)).await
    }

    /// Read the last `size` bytes of a file.
    async fn tail(&self, path: &str, size: usize) -> FsResult<Vec<u8>> {
        self.cat_file(path, Some(-(size as i64)), None).await
    }

    /// Create an empty file, or truncate an existing one.
    async fn touch(&self, path: &str, truncate: bool) -> FsResult<()> {
        if truncate || !self.exists(path).await? {
            self.pipe_file(path, b"").await?;
        }
        Ok(())
    }

    /// Walk a directory tree, yielding (dirpath, dirnames, filenames).
    async fn walk(
        &self,
        path: &str,
        max_depth: Option<usize>,
        topdown: bool,
    ) -> FsResult<Vec<WalkEntry>> {
        // Iterative implementation (avoids recursive async which requires
        // boxed futures due to unknown future size at compile time).
        let mut stack: Vec<(String, usize)> = vec![(path.to_string(), 0)];
        let mut results = Vec::new();

        while let Some((current_path, depth)) = stack.pop() {
            if let Some(max) = max_depth {
                if depth >= max {
                    continue;
                }
            }

            let entries = self.ls(&current_path, true).await?;
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
                dirpath: current_path.clone(),
                dirnames: dirnames.clone(),
                filenames,
            };

            results.push(this_entry);

            // Push child directories onto stack (reverse order so they
            // are visited in sorted order when popped).
            let mut child_dirs: Vec<String> = entries
                .iter()
                .filter(|e| e.is_dir())
                .map(|e| e.name.clone())
                .collect();
            child_dirs.reverse();
            for dir in child_dirs {
                stack.push((dir, depth + 1));
            }
        }

        // For bottom-up ordering, reverse the results.
        if !topdown {
            results.reverse();
        }

        Ok(results)
    }

    /// Find all files below a path.
    async fn find(
        &self,
        path: &str,
        max_depth: Option<usize>,
        with_dirs: bool,
    ) -> FsResult<Vec<String>> {
        let walk = self.walk(path, max_depth, true).await?;
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
    async fn copy(&self, src: &str, dst: &str, recursive: bool) -> FsResult<()> {
        if !recursive {
            return self.cp_file(src, dst).await;
        }

        let src_info = self.info(src).await?;
        if src_info.is_file() {
            return self.cp_file(src, dst).await;
        }

        let files = self.find(src, None, false).await?;
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
            let parent = self.parent(&dst_path);
            if !parent.is_empty() && !self.exists(&parent).await? {
                self.mkdir(&parent, true).await?;
            }
            self.cp_file(file_path, &dst_path).await?;
        }
        Ok(())
    }

    /// Move a file or directory.
    async fn mv(&self, src: &str, dst: &str) -> FsResult<()> {
        self.copy(src, dst, true).await?;
        self.rm(src, true).await?;
        Ok(())
    }

    /// Remove file(s), optionally recursively.
    async fn rm(&self, path: &str, recursive: bool) -> FsResult<()> {
        let info = self.info(path).await?;
        if info.is_file() {
            return self.rm_file(path).await;
        }
        if !recursive {
            return Err(FsError::IsADirectory(format!(
                "cannot remove directory without recursive=true: {path}"
            )));
        }
        let walk = self.walk(path, None, false).await?;
        for entry in &walk {
            for fname in &entry.filenames {
                let sep = self.sep();
                let full = if entry.dirpath.ends_with(sep) {
                    format!("{}{fname}", entry.dirpath)
                } else {
                    format!("{}{sep}{fname}", entry.dirpath)
                };
                self.rm_file(&full).await?;
            }
        }
        for entry in &walk {
            if entry.dirpath != path {
                self.rmdir(&entry.dirpath).await?;
            }
        }
        self.rmdir(path).await?;
        Ok(())
    }

    /// Disk usage.
    async fn du(&self, path: &str, total: bool) -> FsResult<DuResult> {
        let files = self.find(path, None, false).await?;
        if total {
            let mut sum = 0u64;
            for f in &files {
                sum += self.size(f).await?;
            }
            Ok(DuResult::Total(sum))
        } else {
            let mut map = HashMap::new();
            for f in &files {
                map.insert(f.clone(), self.size(f).await?);
            }
            Ok(DuResult::PerPath(map))
        }
    }

    /// Create parent directories as needed.
    async fn makedirs(&self, path: &str, exist_ok: bool) -> FsResult<()> {
        match self.mkdir(path, true).await {
            Ok(()) => Ok(()),
            Err(FsError::AlreadyExists(_)) if exist_ok => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Read file contents as a UTF-8 string.
    async fn read_text(&self, path: &str) -> FsResult<String> {
        let data = self.cat_file(path, None, None).await?;
        String::from_utf8(data).map_err(|e| FsError::Other(format!("UTF-8 decode error: {e}")))
    }

    /// Write a UTF-8 string to a file.
    async fn write_text(&self, path: &str, data: &str) -> FsResult<()> {
        self.pipe_file(path, data.as_bytes()).await
    }

    /// Download a remote file to a local path.
    async fn get_file(&self, remote: &str, local: &str) -> FsResult<()> {
        use std::io::{Read, Write};
        let mut src = self.open(remote, OpenMode::Read, None).await?;
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
    async fn put_file(&self, local: &str, remote: &str) -> FsResult<()> {
        use std::io::{Read, Write};
        let mut src = std::fs::File::open(local)?;
        let mut dst = self.open(remote, OpenMode::Write, None).await?;
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
