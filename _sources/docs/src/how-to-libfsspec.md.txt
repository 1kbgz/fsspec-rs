# How to share filesystems through libfsspec

This guide shows how to build against `libfsspec`, open a URL through its registry,
and add a backend that every other consumer in the process can open. Each step has a
tab for Rust, C, C++, and Python. For every function and type, see the
[C API reference](c-api.md).

## Build against libfsspec

`libfsspec` and its header ship in the `fsspec-rs` wheel. Install it into the
environment you build in:

```bash
pip install fsspec-rs
```

`````{tab-set}
---
sync-group: lang
---
````{tab-item} Rust
:sync: rust

A Python extension written in Rust depends on `fsspec_sys` for the C API and on
`fsspec_rs` for the traits:

```toml
[dependencies]
fsspec_rs = "0.2"
fsspec_sys = "0.2"
```

Declare the wheel's library in `pyproject.toml` and link the extension to it. This needs a hatch-rs
release with `python-package` support
([hatch-rs#95](https://github.com/python-project-templates/hatch-rs/pull/95)), and
`fsspec-rs` must be a build dependency:

```toml
[build-system]
requires = ["hatchling", "hatch-rs", "fsspec-rs"]

[[tool.hatch.build.hooks.hatch-rs.artifacts]]
name = "fsspec"
python-package = "fsspec_rs"

[[tool.hatch.build.hooks.hatch-rs.artifacts]]
name = "my_extension"
destination = "my_package/{python_extension_name}"
links = ["fsspec"]
```

hatch-rs adds an rpath to `../fsspec_rs/lib`, so the extension finds the library when
both wheels are installed in the same `site-packages`.

A standalone Rust program that does not share a process with other extensions does not
need `libfsspec`; it uses `fsspec_rs` and its registry directly.
````

````{tab-item} C
:sync: c

```bash
PREFIX=$(python -c "import fsspec_rs; print(fsspec_rs.get_prefix())")
cc example.c -I"$PREFIX/include" -L"$PREFIX/lib" -lfsspec -Wl,-rpath,"$PREFIX/lib" -o example
```

```c
#include <fsspec/fsspec.h>
```
````

````{tab-item} C++
:sync: cpp

```cmake
find_package(Python COMPONENTS Interpreter REQUIRED)
execute_process(
  COMMAND ${Python_EXECUTABLE} -c "import fsspec_rs; print(fsspec_rs.get_prefix())"
  OUTPUT_VARIABLE FSSPEC_PREFIX
  OUTPUT_STRIP_TRAILING_WHITESPACE)
find_library(FSSPEC_LIBRARY fsspec PATHS ${FSSPEC_PREFIX}/lib NO_DEFAULT_PATH REQUIRED)

add_executable(example example.cpp)
target_include_directories(example PRIVATE ${FSSPEC_PREFIX}/include)
target_link_libraries(example PRIVATE ${FSSPEC_LIBRARY})
set_target_properties(example PROPERTIES BUILD_RPATH ${FSSPEC_PREFIX}/lib)
```

The header has `extern "C"` guards, so include it directly:

```cpp
#include <fsspec/fsspec.h>
```
````

````{tab-item} Python
:sync: python

The installed extension already uses `libfsspec`. Check it:

```python
import fsspec_rs

assert fsspec_rs.fsspec_rs.linked_to_libfsspec()
```

`linked_to_libfsspec()` is `False` in Pyodide builds, which compile the registry into
the extension.
````
`````

## Open a URL

`````{tab-set}
---
sync-group: lang
---
````{tab-item} Rust
:sync: rust

```rust
use fsspec_rs::{FileSystem, FsOptions};

let (fs, path) = fsspec_sys::url_to_fs("file:///tmp/fsspec-example.txt", &FsOptions::new())?;
fs.pipe_file(&path, b"hello")?;
assert_eq!(fs.cat_file(&path, None, None)?, b"hello");
# Ok::<(), fsspec_rs::FsError>(())
```

`fs` is a `SharedFileSystem` over a `libfsspec` handle and supports every
`FileSystem` method.
````

````{tab-item} C
:sync: c

```c
#include <stdio.h>
#include <string.h>
#include <fsspec/fsspec.h>

