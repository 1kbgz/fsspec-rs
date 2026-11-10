//! C ABI for fsspec-rs: `libfsspec`.
//!
//! One copy of the registry and the built-in backends per process, reached
//! through the functions declared in `include/fsspec.h`. Every entry point
//! returns an [`fsspec_status`]; the message for the last failure on the
//! calling thread comes from `fsspec_last_error_message`. No panic crosses
//! the boundary: a panic is reported as `FSSPEC_OTHER`.
//!
//! Strings are passed in as UTF-8 pointer plus length. Memory handed out by
//! this library (`fsspec_bytes`, `fsspec_string`, info lists) is freed with
//! the matching `fsspec_*_free` function, never with the caller's allocator.

#![allow(clippy::missing_safety_doc)]
#![allow(non_camel_case_types)]

mod file;
mod filesystem;
mod memory;
mod options;
mod registry;
mod status;

#[cfg(test)]
mod tests;

pub use file::*;
pub use filesystem::*;
pub use memory::*;
pub use options::*;
pub use registry::*;
pub use status::*;
