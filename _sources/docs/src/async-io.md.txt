# Async I/O and execution contexts

**Status: operation lifetimes and callback dispatch are implemented.** Async
filesystem/file handles, shared byte budgets, native S3 streams, and context shutdown
remain proposed. Existing synchronous interfaces and C vtables are unchanged.
The {download}`draft C header <../proposals/async-io.h>` declares only that future
file/context API; implemented symbols are in the installed `fsspec.h`.

## Implemented operation API

`fsspec_rs::async_io` is available with the `async` feature, also enabled by
`object-store` and the default `s3` feature. `Operation::new` returns an observer
and a `Completion` producer. `Kind::Unit` and `Kind::Bytes` are supported; byte
operations require an explicit maximum result length. A zero limit permits only
an empty byte result. This limit covers the copied result, not provider allocations
or total bytes across operations.

```rust
use fsspec_rs::async_io::{Kind, Operation, Value};

async fn example() -> Result<(), fsspec_rs::OperationError> {
    let (operation, completion) = Operation::new(Kind::Bytes, 1024, None)?;
    std::thread::spawn(move || completion.complete(Ok(Value::Bytes(b"hello".to_vec()))));
    assert_eq!(operation.await?, Value::Bytes(b"hello".to_vec()));
    Ok(())
}
```

Only one clone may be awaited. Dropping that future requests cancellation; dropping
an ordinary observer does not. `is_quiescent()` reports whether the last producer
reference has been released. Producers retain a reference through their cleanup,
even if another reference publishes a result earlier.

The same operations are exported through C:

| Function                                       | Contract                                                                                                                                                   |
| ---------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `fsspec_operation_create`                      | Return observer/producer handles; kind 0 is unit, 1 is bytes. Relative `timeout_ns` includes dispatch; `UINT64_MAX` disables it, zero expires immediately. |
| `fsspec_operation_state` / `take`              | States 0 pending, 1 ready, 2 taken. Transfer one owned result; pending/taken is Invalid.                                                                   |
| `fsspec_operation_subscribe`                   | Register one readiness callback and optional dispatcher; null selects managed dispatch.                                                                    |
| `fsspec_operation_cancel`                      | Publish Cancelled if pending; an expired deadline publishes Timeout. Neither proves cleanup is complete.                                                   |
| `fsspec_operation_is_quiescent`                | Report release of the last producer reference.                                                                                                             |
| `fsspec_completion_on_cancel`                  | Register one cancellation callback, including after cancellation occurred.                                                                                 |
| `fsspec_completion_unit` / `bytes` / `error`   | Consume a producer reference. Bytes and error text are copied; oversized bytes are rejected before copying.                                                |
| `fsspec_async_result_kind` / `error` / `bytes` | Borrow payloads from an opaque result. Kind is 0 unit, 1 bytes, `UINT32_MAX` error. Free only the result with `fsspec_async_result_free`.                  |

Retain/release functions manage independent observer, producer, and dispatcher
references. `fsspec_sys::async_io::Operation` implements `Future` over these C
functions; its `Completion` wrapper can transfer a producer with `into_raw` and
accept one with unsafe `from_raw`. Errors contain owned status/message data and do
not use thread-local last-error state. Generic cancellation, timeout, and abandonment
are conservatively marked unsafe to retry because this layer cannot know whether
backend I/O had side effects.

`ManagedDispatcher` and C null dispatchers use the same lazy, process-owned Tokio
runtime as the object-store bridge. Foreign dispatchers schedule nonblocking jobs
on a caller-owned loop. They never receive Rust futures or native runtime handles.
An accepted job retains its dispatcher until run/discard. Finite deadlines use the
managed runtime even when callbacks use a foreign dispatcher.

`fsspec_rs/tests/test_async_capi.py` provides an asyncio integration fixture using
the bundled shared library. It exercises separate provider/consumer loops,
managed callback threads, cancellation before hook registration, delayed coroutine
cleanup, deadlines, and shutdown rejection/discard. It is a fixture, not a public
Python async filesystem API.

The remaining sections specify shared execution rules and the proposed file/context
extension. No async filesystem registration or native S3 streaming entry point is
exported yet.

## Execution contexts (proposed)

