# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from pathlib import Path

import httpx
import openai
import pytest

from solstone.think.providers.shared import (
    QuotaExhaustedError,
    RUNTIME_REASON_CODES,
    classify_provider_error,
    mark_cloud_model_request,
)


def _response(status_code: int) -> httpx.Response:
    request = httpx.Request("GET", "https://api.openai.example/models/missing")
    return httpx.Response(status_code, request=request)


def test_classifies_quota_exhausted_error() -> None:
    exc = QuotaExhaustedError("quota exhausted", retry_delay_ms=1000)
    assert classify_provider_error(exc, "google") == "provider_quota_exceeded"


@pytest.mark.parametrize(
    "message",
    [
        "request exceeds the available context size",
        "prompt exceeds the context window for this model",
        "maximum context length is 16384 tokens",
    ],
)
def test_context_window_messages_classify_without_provider_wrapper(message: str) -> None:
    assert classify_provider_error(RuntimeError(message), "google") == "context_window_exceeded"


def test_marked_cloud_model_request_404_classifies_model_not_found() -> None:
    class ProviderModelLookupError(Exception):
        status_code = 404

    exc = ProviderModelLookupError("missing model")
    mark_cloud_model_request(exc)
    assert classify_provider_error(exc, "google") == "model_not_found"
    assert classify_provider_error(exc, "local") == "unknown"


def test_openai_sdk_exceptions_keep_cloud_classifications() -> None:
    cases = [
        (
            openai.AuthenticationError("bad key", response=_response(401), body=None),
            "provider_key_invalid",
        ),
        (
            openai.RateLimitError("rate limit", response=_response(429), body=None),
            "provider_quota_exceeded",
        ),
        (
            openai.NotFoundError("model not found", response=_response(404), body=None),
            "model_not_found",
        ),
    ]
    for exc, expected in cases:
        assert classify_provider_error(exc, "openai") == expected


def test_httpx_errors_keep_transport_classifications() -> None:
    assert classify_provider_error(httpx.ReadTimeout("timeout"), "google") == "chat_timeout"
    assert (
        classify_provider_error(httpx.ConnectError("connection failed"), "google")
        == "network_unreachable"
    )


def test_runtime_reason_codes_remain_projected_to_chat() -> None:
    from solstone.convey import provider_readiness

    projection = provider_readiness.chat_reason_projection()
    chat_reasons = Path("solstone/convey/static/chat_reasons.js").read_text(
        encoding="utf-8"
    )
    for reason_code in ("context_window_exceeded", "model_not_found"):
        assert reason_code in RUNTIME_REASON_CODES
        assert reason_code in projection
        assert f'"{reason_code}"' in chat_reasons
