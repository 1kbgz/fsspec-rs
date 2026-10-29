# License bundle reference

Wheels include original Cargo dependency license texts in
`fsspec_rs/licenses/cargo.json`. The bundle also includes Cargo dependencies for
other targets, beyond those used at runtime by the wheel. It does not cover
Python dependencies, native-library notices, or repaired-wheel libraries.

The collector does not discover license notices in the `r-efi` AUTHORS files. Versioned
overrides preserve those notices and supply full license terms through
`licenses.previous`. The [Cargo override reference](../../rust/LICENSES.md) records
their sources and human review requirements.
