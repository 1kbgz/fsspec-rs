//! The Python classes the filesystems share. Each backend module defines its struct,
//! constructor, and `__repr__`, then generates the rest with these macros.

/// A file class wrapping `Box<dyn FsFile>`, released from the GIL during I/O.
macro_rules! file_class {
    ($(#[$doc:meta])* $file:ident, $name:literal) => {
        $(#[$doc])*
        #[pyclass(name = $name)]
        pub struct $file {
            inner: Mutex<Option<Box<dyn fsspec_rs::FsFile>>>,
            path: String,
        }

        #[pymethods]
        impl $file {
            /// Read up to `size` bytes. If `size` is -1 or None, read to EOF.
            #[pyo3(signature = (size = -1))]
            fn read(&self, py: Python<'_>, size: i64) -> PyResult<Vec<u8>> {
                py.detach(|| {
                    let mut guard = self.inner.lock().unwrap();
                    let f = guard
                        .as_mut()
                        .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("file is closed"))?;
                    if size < 0 {
                        let mut buf = Vec::new();
                        f.read_to_end(&mut buf)
                            .map_err(|e| pyo3::exceptions::PyOSError::new_err(e.to_string()))?;
                        Ok(buf)
                    } else {
                        let mut buf = vec![0u8; size as usize];
                        let n = f
                            .read(&mut buf)
                            .map_err(|e| pyo3::exceptions::PyOSError::new_err(e.to_string()))?;
                        buf.truncate(n);
                        Ok(buf)
                    }
                })
            }

            /// Write bytes to the file.
            fn write(&self, py: Python<'_>, data: &[u8]) -> PyResult<usize> {
                let data = data.to_vec();
                py.detach(|| {
                    let mut guard = self.inner.lock().unwrap();
                    let f = guard
                        .as_mut()
                        .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("file is closed"))?;
                    let n = f
                        .write(&data)
                        .map_err(|e| pyo3::exceptions::PyOSError::new_err(e.to_string()))?;
                    Ok(n)
                })
            }

            /// Seek to a position in the file.
            fn seek(&self, py: Python<'_>, offset: i64, whence: Option<u8>) -> PyResult<u64> {
                py.detach(|| {
                    let mut guard = self.inner.lock().unwrap();
                    let f = guard
                        .as_mut()
                        .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("file is closed"))?;
                    let pos = match whence.unwrap_or(0) {
                        0 => SeekFrom::Start(offset as u64),
                        1 => SeekFrom::Current(offset),
                        2 => SeekFrom::End(offset),
                        _ => {
                            return Err(pyo3::exceptions::PyValueError::new_err(
                                "whence must be 0, 1, or 2",
                            ))
                        }
                    };
                    f.seek(pos)
                        .map_err(|e| pyo3::exceptions::PyOSError::new_err(e.to_string()))
                })
            }

            /// Return the current stream position.
            fn tell(&self, py: Python<'_>) -> PyResult<u64> {
                self.seek(py, 0, Some(1))
            }

            /// Flush the file.
            fn flush(&self, py: Python<'_>) -> PyResult<()> {
                py.detach(|| {
                    let mut guard = self.inner.lock().unwrap();
                    let f = guard
                        .as_mut()
                        .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("file is closed"))?;
                    f.flush()
                        .map_err(|e| pyo3::exceptions::PyOSError::new_err(e.to_string()))
                })
            }

            /// Close the file, committing any writes.
            fn close(&self, py: Python<'_>) -> PyResult<()> {
                py.detach(|| {
                    let mut guard = self.inner.lock().unwrap();
                    if let Some(f) = guard.as_mut() {
                        f.commit().map_err(fs_error_to_pyerr)?;
                    }
                    *guard = None;
                    Ok(())
                })
            }

            /// Whether the file is closed.
            #[getter]
            fn closed(&self) -> bool {
                self.inner.lock().unwrap().is_none()
            }

            fn readable(&self) -> bool {
                self.inner.lock().unwrap().is_some()
            }

            fn writable(&self) -> bool {
                self.inner.lock().unwrap().is_some()
            }

            fn seekable(&self) -> bool {
                self.inner.lock().unwrap().is_some()
            }

            fn __enter__(slf: Py<Self>) -> Py<Self> {
                slf
            }

            fn __exit__(
                &self,
                py: Python<'_>,
                _exc_type: Option<&Bound<'_, pyo3::types::PyAny>>,
                _exc_val: Option<&Bound<'_, pyo3::types::PyAny>>,
                _exc_tb: Option<&Bound<'_, pyo3::types::PyAny>>,
            ) -> PyResult<bool> {
                self.close(py)?;
                Ok(false)
            }

            fn __repr__(&self) -> String {
                if self.inner.lock().unwrap().is_some() {
                    format!(concat!($name, "('{}', open)"), self.path)
                } else {
                    format!(concat!($name, "('{}', closed)"), self.path)
                }
            }
        }
    };
}

