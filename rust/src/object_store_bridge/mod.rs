//! Explicit, read-only adapters between [`crate::FileSystem`] and
//! [`object_store::ObjectStore`]. Enable the `object-store` feature independently
//! of S3. Backend authors implementing both traits need neither adapter.
//!
//! [`FileSystemStore::from_snapshot`] requires an immutable filesystem namespace.
//! [`ObjectStoreFs`] provides synchronous access outside Tokio runtime contexts.
//! See the object-store bridge section in `docs/src/rust.md` for the operation contract.

mod filesystem;
mod store;

pub use filesystem::ObjectStoreFs;
pub use store::FileSystemStore;

use std::future::Future;
use std::sync::{Arc, OnceLock};
use std::{fmt, io};

use object_store::{path::Path, Error, ObjectMeta};
use tokio::{runtime::Runtime, sync::Semaphore};

use crate::{FileInfo, FsError, FsResult};

const CHUNK_SIZE: usize = 64 * 1024;
static BLOCKING_LIMIT: OnceLock<Arc<Semaphore>> = OnceLock::new();

// Process-owned: dropping an adapter never drops a runtime inside async code.
fn runtime() -> FsResult<&'static Runtime> {
    static RUNTIME: OnceLock<io::Result<Runtime>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .max_blocking_threads(8)
                .enable_all()
                .build()
        })
        .as_ref()
        .map_err(|e| FsError::Other(format!("bridge runtime: {e}")))
}

fn outside_runtime() -> FsResult<()> {
    if tokio::runtime::Handle::try_current().is_ok() {
        return Err(FsError::NotSupported(
            "synchronous object-store bridge cannot run inside a Tokio runtime; use ObjectStore directly".into(),
        ));
    }
    Ok(())
}

fn wait<T>(future: impl Future<Output = object_store::Result<T>>) -> FsResult<T> {
    outside_runtime()?;
    runtime()?.block_on(future).map_err(to_fs_error)
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> object_store::Result<T> + Send + 'static,
) -> object_store::Result<T> {
    let permit = BLOCKING_LIMIT
        .get_or_init(|| Arc::new(Semaphore::new(8)))
        .clone()
        .acquire_owned()
        .await
        .map_err(generic)?;
    runtime()
        .map_err(|e| from_fs_error("", e))?
        .spawn_blocking(move || {
            // A cancelled waiter cannot release the slot while work still runs.
            let _permit = permit;
            f()
        })
        .await
        .map_err(generic)?
}

fn generic(e: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Error {
    Error::Generic {
        store: "fsspec bridge",
        source: e.into(),
    }
}

fn unsupported(message: &str) -> Error {
    Error::NotSupported {
        source: message.to_owned().into(),
    }
}

fn from_fs_error(path: &str, e: FsError) -> Error {
    let path = path.to_owned();
    match e {
        FsError::NotFound(_) => Error::NotFound {
            path,
            source: Box::new(e),
        },
        FsError::PermissionDenied(_) => Error::PermissionDenied {
            path,
            source: Box::new(e),
        },
        FsError::AlreadyExists(_) => Error::AlreadyExists {
            path,
            source: Box::new(e),
        },
        FsError::NotSupported(_) => Error::NotSupported {
            source: Box::new(e),
        },
        _ => generic(e),
    }
}

fn to_fs_error(e: Error) -> FsError {
    match e {
        Error::NotFound { path, .. } => FsError::NotFound(path),
        Error::PermissionDenied { path, .. } | Error::Unauthenticated { path, .. } => {
            FsError::PermissionDenied(path)
        }
        Error::NotSupported { .. } | Error::NotImplemented { .. } => {
            FsError::NotSupported(e.to_string())
        }
        Error::AlreadyExists { path, .. } => FsError::AlreadyExists(path),
        Error::InvalidPath { .. } => FsError::InvalidArgument(e.to_string()),
        // Preserve precondition failures and other backend causes in the I/O error.
        _ => FsError::IoError(io::Error::other(e)),
    }
}

fn to_io_error(e: FsError) -> io::Error {
    let kind = match &e {
        FsError::NotFound(_) => io::ErrorKind::NotFound,
        FsError::PermissionDenied(_) => io::ErrorKind::PermissionDenied,
        FsError::InvalidArgument(_) => io::ErrorKind::InvalidInput,
        FsError::NotSupported(_) => io::ErrorKind::Unsupported,
        _ => io::ErrorKind::Other,
    };
    match e {
        FsError::IoError(error) => error,
        error => io::Error::new(kind, error),
    }
}

fn file_info(meta: &ObjectMeta, name: &str) -> FileInfo {
    let mut info = FileInfo::file(name, meta.size);
    info.modified = Some(meta.last_modified.into());
    if let Some(etag) = &meta.e_tag {
        info.extra.insert("etag".into(), etag.clone());
    }
    if let Some(version) = &meta.version {
        info.extra.insert("version".into(), version.clone());
    }
    info
}

// Parse without encoding or decoding. Path::from would normalize/encode names.
fn key(value: &str) -> FsResult<Path> {
    let parsed = Path::parse(value).map_err(|e| FsError::InvalidArgument(e.to_string()))?;
    if parsed.as_ref() != value || value.contains("://") || value.contains('\\') {
        return Err(FsError::InvalidArgument(format!(
            "expected a relative object key: {value}"
        )));
    }
    Ok(parsed)
}

fn read_only() -> FsError {
    FsError::NotSupported("object-store bridge is read-only".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Condvar, Mutex};

    #[test]
    fn cancelling_waiters_does_not_release_running_work_slots() {
        // Release blocked work even if an assertion fails.
        #[derive(Default)]
        struct Gate(Mutex<bool>, Condvar);
        struct Release(Arc<Gate>);
        impl Drop for Release {
            fn drop(&mut self) {
                *self.0 .0.lock().unwrap() = true;
                self.0 .1.notify_all();
            }
        }
        let gate = Arc::new(Gate::default());
        let release = Release(gate.clone());
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            let (started, mut receiver) = tokio::sync::mpsc::unbounded_channel();
            let mut jobs = Vec::new();
            for _ in 0..8 {
                let gate = gate.clone();
                let started = started.clone();
                jobs.push(tokio::spawn(blocking(move || {
                    started.send(()).unwrap();
                    let mut open = gate.0.lock().unwrap();
                    while !*open {
                        open = gate.1.wait(open).unwrap();
                    }
                    Ok(())
                })));
            }
            for _ in 0..8 {
                receiver.recv().await.unwrap();
            }
            for job in jobs {
                job.abort();
                assert!(job.await.unwrap_err().is_cancelled());
            }
            assert_eq!(BLOCKING_LIMIT.get().unwrap().available_permits(), 0);
            drop(release);
            blocking(|| Ok(())).await.unwrap();
        });
    }
}
