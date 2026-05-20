# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared utilities and types for AI providers.

This module contains:
- Event TypedDicts emitted by providers during talent execution
- GenerateResult TypedDict returned by run_generate/run_agenerate
- JSONEventCallback for event emission
- Utility functions for common provider operations
"""

from __future__ import annotations

import importlib
import json
from typing import Any, Callable, Literal, Optional, Union

from typing_extensions import Required, TypedDict

from solstone.think.utils import now_ms

# ---------------------------------------------------------------------------
# Event Types
# ---------------------------------------------------------------------------


class ToolStartEvent(TypedDict, total=False):
    """Event emitted when a tool starts."""

    event: Literal["tool_start"]
    ts: int
    tool: str
    args: Optional[dict[str, Any]]
    call_id: Optional[str]  # Unique ID to pair with tool_end event
    raw: Optional[list[dict[str, Any]]]  # Original provider JSON event(s)


class ToolEndEvent(TypedDict, total=False):
    """Event emitted when a tool finishes."""

    event: Literal["tool_end"]
    ts: int
    tool: str
    args: Optional[dict[str, Any]]
    result: Any
    call_id: Optional[str]  # Matches the call_id from tool_start
    raw: Optional[list[dict[str, Any]]]  # Original provider JSON event(s)


class StartEvent(TypedDict, total=False):
    """Event emitted when a talent run begins."""

    event: Required[Literal["start"]]
    ts: Required[int]
    prompt: Required[str]
    name: Required[str]
    model: Required[str]
    provider: Required[str]
    session_id: Optional[str]  # solstone-owned session ID for continuation
    chat_id: Optional[str]  # Chat ID for reverse lookup
    raw: Optional[list[dict[str, Any]]]  # Original provider JSON event(s)


class FinishEvent(TypedDict, total=False):
    """Event emitted when a talent run finishes successfully."""

    event: Required[Literal["finish"]]
    ts: Required[int]
    result: Required[str]
    usage: Optional[dict[str, Any]]
    cli_session_id: Optional[
        str
    ]  # solstone-owned session ID persisted under journal/.cache/cogitate-history/
    raw: Optional[list[dict[str, Any]]]  # Original provider JSON event(s)


class ErrorEvent(TypedDict, total=False):
    """Event emitted when an error occurs."""

    event: Literal["error"]
    ts: int
    error: str
    trace: Optional[str]
    raw: Optional[list[dict[str, Any]]]  # Original provider JSON event(s)


class TalentUpdatedEvent(TypedDict, total=False):
    """Event emitted when the talent context changes."""

    event: Required[Literal["talent_updated"]]
    ts: Required[int]
    talent: Required[str]
    raw: Optional[list[dict[str, Any]]]  # Original provider JSON event(s)


class ThinkingEvent(TypedDict, total=False):
    """Event emitted when thinking/reasoning summaries are available.

    For Anthropic models, may include a signature for verification when
    passing thinking blocks back during tool use continuations.
    For redacted thinking, summary will contain "[redacted]" and
    redacted_data will contain the encrypted content.
    """

    event: Required[Literal["thinking"]]
    ts: Required[int]
    summary: Required[str]
    model: Optional[str]
    signature: Optional[str]  # Anthropic thinking block signature
    redacted_data: Optional[str]  # Encrypted data for redacted thinking
    raw: Optional[list[dict[str, Any]]]  # Original provider JSON event(s)


class TextDeltaEvent(TypedDict, total=False):
    """Event emitted when streamed text content is available."""

    event: Required[Literal["text_delta"]]
    ts: Required[int]
    delta: Required[str]
    model: Optional[str]
    raw: Optional[list[dict[str, Any]]]  # Original provider JSON event(s)


class FallbackEvent(TypedDict, total=False):
    """Event emitted when provider fallback occurs."""

    event: Required[Literal["fallback"]]
    ts: Required[int]
    original_provider: Required[str]
    backup_provider: Required[str]
    reason: Required[str]  # "preflight" or "on_failure"
    error: Optional[str]  # Error message for on_failure case


Event = Union[
    ToolStartEvent,
    ToolEndEvent,
    StartEvent,
    FinishEvent,
    ErrorEvent,
    ThinkingEvent,
    TextDeltaEvent,
    TalentUpdatedEvent,
    FallbackEvent,
]


# ---------------------------------------------------------------------------
# Provider Error Classification
# ---------------------------------------------------------------------------

_CLI_UNAVAILABLE_PATTERNS = ("not installed", "command not found", "missing")
_CLI_TIMEOUT_PATTERNS = ("timed out", "timeout")
_CLI_AUTH_PATTERNS = (
    "authentication",
    "unauthorized",
    " 401",
    " 403",
    "401 ",
    "403 ",
    "401:",
    "403:",
    "permission denied",
    "forbidden",
    "invalid api key",
)


def _import_exception_type(module_name: str, name: str) -> type[BaseException] | None:
    try:
        module = importlib.import_module(module_name)
    except ImportError:
        return None
    value = getattr(module, name, None)
    if isinstance(value, type) and issubclass(value, BaseException):
        return value
    return None


_ANTHROPIC_API_STATUS_ERROR = _import_exception_type("anthropic", "APIStatusError")
_ANTHROPIC_API_CONNECTION_ERROR = _import_exception_type(
    "anthropic", "APIConnectionError"
)
_ANTHROPIC_API_TIMEOUT_ERROR = _import_exception_type("anthropic", "APITimeoutError")
_ANTHROPIC_AUTHENTICATION_ERROR = _import_exception_type(
    "anthropic", "AuthenticationError"
)
_ANTHROPIC_PERMISSION_DENIED_ERROR = _import_exception_type(
    "anthropic", "PermissionDeniedError"
)
_ANTHROPIC_RATE_LIMIT_ERROR = _import_exception_type("anthropic", "RateLimitError")

_OPENAI_API_STATUS_ERROR = _import_exception_type("openai", "APIStatusError")
_OPENAI_API_CONNECTION_ERROR = _import_exception_type("openai", "APIConnectionError")
_OPENAI_API_TIMEOUT_ERROR = _import_exception_type("openai", "APITimeoutError")
_OPENAI_AUTHENTICATION_ERROR = _import_exception_type("openai", "AuthenticationError")
_OPENAI_PERMISSION_DENIED_ERROR = _import_exception_type(
    "openai", "PermissionDeniedError"
)
_OPENAI_RATE_LIMIT_ERROR = _import_exception_type("openai", "RateLimitError")
_OPENAI_INTERNAL_SERVER_ERROR = _import_exception_type("openai", "InternalServerError")

_GOOGLE_CLIENT_ERROR = _import_exception_type("google.genai.errors", "ClientError")
_GOOGLE_SERVER_ERROR = _import_exception_type("google.genai.errors", "ServerError")
_GOOGLE_UNKNOWN_RESPONSE_ERROR = _import_exception_type(
    "google.genai.errors", "UnknownApiResponseError"
)

_HTTPX_HTTP_STATUS_ERROR = _import_exception_type("httpx", "HTTPStatusError")
_HTTPX_NETWORK_ERROR = _import_exception_type("httpx", "NetworkError")
_HTTPX_REQUEST_ERROR = _import_exception_type("httpx", "RequestError")
_HTTPX_TIMEOUT_EXCEPTION = _import_exception_type("httpx", "TimeoutException")


def _isinstance(exc: BaseException, cls: type[BaseException] | None) -> bool:
    return cls is not None and isinstance(exc, cls)


def _status_code(exc: BaseException) -> int | None:
    for attr in ("status_code", "code"):
        value = getattr(exc, attr, None)
        if isinstance(value, int):
            return value
    response = getattr(exc, "response", None)
    value = getattr(response, "status_code", None)
    return value if isinstance(value, int) else None


def _status_text(exc: BaseException) -> str:
    return str(getattr(exc, "status", "") or "").upper()


def _contains_any(text: str, patterns: tuple[str, ...]) -> bool:
    return any(pattern in text for pattern in patterns)


def classify_provider_error(exc: BaseException, provider: str) -> str:
    """Return a chat reason code for a provider exception."""
    try:
        exc_name = type(exc).__name__
        exc_name_lower = exc_name.lower()
        message_lower = str(exc).lower()

        if exc_name == "QuotaExhaustedError":
            return "provider_quota_exceeded"

        if isinstance(exc, ValueError) and "no response from model" in message_lower:
            return "provider_response_invalid"

        if _isinstance(exc, _ANTHROPIC_AUTHENTICATION_ERROR) or _isinstance(
            exc, _ANTHROPIC_PERMISSION_DENIED_ERROR
        ):
            return "provider_key_invalid"
        if _isinstance(exc, _OPENAI_AUTHENTICATION_ERROR) or _isinstance(
            exc, _OPENAI_PERMISSION_DENIED_ERROR
        ):
            return "provider_key_invalid"
        if _isinstance(exc, _GOOGLE_CLIENT_ERROR) and _status_code(exc) in (401, 403):
            return "provider_key_invalid"

        if _isinstance(exc, _ANTHROPIC_RATE_LIMIT_ERROR) or _isinstance(
            exc, _OPENAI_RATE_LIMIT_ERROR
        ):
            return "provider_quota_exceeded"
        if _isinstance(exc, _GOOGLE_CLIENT_ERROR) and (
            _status_code(exc) == 429 or _status_text(exc) == "RESOURCE_EXHAUSTED"
        ):
            return "provider_quota_exceeded"

        if _isinstance(exc, _ANTHROPIC_API_TIMEOUT_ERROR) or _isinstance(
            exc, _OPENAI_API_TIMEOUT_ERROR
        ):
            return "chat_timeout"
        if _isinstance(exc, _HTTPX_TIMEOUT_EXCEPTION):
            return "chat_timeout"

        if _isinstance(exc, _ANTHROPIC_API_CONNECTION_ERROR) or _isinstance(
            exc, _OPENAI_API_CONNECTION_ERROR
        ):
            return "network_unreachable"
        if _isinstance(exc, _HTTPX_NETWORK_ERROR) or _isinstance(
            exc, _HTTPX_REQUEST_ERROR
        ):
            return "network_unreachable"
        if isinstance(exc, ConnectionError):
            return "network_unreachable"

        if _isinstance(exc, _OPENAI_INTERNAL_SERVER_ERROR) or _isinstance(
            exc, _GOOGLE_SERVER_ERROR
        ):
            return "provider_unavailable"
        if (
            _isinstance(exc, _ANTHROPIC_API_STATUS_ERROR)
            or _isinstance(exc, _OPENAI_API_STATUS_ERROR)
            or _isinstance(exc, _HTTPX_HTTP_STATUS_ERROR)
        ) and (_status_code(exc) or 0) >= 500:
            return "provider_unavailable"

        if _isinstance(exc, _GOOGLE_UNKNOWN_RESPONSE_ERROR):
            return "provider_response_invalid"

        if isinstance(exc, RuntimeError):
            if _contains_any(message_lower, _CLI_UNAVAILABLE_PATTERNS):
                return "provider_unavailable"
            if _contains_any(message_lower, _CLI_TIMEOUT_PATTERNS):
                return "chat_timeout"
            if _contains_any(message_lower, _CLI_AUTH_PATTERNS):
                return "provider_key_invalid"
            return "unknown"

        if (
            "authenticationerror" in exc_name_lower
            or "permissiondeniederror" in exc_name_lower
            or "unauthorized" in exc_name_lower
            or "forbidden" in exc_name_lower
        ):
            return "provider_key_invalid"
        if (
            "ratelimit" in exc_name_lower
            or "toomanyrequests" in exc_name_lower
            or "resourceexhausted" in exc_name_lower
        ):
            return "provider_quota_exceeded"
        if "timeout" in exc_name_lower:
            return "chat_timeout"
        if "connection" in exc_name_lower or "network" in exc_name_lower:
            return "network_unreachable"
        if (
            "responsevalidation" in exc_name_lower
            or "unknownapiresponse" in exc_name_lower
        ):
            return "provider_response_invalid"
        if "internalservererror" in exc_name_lower or "servererror" in exc_name_lower:
            return "provider_unavailable"

        return "unknown"
    except Exception:
        return "unknown"


# ---------------------------------------------------------------------------
# Usage Schema
# ---------------------------------------------------------------------------

# Canonical keys for the normalized usage dict returned by all providers.
# log_token_usage() passes through exactly these keys (when present and non-zero).
USAGE_KEYS = frozenset(
    {
        "input_tokens",
        "output_tokens",
        "total_tokens",
        "cached_tokens",
        "reasoning_tokens",
        "cache_creation_tokens",
        "requests",
    }
)

# ---------------------------------------------------------------------------
# GenerateResult
# ---------------------------------------------------------------------------


class GenerateResult(TypedDict, total=False):
    """Result from provider run_generate/run_agenerate functions.

    Structured result that allows the wrapper to handle cross-cutting concerns
    like token logging and JSON validation centrally.

    The thinking field contains dicts with: summary (str), signature (optional str),
    redacted_data (optional str for Anthropic redacted thinking).
    """

    text: Required[str]  # Response text
    usage: Optional[dict]  # Normalized usage dict (input_tokens, output_tokens, etc.)
    finish_reason: Optional[str]  # Normalized: "stop", "max_tokens", "safety", etc.
    thinking: Optional[list]  # List of thinking block dicts
    schema_validation: Optional[dict]  # Validation result when json_schema is supplied


# ---------------------------------------------------------------------------
# JSONEventCallback
# ---------------------------------------------------------------------------


class JSONEventCallback:
    """Emit JSON events via a callback."""

    def __init__(self, callback: Optional[Callable[[Event], None]] = None) -> None:
        self.callback = callback

    def emit(self, data: Event) -> None:
        if "ts" not in data:
            data = {**data, "ts": now_ms()}
        if self.callback:
            self.callback(data)

    def close(self) -> None:
        pass


# ---------------------------------------------------------------------------
# Raw Event Trimming
# ---------------------------------------------------------------------------

# Structural keys preserved when trimming oversized raw events.
_RAW_STRUCTURAL_KEYS = frozenset(
    {
        "type",
        "id",
        "tool_id",
        "tool_name",
        "role",
        "event_type",
        "timestamp",
    }
)

_RAW_BYTE_LIMIT = 16_384  # 16 KB


def safe_raw(
    events: list[dict[str, Any]],
    limit: int = _RAW_BYTE_LIMIT,
) -> list[dict[str, Any]]:
    """Return *events* as-is if small enough, otherwise a trimmed version.

    When the JSON-serialized size exceeds *limit* bytes, each event is reduced
    to its structural keys and a ``_raw_trimmed`` dict is appended with the
    original byte count and the limit that was applied.
    """
    serialized = json.dumps(events, ensure_ascii=False)
    if len(serialized.encode("utf-8")) <= limit:
        return events

    trimmed = [
        {k: v for k, v in e.items() if k in _RAW_STRUCTURAL_KEYS} for e in events
    ]
    trimmed.append(
        {"_raw_trimmed": {"original_bytes": len(serialized), "limit": limit}}
    )
    return trimmed


__all__ = [
    "Event",
    "GenerateResult",
    "JSONEventCallback",
    "ThinkingEvent",
    "USAGE_KEYS",
    "classify_provider_error",
    "safe_raw",
]
