# How to build from source

Source builds require Rust, hatch-rs 0.4.3 or newer, and the Cargo license
collector. In a source checkout, install the collector before installing the
Python package:

```console
cargo install cargo-bundle-licenses --version 4.2.0 --locked
python -m pip install .
```

For an editable development install, use:

```console
python -m pip install -e '.[develop]'
```

If the collector reports missing license texts, check the versioned overrides and
[source review requirements](../../rust/LICENSES.md). See
[License bundle reference](licenses.md) for the wheel bundle's coverage.
