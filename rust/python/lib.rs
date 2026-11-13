#![allow(dead_code)]

use pyo3::prelude::*;
use pyo3::types::PyDict;

mod backend;
mod local;
mod memory;
mod s3;
mod types;
mod wrapper;

pub use local::{RustLocalFile, RustLocalFs};
pub use memory::{RustMemoryFile, RustMemoryFs};
pub use s3::{RustS3File, RustS3Fs};
pub use types::{PyFileInfo, PyFileType};

#[pymodule]
fn fsspec_rs(py: Python, m: &Bound<PyModule>) -> PyResult<()> {
    // Core types
    m.add_class::<PyFileType>()?;
    m.add_class::<PyFileInfo>()?;

    // Local filesystem
    m.add_class::<RustLocalFs>()?;
    m.add_class::<RustLocalFile>()?;

    // In-memory filesystem
    m.add_class::<RustMemoryFs>()?;
    m.add_class::<RustMemoryFile>()?;

    // S3 filesystem
    m.add_class::<RustS3Fs>()?;
    m.add_class::<RustS3File>()?;

    // Add error types as module attributes
    let errors = PyDict::new(py);
    errors.set_item("NotFound", "NotFound")?;
    errors.set_item("PermissionDenied", "PermissionDenied")?;
    errors.set_item("AlreadyExists", "AlreadyExists")?;
    errors.set_item("NotADirectory", "NotADirectory")?;
    errors.set_item("IsADirectory", "IsADirectory")?;
    m.add("error_codes", errors)?;

    m.add_function(wrap_pyfunction!(protocols, m)?)?;
    m.add_function(wrap_pyfunction!(linked_to_libfsspec, m)?)?;

    // In a Python process every fsspec backend is reachable from the registry.
    backend::install_python_fallback().map_err(types::fs_error_to_pyerr)?;

    Ok(())
}

/// The protocols in the registry this extension uses, sorted.
#[pyfunction]
fn protocols() -> PyResult<Vec<String>> {
    backend::protocols().map_err(types::fs_error_to_pyerr)
}

/// Whether the extension reaches the backends through the shared libfsspec
/// rather than its own copy.
#[pyfunction]
fn linked_to_libfsspec() -> bool {
    backend::LINKED_TO_LIBFSSPEC
}
