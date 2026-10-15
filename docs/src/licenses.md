# License bundle reference

Wheels include original Cargo dependency license texts in
`fsspec_rs/licenses/cargo.json`. The collector includes dependencies for other
targets, so the bundle is a superset of the wheel's runtime dependencies.
Python dependencies, native-library notices, and repaired-wheel libraries are
outside its scope.

The collector cannot find the `r-efi` notices in its AUTHORS files. Versioned
overrides preserve those notices and supply full license terms through
`licenses.previous`. The [Cargo override reference](../../rust/LICENSES.md) records
their sources and human review requirements.
