//! Executor-independent operation lifetimes and nonblocking callback dispatch.
//!
//! Cancellation makes a result available immediately. Producers must retain a
//! [`Completion`] until their I/O and cleanup stop; readiness is not quiescence.

use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use tokio::sync::{oneshot, Notify};

use crate::{runtime::runtime, ErrorKind, OperationError};

/// Diagnostic text has a separate fixed bound, outside data-buffer accounting.
pub const MAX_ERROR_BYTES: usize = 4096;

fn bounded_error(error: OperationError) -> OperationError {
    use std::fmt::Write;
    struct Message(String);
    impl std::fmt::Write for Message {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            let mut end = text.len().min(MAX_ERROR_BYTES - self.0.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            self.0.push_str(&text[..end]);
            if end == text.len() {
                Ok(())
            } else {
                Err(std::fmt::Error)
            }
        }
    }
    let mut message = Message(String::new());
    let _ = write!(&mut message, "{error}");
    OperationError::Detailed {
        kind: error.kind(),
        retry_safe: error.retry_safe(),
        message: message.0,
    }
}

pub type Outcome = Result<Value, OperationError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Unit,
    Bytes,
    Info,
    File,
    Position,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Value {
    Unit,
    Bytes(Vec<u8>),
    Info(crate::FileInfo),
    File(crate::async_files::AsyncFile),
    Position(u64),
}

/// An owned, nonblocking callback. Dropping it discards it and releases captures.
pub struct Job(Box<dyn FnOnce() + Send>);

impl Job {
    pub fn new(callback: impl FnOnce() + Send + 'static) -> Self {
        Self(Box::new(callback))
    }

    pub fn run(self) {
        (self.0)();
    }
}

/// Accepted jobs must run or be discarded once, never inline on the caller's
/// stack. Rejection returns ownership without running or retaining the job.
pub trait Dispatcher: Send + Sync {
    fn post(&self, job: Job) -> Result<(), (OperationError, Job)>;
}

pub struct ManagedDispatcher;

impl Dispatcher for ManagedDispatcher {
    fn post(&self, job: Job) -> Result<(), (OperationError, Job)> {
        match runtime() {
            Ok(runtime) => {
                runtime.spawn(async move { job.run() });
                Ok(())
            }
            Err(error) => Err((error.into(), job)),
        }
    }
}

struct Callback(Arc<dyn Dispatcher>, Job);

impl Callback {
    fn dispatch(self) {
        let retained = self.0.clone();
        let job = Job::new(move || {
            self.1.run();
            drop(retained);
        });
        // Failed notification cannot change an already-published result.
        let _ = self.0.post(job);
    }
}

#[derive(Default)]
struct State {
    outcome: Option<Outcome>,
    taken: bool,
    subscribed: bool,
    ready: Option<Callback>,
    cancel_registered: bool,
    cancel: Option<Callback>,
    cancelled: bool,
    quiescent: bool,
    awaiting: bool,
    successful: bool,
    retention: Option<Arc<dyn Send + Sync>>,
    waker: Option<Waker>,
    timer: Option<oneshot::Sender<()>>,
}

type ResultTransform = Arc<dyn Fn(Outcome) -> Outcome + Send + Sync>;

struct Shared {
    state: Mutex<State>,
    kind: Kind,
    max_bytes: usize,
    deadline: Option<Instant>,
    changed: Notify,
    jobs: AtomicUsize,
    settled: AtomicBool,
    transform: Mutex<Option<ResultTransform>>,
}

pub(crate) fn error(kind: ErrorKind, message: &str) -> OperationError {
    OperationError::Detailed {
        kind,
        message: message.into(),
        retry_safe: false,
    }
}

