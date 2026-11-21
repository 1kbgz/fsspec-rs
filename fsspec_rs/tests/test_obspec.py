"""Native S3 object reads satisfy obspec 0.1 without filesystem path semantics."""

import gc
import inspect
from datetime import UTC, datetime
from urllib.parse import quote

import obspec
import pytest
from obspec import exceptions

from fsspec_rs import S3FileSystem
from fsspec_rs.obspec import InvalidPathError, NotFoundError, NotSupportedError, PreconditionError, S3ObjectStore


@pytest.fixture
def object_fs(s3_endpoint):
    return S3FileSystem(bucket="test", endpoint_url=s3_endpoint, anon=True, skip_instance_cache=True, client_options={"max_retries": 0})


@pytest.fixture
def store(object_fs, s3_endpoint):
    s3_endpoint.objects["/test/data"] = (b"0123456789", '"original"')
    return S3ObjectStore(object_fs)


def test_protocol_signatures():
    for protocol, method in [(obspec.Head, "head"), (obspec.GetRange, "get_range"), (obspec.GetRanges, "get_ranges")]:
        expected = inspect.signature(getattr(protocol, method))
        actual = inspect.signature(getattr(S3ObjectStore, method))
        assert [(p.name, p.kind, p.default) for p in actual.parameters.values()] == [
            (p.name, p.kind, p.default) for p in expected.parameters.values()
        ]


def test_native_metadata(store, object_fs, s3_endpoint, monkeypatch):
    original = s3_endpoint.handler.end_headers

    def headers(handler):
        handler.send_header("x-amz-version-id", "native-version")
        original(handler)

    monkeypatch.setattr(s3_endpoint.handler, "end_headers", headers)
    assert store.head("data") == {
        "path": "data",
        "size": 10,
        "last_modified": datetime(2026, 11, 17, 12, tzinfo=UTC),
        "e_tag": '"original"',
        "version": "native-version",
    }
    assert s3_endpoint.requests == [("HEAD", "/test/data", None, None)]
    assert object_fs.head("test/data", 3) == b"012"


def test_ranges_are_ordered_coalesced_and_validated(store, s3_endpoint):
    result = store.get_ranges("data", starts=[7, 1, 7], ends=[99, 4, 9])
    assert [bytes(memoryview(item)) for item in result] == [b"789", b"123", b"78"]
    assert s3_endpoint.requests == [("HEAD", "/test/data", None, None), ("GET", "/test/data", "bytes=1-9", '"original"')]
    assert store.get_ranges("data", starts=[2, 8], lengths=[3, 20]) == [b"234", b"89"]
    assert store.get_range("data", start=4, end=7) == b"456"
    assert store.get_range("data", start=4, length=2) == b"45"


@pytest.mark.parametrize(
    "kwargs",
    [
        {},
        {"ends": [1], "lengths": [1]},
        {"ends": []},
        {"ends": [0]},
        {"lengths": [0]},
        {"ends": [-1]},
        {"ends": [2**63]},
        {"lengths": [2**63]},
    ],
)
def test_invalid_range_arguments_fail_without_io(store, s3_endpoint, kwargs):
    with pytest.raises(ValueError):
        store.get_ranges("data", starts=[0], **kwargs)
    assert not s3_endpoint.requests


@pytest.mark.parametrize(
    "start,end,exception",
    [
        (-1, 2, ValueError),
        (True, 2, TypeError),
        (0, False, TypeError),
        (0.0, 2, TypeError),
        (0, "2", TypeError),
        (2, 1, ValueError),
        (2**63, 2**63 + 1, ValueError),
    ],
)
def test_invalid_bound_types(store, s3_endpoint, start, end, exception):
    with pytest.raises(exception):
        store.get_range("data", start=start, end=end)
    assert not s3_endpoint.requests


def test_length_overflow(store, s3_endpoint):
    with pytest.raises(ValueError):
        store.get_range("data", start=2**63 - 1, length=1)
    assert not s3_endpoint.requests


@pytest.mark.parametrize("start", [10, 11])
def test_eof_start_fails_after_metadata(store, s3_endpoint, start):
    with pytest.raises(ValueError) as caught:
        store.get_range("data", start=start, length=1)
    assert caught.value.kind == "invalid"
    assert [r[0] for r in s3_endpoint.requests] == ["HEAD"]


def test_empty_vectors_need_no_object(store, s3_endpoint):
    assert store.get_ranges("missing", starts=[], ends=[]) == []
    assert not s3_endpoint.requests


def test_empty_object_has_metadata_but_no_valid_range(store, s3_endpoint):
    s3_endpoint.objects["/test/empty"] = (b"", '"empty"')
    assert store.head("empty")["size"] == 0
    with pytest.raises(ValueError):
        store.get_range("empty", start=0, length=1)


@pytest.mark.parametrize("path", ["", "/data", "data/", "a//b", "a/./b", "../b", "s3://test/data", "a\\b", "a\x00b", "a\x7fb"])
def test_invalid_paths_fail_without_io(store, s3_endpoint, path):
    with pytest.raises(InvalidPathError) as caught:
        store.head(path)
    assert isinstance(exceptions.map_exception(caught.value), exceptions.InvalidPathError)
    assert not s3_endpoint.requests


