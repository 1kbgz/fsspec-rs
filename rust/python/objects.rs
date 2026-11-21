use crate::{backend::S3Objects, io::operation_error_to_pyerr};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyTuple};

type ObjectMetadata = (u64, u64, u32, Option<String>, Option<String>);

/// Internal delegate for the separate Python obspec adapter.
#[pyclass]
pub struct RustS3ObjectStore {
    inner: S3Objects,
}
impl RustS3ObjectStore {
    pub fn new(inner: S3Objects) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl RustS3ObjectStore {
    fn head(&self, py: Python<'_>, key: &str) -> PyResult<ObjectMetadata> {
        let meta = py
            .detach(|| self.inner.head(key))
            .map_err(operation_error_to_pyerr)?;
        let modified = meta
            .modified
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .ok_or_else(|| {
                operation_error_to_pyerr(
                    fsspec_rs::FsError::NotSupported(
                        "object modification time is unavailable".into(),
                    )
                    .into(),
                )
            })?;
        Ok((
            meta.size,
            modified.as_secs(),
            modified.subsec_nanos(),
            meta.extra.get("etag").cloned(),
            meta.extra.get("version").cloned(),
        ))
    }

    fn read_ranges(
        &self,
        py: Python<'_>,
        key: &str,
        ranges: Vec<(i64, i64)>,
    ) -> PyResult<Py<PyTuple>> {
        let ranges: Vec<_> = ranges
            .into_iter()
            .map(|(start, end)| fsspec_rs::ByteRange {
                start: Some(start),
                end: Some(end),
            })
            .collect();
        let result = py
            .detach(|| self.inner.read_ranges(key, &ranges))
            .map_err(operation_error_to_pyerr)?;
        Ok(PyTuple::new(py, result.data.iter().map(|b| PyBytes::new(py, b)))?.unbind())
    }
}
