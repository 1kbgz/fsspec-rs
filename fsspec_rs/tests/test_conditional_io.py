"""Typed conditions must be honored or rejected before an operation writes bytes."""

import uuid
from concurrent.futures import ThreadPoolExecutor
from typing import Any

import pytest

from fsspec_rs import (
    ConflictError,
    IoCapabilities,
    LocalFileSystem,
    Match,
    MemoryFileSystem,
    Missing,
    ReadResult,
    RustLocalFs,
    RustMemoryFs,
    RustS3Fs,
    S3FileSystem,
    Validator,
    WriteResult,
)


@pytest.fixture(params=[LocalFileSystem, MemoryFileSystem, RustLocalFs, RustMemoryFs])
def filesystem(request, tmp_path):
    fs = request.param()
    path = str(tmp_path / "value") if request.param in (LocalFileSystem, RustLocalFs) else f"/conditional-{uuid.uuid4().hex}"
    return fs, path


def test_unconditional_results_and_capabilities(filesystem):
    fs, path = filesystem
    capabilities = fs.io_capabilities()
    assert isinstance(capabilities, IoCapabilities)
    assert not capabilities.read_match
    assert not capabilities.write_match
    assert capabilities.write_missing
    result = fs.write(path, b"abcdef")
    assert isinstance(result, WriteResult)
    assert result.validator is None
    read = fs.read(path, start=1, end=-1)
    assert isinstance(read, ReadResult)
    assert read.data == b"bcde"
    assert isinstance(read.data, bytes)
    assert read.validator is None
    with pytest.raises(AttributeError):
        read.data = b"changed"
    with pytest.raises(AttributeError):
        capabilities.write_missing = False
    assert fs.pipe_file(path, b"legacy") is None
    assert bytes(fs.cat_file(path)) == b"legacy"


def test_missing_write_is_atomic_and_conflicts(filesystem):
    fs, path = filesystem

    def create(value):
        try:
            return fs.write(path, value, condition=Missing())
        except ConflictError:
            return None

    with ThreadPoolExecutor(max_workers=2) as pool:
        results = list(pool.map(create, [b"first", b"second"]))
    assert sum(isinstance(result, WriteResult) for result in results) == 1
    winner = fs.read(path).data
    assert winner in (b"first", b"second")
    with pytest.raises(ConflictError):
        fs.pipe_file(path, b"replacement", condition=Missing())
    assert bytes(fs.cat_file(path)) == winner


def test_unsupported_match_does_not_read_or_replace(filesystem):
    fs, path = filesystem
    fs.write(path, b"original")
    condition = Match(Validator("scope", "version"))
    for read in (fs.read, fs.cat_file):
        with pytest.raises(NotImplementedError):
            read(path, condition=condition)
    for write in (fs.write, fs.pipe_file):
        with pytest.raises(NotImplementedError):
            write(path, b"replacement", condition=condition)
    assert fs.read(path).data == b"original"


def test_read_missing_is_rejected(filesystem):
    fs, path = filesystem
    with pytest.raises(ValueError, match="Missing"):
        fs.read(path, condition=Missing())
    assert not fs.exists(path)


@pytest.mark.parametrize("condition", [object(), "etag", {"etag": "version"}, Validator("scope", "version"), Missing])
def test_unknown_conditions_are_rejected_without_writes(filesystem, condition):
    fs, path = filesystem
    for read in (fs.read, fs.cat_file):
        with pytest.raises(TypeError, match="condition"):
            read(path, condition=condition)
    for write in (fs.write, fs.pipe_file):
        with pytest.raises(TypeError, match="condition"):
            write(path, b"replacement", condition=condition)
    assert not fs.exists(path)


@pytest.mark.parametrize("mode", ["rb", "wb", "ab"])
def test_open_rejects_conditions(filesystem, mode):
    fs, path = filesystem
    with pytest.raises((TypeError, NotImplementedError), match="condition"):
        fs.open(path, mode, condition=Missing())
    assert not fs.exists(path)


def test_conditions_are_typed_immutable_values():
    validator = Validator("s3://endpoint/bucket/key", "opaque")
    assert validator.scope == "s3://endpoint/bucket/key"
    assert validator.value == "opaque"
    assert validator == Validator(validator.scope, validator.value)
    condition = Match(validator)
    assert condition.validator == validator
    assert condition == Match(validator)
    assert Missing() == Missing()
    for target, attribute, value in [(validator, "value", "replacement"), (condition, "validator", Validator("other", "version"))]:
        with pytest.raises(AttributeError):
            setattr(target, attribute, value)
    invalid: Any = "etag"
    with pytest.raises(TypeError):
        Match(invalid)
    assert issubclass(ConflictError, OSError)
    assert not issubclass(ConflictError, FileExistsError)


@pytest.mark.parametrize("fs_class", [S3FileSystem, RustS3Fs])
def test_s3_capabilities_and_condition_validation_without_requests(fs_class):
    fs = fs_class(bucket="test", region="us-east-1", anon=True)
    capabilities = fs.io_capabilities()
    assert capabilities.read_match
    assert capabilities.write_match
    assert capabilities.write_missing
    with pytest.raises(TypeError, match="condition"):
        fs.read("key", condition="etag")
    with pytest.raises(TypeError, match="condition"):
        fs.write("key", b"x", condition="etag")
    with pytest.raises((TypeError, NotImplementedError), match="condition"):
        fs.open("key", "wb", condition=Missing())


