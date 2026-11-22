//! Read-only async handles, shared limits, and drainable provider cleanup.
use std::collections::BTreeMap;
use std::io::SeekFrom;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex, OnceLock, Weak,
};
use std::time::Duration;

use tokio::sync::Notify;

use crate::async_io::{error, Completion, Dispatcher, Job, Kind, Operation, Value};
use crate::{runtime::runtime, AsyncFsFile, ErrorKind, FileInfo, IoFuture, OperationError};

type Result<T> = std::result::Result<T, OperationError>;

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_operations: usize,
    pub max_buffered_bytes: usize,
    pub max_read_size: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_operations: 64,
            max_buffered_bytes: 64 * 1024 * 1024,
            max_read_size: 8 * 1024 * 1024,
        }
    }
}

/// Nonblocking provider starts. Keep completion references through I/O cleanup.
/// Calls are dispatched on the registered executor; metadata can run concurrently.
pub trait FileProvider: Send + Sync + 'static {
    fn read(&self, max_bytes: usize, completion: Completion);
    /// Total data reservation, including output and provider staging buffers.
    /// Must be at least max_bytes. Reserved atomically before provider dispatch.
    fn read_buffer_bytes(&self, max_bytes: usize) -> Result<usize> {
        Ok(max_bytes)
    }
    fn seek(&self, _position: SeekFrom, completion: Completion) {
        completion.complete(Err(error(ErrorKind::NotSupported, "file is not seekable")));
    }
    fn info(&self, completion: Completion);
    fn close(&self, completion: Completion);
}

pub trait FilesystemProvider: Send + Sync + 'static {
    fn info(&self, path: &str, completion: Completion);
    fn open_read(&self, path: &str, completion: Completion);
    fn close(&self, completion: Completion) {
        completion.complete(Ok(Value::Unit));
    }
}

fn unavailable(message: &str) -> OperationError {
    OperationError::Detailed {
        kind: ErrorKind::Unavailable,
        message: message.into(),
        retry_safe: true,
    }
}

pub fn info_bytes(info: &FileInfo) -> usize {
    info.extra
        .iter()
        .fold(info.name.capacity(), |size, (key, value)| {
            size.saturating_add(key.capacity())
                .saturating_add(value.capacity())
        })
}

#[derive(Clone)]
struct Failure {
    kind: ErrorKind,
    message: String,
    retry_safe: bool,
}
impl Failure {
    fn from(error: OperationError) -> Self {
        Self {
            kind: error.kind(),
            message: error.to_string(),
            retry_safe: error.retry_safe(),
        }
    }
    fn error(&self) -> OperationError {
        OperationError::Detailed {
            kind: self.kind,
            message: self.message.clone(),
            retry_safe: self.retry_safe,
        }
    }
}
type CloseResult = std::result::Result<(), Failure>;
#[derive(Default)]
struct CloseAttempt {
    result: Mutex<Option<CloseResult>>,
    changed: Notify,
}
impl CloseAttempt {
    fn finish(&self, result: Result<()>) {
        *self.result.lock().unwrap() = Some(result.map_err(Failure::from));
        self.changed.notify_waiters();
    }
    async fn wait(&self) -> Result<()> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(result) = self.result.lock().unwrap().clone() {
                return result.map_err(|e| e.error());
            }
            changed.await;
        }
    }
    fn observer(self: &Arc<Self>, timeout: Option<Duration>) -> Result<Operation> {
        let (operation, completion) = Operation::new(Kind::Unit, 0, timeout)?;
        let attempt = self.clone();
        runtime()?.spawn(async move {
            completion.complete(attempt.wait().await.map(|()| Value::Unit));
        });
        Ok(operation)
    }
}
fn needs_attempt(attempt: &Option<Arc<CloseAttempt>>) -> bool {
    attempt
        .as_ref()
        .is_none_or(|a| matches!(*a.result.lock().unwrap(), Some(Err(_))))
}

