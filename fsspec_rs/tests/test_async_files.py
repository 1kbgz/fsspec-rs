"""Foreign asyncio files through the installed C ABI, including cleanup and limits."""

import asyncio
from ctypes import CFUNCTYPE, POINTER, Structure, byref, c_char_p, c_int32, c_int64, c_size_t, c_uint32, c_uint64, c_void_p, cast, sizeof, string_at

import pytest

from fsspec_rs.tests.test_async_capi import (
    CANCEL,
    READY,
    RELEASE,
    Error,
    LoopDispatcher,
    _load_library,
    create,
)
from fsspec_rs.tests.test_capi import FileInfo, String, StringList

READ = CFUNCTYPE(None, c_void_p, c_size_t, c_void_p)
SEEK = CFUNCTYPE(None, c_void_p, c_int64, c_uint32, c_void_p)
FILE_CALL = CFUNCTYPE(None, c_void_p, c_void_p)
PATH_CALL = CFUNCTYPE(None, c_void_p, c_char_p, c_size_t, c_void_p)


class Limits(Structure):
    _fields_ = [("struct_size", c_size_t), ("max_operations", c_size_t), ("max_buffered_bytes", c_uint64), ("max_read_size", c_size_t)]


class Options(Structure):
    _fields_ = [("struct_size", c_size_t), ("timeout_ns", c_uint64)]


class FileVtable(Structure):
    _fields_ = [
        ("struct_size", c_size_t),
        ("abi_version", c_uint32),
        ("read", READ),
        ("seek", SEEK),
        ("info", FILE_CALL),
        ("close", FILE_CALL),
        ("release", RELEASE),
    ]


class FsVtable(Structure):
    _fields_ = [
        ("struct_size", c_size_t),
        ("abi_version", c_uint32),
        ("info", PATH_CALL),
        ("open_read", PATH_CALL),
        ("close", FILE_CALL),
        ("release", RELEASE),
    ]


@pytest.fixture(scope="module")
def lib():
    lib = _load_library()
    out, error = POINTER(c_void_p), POINTER(POINTER(Error))
    signatures = {
        "async_context_create": ([POINTER(Limits), out, error], c_int32),
        "async_file_from_vtable": ([c_void_p, c_void_p, POINTER(FileVtable), c_void_p, out, error], c_int32),
        "async_fs_from_vtable": ([c_void_p, c_void_p, POINTER(FsVtable), c_void_p, out, error], c_int32),
        "async_file_read": ([c_void_p, c_size_t, POINTER(Options), out, error], c_int32),
        "async_file_seek": ([c_void_p, c_int64, c_uint32, POINTER(Options), out, error], c_int32),
        "async_file_info": ([c_void_p, POINTER(Options), out, error], c_int32),
        "async_fs_open_read": ([c_void_p, c_char_p, c_size_t, POINTER(Options), out, error], c_int32),
        "async_fs_info": ([c_void_p, c_char_p, c_size_t, POINTER(Options), out, error], c_int32),
        "completion_file": ([c_void_p, c_void_p], None),
        "completion_info": ([c_void_p, POINTER(FileInfo)], None),
        "completion_position": ([c_void_p, c_uint64], None),
        "async_result_take_file": ([c_void_p, out, error], c_int32),
        "async_result_take_info": ([c_void_p, POINTER(FileInfo), error], c_int32),
        "async_result_position": ([c_void_p, POINTER(c_uint64), error], c_int32),
        "file_info_free": ([FileInfo], None),
    }
    for name in ("file", "fs", "context"):
        signatures[f"async_{name}_close"] = ([c_void_p, POINTER(Options), out, error], c_int32)
        signatures[f"async_{name}_release"] = ([c_void_p], None)
        signatures[f"async_{name}_retain"] = ([c_void_p], c_void_p)
    for name, (args, result) in signatures.items():
        function = getattr(lib, "fsspec_" + name)
        function.argtypes, function.restype = args, result
    return lib


def context(lib, *, size=64, operations=8, read=32):
    limits = Limits(sizeof(Limits), operations, size, read)
    result = c_void_p()
    assert lib.fsspec_async_context_create(byref(limits), byref(result), None) == 0
    return result


def start(lib, name, *args, timeout=None, expected=0):
    operation = c_void_p()
    options = Options(sizeof(Options), timeout) if timeout is not None else None
    error = POINTER(Error)()
    status = getattr(lib, "fsspec_" + name)(*args, byref(options) if options else None, byref(operation), byref(error))
    message = error.contents.message.value() if error else ""
    lib.fsspec_async_error_free(error)
    assert status & 0xFFFF == expected, message
    return operation


