"""fsspec-rs: fsspec-compatible backends with Rust acceleration."""

__version__ = "0.1.8"

import os as _os
from pathlib import Path as _Path

_PREFIX = _Path(__file__).resolve().parent
if _os.name == "nt" and (_PREFIX / "lib").is_dir():
    # The extension links libfsspec from here; Windows does not search the extension's own directory.
    _DLL_DIRECTORY = _os.add_dll_directory(str(_PREFIX / "lib"))

# Re-export Rust types
# Register with fsspec so protocols are usable immediately after import
from fsspec.registry import register_implementation as _register

from fsspec_rs.fsspec_rs import (
    FileInfo,
    FileType,
    RustLocalFile,
    RustLocalFs,
    RustS3File,
    RustS3Fs,
)

# Re-export Python wrappers
from fsspec_rs.local import LocalFile, LocalFileSystem
from fsspec_rs.s3 import S3File, S3FileSystem

_register("file-rs", LocalFileSystem)
_register("local-rs", LocalFileSystem)
_register("s3-rs", S3FileSystem)
del _register


def get_prefix() -> str:
    """The installed package directory, usable as a CMake prefix for libfsspec."""
    return str(_PREFIX)


def get_include() -> str:
    """The directory holding ``fsspec/fsspec.h``."""
    return str(_Path(get_prefix()) / "include")


def get_library_dirs() -> list[str]:
    """Directories holding the bundled libfsspec shared library."""
    return [str(_Path(get_prefix()) / "lib")]


def get_libraries() -> list[str]:
    """Link names for libfsspec, for ``-l`` or ``target_link_libraries``."""
    return ["fsspec"]