An async context owns admission limits, buffer accounting, deadlines, and operation
tracking. Filesystems, files, and operations retain their context. Context selection
is explicit when constructing a backend; there is no process-wide replacement hook.

Execution and callback dispatch are separate:

| Component                            | Runs where                                           | Ownership                                                           |
| ------------------------------------ | ---------------------------------------------------- | ------------------------------------------------------------------- |
| Native S3 futures                    | Tokio runtime selected by the Rust execution context | Managed by default; explicitly borrowed in a Rust application       |
| Python backend coroutines            | The backend's registered asyncio loop                | Caller-owned; fsspec never starts, stops, or closes it              |
| Backend start/cancel/close callbacks | Dispatcher supplied with the backend                 | Retained for all pending work and cleanup                           |
| Consumer readiness notification      | Dispatcher supplied at subscription                  | May differ from the backend's dispatcher                            |
| Deadlines and operation accounting   | Context's native execution runtime                   | Shared managed default, including for C-registered foreign backends |

An asyncio dispatcher executes callback jobs; it does not poll Tokio futures or
provide Tokio's network reactor. A foreign-only backend uses its own I/O runtime,
while `libfsspec` still supplies the shared runtime for deadline supervision.
Applications requiring zero native runtime threads are outside this initial contract.

### Managed default

The proposed `fsspec_async_context_create(NULL, ...)` creates an independent budget
and uses the existing shared managed Tokio runtime per loaded `libfsspec`. The initial worker count
is two, matching the existing object-store bridge runtime. The operation implementation shares
that runtime with callback dispatch; constructing a filesystem never creates another
worker pool. This does not claim that current `S3Fs` already uses the shared pool.

The runtime lives for the process lifetime; closing a context drains its operations
without stopping the shared runtime or unrelated contexts. `libfsspec` remains loaded
for the process lifetime when using this default. Foreign provider modules can be
unloaded after their own contexts drain and all their handles are released.

The proposed Rust context API lets applications explicitly construct a context with a Tokio `Handle` from
the same linked Rust implementation. The application must keep the owning runtime
alive and drive it until context close completes. A handle alone does not prevent
runtime shutdown, and a current-thread runtime needs an actively driven event loop.
There is no implicit `Handle::current()` selection and no nested `block_on`.

Operation wrappers implement Rust's standard `Future`; their consumers can await
completion on another Rust executor. This does not change the runtime driving native I/O.

A Tokio `Handle`, Rust future, Python loop pointer, or interpreter object is never
passed directly across the C ABI. A Rust extension with its own Tokio copy uses the
foreign backend/dispatcher interface, just as a Python extension does.

### Foreign dispatcher

`fsspec_dispatcher_from_vtable` copies the versioned vtable and takes ownership of
one `data` reference on success. Failure leaves ownership with the caller. A null
dispatcher on a backend or subscription selects managed callback dispatch.

`post(data, job, out_error)` may be called from any thread. It must return promptly
and never run the job on the submitting call stack. Another thread may run an
accepted job before `post` returns. All operation state is installed before posting.
No fsspec lock is held while entering foreign code.

A rejected post may return a library-owned error created by `async_error_new`; its
status must match the returned status. Without a message, fsspec supplies a generic
one. Foreign callbacks never return stack-allocated errors or buffers that fsspec's
allocator would later free. Successful posts return no error.

| `post` outcome | Job ownership                                                                  |
| -------------- | ------------------------------------------------------------------------------ |
| Success        | Dispatcher owns the job and calls exactly one of `job_run` or `job_discard`    |
| Failure        | Dispatcher has neither run nor retained the job; ownership remains with fsspec |

A dispatcher may serialize jobs on an event-loop thread or execute them concurrently;
its provider must match that guarantee. No global FIFO ordering is assumed. A started
job is not discarded, and each job handle is consumed once. Closing a dispatcher
rejects new posts and executes or discards accepted jobs before releasing its context.

Discarding a backend-start job fails its pending operation as `Unavailable` without
starting I/O. Discarding a notification does not overwrite an already recorded result;
that result remains available through operation state/take. Cancel-job discard does
not acknowledge that the underlying I/O stopped. Loop shutdown cannot make an abandoned
coroutine safe to free. Owners must drain work before stopping their loops.

