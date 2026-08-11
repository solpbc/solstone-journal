# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared utilities and types for AI providers.

This module contains:
- Event TypedDicts emitted by providers during talent execution
- GenerateResult TypedDict used by native generate consumers
- JSONEventCallback for event emission
- Utility functions for common provider operations
"""

from __future__ import annotations

import json
import math
import re
from typing import Any, Callable, Literal, Mapping, Optional, Union

from typing_extensions import Required, TypedDict

from solstone.think.providers import is_cloud_provider
from solstone.think.responsiveness import NON_RESPONSIVE_REASON_CODE
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
    reason: Optional[str]
    reason_code: Optional[str]
    provider: Optional[str]
    trace: Optional[str]
    reset_at_ms: Optional[int]
    terminal: Optional[bool]
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


Event = Union[
    ToolStartEvent,
    ToolEndEvent,
    StartEvent,
    FinishEvent,
    ErrorEvent,
    ThinkingEvent,
    TextDeltaEvent,
    TalentUpdatedEvent,
]


class QuotaExhaustedError(Exception):
    """Raised when a provider reports quota exhaustion."""

    def __init__(self, message: str, retry_delay_ms: int | None = None) -> None:
        super().__init__(message)
        self.retry_delay_ms = retry_delay_ms


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
_CONTEXT_WINDOW_PATTERNS = (
    "exceeds the available context size",
    "context size has been exceeded",
    "exceeds the context window",
    "maximum context length",
    "longer than the model's context length",
    "context length exceeded",
)
_CLOUD_MODEL_REQUEST_ATTR = "_solstone_cloud_model_request"
_TRUSTED_MODEL_NOT_FOUND_MODULES = ("openai", "anthropic")


def _status_code(exc: BaseException) -> int | None:
    for attr in ("status_code", "_status_code", "code"):
        value = getattr(exc, attr, None)
        if isinstance(value, int):
            return value
    response = getattr(exc, "response", None)
    value = getattr(response, "status_code", None)
    return value if isinstance(value, int) else None


def _contains_any(text: str, patterns: tuple[str, ...]) -> bool:
    return any(pattern in text for pattern in patterns)


def _module_matches(module_name: str, package: str) -> bool:
    return module_name == package or module_name.startswith(f"{package}.")


def _exception_name_matches(
    exc_name: str, exc_qualname: str, names: tuple[str, ...]
) -> bool:
    return exc_name in names or any(exc_qualname.endswith(f".{name}") for name in names)


def exception_chain(exc: BaseException) -> list[BaseException]:
    chain: list[BaseException] = []
    current: BaseException | None = exc
    while current is not None and current not in chain:
        chain.append(current)
        current = current.__cause__ or current.__context__
    return chain


def _chain_has_status_code(exc: BaseException, code: int) -> bool:
    return any(_status_code(item) == code for item in exception_chain(exc))


def mark_cloud_model_request(exc: BaseException) -> None:
    # Wrapped transport exceptions may carry the status on an inner cause, so
    # mark the complete chain to retain cloud-request provenance.
    for item in exception_chain(exc):
        try:
            setattr(item, _CLOUD_MODEL_REQUEST_ATTR, True)
        except Exception:
            # SDK exceptions that reject attributes must not replace the real failure.
            continue


def has_cloud_model_request_mark(exc: BaseException) -> bool:
    return any(
        getattr(item, _CLOUD_MODEL_REQUEST_ATTR, False) is True
        for item in exception_chain(exc)
    )


def _is_trusted_model_not_found(exc: BaseException) -> bool:
    exc_type = type(exc)
    if "notfound" not in exc_type.__name__.lower():
        return False
    return any(
        _module_matches(exc_type.__module__, module)
        for module in _TRUSTED_MODEL_NOT_FOUND_MODULES
    )


# Missing-model classification first requires a built-in cloud provider. Trusted
# SDK NotFound-shaped exceptions identify provider model misses by class/module;
# otherwise a bare 404 is accepted only when narrow generate/probe transport
# provenance is present on the exception chain. Status alone is insufficient.
def is_cloud_model_not_found(exc: BaseException, provider: str) -> bool:
    if not is_cloud_provider(provider):
        return False
    return any(_is_trusted_model_not_found(item) for item in exception_chain(exc)) or (
        has_cloud_model_request_mark(exc) and _chain_has_status_code(exc, 404)
    )


RUNTIME_REASON_CODES = frozenset(
    {
        "context_window_exceeded",
        "context_budget_exceeded",
        "local_capacity_exhausted",
        "model_not_found",
        "provider_quota_exceeded",
        "provider_key_invalid",
        "chat_timeout",
        "local_queue_timeout",
        "max_turns_exhausted",
        "network_unreachable",
        "provider_request_rejected",
        "provider_unavailable",
        "provider_response_invalid",
        "incomplete_json_length",
        "incomplete_text_length",
        "unknown",
    }
)


def is_non_retryable_generate_reason(reason_code: str | None) -> bool:
    """Return True when retrying the same generate request cannot change outcome.

    Members are failures deterministic for the same request, so another attempt
    only burns quota. `schema_invalid` is deliberately not a member: the model
    may produce valid JSON on retry, preserving that high-volume retry path.
    """

    return reason_code == NON_RESPONSIVE_REASON_CODE


PROVIDER_ERROR_TEXT_CAP_CHARS = 4096


def classify_provider_error(exc: BaseException, provider: str) -> str:
    """Return a chat reason code for a provider exception."""
    try:
        exc_name = type(exc).__name__
        exc_qualname = type(exc).__qualname__
        exc_module = type(exc).__module__
        exc_name_lower = exc_name.lower()
        exc_identity_lower = f"{exc_module}.{exc_qualname}".lower()
        message_lower = str(exc).lower()
        explicit_reason_code = getattr(exc, "reason_code", None)
        if isinstance(explicit_reason_code, str) and explicit_reason_code:
            return explicit_reason_code

        if exc_name == "QuotaExhaustedError":
            return "provider_quota_exceeded"
        if exc_name == "ContextWindowExceededError":
            return "context_window_exceeded"
        if exc_name in {"LLMContextWindowExceedError", "LLMContextWindowTooSmallError"}:
            return "context_window_exceeded"
        if exc_name == "LLMAuthenticationError":
            return "provider_key_invalid"
        if exc_name == "LLMRateLimitError":
            return "provider_quota_exceeded"
        if exc_name == "LLMTimeoutError":
            return "chat_timeout"
        if exc_name == "LLMServiceUnavailableError":
            return "provider_unavailable"
        if exc_name in {"LLMNoResponseError", "LLMResponseError"}:
            return "provider_response_invalid"
        # Keep this message rescue ahead of provider-request rejection fallbacks.
        if _contains_any(message_lower, _CONTEXT_WINDOW_PATTERNS):
            return "context_window_exceeded"
        if isinstance(exc, ValueError) and "no response from model" in message_lower:
            return "provider_response_invalid"

        is_anthropic = _module_matches(exc_module, "anthropic")
        is_openai = _module_matches(exc_module, "openai")
        is_httpx = _module_matches(exc_module, "httpx")
        status_code = _status_code(exc)

        if (is_anthropic or is_openai) and _exception_name_matches(
            exc_name,
            exc_qualname,
            ("AuthenticationError", "PermissionDeniedError"),
        ):
            return "provider_key_invalid"

        if (is_anthropic or is_openai) and _exception_name_matches(
            exc_name, exc_qualname, ("RateLimitError",)
        ):
            return "provider_quota_exceeded"

        if (is_anthropic or is_openai) and _exception_name_matches(
            exc_name, exc_qualname, ("APITimeoutError",)
        ):
            return "chat_timeout"
        if is_httpx and (
            "timeout" in exc_name_lower
            or _exception_name_matches(
                exc_name,
                exc_qualname,
                (
                    "TimeoutException",
                    "ConnectTimeout",
                    "PoolTimeout",
                    "ReadTimeout",
                    "WriteTimeout",
                ),
            )
        ):
            return "chat_timeout"

        if (is_anthropic or is_openai) and _exception_name_matches(
            exc_name, exc_qualname, ("APIConnectionError",)
        ):
            return "network_unreachable"
        if is_httpx and (
            _exception_name_matches(
                exc_name,
                exc_qualname,
                ("NetworkError", "RequestError", "ConnectError"),
            )
            or "connection" in exc_name_lower
            or "connect" in exc_name_lower
        ):
            return "network_unreachable"
        if isinstance(exc, ConnectionError):
            return "network_unreachable"

        if is_openai and _exception_name_matches(
            exc_name, exc_qualname, ("InternalServerError",)
        ):
            return "provider_unavailable"
        if (
            (
                (is_anthropic or is_openai)
                and _exception_name_matches(exc_name, exc_qualname, ("APIStatusError",))
            )
            or (
                is_httpx
                and _exception_name_matches(
                    exc_name, exc_qualname, ("HTTPStatusError",)
                )
            )
        ) and (status_code or 0) >= 500:
            return "provider_unavailable"

        if is_cloud_model_not_found(exc, provider):
            return "model_not_found"

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
            or "unknownapiresponse" in exc_identity_lower
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

CANNED_GENERATE_PROMPT = "Reply with the single word OK."
CANNED_COGITATE_PROBE_PROMPT = (
    "Call the emit_final tool exactly once with the content OK. "
    "Do not reply with plain text and do not call any other tool."
)
CANNED_GENERATE_MAX_OUTPUT_TOKENS = 512
CANNED_GENERATE_THINKING_BUDGET = 0
CANNED_GENERATE_NUM_RETRIES = 0
CANNED_GENERATE_TIMEOUT_S = 30
CannedGenerateVerdict = Literal["pass", "starved", "invalid"]
_MAX_SAFE_TOKEN_COUNT = 1_000_000_000_000
_MAX_SAFE_FINISH_REASON_LENGTH = 64
_UNKNOWN_FINISH_REASON = "unknown"


class GenerateResult(TypedDict, total=False):
    """Result from the native generate boundary.

    The thinking field contains dicts with: summary (str), signature (optional str),
    redacted_data (optional str for Anthropic redacted thinking).
    """

    text: Required[str]  # Response text
    model: Optional[str]  # Resolved model identifier when provider returns one
    usage: Optional[dict]  # Normalized usage dict (input_tokens, output_tokens, etc.)
    finish_reason: Optional[str]  # Normalized: "stop", "max_tokens", "safety", etc.
    thinking: Optional[list]  # List of thinking block dicts
    schema_validation: Optional[dict]  # Validation result when json_schema is supplied
    input_budget: Optional[
        dict
    ]  # Out-of-band truncation metadata when the bundled-local input was clipped
    request_budget: Optional[dict]  # Per-request context/completion clamp facts
    inference: Optional[dict]  # Content-free local inference timing/admission record


def _has_reasoning_usage(result: GenerateResult) -> bool:
    thinking = result.get("thinking")
    if isinstance(thinking, list) and bool(thinking):
        return True
    usage = result.get("usage")
    if not isinstance(usage, dict):
        return False
    for key, value in usage.items():
        if "reasoning" not in str(key):
            continue
        if isinstance(value, bool):
            continue
        if isinstance(value, (int, float)) and value > 0:
            return True
    return False


def _is_blank_visible_output(result: GenerateResult) -> bool:
    text = result.get("text")
    return not isinstance(text, str) or not text.strip()


def _coerce_token_count(value: Any) -> int | None:
    if isinstance(value, bool):
        return None
    if not isinstance(value, (int, float)):
        return None
    if isinstance(value, float) and not math.isfinite(value):
        return None
    count = int(value)
    if count < 0:
        return None
    return min(count, _MAX_SAFE_TOKEN_COUNT)


def _safe_token_counts(usage: Any) -> dict[str, int]:
    if not isinstance(usage, Mapping):
        return {}

    token_counts: dict[str, int] = {}
    aliases = {
        "input_tokens": ("input_tokens", "prompt_tokens"),
        "output_tokens": ("output_tokens", "completion_tokens"),
        "reasoning_tokens": ("reasoning_tokens",),
        "total_tokens": ("total_tokens",),
    }
    for normalized_key, candidate_keys in aliases.items():
        for key in candidate_keys:
            if key not in usage:
                continue
            count = _coerce_token_count(usage.get(key))
            if count is not None:
                token_counts[normalized_key] = count
            break
    return token_counts


_SAFE_FINISH_REASON_PATTERN = re.compile(r"^[a-z0-9_]+$")


def _safe_finish_reason(value: Any) -> str | None:
    """Reduce a provider finish value to a bounded safe token, or None."""
    if not isinstance(value, str):
        return None
    if len(value) > _MAX_SAFE_FINISH_REASON_LENGTH:
        return _UNKNOWN_FINISH_REASON
    normalized = value.strip().lower()
    if _SAFE_FINISH_REASON_PATTERN.fullmatch(normalized):
        return normalized
    return _UNKNOWN_FINISH_REASON


def classify_canned_generate(result: GenerateResult) -> CannedGenerateVerdict:
    """Classify a canned generate result without using brain-state vocabulary.

    Brain refresh maps this later as: pass -> generate ok, starved ->
    probe_output_starved, invalid -> provider_response_invalid.
    """

    finish_reason = result.get("finish_reason")
    if finish_reason == "max_tokens":
        return "starved"

    if not _is_blank_visible_output(result):
        return "pass"

    if _has_reasoning_usage(result) or finish_reason not in {"stop"}:
        return "starved"
    return "invalid"


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
    "CANNED_COGITATE_PROBE_PROMPT",
    "CANNED_GENERATE_MAX_OUTPUT_TOKENS",
    "CANNED_GENERATE_NUM_RETRIES",
    "CANNED_GENERATE_PROMPT",
    "CANNED_GENERATE_THINKING_BUDGET",
    "CANNED_GENERATE_TIMEOUT_S",
    "CannedGenerateVerdict",
    "Event",
    "GenerateResult",
    "JSONEventCallback",
    "PROVIDER_ERROR_TEXT_CAP_CHARS",
    "RUNTIME_REASON_CODES",
    "ThinkingEvent",
    "USAGE_KEYS",
    "classify_canned_generate",
    "classify_provider_error",
    "exception_chain",
    "is_cloud_model_not_found",
    "is_non_retryable_generate_reason",
    "mark_cloud_model_request",
    "safe_raw",
]
