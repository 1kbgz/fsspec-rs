//! A registry mapping URL protocols to filesystem factories, the Rust counterpart of
//! fsspec's `fsspec.specs` entry points. Rust has no runtime plugin discovery, so
//! backend crates export a factory and applications register it.

use std::collections::{BTreeMap, HashMap};
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
use std::sync::LazyLock;
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};

use crate::error::{FsError, FsResult};
use crate::fs::FileSystem;
use crate::local::LocalFs;
use crate::memory::MemoryFs;
use crate::platform::{MaybeSend, MaybeSync};
#[cfg(feature = "s3")]
use crate::s3::{S3ClientOptions, S3Config, S3Fs};

/// A filesystem shared between callers.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub type SharedFileSystem = Arc<dyn FileSystem + Send + Sync>;
/// A filesystem shared by callers on one browser thread; not transferable between workers.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub type SharedFileSystem = Arc<dyn FileSystem>;

/// String storage options for one filesystem, like the keyword arguments of
/// `fsspec.filesystem`.
pub type FsOptions = BTreeMap<String, String>;

/// Builds filesystems for one protocol.
pub trait FilesystemFactory: MaybeSend + MaybeSync {
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
    F: Fn(&str, &FsOptions) -> FsResult<SharedFileSystem> + MaybeSend + MaybeSync,
{
    fn create(&self, path: &str, options: &FsOptions) -> FsResult<SharedFileSystem> {
        self(path, options)
    }
}

impl FilesystemFactory for Box<dyn FilesystemFactory> {
    fn create(&self, path: &str, options: &FsOptions) -> FsResult<SharedFileSystem> {
        (**self).create(path, options)
    }

    fn instance_scope(&self, path: &str) -> String {
        (**self).instance_scope(path)
    }
}

type InstanceKey = (String, String, FsOptions);

enum Lookup<'a> {
    Cached(SharedFileSystem, String),
    Pending(Pending<'a>),
}

/// A factory chosen for a URL that has not built its filesystem yet.
struct Pending<'a> {
    factory: Arc<dyn FilesystemFactory>,
    key: InstanceKey,
    path: &'a str,
}

/// Factories keyed by protocol, with a cache of the filesystems they built.
pub struct FilesystemRegistry {
    factories: HashMap<String, Arc<dyn FilesystemFactory>>,
    fallback: Option<Arc<dyn FilesystemFactory>>,
    instances: Mutex<HashMap<InstanceKey, SharedFileSystem>>,
}

impl Default for FilesystemRegistry {
    /// A registry with `file` and `local` ([`LocalFs`]), `memory` ([`MemoryFs`]), and, with
    /// the `s3` feature, `s3` (`S3Fs`).
    fn default() -> Self {
        let mut registry = Self::empty();
        registry.register("file", local_filesystem);
        registry.register("local", local_filesystem);
        registry.register("memory", memory_filesystem);
        #[cfg(feature = "s3")]
        registry.register("s3", S3Factory);
        registry
    }
}

