"""Uncertain S3 creates resolve by content, never by an ETag checksum assumption."""

import pytest

from fsspec_rs import ConflictError, Missing, RustS3Fs, S3FileSystem, UnknownOutcomeError


@pytest.fixture(params=[S3FileSystem, RustS3Fs])
def fs(request, s3_endpoint):
    return request.param(bucket="test", region="us-east-1", anon=True, endpoint_url=s3_endpoint)


@pytest.mark.parametrize("stored", [b"manifest", b"different", b"manifesz", None])
@pytest.mark.parametrize("failure", ["response", "disconnect"])
def test_uncertain_create_verifies_content(fs, s3_endpoint, monkeypatch, stored, failure):
    attempts = []

    def fail_put(handler):
        data = handler.rfile.read(int(handler.headers["Content-Length"]))
        attempts.append(data)
        if stored is not None:
            s3_endpoint.objects[handler.path] = (stored, '"opaque-not-a-checksum"')
        if failure == "disconnect":
            import socket

            handler.close_connection = True
            handler.connection.shutdown(socket.SHUT_RDWR)
            handler.connection.close()
        else:
            handler.respond(500)

    monkeypatch.setattr(s3_endpoint.handler, "do_PUT", fail_put)
    if stored == b"manifest":
        result = fs.write("test/manifest", b"manifest", condition=Missing())
        assert result.validator.value == '"opaque-not-a-checksum"'
    else:
        error = ConflictError if stored is not None else UnknownOutcomeError
        with pytest.raises(error) as caught:
            fs.write("test/manifest", b"manifest", condition=Missing())
        assert caught.value.kind == ("conflict" if stored is not None else "unknown_outcome")
        assert caught.value.retry_safe is False
    assert attempts == [b"manifest"]


def test_definitive_conflict_is_not_acknowledged_by_equal_content(fs):
    fs.write("test/existing", b"same")
    with pytest.raises(ConflictError) as caught:
        fs.write("test/existing", b"same", condition=Missing())
    assert caught.value.kind == "conflict"
    assert caught.value.retry_safe is False


def test_unresolved_verification_remains_unknown(fs, s3_endpoint, monkeypatch):
    def fail(handler):
        if handler.command == "PUT":
            handler.rfile.read(int(handler.headers["Content-Length"]))
        handler.respond(403)

    def ambiguous(handler):
        handler.rfile.read(int(handler.headers["Content-Length"]))
        handler.respond(500)

    monkeypatch.setattr(s3_endpoint.handler, "do_PUT", ambiguous)
    monkeypatch.setattr(s3_endpoint.handler, "do_GET", fail)
    with pytest.raises(UnknownOutcomeError) as caught:
        fs.write("test/manifest", b"manifest", condition=Missing())
    assert caught.value.retry_safe is False


def test_unconditional_uncertain_write_is_not_retried(fs, s3_endpoint, monkeypatch):
    attempts = []

    def ambiguous(handler):
        attempts.append(handler.rfile.read(int(handler.headers["Content-Length"])))
        handler.respond(500)

    monkeypatch.setattr(s3_endpoint.handler, "do_PUT", ambiguous)
    with pytest.raises(UnknownOutcomeError):
        fs.write("test/value", b"value")
    assert attempts == [b"value"]


def test_permission_error_has_metadata(fs, s3_endpoint, monkeypatch):
    def denied(handler):
        handler.rfile.read(int(handler.headers["Content-Length"]))
        handler.respond(403)

    monkeypatch.setattr(s3_endpoint.handler, "do_PUT", denied)
    with pytest.raises(PermissionError) as caught:
        fs.write("test/denied", b"value", condition=Missing())
    assert caught.value.kind == "permission_denied"
    assert caught.value.retry_safe is False
