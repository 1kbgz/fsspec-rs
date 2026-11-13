use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Mutex;

use fsspec_rs::types::{OpenMode, OpenOptions};
use fsspec_rs::{CacheType, SharedFileSystem};

use crate::backend;
use crate::types::{fs_error_to_pyerr, PyFileInfo};
use crate::wrapper::{file_class, filesystem_methods, local_style_methods};

file_class!(
    /// Python-visible wrapper for a file opened in memory.
    RustMemoryFile,
    "RustMemoryFile"
);

/// Python-visible wrapper for the Rust MemoryFs. Every instance shares the store
/// behind the registry's `memory` protocol.
///
/// This class exposes all FileSystem trait methods to Python,
/// intended to be used as a delegate inside a proper fsspec-inheriting
/// Python class.
#[pyclass(name = "RustMemoryFs")]
pub struct RustMemoryFs {
    inner: SharedFileSystem,
}

#[pymethods]
impl RustMemoryFs {
    #[new]
    fn py_new() -> PyResult<Self> {
        Ok(RustMemoryFs {
            inner: backend::memory().map_err(fs_error_to_pyerr)?,
        })
    }

    fn __repr__(&self) -> String {
        "RustMemoryFs()".to_string()
    }
}

filesystem_methods!(RustMemoryFs, RustMemoryFile);
local_style_methods!(RustMemoryFs);
