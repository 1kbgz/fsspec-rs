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
	python -m mdformat --check README.md docs/src/
	python -m codespell_lib README.md docs/src/

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
	python -m mdformat README.md docs/src/
	python -m codespell_lib --write README.md docs/src/

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
.PHONY: benchmark-local-fs benchmark-s3 benchmark-cache benchmark-pytest

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
.PHONY: minio-start minio-seed minio-stop minio-status test-s3-py test-s3-rs test-s3

CONTAINER_ENGINE ?= $(shell command -v podman >/dev/null 2>&1 && echo podman || echo docker)
MINIO_CONTAINER := fsspec-rs-minio
MINIO_IMAGE ?= docker.io/minio/minio:latest
MINIO_MC_IMAGE ?= docker.io/minio/mc:latest
MINIO_ROOT_USER ?= minioadmin
MINIO_ROOT_PASSWORD ?= minioadmin
MINIO_ENDPOINT ?= http://localhost:9000
MINIO_BUCKET ?= timkpaine-public
MINIO_PREFIX ?= projects/organizeit2
MINIO_EXPECTED_FILE_COUNT ?= 64

minio-start:  ## start MinIO via podman/docker for local S3 tests and benchmarks
	@$(CONTAINER_ENGINE) rm -f $(MINIO_CONTAINER) 2>/dev/null || true
	$(CONTAINER_ENGINE) run -d --name $(MINIO_CONTAINER) \
		-p 9000:9000 -p 9001:9001 \
		-e MINIO_ROOT_USER=$(MINIO_ROOT_USER) \
		-e MINIO_ROOT_PASSWORD=$(MINIO_ROOT_PASSWORD) \
		$(MINIO_IMAGE) server /data --console-address ":9001"
	@echo "Waiting for MinIO to be ready..."
	@ready=0; \
	for i in $$(seq 1 30); do \
		if $(CONTAINER_ENGINE) run --rm --network=host --entrypoint="" $(MINIO_MC_IMAGE) \
			sh -c 'mc alias set local $(MINIO_ENDPOINT) $(MINIO_ROOT_USER) $(MINIO_ROOT_PASSWORD) >/dev/null && mc ready local >/dev/null' >/dev/null 2>&1; then ready=1; break; fi; \
		sleep 1; \
	done; \
	test $$ready -eq 1
	$(MAKE) minio-seed
	@echo "MinIO ready at http://localhost:9000 (console: http://localhost:9001)"

minio-seed:  ## seed MinIO with the S3 integration-test fixture
	$(CONTAINER_ENGINE) run --rm --network=host --entrypoint="" \
		$(MINIO_MC_IMAGE) \
		sh -c 'mc alias set local $(MINIO_ENDPOINT) $(MINIO_ROOT_USER) $(MINIO_ROOT_PASSWORD) && \
			mc mb --ignore-existing local/$(MINIO_BUCKET) && \
			tmp=$$(mktemp) && : > $$tmp && \
			for d in subdir1 subdir2 subdir3 subdir4; do \
				for i in $$(seq 1 16); do \
					mc cp $$tmp local/$(MINIO_BUCKET)/$(MINIO_PREFIX)/$$d/file$$i.txt >/dev/null; \
				done; \
			done'

minio-stop:  ## stop MinIO container
	$(CONTAINER_ENGINE) rm -f $(MINIO_CONTAINER) 2>/dev/null || true

minio-status:  ## check if MinIO is running
	@$(CONTAINER_ENGINE) ps --filter name=$(MINIO_CONTAINER) --format "{{.Names}} {{.Status}}" 2>/dev/null || echo "not running"

test-s3-py:  ## run Python S3 tests against local MinIO
	FSSPEC_S3_ENDPOINT_URL=$(MINIO_ENDPOINT) \
	FSSPEC_S3_KEY=$(MINIO_ROOT_USER) \
	FSSPEC_S3_SECRET=$(MINIO_ROOT_PASSWORD) \
	FSSPEC_S3_REGION=us-east-1 \
	FSSPEC_S3_BUCKET=$(MINIO_BUCKET) \
	FSSPEC_S3_PREFIX=$(MINIO_PREFIX) \
	FSSPEC_S3_EXPECTED_FILE_COUNT=$(MINIO_EXPECTED_FILE_COUNT) \
	python -m pytest -v fsspec_rs/tests/test_s3.py

test-s3-rs:  ## run Rust S3 tests against local MinIO
	cd rust && \
	FSSPEC_S3_ENDPOINT_URL=$(MINIO_ENDPOINT) \
	FSSPEC_S3_KEY=$(MINIO_ROOT_USER) \
	FSSPEC_S3_SECRET=$(MINIO_ROOT_PASSWORD) \
	FSSPEC_S3_REGION=us-east-1 \
	FSSPEC_S3_BUCKET=$(MINIO_BUCKET) \
	FSSPEC_S3_EXPECTED_FILE_COUNT=$(MINIO_EXPECTED_FILE_COUNT) \
	cargo test s3_tests -- --ignored

test-s3: test-s3-py test-s3-rs  ## run all S3 tests against local MinIO

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
