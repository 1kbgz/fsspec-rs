//! Where the extension's filesystems come from.
//!
//! With the `capi` feature (wheels and conda builds) every filesystem is a
//! handle into `libfsspec`, so the registry and the backends exist once per
//! process and are shared with other extensions. S3 uses an explicit built-in
//! constructor to pair filesystem and object handles, bypassing registry lookup
//! and caching. With `direct` (Pyodide and
//! debugging) the crate is compiled in.

use std::collections::HashMap;
use std::sync::Arc;

use fsspec_rs::{
    FilesystemFactory, FsOptions, FsResult, S3ClientOptions, S3Config, SharedFileSystem,
};

/// Whether this extension talks to a shared `libfsspec` rather than its own
/// copy of the backends.
pub const LINKED_TO_LIBFSSPEC: bool = cfg!(feature = "capi");

#[cfg(feature = "capi")]
mod route {
    use fsspec_rs::{FsOptions, FsResult, S3ClientOptions, S3Config, SharedFileSystem};

    pub fn local(auto_mkdir: bool) -> FsResult<SharedFileSystem> {
        let mut options = FsOptions::new();
        options.insert("auto_mkdir".to_string(), auto_mkdir.to_string());
        Ok(fsspec_sys::url_to_fs("file://", &options)?.0)
    }

    pub fn memory() -> FsResult<SharedFileSystem> {
        Ok(fsspec_sys::url_to_fs("memory://", &FsOptions::new())?.0)
    }

    pub type S3Objects = fsspec_sys::ForeignObjectStore;

    pub fn s3(
        config: S3Config,
        client: S3ClientOptions,
    ) -> FsResult<(SharedFileSystem, S3Objects)> {
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
        for (name, timeout) in [
            ("connect_timeout", client.connect_timeout),
            ("request_timeout", client.request_timeout),
        ] {
            if let Some(timeout) = timeout {
                options.insert(name.into(), timeout.as_secs_f64().to_string());
            }
        }
        if let Some(max_retries) = client.max_retries {
            options.insert("max_retries".into(), max_retries.to_string());
        }
        options.insert("anon".to_string(), config.anon.to_string());
        options.insert(
            "virtual_hosted_style_request".to_string(),
            config.virtual_hosted_style_request.to_string(),
        );
        fsspec_sys::s3_with_objects(&config.bucket, &options)
    }

    pub fn protocols() -> FsResult<Vec<String>> {
        fsspec_sys::protocols()
    }
}

#[cfg(not(feature = "capi"))]
mod route {
    use std::sync::{Arc, PoisonError};

    use fsspec_rs::{
        FsResult, LocalFs, S3ClientOptions, S3Config, S3Fs, SharedFileSystem, DEFAULT_FILESYSTEMS,
    };

    pub fn local(auto_mkdir: bool) -> FsResult<SharedFileSystem> {
        Ok(Arc::new(LocalFs::with_auto_mkdir(auto_mkdir)))
    }

    pub fn memory() -> FsResult<SharedFileSystem> {
        Ok(fsspec_rs::url_to_fs("memory://", &fsspec_rs::FsOptions::new())?.0)
    }

    pub type S3Objects = fsspec_rs::S3ObjectReader;

    pub fn s3(
        config: S3Config,
        client: S3ClientOptions,
    ) -> FsResult<(SharedFileSystem, S3Objects)> {
        let fs = Arc::new(S3Fs::with_client_options(config, client)?);
        let objects = fs.objects();
        Ok((fs, objects))
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

/// The process's one in-memory filesystem, the registry's `memory` instance.
pub fn memory() -> FsResult<SharedFileSystem> {
    route::memory()
}

pub use route::S3Objects;

pub fn s3(config: S3Config, client: S3ClientOptions) -> FsResult<(SharedFileSystem, S3Objects)> {
    route::s3(config, client)
}

pub fn protocols() -> FsResult<Vec<String>> {
    route::protocols()
}

/// Opens protocols the registry does not know through Python's own fsspec
/// registry, so a Rust consumer in this process sees every Python backend.
struct PythonFallback;

impl FilesystemFactory for PythonFallback {
    fn create(&self, url: &str, options: &FsOptions) -> FsResult<SharedFileSystem> {
        let options: HashMap<String, String> = options.clone().into_iter().collect();
        let (fs, _) = fsspec_rs_bridge::url_to_fs(url, &options)?;
        Ok(Arc::new(fs))
    }
}

/// Installs [`PythonFallback`] in the registry this extension uses: the shared
/// `libfsspec` with the `capi` feature, so other extensions and C consumers get
/// it too, or the registry compiled in otherwise.
pub fn install_python_fallback() -> FsResult<()> {
    #[cfg(feature = "capi")]
    fsspec_sys::use_shared_registry();
    fsspec_rs::set_fallback_filesystem(Some(PythonFallback))
}