def take(lib, operation):
    result = c_void_p()
    assert lib.fsspec_operation_take(operation, byref(result), None) == 0
    try:
        error = lib.fsspec_async_result_error(result)
        if error:
            return error.contents.status & 0xFFFF, error.contents.message.value()
        kind = lib.fsspec_async_result_kind(result)
        if kind == 0:
            value = None
        elif kind == 1:
            size = c_size_t()
            data = lib.fsspec_async_result_bytes(result, byref(size))
            value = string_at(data, size.value)
        elif kind == 2:
            info = FileInfo()
            assert lib.fsspec_async_result_take_info(result, byref(info), None) == 0
            value = (info.name.value(), info.size)
            lib.fsspec_file_info_free(info)
        elif kind == 3:
            value = c_void_p()
            assert lib.fsspec_async_result_take_file(result, byref(value), None) == 0
        else:
            position = c_uint64()
            assert lib.fsspec_async_result_position(result, byref(position), None) == 0
            value = position.value
        return 0, value
    finally:
        lib.fsspec_async_result_free(result)


async def until(predicate):
    async def wait():
        while not predicate():
            await asyncio.sleep(0.001)

    await asyncio.wait_for(wait(), 5)


async def finish(lib, operation):
    loop = asyncio.get_running_loop()
    ready = loop.create_future()
    released = []
    callback = READY(lambda _data, _operation: loop.call_soon_threadsafe(ready.set_result, None))
    release = RELEASE(lambda _data: released.append(True))
    assert lib.fsspec_operation_subscribe(operation, None, callback, None, release, None) == 0
    try:
        await asyncio.wait_for(ready, 5)
        await until(lambda: released)
        return take(lib, operation)
    finally:
        lib.fsspec_operation_release(operation)


class FileProvider:
    def __init__(self, lib, ctx, dispatch):
        self.lib, self.dispatch = lib, dispatch
        self.handle = c_void_p()
        self.position = self.reads = self.closes = 0
        self.gate, self.cleanup, self.close_gate = asyncio.Event(), asyncio.Event(), asyncio.Event()
        self.gate.set()
        self.cleanup.set()
        self.close_gate.set()
        self.cleaning = asyncio.Event()
        self.released = False
        self.hooks = {}
        self.tasks = set()
        self.read = READ(lambda _data, size, producer: self.launch(producer, size))
        self.seek = SEEK(lambda _data, offset, whence, producer: self.seek_to(producer, offset, whence))
        self.info = FILE_CALL(lambda _data, producer: self.metadata(producer))
        self.close = FILE_CALL(lambda _data, producer: self.launch_close(producer))
        self.release = RELEASE(lambda _data: setattr(self, "released", True))
        self.vtable = FileVtable(sizeof(FileVtable), 1, self.read, self.seek, self.info, self.close, self.release)
        assert lib.fsspec_async_file_from_vtable(ctx, dispatch.handle, byref(self.vtable), None, byref(self.handle), None) == 0

    def launch(self, producer, size):
        async def read():
            self.reads += 1
            try:
                await self.gate.wait()
                data = b"abcdef"[self.position : self.position + size]
                self.position += len(data)
            except asyncio.CancelledError:
                self.cleaning.set()
                await self.cleanup.wait()
                self.lib.fsspec_completion_error(producer, 15, b"cancelled", 9)
            else:
                self.lib.fsspec_completion_bytes(producer, data, len(data))

        task = asyncio.create_task(read())
        self.tasks.add(task)
        task.add_done_callback(self.tasks.discard)
        cancel = CANCEL(lambda _data: task.cancel())
        release = RELEASE(lambda _data: self.hooks.pop(producer, None))
        self.hooks[producer] = (cancel, release)
        assert self.lib.fsspec_completion_on_cancel(producer, self.dispatch.handle, cancel, None, release, None) == 0

    def seek_to(self, producer, offset, whence):
        self.position = offset if whence == 0 else self.position + offset if whence == 1 else 6 + offset
        self.lib.fsspec_completion_position(producer, self.position)

    def metadata(self, producer):
        name = c_char_p(b"part")
        info = FileInfo(String(cast(name, c_void_p), 4), 6, 0, -1, -1, StringList(None, 0))
        self.lib.fsspec_completion_info(producer, byref(info))

    def launch_close(self, producer):
        async def close():
            self.closes += 1
            await self.close_gate.wait()
            self.lib.fsspec_completion_unit(producer)

        task = asyncio.create_task(close())
        self.tasks.add(task)
        task.add_done_callback(self.tasks.discard)


class FilesystemProvider:
    def __init__(self, lib, ctx, dispatch):
        self.lib, self.ctx, self.dispatch = lib, ctx, dispatch
        self.handle = c_void_p()
        self.files = []
        self.events = []
        self.released = False
        self.open = PATH_CALL(lambda _data, _path, _length, producer: self.open_file(producer))
        self.info = PATH_CALL(lambda _data, _path, _length, producer: lib.fsspec_completion_error(producer, 1, b"missing", 7))
        self.close = FILE_CALL(lambda _data, producer: self.close_fs(producer))
        self.release = RELEASE(lambda _data: setattr(self, "released", True))
        self.vtable = FsVtable(sizeof(FsVtable), 1, self.info, self.open, self.close, self.release)
        assert lib.fsspec_async_fs_from_vtable(ctx, dispatch.handle, byref(self.vtable), None, byref(self.handle), None) == 0

    def open_file(self, producer):
        file = FileProvider(self.lib, self.ctx, self.dispatch)
        self.files.append(file)
        self.lib.fsspec_completion_file(producer, file.handle)
        file.handle = c_void_p()

    def close_fs(self, producer):
        self.events.append([file.closes for file in self.files])
        self.lib.fsspec_completion_unit(producer)


