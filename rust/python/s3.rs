use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Mutex;

use fsspec_rs::types::{OpenMode, OpenOptions};
use fsspec_rs::{CacheType, FileSystem, S3Config, S3Fs};

use crate::types::{fs_error_to_pyerr, PyFileInfo};

// ============================================================================
// RustS3File — wraps a Rust S3File for Python
// ============================================================================

/// Python-visible wrapper for an S3-backed opened file.
#[pyclass(name = "RustS3File")]
pub struct RustS3File {
    inner: Mutex<Option<Box<dyn fsspec_rs::FsFile>>>,
    path: String,
}

#[pymethods]
impl RustS3File {
    /// Read up to `size` bytes. If `size` is -1 or None, read to EOF.
    #[pyo3(signature = (size = -1))]
    fn read(&self, size: i64) -> PyResult<Vec<u8>> {
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
    }

    /// Write bytes to the file.
    fn write(&self, data: &[u8]) -> PyResult<usize> {
        let mut guard = self.inner.lock().unwrap();
        let f = guard
            .as_mut()
            .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("file is closed"))?;
        let n = f
            .write(data)
            .map_err(|e| pyo3::exceptions::PyOSError::new_err(e.to_string()))?;
        Ok(n)
    }

    /// Seek to a position in the file.
    fn seek(&self, offset: i64, whence: Option<u8>) -> PyResult<u64> {
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
    }

    /// Return the current stream position.
    fn tell(&self) -> PyResult<u64> {
        self.seek(0, Some(1))
    }

    /// Flush the file.
    fn flush(&self) -> PyResult<()> {
        let mut guard = self.inner.lock().unwrap();
        let f = guard
            .as_mut()
            .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("file is closed"))?;
        f.flush()
            .map_err(|e| pyo3::exceptions::PyOSError::new_err(e.to_string()))
    }

    /// Close the file (triggers upload for write modes).
    fn close(&self) -> PyResult<()> {
        let mut guard = self.inner.lock().unwrap();
        *guard = None;
        Ok(())
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
        _exc_type: Option<&Bound<'_, pyo3::types::PyAny>>,
        _exc_val: Option<&Bound<'_, pyo3::types::PyAny>>,
        _exc_tb: Option<&Bound<'_, pyo3::types::PyAny>>,
    ) -> PyResult<bool> {
        self.close()?;
        Ok(false)
    }

    fn __repr__(&self) -> String {
        if self.inner.lock().unwrap().is_some() {
            format!("RustS3File('{}', open)", self.path)
        } else {
            format!("RustS3File('{}', closed)", self.path)
        }
    }
}

// ============================================================================
// RustS3Fs — wraps a Rust S3Fs for Python
// ============================================================================

/// Python-visible wrapper for the Rust-accelerated S3Fs.
///
/// This class exposes all FileSystem trait methods to Python,
/// intended to be used as a delegate inside a proper fsspec-inheriting
/// Python class.
#[pyclass(name = "RustS3Fs")]
pub struct RustS3Fs {
    inner: S3Fs,
}

