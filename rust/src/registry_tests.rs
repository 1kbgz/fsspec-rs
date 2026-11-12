use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::registry::{
    set_registry_hooks, try_register_filesystem, url_to_fs, FilesystemRegistry, FsOptions,
    RegistryHooks, SharedFileSystem, DEFAULT_FILESYSTEMS,
};
use crate::{FsError, FsResult, LocalFs};

fn options(pairs: &[(&str, &str)]) -> FsOptions {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

#[test]
fn registered_factories_resolve_urls_and_share_instances() {
    let built = Arc::new(AtomicUsize::new(0));
    let counter = built.clone();
    let mut registry = FilesystemRegistry::empty();
    registry.register("memo", move |path: &str, _: &FsOptions| {
        assert_eq!(path, "bucket/a.txt");
        counter.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(LocalFs::new()) as SharedFileSystem)
    });

    let (first, path) = registry
        .url_to_fs("memo://bucket/a.txt", &FsOptions::new())
        .unwrap();
    let (second, _) = registry
        .url_to_fs("memo://bucket/a.txt", &FsOptions::new())
        .unwrap();
    let (other, _) = registry
        .url_to_fs("memo://bucket/a.txt", &options(&[("token", "x")]))
        .unwrap();
    let (uncached, _) = registry
        .url_to_fs_uncached("memo://bucket/a.txt", &FsOptions::new())
        .unwrap();

    assert_eq!(path, "bucket/a.txt");
    assert!(Arc::ptr_eq(&first, &second));
    assert!(!Arc::ptr_eq(&first, &other));
    assert!(!Arc::ptr_eq(&first, &uncached));
    assert_eq!(built.load(Ordering::SeqCst), 3);
    assert_eq!(registry.protocols(), ["memo"]);

    registry.register("memo", |_: &str, _: &FsOptions| {
        Ok(Arc::new(LocalFs::with_auto_mkdir(true)) as SharedFileSystem)
    });
    let (replaced, _) = registry
        .url_to_fs("memo://bucket/a.txt", &FsOptions::new())
        .unwrap();
    assert!(!Arc::ptr_eq(&first, &replaced));
}

#[test]
fn default_registry_resolves_local_paths() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    let file = file.to_str().unwrap();
    let registry = FilesystemRegistry::default();

    let (fs, path) = registry
        .url_to_fs(&format!("file://{file}"), &FsOptions::new())
        .unwrap();
    fs.pipe_file(&path, b"hello").unwrap();
    assert_eq!(path, file);

    let (bare, bare_path) = registry.url_to_fs(file, &FsOptions::new()).unwrap();
    assert!(Arc::ptr_eq(&fs, &bare));
    assert_eq!(bare.cat_file(&bare_path, None, None).unwrap(), b"hello");

    let nested = dir.path().join("new/b.txt");
    let (fs, path) = registry
        .url_to_fs(
            &format!("local://{}", nested.to_str().unwrap()),
            &options(&[("auto_mkdir", "true")]),
        )
        .unwrap();
    fs.pipe_file(&path, b"x").unwrap();
    assert!(nested.exists());
}

#[cfg(feature = "s3")]
#[test]
fn default_registry_builds_one_s3_filesystem_per_bucket() {
    let registry = FilesystemRegistry::default();
    let settings = options(&[("endpoint_url", "http://localhost:9000"), ("anon", "true")]);

    let (first, path) = registry.url_to_fs("s3://one/a/b.txt", &settings).unwrap();
    let (same, _) = registry.url_to_fs("s3://one/c.txt", &settings).unwrap();
    let (other, _) = registry.url_to_fs("s3://two/c.txt", &settings).unwrap();

    assert_eq!(path, "a/b.txt");
    assert!(Arc::ptr_eq(&first, &same));
    assert!(!Arc::ptr_eq(&first, &other));
    assert_eq!(first.unstrip_protocol(&path), "s3://one/a/b.txt");
}