impl Shared {
    fn expired(&self) -> bool {
        self.deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    fn publish(&self, mut outcome: Outcome, mut cancelled: bool) {
        let mut discarded = None;
        let (ready, cancel, waker, timer, retention) = {
            let mut state = self.state.lock().unwrap();
            if state.outcome.is_some() || state.taken {
                return;
            }
            if self.expired() {
                discarded = Some(std::mem::replace(
                    &mut outcome,
                    Err(error(ErrorKind::Timeout, "operation deadline elapsed")),
                ));
                cancelled = true;
            }
            let retention = if outcome.is_err() {
                state.retention.take()
            } else {
                None
            };
            state.successful = outcome.is_ok();
            state.outcome = Some(outcome);
            state.cancelled = cancelled;
            (
                state.ready.take(),
                state.cancel.take(),
                state.waker.take(),
                state.timer.take(),
                retention,
            )
        };
        // Foreign callbacks, captured data and Wakers may reenter this operation.
        self.changed.notify_waiters();
        drop(retention);
        drop(discarded);
        drop(timer);
        if let Some(cancel) = cancel {
            if cancelled {
                cancel.dispatch();
            }
        }
        if let Some(ready) = ready {
            ready.dispatch();
        }
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// Cloning/releasing an observer does not cancel work. Awaiting it creates a
/// future which cancels on drop. Only one clone may be awaited. Results can be
/// taken only once across clones.
#[derive(Clone)]
pub struct Operation(Arc<Shared>);

impl Operation {
    /// Create an observer and producer. `max_bytes` bounds copied result bytes;
    /// it does not account for allocations made by a foreign provider.
    pub fn new(
        kind: Kind,
        max_bytes: usize,
        timeout: Option<Duration>,
    ) -> Result<(Self, Completion), OperationError> {
        let deadline = timeout
            .map(|timeout| {
                Instant::now()
                    .checked_add(timeout)
                    .ok_or_else(|| error(ErrorKind::Invalid, "timeout is too large"))
            })
            .transpose()?;
        let runtime = if timeout.is_some() {
            Some(runtime()?)
        } else {
            None
        };
        let (send, receive) = oneshot::channel();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                timer: runtime.map(|_| send),
                ..State::default()
            }),
            kind,
            max_bytes,
            deadline,
            changed: Notify::new(),
            jobs: AtomicUsize::new(0),
            settled: AtomicBool::new(true),
            transform: Mutex::new(None),
        });
        let operation = Self(shared.clone());
        let completion = Completion(Arc::new(Producer {
            shared: shared.clone(),
        }));
        if shared.expired() {
            shared.publish(
                Err(error(ErrorKind::Timeout, "operation deadline elapsed")),
                true,
            );
        } else if let (Some(runtime), Some(deadline)) = (runtime, deadline) {
            let weak = Arc::downgrade(&shared);
            runtime.spawn(async move {
                tokio::select! {
                    _ = receive => {}
                    _ = tokio::time::sleep_until(deadline.into()) => {
                        if let Some(shared) = weak.upgrade() {
                            shared.publish(Err(error(ErrorKind::Timeout, "operation deadline elapsed")), true);
                        }
                    }
                }
            });
        }
        Ok((operation, completion))
    }

    /// 0 = pending, 1 = ready, 2 = taken.
    pub fn state(&self) -> u32 {
        let state = self.0.state.lock().unwrap();
        if state.taken {
            2
        } else {
            u32::from(state.outcome.is_some())
        }
    }

    pub(crate) fn succeeded(&self) -> bool {
        self.0.state.lock().unwrap().successful
    }

    pub(crate) fn retain_result(&self, resource: Arc<dyn Send + Sync>) {
        let previous = {
            let mut state = self.0.state.lock().unwrap();
            if state.taken || state.outcome.as_ref().is_some_and(Result::is_err) {
                return;
            }
            state.retention.replace(resource)
        };
        drop(previous);
    }

    pub(crate) fn transform(&self, transform: impl Fn(Outcome) -> Outcome + Send + Sync + 'static) {
        let previous = self
            .0
            .transform
            .lock()
            .unwrap()
            .replace(Arc::new(transform));
        drop(previous);
    }

    pub(crate) fn tracked_job(&self, job: Job) -> Job {
        struct Lease(Arc<Shared>);
        impl Drop for Lease {
            fn drop(&mut self) {
                self.0.jobs.fetch_sub(1, Ordering::AcqRel);
                self.0.changed.notify_waiters();
            }
        }
        self.0.jobs.fetch_add(1, Ordering::AcqRel);
        let lease = Lease(self.0.clone());
        Job::new(move || {
            job.run();
            drop(lease);
        })
    }

    pub(crate) async fn wait_ready(&self) {
        loop {
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.state() != 0 {
                return;
            }
            changed.await;
        }
    }

    pub(crate) fn track_cleanup(&self) {
        self.0.settled.store(false, Ordering::Release);
    }
    pub(crate) fn finish_cleanup(&self) {
        self.0.settled.store(true, Ordering::Release);
        self.0.changed.notify_waiters();
    }

    /// Waits for producers, accepted callback jobs, and handle accounting cleanup.
    /// This does not consume the result or cancel the underlying operation.
    pub fn drain_operation(&self) -> Result<Operation, OperationError> {
        let (waiter, completion) = Operation::new(Kind::Unit, 0, None)?;
        let source = self.clone();
        runtime()?.spawn(async move {
            source.drain().await;
            completion.complete(Ok(Value::Unit));
        });
        Ok(waiter)
    }

    pub async fn drain(&self) {
        loop {
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.is_quiescent()
                && self.0.jobs.load(Ordering::Acquire) == 0
                && self.0.settled.load(Ordering::Acquire)
            {
                return;
            }
            changed.await;
        }
    }

    pub(crate) async fn wait_callbacks(&self) {
        loop {
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.is_quiescent() && self.0.jobs.load(Ordering::Acquire) == 0 {
                return;
            }
            changed.await;
        }
    }

    pub(crate) async fn wait_quiescent(&self) {
        loop {
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.is_quiescent() {
                return;
            }
            changed.await;
        }
    }

    pub fn is_quiescent(&self) -> bool {
        self.0.state.lock().unwrap().quiescent
    }

    pub fn cancel(&self) {
        self.0.publish(
            Err(error(ErrorKind::Cancelled, "operation cancelled")),
            true,
        );
    }

    pub fn take(&self) -> Result<Outcome, OperationError> {
        let mut state = self.0.state.lock().unwrap();
        let outcome = state
            .outcome
            .take()
            .ok_or_else(|| error(ErrorKind::Invalid, "operation is pending or already taken"))?;
        state.taken = true;
        let retention = state.retention.take();
        drop(state);
        drop(retention);
        Ok(outcome)
    }

    pub fn subscribe(
        &self,
        dispatcher: Arc<dyn Dispatcher>,
        job: Job,
    ) -> Result<(), OperationError> {
        let callback = Callback(dispatcher, self.tracked_job(job));
        {
            let mut state = self.0.state.lock().unwrap();
            if state.subscribed {
                return Err(error(
                    ErrorKind::Invalid,
                    "operation already has a subscriber",
                ));
            }
            state.subscribed = true;
            if state.outcome.is_none() && !state.taken {
                state.ready = Some(callback);
                return Ok(());
            }
        }
        callback.dispatch();
        Ok(())
    }
}

