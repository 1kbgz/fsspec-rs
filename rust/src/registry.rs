//! A registry mapping URL protocols to filesystem factories, the Rust counterpart of
//! fsspec's `fsspec.specs` entry points. Rust has no runtime plugin discovery, so
//! backend crates export a factory and applications register it.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, Mutex, PoisonError, RwLock};

use crate::error::{FsError, FsResult};
use crate::fs::FileSystem;
use crate::local::LocalFs;
use crate::s3::{S3Config, S3Fs};

/// A filesystem shared between callers.
pub type SharedFileSystem = Arc<dyn FileSystem + Send + Sync>;

/// String storage options for one filesystem, like the keyword arguments of
/// `fsspec.filesystem`.
pub type FsOptions = BTreeMap<String, String>;

/// Builds filesystems for one protocol.
pub trait FilesystemFactory: Send + Sync {
    /// Builds a filesystem for `path`, the URL without its `protocol://` prefix.
    fn create(&self, path: &str, options: &FsOptions) -> FsResult<SharedFileSystem>;

    /// The part of `path` that needs its own instance, such as an S3 bucket. Cached
    /// instances are keyed by protocol, this scope, and the options.
    fn instance_scope(&self, _path: &str) -> String {
        String::new()
    }
}

impl<F> FilesystemFactory for F
where
    F: Fn(&str, &FsOptions) -> FsResult<SharedFileSystem> + Send + Sync,
{
    fn create(&self, path: &str, options: &FsOptions) -> FsResult<SharedFileSystem> {
        self(path, options)
    }
}

type InstanceKey = (String, String, FsOptions);

/// Factories keyed by protocol, with a cache of the filesystems they built.
pub struct FilesystemRegistry {
    factories: HashMap<String, Arc<dyn FilesystemFactory>>,
    instances: Mutex<HashMap<InstanceKey, SharedFileSystem>>,
}

impl Default for FilesystemRegistry {
    /// A registry with `file` and `local` ([`LocalFs`]) and `s3` ([`S3Fs`]).
    fn default() -> Self {
        let mut registry = Self::empty();
        registry.register("file", local_filesystem);
        registry.register("local", local_filesystem);
        registry.register("s3", S3Factory);
        registry
    }
}

impl FilesystemRegistry {
    /// A registry with no protocols.
    pub fn empty() -> Self {
        Self {
            factories: HashMap::new(),
            instances: Mutex::new(HashMap::new()),
        }
    }

    /// Registers `factory` for `protocol`, replacing any earlier factory and dropping
    /// the instances it built.
    pub fn register<F: FilesystemFactory + 'static>(&mut self, protocol: &str, factory: F) {
        self.factories
            .insert(protocol.to_string(), Arc::new(factory));
        self.instances
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(cached, _, _), _| cached != protocol);
    }

    /// Registered protocols in sorted order.
    pub fn protocols(&self) -> Vec<String> {
        let mut protocols: Vec<_> = self.factories.keys().cloned().collect();
        protocols.sort();
        protocols
    }

    /// Returns the filesystem for `url` and the path to pass to it. A URL without
    /// `protocol://` is a local path. Equal protocols, scopes, and options share one
    /// instance.
    pub fn url_to_fs(
        &self,
        url: &str,
        options: &FsOptions,
    ) -> FsResult<(SharedFileSystem, String)> {
        let (protocol, path) = split_protocol(url)?;
        let factory = self.factory(protocol)?;
        let key = (
            protocol.to_string(),
            factory.instance_scope(path),
            options.clone(),
        );
        let instances = || {
            self.instances
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
        };
        let cached = instances().get(&key).cloned();
        let fs = match cached {
            Some(fs) => fs,
            // Build without holding the lock, since a factory may resolve other URLs.
            None => {
                let fs = factory.create(path, options)?;
                instances().entry(key).or_insert(fs).clone()
            }
        };
        let path = fs.strip_protocol(path);
        Ok((fs, path))
    }

    /// Like [`url_to_fs`](Self::url_to_fs), but always builds a new filesystem and
    /// leaves the cache unchanged.
    pub fn url_to_fs_uncached(
        &self,
        url: &str,
        options: &FsOptions,
    ) -> FsResult<(SharedFileSystem, String)> {
        let (protocol, path) = split_protocol(url)?;
        let fs = self.factory(protocol)?.create(path, options)?;
        let path = fs.strip_protocol(path);
        Ok((fs, path))
    }

    fn factory(&self, protocol: &str) -> FsResult<Arc<dyn FilesystemFactory>> {
        self.factories.get(protocol).cloned().ok_or_else(|| {
            FsError::NotSupported(format!("no filesystem registered for protocol: {protocol}"))
        })
    }
}