impl FilesystemRegistry {
    /// A registry with no protocols.
    pub fn empty() -> Self {
        Self {
            factories: HashMap::new(),
            fallback: None,
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
        let pending = match self.lookup(url, options)? {
            Lookup::Cached(fs, path) => return Ok((fs, path)),
            Lookup::Pending(pending) => pending,
        };
        let fs = pending.factory.create(pending.path, options)?;
        Ok(self.cache(pending, fs))
    }

    /// Like [`url_to_fs`](Self::url_to_fs) for a registry behind a lock, but holds the
    /// lock only to look up and cache, never while a factory runs. A factory may then
    /// register filesystems in the same registry, as a Python backend does when the
    /// fallback imports it.
    pub fn url_to_fs_shared(
        registry: &RwLock<Self>,
        url: &str,
        options: &FsOptions,
    ) -> FsResult<(SharedFileSystem, String)> {
        let read = || registry.read().unwrap_or_else(PoisonError::into_inner);
        let pending = match read().lookup(url, options)? {
            Lookup::Cached(fs, path) => return Ok((fs, path)),
            Lookup::Pending(pending) => pending,
        };
        let fs = pending.factory.create(pending.path, options)?;
        Ok(read().cache(pending, fs))
    }

    fn lookup<'a>(&self, url: &'a str, options: &FsOptions) -> FsResult<Lookup<'a>> {
        let (protocol, path) = split_protocol(url)?;
        let (factory, path) = self.factory(protocol, url, path)?;
        let key = (
            protocol.to_string(),
            factory.instance_scope(path),
            options.clone(),
        );
        let cached = self
            .instances
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned();
        Ok(match cached {
            Some(fs) => {
                let path = fs.strip_protocol(path);
                Lookup::Cached(fs, path)
            }
            None => Lookup::Pending(Pending { factory, key, path }),
        })
    }

    /// Caches `fs` unless its factory was replaced while it was built, and returns the
    /// cached instance with its path.
    fn cache(&self, pending: Pending<'_>, fs: SharedFileSystem) -> (SharedFileSystem, String) {
        let current = self
            .factory(&pending.key.0, "", "")
            .is_ok_and(|(factory, _)| Arc::ptr_eq(&factory, &pending.factory));
        let fs = if current {
            self.instances
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(pending.key)
                .or_insert(fs)
                .clone()
        } else {
            fs
        };
        let path = fs.strip_protocol(pending.path);
        (fs, path)
    }

    /// Like [`url_to_fs`](Self::url_to_fs), but always builds a new filesystem and
    /// leaves the cache unchanged.
    pub fn url_to_fs_uncached(
        &self,
        url: &str,
        options: &FsOptions,
    ) -> FsResult<(SharedFileSystem, String)> {
        let (protocol, path) = split_protocol(url)?;
        let (factory, path) = self.factory(protocol, url, path)?;
        let fs = factory.create(path, options)?;
        let path = fs.strip_protocol(path);
        Ok((fs, path))
    }

    /// Installs the factory used for protocols nothing registered, such as one that
    /// opens any Python fsspec backend. It is given the whole URL rather than the
    /// path, since it has to pick a backend from the protocol.
    pub fn set_fallback<F: FilesystemFactory + 'static>(&mut self, factory: Option<F>) {
        self.fallback = factory.map(|factory| Arc::new(factory) as Arc<dyn FilesystemFactory>);
        self.instances
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(protocol, _, _), _| self.factories.contains_key(protocol));
    }

    /// The factory for `protocol` and the argument it takes: the path, or the whole
    /// URL for the fallback.
    fn factory<'a>(
        &self,
        protocol: &str,
        url: &'a str,
        path: &'a str,
    ) -> FsResult<(Arc<dyn FilesystemFactory>, &'a str)> {
        if let Some(factory) = self.factories.get(protocol) {
            return Ok((factory.clone(), path));
        }
        match &self.fallback {
            Some(fallback) => Ok((fallback.clone(), url)),
            None => Err(FsError::NotSupported(format!(
                "no filesystem registered for protocol: {protocol}"
            ))),
        }
    }
}

/// The process-wide registry used by [`register_filesystem`] and [`url_to_fs`].
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub static DEFAULT_FILESYSTEMS: LazyLock<RwLock<FilesystemRegistry>> =
    LazyLock::new(|| RwLock::new(FilesystemRegistry::default()));

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
thread_local! {
    /// The default registry for this browser thread. Factories run outside its locks.
    pub static DEFAULT_FILESYSTEMS: RwLock<FilesystemRegistry> =
        RwLock::new(FilesystemRegistry::default());
}

fn with_default_registry<T>(f: impl FnOnce(&RwLock<FilesystemRegistry>) -> T) -> T {
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    {
        DEFAULT_FILESYSTEMS.with(f)
    }
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    {
        f(&DEFAULT_FILESYSTEMS)
    }
}

/// Routes [`register_filesystem`] and [`url_to_fs`] to another registry, such as the
/// one inside a shared `libfsspec`, so every crate in the process that only knows
/// fsspec_rs still sees one set of protocols.
pub struct RegistryHooks {
    pub register: fn(&str, Box<dyn FilesystemFactory>) -> FsResult<()>,
    pub set_fallback: fn(Option<Box<dyn FilesystemFactory>>) -> FsResult<()>,
    pub url_to_fs: fn(&str, &FsOptions) -> FsResult<(SharedFileSystem, String)>,
}

static HOOKS: OnceLock<RegistryHooks> = OnceLock::new();

/// Installs `hooks` for the rest of the process. Only the first call takes effect;
/// a later call gets its hooks back.
pub fn set_registry_hooks(hooks: RegistryHooks) -> Result<(), RegistryHooks> {
    HOOKS.set(hooks)
}

/// Registers `factory` for `protocol` in [`DEFAULT_FILESYSTEMS`], or through the
/// installed [`RegistryHooks`].
pub fn try_register_filesystem<F: FilesystemFactory + 'static>(
    protocol: &str,
    factory: F,
) -> FsResult<()> {
    match HOOKS.get() {
        Some(hooks) => (hooks.register)(protocol, Box::new(factory)),
        None => {
            with_default_registry(|registry| {
                registry
                    .write()
                    .unwrap_or_else(PoisonError::into_inner)
                    .register(protocol, factory);
            });
            Ok(())
        }
    }
}

