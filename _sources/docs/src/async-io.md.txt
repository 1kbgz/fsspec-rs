# Async I/O and execution contexts

Async operation lifetimes, callback dispatch, read-only file/filesystem handles,
shared limits, cleanup, and native S3 range streaming are implemented in Rust and
the C ABI. A public Python async filesystem wrapper remains future work.
Existing synchronous interfaces and C vtables are unchanged.

## Rust interfaces

Enable the `async` feature for `fsspec_rs::async_io` and
`fsspec_rs::async_files`. The `object-store` feature and default `s3` feature
also enable it. The core without default features still supports WASM.

`AsyncContext::new(Limits)` creates a shared admission and byte budget.
`AsyncFile::new` and `AsyncFilesystem::new` register a provider and dispatcher in
that context. Their `read`, `seek`, `info`, `open_read`, and `close` methods return
operations that can be awaited on any Rust executor. Paths are copied before
start returns; providers borrow them only during their start callback.

`FileProvider` and `FilesystemProvider` expose nonblocking start callbacks. A
provider schedules I/O on its own executor and retains a `Completion` through
cleanup. `ManagedDispatcher` schedules callbacks on the library's shared Tokio
runtime. A caller-supplied `Dispatcher` can instead post them to another executor.
Neither dispatcher polls foreign futures on behalf of their owner.

`AsyncFsFile` provides an object-safe interface with owned read buffers:

```rust
pub type IoFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, OperationError>> + Send + 'a>>;

pub trait AsyncFsFile: Send + Sync {
    fn read(&mut self, max_bytes: usize) -> IoFuture<'_, Vec<u8>>;
    fn seek(&mut self, position: SeekFrom) -> IoFuture<'_, u64>;
    fn info(&self) -> IoFuture<'_, FileInfo>;
    fn close(&mut self) -> IoFuture<'_, ()>;
}
```

The provided core and shared-library file wrappers implement this trait. Successful
trait reads, seeks, and metadata calls wait for producer and accounting cleanup
before returning, so sequential calls work even when a provider publishes success
early. Errors remain prompt; a cancelled cursor operation may still need cleanup
and an absolute seek before reuse. Closing waits for that cleanup.

`AsyncFileSystem::open_async` is an additive, read-only entry point returning this
handle; its default is `NotSupported`. `close_async` defaults to a no-op for borrowed
backend resources. Existing `open` still returns a blocking `FsFile`; its derived
helpers remain potentially blocking. There is no implicit thread-pool adapter. Native S3 uses the explicit
`AsyncFilesystem::s3` constructor below; the synchronous `S3Fs` API is unchanged.

`fsspec_sys::async_files` provides owned context, filesystem, and file handles
through `libfsspec`, with the same operations and Rust trait implementation.
Its unsafe `from_raw` constructors take one owned handle reference; `into_raw`
transfers one to C. The library and provider code must remain loaded through cleanup.

## Native S3 reads

`AsyncFilesystem::s3(bucket, options, context)` constructs a native S3 client
without registry lookup or network I/O. Options match `S3Fs::from_options`, including
credentials, endpoint, region, timeouts, and retries. `fsspec_async_s3_open` exposes
the same constructor in C; a null context selects default limits.
`fsspec_sys::async_files::AsyncFilesystem::s3` wraps it for shared-library callers.
All native operations use the shared managed runtime, without per-client runtimes
or `block_on`. Consumer notifications can still use a caller-owned dispatcher.

Paths accept bucket-relative filesystem keys, bucket-prefixed paths, and
`s3://bucket/key` URLs. URLs naming another bucket are rejected. Keys retain the
synchronous filesystem's escaping rules. Metadata addresses objects directly;
there is no directory inference or listing fallback.

Opening performs HEAD and pins the object's ETag and size. Each nonempty read
issues a bounded GET with `If-Match`, checks the returned ETag, size, and range,
and streams the response into the requested output buffer. No body or prefetch
survives the operation. Changed or missing objects fail as Conflict; missing ETags
at open are NotSupported. Invalid ranges and incomplete bodies fail without
advancing the cursor. Transport errors retain their structured classification.
Metadata on an open file describes the pinned object. Zero-length and EOF reads
return empty buffers without GET requests. Seek uses the pinned size and permits
positions beyond EOF, but rejects negative positions and offset overflow.

