//! # fsspec_rs
//!
//! A Rust-native filesystem abstraction framework inspired by Python's
//! [filesystem_spec (fsspec)](https://filesystem-spec.readthedocs.io/).
//!
//! This crate provides traits and types for building filesystem backends
//! in pure Rust, with the same "implement a few primitives, get everything
//! else for free" design pattern as fsspec.

pub mod error;
pub mod file;
pub mod types;

mod fs;

#[cfg(test)]
mod tests;

pub use error::{FsError, FsResult};
pub use file::FsFile;
pub use fs::FileSystem;
pub use types::{DuResult, FileInfo, FileType, OpenMode, OpenOptions, WalkEntry};
