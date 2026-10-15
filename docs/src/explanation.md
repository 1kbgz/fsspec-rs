# Why fsspec-rs uses explicit protocols and shared traits

`fsspec-rs` has two integration boundaries: Python's fsspec interface and a reusable
Rust filesystem trait. Keeping both boundaries explains the package's protocol names,
backend structure, and separate Python bridge crate.

## Explicit protocols preserve compatibility

fsspec already assigns `file`, `local`, and `s3` to established Python implementations.
Replacing those registrations globally would make importing `fsspec-rs` change unrelated
application behavior. The `file-rs`, `local-rs`, and `s3-rs` names make backend selection
visible in configuration and allow both implementations to coexist.

The classes still inherit from `fsspec.spec.AbstractFileSystem`. Code written against
fsspec operations can therefore select a Rust backend without adopting a separate Python
API.

## Default methods keep native backends small

The Rust `FileSystem` trait follows fsspec's template-method shape. A backend implements
primitive operations such as listing, metadata, open, copy, and removal. Default trait
methods build higher-level operations such as walking, recursive removal, and local
transfers from those primitives.

This design centralizes shared path and traversal behavior while leaving storage-specific
I/O in `LocalFs` and `S3Fs`. The trade-off is that a default method may not exploit every
backend-specific optimization; a backend can override behavior when that distinction
matters.

## The Python bridge is deliberately separate

Native Rust backends are not the only filesystems in the fsspec ecosystem. The
`fsspec_rs_bridge` crate adapts an installed Python filesystem to the same Rust trait, so a
Rust component running inside Python can consume protocols that have no native Rust
implementation.

The bridge depends on PyO3 and a Python runtime, while the core `fsspec_rs` crate does not.
Separating them keeps standalone Rust consumers Python-free and makes the runtime cost of
crossing into Python explicit. Native backends remain preferable when they exist; the
bridge prioritizes ecosystem coverage.

## Synchronous semantics form the current common boundary

The shipped local and S3 backends implement the synchronous `FileSystem` trait. S3 uses an
internal Tokio runtime to present that interface even though object-store calls are
asynchronous underneath. This matches Python file objects and fsspec's common synchronous
surface, at the cost of hiding async scheduling from callers.

An experimental `AsyncFileSystem` trait records a future direction without changing the
synchronous contract. Its status and exact trait methods are documented in the
[Rust reference](rust.md).

## See also

- Follow the [local filesystem tutorial](tutorial.md).
- Apply the [filesystem recipes](how-to.md).
- Look up [filesystem behavior](filesystems.md) and the [Python API](api.md).