/// The process-wide registry used by [`register_filesystem`] and [`url_to_fs`].
pub static DEFAULT_FILESYSTEMS: LazyLock<RwLock<FilesystemRegistry>> =
    LazyLock::new(|| RwLock::new(FilesystemRegistry::default()));

/// Registers `factory` for `protocol` in [`DEFAULT_FILESYSTEMS`].
pub fn register_filesystem<F: FilesystemFactory + 'static>(protocol: &str, factory: F) {
    DEFAULT_FILESYSTEMS
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .register(protocol, factory);
}

/// Resolves `url` through [`DEFAULT_FILESYSTEMS`]; see [`FilesystemRegistry::url_to_fs`].
pub fn url_to_fs(url: &str, options: &FsOptions) -> FsResult<(SharedFileSystem, String)> {
    DEFAULT_FILESYSTEMS
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .url_to_fs(url, options)
}

fn split_protocol(url: &str) -> FsResult<(&str, &str)> {
    if url.contains("::") {
        return Err(FsError::NotSupported(format!(
            "chained URLs are not supported: {url}"
        )));
    }
    Ok(url.split_once("://").unwrap_or(("file", url)))
}

fn local_filesystem(_path: &str, options: &FsOptions) -> FsResult<SharedFileSystem> {
    let mut auto_mkdir = false;
    for (key, value) in options {
        match key.as_str() {
            "auto_mkdir" => auto_mkdir = parse_bool(key, value)?,
            _ => return Err(unknown_option("local", key)),
        }
    }
    Ok(Arc::new(LocalFs::with_auto_mkdir(auto_mkdir)))
}

/// Builds one [`S3Fs`] per bucket from the same option names as the Python `S3Fs`:
/// `key`, `secret`, `token`, `endpoint_url`, `region`, and `anon`, plus
/// `virtual_hosted_style_request`.
struct S3Factory;

impl FilesystemFactory for S3Factory {
    fn create(&self, path: &str, options: &FsOptions) -> FsResult<SharedFileSystem> {
        let bucket = self.instance_scope(path);
        if bucket.is_empty() {
            return Err(FsError::InvalidArgument(
                "S3 URL must include a bucket".to_string(),
            ));
        }
        let mut config = S3Config::new(bucket);
        for (key, value) in options {
            match key.as_str() {
                "key" => config.access_key_id = Some(value.clone()),
                "secret" => config.secret_access_key = Some(value.clone()),
                "token" => config.session_token = Some(value.clone()),
                "endpoint_url" => config.endpoint_url = Some(value.clone()),
                "region" => config.region = Some(value.clone()),
                "anon" => config.anon = parse_bool(key, value)?,
                "virtual_hosted_style_request" => {
                    config.virtual_hosted_style_request = parse_bool(key, value)?
                }
                _ => return Err(unknown_option("s3", key)),
            }
        }
        Ok(Arc::new(S3Fs::new(config)?))
    }

    fn instance_scope(&self, path: &str) -> String {
        path.split('/').next().unwrap_or_default().to_string()
    }
}

fn parse_bool(key: &str, value: &str) -> FsResult<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(FsError::InvalidArgument(format!(
            "{key} must be true or false, got {value:?}"
        ))),
    }
}

fn unknown_option(protocol: &str, key: &str) -> FsError {
    FsError::InvalidArgument(format!("unknown {protocol} option: {key}"))
}
