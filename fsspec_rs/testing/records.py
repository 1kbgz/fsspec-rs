"""Records conformance tests for fsspec filesystems used as keyed state stores.

A records root holds one file per record at ``{root}/{namespace}/{key}``. Callers encode
namespaces and keys so each is a single path segment (csp-bot uses ``urllib.parse.quote``
with ``safe=""``). These tests check the guarantees such callers rely on through ordinary
fsspec operations: per-key atomic replacement, durable acknowledged writes, complete
listings, and ``FileNotFoundError`` when reading missing keys.

Import the tests into a test module and define two fixtures:

.. code-block:: python

    from fsspec_rs.testing.records import *  # noqa: F403

    @pytest.fixture
    def records_url(tmp_path):
        return f"file-rs://{tmp_path}/records"

    @pytest.fixture
    def records_storage_options():
        return {"auto_mkdir": True}

Windows removes trailing dots and spaces from file names, so portable callers must
encode them as well.

``records_url`` must name an empty root that is unique to the test.
``records_storage_options`` must be JSON-serializable because one test reopens the store
in a separate process. Filesystems that need parent directories created before writing
must be configured to create them.
"""

from __future__ import annotations

import json
import subprocess
import sys

import pytest
from fsspec import get_mapper
from fsspec.core import url_to_fs

__all__ = (
    "records",
    "test_records_binary_and_empty_values_round_trip",
    "test_records_encoded_keys_round_trip",
    "test_records_listing_is_complete",
    "test_records_mapper_operations",
    "test_records_missing_keys_raise_file_not_found",
    "test_records_namespaces_and_prefixes_are_isolated",
    "test_records_overwrite_and_delete_are_isolated",
    "test_records_state_is_visible_to_a_new_process",
    "test_records_uncommitted_write_is_not_visible",
)

BINARY = bytes(range(256))
ENCODED_KEYS = ("a%2Fb", "with%20space", "100%25", "%E2%9C%93", "plus%2Bequals%3D", "dots..inside", "UPPER-lower_~")


@pytest.fixture
def records(records_url, records_storage_options):
    """Return ``(fs, root)`` for the store under test."""
    fs, root = url_to_fs(records_url, **records_storage_options)
    return fs, root.rstrip("/")


def test_records_binary_and_empty_values_round_trip(records):
    fs, root = records
    fs.pipe_file(f"{root}/ns/binary", BINARY)
    fs.pipe_file(f"{root}/ns/empty", b"")

    assert fs.cat_file(f"{root}/ns/binary") == BINARY
    assert fs.cat_file(f"{root}/ns/empty") == b""
    assert fs.exists(f"{root}/ns/empty")


def test_records_overwrite_and_delete_are_isolated(records):
    fs, root = records
    fs.pipe_file(f"{root}/ns/a", b"a1")
    fs.pipe_file(f"{root}/ns/b", b"b1")

    fs.pipe_file(f"{root}/ns/a", b"a2")
    assert fs.cat_file(f"{root}/ns/a") == b"a2"
    assert fs.cat_file(f"{root}/ns/b") == b"b1"

    fs.rm_file(f"{root}/ns/a")
    assert not fs.exists(f"{root}/ns/a")
    assert fs.cat_file(f"{root}/ns/b") == b"b1"


def test_records_missing_keys_raise_file_not_found(records):
    fs, root = records
    fs.pipe_file(f"{root}/ns/present", b"x")

    with pytest.raises(FileNotFoundError):
        fs.cat_file(f"{root}/ns/missing")
    with pytest.raises(FileNotFoundError):
        fs.info(f"{root}/ns/missing")
    assert not fs.exists(f"{root}/ns/missing")
    # Deleting a missing key may succeed, since S3 deletes are idempotent.
    try:
        fs.rm_file(f"{root}/ns/missing")
    except FileNotFoundError:
        pass
    assert fs.cat_file(f"{root}/ns/present") == b"x"


def test_records_namespaces_and_prefixes_are_isolated(records):
    fs, root = records
    for path in ("one/a", "one/ab", "one/b", "two/a", "one_more/a"):
        fs.pipe_file(f"{root}/{path}", path.encode())

    assert _keys(fs, root, "one") == {"a", "ab", "b"}
    assert {key for key in _keys(fs, root, "one") if key.startswith("a")} == {"a", "ab"}
    assert _keys(fs, root, "two") == {"a"}
    assert fs.cat_file(f"{root}/two/a") == b"two/a"


def test_records_encoded_keys_round_trip(records):
    fs, root = records
    for key in ENCODED_KEYS:
        fs.pipe_file(f"{root}/ns/{key}", key.encode())

    assert _keys(fs, root, "ns") == set(ENCODED_KEYS)
    for key in ENCODED_KEYS:
        assert fs.cat_file(f"{root}/ns/{key}") == key.encode()


def test_records_listing_is_complete(records):
    fs, root = records
    expected = {f"key-{index:03d}" for index in range(50)}
    for key in expected:
        fs.pipe_file(f"{root}/ns/{key}", key.encode())

    assert _keys(fs, root, "ns") == expected


def test_records_uncommitted_write_is_not_visible(records):
    fs, root = records
    path = f"{root}/ns/record"
    fs.pipe_file(path, b"old")

    with fs.open(path, "wb") as file:
        file.write(b"new value")
        file.flush()
        assert fs.cat_file(path) == b"old"

    assert fs.cat_file(path) == b"new value"


def test_records_state_is_visible_to_a_new_process(records, records_url, records_storage_options):
    fs, root = records
    fs.pipe_file(f"{root}/ns/kept", b"kept")
    fs.pipe_file(f"{root}/ns/replaced", b"first")
    fs.pipe_file(f"{root}/ns/replaced", b"second")
    fs.pipe_file(f"{root}/ns/deleted", b"gone")
    fs.rm_file(f"{root}/ns/deleted")

    script = """
import json, sys
from fsspec.core import url_to_fs
url, options = json.loads(sys.argv[1])
fs, root = url_to_fs(url, **options)
root = root.rstrip("/")
names = sorted(path.rsplit("/", 1)[-1] for path in fs.find(f"{root}/ns"))
print(json.dumps({"names": names, "replaced": fs.cat_file(f"{root}/ns/replaced").decode()}))
"""
    result = subprocess.run(
        [sys.executable, "-c", script, json.dumps([records_url, records_storage_options])],
        capture_output=True,
        check=True,
        text=True,
    )

    assert json.loads(result.stdout) == {"names": ["kept", "replaced"], "replaced": "second"}


def test_records_mapper_operations(records_url, records_storage_options):
    mapper = get_mapper(records_url, **records_storage_options)
    mapper["ns/a"] = b"a"
    mapper["ns/b"] = b""

    assert mapper["ns/a"] == b"a"
    assert mapper["ns/b"] == b""
    assert set(mapper) == {"ns/a", "ns/b"}

    del mapper["ns/a"]
    assert set(mapper) == {"ns/b"}
    with pytest.raises(KeyError):
        mapper["ns/a"]


def _keys(fs, root, namespace):
    return {path.rsplit("/", 1)[-1] for path in fs.find(f"{root}/{namespace}")}