A native read reserves twice its requested size before dispatch: one output buffer
and one incoming chunk. Reservations share the context budget and remain until
producer cleanup and result transfer/disposal. Reads whose combined reservation
exceeds the context budget fail as Invalid. HTTP/TLS internal allocations and
backing allocations retained by transport chunks remain outside this byte budget;
it is not a process memory cap. Cached file metadata is bounded by the context's
read limit per handle and remains provider-owned. Endpoint concurrency permits
cover headers and body consumption and are shared with synchronous range reads.

Cancellation and deadlines abort pending native requests, including waiting for
endpoint capacity or response data. Drain waits for the request future and its
body buffers to be dropped. File/filesystem/context close uses the same cancellation
and drain rules as foreign providers. Async writes and public Python wrappers are
not implemented.

## Operation results

`Operation::new` returns an observer and a `Completion` producer. Results can be
unit, bytes, metadata, a file, or a position. An operation has one terminal outcome
and its result can be taken once across all observers. Only one core observer can
be awaited. Dropping its future requests cancellation; releasing an ordinary raw
observer does not.

```rust
use fsspec_rs::async_io::{Kind, Operation, Value};

async fn example() -> Result<(), fsspec_rs::OperationError> {
    let (operation, completion) = Operation::new(Kind::Bytes, 1024, None)?;
    std::thread::spawn(move || completion.complete(Ok(Value::Bytes(b"hello".to_vec()))));
    assert_eq!(operation.await?, Value::Bytes(b"hello".to_vec()));
    Ok(())
}
```

Success, failure, cancellation, and timeout compete for one terminal transition.
An expired deadline prevents success even if its timer has not run yet. Late
completions consume their payload and producer reference without replacing the
recorded result. Dropping the final producer without completing pending work
records `Unavailable`.

Readiness does not imply cleanup. `is_quiescent()` reports release of the last
producer reference. `drain()` waits for producers, accepted callback jobs, and
handle accounting cleanup. `drain_operation()` returns a unit operation for the
same wait. Neither takes the source result. A retained, untaken byte result keeps
its buffer reservation even after work drains.

Each producer reference is a cleanup lease. Retain one when publishing an early
result while I/O or a coroutine's `finally` block still uses resources. Cancellation
may wake the consumer immediately but does not free those resources or permit
replacement work to exceed the context limits.

## C ABI

Implemented declarations are installed in `fsspec.h`. New handles and vtables are
independent of the synchronous registry. Registering a sync filesystem does not
advertise native async support.