`release(data)` and callback-context release functions may run on any thread. They
must be thread-safe and nonblocking. Loop-affine cleanup happens during async close,
before these final reference releases. Python release trampolines acquire the GIL
while the interpreter is alive; interpreter shutdown must follow context drain.

## Rust file contract (proposed)

The new object-safe handle uses ordinary `Send` futures and owned read buffers:

```rust
type IoFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, OperationError>> + Send + 'a>>;

trait AsyncFsFile: Send {
    fn read(&mut self, max_bytes: usize) -> IoFuture<'_, Vec<u8>>;
    fn seek(&mut self, position: SeekFrom) -> IoFuture<'_, u64>;
    fn info(&self) -> IoFuture<'_, FileInfo>;
    fn close(&mut self) -> IoFuture<'_, ()>;
}
```

`read` returns at most `max_bytes`. An empty result for a positive request means EOF;
a zero-byte request does not test EOF. `seek` uses the existing start/current/end
semantics; non-seekable backends return `NotSupported`. Metadata may require I/O.
`close` is asynchronous, idempotent, and releases resources after pending work stops.

The initial additive entry point is `AsyncFileSystem::open_async`, returning this
handle, with a default `NotSupported` implementation. Its context is bound at backend
construction. The existing `open` returns blocking `FsFile` and remains a legacy route;
its derived helpers remain documented as potentially blocking. New scans and Python
async bindings use `open_async`, not those helpers. An additive `close_async` method
provides backend-owned client cleanup, with a default no-op for borrowed resources. Renaming/removing the legacy route
requires a later compatibility decision. No sync backend becomes native async through
a blanket implementation or an implicit thread-pool wrapper.

Owned reads match the C result lifetime and avoid retaining a caller's borrowed buffer
across suspension. Standard async reader adapters can follow once this contract works;
they must preserve pending data when a caller changes its buffer between polls.

The proposed `fsspec_async_s3_open` uses an explicit context, or a new default-budget
context when passed null. Construction validates configuration without network I/O;
metadata requests occur through async operations. It creates a native client without a
per-filesystem runtime and without probing or wrapping a synchronous registry backend.

Native S3 opened readers pin an ETag or equivalent native validator and check it on
subsequent reads. Replacing an object fails the read instead of mixing versions. A
backend without that guarantee reports its limitation; it does not claim a snapshot.

## Operations and errors

A successful start returns an owned `fsspec_operation`. All path/options inputs are
copied or retained before start returns. Output buffers are never borrowed from callers
for the duration of an asynchronous operation. Backend callbacks borrow those copied
inputs only during the callback; providers copy values they need afterward.

Start functions report admission errors synchronously. The operation reports I/O errors
asynchronously. Both use the existing status kind and retry-safe bit. Errors have owned
UTF-8 messages; asynchronous completions never read thread-local last-error state.
Optional `out_error` receives an owned error on failure and null on success. Callers free
previous errors before reusing that slot. Other output arguments are unchanged on failure.

Operation states are `Pending`, `Ready`, and `Taken`. A Ready result contains exactly one
terminal outcome: success, failure, cancellation, or timeout. Publishing a terminal outcome
is atomic; competing outcomes cannot replace it. A completion must check deadline expiry
before publishing success, even if the timer callback has not run yet.

`operation_subscribe` installs one readiness callback and retains the supplied dispatcher.
Registration atomically checks readiness, including completion before or during registration.
A second subscription fails without replacing the first. Success transfers ownership of its
callback context; failure does not. Posting happens outside internal locks. Delivery may
precede the subscribing function's return on another thread.

Readiness callbacks borrow the operation for their call and run at most once. They are
notifications, not owners of its result. A rejected/discarded notification releases its
callback context; the result remains queryable. No callback is redirected to an arbitrary
thread after a foreign dispatcher rejects it. A stopped caller event loop cannot be made
to resume its awaiting tasks by fsspec.

`operation_take` transfers the owned result once. Taking a Pending or Taken operation is
`InvalidArgument`. Concurrent consumers must coordinate; retaining an operation is useful
for cancellation and inspection, not for broadcasting its payload. Rust Future wrappers
subscribe once, maintain their own current waker, and check readiness when polling.

