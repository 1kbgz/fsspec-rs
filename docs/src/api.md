# API reference

```{eval-rst}
.. currentmodule:: fsspec_rs

.. autosummary::
   :nosignatures:

   LocalFileSystem
   LocalFile
   S3FileSystem
   S3File
   FileInfo
   FileType
```

## Filesystems

```{eval-rst}
.. autoclass:: fsspec_rs.LocalFileSystem
   :members:
   :undoc-members:
   :member-order: bysource
```

```{eval-rst}
.. autoclass:: fsspec_rs.S3FileSystem
   :members:
   :undoc-members:
   :member-order: bysource
```

## Object I/O and conditions

Local, memory and S3 filesystems expose `read(path, start=None, end=None, condition=None)`, `write(path, data, condition=None)` and `io_capabilities()`.
`read` returns a frozen `ReadResult` with `.data` and `.validator`; `write`
returns a frozen `WriteResult` with the committed object's `.validator`.
A backend without validators returns `None`.

```python
from fsspec_rs import ConflictError, Match, Missing

created = fs.write("manifest.json", b"first", condition=Missing())
current = fs.read("manifest.json")
if current.validator is not None:
    try:
        updated = fs.write("manifest.json", b"next", condition=Match(current.validator))
    except ConflictError:
        # Another writer changed or deleted the object. Read again before retrying.
        pass
```

`Match` requires a `Validator` obtained from the same object's read or write
result. Validators are opaque equality tokens; retain both `.scope` and `.value`
unchanged. S3 supports Match for reads and writes. Local and memory support
Missing writes but do not provide validators or Match operations.

`cat_file(..., condition=...)` and `pipe_file(..., condition=...)` accept the same
conditions and keep their existing return values. Use `read` and `write` to obtain
validators. Missing is valid only for writes. Unknown condition types raise
`TypeError`; invalid conditions or validator scope raise `ValueError`;
unsupported conditions raise `NotImplementedError`. Conflicts raise
`ConflictError`, an `OSError` subclass. Other filesystem errors keep their usual
Python exception types. Passing `condition=` to open handles is unsupported.

See [Rust object I/O](rust.md) for capability flags,
range consistency and local exclusive-creation semantics.

## Range reads

`read_ranges(path, ranges, *, condition=None, max_gap=65536, max_request_size=8388608, max_concurrency=8)` returns a frozen `RangeReadResult`.
Its `.data` is a tuple of bytes in requested order; `.validator` identifies the
shared S3 object version or is `None` when unavailable. `ranges` contains
`(start, end)` pairs with signed integers or `None`, as for `read` bounds.

```python
result = fs.read_ranges("events.parquet", [(0, 16), (32, 48), (-8, None)])
first, second, footer = result.data
```

Nearby ranges are merged and oversized requests split. S3 fetches merged ranges
concurrently against one validator. Local and memory use sequential reads without
a snapshot guarantee. Match is supported by S3; Missing is invalid. Conditions and
errors follow the typed read API. A failed operation returns no partial result.

`cat_ranges(paths, starts, ends, max_gap=None, on_error="return", *, condition=None, max_request_size=8388608, max_concurrency=8)` adapts fsspec's list interface.
`paths` is a list; scalar start/end bounds broadcast to its length. List lengths
must match. Requests for the same normalized path are grouped and coalesced, then
restored to input order. Different path groups execute sequentially. A condition
cannot span multiple unique paths. An empty path list returns an empty list without I/O. `max_gap=None` selects the 65,536-byte default.

With `on_error="return"`, a failed path group places its exception at each of that
group's output positions; `"raise"` propagates the error. Invalid argument shapes,
options and condition types raise before I/O. Unknown keyword arguments are rejected.
Use `read_ranges` to retain the validator with the bytes.

Compatibility with the inherited fsspec implementation:

- `cat_ranges` is an existing fsspec API; `read_ranges` is the typed single-object batch API added by fsspec-rs.
- Errors apply to a normalized path group. If a later request fails, previously fetched bytes for that path are discarded. With `on_error="return"`, other path groups still run. The inherited implementation handled each range independently.
- All offsets are validated before any path is read. Invalid offsets raise even with `on_error="return"`.
- S3 batches issue a metadata request and require an ETag when fetching bytes, even without `condition`. A missing ETag produces `NotImplementedError`; an object changed between requests can produce `ConflictError`. These errors follow `on_error`. Individual unconditional `cat_file` reads do not require an ETag or check consistency across calls.

See the [Rust reference](rust.md) for shared endpoint concurrency limits, empty
range behavior and memory bounds.

## Open files

```{eval-rst}
.. autoclass:: fsspec_rs.LocalFile
   :members:
   :undoc-members:
   :member-order: bysource
```

```{eval-rst}
.. autoclass:: fsspec_rs.S3File
   :members:
   :undoc-members:
   :member-order: bysource
```

## Metadata types

```{eval-rst}
.. autoclass:: fsspec_rs.FileInfo
   :members:
   :undoc-members:
```

```{eval-rst}
.. autoclass:: fsspec_rs.FileType
   :members:
   :undoc-members:
```
