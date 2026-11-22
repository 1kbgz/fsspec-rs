use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc,
};

#[derive(Default)]
struct Queue(Mutex<Vec<Job>>);

impl Dispatcher for Queue {
    fn post(&self, job: Job) -> Result<(), (OperationError, Job)> {
        self.0.lock().unwrap().push(job);
        Ok(())
    }
}

impl Queue {
    fn run(&self) {
        let jobs = std::mem::take(&mut *self.0.lock().unwrap());
        for job in jobs {
            job.run();
        }
    }
}

fn unit() -> (Operation, Completion) {
    Operation::new(Kind::Unit, 0, None).unwrap()
}

#[test]
fn completion_before_subscription_is_delivered_once_and_can_reenter() {
    let (operation, completion) = unit();
    completion.complete(Ok(Value::Unit));
    let queue = Arc::new(Queue::default());
    let observer = operation.clone();
    operation
        .subscribe(
            queue.clone(),
            Job::new(move || {
                assert_eq!(observer.take().unwrap().unwrap(), Value::Unit);
                observer.cancel();
            }),
        )
        .unwrap();
    assert!(operation.subscribe(queue.clone(), Job::new(|| {})).is_err());
    queue.run();
    assert_eq!(operation.state(), 2);
    assert!(operation.is_quiescent());
    assert!(operation.take().is_err());
}

#[test]
fn cancellation_before_hook_keeps_producer_alive_until_cleanup() {
    let (operation, completion) = unit();
    let lease = completion.clone();
    operation.cancel();
    let queue = Arc::new(Queue::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    completion
        .on_cancel(
            queue.clone(),
            Job::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
            }),
        )
        .unwrap();
    assert!(completion
        .on_cancel(queue.clone(), Job::new(|| {}))
        .is_err());
    queue.run();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    completion.complete(Ok(Value::Unit));
    assert!(!operation.is_quiescent());
    assert_eq!(
        operation.take().unwrap().unwrap_err().kind(),
        ErrorKind::Cancelled
    );
    drop(lease);
    assert!(operation.is_quiescent());
}

#[test]
fn abandoned_producer_and_discarded_notification_are_observable() {
    let (operation, completion) = unit();
    let queue = Arc::new(Queue::default());
    operation
        .subscribe(queue.clone(), Job::new(|| panic!("discarded")))
        .unwrap();
    drop(completion);
    queue.0.lock().unwrap().clear();
    assert_eq!(
        operation.take().unwrap().unwrap_err().kind(),
        ErrorKind::Unavailable
    );
    assert!(operation.is_quiescent());
}

#[test]
fn result_kind_and_buffer_bound_are_checked() {
    let (operation, completion) = Operation::new(Kind::Bytes, 2, None).unwrap();
    completion.complete(Ok(Value::Bytes(vec![1, 2, 3])));
    assert_eq!(
        operation.take().unwrap().unwrap_err().kind(),
        ErrorKind::Invalid
    );
    let (operation, completion) = unit();
    completion.complete(Ok(Value::Bytes(vec![])));
    assert_eq!(
        operation.take().unwrap().unwrap_err().kind(),
        ErrorKind::Invalid
    );
}

#[test]
fn managed_dispatch_runs_off_submitting_thread() {
    let (send, receive) = mpsc::channel();
    let caller = std::thread::current().id();
    assert!(ManagedDispatcher
        .post(Job::new(move || send
            .send(std::thread::current().id())
            .unwrap()))
        .is_ok());
    assert_ne!(
        receive.recv_timeout(Duration::from_secs(5)).unwrap(),
        caller
    );
}

#[tokio::test]
async fn await_and_deadlines_work_on_an_independent_executor() {
    let (operation, completion) = Operation::new(Kind::Bytes, 3, None).unwrap();
    std::thread::spawn(move || completion.complete(Ok(Value::Bytes(vec![1, 2, 3]))));
    assert_eq!(operation.await.unwrap(), Value::Bytes(vec![1, 2, 3]));
    let (operation, completion) =
        Operation::new(Kind::Unit, 0, Some(Duration::from_millis(5))).unwrap();
    let observer = operation.clone();
    assert_eq!(operation.await.unwrap_err().kind(), ErrorKind::Timeout);
    assert!(completion.is_cancelled());
    assert!(!observer.is_quiescent());
    drop(completion);
    assert!(observer.is_quiescent());
}

#[test]
fn zero_deadline_and_dropping_future_cancel_before_start() {
    let (operation, completion) = Operation::new(Kind::Unit, 0, Some(Duration::ZERO)).unwrap();
    assert!(completion.is_cancelled());
    assert_eq!(
        operation.take().unwrap().unwrap_err().kind(),
        ErrorKind::Timeout
    );
    let (operation, completion) = unit();
    drop(operation.into_future());
    assert!(completion.is_cancelled());
}

#[test]
fn completion_and_cancel_race_preserve_one_result() {
    for _ in 0..100 {
        let (operation, completion) = unit();
        let observer = operation.clone();
        let thread = std::thread::spawn(move || completion.complete(Ok(Value::Unit)));
        observer.cancel();
        thread.join().unwrap();
        let result = operation.take().unwrap();
        assert!(
            matches!(result, Ok(Value::Unit)) || result.unwrap_err().kind() == ErrorKind::Cancelled
        );
        assert!(operation.take().is_err());
        assert!(operation.is_quiescent());
    }
}

#[tokio::test]
async fn second_await_fails_without_cancelling_first_consumer() {
    let (operation, completion) = unit();
    let first = operation.clone().into_future();
    assert_eq!(operation.await.unwrap_err().kind(), ErrorKind::Invalid);
    assert!(!completion.is_cancelled());
    completion.complete(Ok(Value::Unit));
    assert_eq!(first.await.unwrap(), Value::Unit);
}

#[test]
fn expired_completion_drops_provider_error_outside_lock() {
    #[derive(Debug)]
    struct Reenter(std::sync::Weak<Shared>);
    impl std::fmt::Display for Reenter {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("provider failure")
        }
    }
    impl std::error::Error for Reenter {}
    impl Drop for Reenter {
        fn drop(&mut self) {
            assert!(self.0.upgrade().unwrap().state.try_lock().is_ok());
        }
    }
    let shared = Arc::new(Shared {
        state: Mutex::new(State::default()),
        kind: Kind::Unit,
        max_bytes: 0,
        deadline: Some(Instant::now()),
        changed: Notify::new(),
        jobs: AtomicUsize::new(0),
        settled: AtomicBool::new(true),
        transform: Mutex::new(None),
    });
    let failure = crate::FsError::IoError(std::io::Error::other(Reenter(Arc::downgrade(&shared))));
    shared.publish(Err(failure.into()), false);
    assert_eq!(
        Operation(shared).take().unwrap().unwrap_err().kind(),
        ErrorKind::Timeout
    );
}
