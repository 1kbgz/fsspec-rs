from __future__ import annotations

import fsspec
from fsspec.spec import AbstractBufferedFile

from fsspec_rs.fsspec_rs import RustMemoryFs


class MemoryFileSystem(fsspec.AbstractFileSystem):
    """In-memory filesystem backed by the Rust ``MemoryFs`` implementation.

    The protocol is registered as ``"memory-rs"`` so it can coexist with the
    pure-Python ``fsspec.implementations.memory.MemoryFileSystem``, which owns
    ``"memory"``. Every instance shares one store: the Rust registry's ``memory``
    filesystem, which native code in the same process opens as ``memory://``.
    """

    protocol = ("memory-rs",)
    root_marker = "/"

    @classmethod
    def _strip_protocol(cls, path):
        """Strip the ``memory-rs://`` prefix and root the path at ``/``."""
        return "/" + path.removeprefix(f"{cls.protocol[0]}://").strip("/")

    def unstrip_protocol(self, path):
        return f"{self.protocol[0]}://{path}"

    def __init__(self, **storage_options):
        super().__init__(**storage_options)
        self._rust = RustMemoryFs()

    # Core primitives — delegated to Rust

    def ls(self, path: str, detail: bool = True, **kwargs):
        return self._rust.ls(self._strip_protocol(path), detail=detail)

    def info(self, path: str, **kwargs):
        return self._rust.info(self._strip_protocol(path))

    def _open(
        self,
        path: str,
        mode: str = "rb",
        block_size=None,
        autocommit: bool = True,
        cache_options=None,
        cache_type=None,
        **kwargs,
    ):
        """Return a Rust-backed file object."""
        return MemoryFile(
            self,
            self._strip_protocol(path),
            mode=mode,
            cache_type=cache_type,
            block_size=block_size,
        )

    def mkdir(self, path: str, create_parents: bool = True, **kwargs):
        return self._rust.mkdir(self._strip_protocol(path), create_parents=create_parents)

    def makedirs(self, path: str, exist_ok: bool = False):
        return self._rust.makedirs(self._strip_protocol(path), exist_ok=exist_ok)

    def rmdir(self, path: str):
        return self._rust.rmdir(self._strip_protocol(path))

    def rm_file(self, path: str):
        return self._rust.rm_file(self._strip_protocol(path))

    def rm(self, path, recursive: bool = False, maxdepth=None):
        if isinstance(path, str):
            return self._rust.rm(self._strip_protocol(path), recursive=recursive)
        for p in path:
            self._rust.rm(self._strip_protocol(p), recursive=recursive)

    def cp_file(self, path1: str, path2: str, **kwargs):
        return self._rust.cp_file(self._strip_protocol(path1), self._strip_protocol(path2))

    def copy(self, path1: str, path2: str, recursive: bool = False, **kwargs):
        return self._rust.copy(self._strip_protocol(path1), self._strip_protocol(path2), recursive=recursive)

    def mv(self, path1: str, path2: str, **kwargs):
        return self._rust.mv(self._strip_protocol(path1), self._strip_protocol(path2))

    # Higher-level helpers — delegated to Rust for speed

    def exists(self, path: str, **kwargs) -> bool:
        return self._rust.exists(self._strip_protocol(path))

    def isdir(self, path: str) -> bool:
        return self._rust.isdir(self._strip_protocol(path))

    def isfile(self, path: str) -> bool:
        return self._rust.isfile(self._strip_protocol(path))

    def size(self, path: str) -> int:
        return self._rust.size(self._strip_protocol(path))

    def cat_file(self, path: str, start=None, end=None, **kwargs) -> bytes:
        return bytes(self._rust.cat_file(self._strip_protocol(path), start=start, end=end))

    def pipe_file(self, path: str, value: bytes, **kwargs):
        return self._rust.pipe_file(self._strip_protocol(path), value)

    def head(self, path: str, size: int = 1024) -> bytes:
        return bytes(self._rust.head(self._strip_protocol(path), size))

    def tail(self, path: str, size: int = 1024) -> bytes:
        return bytes(self._rust.tail(self._strip_protocol(path), size))

    def touch(self, path: str, truncate: bool = True, **kwargs):
        return self._rust.touch(self._strip_protocol(path), truncate=truncate)

    def walk(self, path: str, maxdepth=None, topdown: bool = True, **kwargs):
        yield from self._rust.walk(self._strip_protocol(path), max_depth=maxdepth, topdown=topdown)

    def find(self, path: str, maxdepth=None, withdirs: bool = False, **kwargs):
        return self._rust.find(self._strip_protocol(path), max_depth=maxdepth, with_dirs=withdirs)

    def du(self, path: str, total: bool = True, maxdepth=None, **kwargs):
        return self._rust.du(self._strip_protocol(path), total=total)

    def read_text(self, path: str, encoding=None, errors=None, newline=None, **kwargs) -> str:
        return self._rust.read_text(self._strip_protocol(path))

    def write_text(self, path: str, value: str, encoding=None, errors=None, newline=None, **kwargs):
        return self._rust.write_text(self._strip_protocol(path), value)

    def get_file(self, rpath: str, lpath: str, **kwargs):
        return self._rust.get_file(self._strip_protocol(rpath), lpath)

    def put_file(self, lpath: str, rpath: str, **kwargs):
        return self._rust.put_file(lpath, self._strip_protocol(rpath))


class MemoryFile(AbstractBufferedFile):
    """File-like wrapper delegating to the Rust ``RustMemoryFile``."""

    def __init__(self, fs: MemoryFileSystem, path: str, mode: str = "rb", cache_type=None, block_size=None, **kwargs):
        # Open the Rust file handle right away
        self._rust_file = None  # set early so __del__/closed don't blow up
        open_kwargs = {}
        if cache_type is not None:
            open_kwargs["cache_type"] = cache_type
        if block_size is not None:
            open_kwargs["block_size"] = block_size
        self._rust_file = fs._rust.open(path, mode, **open_kwargs)
        self.path = path
        self.mode = mode
        self.fs = fs
        # Do NOT call super().__init__() — we are a thin wrapper, and
        # AbstractBufferedFile's __init__ tries to call size() etc.
        # which does not apply for write-mode files. Instead we just
        # set the minimum attributes that __repr__ et al. need.
        self.blocksize = 0
        self.loc = 0

    # io.RawIOBase-like interface

    def read(self, length: int = -1) -> bytes:
        data = bytes(self._rust_file.read(length))
        self.loc += len(data)
        return data

    def write(self, data: bytes) -> int:
        n = self._rust_file.write(data)
        self.loc += n
        return n

    def seek(self, loc: int, whence: int = 0) -> int:
        pos = self._rust_file.seek(loc, whence)
        self.loc = pos
        return pos

    def tell(self) -> int:
        return self._rust_file.tell()

    def flush(self, force: bool = False):
        self._rust_file.flush()

    def close(self):
        self._rust_file.close()

    @property
    def closed(self) -> bool:
        return self._rust_file is None or self._rust_file.closed

    def readable(self) -> bool:
        return "r" in self.mode

    def writable(self) -> bool:
        return "w" in self.mode or "a" in self.mode or "x" in self.mode

    def seekable(self) -> bool:
        return True

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()

    def __repr__(self) -> str:
        status = "closed" if self.closed else "open"
        return f"MemoryFile('{self.path}', mode='{self.mode}', {status})"
