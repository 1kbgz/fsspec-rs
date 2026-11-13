
.PHONY: requirements develop build
requirements:  ## install required dev dependencies
	rustup component add rustfmt
	rustup component add clippy
	cargo install --locked cargo-nextest
	cargo install --locked cargo-llvm-cov

develop: requirements  ## install required dev dependencies

build:  ## build release
	cargo build --workspace --release --all-features

.PHONY: lint lints fix format
lint:  ## run Clippy for linting, rustfmt for autoformat checks
	cargo clippy --workspace --all-features
	cargo fmt --all -- --check
# alias
lints: lint

fix:  ## fix code with rustfmt
	cargo fmt --all
#alias
format: fix

.PHONY: check checks check-wasm test-wasm
check:
	cargo check --workspace --all-features
# alias
checks: check

check-wasm:  ## check that the core crate builds for wasm32 without the s3 feature
	rustup target add wasm32-unknown-unknown
	RUSTFLAGS="-D warnings" cargo check -p fsspec_rs --no-default-features --target wasm32-unknown-unknown

test-wasm:  ## run the core crate's wasm32 tests in Node, without the s3 feature
	rustup target add wasm32-unknown-unknown
	cargo install wasm-bindgen-cli --locked --version $$(cargo pkgid wasm-bindgen | sed 's/.*@//')
	CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
		cargo test -p fsspec_rs --no-default-features --target wasm32-unknown-unknown --test wasm

.PHONY: test tests test-ci tests-ci
test:  ## run the tests
	cargo llvm-cov nextest --workspace --cobertura --output-path coverage.xml

# alias
tests: test

coverage:
	cargo llvm-cov nextest --workspace --lcov --output-path coverage

.PHONY: dist publish
dist:  ## create dist
	cargo publish --workspace --dry-run --allow-dirty

publish:  ## publish to cargo
	cargo publish --workspace

# Thanks to Francoise at marmelab.com for this
.DEFAULT_GOAL := help
help:
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-30s\033[0m %s\n", $$1, $$2}'

print-%:
	@echo '$*=$($*)'
