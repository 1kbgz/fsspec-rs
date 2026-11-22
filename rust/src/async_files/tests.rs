use super::*;
use crate::async_io::ManagedDispatcher;

#[derive(Default)]
struct Queue(Mutex<Vec<Job>>);
impl Dispatcher for Queue {
    fn post(&self, job: Job) -> std::result::Result<(), (OperationError, Job)> {
        self.0.lock().unwrap().push(job);
        Ok(())
    }
}
impl Queue {
    fn run(&self) {
        for job in std::mem::take(&mut *self.0.lock().unwrap()) {
            job.run();
        }
    }
    fn discard(&self) {
        let jobs = std::mem::take(&mut *self.0.lock().unwrap());
        drop(jobs);
    }
}
#[derive(Default)]
struct Memory {
    cursor: Mutex<usize>,
    reads: AtomicUsize,
    closes: AtomicUsize,
    delayed: Mutex<Option<Completion>>,
    hold_reads: AtomicBool,
}
impl FileProvider for Memory {
    fn read(&self, size: usize, completion: Completion) {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.hold_reads.load(Ordering::Acquire) {
            *self.delayed.lock().unwrap() = Some(completion);
            return;
        }
        let mut cursor = self.cursor.lock().unwrap();
        let end = (*cursor + size).min(6);
        let bytes = b"abcdef"[*cursor..end].to_vec();
        *cursor = end;
        completion.complete(Ok(Value::Bytes(bytes)));
    }
    fn seek(&self, position: SeekFrom, completion: Completion) {
        match position {
            SeekFrom::Start(value) if value <= 6 => {
                *self.cursor.lock().unwrap() = value as usize;
                completion.complete(Ok(Value::Position(value)));
            }
            _ => completion.complete(Err(error(ErrorKind::Invalid, "invalid memory offset"))),
        }
    }
    fn info(&self, completion: Completion) {
        completion.complete(Ok(Value::Info(FileInfo::file("memory", 6))));
    }
    fn close(&self, completion: Completion) {
        self.closes.fetch_add(1, Ordering::SeqCst);
        completion.complete(Ok(Value::Unit));
    }
}
fn context(bytes: usize, operations: usize) -> AsyncContext {
    AsyncContext::new(Limits {
        max_operations: operations,
        max_buffered_bytes: bytes,
        max_read_size: bytes,
    })
    .unwrap()
}
async fn until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !predicate() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
async fn finish(operation: Operation) -> Result<Value> {
    tokio::time::timeout(Duration::from_secs(3), operation.into_future())
        .await
        .unwrap()
}
use std::future::IntoFuture;

#[tokio::test]
async fn reads_share_byte_budget_until_result_transfer() {
    let context = context(4, 4);
    let first = Arc::new(Memory::default());
    let second = Arc::new(Memory::default());
    let file = AsyncFile::new(&context, Arc::new(ManagedDispatcher), first).unwrap();
    let other = AsyncFile::new(&context, Arc::new(ManagedDispatcher), second.clone()).unwrap();
    let a = file.read(4, None).unwrap();
    a.wait_quiescent().await;
    let b = other.read(4, None).unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(second.reads.load(Ordering::Acquire), 0);
    assert_eq!(a.take().unwrap().unwrap(), Value::Bytes(b"abcd".to_vec()));
    assert_eq!(finish(b).await.unwrap(), Value::Bytes(b"abcd".to_vec()));
    finish(context.close(None).unwrap()).await.unwrap();
}

#[tokio::test]
async fn cancellation_keeps_admission_and_bytes_until_cleanup_and_requires_seek() {
    let context = context(8, 1);
    let provider = Arc::new(Memory::default());
    provider.hold_reads.store(true, Ordering::Release);
    let file = AsyncFile::new(&context, Arc::new(ManagedDispatcher), provider.clone()).unwrap();
    let operation = file.read(4, None).unwrap();
    until(|| provider.delayed.lock().unwrap().is_some()).await;
    operation.cancel();
    assert_eq!(
        operation.take().unwrap().unwrap_err().kind(),
        ErrorKind::Cancelled
    );
    assert_eq!(
        file.info(None).err().unwrap().kind(),
        ErrorKind::Unavailable
    );
    assert_eq!(context.0 .0.state.lock().unwrap().bytes, 4);
    provider
        .delayed
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .complete(Ok(Value::Bytes(vec![])));
    until(|| context.0 .0.state.lock().unwrap().active == 0).await;
    assert_eq!(context.0 .0.state.lock().unwrap().bytes, 0);
    assert_eq!(file.read(1, None).err().unwrap().kind(), ErrorKind::Invalid);
    finish(file.seek(SeekFrom::Start(0), None).unwrap())
        .await
        .unwrap();
    until(|| context.0 .0.state.lock().unwrap().active == 0).await;
    provider.hold_reads.store(false, Ordering::Release);
    assert_eq!(
        finish(file.read(2, None).unwrap()).await.unwrap(),
        Value::Bytes(b"ab".to_vec())
    );
    finish(context.close(None).unwrap()).await.unwrap();
}