#[derive(Default)]
struct ContextState {
    closing: bool,
    active: usize,
    bytes: usize,
    resources: Vec<Weak<Resource>>,
    close: Option<Arc<CloseAttempt>>,
}
struct ContextInner {
    limits: Limits,
    state: Mutex<ContextState>,
    changed: Notify,
}
struct ContextOwner(Arc<ContextInner>);
impl Drop for ContextOwner {
    fn drop(&mut self) {
        self.0.start_close();
    }
}
#[derive(Clone)]
pub struct AsyncContext(Arc<ContextOwner>);
impl AsyncContext {
    pub fn new(limits: Limits) -> Result<Self> {
        if limits.max_operations == 0
            || limits.max_buffered_bytes == 0
            || limits.max_read_size == 0
            || limits.max_read_size > limits.max_buffered_bytes
        {
            return Err(error(ErrorKind::Invalid, "invalid async context limits"));
        }
        runtime()?;
        Ok(Self(Arc::new(ContextOwner(Arc::new(ContextInner {
            limits,
            state: Mutex::new(ContextState::default()),
            changed: Notify::new(),
        })))))
    }
    pub fn close(&self, timeout: Option<Duration>) -> Result<Operation> {
        self.0 .0.start_close().observer(timeout)
    }
    pub fn limits(&self) -> Limits {
        self.0 .0.limits
    }
}

struct Admission(Arc<ContextInner>);
impl Drop for Admission {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().active -= 1;
        self.0.changed.notify_waiters();
    }
}
struct Reservation {
    context: Arc<ContextInner>,
    bytes: usize,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.context.state.lock().unwrap().bytes -= self.bytes;
        self.context.changed.notify_waiters();
    }
}
impl ContextInner {
    fn admit(self: &Arc<Self>) -> Result<Admission> {
        let mut state = self.state.lock().unwrap();
        if state.closing {
            return Err(unavailable("async context is closing"));
        }
        if state.active >= self.limits.max_operations {
            return Err(unavailable("async operation limit reached"));
        }
        state.active += 1;
        Ok(Admission(self.clone()))
    }
    fn reserve(self: &Arc<Self>, bytes: usize) -> Result<Arc<Reservation>> {
        if bytes > self.limits.max_buffered_bytes {
            return Err(error(
                ErrorKind::Invalid,
                "request exceeds context byte limit",
            ));
        }
        let mut state = self.state.lock().unwrap();
        if bytes > self.limits.max_buffered_bytes - state.bytes {
            return Err(unavailable("async byte capacity unavailable"));
        }
        state.bytes += bytes;
        Ok(Arc::new(Reservation {
            context: self.clone(),
            bytes,
        }))
    }
    async fn reserve_wait(self: &Arc<Self>, bytes: usize) -> Arc<Reservation> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Ok(reservation) = self.reserve(bytes) {
                return reservation;
            }
            changed.await;
        }
    }
    fn start_close(self: &Arc<Self>) -> Arc<CloseAttempt> {
        let (attempt, resources) = {
            let mut state = self.state.lock().unwrap();
            if !needs_attempt(&state.close) {
                return state.close.as_ref().unwrap().clone();
            }
            state.closing = true;
            let attempt = Arc::new(CloseAttempt::default());
            state.close = Some(attempt.clone());
            (
                attempt,
                state
                    .resources
                    .iter()
                    .filter_map(Weak::upgrade)
                    .collect::<Vec<_>>(),
            )
        };
        let attempts: Vec<_> = resources.iter().map(|node| node.start_close()).collect();
        let context = self.clone();
        let finished = attempt.clone();
        runtime()
            .expect("context runtime initialized")
            .spawn(async move {
                let mut failure = None;
                for attempt in attempts {
                    if let Err(error) = attempt.wait().await {
                        failure.get_or_insert(error);
                    }
                }
                loop {
                    let changed = context.changed.notified();
                    tokio::pin!(changed);
                    changed.as_mut().enable();
                    if context.state.lock().unwrap().active == 0 {
                        break;
                    }
                    changed.await;
                }
                finished.finish(failure.map_or(Ok(()), Err));
            });
        attempt
    }
}

