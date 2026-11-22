"""Exercise foreign asyncio dispatch against the library shipped in the wheel."""

import asyncio
import threading
from ctypes import CDLL, CFUNCTYPE, POINTER, Structure, byref, c_bool, c_char_p, c_int32, c_size_t, c_uint32, c_uint64, c_void_p, sizeof, string_at

import pytest

from fsspec_rs.tests.test_capi import String, _library_path


class Error(Structure):
    _fields_ = [("status", c_int32), ("message", String)]


POST = CFUNCTYPE(c_int32, c_void_p, c_void_p, POINTER(POINTER(Error)))
RELEASE = CFUNCTYPE(None, c_void_p)
READY = CFUNCTYPE(None, c_void_p, c_void_p)
CANCEL = CFUNCTYPE(None, c_void_p)


class DispatcherVtable(Structure):
    _fields_ = [("struct_size", c_size_t), ("abi_version", c_uint32), ("post", POST), ("release", RELEASE)]


def _load_library():
    path = _library_path()
    if not path.exists():
        pytest.skip(f"libfsspec is not built: {path}")
    lib = CDLL(str(path))
    error_out = POINTER(POINTER(Error))
    signatures = {
        "operation_create": ([c_uint32, c_size_t, c_uint64, POINTER(c_void_p), POINTER(c_void_p), error_out], c_int32),
        "operation_take": ([c_void_p, POINTER(c_void_p), error_out], c_int32),
        "operation_subscribe": ([c_void_p, c_void_p, READY, c_void_p, RELEASE, error_out], c_int32),
        "operation_state": ([c_void_p], c_uint32),
        "operation_is_quiescent": ([c_void_p], c_bool),
        "operation_retain": ([c_void_p], c_void_p),
        "completion_retain": ([c_void_p], c_void_p),
        "completion_is_cancelled": ([c_void_p], c_bool),
        "completion_remaining_ns": ([c_void_p], c_uint64),
        "completion_on_cancel": ([c_void_p, c_void_p, CANCEL, c_void_p, RELEASE, error_out], c_int32),
        "completion_bytes": ([c_void_p, c_char_p, c_size_t], None),
        "completion_error": ([c_void_p, c_int32, c_char_p, c_size_t], None),
        "async_error_new": ([c_int32, c_char_p, c_size_t], POINTER(Error)),
        "async_error_free": ([POINTER(Error)], None),
        "async_result_error": ([c_void_p], POINTER(Error)),
        "async_result_kind": ([c_void_p], c_uint32),
        "async_result_bytes": ([c_void_p, POINTER(c_size_t)], c_void_p),
        "dispatcher_from_vtable": ([POINTER(DispatcherVtable), c_void_p, POINTER(c_void_p), error_out], c_int32),
        "dispatcher_retain": ([c_void_p], c_void_p),
        "set_last_error": ([c_char_p, c_size_t], None),
    }
    for name in (
        "operation_cancel",
        "operation_release",
        "completion_release",
        "completion_unit",
        "dispatcher_release",
        "dispatch_job_run",
        "dispatch_job_discard",
        "async_result_free",
    ):
        signatures[name] = ([c_void_p], None)
    for name, (args, result) in signatures.items():
        function = getattr(lib, "fsspec_" + name)
        function.argtypes, function.restype = args, result
    return lib


@pytest.fixture(scope="module")
def lib():
    return _load_library()


def create(lib, *, kind=1, limit=16, timeout=2**64 - 1):
    operation, completion = c_void_p(), c_void_p()
    error = POINTER(Error)()
    status = lib.fsspec_operation_create(kind, limit, timeout, byref(operation), byref(completion), byref(error))
    assert status == 0 and not error
    return operation, completion


def take(lib, operation):
    result, error = c_void_p(), POINTER(Error)()
    assert lib.fsspec_operation_take(operation, byref(result), byref(error)) == 0
    try:
        error = lib.fsspec_async_result_error(result)
        if error:
            return error.contents.status, error.contents.message.value(), None
        kind = lib.fsspec_async_result_kind(result)
        length = c_size_t()
        data = lib.fsspec_async_result_bytes(result, byref(length))
        return 0, "", string_at(data, length.value) if kind == 1 else None
    finally:
        lib.fsspec_async_result_free(result)


class LoopDispatcher:
    """Track accepted jobs so loop shutdown can discard jobs it never ran."""

    def __init__(self, lib, loop):
        self.lib, self.loop = lib, loop
        self.lock = threading.Lock()
        self.jobs = set()
        self.closed = False
        self.released = threading.Event()

        @POST
        def post(_data, job, _error):
            with self.lock:
                if self.closed:
                    return 12  # Unavailable; caller still owns job.
                self.jobs.add(job)
                try:
                    loop.call_soon_threadsafe(self.run, job)
                except RuntimeError:
                    self.jobs.remove(job)
                    return 12
            return 0

        self.post = post
        self.release = RELEASE(lambda _data: self.released.set())
        self.vtable = DispatcherVtable(sizeof(DispatcherVtable), 1, post, self.release)
        self.handle = c_void_p()
        assert lib.fsspec_dispatcher_from_vtable(byref(self.vtable), None, byref(self.handle), None) == 0

    def run(self, job):
        with self.lock:
            if job not in self.jobs:
                return
            self.jobs.remove(job)
        self.lib.fsspec_dispatch_job_run(job)

    def close(self):
        with self.lock:
            self.closed = True
            jobs, self.jobs = self.jobs, set()
        for job in jobs:
            self.lib.fsspec_dispatch_job_discard(job)

    def release_handle(self):
        self.lib.fsspec_dispatcher_release(self.handle)
        self.handle = None