#[tokio::test]
async fn cancel_before_dispatch_skips_provider_without_poisoning_cursor() {
    let context = context(8, 8);
    let queue = Arc::new(Queue::default());
    let provider = Arc::new(Memory::default());
    let file = AsyncFile::new(&context, queue.clone(), provider.clone()).unwrap();
    let operation = file.read(1, None).unwrap();
    until(|| !queue.0.lock().unwrap().is_empty()).await;
    operation.cancel();
    queue.run();
    operation.drain().await;
    assert_eq!(provider.reads.load(Ordering::Acquire), 0);
    let next = file.read(1, None).unwrap();
    until(|| !queue.0.lock().unwrap().is_empty()).await;
    queue.run();
    assert_eq!(finish(next).await.unwrap(), Value::Bytes(b"a".to_vec()));
    let close = file.close(None).unwrap();
    until(|| !queue.0.lock().unwrap().is_empty()).await;
    queue.run();
    finish(close).await.unwrap();
}

#[tokio::test]
async fn close_timeout_and_repeat_join_cleanup_with_full_admission() {
    let context = context(8, 1);
    let provider = Arc::new(Memory::default());
    provider.hold_reads.store(true, Ordering::Release);
    let file = AsyncFile::new(&context, Arc::new(ManagedDispatcher), provider.clone()).unwrap();
    let read = file.read(4, None).unwrap();
    until(|| provider.delayed.lock().unwrap().is_some()).await;
    let close = file.close(Some(Duration::ZERO)).unwrap();
    assert_eq!(finish(close).await.unwrap_err().kind(), ErrorKind::Timeout);
    assert_eq!(provider.closes.load(Ordering::Acquire), 0);
    let again = file.close(None).unwrap();
    assert!(file.info(None).is_err());
    provider
        .delayed
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .complete(Ok(Value::Bytes(vec![])));
    finish(again).await.unwrap();
    assert_eq!(provider.closes.load(Ordering::Acquire), 1);
    assert_eq!(
        read.take().unwrap().unwrap_err().kind(),
        ErrorKind::Cancelled
    );
    finish(file.close(None).unwrap()).await.unwrap();
    assert_eq!(provider.closes.load(Ordering::Acquire), 1);
    finish(context.close(None).unwrap()).await.unwrap();
}

#[tokio::test]
async fn discarded_start_fails_and_close_can_retry_rejected_cleanup() {
    let context = context(8, 2);
    let queue = Arc::new(Queue::default());
    let provider = Arc::new(Memory::default());
    let file = AsyncFile::new(&context, queue.clone(), provider.clone()).unwrap();
    let read = file.read(2, None).unwrap();
    until(|| !queue.0.lock().unwrap().is_empty()).await;
    queue.discard();
    assert_eq!(
        finish(read).await.unwrap_err().kind(),
        ErrorKind::Unavailable
    );
    let close = file.close(None).unwrap();
    until(|| !queue.0.lock().unwrap().is_empty()).await;
    queue.discard();
    assert_eq!(
        finish(close).await.unwrap_err().kind(),
        ErrorKind::Unavailable
    );
    let close = file.close(None).unwrap();
    until(|| !queue.0.lock().unwrap().is_empty()).await;
    queue.run();
    finish(close).await.unwrap();
    assert_eq!(provider.closes.load(Ordering::Acquire), 1);
}

struct Filesystem {
    context: AsyncContext,
    file: Arc<Memory>,
    closes: AtomicUsize,
}
impl FilesystemProvider for Filesystem {
    fn info(&self, path: &str, completion: Completion) {
        completion.complete(Ok(Value::Info(FileInfo::file(path, 6))));
    }
    fn open_read(&self, _: &str, completion: Completion) {
        let file = AsyncFile::new(
            &self.context,
            Arc::new(ManagedDispatcher),
            self.file.clone(),
        );
        completion.complete(file.map(Value::File));
    }
    fn close(&self, completion: Completion) {
        assert_eq!(self.file.closes.load(Ordering::Acquire), 1);
        self.closes.fetch_add(1, Ordering::AcqRel);
        completion.complete(Ok(Value::Unit));
    }
}
#[tokio::test]
async fn filesystem_close_drains_opened_files_before_client_cleanup() {
    let context = context(64, 8);
    let provider = Arc::new(Filesystem {
        context: context.clone(),
        file: Arc::new(Memory::default()),
        closes: AtomicUsize::new(0),
    });
    let fs = AsyncFilesystem::new(&context, Arc::new(ManagedDispatcher), provider.clone()).unwrap();
    let file = match finish(fs.open_read("part", None).unwrap()).await.unwrap() {
        Value::File(file) => file,
        _ => panic!(),
    };
    assert_eq!(
        finish(file.read(2, None).unwrap()).await.unwrap(),
        Value::Bytes(b"ab".to_vec())
    );
    finish(fs.close(None).unwrap()).await.unwrap();
    assert_eq!(provider.closes.load(Ordering::Acquire), 1);
    assert!(file.read(1, None).is_err());
    finish(context.close(None).unwrap()).await.unwrap();
}