struct Producer {
    shared: Arc<Shared>,
}

impl Drop for Producer {
    fn drop(&mut self) {
        self.shared.state.lock().unwrap().quiescent = true;
        self.shared.changed.notify_waiters();
        self.shared.publish(
            Err(error(
                ErrorKind::Unavailable,
                "producer abandoned operation",
            )),
            false,
        );
    }
}

/// Every producer reference is a cleanup lease. Keep the last reference alive
/// until backend work has stopped, even after cancellation or early completion.
#[derive(Clone)]
pub struct Completion(Arc<Producer>);

impl Completion {
    pub fn is_cancelled(&self) -> bool {
        self.0.shared.state.lock().unwrap().cancelled
    }

    pub fn remaining(&self) -> Option<Duration> {
        self.0
            .shared
            .deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }

    pub fn max_bytes(&self) -> usize {
        self.0.shared.max_bytes
    }

    pub fn on_cancel(
        &self,
        dispatcher: Arc<dyn Dispatcher>,
        job: Job,
    ) -> Result<(), OperationError> {
        let callback = Callback(
            dispatcher,
            Operation(self.0.shared.clone()).tracked_job(job),
        );
        let cancelled = {
            let mut state = self.0.shared.state.lock().unwrap();
            if state.cancel_registered {
                return Err(error(
                    ErrorKind::Invalid,
                    "operation already has a cancellation hook",
                ));
            }
            state.cancel_registered = true;
            if state.outcome.is_none() && !state.taken {
                state.cancel = Some(callback);
                return Ok(());
            }
            state.cancelled
        };
        if cancelled {
            callback.dispatch();
        }
        Ok(())
    }