| Function family                                                        | Behavior                                                                                                                                                 |
| ---------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `fsspec_async_s3_open`                                                 | Construct native read-only S3 handles; null context selects defaults.                                                                                    |
| `fsspec_async_context_create`                                          | Construct a budget context; null options select defaults.                                                                                                |
| `fsspec_async_fs_from_vtable`, `fsspec_async_file_from_vtable`         | Copy a versioned vtable and retain a required context and optional dispatcher. Null dispatcher selects managed dispatch.                                 |
| `fsspec_async_fs_open_read`, `fsspec_async_fs_info`                    | Copy the path and start an operation.                                                                                                                    |
| `fsspec_async_file_read`, `seek`, `info`                               | Start cursor or metadata operations. Seek whence is 0 start, 1 current, 2 end; negative start offsets are invalid.                                       |
| `fsspec_async_file_close`, `fs_close`, `context_close`                 | Start or join cleanup without consuming the handle. Control operations bypass data admission limits.                                                     |
| `fsspec_operation_create`                                              | Construct an independent observer/producer pair without a context budget.                                                                                |
| `fsspec_operation_state`, `take`                                       | States: 0 pending, 1 ready, 2 taken. Transfer one owned result. Pending/taken is Invalid.                                                                |
| `fsspec_operation_subscribe`                                           | Install one notification, including after completion; null dispatcher uses managed dispatch.                                                             |
| `fsspec_operation_cancel`, `is_quiescent`, `drain`                     | Request cancellation, inspect producer cleanup, or return a unit operation waiting for complete drain.                                                   |
| `fsspec_completion_on_cancel`                                          | Register one cancellation hook, including after cancellation occurred.                                                                                   |
| `fsspec_completion_unit`, `bytes`, `info`, `file`, `position`, `error` | Complete and consume one producer reference, including late or invalid completion. Bytes, metadata, and error text are copied; file ownership transfers. |
| `fsspec_async_result_kind`                                             | 0 unit, 1 bytes, 2 info, 3 file, 4 position; `UINT32_MAX` error.                                                                                         |
| `fsspec_async_result_error`, `bytes`                                   | Borrow data until result free; never free those borrowed pointers.                                                                                       |
| `fsspec_async_result_take_info`, `take_file`                           | Move payload out once and leave a unit result. Free metadata with `fsspec_file_info_free`; release files with `fsspec_async_file_release`.               |
| `fsspec_async_result_position`                                         | Copy a successful position value.                                                                                                                        |

Filesystem callbacks `info`, `open_read`, and `release` are required; `close` is
optional for borrowed clients. File callbacks `read`, `info`, `close`, and `release`
are required; missing `seek` returns `NotSupported`. C start offsets above
`INT64_MAX` are unsupported. All callbacks must return promptly and never unwind.

Constructors take one provider `data` reference only on success. On failure,
ownership remains with the caller. Start errors and asynchronous results use owned
status/message errors, never thread-local last-error state. Optional `out_error`
is set to null on success or an owned error on failure; other outputs are unchanged
on failure. Free prior errors before reusing an error slot. Retain of null returns
null; release/free of null is a no-op. Query functions require live nonnull handles.

## Runtime and dispatcher ownership

Native coordination, callback defaults, and deadlines share one lazy, process-owned
Tokio runtime with the object-store bridge: two async workers and up to eight
blocking workers. Constructing a context or file does not create a worker pool.
Current synchronous `S3Fs` still has its existing runtime behavior.

A foreign dispatcher schedules callback jobs on a caller-owned event loop. Python
coroutines use that loop's I/O facilities; Tokio futures need their native runtime
and reactor. No Python loop object, Rust future, or Tokio handle crosses C. Finite
deadlines still use the managed runtime when callbacks use a foreign dispatcher.
Selecting a borrowed Tokio runtime for native coordination is not implemented yet.

`post(data, job, out_error)` may run on any thread. It must not execute the job on
the submitting stack. Another thread may execute it before `post` returns.

| Post result | Ownership                                                                                    |
| ----------- | -------------------------------------------------------------------------------------------- |
| Success     | Dispatcher owns the job and must call exactly one of `fsspec_dispatch_job_run` or `discard`. |
| Failure     | Dispatcher has neither run nor retained the job; library retains ownership.                  |

Accepted jobs retain their dispatcher until execution/discard. Closing a dispatcher
rejects new posts and runs or discards accepted jobs. Rejected/discarded starts fail
as `Unavailable`. Discarding a readiness notification preserves its recorded result;
discarding cancellation does not prove I/O stopped. There is no fallback to an
arbitrary thread after a caller-owned dispatcher rejects work.

Provider and callback `release(data)` functions may run on any thread and must be
thread-safe and nonblocking. Loop-affine cleanup belongs in `close`. Python release
trampolines acquire the GIL while the interpreter is alive. Keep loops driven and
all callback code loaded until operations drain and references are released.

## Limits and backpressure

Context defaults are 64 admitted operations, 64 MiB of data buffers, and 8 MiB per
read/metadata result. Limits must be positive, with read limit no greater than the
context byte limit. All handles in a context share limits.