`fsspec_async_result` is opaque. Its error and byte accessors borrow data until
`async_result_free`; callers must not free those borrowed pointers. The current
payloads are unit and bytes. Metadata, file, and position payloads require additive
accessors when the file API is implemented; no public result union is frozen now.

Raw C `operation_release` drops a reference, without implicitly cancelling work. High-level
Rust/Python wrappers request cancellation when an await is abandoned, then release their
reference. Provider leases keep operation state and resources valid independently.

## Foreign backend completion and cancellation

A backend receives an owned `fsspec_completion` producer token. Its start callback does
not wait for I/O: it schedules work and returns, retaining the token until I/O and cleanup
finish. It may complete synchronously inside the callback. The caller's readiness callback
is still dispatched separately.

Completion functions consume one producer reference. Bytes and error messages are
copied during that call. The proposed `info` completion copies metadata; `file`
transfers one owned handle reference. Completion payloads
must match the operation type and declared read limit. Invalid payloads fail as
`InvalidArgument`. Late completions consume/dispose their payloads but do not change the
terminal outcome. Releasing the final producer token without a completion fails an otherwise
pending operation as `Unavailable`; it acknowledges that no producer work remains.

A producer registers at most one cancellation hook with `completion_on_cancel`. The hook
has its own dispatcher and context lifetime. Registration catches cancellation that happened
before it; cancellation after registration schedules the same hook. It runs at most once,
including when timeout and explicit cancellation race. It is a request to stop, not proof of
cleanup. Token completion/final release is the acknowledgement of quiescence. Providers
retain tokens for work that ignores cancellation or is still in a `finally` block.

| Race                                                | Required outcome                                                         |
| --------------------------------------------------- | ------------------------------------------------------------------------ |
| Cancel before backend-start job runs                | Record Cancelled and skip backend invocation                             |
| Cancel during/after start, before hook installation | Record Cancelled; run hook when installed                                |
| Success versus cancellation/timeout                 | First valid terminal transition wins; expired deadlines prevent success  |
| Completion after cancellation/timeout               | Dispose payload; release producer lease only after actual cleanup        |
| Consumer releases operation while work continues    | Producer lease prevents use-after-free                                   |
| Provider loses its last token without completing    | Pending operation becomes Unavailable; no silent permanent Pending state |

Logical cancellation may wake a consumer promptly while the provider is still cleaning up.
The proposed admission slots and buffer reservations remain charged until work is quiescent; cancellation
cannot be used to exceed resource limits by repeatedly starting replacements.

## File and context shutdown (proposed)

Read/seek operations share a cursor. Rust's mutable borrow serializes them; the C wrapper
allows one outstanding cursor operation per file. An overlapping operation fails with
`Unavailable` and the retry-safe bit. Pending close rejects new I/O. Immutable metadata
queries can proceed separately where the provider supports them.

A cancelled read or seek may have changed the cursor. The handle remains busy until its
producer stops. Before reuse, a seekable handle requires a successful absolute seek;
a non-seekable handle must be closed. Queued operations that never reached the backend
have not changed the cursor. Rust adapters preserve this distinction internally.

`file_close` cancels pending work, waits for quiescence, invokes provider close, and finishes
only after cleanup completes. Repeated close joins the same cleanup; it does not invoke
provider close twice. A close timeout leaves the file Closing. Another close can await the
remaining cleanup. Final handle release schedules best-effort cleanup; explicit close is the
observable guarantee. Read-handle close never publishes writes.

`fs_close` rejects new filesystem work, drains its operations/files, then invokes the
optional provider filesystem-close hook once. An open result attaches its file to that
filesystem and must use the same context. Independently constructed file handles belong
to their context directly. The filesystem hook closes provider-owned async clients;
it never implicitly closes a caller-owned shared client. A missing hook means there
are no provider-owned resources needing async cleanup.

`context_close` rejects new work, cancels pending operations, closes its files and
filesystems in that order, and drains producer leases and queued jobs. Its own close operation is excluded from the drain count.
Control operations have reserved admission, so a full data queue cannot prevent cancellation
or close. Repeated context close joins existing cleanup. A timed-out close does not authorize
releasing resources still used by a foreign provider.

