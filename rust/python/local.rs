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
    /// Python-visible wrapper for a locally opened file.
    RustLocalFile,
    "RustLocalFile"
);

/// Python-visible wrapper for the Rust-accelerated LocalFs.
///
/// This class exposes all FileSystem trait methods to Python,
/// intended to be used as a delegate inside a proper fsspec-inheriting
/// Python class.
#[pyclass(name = "RustLocalFs")]
pub struct RustLocalFs {
    inner: SharedFileSystem,
    auto_mkdir: bool,
}

#[pymethods]
impl RustLocalFs {
    #[new]
    #[pyo3(signature = (auto_mkdir = false))]
    fn py_new(auto_mkdir: bool) -> PyResult<Self> {
        Ok(RustLocalFs {
            inner: backend::local(auto_mkdir).map_err(fs_error_to_pyerr)?,
            auto_mkdir,
        })
    }

    fn __repr__(&self) -> String {
        format!("RustLocalFs(auto_mkdir={})", self.auto_mkdir)
    }
}

filesystem_methods!(RustLocalFs, RustLocalFile);
local_style_methods!(RustLocalFs);
