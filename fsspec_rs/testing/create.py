"""Create-if-absent conformance for immutable manifest stores.

Import these tests and supply ``create_url`` (a unique scratch directory/prefix)
plus ``create_storage_options``. AWS and on-prem S3 use ``s3-rs://bucket/prefix``;
NFS uses ``file-rs:///mount/prefix`` with ``auto_mkdir=True``. Only files created
by these tests are removed. Passing locally does not certify another deployment.
"""

from concurrent.futures import ThreadPoolExecutor
from threading import Barrier

import pytest
from fsspec.core import url_to_fs

from fsspec_rs import ConflictError, Missing

__all__ = ("create_store", "test_competing_creators_publish_one_complete_value", "test_create_conflict_preserves_content")


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