enum Provider {
    File(Arc<dyn FileProvider>),
    Filesystem(Arc<dyn FilesystemProvider>),
}
#[derive(Default)]
struct ResourceState {
    closing: bool,
    cursor: Option<Cursor>,
    dirty: bool,
    operations: BTreeMap<usize, Operation>,
    children: Vec<Weak<Resource>>,
    parent: Option<Arc<Handle>>,
    close: Option<Arc<CloseAttempt>>,
}
struct Cursor {
    operation: Operation,
    started: Arc<AtomicBool>,
    absolute: bool,
}
impl ResourceState {
    fn refresh_cursor(&mut self) {
        if self
            .cursor
            .as_ref()
            .is_some_and(|cursor| cursor.operation.is_quiescent())
        {
            let cursor = self.cursor.take().unwrap();
            if cursor.started.load(Ordering::Acquire) {
                if !cursor.operation.succeeded() {
                    self.dirty = true;
                } else if cursor.absolute {
                    self.dirty = false;
                }
            }
        }
    }
}
struct Resource {
    _context_owner: Arc<ContextOwner>,
    owner: OnceLock<Weak<Handle>>,
    context: Arc<ContextInner>,
    dispatcher: Arc<dyn Dispatcher>,
    provider: Provider,
    state: Mutex<ResourceState>,
}
struct Handle(Arc<Resource>);
impl Drop for Handle {
    fn drop(&mut self) {
        self.0.start_close();
    }
}

