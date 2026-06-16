use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};

use fsspec_rs::{FileInfo, FileSystem, FileType, FsError, FsFile, FsResult, OpenMode, OpenOptions};
use pyo3::exceptions::{
    PyFileExistsError, PyFileNotFoundError, PyIsADirectoryError, PyNotADirectoryError,
    PyPermissionError, PyValueError,
};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyTuple};

const BRIDGE_PROTOCOL: &[&str] = &["python"];

pub type StorageOptions = HashMap<String, String>;

pub fn url_to_fs(url: &str, storage_options: &StorageOptions) -> FsResult<(PyFsspecFs, String)> {
    Python::initialize();
    Python::attach(|py| {
        let fsspec = py
            .import("fsspec")
            .map_err(|err| pyerr_to_fs_error(py, err))?;
        let kwargs = storage_options_kwargs(py, storage_options)?;
        let pair = fsspec
            .getattr("core")
            .and_then(|core| core.call_method("url_to_fs", (url,), Some(&kwargs)))
            .map_err(|err| pyerr_to_fs_error(py, err))?;
        let pair = pair
            .cast::<PyTuple>()
            .map_err(|err| pyerr_to_fs_error(py, err.into()))?;
        let fs = pair
            .get_item(0)
            .map_err(|err| pyerr_to_fs_error(py, err))?
            .unbind();
        let token = pair
            .get_item(1)
            .and_then(|value| value.extract::<String>())
            .map_err(|err| pyerr_to_fs_error(py, err))?;
        let fs = PyFsspecFs::from_py_fs(fs)?;
        let start = if token.is_empty() {
            fs.root_marker().to_string()
        } else {
            token
        };
        Ok((fs, start))
    })
}

pub struct PyFsspecFs {
    fs: Py<PyAny>,
    source_protocol: String,
    root_marker: String,
    sep: String,
}

impl PyFsspecFs {
    pub fn from_protocol(protocol: &str, storage_options: &StorageOptions) -> FsResult<Self> {
        Python::initialize();
        Python::attach(|py| {
            let fsspec = py
                .import("fsspec")
                .map_err(|err| pyerr_to_fs_error(py, err))?;
            let kwargs = storage_options_kwargs(py, storage_options)?;
            let fs = fsspec
                .call_method("filesystem", (protocol,), Some(&kwargs))
                .map_err(|err| pyerr_to_fs_error(py, err))?
                .unbind();
            Self::from_py_fs(fs)
        })
    }

    pub fn from_py_fs(fs: Py<PyAny>) -> FsResult<Self> {
        Python::initialize();
        Python::attach(|py| {
            let bound = fs.bind(py);
            let source_protocol = bound
                .getattr("protocol")
                .and_then(|value| protocol_to_string(&value))
                .unwrap_or_else(|_| "python".to_string());
            let root_marker = bound
                .getattr("root_marker")
                .and_then(|value| value.extract::<String>())
                .unwrap_or_else(|_| "/".to_string());
            let sep = bound
                .getattr("sep")
                .and_then(|value| value.extract::<String>())
                .unwrap_or_else(|_| "/".to_string());
            Ok(Self {
                fs,
                source_protocol,
                root_marker,
                sep,
            })
        })
    }

    pub fn source_protocol(&self) -> &str {
        &self.source_protocol
    }
}

impl FileSystem for PyFsspecFs {
    fn protocol(&self) -> &[&str] {
        BRIDGE_PROTOCOL
    }

    fn root_marker(&self) -> &str {
        &self.root_marker
    }

    fn sep(&self) -> &str {
        &self.sep
    }

    fn ls(&self, path: &str, detail: bool) -> FsResult<Vec<FileInfo>> {
        Python::attach(|py| {
            let value = self
                .fs
                .bind(py)
                .call_method1("ls", (path, detail))
                .map_err(|err| pyerr_to_fs_error(py, err))?;
            py_file_infos(py, &value)
        })
    }

