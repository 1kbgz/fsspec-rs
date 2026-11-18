import json
import os
import uuid

import pytest

from fsspec_rs.testing.create import *


@pytest.fixture(params=["local", "memory", "s3", "deployment"])
def create_backend(request, tmp_path, s3_endpoint):
    name = request.param
    if name == "local":
        return f"file-rs://{tmp_path}/create", {"auto_mkdir": True}
    if name == "memory":
        return f"memory-rs:///create-{uuid.uuid4().hex}", {}
    if name == "s3":
        return "s3-rs://test/create", {"anon": True, "region": "us-east-1", "endpoint_url": s3_endpoint}
    prefix = os.environ.get("FSSPEC_CREATE_TEST_URL")
    if not prefix:
        pytest.skip("FSSPEC_CREATE_TEST_URL not set; live store certification not run")
    return prefix.rstrip("/") + f"/create-{uuid.uuid4().hex}", json.loads(os.environ.get("FSSPEC_CREATE_TEST_OPTIONS", "{}"))


@pytest.fixture
def create_url(create_backend):
    return create_backend[0]


@pytest.fixture
def create_storage_options(create_backend):
    return create_backend[1]