/// Registers `factory` for `protocol`; see [`try_register_filesystem`].
///
/// # Panics
///
/// When installed [`RegistryHooks`] reject the registration.
pub fn register_filesystem<F: FilesystemFactory + 'static>(protocol: &str, factory: F) {
    try_register_filesystem(protocol, factory).expect("registering a filesystem factory");
}

/// Installs the fallback factory of [`DEFAULT_FILESYSTEMS`], or of the registry the
/// installed [`RegistryHooks`] point at; see [`FilesystemRegistry::set_fallback`].
pub fn set_fallback_filesystem<F: FilesystemFactory + 'static>(factory: Option<F>) -> FsResult<()> {
    match HOOKS.get() {
        Some(hooks) => (hooks.set_fallback)(
            factory.map(|factory| Box::new(factory) as Box<dyn FilesystemFactory>),
        ),
        None => {
            with_default_registry(|registry| {
                registry
                    .write()
                    .unwrap_or_else(PoisonError::into_inner)
                    .set_fallback(factory);
            });
            Ok(())
        }
    }
}

/// Resolves `url` through [`DEFAULT_FILESYSTEMS`] or the installed [`RegistryHooks`];
/// see [`FilesystemRegistry::url_to_fs`].
pub fn url_to_fs(url: &str, options: &FsOptions) -> FsResult<(SharedFileSystem, String)> {
    if let Some(hooks) = HOOKS.get() {
        return (hooks.url_to_fs)(url, options);
    }
    with_default_registry(|registry| FilesystemRegistry::url_to_fs_shared(registry, url, options))
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

/// One [`MemoryFs`] per process: the registry caches the instance, and it takes no options.
fn memory_filesystem(_path: &str, options: &FsOptions) -> FsResult<SharedFileSystem> {
    if let Some(key) = options.keys().next() {
        return Err(unknown_option("memory", key));
    }
    Ok(Arc::new(MemoryFs::new()))
}

/// Builds one [`S3Fs`] per bucket from the same option names as the Python `S3Fs`:
/// `key`, `secret`, `token`, `endpoint_url`, `region`, and `anon`, plus
/// `virtual_hosted_style_request`, `connect_timeout`, `request_timeout`, and `max_retries`.
#[cfg(feature = "s3")]
struct S3Factory;

#[cfg(feature = "s3")]
impl FilesystemFactory for S3Factory {
    fn create(&self, path: &str, options: &FsOptions) -> FsResult<SharedFileSystem> {
        let bucket = self.instance_scope(path);
        if bucket.is_empty() {
            return Err(FsError::InvalidArgument(
                "S3 URL must include a bucket".to_string(),
            ));
        }
        Ok(Arc::new(S3Fs::from_options(&bucket, options)?))
    }

    fn instance_scope(&self, path: &str) -> String {
        path.split('/').next().unwrap_or_default().to_string()
    }
}

#[cfg(feature = "s3")]
fn parse_timeout(key: &str, value: &str) -> FsResult<std::time::Duration> {
    let invalid = || FsError::InvalidArgument(format!("{key} must be finite positive seconds"));
    let seconds: f64 = value.parse().map_err(|_| invalid())?;
    let duration = std::time::Duration::try_from_secs_f64(seconds).map_err(|_| invalid())?;
    if duration.is_zero() {
        return Err(invalid());
    }
    Ok(duration)
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

#[cfg(feature = "s3")]
impl S3Fs {
    /// Construct a built-in S3 client explicitly, without registry lookup or caching.
    /// Uses the same option validation as the registered S3 factory.
    pub fn from_options(bucket: &str, options: &FsOptions) -> FsResult<Self> {
        let (config, client) = s3_options(bucket, options)?;
        S3Fs::with_client_options(config, client)
    }
}

#[cfg(feature = "s3")]
pub(crate) fn s3_options(
    bucket: &str,
    options: &FsOptions,
) -> FsResult<(S3Config, S3ClientOptions)> {
    if bucket.is_empty() || bucket.contains('/') {
        return Err(FsError::InvalidArgument(
            "expected one nonempty bucket name".into(),
        ));
    }
    let mut config = S3Config::new(bucket);
    let mut client = S3ClientOptions::default();
    for (key, value) in options {
        match key.as_str() {
            "connect_timeout" => client.connect_timeout = Some(parse_timeout(key, value)?),
            "request_timeout" => client.request_timeout = Some(parse_timeout(key, value)?),
            "max_retries" => {
                client.max_retries = Some(value.parse().map_err(|_| {
                    FsError::InvalidArgument("max_retries must be a nonnegative integer".into())
                })?)
            }
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
    Ok((config, client))
}