Admission exhaustion returns retry-safe `Unavailable` before dispatch. An admitted
file read or metadata request waits asynchronously for byte capacity; that wait
counts toward its deadline. Paths must be copied before a C start returns, so
filesystem `open_read` and `info` reserve their argument and maximum result capacity
up front. If capacity is unavailable, these starts return retry-safe `Unavailable`
instead of retaining unaccounted path data while waiting. Oversized requests fail
as Invalid before allocation or dispatch.

Reservations cover copied paths and retained byte/metadata string buffers. A generic read
reserves its requested size (providers can declare additional staging capacity); metadata reserves the context's read limit. Native
`Vec` and `String` capacities, including unused capacity, must fit the reservation.
C byte/metadata inputs are checked before copying. Reservations remain until both
producer cleanup and result transfer/disposal; errors release unused result capacity
once producers stop. Caller-owned returned data and foreign provider allocations
are outside the budget.

Diagnostics have a separate 4096-byte UTF-8 text limit. Oversized provider errors are
truncated at a character boundary while preserving error kind and retry safety.
Diagnostics, handle/control structures, metadata map overhead, allocator overhead,
and uninstrumented provider/HTTP buffers are outside the data-byte budget. These
limits are not an RSS cap. Providers must bound their own buffering.

Timeouts use relative monotonic nanoseconds. Zero expires immediately;
`UINT64_MAX` means no deadline. A producer can query remaining time without sharing
clock epochs. Cancellation/timeouts are conservatively unsafe to retry. Provider
errors preserve their retry-safe flag, except unknown outcomes are never safe to
retry. Writes require a separate async publication contract.

## Cursor and shutdown behavior

One read/seek can use a file cursor at a time. Metadata can run separately. Raw
operation calls report retry-safe `Unavailable` when a previous cursor operation
has not stopped. Failed or cancelled dispatched cursor operations require an
absolute seek before reuse; a non-seekable file must be closed. Cancellation before
provider invocation does not invalidate the cursor.

Files retain their context; files returned by `open_read` also retain their parent
filesystem. Releasing the caller's context or filesystem reference does not close
these dependents. The final file reference schedules best-effort close; use explicit
close to observe cleanup and failures. Dropping an untaken file result also releases
its file reference.

File close rejects new I/O, cancels existing operations, drains producers and
accepted jobs, and invokes provider close. Filesystem close drains its open/info
operations, closes its children, then closes its provider client. Context close
rejects new work and joins cleanup for its resources. Provider close itself must
retain its completion until resource cleanup is finished.

Repeated close calls join a pending cleanup or reuse successful cleanup. A close
waiter's cancellation/timeout does not stop cleanup; another close can await it.
A failed provider close can be retried, while the handle remains closed to new I/O.
Read-only cleanup never commits writes. Closing cannot guarantee progress after
an event-loop owner stops driving its loop.

## Integration coverage and remaining native work

The ctypes fixtures in `test_async_capi.py` and `test_async_files.py` exercise
managed notification threads, caller-owned asyncio providers, read/seek/metadata,
parent close ordering, cancelled coroutine cleanup, shared limits, and final release.
Rust tests cover retained producer leases, result capacities, cursor recovery, close
retries, dispatcher rejection/discard, and safe futures through C. These fixtures
are not a public Python async filesystem implementation.

Native S3 fixtures cover HTTP ranges, validator changes, stalled response bodies,
shared output/staging budgets, and cancellation cleanup. The ctypes fixture reads
S3 objects through the installed library from asyncio. Async scan integration and
context-aware Python wrappers can now use these handles; live-store certification
remains separate.

The execution rules are grounded in Python's
[thread-safe asyncio scheduling](https://docs.python.org/3/library/asyncio-dev.html#concurrency-and-multithreading)
and [event-loop lifecycle](https://docs.python.org/3/library/asyncio-eventloop.html#running-and-stopping-the-loop),
Tokio's [runtime handle contract](https://docs.rs/tokio/latest/tokio/runtime/struct.Handle.html)
and [runtime ownership rules](https://docs.rs/tokio/latest/tokio/runtime/struct.Runtime.html#sharing).
