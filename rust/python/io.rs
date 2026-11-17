use pyo3::exceptions::{PyOSError, PyTypeError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyTuple};

use fsspec_rs::{Condition, OperationError, Validator};

pyo3::create_exception!(fsspec_rs, ConflictError, PyOSError);

#[pyclass(name = "Validator", frozen, get_all, eq)]
#[derive(Clone, PartialEq)]
pub struct PyValidator {
    pub scope: String,
    pub value: String,
}

#[pymethods]
impl PyValidator {
    #[new]
    fn new(scope: String, value: String) -> Self {
        Self { scope, value }
    }
}

impl From<Validator> for PyValidator {
    fn from(value: Validator) -> Self {
        Self {
            scope: value.scope,
            value: value.value,
        }
    }
}

#[pyclass(name = "Match", frozen, get_all, eq)]
#[derive(Clone, PartialEq)]
pub struct PyMatch {
    pub validator: PyValidator,
}

#[pymethods]
impl PyMatch {
    #[new]
    fn new(validator: PyRef<'_, PyValidator>) -> Self {
        Self {
            validator: validator.clone(),
        }
    }
}

#[pyclass(name = "Missing", frozen, eq)]
#[derive(Clone, PartialEq)]
pub struct PyMissing;

#[pymethods]
impl PyMissing {
    #[new]
    fn new() -> Self {
        Self
    }
}

#[pyclass(name = "ReadResult", frozen, get_all)]
pub struct PyReadResult {
    pub data: Py<PyBytes>,
    pub validator: Option<PyValidator>,
}

impl PyReadResult {
    pub fn from_rust(py: Python<'_>, result: fsspec_rs::ReadResult) -> Self {
        Self {
            data: PyBytes::new(py, &result.data).unbind(),
            validator: result.validator.map(Into::into),
        }
    }
}

#[pyclass(name = "RangeReadResult", frozen, get_all)]
pub struct PyRangeReadResult {
    pub data: Py<PyTuple>,
    pub validator: Option<PyValidator>,
}

impl PyRangeReadResult {
    pub fn from_rust(py: Python<'_>, result: fsspec_rs::RangeReadResult) -> PyResult<Self> {
        Ok(Self {
            data: PyTuple::new(py, result.data.iter().map(|data| PyBytes::new(py, data)))?.unbind(),
            validator: result.validator.map(Into::into),
        })
    }
}

#[pyclass(name = "WriteResult", frozen, get_all)]
pub struct PyWriteResult {
    pub validator: Option<PyValidator>,
}

#[pyclass(name = "IoCapabilities", frozen, get_all)]
pub struct PyIoCapabilities {
    pub read_match: bool,
    pub write_match: bool,
    pub write_missing: bool,
}

pub fn condition(value: Option<&Bound<'_, PyAny>>) -> PyResult<Option<Condition>> {
    let Some(value) = value else { return Ok(None) };
    if let Ok(matched) = value.extract::<PyRef<'_, PyMatch>>() {
        Ok(Some(Condition::Match(Validator {
            scope: matched.validator.scope.clone(),
            value: matched.validator.value.clone(),
        })))
    } else if value.is_instance_of::<PyMissing>() {
        Ok(Some(Condition::Missing))
    } else {
        Err(PyTypeError::new_err(
            "condition must be Match, Missing, or None",
        ))
    }
}

pub fn operation_error_to_pyerr(error: OperationError) -> PyErr {
    match error {
        OperationError::Conflict(message) => ConflictError::new_err(message),
        OperationError::FileSystem(error) => crate::types::fs_error_to_pyerr(error),
        other => PyOSError::new_err(other.to_string()),
    }
}