    fn rm_file(&self, path: &str) -> FsResult<()> {
        Python::attach(|py| {
            self.fs
                .bind(py)
                .call_method1("rm_file", (path,))
                .map(|_| ())
                .map_err(|err| pyerr_to_fs_error(py, err))
        })
    }

    fn cp_file(&self, src: &str, dst: &str) -> FsResult<()> {
        Python::attach(|py| {
            self.fs
                .bind(py)
                .call_method1("cp_file", (src, dst))
                .map(|_| ())
                .map_err(|err| pyerr_to_fs_error(py, err))
        })
    }

    fn open(
        &self,
        path: &str,
        mode: OpenMode,
        _opts: Option<OpenOptions>,
    ) -> FsResult<Box<dyn FsFile>> {
        Python::attach(|py| {
            let file = self
                .fs
                .bind(py)
                .call_method1("open", (path, mode.as_str()))
                .map_err(|err| pyerr_to_fs_error(py, err))?
                .unbind();
            Ok(Box::new(PyFsspecFile {
                fs: self.fs.clone_ref(py),
                file,
                path: path.to_string(),
            }) as Box<dyn FsFile>)
        })
    }

    fn info(&self, path: &str) -> FsResult<FileInfo> {
        Python::attach(|py| {
            let value = self
                .fs
                .bind(py)
                .call_method1("info", (path,))
                .map_err(|err| pyerr_to_fs_error(py, err))?;
            py_file_info(&value).map_err(|err| pyerr_to_fs_error(py, err))
        })
    }

    fn mkdir(&self, path: &str, create_parents: bool) -> FsResult<()> {
        Python::attach(|py| {
            self.fs
                .bind(py)
                .call_method1("mkdir", (path, create_parents))
                .map(|_| ())
                .map_err(|err| pyerr_to_fs_error(py, err))
        })
    }

    fn rmdir(&self, path: &str) -> FsResult<()> {
        Python::attach(|py| {
            self.fs
                .bind(py)
                .call_method1("rmdir", (path,))
                .map(|_| ())
                .map_err(|err| pyerr_to_fs_error(py, err))
        })
    }

    fn cat_file(&self, path: &str, start: Option<i64>, end: Option<i64>) -> FsResult<Vec<u8>> {
        Python::attach(|py| {
            self.fs
                .bind(py)
                .call_method1("cat_file", (path, start, end))
                .and_then(|value| value.extract::<Vec<u8>>())
                .map_err(|err| pyerr_to_fs_error(py, err))
        })
    }

    fn pipe_file(&self, path: &str, data: &[u8]) -> FsResult<()> {
        Python::attach(|py| {
            let data = PyBytes::new(py, data);
            self.fs
                .bind(py)
                .call_method1("pipe_file", (path, data))
                .map(|_| ())
                .map_err(|err| pyerr_to_fs_error(py, err))
        })
    }

    fn get_file(&self, remote: &str, local: &str) -> FsResult<()> {
        Python::attach(|py| {
            self.fs
                .bind(py)
                .call_method1("get_file", (remote, local))
                .map(|_| ())
                .map_err(|err| pyerr_to_fs_error(py, err))
        })
    }

    fn put_file(&self, local: &str, remote: &str) -> FsResult<()> {
        Python::attach(|py| {
            self.fs
                .bind(py)
                .call_method1("put_file", (local, remote))
                .map(|_| ())
                .map_err(|err| pyerr_to_fs_error(py, err))
        })
    }
}

pub struct PyFsspecFile {
    fs: Py<PyAny>,
    file: Py<PyAny>,
    path: String,
}

impl Read for PyFsspecFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        Python::attach(|py| {
            let data = self
                .file
                .bind(py)
                .call_method1("read", (buf.len(),))
                .and_then(|value| value.extract::<Vec<u8>>())
                .map_err(|err| pyerr_to_io_error(py, err))?;
            let len = data.len().min(buf.len());
            buf[..len].copy_from_slice(&data[..len]);
            Ok(len)
        })
    }
}