/// The methods every filesystem class has. `$fs` needs an `inner: SharedFileSystem`
/// field, and `$file` is its file class.
macro_rules! filesystem_methods {
    ($fs:ident, $file:ident) => {
        #[pymethods]
        impl $fs {
            /// Return the protocol names.
            fn protocol(&self) -> Vec<String> {
                self.inner
                    .protocol()
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            }

            /// List a directory, returning a list of dicts.
            #[pyo3(signature = (path, detail = true))]
            fn ls<'py>(&self, py: Python<'py>, path: &str, detail: bool) -> PyResult<Py<PyAny>> {
                let entries = py
                    .detach(|| self.inner.ls(path, detail))
                    .map_err(fs_error_to_pyerr)?;
                if detail {
                    let list = pyo3::types::PyList::empty(py);
                    for info in entries {
                        let d = PyDict::new(py);
                        d.set_item("name", &info.name)?;
                        d.set_item("size", info.size)?;
                        d.set_item(
                            "type",
                            match info.file_type {
                                fsspec_rs::types::FileType::File => "file",
                                fsspec_rs::types::FileType::Directory => "directory",
                                fsspec_rs::types::FileType::Other => "other",
                            },
                        )?;
                        if let Some(created) = info.created {
                            if let Ok(dur) = created.duration_since(std::time::SystemTime::UNIX_EPOCH) {
                                d.set_item("created", dur.as_secs_f64())?;
                            }
                        }
                        if let Some(modified) = info.modified {
                            if let Ok(dur) = modified.duration_since(std::time::SystemTime::UNIX_EPOCH) {
                                d.set_item("modified", dur.as_secs_f64())?;
                            }
                        }
                        list.append(d)?;
                    }
                    Ok(list.into_any().unbind())
                } else {
                    let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
                    Ok(pyo3::types::PyList::new(py, names)?.into_any().unbind())
                }
            }

            /// Return metadata about a path as a dict.
            fn info<'py>(&self, py: Python<'py>, path: &str) -> PyResult<Bound<'py, PyDict>> {
                let info = py
                    .detach(|| self.inner.info(path))
                    .map_err(fs_error_to_pyerr)?;
                let d = PyDict::new(py);
                d.set_item("name", &info.name)?;
                d.set_item("size", info.size)?;
                d.set_item(
                    "type",
                    match info.file_type {
                        fsspec_rs::types::FileType::File => "file",
                        fsspec_rs::types::FileType::Directory => "directory",
                        fsspec_rs::types::FileType::Other => "other",
                    },
                )?;
                if let Some(created) = info.created {
                    if let Ok(dur) = created.duration_since(std::time::SystemTime::UNIX_EPOCH) {
                        d.set_item("created", dur.as_secs_f64())?;
                    }
                }
                if let Some(modified) = info.modified {
                    if let Ok(dur) = modified.duration_since(std::time::SystemTime::UNIX_EPOCH) {
                        d.set_item("modified", dur.as_secs_f64())?;
                    }
                }
                Ok(d)
            }

            /// Open a file and return a file object.
            ///
            /// # Arguments
            /// * `path` — file path
            /// * `mode` — "rb", "wb", "ab", "xb" (default: "rb")
            /// * `cache_type` — optional cache strategy: "none", "readahead", "block", "all"
            /// * `block_size` — cache block size in bytes (default: 4 MiB)
            /// * `max_blocks` — maximum cached blocks for "block" strategy (default: 32)
            #[pyo3(signature = (path, mode = "rb", cache_type = None, block_size = None, max_blocks = None))]
            fn open(
                &self,
                py: Python<'_>,
                path: &str,
                mode: &str,
                cache_type: Option<&str>,
                block_size: Option<usize>,
                max_blocks: Option<usize>,
            ) -> PyResult<$file> {
                let open_mode = OpenMode::from_str_mode(mode).ok_or_else(|| {
                    pyo3::exceptions::PyValueError::new_err(format!("unsupported mode: {mode}"))
                })?;

                let opts = if cache_type.is_some() || block_size.is_some() || max_blocks.is_some() {
                    let ct = match cache_type {
                        Some(s) => Some(
                            s.parse::<CacheType>()
                                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?,
                        ),
                        None => None,
                    };
                    let mut o = OpenOptions::default();
                    o.cache_type = ct;
                    if let Some(bs) = block_size {
                        o.block_size = bs;
                    }
                    if let Some(mb) = max_blocks {
                        o.max_blocks = mb;
                    }
                    Some(o)
                } else {
                    None
                };

                let f = py
                    .detach(|| self.inner.open(path, open_mode, opts))
                    .map_err(fs_error_to_pyerr)?;
                Ok($file {
                    inner: Mutex::new(Some(f)),
                    path: path.to_string(),
                })
            }

            /// Delete a single file.
            fn rm_file(&self, py: Python<'_>, path: &str) -> PyResult<()> {
                py.detach(|| self.inner.rm_file(path))
                    .map_err(fs_error_to_pyerr)
            }

            /// Remove a file or prefix.
            #[pyo3(signature = (path, recursive = false))]
            fn rm(&self, py: Python<'_>, path: &str, recursive: bool) -> PyResult<()> {
                py.detach(|| self.inner.rm(path, recursive))
                    .map_err(fs_error_to_pyerr)
            }

            /// Copy a single file.
            fn cp_file(&self, py: Python<'_>, src: &str, dst: &str) -> PyResult<()> {
                py.detach(|| self.inner.cp_file(src, dst))
                    .map_err(fs_error_to_pyerr)
            }

            /// Create a directory.
            #[pyo3(signature = (path, create_parents = true))]
            fn mkdir(&self, py: Python<'_>, path: &str, create_parents: bool) -> PyResult<()> {
                py.detach(|| self.inner.mkdir(path, create_parents))
                    .map_err(fs_error_to_pyerr)
            }

            /// Remove an empty directory.
            fn rmdir(&self, py: Python<'_>, path: &str) -> PyResult<()> {
                py.detach(|| self.inner.rmdir(path))
                    .map_err(fs_error_to_pyerr)
            }

            /// Check if a path exists.
            fn exists(&self, py: Python<'_>, path: &str) -> PyResult<bool> {
                py.detach(|| self.inner.exists(path))
                    .map_err(fs_error_to_pyerr)
            }

            /// Check if a path is a directory.
            fn isdir(&self, py: Python<'_>, path: &str) -> PyResult<bool> {
                py.detach(|| self.inner.isdir(path))
                    .map_err(fs_error_to_pyerr)
            }

            /// Check if a path is a file.
            fn isfile(&self, py: Python<'_>, path: &str) -> PyResult<bool> {
                py.detach(|| self.inner.isfile(path))
                    .map_err(fs_error_to_pyerr)
            }

            /// Return size in bytes.
            fn size(&self, py: Python<'_>, path: &str) -> PyResult<u64> {
                py.detach(|| self.inner.size(path))
                    .map_err(fs_error_to_pyerr)
            }

            /// Read file contents as bytes.
            #[pyo3(signature = (path, start = None, end = None))]
            fn cat_file(
                &self,
                py: Python<'_>,
                path: &str,
                start: Option<i64>,
                end: Option<i64>,
            ) -> PyResult<Vec<u8>> {
                py.detach(|| self.inner.cat_file(path, start, end))
                    .map_err(fs_error_to_pyerr)
            }

            /// Write bytes to a file.
            fn pipe_file(&self, py: Python<'_>, path: &str, data: &[u8]) -> PyResult<()> {
                let data = data.to_vec();
                py.detach(|| self.inner.pipe_file(path, &data))
                    .map_err(fs_error_to_pyerr)
            }

            /// First n bytes.
            fn head(&self, py: Python<'_>, path: &str, size: usize) -> PyResult<Vec<u8>> {
                py.detach(|| self.inner.head(path, size))
                    .map_err(fs_error_to_pyerr)
            }

            /// Last n bytes.
            fn tail(&self, py: Python<'_>, path: &str, size: usize) -> PyResult<Vec<u8>> {
                py.detach(|| self.inner.tail(path, size))
                    .map_err(fs_error_to_pyerr)
            }

            /// Walk a directory tree.
            #[pyo3(signature = (path, max_depth = None, topdown = true))]
            fn walk<'py>(
                &self,
                py: Python<'py>,
                path: &str,
                max_depth: Option<usize>,
                topdown: bool,
            ) -> PyResult<Py<PyAny>> {
                let entries = py
                    .detach(|| self.inner.walk(path, max_depth, topdown))
                    .map_err(fs_error_to_pyerr)?;
                let result = pyo3::types::PyList::empty(py);
                for entry in entries {
                    let tuple = pyo3::types::PyTuple::new(
                        py,
                        [
                            pyo3::types::PyString::new(py, &entry.dirpath).as_any(),
                            pyo3::types::PyList::new(py, &entry.dirnames)?.as_any(),
                            pyo3::types::PyList::new(py, &entry.filenames)?.as_any(),
                        ],
                    )?;
                    result.append(tuple)?;
                }
                Ok(result.into_any().unbind())
            }

            /// Find all files below a path.
            #[pyo3(signature = (path, max_depth = None, with_dirs = false))]
            fn find(
                &self,
                py: Python<'_>,
                path: &str,
                max_depth: Option<usize>,
                with_dirs: bool,
            ) -> PyResult<Vec<String>> {
                py.detach(|| self.inner.find(path, max_depth, with_dirs))
                    .map_err(fs_error_to_pyerr)
            }

            /// Read file as UTF-8 string.
            fn read_text(&self, py: Python<'_>, path: &str) -> PyResult<String> {
                py.detach(|| self.inner.read_text(path))
                    .map_err(fs_error_to_pyerr)
            }

            /// Return file-level info as a PyFileInfo.
            fn info_as_fileinfo(&self, py: Python<'_>, path: &str) -> PyResult<PyFileInfo> {
                let info = py
                    .detach(|| self.inner.info(path))
                    .map_err(fs_error_to_pyerr)?;
                Ok(PyFileInfo::from_rust(info))
            }

        }
    };
}