#[tokio::test]
async fn context_close_waits_for_accepted_notification_jobs() {
    let context = context(8, 4);
    let queue = Arc::new(Queue::default());
    let file = AsyncFile::new(
        &context,
        Arc::new(ManagedDispatcher),
        Arc::new(Memory::default()),
    )
    .unwrap();
    let read = file.read(1, None).unwrap();
    read.subscribe(queue.clone(), Job::new(|| {})).unwrap();
    read.wait_quiescent().await;
    let close = context.close(None).unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(close.state(), 0);
    queue.discard();
    finish(close).await.unwrap();
}

#[tokio::test]
async fn dropping_context_or_filesystem_reference_keeps_dependent_file_open() {
    let ctx = context(64, 4);
    let file = AsyncFile::new(
        &ctx,
        Arc::new(ManagedDispatcher),
        Arc::new(Memory::default()),
    )
    .unwrap();
    drop(ctx);
    assert_eq!(
        finish(file.read(1, None).unwrap()).await.unwrap(),
        Value::Bytes(b"a".to_vec())
    );
    finish(file.close(None).unwrap()).await.unwrap();

    let ctx = context(64, 4);
    let provider = Arc::new(Filesystem {
        context: ctx.clone(),
        file: Arc::new(Memory::default()),
        closes: AtomicUsize::new(0),
    });
    let fs = AsyncFilesystem::new(&ctx, Arc::new(ManagedDispatcher), provider.clone()).unwrap();
    let file = match finish(fs.open_read("part", None).unwrap()).await.unwrap() {
        Value::File(file) => file,
        _ => panic!(),
    };
    drop(fs);
    assert_eq!(
        finish(file.read(1, None).unwrap()).await.unwrap(),
        Value::Bytes(b"a".to_vec())
    );
    finish(ctx.close(None).unwrap()).await.unwrap();
    assert_eq!(provider.closes.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn large_spare_capacity_and_diagnostics_cannot_bypass_bounds() {
    let (operation, completion) = Operation::new(Kind::Bytes, 1, None).unwrap();
    let mut bytes = Vec::with_capacity(1024 * 1024);
    bytes.push(1);
    completion.complete(Ok(Value::Bytes(bytes)));
    assert_eq!(
        finish(operation).await.unwrap_err().kind(),
        ErrorKind::Invalid
    );
    let (operation, completion) = Operation::new(Kind::Unit, 0, None).unwrap();
    completion.complete(Err(error(ErrorKind::Unavailable, &"é".repeat(10000))));
    let result = finish(operation).await.unwrap_err();
    assert_eq!(result.kind(), ErrorKind::Unavailable);
    assert_eq!(result.to_string().len(), crate::async_io::MAX_ERROR_BYTES);
}

#[derive(Default)]
struct Early(Mutex<Vec<Completion>>);
impl FileProvider for Early {
    fn read(&self, _: usize, completion: Completion) {
        self.0.lock().unwrap().push(completion.clone());
        completion.complete(Ok(Value::Bytes(vec![42])));
    }
    fn info(&self, completion: Completion) {
        completion.complete(Ok(Value::Info(FileInfo::file("", 1))));
    }
    fn close(&self, completion: Completion) {
        completion.complete(Ok(Value::Unit));
    }
}
#[tokio::test]
async fn trait_reads_wait_for_success_cleanup_and_release_single_admission_slot() {
    let ctx = context(1, 1);
    let provider = Arc::new(Early::default());
    let mut file = AsyncFile::new(&ctx, Arc::new(ManagedDispatcher), provider.clone()).unwrap();
    for _ in 0..2 {
        let read = AsyncFsFile::read(&mut file, 1);
        tokio::pin!(read);
        assert!(tokio::time::timeout(Duration::from_millis(10), &mut read)
            .await
            .is_err());
        assert_eq!(provider.0.lock().unwrap().len(), 1);
        provider.0.lock().unwrap().clear();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), read)
                .await
                .unwrap()
                .unwrap(),
            vec![42]
        );
    }
    finish(ctx.close(None).unwrap()).await.unwrap();
}
