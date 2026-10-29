# Read and write a local file with fsspec-rs

Create a file with the Rust-backed local filesystem, then read it through an
fsspec URL and inspect its metadata.

## Before you start

Install `fsspec-rs` in a Python 3.10 or newer environment:

```console
python -m pip install fsspec-rs
```

## Create the script

Create `local_demo.py` with these imports and a temporary directory:

```python
from pathlib import Path
from tempfile import TemporaryDirectory

import fsspec
import fsspec_rs

with TemporaryDirectory() as directory:
    path = Path(directory, "greeting.txt")
```

## Write through the Rust-backed filesystem

Add this code inside the `with` block:

```python
    fs = fsspec.filesystem("file-rs", auto_mkdir=True)
    fs.pipe_file(str(path), b"hello from Rust\n")
    print(fs.ls(directory, detail=False))
```

Run the script:

```console
python local_demo.py
```

The listing contains the file you created:

```text
['.../greeting.txt']
```

## Read through an fsspec URL

Add this code after the listing, still inside the `with` block:

```python
    with fsspec.open(f"file-rs://{path}", "rb") as file:
        print(file.read().decode().strip())

    info = fs.info(str(path))
    print(info["type"], info["size"])
```

Run the script again. The final lines are:

```text
hello from Rust
file 16
```

Use the [filesystem how-to guide](how-to.md) for S3 and caching,
or read [why fsspec-rs has separate protocols and layers](explanation.md).
