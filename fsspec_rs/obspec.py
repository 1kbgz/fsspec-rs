"""Read-only obspec protocols over an existing Rust S3 filesystem.

No obspec runtime dependency is required. These exception names support
``obspec.exceptions.map_exception``. Only Head, GetRange and GetRanges are exposed.
"""

from __future__ import annotations

from collections.abc import Sequence
from datetime import UTC, datetime, timedelta
from operator import index
from typing import TypedDict

from fsspec_rs.s3 import S3FileSystem


class ObjectMeta(TypedDict):
    path: str
    size: int
    last_modified: datetime
    e_tag: str | None
    version: str | None


class NotFoundError(FileNotFoundError):
    """No object exists at the requested key."""


class PermissionDeniedError(PermissionError):
    """The store denied access."""


class PreconditionError(OSError):
    """The object changed during a ranged read."""


class NotSupportedError(NotImplementedError):
    """The backend cannot provide the requested guarantee."""


class InvalidPathError(ValueError):
    """The key is not a canonical relative object path."""


_ERRORS = {
    "not_found": NotFoundError,
    "permission_denied": PermissionDeniedError,
    "conflict": PreconditionError,
    "not_supported": NotSupportedError,
}
_MAX_OFFSET = (1 << 63) - 1
_EPOCH = datetime(1970, 1, 1, tzinfo=UTC)


def _path(value: str, *, allow_empty: bool = False) -> str:
    if not isinstance(value, str):
        raise TypeError("object keys must be strings")
    if allow_empty and value == "":
        return value
    if not value or any(part in ("", ".", "..") for part in value.split("/")) or "\\" in value or "://" in value:
        raise InvalidPathError("expected a nonempty relative object key")
    if any(ord(char) < 32 or ord(char) == 127 for char in value):
        raise InvalidPathError("object keys cannot contain control characters")
    try:
        value.encode("utf-8")
    except UnicodeEncodeError as error:
        raise InvalidPathError("object keys must be valid UTF-8") from error
    return value


def _offset(value: int) -> int:
    if isinstance(value, bool):
        raise TypeError("range bounds must be integers")
    value = index(value)
    if not 0 <= value <= _MAX_OFFSET:
        raise ValueError("range bounds must fit a nonnegative signed 64-bit integer")
    return value


class S3ObjectStore:
    """Read-only object view sharing ``fs``'s S3 client, credentials and runtime.

    Paths are relative to ``prefix`` within the configured bucket. No filesystem
    URL or bucket stripping occurs. Construction performs no I/O. Methods are
    synchronous; this class does not advertise async, streaming or write protocols.
    """

    def __init__(self, fs: S3FileSystem, *, prefix: str = ""):
        if not isinstance(fs, S3FileSystem):
            raise TypeError("S3ObjectStore requires a Rust S3FileSystem")
        self._prefix = _path(prefix, allow_empty=True)
        self._rust = fs._rust.object_store()

    def _key(self, path: str) -> str:
        path = _path(path)
        return f"{self._prefix}/{path}" if self._prefix else path

    def _call(self, method, *args):
        try:
            return method(*args)
        except Exception as error:
            cls = _ERRORS.get(getattr(error, "kind", None))
            if cls is None:
                raise
            mapped = cls(str(error))
            mapped.__dict__.update(kind=getattr(error, "kind", None), retry_safe=getattr(error, "retry_safe", False))
            raise mapped from error

    def head(self, path: str) -> ObjectMeta:
        """Return native object metadata; missing prefixes never trigger a listing."""
        size, seconds, nanos, etag, version = self._call(self._rust.head, self._key(path))
        return {
            "path": path,
            "size": size,
            "last_modified": _EPOCH + timedelta(seconds=seconds, microseconds=nanos // 1000),
            "e_tag": etag,
            "version": version,
        }

    def get_range(self, path: str, *, start: int, end: int | None = None, length: int | None = None) -> bytes:
        """Read a nonempty range. End is exclusive; oversized ends are truncated."""
        return self.get_ranges(path, starts=[start], ends=None if end is None else [end], lengths=None if length is None else [length])[0]

    def get_ranges(
        self,
        path: str,
        *,
        starts: Sequence[int],
        ends: Sequence[int] | None = None,
        lengths: Sequence[int] | None = None,
    ) -> list[bytes]:
        """Return buffers in input order with bounded, coalesced native reads.

        Exactly one of ends/lengths is required. Empty and EOF-starting ranges
        fail. Empty vectors return an empty list without contacting the store.
        """
        key = self._key(path)
        if (ends is None) == (lengths is None):
            raise ValueError("provide exactly one of ends or lengths")
        bounds = ends if ends is not None else lengths
        assert bounds is not None
        if len(starts) != len(bounds):
            raise ValueError("starts and ends/lengths must have equal lengths")
        ranges = []
        for start, bound in zip(starts, bounds, strict=True):
            start, bound = _offset(start), _offset(bound)
            end = bound if ends is not None else _offset(start + bound)
            if end <= start:
                raise ValueError("object ranges must be nonempty")
            ranges.append((start, end))
        return list(self._call(self._rust.read_ranges, key, ranges))
