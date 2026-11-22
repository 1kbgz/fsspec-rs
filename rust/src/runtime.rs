use std::{io, sync::OnceLock};

use tokio::runtime::Runtime;

use crate::{FsError, FsResult};

// Process-owned: dropping an adapter never drops a runtime inside async code.
pub(crate) fn runtime() -> FsResult<&'static Runtime> {
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
        .map_err(|e| FsError::Other(format!("managed runtime: {e}")))
}
