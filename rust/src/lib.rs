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
pub mod types;

mod async_fs;
mod fs;
mod local;
mod s3;

#[cfg(test)]
mod cache_tests;
#[cfg(test)]
mod local_tests;
#[cfg(test)]
mod s3_tests;
#[cfg(test)]
mod tests;

pub use async_fs::AsyncFileSystem;
pub use buffered::BufferedFile;
pub use caching::CacheType;
pub use error::{FsError, FsResult};
pub use file::FsFile;
pub use fs::FileSystem;
pub use local::{LocalFile, LocalFs};
pub use s3::{S3Config, S3File, S3Fs};
pub use types::{DuResult, FileInfo, FileType, OpenMode, OpenOptions, WalkEntry};
