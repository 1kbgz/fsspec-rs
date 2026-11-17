"""Reject conditions on helpers that do not define conditional I/O semantics."""

from collections.abc import Callable
from functools import wraps
from typing import ParamSpec, TypeVar

_P = ParamSpec("_P")
_R = TypeVar("_R")
_T = TypeVar("_T", bound=type)


def _reject_condition(method: Callable[_P, _R]) -> Callable[_P, _R]:
    @wraps(method)
    def guarded(*args: _P.args, **kwargs: _P.kwargs) -> _R:
        if "condition" in kwargs:
            raise NotImplementedError("conditions are supported only by read, write, cat_file and pipe_file")
        return method(*args, **kwargs)

    return guarded


def reject_unsupported_conditions(cls: _T) -> _T:
    # Include inherited helpers and aliases: several discard kwargs or catch errors
    # from their underlying operations, so rejection must happen at the entry point.
    for name in (
        "cat",
        "cat_ranges",
        "copy",
        "cp",
        "cp_file",
        "disk_usage",
        "download",
        "du",
        "exists",
        "expand_path",
        "find",
        "get",
        "get_file",
        "glob",
        "info",
        "lexists",
        "listdir",
        "ls",
        "makedir",
        "mkdir",
        "move",
        "mv",
        "open",
        "pipe",
        "put",
        "put_file",
        "read_bytes",
        "read_text",
        "rename",
        "sign",
        "stat",
        "touch",
        "upload",
        "walk",
        "write_bytes",
        "write_text",
    ):
        setattr(cls, name, _reject_condition(getattr(cls, name)))
    return cls
