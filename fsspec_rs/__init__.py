"""fsspec-rs: fsspec-compatible backends with Rust acceleration."""

__version__ = "0.1.8"

from pathlib import Path as _Path

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
    return str(_Path(__file__).resolve().parent)


def get_include() -> str:
    """The directory holding ``fsspec/fsspec.h``."""
    return str(_Path(get_prefix()) / "include")


def get_library_dirs() -> list[str]:
    """Directories holding the bundled libfsspec shared library."""
    return [str(_Path(get_prefix()) / "lib")]


def get_libraries() -> list[str]:
    """Link names for libfsspec, for ``-l`` or ``target_link_libraries``."""
    return ["fsspec"]