def test_bucket_relative_paths_prefix_and_literal_keys(object_fs, s3_endpoint):
    store = S3ObjectStore(object_fs, prefix="test")
    # Seed native keys independently: filesystem uploads escape some characters.
    for name in ["plain", "space key", "literal%2Fkey", "snow-雪", "question?hash#"]:
        wire_path = "/test/test/" + quote(name, safe="/")
        s3_endpoint.objects[wire_path] = (b"native", '"version"')
        assert store.head(name)["path"] == name
        assert store.get_range(name, start=0, length=6) == b"native"
        assert s3_endpoint.requests[-1][1] == wire_path
    # These keys need no encoding and are shared with the filesystem view.
    object_fs.pipe_file("test/test/shared", b"shared")
    assert store.get_range("shared", start=0, length=6) == b"shared"
    object_fs.pipe_file("test/test", b"bucket-named key")
    assert S3ObjectStore(object_fs).get_range("test", start=0, length=6) == b"bucket"


def test_missing_prefix_is_not_a_directory(store, s3_endpoint):
    s3_endpoint.objects["/test/prefix/child"] = (b"child", '"child"')
    with pytest.raises(NotFoundError) as caught:
        store.head("prefix")
    assert caught.value.kind == "not_found"
    assert caught.value.__cause__ is not None
    assert isinstance(exceptions.map_exception(caught.value), exceptions.NotFoundError)
    assert s3_endpoint.requests == [("HEAD", "/test/prefix", None, None)]


def test_changed_object_fails_instead_of_mixing_versions(store, s3_endpoint, monkeypatch):
    original = s3_endpoint.handler.do_GET

    def replace(handler):
        s3_endpoint.objects["/test/data"] = (b"replacement", '"changed"')
        original(handler)

    monkeypatch.setattr(s3_endpoint.handler, "do_GET", replace)
    with pytest.raises(PreconditionError) as caught:
        store.get_range("data", start=0, length=2)
    assert caught.value.kind == "conflict"
    assert isinstance(exceptions.map_exception(caught.value), exceptions.PreconditionError)


def test_missing_etag_rejects_unverifiable_ranges(store, s3_endpoint):
    s3_endpoint.objects["/test/data"] = (b"data", None)
    assert store.head("data")["e_tag"] is None
    with pytest.raises(NotSupportedError) as caught:
        store.get_range("data", start=0, length=1)
    assert isinstance(exceptions.map_exception(caught.value), exceptions.NotSupportedError)
    assert all(request[0] == "HEAD" for request in s3_endpoint.requests)


def test_object_handle_outlives_filesystem(s3_endpoint):
    fs = S3FileSystem(bucket="test", endpoint_url=s3_endpoint, anon=True, skip_instance_cache=True)
    store = S3ObjectStore(fs)
    del fs
    gc.collect()
    s3_endpoint.objects["/test/data"] = (b"alive", '"live"')
    assert store.get_range("data", start=0, length=5) == b"alive"


def test_permission_error_maps_by_name(store, s3_endpoint, monkeypatch):
    def denied(handler):
        handler.respond(403)

    monkeypatch.setattr(s3_endpoint.handler, "do_HEAD", denied)
    with pytest.raises(PermissionError) as caught:
        store.head("data")
    assert caught.value.kind == "permission_denied"
    assert isinstance(exceptions.map_exception(caught.value), exceptions.PermissionDeniedError)


def test_native_binding_rejects_invalid_ranges_before_io(object_fs, s3_endpoint):
    # The C ABI and Rust delegate must also enforce the contract without Python validation.
    native = object_fs._rust.object_store()
    for ranges in [[(-1, 1)], [(0, 0)], [(2, 1)]]:
        with pytest.raises(ValueError) as caught:
            native.read_ranges("missing", ranges)
        assert caught.value.kind == "invalid"
    assert not s3_endpoint.requests


def test_integer_like_offsets(store):
    class Offset:
        def __init__(self, value):
            self.value = value

        def __index__(self):
            return self.value

    assert store.get_range("data", start=Offset(1), end=Offset(4)) == b"123"
    assert store.get_ranges("data", starts=[Offset(2)], lengths=[Offset(3)]) == [b"234"]
    with pytest.raises(ValueError):
        store.get_range("data", start=Offset(-1), end=Offset(4))
    with pytest.raises(ValueError):
        store.get_range("data", start=Offset(2**63 - 1), length=Offset(1))


@pytest.mark.parametrize("codepoint", [0xD800, 0xDFFF])
def test_invalid_unicode_keys_and_prefixes(object_fs, store, s3_endpoint, codepoint):
    surrogate = chr(codepoint)
    with pytest.raises(InvalidPathError):
        store.head("key" + surrogate)
    with pytest.raises(InvalidPathError):
        S3ObjectStore(object_fs, prefix=surrogate)
    assert not s3_endpoint.requests


def test_pre_epoch_metadata_is_unsupported(store, s3_endpoint, monkeypatch):
    original = s3_endpoint.handler.send_header

    def header(handler, name, value):
        if name == "Last-Modified":
            value = "Wed, 31 Dec 1969 23:59:59 GMT"
        original(handler, name, value)

    monkeypatch.setattr(s3_endpoint.handler, "send_header", header)
    with pytest.raises(NotSupportedError) as caught:
        store.head("data")
    assert caught.value.kind == "not_supported"
