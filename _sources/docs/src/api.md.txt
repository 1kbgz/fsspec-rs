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

## S3 configuration

`S3FileSystem` and `RustS3Fs` accept `client_options`, a dictionary with
`connect_timeout` and `request_timeout` in finite positive seconds, and
`max_retries` as a nonnegative integer. Unknown keys and invalid values are
rejected. Omitted settings retain the backend defaults: 5 seconds to connect,
30 seconds per request, and 10 retries. Typed writes always use zero retries;
the setting applies to the general client used by reads and legacy operations.
Request timeout is not an overall operation deadline.

For `S3FileSystem`, explicit credentials take precedence over
`fsspec.config.conf["s3"]`, then AWS environment credentials. Each source supplies
key, secret and optional token together; partial explicit credentials are errors.
`RustS3Fs` uses explicit credentials or AWS environment values directly.
`anon=True` ignores credentials. Region precedence is explicit, Python fsspec
configuration where applicable, `AWS_REGION`, then `AWS_DEFAULT_REGION`.
Custom endpoints are explicit; AWS endpoint environment variables are ignored.
Profiles, SSO and `credential_process` are not supported.

Instance caches retain construction-time environment defaults. Explicit endpoint,
credentials and client options distinguish configured instances. To select a
new credential context in Python, pass the new credentials explicitly; bypassing
fsspec's Python cache with `skip_instance_cache=True` also creates a new native
S3 client. `S3FileSystem` and `RustS3Fs` use the built-in S3 constructor, bypassing
protocol overrides and the shared C registry cache. General `url_to_fs` calls
continue to use that registry.

## S3 object-store adapter

`fsspec_rs.obspec.S3ObjectStore(fs, *, prefix="")` provides the synchronous
`Head`, `GetRange` and `GetRanges` protocols from obspec 0.1.0. It requires a
`fsspec_rs.S3FileSystem` and shares that filesystem's native S3 client, credentials,
runtime and range-request concurrency limit. It retains its own handle, so it
remains usable after the filesystem is dropped. Construction performs no I/O.
The adapter is included in both direct builds and wheels linked to `libfsspec`;
`obspec` is not a runtime dependency.

```python
from fsspec_rs import S3FileSystem
from fsspec_rs.obspec import S3ObjectStore

fs = S3FileSystem(bucket="measurements", region="us-east-1")
store = S3ObjectStore(fs, prefix="daily")
meta = store.head("data.parquet")
footer = store.get_range("data.parquet", start=meta["size"] - 8, length=8)
buffers = store.get_ranges("data.parquet", starts=[0, 4], ends=[4, 12])
```

Keys are nonempty paths relative to the bucket or optional prefix. URLs, leading
or trailing slashes, empty segments, `.` and `..` segments, backslashes and control
characters are rejected. A bucket name inside a key is literal. Spaces, Unicode
and percent signs are literal object-key characters; the adapter does not decode
URL escapes. The filesystem currently escapes some characters in its logical
paths before storage, while this native view uses the stored S3 key directly. Prefix follows the same rules, but may be empty.

| Method                                                 | Result                                                                                |
| ------------------------------------------------------ | ------------------------------------------------------------------------------------- |
| `head(path)`                                           | Metadata dictionary: `path`, `size`, UTC `last_modified` datetime, `e_tag`, `version` |
| `get_range(path, *, start, end=None, length=None)`     | `bytes` for one range                                                                 |
| `get_ranges(path, *, starts, ends=None, lengths=None)` | List of `bytes`, in input order including duplicates                                  |

Metadata uses native HEAD and never infers directories. `path` is relative to the
adapter's prefix; ETag and version are strings or `None`. Datetimes have Python's
microsecond precision. Modification times before the Unix epoch are unsupported
and raise `NotSupportedError`. Each metadata call makes a fresh request. Filesystem
`fs.head(path)` retains its existing meaning: read the first bytes of a file.

Ranges require exactly one of end/length (or ends/lengths), nonnegative integer
bounds through `2**63 - 1`, equal vector lengths, and a nonempty interval. Boolean
bounds are rejected; integer-like bounds implementing `__index__` are accepted.
Starts at or past EOF fail; ends past EOF are clipped. Empty
vectors return `[]` without I/O. Reads use the native coalescing defaults and
require an ETag; every GET is checked against the metadata request's validator.
No cached metadata, async methods, streaming, listing or writes are exposed.

Exceptions named `NotFoundError`, `PermissionDeniedError`, `PreconditionError`,
`NotSupportedError` and `InvalidPathError` support
`obspec.exceptions.map_exception`. Native errors retain `.kind` and `.retry_safe`,
with the original exception as their cause when mapped. Invalid range arguments
raise `ValueError` or `TypeError`; other backend errors retain the normal operation
error contract. Range request sizes and concurrency are bounded, but the returned
buffers occupy memory proportional to the requested output.

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

### Operation errors

Errors returned by typed `read`, `write` and `read_ranges` expose `.kind` and
`.retry_safe`. The kind is one of `not_found`, `already_exists`, `conflict`,
`gap`, `permission_denied`, `unavailable`, `timeout`, `corrupt`, `invalid`,
`cancelled`, `unknown_outcome`, or the compatibility categories `not_a_directory`,
`is_a_directory`, `io`, `not_supported` and `other`.

Standard Python exceptions remain available: `FileNotFoundError`,
`FileExistsError`, `PermissionError`, `TimeoutError`, `ValueError` and
`NotImplementedError`. Additional `OSError` subclasses are `ConflictError`,
`GapError`, `UnavailableError`, `CorruptError`, `CancelledError` and
`UnknownOutcomeError`. `GapError` is available for backends with positioned
writes; byte writes do not produce it themselves. Argument errors raised before
backend dispatch do not necessarily have operation metadata.

`retry_safe=True` means repeating the operation is safe. False includes cases
where safety cannot be established. Transient S3 read failures are safe to retry;
an uncertain write is not. Legacy file-handle operations retain their existing
error contract.

### Uncertain creates

S3 typed writes send one PUT attempt. Following an uncertain response to
`write(..., condition=Missing())`, the backend reads the object and compares its
SHA-256 checksum with the supplied bytes. Matching content acknowledges success;
different content returns `ConflictError`. An absent object or failed verification
returns `UnknownOutcomeError` with `retry_safe=False`. Local exclusive writes
use the same content check after an I/O failure. An ETag is never treated as a
content checksum.

Recovered success acknowledges matching content for immutable publication; it
does not establish ownership of a lock or prove which writer created the object.
A definitive precondition failure remains a conflict, including when existing
content matches. Ordinary overwrites and Match writes with uncertain outcomes
return `UnknownOutcomeError` without content-based recovery.

## File write lifecycle

Native Python handles and the filesystem wrappers publish buffered writes on
explicit `close()` or clean context-manager exit. `discard()` closes the handle
without publishing pending data. Exceptional exits and finalizers discard.
Uncompressed text handles returned by `fs.open` follow the same rules.

Direct local append writes are immediately visible and cannot be rolled back.
Explicit flushes can pass text to the underlying handle. External wrappers,
including compression and `fsspec.open`'s `OpenFile`, control their own close
behavior; their exception handling is outside this lifecycle contract.

S3 exclusive handles still buffer the complete object. Ordinary S3 writers use
multipart uploads for large objects; abandoned uploads are aborted on a
best-effort basis. A process crash can leave multipart uploads requiring the
store's retention policy.

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
