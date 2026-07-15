# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import pytest

from solstone.think.providers import openhands
from solstone.think.providers.cli import ProviderKeyMissingError, QuotaExhaustedError
from solstone.think.providers.shared import classify_provider_error


def _require_attrs(module, *names: str):
    missing = [name for name in names if not hasattr(module, name)]
    if missing:
        pytest.skip(f"{module.__name__} does not expose {', '.join(missing)}")
    return [getattr(module, name) for name in names]


def test_classifies_quota_exhausted_error():
    exc = QuotaExhaustedError("quota exhausted", retry_delay_ms=1000)
    assert classify_provider_error(exc, "google") == "provider_quota_exceeded"


def test_provider_key_missing_reason_passes_through():
    exc = ProviderKeyMissingError("google", "GOOGLE_API_KEY", "msg")
    assert classify_provider_error(exc, "google") == "provider_key_missing"

    wrapped = RuntimeError("wrapper")
    wrapped.__cause__ = exc

    assert (
        classify_provider_error(openhands._unwrap_provider_exception(wrapped), "google")
        == "provider_key_missing"
    )


def test_classifies_litellm_context_window_exceeded():
    from litellm.exceptions import ContextWindowExceededError

    exc = ContextWindowExceededError(
        "context window exceeded",
        model="gpt-5",
        llm_provider="openai",
    )

    assert classify_provider_error(exc, "openhands") == "context_window_exceeded"


@pytest.mark.parametrize(
    "message",
    [
        "request (16942 tokens) exceeds the available context size (16384 tokens)",
        "prompt exceeds the context window for this model",
        "This model's maximum context length is 16384 tokens",
        "context length exceeded while evaluating prompt",
    ],
)
def test_classifies_litellm_bad_request_context_window_messages(message):
    from litellm.exceptions import BadRequestError

    exc = BadRequestError(message, model="local-model", llm_provider="local")

    assert classify_provider_error(exc, "local") == "context_window_exceeded"


def test_classifies_litellm_bad_request_unrelated_unknown():
    from litellm.exceptions import BadRequestError

    exc = BadRequestError(
        "Invalid value for parameter 'temperature'",
        model="local-model",
        llm_provider="local",
    )

    assert classify_provider_error(exc, "local") == "unknown"
    assert classify_provider_error(exc, "local") != "context_window_exceeded"


def test_classifies_max_turns_exhausted():
    from solstone.think.cogitate_policy import MaxTurnsExhausted

    exc = MaxTurnsExhausted("max_turns_exhausted: OpenHands cogitate exceeded 60 turns")

    assert classify_provider_error(exc, "openhands") == "max_turns_exhausted"


def test_classifies_builtin_connection_error():
    assert classify_provider_error(ConnectionError("offline"), "google") == (
        "network_unreachable"
    )


def test_classifies_no_response_value_error():
    exc = ValueError("No response from model")
    assert classify_provider_error(exc, "google") == "provider_response_invalid"


def test_classifies_generic_exception_unknown():
    assert classify_provider_error(Exception("anything"), "google") == "unknown"


@pytest.mark.parametrize(
    ("message", "expected"),
    [
        ("Gemini CLI not installed", "provider_unavailable"),
        ("command not found: gemini", "provider_unavailable"),
        ("timed out after 30s", "chat_timeout"),
        ("authentication failed", "provider_key_invalid"),
        ("unauthorized 401", "provider_key_invalid"),
        ("unexpected failure", "unknown"),
    ],
)
def test_classifies_cli_runtime_patterns(message, expected):
    assert classify_provider_error(RuntimeError(message), "google") == expected


def test_classifies_anthropic_sdk_auth_and_permission_errors():
    anthropic = pytest.importorskip("anthropic")
    httpx = pytest.importorskip("httpx")
    authentication_error, permission_denied_error = _require_attrs(
        anthropic, "AuthenticationError", "PermissionDeniedError"
    )
    request = httpx.Request("GET", "https://api.anthropic.com")

    for cls, status_code in (
        (authentication_error, 401),
        (permission_denied_error, 403),
    ):
        response = httpx.Response(status_code, request=request)
        exc = cls("auth failed", response=response, body={})
        assert classify_provider_error(exc, "anthropic") == "provider_key_invalid"