The owner keeps its event loop driven through drain. The interpreter and provider code
remain alive until drain completes and all foreign handles and callback references are released.
A loop that rejects new jobs fails newly submitted backend work as Unavailable. Stopping a
loop after it accepted jobs without executing/discarding them violates the dispatcher contract;
fsspec retains outstanding leases rather than freeing live pointers. No runtime can guarantee
cleanup progress after its owner has stopped driving it.

## Deadlines and shared bounds

Proposed context defaults are 64 admitted data operations, 64 MiB of accounted buffers, and
8 MiB per read. Values must be positive and `max_read_size <= max_buffered_bytes`. All
backends under a context share its budget, regardless of bucket or credential identity.
Separate contexts have separate budgets but share the default runtime.

Admission exhaustion returns retry-safe Unavailable before dispatch. An admitted operation
waits asynchronously for byte capacity; the wait counts toward its deadline. Large reads
are decomposed by callers or streaming adapters into bounded requests. One request above
the context's read limit is InvalidArgument, with no allocation or backend I/O.

Accounting covers copied arguments, fsspec-owned transport/output buffers, and queued
results. Reserve capacity before allocation/dispatch, including copies at an ABI crossing.
Count retained allocations, not just the lengths of slices sharing a larger allocation.
Credit is released when memory is freed or the result is transferred to its consumer.
Foreign producer buffers borrowed during completion remain provider-owned; the copied
result is charged to fsspec. Foreign clients must bound their own buffering, and returning
more bytes than requested is an error. Caller-retained results, provider allocations,
independent backend caches, allocator overhead, and uninstrumented HTTP/codec internals
are outside the context budget: it is not an RSS cap or a bound on arbitrary foreign code.
A later shared buffer-allocation ABI would be needed to account for foreign allocations.

Streaming has bounded prefetch and stops requesting more data when consumers stop reading.
Early close cancels pending reads and drains their leases. Existing materializing
`get_ranges` remains a collecting API; this proposal does not relabel it as streaming.

Timeouts are durations from submission, measured by fsspec's monotonic clock, including
queueing and dispatch. Zero expires without starting I/O; `UINT64_MAX` means no deadline.
The producer can query remaining time without sharing clock epochs. Timeout records the
Timeout kind, requests cancellation, and follows the same lease rules as explicit cancel.
The future file wrapper can mark cancellation/timeouts before dispatch safe to retry.
The implemented generic operation layer always marks its cancellation/timeouts unsafe to retry. After dispatch, cursor reads
and relative seeks report `retry_safe=false` when their position is uncertain; callers
recover position before submitting a new read. Metadata, immutable positioned reads,
and idempotent close can report retry-safe errors. Future writes must define their own
unknown-outcome rules.

## Python asyncio binding (proposed)

The binding captures an explicit loop, or `asyncio.get_running_loop()` at construction from
an async context. It holds strong references to the loop and provider while work exists.
It does not select a process-global loop or call `asyncio.run()` for each request.

| C hook             | Python behavior                                                                                              |
| ------------------ | ------------------------------------------------------------------------------------------------------------ |
| Dispatcher `post`  | Acquire GIL; enqueue `job_run` with `loop.call_soon_threadsafe`; retain job wrapper until run/discard        |
| Backend start      | On provider loop, create an asyncio Task for the backend coroutine; keep producer token through task cleanup |
| Cancellation hook  | Dispatch `task.cancel` onto provider loop; do not mutate a Task from a worker thread                         |
| Backend completion | Convert task result/exception and finish producer; cancellation is acknowledged only after coroutine cleanup |
| Consumer readiness | On consumer loop, take result and settle its asyncio Future if still pending                                 |
| Await cancelled    | Request native operation cancellation; preserve producer lifetime until cleanup finishes                     |
| `aclose()`         | Await context/file drain while provider and consumer loops remain driven                                     |

The adapter must register task cancellation and completion safely even with an eager task
factory or a task already completed at registration. Consumer subscriptions capture the
originating Python caller's `contextvars` context for readiness delivery. Python backend
registration captures a provider context and copies it for each C-originated operation;
arbitrary per-request Python context is not implicitly transported through C. Neither
side inherits a native callback thread's context. Credentials remain explicit backend
configuration. Callback exceptions are caught and translated to structured operation errors;
Python exceptions and Rust panics never unwind across C. GIL acquisition alone does not
make manipulating asyncio Futures from a foreign thread valid.