impl Write for PyFsspecFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Python::attach(|py| {
            let data = PyBytes::new(py, buf);
            let value = self
                .file
                .bind(py)
                .call_method1("write", (data,))
                .map_err(|err| pyerr_to_io_error(py, err))?;
            if value.is_none() {
                return Ok(buf.len());
            }
            value
                .extract::<usize>()
                .map_err(|err| pyerr_to_io_error(py, err))
        })
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Python::attach(|py| {
            if let Ok(flush) = self.file.bind(py).getattr("flush") {
                flush.call0().map_err(|err| pyerr_to_io_error(py, err))?;
            }
            Ok(())
        })
    }
}

impl Seek for PyFsspecFile {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let (offset, whence) = match pos {
            SeekFrom::Start(offset) => (offset as i64, 0),
            SeekFrom::End(offset) => (offset, 2),
            SeekFrom::Current(offset) => (offset, 1),
        };
        Python::attach(|py| {
            self.file
                .bind(py)
                .call_method1("seek", (offset, whence))
                .and_then(|value| value.extract::<u64>())
                .map_err(|err| pyerr_to_io_error(py, err))
        })
    }
}

impl FsFile for PyFsspecFile {
    fn info(&self) -> FsResult<FileInfo> {
        Python::attach(|py| {
            let value = self
                .fs
                .bind(py)
                .call_method1("info", (self.path.as_str(),))
                .map_err(|err| pyerr_to_fs_error(py, err))?;
            py_file_info(&value).map_err(|err| pyerr_to_fs_error(py, err))
        })
    }

    fn size(&self) -> FsResult<Option<u64>> {
        Ok(Some(self.info()?.size))
    }

    fn commit(&mut self) -> FsResult<()> {
        self.flush()?;
        Python::attach(|py| {
            if let Ok(commit) = self.file.bind(py).getattr("commit") {
                commit.call0().map_err(|err| pyerr_to_fs_error(py, err))?;
            }
            Ok(())
        })
    }

    fn discard(&mut self) -> FsResult<()> {
        Python::attach(|py| {
            if let Ok(discard) = self.file.bind(py).getattr("discard") {
                discard.call0().map_err(|err| pyerr_to_fs_error(py, err))?;
            }
            Ok(())
        })
    }
}

impl Drop for PyFsspecFile {
    fn drop(&mut self) {
        Python::attach(|py| {
            if let Ok(close) = self.file.bind(py).getattr("close") {
                let _ = close.call0();
            }
        });
    }
}

fn storage_options_kwargs<'py>(
    py: Python<'py>,
    storage_options: &StorageOptions,
) -> FsResult<Bound<'py, PyDict>> {
    let kwargs = PyDict::new(py);
    for (key, value) in storage_options {
        kwargs
            .set_item(key, value)
            .map_err(|err| pyerr_to_fs_error(py, err))?;
    }
    Ok(kwargs)
}

fn protocol_to_string(value: &Bound<'_, PyAny>) -> PyResult<String> {
    if let Ok(protocol) = value.extract::<String>() {
        return Ok(protocol);
    }
    if let Ok(protocols) = value.extract::<Vec<String>>() {
        return Ok(protocols
            .first()
            .cloned()
            .unwrap_or_else(|| "python".to_string()));
    }
    Ok(value.str()?.to_str()?.to_string())
}

fn py_file_infos(py: Python<'_>, value: &Bound<'_, PyAny>) -> FsResult<Vec<FileInfo>> {
    if let Ok(items) = value.try_iter() {
        let mut entries = Vec::new();
        for item in items {
            let item = item.map_err(|err| pyerr_to_fs_error(py, err))?;
            if item.cast::<PyDict>().is_ok() {
                entries.push(py_file_info(&item).map_err(|err| pyerr_to_fs_error(py, err))?);
            } else {
                let name = item
                    .extract::<String>()
                    .map_err(|err| pyerr_to_fs_error(py, err))?;
                entries.push(FileInfo::file(name, 0));
            }
        }
        Ok(entries)
    } else {
        Err(FsError::InvalidArgument(
            "ls() did not return an iterable".to_string(),
        ))
    }
}

