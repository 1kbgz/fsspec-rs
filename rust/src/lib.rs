//! # fsspec_rs
//!
//! A Rust-native filesystem abstraction framework inspired by Python's
//! [filesystem_spec (fsspec)](https://filesystem-spec.readthedocs.io/).
//!
//! This crate provides traits and types for building filesystem backends
//! in pure Rust, with the same "implement a few primitives, get everything
//! else for free" design pattern as fsspec.

pub mod buffered;
pub mod caching;
pub mod error;
pub mod file;
#[cfg(feature = "object-store")]
pub mod object_store_bridge;
pub mod registry;
pub mod types;

mod async_fs;
mod fs;
mod local;
mod memory;
mod ranges;
#[cfg(feature = "s3")]
mod s3;
#[cfg(feature = "s3")]
mod s3_http;

#[cfg(test)]
mod cache_tests;
#[cfg(test)]
mod local_tests;
#[cfg(test)]
mod memory_tests;
#[cfg(test)]
mod registry_tests;
#[cfg(all(test, feature = "s3"))]
mod s3_range_tests;
#[cfg(all(test, feature = "s3"))]
mod s3_tests;
#[cfg(test)]
mod tests;

pub use async_fs::AsyncFileSystem;
pub use buffered::BufferedFile;
pub use caching::CacheType;
pub use error::{ErrorKind, FsError, FsResult, OperationError};
pub use file::FsFile;
pub use fs::{
    cat_file_default, pipe_file_default, read_default, read_ranges_default, strip_protocol_default,
    unstrip_protocol_default, write_default, FileSystem,
};
pub use local::{LocalFile, LocalFs};
pub use memory::{MemoryFile, MemoryFs};
pub use registry::{
    register_filesystem, set_fallback_filesystem, set_registry_hooks, try_register_filesystem,
    url_to_fs, FilesystemFactory, FilesystemRegistry, FsOptions, RegistryHooks, SharedFileSystem,
    DEFAULT_FILESYSTEMS,
};
#[cfg(feature = "s3")]
pub use s3::{S3ClientOptions, S3Config, S3File, S3Fs, S3ObjectReader};
pub use types::{
    ByteRange, Condition, DuResult, FileInfo, FileType, IoCapabilities, OpenMode, OpenOptions,
    RangeReadOptions, RangeReadResult, ReadOptions, ReadResult, Validator, WalkEntry, WriteOptions,
    WriteResult,
};
