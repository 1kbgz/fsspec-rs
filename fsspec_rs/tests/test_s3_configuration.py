"""S3 configuration is consistent through direct and C-linked Python routes."""

from threading import Event

import pytest
from fsspec.config import conf

from fsspec_rs import Missing, RustS3Fs, S3FileSystem, UnavailableError, UnknownOutcomeError


@pytest.fixture(params=[S3FileSystem, RustS3Fs])
def s3_class(request, monkeypatch):
    monkeypatch.setitem(conf, "s3", {})
    for key in ("AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_SESSION_TOKEN", "AWS_REGION", "AWS_DEFAULT_REGION"):
        monkeypatch.delenv(key, raising=False)
    return request.param


def capture_read(s3_endpoint, monkeypatch):
    headers = []
    original = s3_endpoint.handler.do_GET
    s3_endpoint.objects["/test/value"] = (b"value", '"version"')

    def read(handler):
        headers.append(dict(handler.headers))
        original(handler)

    monkeypatch.setattr(s3_endpoint.handler, "do_GET", read)
    return headers


@pytest.mark.parametrize("explicit", [False, True])
def test_credentials_and_region_precedence(s3_class, s3_endpoint, monkeypatch, explicit):
    headers = capture_read(s3_endpoint, monkeypatch)
    for key, value in {
        "AWS_ACCESS_KEY_ID": "env-key",
        "AWS_SECRET_ACCESS_KEY": "env-secret",
        "AWS_SESSION_TOKEN": "env-token",
        "AWS_REGION": "env-region",
        "AWS_DEFAULT_REGION": "default-region",
    }.items():
        monkeypatch.setenv(key, value)
    options = {"key": "explicit-key", "secret": "explicit-secret", "region": "explicit-region"} if explicit else {}
    fs = s3_class(bucket="test", endpoint_url=s3_endpoint, **options)
    assert fs.read("test/value").data == b"value"
    signed = {key.lower(): value for key, value in headers[-1].items()}
    assert f"Credential={'explicit-key' if explicit else 'env-key'}/" in signed["authorization"]
    assert f"/{'explicit-region' if explicit else 'env-region'}/s3/aws4_request" in signed["authorization"]
    assert signed.get("x-amz-security-token") == (None if explicit else "env-token")


def test_default_region_and_explicit_token(s3_class, s3_endpoint, monkeypatch):
    headers = capture_read(s3_endpoint, monkeypatch)
    monkeypatch.setenv("AWS_DEFAULT_REGION", "fallback-region")
    fs = s3_class(bucket="test", endpoint_url=s3_endpoint, key="key", secret="secret", token="explicit-token")
    fs.read("test/value")
    signed = {key.lower(): value for key, value in headers[-1].items()}
    assert "/fallback-region/s3/aws4_request" in signed["authorization"]
    assert signed["x-amz-security-token"] == "explicit-token"


def test_anonymous_ignores_incomplete_credentials(s3_class, s3_endpoint, monkeypatch):
    headers = capture_read(s3_endpoint, monkeypatch)
    monkeypatch.setenv("AWS_ACCESS_KEY_ID", "incomplete-env-key")
    fs = s3_class(bucket="test", endpoint_url=s3_endpoint, key="incomplete-explicit-key", anon=True)
    fs.read("test/value")
    assert "authorization" not in {key.lower() for key in headers[-1]}


@pytest.mark.parametrize("credentials", [{"key": "explicit"}, {"secret": "explicit"}, {"token": "explicit"}])
def test_partial_explicit_credentials_do_not_mix_with_environment(s3_class, s3_endpoint, monkeypatch, credentials):
    monkeypatch.setenv("AWS_ACCESS_KEY_ID", "env-key")
    monkeypatch.setenv("AWS_SECRET_ACCESS_KEY", "env-secret")
    with pytest.raises(ValueError, match="both key and secret"):
        s3_class(bucket="test", endpoint_url=s3_endpoint, **credentials)


def test_python_config_credentials_do_not_supply_explicit_token(s3_endpoint, monkeypatch):
    monkeypatch.setitem(conf, "s3", {"key": "config-key", "secret": "config-secret", "token": "config-token"})
    headers = capture_read(s3_endpoint, monkeypatch)
    fs = S3FileSystem(bucket="test", endpoint_url=s3_endpoint, key="explicit-key", secret="explicit-secret")
    fs.read("test/value")
    signed = {key.lower(): value for key, value in headers[-1].items()}
    assert "Credential=explicit-key/" in signed["authorization"]
    assert "x-amz-security-token" not in signed


@pytest.mark.parametrize("retries", [0, 2])
def test_read_retry_limit(s3_class, s3_endpoint, monkeypatch, retries):
    attempts = []

    def unavailable(handler):
        attempts.append(handler.path)
        handler.respond(503)

    monkeypatch.setattr(s3_endpoint.handler, "do_GET", unavailable)
    fs = s3_class(bucket="test", endpoint_url=s3_endpoint, anon=True, client_options={"max_retries": retries})
    with pytest.raises(UnavailableError) as caught:
        fs.read("test/value")
    assert caught.value.retry_safe
    assert len(attempts) == retries + 1


def test_request_timeout(s3_class, s3_endpoint, monkeypatch):
    release = Event()

    def slow(handler):
        release.wait(timeout=2)
        handler.close_connection = True

    monkeypatch.setattr(s3_endpoint.handler, "do_GET", slow)
    fs = s3_class(
        bucket="test", endpoint_url=s3_endpoint, anon=True, client_options={"request_timeout": 0.05, "connect_timeout": 0.5, "max_retries": 0}
    )
    try:
        with pytest.raises(TimeoutError) as caught:
            fs.read("test/value")
        assert caught.value.kind == "timeout"
        assert caught.value.retry_safe
    finally:
        release.set()


def test_write_retry_setting_cannot_repeat_uncertain_create(s3_class, s3_endpoint, monkeypatch):
    attempts = []

    def uncertain(handler):
        attempts.append(handler.rfile.read(int(handler.headers["Content-Length"])))
        handler.respond(500)

    monkeypatch.setattr(s3_endpoint.handler, "do_PUT", uncertain)
    fs = s3_class(bucket="test", endpoint_url=s3_endpoint, anon=True, client_options={"max_retries": 3})
    with pytest.raises(UnknownOutcomeError):
        fs.write("test/manifest", b"manifest", condition=Missing())
    assert attempts == [b"manifest"]


@pytest.mark.parametrize(
    "options",
    [
        {"connect_timeout": 0},
        {"request_timeout": -1},
        {"request_timeout": float("nan")},
        {"request_timeout": float("inf")},
        {"request_timeout": "1"},
        {"max_retries": -1},
        {"max_retries": 1.5},
        {"max_retries": True},
        {"unknown": 1},
    ],
)
def test_invalid_client_options_fail_before_io(s3_class, s3_endpoint, options):
    with pytest.raises((ValueError, TypeError, OverflowError)):
        s3_class(bucket="test", endpoint_url=s3_endpoint, anon=True, client_options=options)
    assert not s3_endpoint.requests