def test_asyncio_provider_and_consumer_on_separate_loops(lib):
    provider_loop = asyncio.new_event_loop()
    thread = threading.Thread(target=provider_loop.run_forever)
    thread.start()

    async def run():
        loop = asyncio.get_running_loop()
        dispatch = LoopDispatcher(lib, loop)
        operation, completion = create(lib)
        done = loop.create_future()
        callback_threads = []
        released = threading.Event()

        @READY
        def ready(_data, borrowed):
            callback_threads.append(threading.get_ident())
            retained = lib.fsspec_operation_retain(borrowed)
            done.set_result(take(lib, retained))
            lib.fsspec_operation_release(retained)

        release = RELEASE(lambda _data: released.set())
        assert lib.fsspec_operation_subscribe(operation, dispatch.handle, ready, None, release, None) == 0

        async def produce():
            await asyncio.sleep(0)
            lib.fsspec_completion_bytes(completion, b"hello", 5)
            return threading.get_ident()

        provider = asyncio.run_coroutine_threadsafe(produce(), provider_loop)
        try:
            assert await asyncio.wait_for(done, 5) == (0, "", b"hello")
            assert await asyncio.wrap_future(provider) != threading.get_ident()
            assert callback_threads == [threading.get_ident()]
            assert released.is_set()
            assert lib.fsspec_operation_is_quiescent(operation)
        finally:
            lib.fsspec_operation_release(operation)
            dispatch.close()
            dispatch.release_handle()
        assert dispatch.released.is_set()

    try:
        asyncio.run(run())
    finally:
        provider_loop.call_soon_threadsafe(provider_loop.stop)
        thread.join(timeout=5)
        provider_loop.close()


@pytest.mark.parametrize("deadline", [False, True])
def test_cancel_or_deadline_wakes_consumer_before_asyncio_cleanup(lib, deadline):
    async def run():
        loop = asyncio.get_running_loop()
        dispatch = LoopDispatcher(lib, loop)
        operation, completion = create(lib, timeout=20_000_000 if deadline else 2**64 - 1)
        cleanup_started, cleanup_allowed = asyncio.Event(), asyncio.Event()
        ready_seen = loop.create_future()
        released = []

        async def produce():
            try:
                await asyncio.Event().wait()
            finally:
                cleanup_started.set()
                await cleanup_allowed.wait()
                lib.fsspec_completion_bytes(completion, b"late", 4)

        task = asyncio.create_task(produce())
        await asyncio.sleep(0)  # Start its try/finally before cancelling.
        cancel = CANCEL(lambda _data: task.cancel())
        ready = READY(lambda _data, _operation: ready_seen.set_result(None))
        release = RELEASE(lambda data: released.append(data))
        try:
            if not deadline:
                lib.fsspec_operation_cancel(operation)  # Before hook registration.
            assert lib.fsspec_completion_on_cancel(completion, dispatch.handle, cancel, 1, release, None) == 0
            assert lib.fsspec_operation_subscribe(operation, dispatch.handle, ready, 2, release, None) == 0
            await asyncio.wait_for(ready_seen, 5)
            await asyncio.wait_for(cleanup_started.wait(), 5)
            assert lib.fsspec_completion_is_cancelled(completion)
            assert not lib.fsspec_operation_is_quiescent(operation)
            assert take(lib, operation)[0] == (13 if deadline else 15)
            cleanup_allowed.set()
            with pytest.raises(asyncio.CancelledError):
                await task
            assert lib.fsspec_operation_is_quiescent(operation)
            assert sorted(released) == [1, 2]
        finally:
            cleanup_allowed.set()
            if not task.done():
                task.cancel()
                await asyncio.gather(task, return_exceptions=True)
            lib.fsspec_operation_release(operation)
            dispatch.close()
            dispatch.release_handle()

    asyncio.run(run())