/// The methods the local and memory filesystem classes add.
macro_rules! local_style_methods {
    ($fs:ident) => {
        #[pymethods]
        impl $fs {
            /// Touch a file.
            #[pyo3(signature = (path, truncate = false))]
            fn touch(&self, path: &str, truncate: bool) -> PyResult<()> {
                self.inner.touch(path, truncate).map_err(fs_error_to_pyerr)
            }

            /// Copy files, optionally recursively.
            #[pyo3(signature = (src, dst, recursive = false))]
            fn copy(&self, src: &str, dst: &str, recursive: bool) -> PyResult<()> {
                self.inner
                    .copy(src, dst, recursive)
                    .map_err(fs_error_to_pyerr)
            }

            /// Move a file or directory.
            fn mv(&self, src: &str, dst: &str) -> PyResult<()> {
                self.inner.mv(src, dst).map_err(fs_error_to_pyerr)
            }

            /// Disk usage.
            #[pyo3(signature = (path, total = true))]
            fn du<'py>(&self, py: Python<'py>, path: &str, total: bool) -> PyResult<Py<PyAny>> {
                let result = self.inner.du(path, total).map_err(fs_error_to_pyerr)?;
                match result {
                    fsspec_rs::types::DuResult::Total(n) => {
                        Ok(n.into_pyobject(py)?.into_any().unbind())
                    }
                    fsspec_rs::types::DuResult::PerPath(map) => {
                        let d = PyDict::new(py);
                        for (k, v) in map {
                            d.set_item(k, v)?;
                        }
                        Ok(d.into_any().unbind())
                    }
                }
            }

            /// Create parent directories.
            #[pyo3(signature = (path, exist_ok = false))]
            fn makedirs(&self, path: &str, exist_ok: bool) -> PyResult<()> {
                self.inner
                    .makedirs(path, exist_ok)
                    .map_err(fs_error_to_pyerr)
            }

            /// Write a UTF-8 string to a file.
            fn write_text(&self, path: &str, data: &str) -> PyResult<()> {
                self.inner.write_text(path, data).map_err(fs_error_to_pyerr)
            }

            /// Download from this filesystem to a local path.
            fn get_file(&self, remote: &str, local: &str) -> PyResult<()> {
                self.inner
                    .get_file(remote, local)
                    .map_err(fs_error_to_pyerr)
            }

            /// Upload from a local path to this filesystem.
            fn put_file(&self, local: &str, remote: &str) -> PyResult<()> {
                self.inner
                    .put_file(local, remote)
                    .map_err(fs_error_to_pyerr)
            }
        }
    };
}

pub(crate) use {file_class, filesystem_methods, local_style_methods};