#[test]
fn rejects_unknown_protocols_options_and_chained_urls() {
    let registry = FilesystemRegistry::default();
    let message = |result: crate::FsResult<(SharedFileSystem, String)>| match result {
        Err(error) => error.to_string(),
        Ok(_) => panic!("expected an error"),
    };

    assert_eq!(
        message(registry.url_to_fs("gs://bucket/a", &FsOptions::new())),
        "not supported: no filesystem registered for protocol: gs"
    );
    #[cfg(feature = "s3")]
    {
        assert_eq!(
            message(registry.url_to_fs("s3://", &FsOptions::new())),
            "invalid argument: S3 URL must include a bucket"
        );
        assert_eq!(
            message(registry.url_to_fs("s3://b/a", &options(&[("secrets", "x")]))),
            "invalid argument: unknown s3 option: secrets"
        );
    }
    assert_eq!(
        message(registry.url_to_fs("/tmp/a", &options(&[("auto_mkdir", "yes")]))),
        "invalid argument: auto_mkdir must be true or false, got \"yes\""
    );
    assert!(matches!(
        registry.url_to_fs("simplecache::s3://b/a", &FsOptions::new()),
        Err(FsError::NotSupported(_))
    ));
}

#[test]
fn hooks_route_the_process_wide_functions() {
    // The hooks are process-wide, so these forward to the default registry and only count.
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    let hooks = RegistryHooks {
        register: |protocol, factory| {
            CALLS.fetch_add(1, Ordering::SeqCst);
            DEFAULT_FILESYSTEMS
                .write()
                .unwrap()
                .register(protocol, factory);
            Ok(())
        },
        set_fallback: |factory| {
            CALLS.fetch_add(1, Ordering::SeqCst);
            DEFAULT_FILESYSTEMS.write().unwrap().set_fallback(factory);
            Ok(())
        },
        url_to_fs: |url, options| {
            CALLS.fetch_add(1, Ordering::SeqCst);
            DEFAULT_FILESYSTEMS.read().unwrap().url_to_fs(url, options)
        },
    };
    assert!(set_registry_hooks(hooks).is_ok());
    assert!(set_registry_hooks(RegistryHooks {
        register: |_, _| Err(FsError::Other("second".into())),
        set_fallback: |_| Err(FsError::Other("second".into())),
        url_to_fs: |_, _| Err(FsError::Other("second".into())),
    })
    .is_err());

    let before = CALLS.load(Ordering::SeqCst);
    try_register_filesystem("hooked", |_: &str, _: &FsOptions| {
        Ok(Arc::new(LocalFs::new()) as SharedFileSystem)
    })
    .unwrap();
    let (fs, path) = url_to_fs("hooked://x/y", &FsOptions::new()).unwrap();
    assert_eq!(path, "x/y");
    assert!(fs.exists("/").unwrap());
    assert_eq!(CALLS.load(Ordering::SeqCst), before + 2);
}

#[test]
fn fallback_factory_takes_unknown_protocols_with_the_whole_url() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut registry = FilesystemRegistry::empty();
    let recorded = seen.clone();
    registry.set_fallback(Some(move |url: &str, _: &FsOptions| {
        recorded.lock().unwrap().push(url.to_string());
        Ok(Arc::new(LocalFs::new()) as SharedFileSystem)
    }));
    registry.register("known", |_: &str, _: &FsOptions| {
        Ok(Arc::new(LocalFs::new()) as SharedFileSystem)
    });

    let (_, path) = registry
        .url_to_fs("anything://bucket/key", &FsOptions::new())
        .unwrap();
    assert_eq!(path, "anything://bucket/key");
    registry.url_to_fs("known://x", &FsOptions::new()).unwrap();
    assert_eq!(*seen.lock().unwrap(), ["anything://bucket/key"]);

    registry.set_fallback::<fn(&str, &FsOptions) -> FsResult<SharedFileSystem>>(None);
    assert!(registry
        .url_to_fs("anything://bucket/key", &FsOptions::new())
        .is_err());
}