int main(void) {
    const char *url = "file:///tmp/fsspec-example.txt";
    fsspec_fs *fs = NULL;
    fsspec_string path = {0};
    if (fsspec_url_to_fs(url, strlen(url), NULL, &fs, &path) != FSSPEC_OK) {
        fprintf(stderr, "%s\n", fsspec_last_error_message());
        return 1;
    }

    const char *text = "hello";
    fsspec_bytes data = {0};
    if (fsspec_fs_pipe_file(fs, path.data, path.len, (const uint8_t *)text, strlen(text)) != FSSPEC_OK ||
        fsspec_fs_cat_file(fs, path.data, path.len, NULL, NULL, &data) != FSSPEC_OK) {
        fprintf(stderr, "%s\n", fsspec_last_error_message());
    } else {
        printf("%.*s\n", (int)data.len, (const char *)data.data);
        fsspec_bytes_free(data);
    }

    fsspec_string_free(path);
    fsspec_fs_release(fs);
    return 0;
}
```
````

````{tab-item} C++
:sync: cpp

Wrap handles in `std::unique_ptr` and turn failing statuses into exceptions:

```cpp
#include <fsspec/fsspec.h>

#include <iostream>
#include <memory>
#include <stdexcept>
#include <string>
#include <string_view>

struct FsRelease {
    void operator()(fsspec_fs *fs) const { fsspec_fs_release(fs); }
};
using Fs = std::unique_ptr<fsspec_fs, FsRelease>;

void check(fsspec_status status) {
    if (status != FSSPEC_OK) {
        const char *message = fsspec_last_error_message();
        throw std::runtime_error(message ? message : "fsspec error");
    }
}

int main() {
    std::string_view url = "file:///tmp/fsspec-example.txt";
    fsspec_fs *raw = nullptr;
    fsspec_string resolved{};
    check(fsspec_url_to_fs(url.data(), url.size(), nullptr, &raw, &resolved));
    Fs fs(raw);
    std::string path(resolved.data, resolved.len);
    fsspec_string_free(resolved);

    std::string_view text = "hello";
    check(fsspec_fs_pipe_file(fs.get(), path.data(), path.size(),
                              reinterpret_cast<const uint8_t *>(text.data()), text.size()));
    fsspec_bytes data{};
    check(fsspec_fs_cat_file(fs.get(), path.data(), path.size(), nullptr, nullptr, &data));
    std::cout << std::string(reinterpret_cast<const char *>(data.data), data.len) << "\n";
    fsspec_bytes_free(data);
}
```
````

````{tab-item} Python
:sync: python

Python code keeps using fsspec. The `-rs` protocols reach `libfsspec` through the
extension:

```python
import fsspec
import fsspec_rs

fs, path = fsspec.url_to_fs("file-rs:///tmp/fsspec-example.txt")
fs.pipe_file(path, b"hello")
assert fs.cat_file(path) == b"hello"
```
````
`````

Options are strings on every side. Pass them as `FsOptions` in Rust, an
`fsspec_options` map in C and C++, or keyword arguments in Python. The accepted keys
for each built-in protocol are listed under
"Filesystem registry" in the [Rust reference](rust.md).

## Register a backend

A backend registered in `libfsspec` can be opened by every consumer in the process,
whatever language it is written in. Register it once, when your library loads.

`````{tab-set}
---
sync-group: lang
---
````{tab-item} Rust
:sync: rust

Register a factory from the extension's module initializer. Any closure from a path
and options to a `SharedFileSystem` is a factory:

```rust
use std::sync::Arc;

use fsspec_rs::{FsOptions, FsResult, SharedFileSystem};
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

#[pymodule]
fn my_extension(m: &Bound<'_, PyModule>) -> PyResult<()> {
    fsspec_sys::register_filesystem(
        "example",
        |_path: &str, options: &FsOptions| -> FsResult<SharedFileSystem> {
            Ok(Arc::new(ExampleFs::new(options)?))
        },
    )
    .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
    Ok(())
}
```

`fsspec_sys` exports `ExampleFs` through the vtables, so C and C++ callers see every
`FileSystem` method it implements.
````

````{tab-item} C
:sync: c

Fill a vtable with the operations the backend supports; null entries return
`FSSPEC_NOT_SUPPORTED`. Values returned through callbacks are freed by `libfsspec`, so
allocate them with `malloc` (`HeapAlloc(GetProcessHeap(), ...)` on Windows).

```c
#include <stdlib.h>
#include <string.h>
#include <fsspec/fsspec.h>

