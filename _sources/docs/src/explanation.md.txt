# Why fsspec-rs uses explicit protocols and shared traits

`fsspec-rs` provides Python's fsspec interface and a Rust filesystem trait. Python
applications can select a Rust-backed filesystem through fsspec, while Rust
applications can use the native backends without Python. A separate bridge crate
lets Rust callers use Python fsspec implementations through the same trait.

## Explicit protocols preserve compatibility

fsspec already assigns `file`, `local`, `memory`, and `s3` to established Python
implementations. Replacing those registrations globally would make importing `fsspec-rs`
change unrelated application behavior. The `file-rs`, `local-rs`, `memory-rs`, and `s3-rs`
names make backend selection explicit in configuration and allow both implementations to
coexist.

The classes still inherit from `fsspec.spec.AbstractFileSystem`. Code written against
fsspec operations can therefore select a Rust backend without switching to a different
API.

## Default methods keep native backends small

The Rust `FileSystem` trait follows fsspec's template-method pattern. A backend implements
primitive operations such as listing files, reading metadata, opening files, copying,
and removing files. Default trait methods build higher-level operations such as
walking, recursive removal, and local
transfers from those primitives.

This design centralizes shared path and traversal behavior while leaving storage-specific
I/O in `LocalFs` and `S3Fs`. Default methods may miss backend-specific optimizations,
so backends can override them where needed.

## The Python bridge is a separate crate

The `fsspec_rs_bridge` crate adapts an installed Python filesystem to the Rust trait, so a
Rust component running inside Python can use protocols that have no native Rust
implementation.

The bridge depends on PyO3 and a Python runtime, while the core `fsspec_rs` crate does not.
Keeping them in separate crates lets standalone Rust applications avoid the Python
dependency. Native backends avoid calls into Python; the bridge provides access to
Python backends when no native implementation is available.

## Backends share a synchronous interface

The local and S3 backends implement the synchronous `FileSystem` trait. S3 runs async
object-store calls through an internal Tokio runtime. Callers use a synchronous interface
that matches Python file objects and fsspec, but cannot control async scheduling.

The experimental `AsyncFileSystem` trait defines an async interface without changing the
synchronous one. Its status and methods are documented in the
[Rust reference](rust.md).

## One registry per process

A Rust crate compiled into a Python extension is private to that extension. When
`fsspec-rs`, `fsspec-db`, and `fsspec-kafka` each compile `fsspec_rs` in, a process loads
three copies of its code and three registries, so a backend registered by one extension
is invisible to the others. Rust has no stable ABI, so a `dyn FileSystem` cannot be passed
from one extension to another either.

`libfsspec` fixes both with a C ABI. The registry, its instance cache, and the
built-in backends are compiled into that one shared library, and every extension links
it dynamically. A filesystem is passed between extensions as an opaque handle with a table
of C function pointers, which every compiler and language supports.

The work is split across three crates so that only one of them has shared state:

- `fsspec_rs` has the traits and backends. Compiling the traits into several extensions
  is harmless; its own registry stays private to the binary that compiles it in.
- `fsspec_capi` builds `libfsspec` and is the only crate with the shared registry.
- `fsspec_sys` declares the C functions without implementing them. Extensions depend on
  it to reach `libfsspec`; it must never depend on `fsspec_capi`, or every extension
  would export its own copy of the C functions.

The C ABI costs a function-pointer call per operation and copies of returned strings and
buffers. Single-binary Rust programs avoid both by using `fsspec_rs` and its registry
directly. Pyodide builds do the same, because Emscripten wheels cannot share libraries
yet.

The same library serves C and C++ programs. They link `libfsspec` and see the backends
that Python extensions register.

## See also

- Follow the [local filesystem tutorial](tutorial.md).
- Use the [filesystem how-to guide](how-to.md) for S3 and caching.
- Look up [filesystem behavior](filesystems.md) and the [Python API](api.md).
- Share filesystems across extensions and languages with the
  [libfsspec how-to](how-to-libfsspec.md) and the [C API reference](c-api.md).