A Python backend can therefore export its async files through C to a Rust consumer:
Rust starts an operation, the registered dispatcher schedules the coroutine on Python's
loop, and the producer resolves a Rust Future through completion notification. The inverse
uses the managed Tokio backend and posts its completion onto the awaiting Python loop.
These loops can be different and may run on different threads.

The Python adapter must use an explicit closing state and retain accepted job wrappers;
`call_soon_threadsafe` alone does not provide discard/drain accounting. `aclose()` disposes
queued wrappers before loop shutdown. Closing somebody else's loop is never part of release.

## Compatibility and acceptance

New opaque handles, vtables, and symbols are additive. Existing file/fs vtables are not
reinterpreted. New vtables start with `struct_size`/`abi_version`, use existing validation
rules, and copy callback tables. Missing required callbacks fail construction; optional
seek reports NotSupported. Required filesystem callbacks are info/open/release; close is optional for borrowed
resources. Required
file callbacks are read/info/close/release. Dispatcher post/release are required.

Constructor success transfers the provider's one data reference; failure leaves it
with the caller. Context and dispatcher arguments are borrowed and internally retained.
Null options select defaults, including no operation deadline. Nonnull option structs
must include all fields declared in this draft; invalid sizes or zero context limits
are rejected before dispatch. Retain of null returns null; release/free of null is a
no-op. Query and completion functions require valid nonnull handles. Typed completion
entry points consume their token even when the payload is invalid or the result is late.

New async APIs do not make synchronous `FsFile` calls on event-loop threads. Explicit
blocking adapters, if added later, use a bounded shared executor and retain accounting until
the blocking call ends. Async operations never wait synchronously for callbacks on their
own event loop. Native async registration and lookup remain separate from the existing
synchronous registry until an additive factory contract is implemented; a sync registry
entry alone does not advertise async support.

Implementation must pass these cases in Rust and through the built wheel's C ABI:

- Two native S3 backends share the managed runtime and context budget; none creates a worker
  pool per file/backend. An explicitly borrowed Tokio runtime remains caller-owned.
- A Python asyncio backend serves a Rust consumer and a native Rust backend serves an asyncio
  consumer. Heartbeats continue during delayed I/O and cancellation cleanup.
- Provider loop and consumer loop differ; contextvars and callback thread identity are correct.
- Complete-before-subscribe, cancellation-before-hook, synchronous completion, eager Python
  tasks, completion-after-timeout, discarded dispatch jobs, and released consumer handles.
- Start errors and asynchronous failures preserve kind, retry safety, and owned messages
  despite unrelated errors on worker threads. Invalid pointers/options fail before dispatch.
- Slow consumers, oversized requests, retained slices, ABI copies, repeated cancelled requests,
  and pending cleanup cannot exceed the declared accounting budget.
- Same-handle cursor races, cancelled seek/read recovery, repeated close, close timeout, and
  context close with a saturated data queue. Cleanup never implicitly commits.
- Provider/loop shutdown rejects new work and drains accepted work; callbacks and finalizers
  cannot use an unloaded module or finalized Python interpreter.
- Unchanged synchronous suites, C/C++ header compilation, direct Rust builds, shared-library
  wheels, and the no-default-features core/WASM build.

Operation/producer lifetimes, managed and foreign callback dispatch, Rust Future
wrappers, and the asyncio integration fixture are implemented. Next add async file
handles with shared admission/byte budgets and drainable close, then native S3
reads over that boundary.

The execution rules are grounded in Python's
[thread-safe asyncio scheduling](https://docs.python.org/3/library/asyncio-dev.html#concurrency-and-multithreading)
and [event-loop lifecycle](https://docs.python.org/3/library/asyncio-eventloop.html#running-and-stopping-the-loop),
Tokio's [runtime handle contract](https://docs.rs/tokio/latest/tokio/runtime/struct.Handle.html)
and [runtime ownership rules](https://docs.rs/tokio/latest/tokio/runtime/struct.Runtime.html#sharing).
