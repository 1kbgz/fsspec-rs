"""fsspec-rs: fsspec-compatible backends with Rust acceleration."""

__version__ = "0.1.5"

# Re-export Rust types
# Register with fsspec so protocols are usable immediately after import
from fsspec.registry import register_implementation as _register

from fsspec_rs.fsspec_rs import (  # noqa: F401
    FileInfo,
    FileType,
    RustLocalFile,
    RustLocalFs,
    RustS3File,
    RustS3Fs,
)

# Re-export Python wrappers
from fsspec_rs.local import LocalFile, LocalFileSystem  # noqa: F401
from fsspec_rs.s3 import S3File, S3FileSystem  # noqa: F401

_register("file-rs", LocalFileSystem)
_register("local-rs", LocalFileSystem)
_register("s3-rs", S3FileSystem)
del _register
