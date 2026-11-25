"""Create-if-absent conformance for immutable manifest stores.

Import these tests and supply ``create_url`` (a unique scratch directory/prefix)
plus ``create_storage_options``. AWS and on-prem S3 use ``s3-rs://bucket/prefix``;
NFS uses ``file-rs:///mount/prefix`` with ``auto_mkdir=True``. Only files created
by these tests are removed. Passing locally does not certify another deployment.
"""

import hashlib
import json
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from threading import Barrier, Event

import pytest
from fsspec.core import url_to_fs

from fsspec_rs import ConflictError, Missing

__all__ = (
    "create_store",
    "test_acknowledged_create_is_visible_to_a_new_process",
    "test_competing_creators_publish_one_complete_value",
    "test_create_conflict_preserves_content",
    "test_readers_never_observe_a_partial_create",
)


@pytest.fixture
def create_store(create_url, create_storage_options):
    first, root = url_to_fs(create_url, skip_instance_cache=True, **create_storage_options)
    second, second_root = url_to_fs(create_url, skip_instance_cache=True, **create_storage_options)
    assert root == second_root
    assert first.io_capabilities().write_missing
    path = root.rstrip("/") + "/manifest"
    try:
        yield first, second, path
    finally:
        try:
            first.rm_file(path)
        except FileNotFoundError:
            pass


@pytest.mark.parametrize("value", [b"", bytes(range(256)), b"manifest" * 65536], ids=["empty", "binary", "large"])
def test_create_conflict_preserves_content(create_store, value):
    first, second, path = create_store
    first.write(path, value, condition=Missing())
    assert second.read(path).data == value
    for replacement in (value, b"replacement"):
        with pytest.raises(ConflictError):
            second.write(path, replacement, condition=Missing())
        assert first.read(path).data == value


def test_competing_creators_publish_one_complete_value(create_store):
    first, second, path = create_store
    barrier = Barrier(2)
    values = (b"first" * 65536, b"second" * 65536)

    def create(args):
        fs, value = args
        barrier.wait(timeout=10)
        try:
            fs.write(path, value, condition=Missing())
            return value
        except ConflictError:
            return None

    with ThreadPoolExecutor(max_workers=2) as pool:
        results = list(pool.map(create, zip((first, second), values, strict=True)))
    winners = [value for value in results if value is not None]
    assert len(winners) == 1
    assert first.read(path).data == second.read(path).data == winners[0]


def test_readers_never_observe_a_partial_create(create_store):
    first, second, path = create_store
    value = bytes(range(256)) * 32768
    barrier, done = Barrier(2), Event()

    def create():
        barrier.wait(timeout=10)
        try:
            first.write(path, value, condition=Missing())
        finally:
            done.set()

    with ThreadPoolExecutor(max_workers=1) as pool:
        future = pool.submit(create)
        with pytest.raises(FileNotFoundError):
            second.read(path)
        barrier.wait(timeout=10)
        while not done.is_set():
            try:
                observed = second.read(path).data
            except FileNotFoundError:
                continue
            assert observed == value
        future.result()
    assert second.read(path).data == value


def test_acknowledged_create_is_visible_to_a_new_process(create_store, create_url, create_storage_options):
    first, _, path = create_store
    if "memory" in first.protocol or "memory-rs" in first.protocol:
        pytest.skip("memory storage is process-local")
    value = bytes(range(256)) * 4096
    first.write(path, value, condition=Missing())
    script = """
import hashlib, json, sys
from fsspec.core import url_to_fs
from fsspec_rs import ConflictError, Missing
url, options = json.load(sys.stdin)
fs, root = url_to_fs(url, skip_instance_cache=True, **options)
path = root.rstrip('/') + '/manifest'
value = fs.read(path).data
try:
    fs.write(path, b'replacement', condition=Missing())
except ConflictError:
    pass
else:
    raise AssertionError('reopened client overwrote published object')
assert fs.read(path).data == value
print(hashlib.sha256(value).hexdigest())
"""
    result = subprocess.run(
        [sys.executable, "-c", script],
        input=json.dumps([create_url, create_storage_options]),
        capture_output=True,
        check=True,
        text=True,
        timeout=60,
    )
    assert result.stdout.strip() == hashlib.sha256(value).hexdigest()