static fsspec_status example_info(void *data, const char *path, size_t path_len,
                                  fsspec_file_info *out) {
    (void)data;
    memset(out, 0, sizeof *out);
    out->name.data = malloc(path_len + 1);
    memcpy(out->name.data, path, path_len);
    out->name.data[path_len] = '\0';
    out->name.len = path_len;
    out->file_type = FSSPEC_FILE_TYPE_FILE;
    out->created_ns = -1;
    out->modified_ns = -1;
    return FSSPEC_OK;
}

static fsspec_status example_create(void *data, const char *path, size_t path_len,
                                    const fsspec_options *options, fsspec_fs **out) {
    (void)data, (void)path, (void)path_len, (void)options;
    fsspec_fs_vtable vtable = {0};
    vtable.struct_size = sizeof vtable;
    vtable.abi_version = FSSPEC_ABI_VERSION;
    vtable.protocol = "example";
    vtable.info = example_info;
    *out = fsspec_fs_from_vtable(&vtable, NULL);
    return *out ? FSSPEC_OK : FSSPEC_INVALID_ARGUMENT;
}

fsspec_status register_example(void) {
    fsspec_fs_factory factory = {0};
    factory.struct_size = sizeof factory;
    factory.abi_version = FSSPEC_ABI_VERSION;
    factory.create = example_create;
    return fsspec_register_filesystem("example", strlen("example"), &factory);
}
```

To keep state, pass a pointer as `data` and free it in `release`.
````

````{tab-item} C++
:sync: cpp

Captureless lambdas convert to the vtable's function pointers. Keep state in an
object passed as `data` and delete it in `release`:

```cpp
#include <fsspec/fsspec.h>

#include <cstdlib>
#include <cstring>
#include <string_view>

struct ExampleFs {
    // backend state
};

fsspec_status register_example() {
    fsspec_fs_factory factory{};
    factory.struct_size = sizeof factory;
    factory.abi_version = FSSPEC_ABI_VERSION;
    factory.create = [](void *, const char *, size_t, const fsspec_options *,
                        fsspec_fs **out) -> fsspec_status {
        fsspec_fs_vtable vtable{};
        vtable.struct_size = sizeof vtable;
        vtable.abi_version = FSSPEC_ABI_VERSION;
        vtable.protocol = "example";
        vtable.info = [](void *, const char *path, size_t path_len,
                         fsspec_file_info *out) -> fsspec_status {
            std::memset(out, 0, sizeof *out);
            out->name.data = static_cast<char *>(std::malloc(path_len + 1));
            std::memcpy(out->name.data, path, path_len);
            out->name.data[path_len] = '\0';
            out->name.len = path_len;
            out->file_type = FSSPEC_FILE_TYPE_FILE;
            out->created_ns = -1;
            out->modified_ns = -1;
            return FSSPEC_OK;
        };
        vtable.release = [](void *data) { delete static_cast<ExampleFs *>(data); };
        *out = fsspec_fs_from_vtable(&vtable, new ExampleFs{});
        return *out ? FSSPEC_OK : FSSPEC_INVALID_ARGUMENT;
    };
    std::string_view protocol = "example";
    return fsspec_register_filesystem(protocol.data(), protocol.size(), &factory);
}
```

Callbacks must not let exceptions escape. Catch them, call `fsspec_set_last_error`,
and return a failing status.
````

````{tab-item} Python
:sync: python

Python code registers a native backend by importing the package whose extension
registers it, then checks the shared registry:

```python
import fsspec_rs
import my_package  # its extension registers "example" when imported

assert "example" in fsspec_rs.fsspec_rs.protocols()
```

Import `fsspec_rs` before the package so that, on Windows, its `lib/` directory is on
the DLL search path.

A backend written in Python registers with `fsspec.register_implementation` instead.
It is visible to fsspec but not to `libfsspec`, so native callers cannot open it.
````
`````

## Ship a wheel that links libfsspec

Wheel repair tools copy external libraries into the wheel by default, which would give
your package its own copy of `libfsspec` and its own registry. Exclude it:

```bash
auditwheel repair --exclude 'libfsspec.so*' dist/*.whl
delocate-wheel --exclude libfsspec dist/*.whl
delvewheel repair --exclude fsspec.dll dist/*.whl
```

Require `fsspec-rs` at run time, at least the version you built against:

```toml
[project]
dependencies = ["fsspec-rs>=0.2"]
```

To fail early on a mismatched library, compare `fsspec_abi_version()` with
`FSSPEC_ABI_VERSION` (`fsspec_sys::abi_version()` in Rust) when your library loads.
