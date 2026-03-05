use pyo3::exceptions::{
    PyFileExistsError, PyFileNotFoundError, PyIsADirectoryError, PyNotADirectoryError,
    PyOSError, PyPermissionError, PyValueError,
};
use pyo3::prelude::*;
use pyo3::types::PyDict;

use fsspec_rs::types::{FileInfo, FileType};
use fsspec_rs::FsError;

// ---- FileType ----

/// Python-visible enum for file types.
#[pyclass(name = "FileType", skip_from_py_object)]
#[derive(Clone, Debug)]
pub struct PyFileType {
    inner: FileType,
}

#[pymethods]
impl PyFileType {
    /// Create a FileType representing a regular file.
    #[staticmethod]
    fn file() -> Self {
        PyFileType {
            inner: FileType::File,
        }
    }

    /// Create a FileType representing a directory.
    #[staticmethod]
    fn directory() -> Self {
        PyFileType {
            inner: FileType::Directory,
        }
    }

    /// Create a FileType representing an other entry.
    #[staticmethod]
    fn other() -> Self {
        PyFileType {
            inner: FileType::Other,
        }
    }

    fn __str__(&self) -> String {
        format!("{}", self.inner)
    }

    fn __repr__(&self) -> String {
        format!("FileType.{}", self.inner)
    }

    fn __eq__(&self, other: &PyFileType) -> bool {
        self.inner == other.inner
    }

    /// Return the string representation used by fsspec ("file", "directory", "other").
    fn as_str(&self) -> &str {
        match self.inner {
            FileType::File => "file",
            FileType::Directory => "directory",
            FileType::Other => "other",
        }
    }
}

impl PyFileType {
    pub fn from_rust(ft: FileType) -> Self {
        PyFileType { inner: ft }
    }

    pub fn to_rust(&self) -> FileType {
        self.inner.clone()
    }
}

// ---- FileInfo ----

/// Python-visible file metadata, analogous to the dict returned by fsspec's info().
#[pyclass(name = "FileInfo", skip_from_py_object)]
#[derive(Clone, Debug)]
pub struct PyFileInfo {
    inner: FileInfo,
}

#[pymethods]
impl PyFileInfo {
    /// Create a new FileInfo.
    #[new]
    #[pyo3(signature = (name, size=0, file_type=None))]
    fn py_new(name: String, size: u64, file_type: Option<&str>) -> PyResult<Self> {
        let ft = match file_type.unwrap_or("file") {
            "file" => FileType::File,
            "directory" => FileType::Directory,
            "other" => FileType::Other,
            other => {
                return Err(PyValueError::new_err(format!(
                    "unknown file type: {other}"
                )))
            }
        };
        Ok(PyFileInfo {
            inner: FileInfo {
                name,
                size,
                file_type: ft,
                created: None,
                modified: None,
                extra: std::collections::HashMap::new(),
            },
        })
    }

    /// The full path name.
    #[getter]
    fn name(&self) -> &str {
        &self.inner.name
    }

    /// Size in bytes.
    #[getter]
    fn size(&self) -> u64 {
        self.inner.size
    }

    /// The file type as a string ("file", "directory", "other").
    #[getter]
    fn file_type(&self) -> &str {
        match self.inner.file_type {
            FileType::File => "file",
            FileType::Directory => "directory",
            FileType::Other => "other",
        }
    }

    /// Whether this is a regular file.
    fn is_file(&self) -> bool {
        self.inner.is_file()
    }

    /// Whether this is a directory.
    fn is_dir(&self) -> bool {
        self.inner.is_dir()
    }

    /// Convert to a Python dict matching fsspec's info() format.
    fn to_dict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let d = PyDict::new(py);
        d.set_item("name", &self.inner.name)?;
        d.set_item("size", self.inner.size)?;
        d.set_item(
            "type",
            match self.inner.file_type {
                FileType::File => "file",
                FileType::Directory => "directory",
                FileType::Other => "other",
            },
        )?;
        Ok(d)
    }

    fn __str__(&self) -> String {
        format!(
            "FileInfo(name={}, size={}, type={})",
            self.inner.name, self.inner.size, self.inner.file_type
        )
    }

    fn __repr__(&self) -> String {
        format!(
            "FileInfo(name='{}', size={}, file_type='{}')",
            self.inner.name, self.inner.size, self.inner.file_type
        )
    }

    fn __eq__(&self, other: &PyFileInfo) -> bool {
        self.inner.name == other.inner.name
            && self.inner.size == other.inner.size
            && self.inner.file_type == other.inner.file_type
    }
}

impl PyFileInfo {
    /// Create from a Rust FileInfo.
    pub fn from_rust(info: FileInfo) -> Self {
        PyFileInfo { inner: info }
    }

    /// Convert to a Rust FileInfo.
    pub fn to_rust(&self) -> FileInfo {
        self.inner.clone()
    }
}

// ---- Error conversion ----

/// Convert an FsError into a Python exception.
#[allow(dead_code)]
pub fn fs_error_to_pyerr(err: FsError) -> PyErr {
    match err {
        FsError::NotFound(msg) => PyFileNotFoundError::new_err(msg),
        FsError::PermissionDenied(msg) => PyPermissionError::new_err(msg),
        FsError::AlreadyExists(msg) => PyFileExistsError::new_err(msg),
        FsError::NotADirectory(msg) => PyNotADirectoryError::new_err(msg),
        FsError::IsADirectory(msg) => PyIsADirectoryError::new_err(msg),
        FsError::IoError(err) => PyOSError::new_err(err.to_string()),
        FsError::InvalidArgument(msg) => PyValueError::new_err(msg),
        FsError::NotSupported(msg) => PyOSError::new_err(format!("not supported: {msg}")),
        FsError::Other(msg) => PyOSError::new_err(msg),
    }
}
