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
import logging
import time
import traceback
import uuid
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

from solstone.think.models import LOCAL_MODEL
from solstone.think.providers._image import encode_image_part, is_image_part
from solstone.think.providers.local_endpoint import (
    LOCAL_ENDPOINT_CONTRACT_COPY,
    LOCAL_ENDPOINT_UNREACHABLE_COPY,
    classify_byo_cogitate_error,
    is_byo_network_error,
    local_endpoint_reason_copy,
    redact_local_endpoint_credential,
    resolve_local_endpoint,
)
from solstone.think.providers.shared import (
    _CONTEXT_WINDOW_PATTERNS,
    GenerateResult,
    _contains_any,
    classify_provider_error,
    safe_raw,
)

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

    def __init__(self, reason_code: str, message: str) -> None:
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
    limit, and pattern anchors are mistranslated into literal characters.
    models.py still checks the canonical schema: generate() raises
    SchemaValidationError, while generate_with_result() records
    schema_validation for callers; the talent path writes output and withholds
    clean provenance on violations. Adds maxItems to array nodes, because
    bounded arrays force closure before Qwen can repeat entries to the context
    wall. Does not recurse into enum/const values, because those are JSON
    literals, not schemas. Deep-copies so the caller's schema is never mutated.
    """
    prepared = copy.deepcopy(schema)

    def _walk(node: Any) -> None:
        if isinstance(node, dict):
            for key in _LOCAL_UNSUPPORTED_STRING_KEYWORDS:
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
    apply_qwen_sampling: bool,
) -> dict[str, Any]:
    body: dict[str, Any] = {
        "model": model_id,
        "messages": messages,
        "temperature": temperature,
        "max_tokens": max_output_tokens,
        "stream": False,
        "chat_template_kwargs": {"enable_thinking": False},
    }
    if apply_qwen_sampling:
        body.update(
            {
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


def _classify_byo_generate_error(exc: BaseException) -> LocalProviderError:
    if is_byo_network_error(exc):
        return LocalProviderError(
            "local_endpoint_unreachable",
            LOCAL_ENDPOINT_UNREACHABLE_COPY,
        )
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


def _prepare_bundled_request(
    *,
    server: Any,
    contents: str | list[Any],
    system_instruction: str | None,
    temperature: float,
    max_output_tokens: int,
    json_output: bool,
    json_schema: dict | None,
) -> tuple[dict[str, Any], dict[str, Any] | None]:
    from solstone.think.providers import local_budget

    def counter(text: str) -> int:
        return local_budget.count_tokens(text, server.base_url)

    fitted_contents, input_budget = local_budget.fit_contents(
        contents,
        system_instruction,
        max_output_tokens,
        count=counter,
    )
    messages = _build_messages(fitted_contents, system_instruction)
    return (
        _build_request_body(
            server.served_model_id,
            messages,
            temperature,
            max_output_tokens,
            json_output,
            json_schema,
            True,
        ),
        input_budget,
    )


def _raise_bundled_status(response: Any) -> None:
    import httpx

    try:
        response.raise_for_status()
    except httpx.HTTPStatusError as exc:
        if _contains_any(response.text.lower(), _CONTEXT_WINDOW_PATTERNS):
            if _bundled_error_type(response) == "exceed_context_size_error":
                raise ContextBudgetExceeded(
                    "Local request exceeded the model context window after fitting."
                ) from exc
            raise LocalCapacityExhausted() from exc
        raise


def _bundled_error_type(response: Any) -> str | None:
    try:
        data = response.json()
    except Exception:
        return None
    if not isinstance(data, dict):
        return None
    error = data.get("error")
    if not isinstance(error, dict):
        return None
    error_type = error.get("type")
    if isinstance(error_type, str):
        return error_type
    return None


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
    messages = _build_messages(contents, system_instruction)
    if endpoint.is_bundled:
        from solstone.think.providers import local_server
        from solstone.think.providers.local_admission import (
            LocalAdmissionTimeout,
            acquire_local_slot,
            record_local_inference,
        )

        started = time.monotonic()
        request_id = uuid.uuid4().hex
        timeout = timeout_s or _DEFAULT_TIMEOUT
        server = local_server.connect()
        capacity = local_server.read_server_capacity()
        body, input_budget = _prepare_bundled_request(
            server=server,
            contents=contents,
            system_instruction=system_instruction,
            temperature=temperature,
            max_output_tokens=max_output_tokens,
            json_output=json_output,
            json_schema=json_schema,
        )

        import httpx

        permit = None
        try:
            permit = acquire_local_slot(
                capacity.parallel_slots,
                _remaining_timeout(started, timeout),
                exclusive=local_exclusive_admission,
            )
            with permit:
                response = httpx.post(
                    f"{server.base_url}/v1/chat/completions",
                    json=body,
                    timeout=_remaining_timeout(started, timeout),
                )
                _raise_bundled_status(response)
                response_data = response.json()
            result = _parse_response(response_data)
            telemetry = _telemetry_record(
                request_id=request_id,
                kind="generate",
                model=LOCAL_MODEL,
                profile=capacity.profile,
                capacity=capacity.parallel_slots,
                capacity_source=capacity.source,
                started=started,
                queue_wait_ms=permit.queue_wait_ms,
                admission_slot=permit.slot_index,
                retry_index=retry_index,
                outcome="success",
                finish_reason=result.get("finish_reason"),
                response_data=response_data,
            )
            record_local_inference(telemetry)
            result["inference"] = telemetry
            if input_budget is not None:
                result["input_budget"] = input_budget
            return result
        except BaseException as exc:
            if isinstance(exc, KeyboardInterrupt | SystemExit):
                raise
            if permit is not None:
                permit.release()
            outcome = (
                "timeout"
                if isinstance(exc, (LocalAdmissionTimeout, httpx.TimeoutException))
                else "cancelled"
                if isinstance(exc, asyncio.CancelledError)
                else "error"
            )
            record_local_inference(
                _telemetry_record(
                    request_id=request_id,
                    kind="generate",
                    model=LOCAL_MODEL,
                    profile=capacity.profile,
                    capacity=capacity.parallel_slots,
                    capacity_source=capacity.source,
                    started=started,
                    queue_wait_ms=(
                        permit.queue_wait_ms
                        if permit is not None
                        else (time.monotonic() - started) * 1000.0
                    ),
                    admission_slot=permit.slot_index if permit is not None else None,
                    retry_index=retry_index,
                    outcome=outcome,
                    reason_code=getattr(exc, "reason_code", None)
                    or classify_provider_error(exc, "local"),
                )
            )
            raise

    body = _build_request_body(
        endpoint.served_model_id,
        messages,
        temperature,
        max_output_tokens,
        json_output,
        json_schema,
        endpoint.is_bundled,
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
            response = httpx.post(
                f"{base_url}/v1/chat/completions",
                timeout=post_timeout,
                **post_kwargs,
            )
            response.raise_for_status()
            return _parse_response(response.json())
    except LocalAdmissionTimeout:
        raise
    except LocalProviderError:
        raise
    except Exception as exc:
        raise _classify_byo_generate_error(exc) from exc


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
    messages = _build_messages(contents, system_instruction)

    import httpx

    if not endpoint.is_bundled:
        from solstone.think.providers.local_admission import (
            LocalAdmissionTimeout,
            acquire_local_slot_async,
        )
        from solstone.think.services.spp_transport import (
            confidential_egress_base_url,
        )

        body = _build_request_body(
            endpoint.served_model_id,
            messages,
            temperature,
            max_output_tokens,
            json_output,
            json_schema,
            False,
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
                async with httpx.AsyncClient() as client:
                    response = await client.post(
                        f"{base_url}/v1/chat/completions",
                        timeout=post_timeout,
                        **post_kwargs,
                    )
                response.raise_for_status()
                return _parse_response(response.json())
        except asyncio.CancelledError:
            raise
        except LocalAdmissionTimeout:
            raise
        except LocalProviderError:
            raise
        except Exception as exc:
            raise _classify_byo_generate_error(exc) from exc

    from solstone.think.providers import local_server
    from solstone.think.providers.local_admission import (
        LocalAdmissionTimeout,
        acquire_local_slot_async,
        record_local_inference,
    )

    started = time.monotonic()
    request_id = uuid.uuid4().hex
    timeout = timeout_s or _DEFAULT_TIMEOUT
    server = local_server.connect()
    capacity = local_server.read_server_capacity()
    body, input_budget = await asyncio.to_thread(
        _prepare_bundled_request,
        server=server,
        contents=contents,
        system_instruction=system_instruction,
        temperature=temperature,
        max_output_tokens=max_output_tokens,
        json_output=json_output,
        json_schema=json_schema,
    )
    permit = None
    try:
        permit = await acquire_local_slot_async(
            capacity.parallel_slots,
            _remaining_timeout(started, timeout),
            exclusive=local_exclusive_admission,
        )
        async with permit:
            async with httpx.AsyncClient() as client:
                response = await client.post(
                    f"{server.base_url}/v1/chat/completions",
                    json=body,
                    timeout=_remaining_timeout(started, timeout),
                )
            _raise_bundled_status(response)
            response_data = response.json()
        result = _parse_response(response_data)
        telemetry = _telemetry_record(
            request_id=request_id,
            kind="generate",
            model=LOCAL_MODEL,
            profile=capacity.profile,
            capacity=capacity.parallel_slots,
            capacity_source=capacity.source,
            started=started,
            queue_wait_ms=permit.queue_wait_ms,
            admission_slot=permit.slot_index,
            retry_index=retry_index,
            outcome="success",
            finish_reason=result.get("finish_reason"),
            response_data=response_data,
        )
        record_local_inference(telemetry)
        result["inference"] = telemetry
        if input_budget is not None:
            result["input_budget"] = input_budget
        return result
    except BaseException as exc:
        if isinstance(exc, KeyboardInterrupt | SystemExit):
            raise
        if permit is not None:
            permit.release()
        outcome = (
            "cancelled"
            if isinstance(exc, asyncio.CancelledError)
            else "timeout"
            if isinstance(exc, (LocalAdmissionTimeout, httpx.TimeoutException))
            else "error"
        )
        record_local_inference(
            _telemetry_record(
                request_id=request_id,
                kind="generate",
                model=LOCAL_MODEL,
                profile=capacity.profile,
                capacity=capacity.parallel_slots,
                capacity_source=capacity.source,
                started=started,
                queue_wait_ms=(
                    permit.queue_wait_ms
                    if permit is not None
                    else (time.monotonic() - started) * 1000.0
                ),
                admission_slot=permit.slot_index if permit is not None else None,
                retry_index=retry_index,
                outcome=outcome,
                reason_code=getattr(exc, "reason_code", None)
                or classify_provider_error(exc, "local"),
            )
        )
        raise


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
