from __future__ import annotations

import fsspec
from fsspec.config import conf
from fsspec.spec import AbstractBufferedFile

from fsspec_rs.fsspec_rs import RustS3Fs


class S3FileSystem(fsspec.AbstractFileSystem):
    """S3 filesystem backed by the Rust ``S3Fs`` implementation.

    The protocol is registered as ``"s3-rs"`` so it can coexist with the
    pure-Python ``s3fs.S3FileSystem`` which owns the ``"s3"`` protocol.

    Credential resolution (matching s3fs conventions):
      1. Explicit keyword arguments (``key``, ``secret``, ``endpoint_url``, …)
      2. Values from ``fsspec.config.conf["s3"]`` (populated from ``FSSPEC_S3_*``
         environment variables)
      3. Falls through to object_store's own credential chain (IAM roles, etc.)
    """

    protocol = ("s3-rs",)

    def __init__(
        self,
        bucket: str,
        key: str | None = None,
        secret: str | None = None,
        endpoint_url: str | None = None,
        region: str | None = None,
        token: str | None = None,
        anon: bool = False,
        client_kwargs: dict | None = None,
        **storage_options,
    ):
        super().__init__(**storage_options)

        # Merge from fsspec.config.conf (populated by FSSPEC_S3_* env vars)
        s3_conf = conf.get("s3", {})
        key = key or s3_conf.get("key")
        secret = secret or s3_conf.get("secret")
        endpoint_url = endpoint_url or s3_conf.get("endpoint_url")
        region = region or s3_conf.get("region")
        token = token or s3_conf.get("token")

        # Also honour client_kwargs.endpoint_url (s3fs convention)
        if client_kwargs and not endpoint_url:
            endpoint_url = client_kwargs.get("endpoint_url")

        self._rust = RustS3Fs(
            bucket=bucket,
            key=key,
            secret=secret,
            endpoint_url=endpoint_url,
            region=region,
            token=token,
            anon=anon,
        )
        self.bucket = bucket

    # Core primitives — delegated to Rust

    def ls(self, path: str, detail: bool = True, **kwargs):
        return self._rust.ls(path, detail=detail)

    def info(self, path: str, **kwargs):
        return self._rust.info(path)

    def _open(
        self,
        path: str,
        mode: str = "rb",
        block_size=None,
        autocommit: bool = True,
        cache_options=None,
        **kwargs,
    ):
        """Return a Rust-backed S3 file object."""
        return S3File(self, path, mode=mode)

    def mkdir(self, path: str, create_parents: bool = True, **kwargs):
        return self._rust.mkdir(path, create_parents=create_parents)

    def rmdir(self, path: str):
        return self._rust.rmdir(path)

    def rm_file(self, path: str):
        return self._rust.rm_file(path)

    def rm(self, path, recursive: bool = False, maxdepth=None):
        if isinstance(path, str):
            return self._rust.rm_file(path)
        for p in path:
            self._rust.rm_file(p)

    def cp_file(self, path1: str, path2: str, **kwargs):
        return self._rust.cp_file(path1, path2)

    # Higher-level helpers — delegated to Rust for speed

    def exists(self, path: str, **kwargs) -> bool:
        return self._rust.exists(path)

    def isdir(self, path: str) -> bool:
        return self._rust.isdir(path)

    def isfile(self, path: str) -> bool:
        return self._rust.isfile(path)

    def size(self, path: str) -> int:
        return self._rust.size(path)

    def cat_file(self, path: str, start=None, end=None, **kwargs) -> bytes:
        return bytes(self._rust.cat_file(path, start=start, end=end))

    def pipe_file(self, path: str, value: bytes, **kwargs):
        return self._rust.pipe_file(path, value)

    def head(self, path: str, size: int = 1024) -> bytes:
        return bytes(self._rust.head(path, size))

    def tail(self, path: str, size: int = 1024) -> bytes:
        return bytes(self._rust.tail(path, size))

    def walk(self, path: str, maxdepth=None, topdown: bool = True, **kwargs):
        entries = self._rust.walk(path, max_depth=maxdepth, topdown=topdown)
        yield from entries

    def find(self, path: str, maxdepth=None, withdirs: bool = False, **kwargs):
        return self._rust.find(path, max_depth=maxdepth, with_dirs=withdirs)

    def read_text(self, path: str, encoding=None, errors=None, newline=None, **kwargs) -> str:
        return self._rust.read_text(path)

    def __repr__(self) -> str:
        return f"S3FileSystem(bucket='{self.bucket}')"


class S3File(AbstractBufferedFile):
    """File-like wrapper delegating to the Rust ``RustS3File``."""

    def __init__(self, fs: S3FileSystem, path: str, mode: str = "rb", **kwargs):
        self._rust_file = fs._rust.open(path, mode)
        self.path = path
        self.mode = mode
        self.fs = fs
        # Minimal attributes — don't call super().__init__()
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
        return self._rust_file.closed

    def readable(self) -> bool:
        return "r" in self.mode

    def writable(self) -> bool:
        return "w" in self.mode or "a" in self.mode

    def seekable(self) -> bool:
        return True

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()

    def __repr__(self) -> str:
        state = "closed" if self.closed else "open"
        return f"S3File('{self.path}', {state})"
