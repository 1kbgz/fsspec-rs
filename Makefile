#########
# BUILD #
#########
.PHONY: develop-py develop-rs develop
develop-py:
	uv pip install -e .[develop]

develop-rs:
	make -C rust develop

develop: develop-rs develop-py  ## setup project for development

.PHONY: requirements-py requirements-rs requirements
requirements-py:  ## install prerequisite python build requirements
	python -m pip install --upgrade pip toml
	python -m pip install `python -c 'import toml; c = toml.load("pyproject.toml"); print("\n".join(c["build-system"]["requires"]))'`
	python -m pip install `python -c 'import toml; c = toml.load("pyproject.toml"); print(" ".join(c["project"]["optional-dependencies"]["develop"]))'`

requirements-rs:  ## install prerequisite rust build requirements
	make -C rust requirements

requirements: requirements-rs requirements-py  ## setup project for development

.PHONY: build-py build-rs build
build-py:
	python -m build -w -n

build-rs:
	make -C rust build

build: build-rs build-py  ## build the project

.PHONY: install
install:  ## install python library
	uv pip install .

#########
# LINTS #
#########
.PHONY: lint-py lint-rs lint-docs lint lints
lint-py:  ## run python linter with ruff
	python -m ruff check fsspec_rs
	python -m ruff format --check fsspec_rs

lint-rs:  ## run rust linter
	make -C rust lint

lint-docs:  ## lint docs with mdformat and codespell
	python -m mdformat --check README.md
	python -m codespell_lib README.md

lint: lint-rs lint-py lint-docs  ## run project linters

# alias
lints: lint

.PHONY: fix-py fix-rs fix-docs fix format
fix-py:  ## fix python formatting with ruff
	python -m ruff check --fix fsspec_rs
	python -m ruff format fsspec_rs

fix-rs:  ## fix rust formatting
	make -C rust fix

fix-docs:  ## autoformat docs with mdformat and codespell
	python -m mdformat README.md
	python -m codespell_lib --write README.md

fix: fix-rs fix-py fix-docs  ## run project autoformatters

# alias
format: fix

################
# Other Checks #
################
.PHONY: check-dist check-types checks check

check-dist:  ## check python sdist and wheel with check-dist
	check-dist -v

check-types:  ## check python types with ty
	ty check --python $$(which python)

checks: check-dist

# alias
check: checks

#########
# TESTS #
#########
.PHONY: test-py tests-py coverage-py
test-py:  ## run python tests
	python -m pytest -v fsspec_rs/tests

# alias
tests-py: test-py

coverage-py:  ## run python tests and collect test coverage
	python -m pytest -v fsspec_rs/tests --cov=fsspec_rs --cov-report term-missing --cov-report xml

.PHONY: test-rs tests-rs coverage-rs
test-rs:  ## run rust tests
	make -C rust test

# alias
tests-rs: test-rs

coverage-rs:  ## run rust tests and collect test coverage
	make -C rust coverage

.PHONY: test coverage tests
test: test-py test-rs  ## run all tests
coverage: coverage-py coverage-rs  ## run all tests and collect test coverage

# alias
tests: test

##############
# BENCHMARKS #
##############
.PHONY: benchmark benchmarks benchmark-quick benchmark-local benchmark-local-quick benchmark-publish benchmark-view
.PHONY: benchmark-local-fs benchmark-s3 benchmark-cache benchmark-pytest

ASV_CONFIG := $(CURDIR)/fsspec_rs/benchmarks/asv.conf.json
ASV_PUBLISH_CONFIG := $(ASV_CONFIG)

benchmark-machine:  ## initialize machine for benchmarks
	python -m asv machine --config $(ASV_CONFIG) --yes

benchmark: benchmark-machine  ## run benchmarks for current commit
	python -m asv run --config $(ASV_CONFIG) --verbose HEAD^!

benchmark-quick: benchmark-machine  ## run quick benchmark
	python -m asv run --config $(ASV_CONFIG) --quick --verbose HEAD^!

benchmark-local: benchmark-machine  ## run benchmark using local environment
	python -m asv run --config $(ASV_CONFIG) --python=same --verbose