fn py_file_info(value: &Bound<'_, PyAny>) -> PyResult<FileInfo> {
    let name = required_py_string(value, "name")?;
    let size = optional_py_u64(value, "size")?.unwrap_or(0);
    let file_type = match optional_py_string(value, "type")?.as_deref() {
        Some("directory" | "dir") => FileType::Directory,
        Some("file") => FileType::File,
        _ => FileType::Other,
    };
    let mut info = FileInfo {
        name,
        size,
        file_type,
        created: None,
        modified: None,
        extra: HashMap::new(),
    };
    if let Ok(dict) = value.cast::<PyDict>() {
        for (key, value) in dict.iter() {
            let key = key.extract::<String>()?;
            if matches!(key.as_str(), "name" | "size" | "type") || value.is_none() {
                continue;
            }
            info.extra.insert(key, py_value_to_string(&value)?);
        }
    }
    Ok(info)
}

fn optional_py_field<'py>(
    obj: &Bound<'py, PyAny>,
    name: &str,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    if let Ok(dict) = obj.cast::<PyDict>() {
        return dict.get_item(name);
    }
    obj.getattr_opt(name)
}

fn required_py_field<'py>(obj: &Bound<'py, PyAny>, name: &str) -> PyResult<Bound<'py, PyAny>> {
    optional_py_field(obj, name)?
        .ok_or_else(|| PyValueError::new_err(format!("missing field: {name}")))
}

fn required_py_string(obj: &Bound<'_, PyAny>, name: &str) -> PyResult<String> {
    required_py_field(obj, name)?.extract::<String>()
}

fn optional_py_string(obj: &Bound<'_, PyAny>, name: &str) -> PyResult<Option<String>> {
    match optional_py_field(obj, name)? {
        Some(value) if value.is_none() => Ok(None),
        Some(value) => value.extract::<String>().map(Some),
        None => Ok(None),
    }
}

fn optional_py_u64(obj: &Bound<'_, PyAny>, name: &str) -> PyResult<Option<u64>> {
    match optional_py_field(obj, name)? {
        Some(value) if value.is_none() => Ok(None),
        Some(value) => {
            if let Ok(value) = value.extract::<u64>() {
                return Ok(Some(value));
            }
            let value = value.extract::<i64>()?;
            Ok(Some(value.max(0) as u64))
        }
        None => Ok(None),
    }
}

fn py_value_to_string(value: &Bound<'_, PyAny>) -> PyResult<String> {
    if let Ok(value) = value.extract::<String>() {
        return Ok(value);
    }
    if let Ok(value) = value.extract::<bool>() {
        return Ok(value.to_string());
    }
    if let Ok(value) = value.extract::<i64>() {
        return Ok(value.to_string());
    }
    if let Ok(value) = value.extract::<u64>() {
        return Ok(value.to_string());
    }
    if let Ok(value) = value.extract::<f64>() {
        return Ok(value.to_string());
    }
    Ok(value.str()?.to_str()?.to_string())
}

fn pyerr_to_io_error(py: Python<'_>, err: PyErr) -> std::io::Error {
    std::io::Error::other(pyerr_to_fs_error(py, err).to_string())
}

fn pyerr_to_fs_error(py: Python<'_>, err: PyErr) -> FsError {
    let message = err.to_string();
    if err.is_instance_of::<PyFileNotFoundError>(py) {
        FsError::NotFound(message)
    } else if err.is_instance_of::<PyPermissionError>(py) {
        FsError::PermissionDenied(message)
    } else if err.is_instance_of::<PyFileExistsError>(py) {
        FsError::AlreadyExists(message)
    } else if err.is_instance_of::<PyNotADirectoryError>(py) {
        FsError::NotADirectory(message)
    } else if err.is_instance_of::<PyIsADirectoryError>(py) {
        FsError::IsADirectory(message)
    } else if err.is_instance_of::<PyValueError>(py) {
        FsError::InvalidArgument(message)
    } else {
        FsError::Other(message)
    }
}
