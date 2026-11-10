# License bundle reference

Wheels include original Cargo dependency license texts in
`fsspec_rs/licenses/fsspec_rs.json` for the Python extension and
`fsspec_rs/licenses/fsspec.json` for the bundled `libfsspec` C ABI library. Each
bundle also includes Cargo dependencies for other targets, beyond those used at
runtime by the wheel. They do not cover Python dependencies, native-library
notices, or repaired-wheel libraries.

The collector does not discover license notices in the `r-efi` AUTHORS files. Versioned
overrides preserve those notices and supply full license terms through
`licenses.previous`. The [Cargo override reference](../../rust/LICENSES.md) records
their sources and human review requirements.
