"""fsspec-rs: fsspec-compatible backends with Rust acceleration."""

__version__ = "0.1.0"

# Re-export Rust types
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