@pytest.mark.parametrize("fs_class", [S3FileSystem, RustS3Fs])
def test_s3_validator_roundtrip_and_conflicts(s3_endpoint, fs_class):
    fs = fs_class(bucket="test", region="us-east-1", anon=True, endpoint_url=s3_endpoint)
    unconditional = fs.write("unconditional", b"data")
    assert isinstance(unconditional.validator, Validator)
    read_unconditional = fs.read("unconditional")
    assert read_unconditional.data == b"data"
    assert read_unconditional.validator == unconditional.validator
    created = fs.write("key", b"first", condition=Missing())
    assert isinstance(created.validator, Validator)
    read = fs.read("key", condition=Match(created.validator))
    assert read.data == b"first"
    assert read.validator == created.validator
    updated = fs.write("key", b"second", condition=Match(read.validator))
    assert isinstance(updated.validator, Validator)
    assert updated.validator != read.validator
    assert bytes(fs.cat_file("key", condition=Match(updated.validator))) == b"second"
    for read_bytes in (fs.read, fs.cat_file):
        with pytest.raises(ConflictError):
            read_bytes("key", condition=Match(created.validator))
    for write in (fs.write, fs.pipe_file):
        with pytest.raises(ConflictError):
            write("key", b"stale", condition=Match(created.validator))
        with pytest.raises(ConflictError):
            write("key", b"duplicate", condition=Missing())
    with pytest.raises(ValueError):
        fs.write("other", b"wrong scope", condition=Match(updated.validator))
    with pytest.raises(ValueError, match="Missing"):
        fs.read("key", condition=Missing())
    with pytest.raises(ConflictError):
        fs.read("key", start=0, end=0, condition=Match(created.validator))
    empty = fs.read("key", start=0, end=0, condition=Match(updated.validator))
    assert empty.data == b""
    assert empty.validator == updated.validator
    assert fs.read("key").data == b"second"


@pytest.mark.parametrize("fs_class", [LocalFileSystem, MemoryFileSystem, S3FileSystem])
@pytest.mark.parametrize(
    ("method", "args"),
    [
        ("write_text", ("source", "replacement")),
        ("read_text", ("source",)),
        ("touch", ("source",)),
        ("copy", ("source", "destination")),
        ("cp_file", ("source", "destination")),
        ("mv", ("source", "destination")),
        ("put_file", ("source", "destination")),
        ("get_file", ("source", "destination")),
        ("mkdir", ("destination",)),
        ("ls", ("source",)),
        ("info", ("source",)),
        ("exists", ("source",)),
        ("walk", ("source",)),
        ("find", ("source",)),
        ("du", ("source",)),
        ("cat", ("source",)),
        ("pipe", ({"source": b"replacement"},)),
        ("read_bytes", ("source",)),
        ("write_bytes", ("source", b"replacement")),
        ("cp", ("source", "destination")),
        ("move", ("source", "destination")),
        ("rename", ("source", "destination")),
        ("get", ("source", "destination")),
        ("download", ("source", "destination")),
        ("put", ("source", "destination")),
        ("upload", ("source", "destination")),
        ("glob", ("source*",)),
        ("expand_path", ("source",)),
        ("listdir", ("source",)),
        ("stat", ("source",)),
        ("lexists", ("source",)),
        ("makedir", ("destination",)),
        ("disk_usage", ("source",)),
        ("sign", ("source",)),
        ("rm", ("source",)),
        ("rm_file", ("source",)),
        ("rmdir", ("source",)),
        ("delete", ("source",)),
    ],
)
def test_unsupported_helpers_reject_conditions_before_io(fs_class, method, args, monkeypatch):
    class NoIo:
        def __getattr__(self, name):
            pytest.fail(f"unexpected native operation: {name}")

    fs = fs_class(bucket="test", anon=True, region="us-east-1") if fs_class is S3FileSystem else fs_class()
    monkeypatch.setattr(fs, "_rust", NoIo())
    with pytest.raises((NotImplementedError, TypeError), match="condition"):
        result = getattr(fs, method)(*args, condition=Missing())
        if method == "walk":
            list(result)


@pytest.mark.parametrize("fs_class", [LocalFileSystem, MemoryFileSystem])
def test_unsupported_write_helpers_preserve_existing_bytes(fs_class, tmp_path):
    fs = fs_class()
    path = str(tmp_path / "existing") if fs_class is LocalFileSystem else f"/conditional-{uuid.uuid4().hex}"
    fs.write(path, b"original")
    with pytest.raises(NotImplementedError, match="condition"):
        fs.write_text(path, "replacement", condition=Missing())
    with pytest.raises(NotImplementedError, match="condition"):
        fs.touch(path, truncate=True, condition=Match(Validator("scope", "version")))
    assert fs.read(path).data == b"original"
