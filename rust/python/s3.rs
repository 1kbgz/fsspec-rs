use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Mutex;

use fsspec_rs::types::{OpenMode, OpenOptions};
use fsspec_rs::{CacheType, S3Config, SharedFileSystem};

use crate::backend;
use crate::types::{fs_error_to_pyerr, PyFileInfo};
use crate::wrapper::{file_class, filesystem_methods};

file_class!(
    /// Python-visible wrapper for an S3-backed opened file.
    RustS3File,
    "RustS3File"
);

/// Python-visible wrapper for the Rust-accelerated S3Fs.
///
/// This class exposes all FileSystem trait methods to Python,
/// intended to be used as a delegate inside a proper fsspec-inheriting
/// Python class.
#[pyclass(name = "RustS3Fs")]
pub struct RustS3Fs {
    inner: SharedFileSystem,
    bucket: String,
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

        let bucket = cfg.bucket.clone();
        let inner = backend::s3(cfg).map_err(fs_error_to_pyerr)?;
        Ok(RustS3Fs { inner, bucket })
    }

    /// Return the bucket name.
    #[getter]
    fn bucket(&self) -> &str {
        &self.bucket
    }

    fn __repr__(&self) -> String {
        format!("RustS3Fs(bucket='{}')", self.bucket)
    }
}

filesystem_methods!(RustS3Fs, RustS3File);
