import os
import uuid

import pytest

from fsspec_rs.testing.records import *

S3_OPTIONS = {
    "key": os.environ.get("FSSPEC_S3_KEY"),
    "secret": os.environ.get("FSSPEC_S3_SECRET"),
    "endpoint_url": os.environ.get("FSSPEC_S3_ENDPOINT_URL"),
}
S3_BUCKET = os.environ.get("FSSPEC_S3_BUCKET", "timkpaine-public")
needs_s3 = pytest.mark.skipif(not S3_OPTIONS["endpoint_url"], reason="FSSPEC_S3_ENDPOINT_URL not set")


@pytest.fixture(
    params=[
        "file-rs",
        "file",
        pytest.param("s3-rs", marks=needs_s3),
    ]
)
def backend(request, tmp_path):
    name = request.param
    if name == "file" and request.node.originalname == "test_records_uncommitted_write_is_not_visible":
        request.applymarker(pytest.mark.xfail(strict=True, reason="fsspec's local file writes directly to the target path"))
    if name in ("file-rs", "file"):
        yield f"{name}://{tmp_path}/records", {"auto_mkdir": True}
        return
    options = {**S3_OPTIONS, "region": os.environ.get("FSSPEC_S3_REGION", "us-east-1")}
    url = f"{name}://{S3_BUCKET}/records-test-{uuid.uuid4().hex}"
    yield url, options
    from fsspec.core import url_to_fs

    fs, root = url_to_fs(url, **options)
    if fs.exists(root):
        fs.rm(root, recursive=True)


@pytest.fixture
def records_url(backend):
    return backend[0]


@pytest.fixture
def records_storage_options(backend):
    return backend[1]
