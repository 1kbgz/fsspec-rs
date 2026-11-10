//! Where the extension's filesystems come from.
//!
//! With the `capi` feature (wheels and conda builds) every filesystem is a
//! handle into `libfsspec`, so the registry and the backends exist once per
//! process and are shared with other extensions. With `direct` (Pyodide and
//! debugging) the crate is compiled in.

use fsspec_rs::{FsResult, S3Config, SharedFileSystem};

/// Whether this extension talks to a shared `libfsspec` rather than its own
/// copy of the backends.
pub const LINKED_TO_LIBFSSPEC: bool = cfg!(feature = "capi");

#[cfg(feature = "capi")]
mod route {
    use fsspec_rs::{FsOptions, FsResult, S3Config, SharedFileSystem};

    pub fn local(auto_mkdir: bool) -> FsResult<SharedFileSystem> {
        let mut options = FsOptions::new();
        options.insert("auto_mkdir".to_string(), auto_mkdir.to_string());
        Ok(fsspec_sys::url_to_fs("file://", &options)?.0)
    }

    pub fn s3(config: S3Config) -> FsResult<SharedFileSystem> {
        let mut options = FsOptions::new();
        let strings = [
            ("key", config.access_key_id),
            ("secret", config.secret_access_key),
            ("token", config.session_token),
            ("endpoint_url", config.endpoint_url),
            ("region", config.region),
        ];
        for (key, value) in strings {
            if let Some(value) = value {
                options.insert(key.to_string(), value);
            }
        }
        options.insert("anon".to_string(), config.anon.to_string());
        options.insert(
            "virtual_hosted_style_request".to_string(),
            config.virtual_hosted_style_request.to_string(),
        );
        Ok(fsspec_sys::url_to_fs(&format!("s3://{}", config.bucket), &options)?.0)
    }

    pub fn protocols() -> FsResult<Vec<String>> {
        fsspec_sys::protocols()
    }
}

#[cfg(not(feature = "capi"))]
mod route {
    use std::sync::{Arc, PoisonError};

    use fsspec_rs::{FsResult, LocalFs, S3Config, S3Fs, SharedFileSystem, DEFAULT_FILESYSTEMS};

    pub fn local(auto_mkdir: bool) -> FsResult<SharedFileSystem> {
        Ok(Arc::new(LocalFs::with_auto_mkdir(auto_mkdir)))
    }

    pub fn s3(config: S3Config) -> FsResult<SharedFileSystem> {
        Ok(Arc::new(S3Fs::new(config)?))
    }

    pub fn protocols() -> FsResult<Vec<String>> {
        Ok(DEFAULT_FILESYSTEMS
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .protocols())
    }
}

pub fn local(auto_mkdir: bool) -> FsResult<SharedFileSystem> {
    route::local(auto_mkdir)
}

pub fn s3(config: S3Config) -> FsResult<SharedFileSystem> {
    route::s3(config)
}

pub fn protocols() -> FsResult<Vec<String>> {
    route::protocols()
}