benchmark-local-quick: benchmark-machine  ## run quick benchmark using local environment
	python -m asv run --config $(ASV_CONFIG) --python=same --quick --verbose

benchmark-publish:  ## generate benchmark results
	python -m asv publish --config $(ASV_PUBLISH_CONFIG)

benchmark-view:  ## view benchmark results
	python -m asv preview --config $(ASV_PUBLISH_CONFIG)

# pytest-benchmark targets
benchmark-local-fs:  ## benchmark local filesystem (Rust vs Python)
	python -m pytest fsspec_rs/benchmarks/bench_local.py -v --benchmark-columns=mean,stddev,rounds

benchmark-s3:  ## benchmark S3 filesystem against MinIO (Rust vs s3fs)
	python -m pytest fsspec_rs/benchmarks/bench_s3.py -v --benchmark-columns=mean,stddev,rounds

benchmark-cache:  ## benchmark cached vs uncached S3 reads
	python -m pytest fsspec_rs/benchmarks/bench_cache.py -v --benchmark-columns=mean,stddev,rounds

benchmark-pytest:  ## run all pytest-benchmark suites
	python -m pytest fsspec_rs/benchmarks/bench_*.py -v --benchmark-columns=mean,stddev,rounds

# Alias
benchmarks: benchmark

#########
# MINIO #
#########
.PHONY: minio-start minio-stop minio-status

MINIO_CONTAINER := fsspec-rs-minio

minio-start:  ## start MinIO via podman for local S3 benchmarks
	@podman rm -f $(MINIO_CONTAINER) 2>/dev/null || true
	podman run -d --name $(MINIO_CONTAINER) \
		-p 9000:9000 -p 9001:9001 \
		-e MINIO_ROOT_USER=minioadmin \
		-e MINIO_ROOT_PASSWORD=minioadmin \
		docker.io/minio/minio:latest server /data --console-address ":9001"
	@echo "Waiting for MinIO to be ready..."
	@for i in $$(seq 1 30); do \
		podman exec $(MINIO_CONTAINER) mc ready local 2>/dev/null && break; \
		sleep 1; \
	done
	podman run --rm --network=host --entrypoint="" \
		docker.io/minio/mc:latest \
		sh -c "mc alias set local http://localhost:9000 minioadmin minioadmin && mc mb --ignore-existing local/benchmark"
	@echo "MinIO ready at http://localhost:9000 (console: http://localhost:9001)"

minio-stop:  ## stop MinIO container
	podman rm -f $(MINIO_CONTAINER) 2>/dev/null || true

minio-status:  ## check if MinIO is running
	@podman ps --filter name=$(MINIO_CONTAINER) --format "{{.Names}} {{.Status}}" 2>/dev/null || echo "not running"

###########
# VERSION #
###########
.PHONY: show-version patch minor major

show-version:  ## show current library version
	@bump-my-version show current_version

patch:  ## bump a patch version
	@bump-my-version bump patch

minor:  ## bump a minor version
	@bump-my-version bump minor

major:  ## bump a major version
	@bump-my-version bump major

########
# DIST #
########
.PHONY: dist-py-wheel dist-py-sdist dist-rs dist-check dist publish

dist-py-wheel:  ## build python wheel
	python -m cibuildwheel --output-dir dist

dist-py-sdist:  ## build python sdist
	python -m build --sdist -o dist

dist-rs:  ## build rust dists
	make -C rust dist

dist-check:  ## run python dist checker with twine
	python -m twine check dist/*

dist: clean build dist-rs dist-py-wheel dist-py-sdist dist-check  ## build all dists

publish: dist  ## publish python assets

#########
# CLEAN #
#########
.PHONY: deep-clean clean

deep-clean: ## clean everything from the repository
	git clean -fdx

clean: ## clean the repository
	rm -rf .coverage coverage cover htmlcov logs build dist *.egg-info

############################################################################################

.PHONY: help

# Thanks to Francoise at marmelab.com for this
.DEFAULT_GOAL := help
help:
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-30s\033[0m %s\n", $$1, $$2}'

print-%:
	@echo '$*=$($*)'
