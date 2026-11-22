//! Executor-independent operation lifetimes and nonblocking callback dispatch.
//!
//! Cancellation makes a result available immediately. Producers must retain a
//! [`Completion`] until their I/O and cleanup stop; readiness is not quiescence.

use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use crate::{runtime::runtime, ErrorKind, OperationError};

pub type Outcome = Result<Value, OperationError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Unit,
    Bytes,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Value {
    Unit,
    Bytes(Vec<u8>),
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
    waker: Option<Waker>,
    timer: Option<oneshot::Sender<()>>,
}

struct Shared {
    state: Mutex<State>,
    kind: Kind,
    max_bytes: usize,
    deadline: Option<Instant>,
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
        let (ready, cancel, waker, timer) = {
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
            state.outcome = Some(outcome);
            state.cancelled = cancelled;
            (
                state.ready.take(),
                state.cancel.take(),
                state.waker.take(),
                state.timer.take(),
            )
        };
        // Foreign callbacks, captured data and Wakers may reenter this operation.
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
        Ok(outcome)
    }

    pub fn subscribe(
        &self,
        dispatcher: Arc<dyn Dispatcher>,
        job: Job,
    ) -> Result<(), OperationError> {
        let callback = Callback(dispatcher, job);
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
        let callback = Callback(dispatcher, job);
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
        let valid = match &outcome {
            Ok(Value::Unit) => self.0.shared.kind == Kind::Unit,
            Ok(Value::Bytes(bytes)) => {
                self.0.shared.kind == Kind::Bytes && bytes.len() <= self.max_bytes()
            }
            Err(_) => true,
        };
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
        let (result, old_waker) = {
            let mut state = self.operation.0.state.lock().unwrap();
            if let Some(outcome) = state.outcome.take() {
                state.taken = true;
                (Poll::Ready(outcome), state.waker.take())
            } else if state.taken {
                (
                    Poll::Ready(Err(error(
                        ErrorKind::Invalid,
                        "operation result already taken",
                    ))),
                    state.waker.take(),
                )
            } else {
                (Poll::Pending, state.waker.replace(new_waker))
            }
        };
        drop(old_waker);
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