#[pymethods]
impl RustS3Fs {
    /// Create a new RustS3Fs.
    ///
    /// # Arguments
    /// * `bucket` — S3 bucket name (required)
    /// * `key` — access key id
    /// * `secret` — secret access key
    /// * `endpoint_url` — custom endpoint (for B2, MinIO, etc.)
    /// * `region` — AWS region
    /// * `token` — session token
    /// * `anon` — anonymous access (no signing)
    /// * `client_kwargs` — optional dict; `endpoint_url` is extracted if present
    #[new]
    #[pyo3(signature = (
        bucket,
        key = None,
        secret = None,
        endpoint_url = None,
        region = None,
        token = None,
        anon = false,
        client_kwargs = None,
    ))]
    fn py_new(
        bucket: &str,
        key: Option<&str>,
        secret: Option<&str>,
        endpoint_url: Option<&str>,
        region: Option<&str>,
        token: Option<&str>,
        anon: bool,
        client_kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        // Allow endpoint_url to be overridden via client_kwargs (matching s3fs convention)
        let effective_endpoint = if endpoint_url.is_some() {
            endpoint_url.map(|s| s.to_string())
        } else if let Some(kwargs) = client_kwargs {
            kwargs
                .get_item("endpoint_url")
                .ok()
                .flatten()
                .and_then(|v| v.extract::<String>().ok())
        } else {
            None
        };

        let cfg = S3Config {
            bucket: bucket.to_string(),
            region: region.map(|s| s.to_string()),
            endpoint_url: effective_endpoint,
            access_key_id: key.map(|s| s.to_string()),
            secret_access_key: secret.map(|s| s.to_string()),
            session_token: token.map(|s| s.to_string()),
            anon,
            virtual_hosted_style_request: false,
        };

        let inner = S3Fs::new(cfg).map_err(fs_error_to_pyerr)?;
        Ok(RustS3Fs { inner })
    }

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
        let entries = self.inner.ls(path, detail).map_err(fs_error_to_pyerr)?;
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
        let info = self.inner.info(path).map_err(fs_error_to_pyerr)?;
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

    /// Open a file and return a RustS3File.
    ///
    /// # Arguments
    /// * `path` — S3 key (or full s3://bucket/key path)
    /// * `mode` — "rb", "wb", "ab", "xb" (default: "rb")
    /// * `cache_type` — optional cache strategy: "none", "readahead", "block", "all"
    /// * `block_size` — cache block size in bytes (default: 4 MiB)
    /// * `max_blocks` — maximum cached blocks for "block" strategy (default: 32)
    #[pyo3(signature = (path, mode = "rb", cache_type = None, block_size = None, max_blocks = None))]
    fn open(
        &self,
        path: &str,
        mode: &str,
        cache_type: Option<&str>,
        block_size: Option<usize>,
        max_blocks: Option<usize>,
    ) -> PyResult<RustS3File> {
        let open_mode = OpenMode::from_str_mode(mode).ok_or_else(|| {
            pyo3::exceptions::PyValueError::new_err(format!("unsupported mode: {mode}"))
        })?;

        let opts = if cache_type.is_some() || block_size.is_some() || max_blocks.is_some() {
            let ct = match cache_type {
                Some(s) => Some(CacheType::from_str(s).ok_or_else(|| {
                    pyo3::exceptions::PyValueError::new_err(format!(
                        "unknown cache_type: {s}. Valid: none, readahead, block, all"
                    ))
                })?),
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

        let f = self
            .inner
            .open(path, open_mode, opts)
            .map_err(fs_error_to_pyerr)?;
        Ok(RustS3File {
            inner: Mutex::new(Some(f)),
            path: path.to_string(),
        })
    }

    /// Delete a single file.
    fn rm_file(&self, path: &str) -> PyResult<()> {
        self.inner.rm_file(path).map_err(fs_error_to_pyerr)
    }

    /// Copy a single file within S3.
    fn cp_file(&self, src: &str, dst: &str) -> PyResult<()> {
        self.inner.cp_file(src, dst).map_err(fs_error_to_pyerr)
    }

    /// Create a directory (no-op on S3).
    #[pyo3(signature = (path, create_parents = true))]
    fn mkdir(&self, path: &str, create_parents: bool) -> PyResult<()> {
        self.inner
            .mkdir(path, create_parents)
            .map_err(fs_error_to_pyerr)
    }

    /// Remove a directory (no-op on S3).
    fn rmdir(&self, path: &str) -> PyResult<()> {
        self.inner.rmdir(path).map_err(fs_error_to_pyerr)
    }

    /// Check if a path exists.
    fn exists(&self, path: &str) -> PyResult<bool> {
        self.inner.exists(path).map_err(fs_error_to_pyerr)
    }

    /// Check if a path is a directory.
    fn isdir(&self, path: &str) -> PyResult<bool> {
        self.inner.isdir(path).map_err(fs_error_to_pyerr)
    }

    /// Check if a path is a file.
    fn isfile(&self, path: &str) -> PyResult<bool> {
        self.inner.isfile(path).map_err(fs_error_to_pyerr)
    }

    /// Return size in bytes.
    fn size(&self, path: &str) -> PyResult<u64> {
        self.inner.size(path).map_err(fs_error_to_pyerr)
    }

    /// Read file contents as bytes.
    #[pyo3(signature = (path, start = None, end = None))]
    fn cat_file(&self, path: &str, start: Option<i64>, end: Option<i64>) -> PyResult<Vec<u8>> {
        self.inner
            .cat_file(path, start, end)
            .map_err(fs_error_to_pyerr)
    }

    /// Write bytes to a file.
    fn pipe_file(&self, path: &str, data: &[u8]) -> PyResult<()> {
        self.inner.pipe_file(path, data).map_err(fs_error_to_pyerr)
    }

    /// First n bytes.
    fn head(&self, path: &str, size: usize) -> PyResult<Vec<u8>> {
        self.inner.head(path, size).map_err(fs_error_to_pyerr)
    }

    /// Last n bytes.
    fn tail(&self, path: &str, size: usize) -> PyResult<Vec<u8>> {
        self.inner.tail(path, size).map_err(fs_error_to_pyerr)
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
        let entries = self
            .inner
            .walk(path, max_depth, topdown)
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
        path: &str,
        max_depth: Option<usize>,
        with_dirs: bool,
    ) -> PyResult<Vec<String>> {
        self.inner
            .find(path, max_depth, with_dirs)
            .map_err(fs_error_to_pyerr)
    }

    /// Read file as UTF-8 string.
    fn read_text(&self, path: &str) -> PyResult<String> {
        self.inner.read_text(path).map_err(fs_error_to_pyerr)
    }

    /// Return file-level info as a PyFileInfo.
    fn info_as_fileinfo(&self, path: &str) -> PyResult<PyFileInfo> {
        let info = self.inner.info(path).map_err(fs_error_to_pyerr)?;
        Ok(PyFileInfo::from_rust(info))
    }

    /// Return the bucket name.
    #[getter]
    fn bucket(&self) -> &str {
        &self.inner.bucket
    }

    fn __repr__(&self) -> String {
        format!("RustS3Fs(bucket='{}')", self.inner.bucket)
    }
}