    pub fn complete(self, outcome: Outcome) {
        let transform = self.0.shared.transform.lock().unwrap().clone();
        let outcome = match transform {
            Some(transform) => transform(outcome),
            None => outcome,
        };
        let valid = match &outcome {
            Ok(Value::Unit) => self.0.shared.kind == Kind::Unit,
            Ok(Value::Bytes(bytes)) => {
                self.0.shared.kind == Kind::Bytes && bytes.capacity() <= self.max_bytes()
            }
            Ok(Value::Info(info)) => {
                self.0.shared.kind == Kind::Info
                    && crate::async_files::info_bytes(info) <= self.max_bytes()
            }
            Ok(Value::File(_)) => self.0.shared.kind == Kind::File,
            Ok(Value::Position(_)) => self.0.shared.kind == Kind::Position,
            Err(_) => true,
        };
        let outcome = outcome.map_err(bounded_error);
        self.0.shared.publish(
            if valid {
                outcome
            } else {
                Err(error(
                    ErrorKind::Invalid,
                    "result kind or byte limit does not match operation",
                ))
            },
            false,
        );
    }
}

pub struct OperationFuture {
    operation: Operation,
    claimed: bool,
}

impl IntoFuture for Operation {
    type Output = Outcome;
    type IntoFuture = OperationFuture;

    fn into_future(self) -> Self::IntoFuture {
        let claimed = {
            let mut state = self.0.state.lock().unwrap();
            let claimed = !state.awaiting;
            state.awaiting = true;
            claimed
        };
        OperationFuture {
            operation: self,
            claimed,
        }
    }
}

impl Future for OperationFuture {
    type Output = Outcome;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if !self.claimed {
            return Poll::Ready(Err(error(
                ErrorKind::Invalid,
                "operation already has an awaiting consumer",
            )));
        }
        // Clone/drop/wake can invoke foreign code: never do these under the lock.
        let new_waker = cx.waker().clone();
        let (result, old_waker, retention) = {
            let mut state = self.operation.0.state.lock().unwrap();
            if let Some(outcome) = state.outcome.take() {
                state.taken = true;
                (
                    Poll::Ready(outcome),
                    state.waker.take(),
                    state.retention.take(),
                )
            } else if state.taken {
                (
                    Poll::Ready(Err(error(
                        ErrorKind::Invalid,
                        "operation result already taken",
                    ))),
                    state.waker.take(),
                    state.retention.take(),
                )
            } else {
                (Poll::Pending, state.waker.replace(new_waker), None)
            }
        };
        drop(old_waker);
        drop(retention);
        result
    }
}

impl Drop for OperationFuture {
    fn drop(&mut self) {
        if self.claimed {
            self.operation.cancel();
        }
    }
}

#[cfg(test)]
mod tests;
