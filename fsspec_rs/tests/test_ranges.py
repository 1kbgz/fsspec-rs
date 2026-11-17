"""Range batches retain caller ordering while sharing reads of one object."""

import uuid

import pytest

from fsspec_rs import (
    ConflictError,
    LocalFileSystem,
    Match,
    MemoryFileSystem,
    Missing,
    RangeReadResult,
    RustLocalFs,
    RustMemoryFs,
    RustS3Fs,
    S3FileSystem,
    Validator,
)


@pytest.fixture(params=[LocalFileSystem, MemoryFileSystem, RustLocalFs, RustMemoryFs])
def ranged_filesystem(request, tmp_path):
    fs = request.param()
    path = str(tmp_path / "ranges") if request.param in (LocalFileSystem, RustLocalFs) else f"/ranges-{uuid.uuid4().hex}"
    fs.write(path, b"0123456789")
    return fs, path


def test_ranges_preserve_order_duplicates_empty_and_signed_bounds(ranged_filesystem):
    fs, path = ranged_filesystem
    ranges = [(7, 10), (0, 3), (2, 6), (0, 3), (5, 5), (8, 3), (-3, None), (None, -8), (-100, 100), (100, None)]
    result = fs.read_ranges(path, ranges, max_gap=1, max_request_size=4, max_concurrency=2)
    assert isinstance(result, RangeReadResult)
    assert result.data == (b"789", b"012", b"2345", b"012", b"", b"", b"789", b"01", b"0123456789", b"")
    assert isinstance(result.data, tuple)
    assert result.validator is None
    with pytest.raises(AttributeError):
        result.data = ()
    assert fs.read_ranges(path, []).data == ()


def test_ranges_reject_directories(ranged_filesystem):
    fs, path = ranged_filesystem
    directory = f"{path}-directory"
    fs.mkdir(directory)
    with pytest.raises(IsADirectoryError):
        fs.read_ranges(directory, [(0, 1)])


@pytest.mark.parametrize("options", [{"max_request_size": 0}, {"max_concurrency": 0}, {"max_gap": -1}])
def test_invalid_range_options_are_rejected(ranged_filesystem, options):
    fs, path = ranged_filesystem
    with pytest.raises((ValueError, OverflowError)):
        fs.read_ranges(path, [(0, 1)], **options)


def test_ranges_reject_unsupported_and_invalid_conditions(ranged_filesystem):
    fs, path = ranged_filesystem
    with pytest.raises(NotImplementedError):
        fs.read_ranges(path, [(0, 1)], condition=Match(Validator("scope", "value")))
    with pytest.raises(ValueError, match="Missing"):
        fs.read_ranges(path, [(0, 1)], condition=Missing())
    with pytest.raises(TypeError, match="condition"):
        fs.read_ranges(path, [(0, 1)], condition="etag")
    with pytest.raises(ValueError, match="length 2"):
        fs.read_ranges(path, [(0, 1, 2)])


@pytest.fixture(params=[LocalFileSystem, MemoryFileSystem])
def cat_filesystem(request, tmp_path):
    fs = request.param()
    base = str(tmp_path) if request.param is LocalFileSystem else f"/ranges-{uuid.uuid4().hex}"
    first, second = f"{base}/first", f"{base}/second"
    fs.write(first, b"0123456789")
    fs.write(second, b"abcdefghij")
    return fs, first, second


def test_cat_ranges_groups_normalized_paths_and_restores_order(cat_filesystem, monkeypatch):
    fs, first, second = cat_filesystem
    calls = []
    native = fs.read_ranges

    def record(path, ranges, **options):
        calls.append((path, ranges, options))
        return native(path, ranges, **options)

    monkeypatch.setattr(fs, "read_ranges", record)
    result = fs.cat_ranges(
        [first, second, fs.unstrip_protocol(first), second], [5, 0, 0, 2], [8, 2, 3, 4], max_gap=2, max_request_size=4, max_concurrency=1
    )
    assert result == [b"567", b"ab", b"012", b"cd"]
    assert len(calls) == 2
    assert calls[0][1] == [(5, 8), (0, 3)]
    assert calls[0][2]["max_gap"] == 2
    assert calls[0][2]["max_request_size"] == 4
    assert calls[0][2]["max_concurrency"] == 1
    assert fs.cat_ranges([first, second], 1, 3) == [b"12", b"bc"]
    assert fs.cat_ranges([first, second], None, -8) == [b"01", b"ab"]
    assert fs.cat_ranges([], None, None) == []


