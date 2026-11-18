use pyo3::exceptions::{PyOSError, PyTypeError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyTuple};

use fsspec_rs::{Condition, ErrorKind, OperationError, Validator};

pyo3::create_exception!(fsspec_rs, ConflictError, PyOSError);
pyo3::create_exception!(fsspec_rs, GapError, PyOSError);
pyo3::create_exception!(fsspec_rs, UnavailableError, PyOSError);
pyo3::create_exception!(fsspec_rs, CorruptError, PyOSError);
pyo3::create_exception!(fsspec_rs, CancelledError, PyOSError);
pyo3::create_exception!(fsspec_rs, UnknownOutcomeError, PyOSError);

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

pub fn error_metadata(error: PyErr, kind: ErrorKind, retry_safe: bool) -> PyErr {
    Python::attach(|py| {
        let value = error.value(py);
        let _ = value.setattr("kind", kind.as_str());
        let _ = value.setattr("retry_safe", retry_safe);
    });
    error
}

pub fn operation_error_to_pyerr(error: OperationError) -> PyErr {
    use pyo3::exceptions::*;
    let kind = error.kind();
    let retry_safe = error.retry_safe();
    let message = error.to_string();
    let exception = match kind {
        ErrorKind::NotFound => PyFileNotFoundError::new_err(message),
        ErrorKind::PermissionDenied => PyPermissionError::new_err(message),
        ErrorKind::AlreadyExists => PyFileExistsError::new_err(message),
        ErrorKind::NotADirectory => PyNotADirectoryError::new_err(message),
        ErrorKind::IsADirectory => PyIsADirectoryError::new_err(message),
        ErrorKind::Invalid => PyValueError::new_err(message),
        ErrorKind::NotSupported => PyNotImplementedError::new_err(message),
        ErrorKind::Conflict => ConflictError::new_err(message),
        ErrorKind::Gap => GapError::new_err(message),
        ErrorKind::Unavailable => UnavailableError::new_err(message),
        ErrorKind::Timeout => PyTimeoutError::new_err(message),
        ErrorKind::Corrupt => CorruptError::new_err(message),
        ErrorKind::Cancelled => CancelledError::new_err(message),
        ErrorKind::UnknownOutcome => UnknownOutcomeError::new_err(message),
        _ => PyOSError::new_err(message),
    };
    error_metadata(exception, kind, retry_safe)
}