#[derive(Clone)]
pub struct AsyncFile(Arc<Handle>);
impl std::fmt::Debug for AsyncFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncFile").finish_non_exhaustive()
    }
}
impl PartialEq for AsyncFile {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for AsyncFile {}
#[derive(Clone)]
pub struct AsyncFilesystem(Arc<Handle>);

impl Resource {
    fn create_handle(
        context: &AsyncContext,
        dispatcher: Arc<dyn Dispatcher>,
        provider: Provider,
    ) -> Result<Arc<Handle>> {
        let context_owner = context.0.clone();
        let context = &context.0 .0;
        let mut state = context.state.lock().unwrap();
        if state.closing {
            return Err(unavailable("async context is closing"));
        }
        let resource = Arc::new(Self {
            _context_owner: context_owner,
            owner: OnceLock::new(),
            context: context.clone(),
            dispatcher,
            provider,
            state: Mutex::new(ResourceState::default()),
        });
        state.resources.retain(|node| node.strong_count() != 0);
        state.resources.push(Arc::downgrade(&resource));
        let handle = Arc::new(Handle(resource.clone()));
        let _ = resource.owner.set(Arc::downgrade(&handle));
        Ok(handle)
    }
    fn dispatch(
        self: &Arc<Self>,
        operation: &Operation,
        completion: Completion,
        started: Arc<AtomicBool>,
        call: impl FnOnce(&Provider, Completion) + Send + 'static,
    ) {
        let resource = self.clone();
        let observer = operation.clone();
        let dispatcher = self.dispatcher.clone();
        let retained = dispatcher.clone();
        let job = operation.tracked_job(Job::new(move || {
            let _retained = retained;
            if completion
                .remaining()
                .is_some_and(|remaining| remaining.is_zero())
            {
                observer.cancel();
            }
            if observer.state() != 0 {
                return;
            }
            started.store(true, Ordering::Release);
            call(&resource.provider, completion);
        }));
        let _ = dispatcher.post(job);
    }
    fn start(
        self: &Arc<Self>,
        kind: Kind,
        bytes: usize,
        timeout: Option<Duration>,
        cursor: Option<bool>,
        reserved: Option<Arc<Reservation>>,
        call: impl FnOnce(&Provider, Completion) + Send + 'static,
    ) -> Result<Operation> {
        let buffer_bytes = match (&self.provider, kind) {
            (Provider::File(provider), Kind::Bytes) => provider.read_buffer_bytes(bytes)?,
            _ => bytes,
        };
        if buffer_bytes < bytes || buffer_bytes > self.context.limits.max_buffered_bytes {
            return Err(error(
                ErrorKind::Invalid,
                "read buffers exceed context byte limit",
            ));
        }
        let admission = self.context.admit()?;
        let (operation, completion) = Operation::new(kind, bytes, timeout)?;
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let started = Arc::new(AtomicBool::new(false));
        {
            let mut state = self.state.lock().unwrap();
            if state.closing {
                return Err(unavailable("async handle is closing"));
            }
            state.refresh_cursor();
            if let Some(absolute) = cursor {
                if state.cursor.is_some() {
                    return Err(unavailable("cursor operation still running"));
                }
                if state.dirty && !absolute {
                    return Err(error(
                        ErrorKind::Invalid,
                        "cursor requires an absolute seek after failed or cancelled I/O",
                    ));
                }
                state.cursor = Some(Cursor {
                    operation: operation.clone(),
                    started: started.clone(),
                    absolute,
                });
            }
            operation.track_cleanup();
            state.operations.insert(id, operation.clone());
        }
        if kind == Kind::File {
            let owner = self
                .owner
                .get()
                .and_then(Weak::upgrade)
                .expect("start borrows a live handle");
            operation.transform(move |outcome| {
                let parent = &owner.0;
                if let Ok(Value::File(file)) = &outcome {
                    let child = &file.0 .0;
                    if !Arc::ptr_eq(&parent.context, &child.context) {
                        return Err(error(
                            ErrorKind::Invalid,
                            "opened file belongs to another context",
                        ));
                    }
                    let mut child_state = child.state.lock().unwrap();
                    if child_state.parent.is_some() || child_state.closing {
                        return Err(error(
                            ErrorKind::Invalid,
                            "opened file is attached or closing",
                        ));
                    }
                    child_state.parent = Some(owner.clone());
                    drop(child_state);
                    parent
                        .state
                        .lock()
                        .unwrap()
                        .children
                        .push(Arc::downgrade(child));
                }
                outcome
            });
        }
        let resource = self.clone();
        let observer = operation.clone();
        runtime()?.spawn(async move {
            let reservation = match reserved {
                Some(reservation) => Some(reservation),
                None => tokio::select! {
                    reservation = resource.context.reserve_wait(buffer_bytes) => Some(reservation),
                    _ = observer.wait_ready() => None,
                },
            };
            if let Some(reservation) = &reservation {
                observer.retain_result(reservation.clone());
                resource.dispatch(&observer, completion, started.clone(), call);
            } else {
                drop(completion);
            }
            observer.wait_quiescent().await;
            resource.state.lock().unwrap().refresh_cursor();
            drop(admission);
            drop(reservation);
            observer.wait_callbacks().await;
            let removed = resource.state.lock().unwrap().operations.remove(&id);
            drop(removed);
            observer.finish_cleanup();
        });
        Ok(operation)
    }
    fn start_close(self: &Arc<Self>) -> Arc<CloseAttempt> {
        let (attempt, operations) = {
            let mut state = self.state.lock().unwrap();
            if !needs_attempt(&state.close) {
                return state.close.as_ref().unwrap().clone();
            }
            state.closing = true;
            let attempt = Arc::new(CloseAttempt::default());
            state.close = Some(attempt.clone());
            (
                attempt,
                state.operations.values().cloned().collect::<Vec<_>>(),
            )
        };
        for operation in &operations {
            operation.cancel();
        }
        let resource = self.clone();
        let finished = attempt.clone();
        runtime()
            .expect("context runtime initialized")
            .spawn(async move {
                for operation in operations {
                    operation.drain().await;
                }
                let children = resource
                    .state
                    .lock()
                    .unwrap()
                    .children
                    .iter()
                    .filter_map(Weak::upgrade)
                    .collect::<Vec<_>>();
                let mut failure = None;
                for child in children {
                    if let Err(error) = child.start_close().wait().await {
                        failure.get_or_insert(error);
                    }
                }
                if let Some(error) = failure {
                    finished.finish(Err(error));
                    return;
                }
                let (operation, completion) =
                    Operation::new(Kind::Unit, 0, None).expect("no deadline");
                resource.dispatch(
                    &operation,
                    completion,
                    Arc::new(AtomicBool::new(false)),
                    |provider, completion| match provider {
                        Provider::File(provider) => provider.close(completion),
                        Provider::Filesystem(provider) => provider.close(completion),
                    },
                );
                let result = operation.clone().await.map(|_| ());
                operation.drain().await;
                finished.finish(result);
            });
        attempt
    }
}

impl AsyncFile {
    pub fn new(
        context: &AsyncContext,
        dispatcher: Arc<dyn Dispatcher>,
        provider: Arc<dyn FileProvider>,
    ) -> Result<Self> {
        Resource::create_handle(context, dispatcher, Provider::File(provider)).map(Self)
    }
    pub fn read(&self, max_bytes: usize, timeout: Option<Duration>) -> Result<Operation> {
        if max_bytes > self.0 .0.context.limits.max_read_size {
            return Err(error(ErrorKind::Invalid, "read exceeds context read limit"));
        }
        self.0 .0.start(
            Kind::Bytes,
            max_bytes,
            timeout,
            Some(false),
            None,
            move |provider, completion| {
                if let Provider::File(provider) = provider {
                    provider.read(max_bytes, completion);
                }
            },
        )
    }
    pub fn seek(&self, position: SeekFrom, timeout: Option<Duration>) -> Result<Operation> {
        self.0 .0.start(
            Kind::Position,
            0,
            timeout,
            Some(matches!(position, SeekFrom::Start(_))),
            None,
            move |provider, completion| {
                if let Provider::File(provider) = provider {
                    provider.seek(position, completion);
                }
            },
        )
    }
    pub fn info(&self, timeout: Option<Duration>) -> Result<Operation> {
        self.0 .0.start(
            Kind::Info,
            self.0 .0.context.limits.max_read_size,
            timeout,
            None,
            None,
            |provider, completion| {
                if let Provider::File(provider) = provider {
                    provider.info(completion);
                }
            },
        )
    }
    pub fn close(&self, timeout: Option<Duration>) -> Result<Operation> {
        self.0 .0.start_close().observer(timeout)
    }
}
impl AsyncFilesystem {
    pub fn new(
        context: &AsyncContext,
        dispatcher: Arc<dyn Dispatcher>,
        provider: Arc<dyn FilesystemProvider>,
    ) -> Result<Self> {
        Resource::create_handle(context, dispatcher, Provider::Filesystem(provider)).map(Self)
    }
    fn path_operation(
        &self,
        path: &str,
        timeout: Option<Duration>,
        open: bool,
    ) -> Result<Operation> {
        let output_bytes = if open {
            0
        } else {
            self.0 .0.context.limits.max_read_size
        };
        let bytes = path
            .len()
            .checked_add(output_bytes)
            .ok_or_else(|| error(ErrorKind::Invalid, "path too large"))?;
        let reserved = self.0 .0.context.reserve(bytes)?;
        let path = path.to_owned();
        self.0 .0.start(
            if open { Kind::File } else { Kind::Info },
            output_bytes,
            timeout,
            None,
            Some(reserved),
            move |provider, completion| {
                if let Provider::Filesystem(provider) = provider {
                    if open {
                        provider.open_read(&path, completion);
                    } else {
                        provider.info(&path, completion);
                    }
                }
            },
        )
    }
    pub fn info(&self, path: &str, timeout: Option<Duration>) -> Result<Operation> {
        self.path_operation(path, timeout, false)
    }
    pub fn open_read(&self, path: &str, timeout: Option<Duration>) -> Result<Operation> {
        self.path_operation(path, timeout, true)
    }
    pub fn close(&self, timeout: Option<Duration>) -> Result<Operation> {
        self.0 .0.start_close().observer(timeout)
    }
}

impl AsyncFsFile for AsyncFile {
    fn read(&mut self, max_bytes: usize) -> IoFuture<'_, Vec<u8>> {
        Box::pin(async move {
            let operation = AsyncFile::read(self, max_bytes, None)?;
            let value = operation.clone().await?;
            operation.drain().await;
            match value {
                Value::Bytes(bytes) => Ok(bytes),
                _ => unreachable!(),
            }
        })
    }
    fn seek(&mut self, position: SeekFrom) -> IoFuture<'_, u64> {
        Box::pin(async move {
            let operation = AsyncFile::seek(self, position, None)?;
            let value = operation.clone().await?;
            operation.drain().await;
            match value {
                Value::Position(position) => Ok(position),
                _ => unreachable!(),
            }
        })
    }
    fn info(&self) -> IoFuture<'_, FileInfo> {
        Box::pin(async move {
            let operation = AsyncFile::info(self, None)?;
            let value = operation.clone().await?;
            operation.drain().await;
            match value {
                Value::Info(info) => Ok(info),
                _ => unreachable!(),
            }
        })
    }
    fn close(&mut self) -> IoFuture<'_, ()> {
        Box::pin(async move { AsyncFile::close(self, None)?.await.map(|_| ()) })
    }
}

#[cfg(test)]
mod tests;