def test_managed_dispatch_and_completed_subscription(lib):
    operation, completion = create(lib, kind=0)
    lib.fsspec_completion_unit(completion)
    seen, released = threading.Event(), threading.Event()
    threads = []

    @READY
    def ready(_data, borrowed):
        threads.append((threading.get_ident(), lib.fsspec_operation_state(borrowed)))
        seen.set()

    release = RELEASE(lambda _data: released.set())
    try:
        assert lib.fsspec_operation_subscribe(operation, None, ready, None, release, None) == 0
        assert seen.wait(5) and released.wait(5)
        assert threads[0][0] != threading.get_ident() and threads[0][1] == 1
        assert take(lib, operation) == (0, "", None)
        error, result = POINTER(Error)(), c_void_p(42)
        assert lib.fsspec_operation_take(operation, byref(result), byref(error)) == 7
        assert result.value == 42 and "already taken" in error.contents.message.value()
        lib.fsspec_async_error_free(error)
    finally:
        lib.fsspec_operation_release(operation)


@pytest.mark.parametrize("reject", [False, True])
def test_stopped_dispatcher_discards_or_rejects_without_losing_result(lib, reject):
    loop = asyncio.new_event_loop()  # Intentionally never driven.
    dispatch = LoopDispatcher(lib, loop)
    operation, completion = create(lib, kind=0)
    released, called = [], []
    ready = READY(lambda _data, _operation: called.append(True))
    release = RELEASE(lambda data: released.append(data))
    try:
        if reject:
            dispatch.close()
        assert lib.fsspec_operation_subscribe(operation, dispatch.handle, ready, 1, release, None) == 0
        error = POINTER(Error)()
        assert lib.fsspec_operation_subscribe(operation, dispatch.handle, ready, 2, release, byref(error)) == 7
        lib.fsspec_async_error_free(error)
        lib.fsspec_completion_unit(completion)
        dispatch.release_handle()
        if not reject:
            assert not dispatch.released.is_set()  # Queued job retains dispatcher.
        dispatch.close()
        assert dispatch.released.is_set()
        assert released == [1]  # Rejected registration never takes data=2.
        assert called == []
        assert take(lib, operation) == (0, "", None)
    finally:
        lib.fsspec_operation_release(operation)
        dispatch.close()
        dispatch.release_handle()
        loop.close()


@pytest.mark.parametrize("mode, expected", [("bound", 7), ("null", 7), ("kind", 7), ("abandon", 12), ("error", 0x10000 | 13), ("zero", 13)])
def test_owned_results_and_producer_lifetimes(lib, mode, expected):
    operation, completion = create(lib, kind=0 if mode == "kind" else 1, limit=2, timeout=0 if mode == "zero" else 2**64 - 1)
    lease = lib.fsspec_completion_retain(completion)
    try:
        if mode == "bound":
            lib.fsspec_completion_bytes(completion, None, 3)  # Bound checked before dereferencing.
        elif mode == "null":
            lib.fsspec_completion_bytes(completion, None, 1)
        elif mode == "kind":
            lib.fsspec_completion_bytes(completion, b"x", 1)
        elif mode == "error":
            lib.fsspec_completion_error(completion, expected, b"provider timeout", 16)
        else:
            lib.fsspec_completion_release(completion)
        assert not lib.fsspec_operation_is_quiescent(operation)
        if mode == "abandon":
            assert lib.fsspec_operation_state(operation) == 0
        lib.fsspec_completion_release(lease)
        lease = None
        assert lib.fsspec_operation_is_quiescent(operation)
        status, message, _data = take(lib, operation)
        assert status == expected
        if mode == "error":
            assert message == "provider timeout"
    finally:
        lib.fsspec_completion_release(lease)
        lib.fsspec_operation_release(operation)


def test_invalid_creation_leaves_output_and_callback_ownership_with_caller(lib):
    operation, completion, error = c_void_p(41), c_void_p(42), POINTER(Error)()
    assert lib.fsspec_operation_create(99, 0, 0, byref(operation), byref(completion), byref(error)) == 7
    assert (operation.value, completion.value) == (41, 42)
    lib.fsspec_async_error_free(error)
    released = []
    post = POST(lambda _data, _job, _error: 12)
    release = RELEASE(lambda data: released.append(data))
    vtable = DispatcherVtable(sizeof(DispatcherVtable), 99, post, release)
    handle = c_void_p(43)
    assert lib.fsspec_dispatcher_from_vtable(byref(vtable), 1, byref(handle), byref(error)) == 7
    assert handle.value == 43 and not released
    lib.fsspec_async_error_free(error)
    assert not lib.fsspec_async_error_new(0, b"", 0)
    assert not lib.fsspec_async_error_new(13, b"\xff", 1)


@pytest.mark.parametrize("status, expected", [(2, 2), (0x10000 | 13, 0x10000 | 13), (0x10000 | 16, 16)])
def test_foreign_error_message_survives_thread_handoff_and_unrelated_errors(lib, status, expected):
    operation, completion = create(lib)
    thread = threading.Thread(target=lib.fsspec_completion_error, args=(completion, status, b"provider failure", 16))
    thread.start()
    thread.join(timeout=5)
    try:
        assert not thread.is_alive()
        lib.fsspec_set_last_error(b"unrelated", 9)
        assert take(lib, operation) == (expected, "provider failure", None)
    finally:
        lib.fsspec_operation_release(operation)
