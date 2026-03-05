#![allow(dead_code)]

use pyo3::prelude::*;
use pyo3::types::PyDict;

mod local;
mod s3;
mod types;

pub use local::{RustLocalFile, RustLocalFs};
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

    Ok(())
}