def test_cat_ranges_returns_path_errors_and_can_raise(cat_filesystem):
    fs, first, second = cat_filesystem
    missing = f"{first}-missing"
    result = fs.cat_ranges([first, missing, second, missing], 0, 2)
    assert result[0] == b"01"
    assert result[2] == b"ab"
    assert isinstance(result[1], FileNotFoundError)
    assert isinstance(result[3], FileNotFoundError)
    with pytest.raises(FileNotFoundError):
        fs.cat_ranges([missing, first], 0, 2, on_error="raise")


@pytest.mark.parametrize(
    ("paths", "starts", "ends", "options", "error"),
    [
        (("first",), 0, 1, {}, TypeError),
        (["first", "second"], [0], [1, 2], {}, ValueError),
        (["first"], 0, 1, {"unexpected": 1}, TypeError),
        (["first"], 0, 1, {"max_gap": -1}, ValueError),
        (["first"], 0, 1, {"max_request_size": 0}, ValueError),
        (["first"], 0, 1, {"max_concurrency": 0}, ValueError),
        (["first"], 0, 1, {"on_error": "ignore"}, ValueError),
        (["first"], 0, 1, {"condition": "etag"}, TypeError),
        (["first"], 0, 1, {"condition": Missing()}, ValueError),
        (["first", "second"], 0, 1, {"condition": Match(Validator("scope", "value"))}, ValueError),
        (["first"], "start", 1, {}, TypeError),
        (["first"], 1 << 63, 1, {}, OverflowError),
    ],
)
def test_cat_ranges_validates_before_io(paths, starts, ends, options, error, monkeypatch):
    fs = MemoryFileSystem()

    def no_io(*args, **kwargs):
        pytest.fail("invalid arguments reached native I/O")

    monkeypatch.setattr(fs, "read_ranges", no_io)
    with pytest.raises(error):
        fs.cat_ranges(paths, starts, ends, **options)


@pytest.mark.parametrize("fs_class", [S3FileSystem, RustS3Fs])
def test_s3_ranges_coalesce_and_pin_reads(s3_endpoint, fs_class):
    fs = fs_class(bucket="test", region="us-east-1", anon=True, endpoint_url=s3_endpoint)
    version = fs.write("key", b"0123456789").validator
    assert version is not None
    result = fs.read_ranges("key", [(6, 8), (0, 2), (3, 5), (0, 2), (9, 9)], max_gap=1, max_request_size=5)
    assert result.data == (b"67", b"01", b"34", b"01", b"")
    assert result.validator == version
    requests = s3_endpoint.requests
    assert sum(method == "HEAD" for method, _, _, _ in requests) == 1
    gets = [(bounds, match) for method, _, bounds, match in requests if method == "GET"]
    assert sorted(bounds for bounds, _ in gets) == ["bytes=0-4", "bytes=6-7"]
    assert len({match for _, match in gets}) == 1
    assert all(match is not None for _, match in gets)
    fs.write("key", b"abcdefghij")
    with pytest.raises(ConflictError):
        fs.read_ranges("key", [(0, 2)], condition=Match(version))


def test_s3_cat_ranges_keeps_condition_and_alias_grouping(s3_endpoint):
    fs = S3FileSystem(bucket="test", region="us-east-1", anon=True, endpoint_url=s3_endpoint)
    version = fs.write("key", b"0123456789").validator
    assert version is not None
    assert fs.cat_ranges(["key", "test/key", "s3-rs://test/key"], [3, 0, 1], [5, 2, 3], condition=Match(version)) == [b"34", b"01", b"12"]
    assert sum(method == "HEAD" for method, _, _, _ in s3_endpoint.requests) == 1


def test_s3_cat_ranges_preserves_bucket_root_identity(s3_endpoint):
    fs = S3FileSystem(bucket="test", region="us-east-1", anon=True, endpoint_url=s3_endpoint)
    fs.write("test/test", b"key named test")
    roots = ["test", "test/", "s3-rs://test", "s3-rs://test/"]
    for path in roots:
        with pytest.raises(FileNotFoundError):
            fs.read_ranges(path, [(0, 3)])
        with pytest.raises(FileNotFoundError):
            fs.cat_ranges([path], [0], [3], on_error="raise")
    s3_endpoint.requests.clear()
    result = fs.cat_ranges([*roots, "test/test"], 0, 3)
    assert all(isinstance(error, FileNotFoundError) for error in result[:-1])
    assert result[-1] == b"key"
    heads = [path for method, path, _, _ in s3_endpoint.requests if method == "HEAD"]
    assert heads == ["/test/", "/test/test"]