def test_classifies_anthropic_sdk_rate_timeout_network_and_5xx_errors():
    anthropic = pytest.importorskip("anthropic")
    httpx = pytest.importorskip("httpx")
    (
        rate_limit_error,
        api_timeout_error,
        api_connection_error,
        api_status_error,
    ) = _require_attrs(
        anthropic,
        "RateLimitError",
        "APITimeoutError",
        "APIConnectionError",
        "APIStatusError",
    )
    request = httpx.Request("GET", "https://api.anthropic.com")

    rate_response = httpx.Response(429, request=request)
    rate_exc = rate_limit_error("rate limited", response=rate_response, body={})
    assert classify_provider_error(rate_exc, "anthropic") == ("provider_quota_exceeded")

    assert classify_provider_error(api_timeout_error(request), "anthropic") == (
        "chat_timeout"
    )
    assert (
        classify_provider_error(api_connection_error(request=request), "anthropic")
        == "network_unreachable"
    )

    server_response = httpx.Response(503, request=request)
    server_exc = api_status_error("server failed", response=server_response, body={})
    assert classify_provider_error(server_exc, "anthropic") == "provider_unavailable"


def test_classifies_openai_sdk_auth_and_permission_errors():
    openai = pytest.importorskip("openai")
    httpx = pytest.importorskip("httpx")
    request = httpx.Request("GET", "https://api.openai.com")

    for cls, status_code in (
        (openai.AuthenticationError, 401),
        (openai.PermissionDeniedError, 403),
    ):
        response = httpx.Response(status_code, request=request)
        exc = cls("auth failed", response=response, body={})
        assert classify_provider_error(exc, "openai") == "provider_key_invalid"


def test_classifies_openai_sdk_rate_timeout_network_and_5xx_errors():
    openai = pytest.importorskip("openai")
    httpx = pytest.importorskip("httpx")
    request = httpx.Request("GET", "https://api.openai.com")

    rate_response = httpx.Response(429, request=request)
    rate_exc = openai.RateLimitError("rate limited", response=rate_response, body={})
    assert classify_provider_error(rate_exc, "openai") == "provider_quota_exceeded"

    assert classify_provider_error(openai.APITimeoutError(request), "openai") == (
        "chat_timeout"
    )
    assert (
        classify_provider_error(openai.APIConnectionError(request=request), "openai")
        == "network_unreachable"
    )

    server_response = httpx.Response(500, request=request)
    server_exc = openai.InternalServerError(
        "server failed", response=server_response, body={}
    )
    assert classify_provider_error(server_exc, "openai") == "provider_unavailable"


def test_classifies_google_sdk_errors():
    errors = pytest.importorskip("google.genai.errors")

    auth_exc = errors.ClientError(
        401,
        {"error": {"status": "UNAUTHENTICATED", "message": "bad key"}},
    )
    assert classify_provider_error(auth_exc, "google") == "provider_key_invalid"

    rate_exc = errors.ClientError(
        429,
        {"error": {"status": "RESOURCE_EXHAUSTED", "message": "quota"}},
    )
    assert classify_provider_error(rate_exc, "google") == "provider_quota_exceeded"

    server_exc = errors.ServerError(
        503,
        {"error": {"status": "UNAVAILABLE", "message": "down"}},
    )
    assert classify_provider_error(server_exc, "google") == "provider_unavailable"


def test_classifies_httpx_errors():
    httpx = pytest.importorskip("httpx")
    request = httpx.Request("GET", "http://localhost:11434")

    assert (
        classify_provider_error(
            httpx.ConnectError("connect failed", request=request), "local"
        )
        == "network_unreachable"
    )
    assert (
        classify_provider_error(httpx.ReadTimeout("timeout", request=request), "local")
        == "chat_timeout"
    )

    response = httpx.Response(503, request=request)
    status_exc = httpx.HTTPStatusError(
        "server failed", request=request, response=response
    )
    assert classify_provider_error(status_exc, "local") == "provider_unavailable"


def test_classifies_local_reason_code():
    from solstone.think.providers.local import LocalProviderError

    assert (
        classify_provider_error(LocalProviderError("model_missing", "missing"), "local")
        == "model_missing"
    )
