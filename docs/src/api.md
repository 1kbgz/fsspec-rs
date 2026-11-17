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