def test_asyncio_open_read_seek_metadata_and_parent_close(lib):
    async def run():
        ctx = context(lib)
        dispatch = LoopDispatcher(lib, asyncio.get_running_loop())
        fs = FilesystemProvider(lib, ctx, dispatch)
        status, file = await finish(lib, start(lib, "async_fs_open_read", fs.handle, b"part", 4))
        assert status == 0
        assert await finish(lib, start(lib, "async_file_read", file, 3)) == (0, b"abc")
        assert await finish(lib, start(lib, "async_file_seek", file, 1, 0)) == (0, 1)
        assert await finish(lib, start(lib, "async_file_read", file, 2)) == (0, b"bc")
        assert await finish(lib, start(lib, "async_file_info", file)) == (0, ("part", 6))
        assert (await finish(lib, start(lib, "async_fs_info", fs.handle, b"none", 4)))[0] == 1
        assert await finish(lib, start(lib, "async_fs_close", fs.handle)) == (0, None)
        assert fs.events == [[1]]
        start(lib, "async_file_read", file, 1, expected=12)
        assert await finish(lib, start(lib, "async_context_close", ctx)) == (0, None)
        lib.fsspec_async_file_release(file)
        lib.fsspec_async_fs_release(fs.handle)
        lib.fsspec_async_context_release(ctx)
        await until(lambda: fs.released and all(file.released for file in fs.files))
        dispatch.close()
        dispatch.release_handle()

    asyncio.run(run())


def test_close_timeout_waits_for_cancelled_coroutine_cleanup(lib):
    async def run():
        ctx = context(lib, operations=1)
        dispatch = LoopDispatcher(lib, asyncio.get_running_loop())
        provider = FileProvider(lib, ctx, dispatch)
        provider.gate.clear()
        provider.cleanup.clear()
        read = start(lib, "async_file_read", provider.handle, 4)
        await until(lambda: provider.reads == 1)
        start(lib, "async_file_info", provider.handle, expected=12)
        assert (await finish(lib, start(lib, "async_file_close", provider.handle, timeout=0)))[0] == 13
        await asyncio.wait_for(provider.cleaning.wait(), 5)
        close = start(lib, "async_file_close", provider.handle)
        assert lib.fsspec_operation_state(close) == 0 and provider.closes == 0
        provider.cleanup.set()
        assert await finish(lib, close) == (0, None)
        assert (await finish(lib, read))[0] == 15
        assert provider.closes == 1
        assert await finish(lib, start(lib, "async_context_close", ctx)) == (0, None)
        lib.fsspec_async_file_release(provider.handle)
        lib.fsspec_async_context_release(ctx)
        await until(lambda: provider.released)
        dispatch.close()
        dispatch.release_handle()

    asyncio.run(run())


def test_bytes_wait_across_files_and_final_release_closes(lib):
    async def run():
        ctx = context(lib, size=4, read=4)
        dispatch = LoopDispatcher(lib, asyncio.get_running_loop())
        first, second = FileProvider(lib, ctx, dispatch), FileProvider(lib, ctx, dispatch)
        a = start(lib, "async_file_read", first.handle, 4)
        await until(lambda: lib.fsspec_operation_is_quiescent(a))
        b = start(lib, "async_file_read", second.handle, 4)
        await asyncio.sleep(0.01)
        assert second.reads == 0
        assert await finish(lib, a) == (0, b"abcd")
        assert await finish(lib, b) == (0, b"abcd")
        lib.fsspec_async_file_release(first.handle)
        await until(lambda: first.closes == 1)
        assert await finish(lib, start(lib, "async_context_close", ctx)) == (0, None)
        lib.fsspec_async_file_release(second.handle)
        lib.fsspec_async_context_release(ctx)
        await until(lambda: first.released and second.released)
        dispatch.close()
        dispatch.release_handle()

    asyncio.run(run())


def test_releasing_context_reference_does_not_close_live_file(lib):
    async def run():
        ctx = context(lib)
        dispatch = LoopDispatcher(lib, asyncio.get_running_loop())
        provider = FileProvider(lib, ctx, dispatch)
        lib.fsspec_async_context_release(ctx)
        await asyncio.sleep(0.01)
        assert await finish(lib, start(lib, "async_file_read", provider.handle, 2)) == (0, b"ab")
        assert await finish(lib, start(lib, "async_file_close", provider.handle)) == (0, None)
        lib.fsspec_async_file_release(provider.handle)
        await until(lambda: provider.released)
        dispatch.close()
        dispatch.release_handle()

    asyncio.run(run())


def test_large_diagnostic_is_bounded_without_losing_error_kind(lib):
    operation, completion = create(lib, kind=0, limit=0)
    message = ("€" * 5000).encode()
    lib.fsspec_completion_error(completion, 13, message, len(message))
    try:
        status, text = take(lib, operation)
        assert status == 13
        assert len(text.encode()) == 4095
    finally:
        lib.fsspec_operation_release(operation)
