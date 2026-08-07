# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Local provider backed by bundled llama-server or a configured endpoint.

The module must remain importable before the local runtime or GGUF files exist.
Network clients and daemon startup are created only inside provider functions.
"""

from __future__ import annotations

import asyncio
import contextlib
import copy
import json
import logging
import re
import subprocess
import sys
import time
import traceback
import uuid
from collections.abc import Callable
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path
from typing import Any, Literal

from solstone.think.models import LOCAL_MODEL
from solstone.think.providers._image import encode_image_part, is_image_part
from solstone.think.providers.local_endpoint import (
    LOCAL_ENDPOINT_CONTRACT_COPY,
    LOCAL_ENDPOINT_UNREACHABLE_COPY,
    classify_byo_cogitate_error,
    is_byo_capacity_error,
    is_byo_network_error,
    local_endpoint_reason_copy,
    redact_local_endpoint_credential,
    resolve_endpoint_served_window,
    resolve_local_endpoint,
)
from solstone.think.providers.shared import (
    _CONTEXT_WINDOW_PATTERNS,
    PROVIDER_ERROR_TEXT_CAP_CHARS,
    GenerateResult,
    _contains_any,
    classify_provider_error,
    safe_raw,
)
from solstone.think.schema_prep import SCHEMA_TRUNCATE_KEY
from solstone.think.utils import get_journal

LOG = logging.getLogger(__name__)

_DEFAULT_TIMEOUT = 120.0
_LOCAL_PREFIX = "local/"
# Qwen3.5-4B runaway repetition emits duplicate array entries until the context
# wall. llama.cpp's GBNF converter honors maxItems, so bounded arrays force
# grammar-friendly closure with finish_reason="stop" and valid (if bloated) JSON.
# 192 is 2.4x the largest observed sense.entities[] length (80, n=1128);
# downstream dedupe absorbs the slack.
_LOCAL_SCHEMA_MAX_ITEMS = 192
# llama.cpp's GBNF converter turns string length limits into repetition counts
# that can exceed its grammar parser limit, and it mistranslates pattern anchors
# into literal characters. Drop these request-side only; canonical validation
# still enforces them after generation.
_LOCAL_UNSUPPORTED_STRING_KEYWORDS = frozenset({"pattern", "minLength", "maxLength"})
# Unknown x-* handling by llama.cpp, mlx-vlm, and llguidance is unproven. Strip
# Solstone annotations before a schema reaches any local structured-output path.
_LOCAL_UNSUPPORTED_ANNOTATION_KEYWORDS = frozenset({SCHEMA_TRUNCATE_KEY})
# Qwen3.5-4B model card sampling recommendations. The card explicitly warns
# against greedy / near-greedy decoding, which drives runaway repetition on
# entity-rich extractions. presence_penalty is the vendor-sanctioned
# anti-repetition lever; we do not touch repeat_penalty or enable DRY/XTC.
_QWEN_TOP_P = 0.8
_QWEN_TOP_K = 20
_QWEN_MIN_P = 0.0
_QWEN_PRESENCE_PENALTY = 1.5
_LOCAL_FINISH_REASON_MAP = {
    "stop": "stop",
    "length": "max_tokens",
    "max_tokens": "max_tokens",
    "content_filter": "content_filter",
}
_LOCAL_UNSUPPORTED_FINISH_REASONS = frozenset({"tool_calls", "function_call"})
_LOCAL_CAPACITY_EXHAUSTED_MESSAGE = (
    "The local model was busy and could not finish this request. Try again in a moment."
)
_ENDPOINT_CONTEXT_WINDOW_MESSAGE = (
    "The configured endpoint rejected the request: prompt and completion exceed "
    "the served context window."
)
_MIN_COMPLETION_TOKENS = 256
_ENDPOINT_RECLAMP_SLACK_TOKENS = 16
_ENDPOINT_COMPLETION_ANCHOR = "tokens for the completion"
_ENDPOINT_LIMIT_RE = re.compile(r"maximum context length of\s+(?P<limit>\d+)\s+tokens")
_ENDPOINT_INPUT_RE = re.compile(
    r"(?P<input>\d+)\s+tokens?\s+from\s+the\s+input\s+messages?\s+and\s+"
    r"\d+\s+tokens?\s+for\s+the\s+completion"
)
_SURROGATE_RE = re.compile(r"[\ud800-\udfff]")


@dataclass(frozen=True)
class LocalModelSpec:
    model_id: str
    repo: str
    filename: str
    revision: str
    sha256: str
    size_bytes: int
    min_ram_bytes: int
    mmproj_filename: str | None = None
    mmproj_sha256: str | None = None
    mmproj_size_bytes: int | None = None


LOCAL_MODEL_SPECS: dict[str, LocalModelSpec] = {
    LOCAL_MODEL: LocalModelSpec(
        model_id=LOCAL_MODEL,
        repo="unsloth/Qwen3.5-4B-GGUF",
        filename="Qwen3.5-4B-Q4_K_M.gguf",
        revision="main",
        sha256="00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4",
        size_bytes=2740937888,
        min_ram_bytes=8 * 1024**3,
        mmproj_filename="mmproj-F16.gguf",
        mmproj_sha256="cd88edcf8d031894960bb0c9c5b9b7e1fea6ebee02b9f7ce925a00d12891f864",
        mmproj_size_bytes=672423616,
    ),
}


class LocalProviderError(RuntimeError):
    """Local provider failure with a recovery reason code."""

    def __init__(self, reason_code: str | None, message: str) -> None:
        super().__init__(message)
        self.reason_code = reason_code


class ContextBudgetExceeded(LocalProviderError):
    """Assembled request cannot fit the bundled local context window."""

    def __init__(self, message: str) -> None:
        super().__init__("context_budget_exceeded", message)


class LocalCapacityExhausted(LocalProviderError):
    """Bundled local server ran out of serving capacity after admission."""

    def __init__(self) -> None:
        super().__init__("local_capacity_exhausted", _LOCAL_CAPACITY_EXHAUSTED_MESSAGE)


@dataclass(frozen=True)
class _EndpointOverflowDecision:
    kind: Literal["retry", "context", "budget", "contract"]
    max_tokens: int | None = None


def normalize_model_id(model: str | None) -> str:
    model_id = str(model or LOCAL_MODEL)
    if model_id.startswith("openai/"):
        model_id = model_id[len("openai/") :]
    if not model_id.startswith(_LOCAL_PREFIX):
        raise LocalProviderError(
            "unsupported_model",
            f"Local provider model must start with {_LOCAL_PREFIX!r}: {model_id}",
        )
    return LOCAL_MODEL


def _contains_image(value: Any) -> bool:
    if is_image_part(value):
        return True
    if isinstance(value, dict):
        return any(_contains_image(item) for item in value.values())
    if isinstance(value, list | tuple):
        return any(_contains_image(item) for item in value)
    return False


def _normalize_request_text(value: Any) -> Any:
    """Return a request payload tree with invalid UTF-16 surrogate text repaired.

    Only string leaves are normalized; image parts and unchanged containers are
    returned by identity. Caller-provided containers and schemas are never
    mutated.
    """
    if isinstance(value, str):
        if _SURROGATE_RE.search(value) is None:
            return value
        return value.encode("utf-16", "surrogatepass").decode(
            "utf-16",
            errors="replace",
        )
    if is_image_part(value):
        return value
    if isinstance(value, list | tuple):
        normalized_items = [_normalize_request_text(item) for item in value]
        if all(new is old for new, old in zip(normalized_items, value)):
            return value
        return tuple(normalized_items) if isinstance(value, tuple) else normalized_items
    if isinstance(value, dict):
        changed = False
        normalized_items = {}
        for key, item in value.items():
            normalized_item = _normalize_request_text(item)
            if normalized_item is not item:
                changed = True
            normalized_items[key] = normalized_item
        if not changed:
            return value
        return normalized_items
    return value


_LOCAL_CONTRACT_PATH = Path(__file__).parents[3] / "core/fixtures/local_contract.json"


@lru_cache(maxsize=1)
def _local_generate_contract() -> dict[str, Any]:
    with _LOCAL_CONTRACT_PATH.open(encoding="utf-8") as handle:
        return json.load(handle)["local_generate"]


def _native_generate_contents(value: Any) -> Any:
    """Copy a request tree into the local-core JSON image representation."""
    if is_image_part(value):
        mime_type, data = encode_image_part(value)
        return {"type": "image", "mime_type": mime_type, "data": data}
    if isinstance(value, list | tuple):
        converted = [_native_generate_contents(item) for item in value]
        return tuple(converted) if isinstance(value, tuple) else converted
    if isinstance(value, dict):
        return {key: _native_generate_contents(item) for key, item in value.items()}
    return value


def _native_generate_payload(
    *,
    contents: Any,
    system_instruction: str | None,
    temperature: float,
    max_output_tokens: int,
    json_output: bool,
    json_schema: dict | None,
    timeout_s: float | None,
    local_exclusive_admission: bool,
    retry_index: int,
) -> dict[str, Any]:
    contract = _local_generate_contract()
    return {
        "schema": contract["schema_identifiers"]["input"],
        "journal_path": str(get_journal()),
        "bind_address": "127.0.0.1",
        "default_model_id": LOCAL_MODEL,
        "platform": "darwin" if sys.platform == "darwin" else "linux",
        "contents": _native_generate_contents(_normalize_request_text(contents)),
        "system_instruction": _normalize_request_text(system_instruction),
        "temperature": temperature,
        "max_output_tokens": max_output_tokens,
        "json_output": json_output,
        "json_schema": json_schema,
        "timeout_s": timeout_s,
        "exclusive_admission": local_exclusive_admission,
        "attempt_index": retry_index,
    }


def _run_native_generate(payload: dict[str, Any]) -> dict[str, Any]:
    from solstone.think import core_handshake

    handshake = core_handshake.check_solstone_core_handshake()
    if handshake.status != "ok":
        raise RuntimeError(
            "local generate requires a usable solstone-core helper: "
            f"{handshake.message or 'unknown handshake failure'}"
        )
    try:
        completed = subprocess.run(
            [str(core_handshake.helper_path_for_executable()), "local", "generate"],
            input=json.dumps(payload),
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        raise RuntimeError(f"solstone-core local generate failed to launch: {exc}") from exc
    if completed.returncode != 0:
        raise RuntimeError(f"solstone-core local generate failed: {completed.stderr}")
    return json.loads(completed.stdout)


def _native_telemetry(inference: dict[str, Any], request_id: str) -> dict[str, Any]:
    public_reason = _local_generate_contract()["reason_codes"].get(
        inference.get("reason_code")
    )
    telemetry = {
        "timestamp": time.time(), "request_id": request_id, "kind": "generate",
        "provider": "local", "model": LOCAL_MODEL,
        "profile": inference["profile"], "serving_capacity": inference["serving_capacity"],
        "capacity_source": inference["capacity_source"], "admission_slot": inference["admission_slot"],
        "queue_wait_ms": round(inference["queue_wait_ms"], 3),
        "client_total_ms": round(inference["client_total_ms"], 3),
        "retry_index": inference["retry_index"], "outcome": inference["outcome"],
        "finish_reason": inference["finish_reason"], "reason_code": public_reason,
        "timed_out": inference["timed_out"], "cancelled": inference["cancelled"],
    }
    if isinstance(inference.get("server"), dict):
        telemetry.update(inference["server"])
    return telemetry


def _raise_native_generate_failure(result: dict[str, Any]) -> None:
    local_code = result.get("reason_code")
    detail = str(result.get("detail", "Local generation failed."))
    if local_code in {"context_image_overflow", "context_preserved_overflow", "context_fitted_overflow", "context_server_overflow"}:
        raise ContextBudgetExceeded(detail)
    if local_code == "capacity_exhausted":
        raise LocalCapacityExhausted()
    if local_code == "admission_timeout":
        from solstone.think.providers.local_admission import LocalAdmissionTimeout
        raise LocalAdmissionTimeout(detail)
    public_code = _local_generate_contract()["reason_codes"].get(local_code)
    raise LocalProviderError(public_code, detail)


def _native_generate_result(result: dict[str, Any], request_id: str) -> GenerateResult:
    inference = result.get("inference")
    if isinstance(inference, dict):
        from solstone.think.providers.local_admission import record_local_inference
        telemetry = _native_telemetry(inference, request_id)
        record_local_inference(telemetry)
    else:
        telemetry = None
    if result.get("outcome") == "failure":
        _raise_native_generate_failure(result)
    output: GenerateResult = {
        "text": str(result["text"]), "model": str(result["model"]),
        "usage": result.get("usage"), "finish_reason": result["finish_reason"], "thinking": None,
        "inference": telemetry,
        "request_budget": result["request_budget"],
    }
    if result.get("input_budget") is not None:
        output["input_budget"] = result["input_budget"]
    return output


def _image_content_part(part: Any) -> dict[str, Any]:
    media_type, b64 = encode_image_part(part)
    return {
        "type": "image_url",
        "image_url": {"url": f"data:{media_type};base64,{b64}"},
    }


def _content_parts(value: Any) -> list[dict[str, Any]]:
    if is_image_part(value):
        return [_image_content_part(value)]
    if isinstance(value, list | tuple):
        parts: list[dict[str, Any]] = []
        for item in value:
            parts.extend(_content_parts(item))
        return parts
    return [{"type": "text", "text": str(value)}]


def _message_content(value: Any) -> str | list[dict[str, Any]]:
    if _contains_image(value):
        return _content_parts(value)
    if isinstance(value, str):
        return value
    if isinstance(value, list | tuple):
        return "\n".join(str(item) for item in value)
    return str(value)


def _build_messages(
    contents: str | list[Any],
    system_instruction: str | None = None,
) -> list[dict[str, Any]]:
    messages: list[dict[str, Any]] = []
    if system_instruction:
        messages.append({"role": "system", "content": system_instruction})

    if isinstance(contents, str):
        messages.append({"role": "user", "content": contents})
    elif isinstance(contents, list):
        if contents and isinstance(contents[0], dict) and "role" in contents[0]:
            for item in contents:
                role = str(item.get("role", "user"))
                content = item.get("content", "")
                messages.append({"role": role, "content": _message_content(content)})
        else:
            messages.append({"role": "user", "content": _message_content(contents)})
    else:
        messages.append({"role": "user", "content": str(contents)})
    return messages


def _prepare_local_schema(schema: dict) -> dict:
    """Prepare a JSON Schema for llama.cpp's GBNF converter.

    Strip string constraints that break llama.cpp grammar generation:
    maxLength becomes repetition counts that can exceed the grammar parser
    limit, and pattern anchors are mistranslated into literal characters. Strip
    Solstone annotations because unknown x-* handling by llama.cpp, mlx-vlm, and
    llguidance is unproven. models.py still checks the canonical schema and
    honors annotations: generate() raises SchemaValidationError, while
    generate_with_result() records schema_validation for callers; the talent path
    writes output and withholds clean provenance on violations. Adds maxItems to
    array nodes, because bounded arrays force closure before Qwen can repeat
    entries to the context wall. Does not recurse into enum/const values, because
    those are JSON literals, not schemas. Deep-copies so the caller's schema is
    never mutated.
    """
    prepared = copy.deepcopy(schema)

    def _walk(node: Any) -> None:
        if isinstance(node, dict):
            for key in _LOCAL_UNSUPPORTED_STRING_KEYWORDS:
                node.pop(key, None)
            for key in _LOCAL_UNSUPPORTED_ANNOTATION_KEYWORDS:
                node.pop(key, None)
            node_type = node.get("type")
            if "maxItems" not in node and (
                node_type == "array"
                or (isinstance(node_type, list) and "array" in node_type)
            ):
                node["maxItems"] = _LOCAL_SCHEMA_MAX_ITEMS
            for key, value in node.items():
                if key in {"const", "enum"}:
                    continue
                _walk(value)
        elif isinstance(node, list):
            for item in node:
                _walk(item)

    _walk(prepared)

    return prepared


def _build_request_body(
    model_id: str,
    messages: list[dict[str, Any]],
    temperature: float,
    max_output_tokens: int,
    json_output: bool,
    json_schema: dict | None,
    is_bundled: bool,
    is_confidential: bool = False,
) -> dict[str, Any]:
    body: dict[str, Any] = {
        "model": model_id,
        "messages": messages,
        "temperature": temperature,
        "max_tokens": max_output_tokens,
        "stream": False,
    }
    if is_bundled or is_confidential:
        body.update(
            {
                "chat_template_kwargs": {"enable_thinking": False},
                "top_p": _QWEN_TOP_P,
                "top_k": _QWEN_TOP_K,
                "min_p": _QWEN_MIN_P,
                "presence_penalty": _QWEN_PRESENCE_PENALTY,
            }
        )
    if json_schema is not None:
        body["response_format"] = {
            "type": "json_schema",
            "json_schema": {
                "name": "local_schema",
                "schema": _prepare_local_schema(json_schema),
                "strict": True,
            },
        }
    elif json_output:
        body["response_format"] = {"type": "json_object"}
    return body


def _count_image_parts(value: Any) -> int:
    if is_image_part(value):
        return 1
    if isinstance(value, dict):
        return sum(_count_image_parts(item) for item in value.values())
    if isinstance(value, list | tuple):
        return sum(_count_image_parts(item) for item in value)
    return 0


def _serialized_message_text(messages: list[dict[str, Any]]) -> str:
    text_parts: list[str] = []
    for message in messages:
        content = message.get("content")
        if isinstance(content, str):
            text_parts.append(content)
        elif isinstance(content, list):
            for part in content:
                if isinstance(part, dict) and part.get("type") == "text":
                    text = part.get("text")
                    if isinstance(text, str):
                        text_parts.append(text)
    return "\n".join(text_parts)


def _extract_usage(data: dict[str, Any]) -> dict[str, int] | None:
    usage = data.get("usage")
    if not isinstance(usage, dict):
        return None
    input_tokens = int(usage.get("prompt_tokens") or 0)
    output_tokens = int(usage.get("completion_tokens") or 0)
    total_tokens = int(usage.get("total_tokens") or input_tokens + output_tokens)
    normalized = {
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": total_tokens,
    }
    prompt_details = usage.get("prompt_tokens_details")
    if isinstance(prompt_details, dict):
        cached_tokens = int(prompt_details.get("cached_tokens") or 0)
        if cached_tokens:
            normalized["cached_tokens"] = cached_tokens
    return normalized


def _normalize_finish_reason(raw: Any) -> str:
    if not isinstance(raw, str) or not raw.strip():
        raise LocalProviderError(
            "provider_response_invalid",
            "Local model response did not include a finish reason.",
        )
    reason = raw.strip().lower()
    normalized = _LOCAL_FINISH_REASON_MAP.get(reason)
    if normalized is not None:
        return normalized
    if reason in _LOCAL_UNSUPPORTED_FINISH_REASONS:
        raise LocalProviderError(
            "provider_response_invalid",
            f"Local model returned unsupported finish reason: {reason}",
        )
    raise LocalProviderError(
        "provider_response_invalid",
        f"Local model returned unknown finish reason: {reason}",
    )


def _parse_response(data: dict[str, Any]) -> GenerateResult:
    choices = data.get("choices")
    if not isinstance(choices, list) or not choices:
        raise LocalProviderError("provider_response_invalid", "No response from model.")
    choice = choices[0]
    if not isinstance(choice, dict):
        raise LocalProviderError(
            "provider_response_invalid", "Malformed model response."
        )
    message = choice.get("message")
    text = ""
    if isinstance(message, dict):
        content = message.get("content", "")
        text = content if isinstance(content, str) else ""
    return GenerateResult(
        text=text,
        model=LOCAL_MODEL,
        usage=_extract_usage(data),
        finish_reason=_normalize_finish_reason(choice.get("finish_reason")),
        thinking=None,
    )


def _number(value: Any) -> int | float | None:
    if isinstance(value, bool) or not isinstance(value, int | float):
        return None
    return value


def _server_response_fields(data: dict[str, Any]) -> dict[str, Any]:
    """Normalize content-free timing/cache/slot fields exposed by local servers."""
    timings = data.get("timings")
    timings = timings if isinstance(timings, dict) else {}
    usage = data.get("usage")
    usage = usage if isinstance(usage, dict) else {}
    prompt_details = usage.get("prompt_tokens_details")
    prompt_details = prompt_details if isinstance(prompt_details, dict) else {}

    cached_tokens = _number(timings.get("cache_n"))
    if cached_tokens is None:
        cached_tokens = _number(prompt_details.get("cached_tokens"))
    slot_id = _number(data.get("id_slot"))
    if slot_id is None:
        slot_id = _number(data.get("slot_id"))
    if slot_id is None:
        slot_id = _number(timings.get("slot_id"))

    prompt_ms = _number(timings.get("prompt_ms"))
    generation_ms = _number(timings.get("predicted_ms"))
    server_total_ms = None
    if prompt_ms is not None or generation_ms is not None:
        server_total_ms = float(prompt_ms or 0) + float(generation_ms or 0)

    return {
        "prompt_eval_ms": prompt_ms,
        "generation_ms": generation_ms,
        "server_total_ms": server_total_ms,
        "prompt_tokens": _number(usage.get("prompt_tokens"))
        or _number(timings.get("prompt_n")),
        "generated_tokens": _number(usage.get("completion_tokens"))
        or _number(timings.get("predicted_n")),
        "prompt_cached_tokens": cached_tokens,
        "selected_slot": slot_id,
        "prompt_cache_state": (
            "warm"
            if cached_tokens is not None and cached_tokens > 0
            else "cold"
            if cached_tokens is not None
            else "unknown"
        ),
    }


def _telemetry_record(
    *,
    request_id: str,
    kind: str,
    model: str,
    profile: str,
    capacity: int,
    capacity_source: str,
    started: float,
    queue_wait_ms: float,
    admission_slot: int | None,
    retry_index: int | None,
    outcome: str,
    finish_reason: str | None = None,
    response_data: dict[str, Any] | None = None,
    reason_code: str | None = None,
) -> dict[str, Any]:
    record: dict[str, Any] = {
        "timestamp": time.time(),
        "request_id": request_id,
        "kind": kind,
        "provider": "local",
        "model": model,
        "profile": profile,
        "serving_capacity": capacity,
        "capacity_source": capacity_source,
        "admission_slot": admission_slot,
        "queue_wait_ms": round(queue_wait_ms, 3),
        "client_total_ms": round((time.monotonic() - started) * 1000.0, 3),
        "retry_index": retry_index,
        "outcome": outcome,
        "finish_reason": finish_reason,
        "reason_code": reason_code,
        "timed_out": outcome == "timeout",
        "cancelled": outcome == "cancelled",
    }
    if response_data is not None:
        record.update(_server_response_fields(response_data))
    return record


def _classify_byo_generate_error(
    exc: BaseException,
    endpoint: Any,
) -> LocalProviderError:
    if is_byo_capacity_error(exc):
        return LocalCapacityExhausted()
    if is_byo_network_error(exc):
        return LocalProviderError(
            "local_endpoint_unreachable",
            LOCAL_ENDPOINT_UNREACHABLE_COPY,
        )
    response = getattr(exc, "response", None)
    body_text = getattr(response, "text", None)
    if isinstance(body_text, str) and body_text:
        excerpt = redact_local_endpoint_credential(
            body_text[:PROVIDER_ERROR_TEXT_CAP_CHARS],
            endpoint,
        )
        if _contains_any(excerpt.lower(), _CONTEXT_WINDOW_PATTERNS):
            return _context_window_exceeded_error()
    return LocalProviderError(
        "local_endpoint_contract_failed",
        LOCAL_ENDPOINT_CONTRACT_COPY,
    )


def _remaining_timeout(started: float, timeout_s: float) -> float:
    remaining = timeout_s - (time.monotonic() - started)
    if remaining <= 0:
        from solstone.think.providers.local_admission import LocalAdmissionTimeout

        raise LocalAdmissionTimeout(
            f"Local inference request exceeded its {timeout_s:.3f}s deadline."
        )
    return remaining


def _context_window_exceeded_error() -> LocalProviderError:
    return LocalProviderError(
        "context_window_exceeded",
        _ENDPOINT_CONTEXT_WINDOW_MESSAGE,
    )


def _endpoint_overflow_decision(
    body_text: str,
    served_window: int | None,
    attempt: int,
) -> _EndpointOverflowDecision:
    body_lower = body_text.lower()
    if _ENDPOINT_COMPLETION_ANCHOR in body_lower:
        limit_match = _ENDPOINT_LIMIT_RE.search(body_lower)
        input_match = _ENDPOINT_INPUT_RE.search(body_lower)
        limit = (
            int(limit_match.group("limit"))
            if limit_match is not None
            else served_window
        )
        if limit is not None and input_match is not None:
            reported_input = int(input_match.group("input"))
            new_max = limit - reported_input - _ENDPOINT_RECLAMP_SLACK_TOKENS
            if attempt == 0 and new_max >= _MIN_COMPLETION_TOKENS:
                return _EndpointOverflowDecision("retry", new_max)
            if attempt == 0:
                return _EndpointOverflowDecision("budget")
            return _EndpointOverflowDecision("context")

    if _contains_any(body_lower, _CONTEXT_WINDOW_PATTERNS):
        return _EndpointOverflowDecision("context")
    return _EndpointOverflowDecision("contract")


def _prepare_endpoint_request(
    *,
    endpoint: Any,
    served_window: int | None,
    contents: str | list[Any],
    system_instruction: str | None,
    temperature: float,
    max_output_tokens: int,
    json_output: bool,
    json_schema: dict | None,
) -> tuple[dict[str, Any], dict[str, Any] | None, dict[str, int | None]]:
    # Normalize here because this prep entry sits above the served_window is None
    # branch that returns before fit_contents runs.
    contents = _normalize_request_text(contents)
    system_instruction = _normalize_request_text(system_instruction)

    from solstone.think.providers import local_budget

    if served_window is None:
        messages = _build_messages(contents, system_instruction)
        return (
            _build_request_body(
                endpoint.served_model_id,
                messages,
                temperature,
                max_output_tokens,
                json_output,
                json_schema,
                endpoint.is_bundled,
                endpoint.is_confidential,
            ),
            None,
            {
                "window": None,
                "slots": endpoint.parallel_slots,
                "estimated_prompt_tokens": None,
                "image_tokens": None,
                "clamped_max_tokens": max_output_tokens,
                "requested_max_output_tokens": max_output_tokens,
            },
        )

    fitted_contents, input_budget = local_budget.fit_contents(
        contents,
        system_instruction,
        max_output_tokens,
        count=local_budget.estimate_tokens,
        window=served_window,
    )
    messages = _build_messages(fitted_contents, system_instruction)
    estimated_prompt_tokens = local_budget.estimate_tokens(
        _serialized_message_text(messages)
    )
    image_tokens = local_budget._ESTIMATED_IMAGE_TOKENS * _count_image_parts(
        fitted_contents
    )
    room = (
        served_window
        - estimated_prompt_tokens
        - image_tokens
        - local_budget._SAFETY_MARGIN_TOKENS
    )
    if room < _MIN_COMPLETION_TOKENS:
        raise ContextBudgetExceeded(
            "Local endpoint request prompt content exceeds the served context window."
        )
    clamped_max_tokens = min(max_output_tokens, room)
    return (
        _build_request_body(
            endpoint.served_model_id,
            messages,
            temperature,
            clamped_max_tokens,
            json_output,
            json_schema,
            endpoint.is_bundled,
            endpoint.is_confidential,
        ),
        input_budget,
        {
            "window": served_window,
            "slots": endpoint.parallel_slots,
            "estimated_prompt_tokens": estimated_prompt_tokens,
            "image_tokens": image_tokens,
            "clamped_max_tokens": clamped_max_tokens,
            "requested_max_output_tokens": max_output_tokens,
        },
    )


def _prepare_endpoint_request_with_resolution(
    *,
    endpoint: Any,
    contents: str | list[Any],
    system_instruction: str | None,
    temperature: float,
    max_output_tokens: int,
    json_output: bool,
    json_schema: dict | None,
) -> tuple[dict[str, Any], dict[str, Any] | None, dict[str, int | None]]:
    return _prepare_endpoint_request(
        endpoint=endpoint,
        served_window=resolve_endpoint_served_window(endpoint),
        contents=contents,
        system_instruction=system_instruction,
        temperature=temperature,
        max_output_tokens=max_output_tokens,
        json_output=json_output,
        json_schema=json_schema,
    )


def run_generate(
    contents: str | list[Any],
    model: str,
    temperature: float = 0.3,
    max_output_tokens: int = 8192 * 2,
    system_instruction: str | None = None,
    json_output: bool = False,
    thinking_budget: int | None = None,
    json_schema: dict | None = None,
    timeout_s: float | None = None,
    **kwargs: Any,
) -> GenerateResult:
    del thinking_budget
    retry_index = int(kwargs.pop("inference_retry_index", 0) or 0)
    local_exclusive_admission = bool(kwargs.pop("local_exclusive_admission", False))
    if kwargs:
        unknown = ", ".join(sorted(kwargs))
        raise TypeError(f"Unsupported local generate options: {unknown}")
    endpoint = resolve_local_endpoint()
    # Validate the requested logical id; served id comes from the server.
    normalize_model_id(model)
    if endpoint.is_bundled:
        payload = _native_generate_payload(
            contents=contents, system_instruction=system_instruction,
            temperature=temperature, max_output_tokens=max_output_tokens,
            json_output=json_output, json_schema=json_schema, timeout_s=timeout_s,
            local_exclusive_admission=local_exclusive_admission, retry_index=retry_index,
        )
        return _native_generate_result(_run_native_generate(payload), uuid.uuid4().hex)

    body, input_budget, request_budget = _prepare_endpoint_request_with_resolution(
        endpoint=endpoint,
        contents=contents,
        system_instruction=system_instruction,
        temperature=temperature,
        max_output_tokens=max_output_tokens,
        json_output=json_output,
        json_schema=json_schema,
    )

    import httpx

    from solstone.think.providers.local_admission import (
        LocalAdmissionTimeout,
        acquire_local_slot,
    )
    from solstone.think.services.spp_transport import confidential_egress_base_url

    post_kwargs: dict[str, Any] = {
        "json": body,
    }
    if endpoint.credential:
        post_kwargs["headers"] = {"Authorization": f"Bearer {endpoint.credential}"}
    started = time.monotonic()
    timeout = timeout_s or _DEFAULT_TIMEOUT
    if endpoint.parallel_slots is None:
        admission = contextlib.nullcontext()
        post_timeout = timeout
    else:
        admission = acquire_local_slot(
            endpoint.parallel_slots,
            _remaining_timeout(started, timeout),
        )
        try:
            post_timeout = _remaining_timeout(started, timeout)
        except LocalAdmissionTimeout:
            admission.release()
            raise
    try:
        with admission:
            base_url = confidential_egress_base_url(endpoint.base_url)
            attempt = 0
            while True:
                response = httpx.post(
                    f"{base_url}/v1/chat/completions",
                    timeout=(
                        post_timeout
                        if attempt == 0
                        else _remaining_timeout(started, timeout)
                    ),
                    **post_kwargs,
                )
                try:
                    response.raise_for_status()
                    break
                except httpx.HTTPStatusError as exc:
                    if response.status_code != 400:
                        raise
                    decision = _endpoint_overflow_decision(
                        response.text,
                        request_budget.get("window"),
                        attempt,
                    )
                    if decision.kind == "retry" and decision.max_tokens is not None:
                        post_kwargs["json"] = {
                            **post_kwargs["json"],
                            "max_tokens": decision.max_tokens,
                        }
                        request_budget = {
                            **request_budget,
                            "clamped_max_tokens": decision.max_tokens,
                        }
                        attempt += 1
                        continue
                    if decision.kind == "budget":
                        raise ContextBudgetExceeded(
                            "Local endpoint request exceeded the served context "
                            "window after completion re-clamp."
                        ) from exc
                    if decision.kind == "context":
                        raise _context_window_exceeded_error() from exc
                    raise
            result = _parse_response(response.json())
            if input_budget is not None:
                result["input_budget"] = input_budget
            if request_budget.get("window") is not None:
                result["request_budget"] = request_budget
            return result
    except LocalAdmissionTimeout:
        raise
    except LocalProviderError:
        raise
    except Exception as exc:
        raise _classify_byo_generate_error(exc, endpoint) from exc


async def run_agenerate(
    contents: str | list[Any],
    model: str,
    temperature: float = 0.3,
    max_output_tokens: int = 8192 * 2,
    system_instruction: str | None = None,
    json_output: bool = False,
    thinking_budget: int | None = None,
    json_schema: dict | None = None,
    timeout_s: float | None = None,
    **kwargs: Any,
) -> GenerateResult:
    del thinking_budget
    retry_index = int(kwargs.pop("inference_retry_index", 0) or 0)
    local_exclusive_admission = bool(kwargs.pop("local_exclusive_admission", False))
    if kwargs:
        unknown = ", ".join(sorted(kwargs))
        raise TypeError(f"Unsupported local generate options: {unknown}")
    endpoint = resolve_local_endpoint()
    normalize_model_id(model)

    import httpx

    if not endpoint.is_bundled:
        from solstone.think.providers.local_admission import (
            LocalAdmissionTimeout,
            acquire_local_slot_async,
        )
        from solstone.think.services.spp_transport import (
            confidential_egress_base_url,
        )

        body, input_budget, request_budget = await asyncio.to_thread(
            _prepare_endpoint_request_with_resolution,
            endpoint=endpoint,
            contents=contents,
            system_instruction=system_instruction,
            temperature=temperature,
            max_output_tokens=max_output_tokens,
            json_output=json_output,
            json_schema=json_schema,
        )
        post_kwargs: dict[str, Any] = {
            "json": body,
        }
        if endpoint.credential:
            post_kwargs["headers"] = {"Authorization": f"Bearer {endpoint.credential}"}
        started = time.monotonic()
        timeout = timeout_s or _DEFAULT_TIMEOUT
        if endpoint.parallel_slots is None:
            admission = contextlib.nullcontext()
            post_timeout = timeout
        else:
            admission = await acquire_local_slot_async(
                endpoint.parallel_slots,
                _remaining_timeout(started, timeout),
            )
            try:
                post_timeout = _remaining_timeout(started, timeout)
            except LocalAdmissionTimeout:
                admission.release()
                raise
        try:
            async with admission:
                base_url = confidential_egress_base_url(endpoint.base_url)
                attempt = 0
                async with httpx.AsyncClient() as client:
                    while True:
                        response = await client.post(
                            f"{base_url}/v1/chat/completions",
                            timeout=(
                                post_timeout
                                if attempt == 0
                                else _remaining_timeout(started, timeout)
                            ),
                            **post_kwargs,
                        )
                        try:
                            response.raise_for_status()
                            break
                        except httpx.HTTPStatusError as exc:
                            if response.status_code != 400:
                                raise
                            decision = _endpoint_overflow_decision(
                                response.text,
                                request_budget.get("window"),
                                attempt,
                            )
                            if (
                                decision.kind == "retry"
                                and decision.max_tokens is not None
                            ):
                                post_kwargs["json"] = {
                                    **post_kwargs["json"],
                                    "max_tokens": decision.max_tokens,
                                }
                                request_budget = {
                                    **request_budget,
                                    "clamped_max_tokens": decision.max_tokens,
                                }
                                attempt += 1
                                continue
                            if decision.kind == "budget":
                                raise ContextBudgetExceeded(
                                    "Local endpoint request exceeded the served "
                                    "context window after completion re-clamp."
                                ) from exc
                            if decision.kind == "context":
                                raise _context_window_exceeded_error() from exc
                            raise
                result = _parse_response(response.json())
                if input_budget is not None:
                    result["input_budget"] = input_budget
                if request_budget.get("window") is not None:
                    result["request_budget"] = request_budget
                return result
        except asyncio.CancelledError:
            raise
        except LocalAdmissionTimeout:
            raise
        except LocalProviderError:
            raise
        except Exception as exc:
            raise _classify_byo_generate_error(exc, endpoint) from exc

    payload = _native_generate_payload(
        contents=contents, system_instruction=system_instruction,
        temperature=temperature, max_output_tokens=max_output_tokens,
        json_output=json_output, json_schema=json_schema, timeout_s=timeout_s,
        local_exclusive_admission=local_exclusive_admission, retry_index=retry_index,
    )
    result = await asyncio.to_thread(_run_native_generate, payload)
    return _native_generate_result(result, uuid.uuid4().hex)


async def run_cogitate(
    config: dict[str, Any],
    on_event: Callable[[dict], None] | None = None,
) -> str:
    from solstone.think.providers import local_server, openhands
    from solstone.think.providers.local_admission import (
        LocalAdmissionTimeout,
        LocalSlotLease,
        acquire_local_slot_async,
        record_local_inference,
    )

    config = {**config, "model": normalize_model_id(config.get("model", LOCAL_MODEL))}
    endpoint = resolve_local_endpoint()
    started = time.monotonic()
    request_id = uuid.uuid4().hex
    timeout = float(config.get("timeout_seconds", 600) or 600)
    server = None
    capacity = None
    slot_lease = None
    outcome = "success"
    reason_code: str | None = None
    try:
        if endpoint.is_bundled:
            server = local_server.connect()
            capacity = local_server.read_server_capacity()
            permit = await acquire_local_slot_async(
                capacity.parallel_slots,
                _remaining_timeout(started, timeout),
            )
            slot_lease = LocalSlotLease(
                capacity=capacity.parallel_slots,
                deadline=started + timeout,
                permit=permit,
            )
        elif endpoint.parallel_slots is not None:
            permit = await acquire_local_slot_async(
                endpoint.parallel_slots,
                _remaining_timeout(started, timeout),
            )
            slot_lease = LocalSlotLease(
                capacity=endpoint.parallel_slots,
                deadline=started + timeout,
                permit=permit,
            )
        return await openhands.run_cogitate(
            config,
            on_event=on_event,
            slot_lease=slot_lease,
        )
    except asyncio.CancelledError:
        outcome = "cancelled"
        reason_code = "cancelled"
        raise
    except Exception as exc:
        outcome = (
            "timeout"
            if isinstance(exc, LocalAdmissionTimeout)
            or getattr(exc, "reason_code", None) == "wall_clock_exceeded"
            else "error"
        )
        from solstone.think.talents import TalentHookError

        if isinstance(exc, TalentHookError):
            raise

        if not endpoint.is_bundled:
            reason_code = classify_byo_cogitate_error(exc) or getattr(
                exc, "reason_code", None
            )
        reason_code = (
            reason_code
            or getattr(exc, "reason_code", None)
            or classify_provider_error(exc, "local")
        )
        if on_event and not getattr(exc, "_evented", False):
            error_text = str(exc)
            trace_text = traceback.format_exc()
            fixed_copy = local_endpoint_reason_copy(reason_code)
            if fixed_copy:
                error_text = fixed_copy
            if not endpoint.is_bundled:
                error_text = redact_local_endpoint_credential(error_text, endpoint)
                trace_text = redact_local_endpoint_credential(trace_text, endpoint)
                if not fixed_copy:
                    error_text = error_text[:PROVIDER_ERROR_TEXT_CAP_CHARS]
            on_event(
                {
                    "event": "error",
                    "error": error_text,
                    "reason_code": reason_code,
                    "provider": "local",
                    "trace": trace_text,
                    "raw": safe_raw([{"reason_code": reason_code}]),
                }
            )
            setattr(exc, "_evented", True)
        fixed_copy = local_endpoint_reason_copy(reason_code)
        if fixed_copy:
            wrapped = LocalProviderError(reason_code or "unknown", fixed_copy)
            setattr(wrapped, "_evented", getattr(exc, "_evented", False))
            raise wrapped from exc
        raise
    finally:
        if slot_lease is not None:
            slot_lease.close()
        if server is not None and capacity is not None:
            record_local_inference(
                _telemetry_record(
                    request_id=request_id,
                    kind="cogitate",
                    model=LOCAL_MODEL,
                    profile=capacity.profile,
                    capacity=capacity.parallel_slots,
                    capacity_source=capacity.source,
                    started=started,
                    queue_wait_ms=(
                        slot_lease.initial_queue_wait_ms
                        if slot_lease is not None
                        else (time.monotonic() - started) * 1000.0
                    ),
                    admission_slot=(
                        slot_lease.initial_slot_index
                        if slot_lease is not None
                        else None
                    ),
                    retry_index=None,
                    outcome=outcome,
                    finish_reason="stop" if outcome == "success" else None,
                    reason_code=reason_code,
                )
            )


def list_models(provider: str = "local") -> list[dict[str, Any]]:
    del provider
    return [
        {
            "name": spec.model_id,
            "model": spec.model_id,
            "repo": spec.repo,
            "filename": spec.filename,
            "size_bytes": spec.size_bytes,
            "min_ram_bytes": spec.min_ram_bytes,
        }
        for spec in LOCAL_MODEL_SPECS.values()
    ]


def validate_key(provider: str = "local", api_key: str = "") -> dict[str, Any]:
    del provider, api_key
    try:
        run_generate(
            "Say OK",
            model=LOCAL_MODEL,
            temperature=0,
            max_output_tokens=8,
            timeout_s=10,
        )
        return {"valid": True}
    except Exception as exc:
        return {
            "valid": False,
            "error": str(exc),
            "reason_code": getattr(exc, "reason_code", None)
            or classify_provider_error(exc, "local"),
        }


__all__ = [
    "LOCAL_MODEL_SPECS",
    "ContextBudgetExceeded",
    "LocalCapacityExhausted",
    "LocalModelSpec",
    "LocalProviderError",
    "normalize_model_id",
    "run_generate",
    "run_agenerate",
    "run_cogitate",
    "list_models",
    "validate_key",
]
