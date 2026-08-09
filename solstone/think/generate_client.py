# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Native one-shot client for the generate contract."""

from __future__ import annotations

import asyncio
import json
import subprocess
from functools import lru_cache
from pathlib import Path
from typing import Any

from solstone.think import core_handshake
from solstone.think.providers._image import (
    CLOUD_IMAGE_MEDIA_TYPES,
    encode_image_part,
    is_image_part,
)

_GENERATE_CONTRACT_PATH = (
    Path(__file__).parents[2] / "core/fixtures/generate_contract.json"
)


@lru_cache(maxsize=1)
def _generate_contract() -> dict[str, Any]:
    with _GENERATE_CONTRACT_PATH.open(encoding="utf-8") as handle:
        return json.load(handle)


@lru_cache(maxsize=1)
def _native_binary() -> Path:
    handshake = core_handshake.check_solstone_core_handshake()
    if handshake.status != "ok":
        detail = handshake.message or "unknown solstone-core handshake failure"
        raise RuntimeError(f"generate requires solstone-core: {detail}")
    return core_handshake.helper_path_for_executable()


def _contents_parts(contents: str | list[Any]) -> list[dict[str, str]]:
    values = [contents] if isinstance(contents, str) else contents
    if not values:
        raise ValueError("generate contents must be non-empty")

    parts: list[dict[str, str]] = []
    for item in values:
        if isinstance(item, str):
            parts.append({"type": "text", "text": item})
        elif is_image_part(item):
            mime_type, data = encode_image_part(item, accepts=CLOUD_IMAGE_MEDIA_TYPES)
            parts.append({"type": "image", "mime_type": mime_type, "data": data})
        else:
            raise ValueError("generate contents must contain only text or image parts")
    return parts


def _request(
    *,
    contents: str | list[Any],
    context: str,
    temperature: float,
    max_output_tokens: int,
    system_instruction: str | None,
    json_output: bool,
    json_schema: dict | None,
    thinking_budget: int | None,
    timeout_s: float | None,
    num_retries: int | None,
    inference_retry_index: int,
    local_exclusive_admission: bool,
    enforce_responsiveness: bool,
) -> dict[str, Any]:
    contract = _generate_contract()
    return {
        "schema": contract["schema_identifiers"]["request"],
        "context": context,
        "contents": _contents_parts(contents),
        "system_instruction": system_instruction,
        "temperature": temperature,
        "max_output_tokens": max_output_tokens,
        "thinking_budget": thinking_budget,
        "timeout_s": timeout_s,
        "json_output": json_output,
        "json_schema": json_schema,
        "enforce_responsiveness": enforce_responsiveness,
        "attempt_index": inference_retry_index,
        "exclusive_admission": local_exclusive_admission,
        "transport_retries": num_retries,
    }


def _decode_protocol_response(raw: str, *, session: bool = False) -> dict[str, Any]:
    try:
        response = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise RuntimeError("generate native command returned invalid JSON") from exc
    if not isinstance(response, dict):
        raise RuntimeError("generate native command returned a non-object response")

    contract = _generate_contract()
    if response.get("schema") != contract["schema_identifiers"]["response"]:
        raise RuntimeError(
            "generate native command returned an unexpected response schema"
        )
    response_id = response.get("id")
    if session:
        if not isinstance(response_id, str):
            raise RuntimeError(
                "generate native command returned an invalid response id"
            )
    elif response_id is not None:
        raise RuntimeError("generate native command returned an unexpected response id")
    if response.get("outcome") not in contract["outcomes"]:
        raise RuntimeError("generate native command returned an unsupported outcome")
    return response


def _generated_result(response: dict[str, Any]) -> dict[str, Any]:
    if not isinstance(response.get("text"), str):
        raise RuntimeError("generated response text is invalid")
    if not isinstance(response.get("model"), str):
        raise RuntimeError("generated response model is invalid")
    if not isinstance(response.get("usage"), dict):
        raise RuntimeError("generated response usage is invalid")
    if not isinstance(response.get("finish_reason"), str):
        raise RuntimeError("generated response finish reason is invalid")

    fields = _generate_contract()["response"]["outcomes"]["generated"]["fields"]
    return {
        field: response[field]
        for field in fields
        if field not in {"schema", "id", "outcome"} and field in response
    }


@lru_cache(maxsize=1)
def _refusal_exception_names() -> dict[str, str]:
    names: dict[str, str] = {}
    for vector in _generate_contract()["conformance_vectors"]:
        source = vector.get("source")
        response = vector.get("response")
        if not isinstance(source, dict) or not isinstance(response, dict):
            continue
        exception_name = source.get("exception")
        reason = response.get("reason")
        if isinstance(exception_name, str) and isinstance(reason, str):
            names[reason] = exception_name
    return names


def _refusal_exception(response: dict[str, Any]) -> Exception:
    from solstone.think import models
    from solstone.think.responsiveness import NonResponsiveOutputError

    reason = response["reason"]
    detail = response["detail"]
    exception_name = _refusal_exception_names().get(reason)
    if exception_name == "NoBrainConfiguredError":
        exc: Exception = models.NoBrainConfiguredError()
    elif exception_name == "IncompleteJSONError":
        exc = models.IncompleteJSONError(reason, "")
    elif exception_name == "IncompleteTextError":
        exc = models.IncompleteTextError(reason, "")
    elif exception_name == "ProviderResponseInvalidError":
        # No lane discriminator: blank_visible_output is an accepted diagnostic-only gap.
        exc = models.ProviderResponseInvalidError(reason)
    elif exception_name == "SchemaValidationError":
        exc = models.SchemaValidationError(
            [{"path": "", "constraint": "schema_validation", "message": detail}],
            "",
        )
    elif exception_name == "NonResponsiveOutputError":
        exc = NonResponsiveOutputError()
    elif exception_name == "AttestationNotVerifiedError":
        exc = models.AttestationNotVerifiedError()
    elif exception_name == "AttestationFailedError":
        exc = models.AttestationFailedError(detail)
    elif exception_name == "AttestationStaleError":
        exc = models.AttestationStaleError(detail)
    else:
        exc = models.ProviderResponseInvalidError(reason)

    exc.reason = reason
    exc.reason_code = response["reason_code"]
    exc.retryable = response["retryable"]
    exc.blocking = response["blocking"]
    exc.reset_at_ms = response["reset_at_ms"]
    exc.provider = response["provider"]
    exc.detail = detail
    return exc


def _response_result_from_response(response: dict[str, Any]) -> dict[str, Any]:
    if response["outcome"] == "generated":
        return _generated_result(response)

    required = (
        "reason",
        "reason_code",
        "retryable",
        "blocking",
        "reset_at_ms",
        "provider",
        "detail",
    )
    if any(field not in response for field in required):
        raise RuntimeError("refused response is missing required fields")
    if not isinstance(response["reason"], str) or not isinstance(
        response["detail"], str
    ):
        raise RuntimeError("refused response has invalid text fields")
    if response["reason_code"] is not None and not isinstance(
        response["reason_code"], str
    ):
        raise RuntimeError("refused response has an invalid reason code")
    if not isinstance(response["blocking"], bool) or not isinstance(
        response["retryable"], bool
    ):
        raise RuntimeError("refused response has invalid classification fields")
    if response["reset_at_ms"] is not None and not isinstance(
        response["reset_at_ms"], int
    ):
        raise RuntimeError("refused response has an invalid reset time")
    if response["provider"] is not None and not isinstance(response["provider"], str):
        raise RuntimeError("refused response has an invalid provider")
    raise _refusal_exception(response)


def _response_result(raw: str) -> dict[str, Any]:
    return _response_result_from_response(_decode_protocol_response(raw))


def _run_one_shot(request: dict[str, Any]) -> dict[str, Any]:
    try:
        process = subprocess.Popen(
            [str(_native_binary()), "generate", "--one-shot"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
    except OSError as exc:
        raise RuntimeError(f"generate native command could not start: {exc}") from exc
    stdout, stderr = process.communicate(json.dumps(request, allow_nan=False))
    if process.returncode != 0:
        detail = stderr.strip() or "no diagnostic output"
        raise RuntimeError(
            f"generate native command failed (exit {process.returncode}): {detail}"
        )
    return _response_result(stdout)


async def _arun_one_shot(request: dict[str, Any]) -> dict[str, Any]:
    try:
        process = await asyncio.create_subprocess_exec(
            str(_native_binary()),
            "generate",
            "--one-shot",
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
    except OSError as exc:
        raise RuntimeError(f"generate native command could not start: {exc}") from exc
    stdout, stderr = await process.communicate(
        json.dumps(request, allow_nan=False).encode()
    )
    if process.returncode != 0:
        detail = stderr.decode().strip() or "no diagnostic output"
        raise RuntimeError(
            f"generate native command failed (exit {process.returncode}): {detail}"
        )
    return _response_result(stdout.decode())


def _text_result(
    result: dict[str, Any], *, json_output: bool, json_schema: dict | None
) -> str:
    from solstone.think.models import SchemaValidationError, finish_reason_error

    if json_output:
        error = finish_reason_error(result, json_output=True)
        if error is not None:
            raise error
    validation = result.get("schema_validation")
    if (
        json_schema is not None
        and isinstance(validation, dict)
        and validation.get("valid") is False
    ):
        raise SchemaValidationError(validation.get("errors") or [], result["text"])
    return result["text"]


def generate_with_result(
    contents: str | list[Any],
    context: str,
    temperature: float = 0.3,
    max_output_tokens: int = 8192 * 2,
    system_instruction: str | None = None,
    json_output: bool = False,
    *,
    json_schema: dict | None = None,
    thinking_budget: int | None = None,
    timeout_s: float | None = None,
    num_retries: int | None = None,
    inference_retry_index: int = 0,
    local_exclusive_admission: bool = False,
    enforce_responsiveness: bool = True,
) -> dict[str, Any]:
    if json_schema is not None:
        json_output = True
    return _run_one_shot(
        _request(
            contents=contents,
            context=context,
            temperature=temperature,
            max_output_tokens=max_output_tokens,
            system_instruction=system_instruction,
            json_output=json_output,
            json_schema=json_schema,
            thinking_budget=thinking_budget,
            timeout_s=timeout_s,
            num_retries=num_retries,
            inference_retry_index=inference_retry_index,
            local_exclusive_admission=local_exclusive_admission,
            enforce_responsiveness=enforce_responsiveness,
        )
    )


def generate(
    contents: str | list[Any],
    context: str,
    temperature: float = 0.3,
    max_output_tokens: int = 8192 * 2,
    system_instruction: str | None = None,
    json_output: bool = False,
    *,
    json_schema: dict | None = None,
    thinking_budget: int | None = None,
    timeout_s: float | None = None,
) -> str:
    if json_schema is not None:
        json_output = True
    result = generate_with_result(
        contents,
        context,
        temperature,
        max_output_tokens,
        system_instruction,
        json_output,
        json_schema=json_schema,
        thinking_budget=thinking_budget,
        timeout_s=timeout_s,
    )
    return _text_result(
        result,
        json_output=json_output,
        json_schema=json_schema,
    )


async def agenerate_with_result(
    contents: str | list[Any],
    context: str,
    temperature: float = 0.3,
    max_output_tokens: int = 8192 * 2,
    system_instruction: str | None = None,
    json_output: bool = False,
    *,
    json_schema: dict | None = None,
    thinking_budget: int | None = None,
    timeout_s: float | None = None,
    num_retries: int | None = None,
    inference_retry_index: int = 0,
    local_exclusive_admission: bool = False,
    enforce_responsiveness: bool = True,
) -> dict[str, Any]:
    if json_schema is not None:
        json_output = True
    return await _arun_one_shot(
        _request(
            contents=contents,
            context=context,
            temperature=temperature,
            max_output_tokens=max_output_tokens,
            system_instruction=system_instruction,
            json_output=json_output,
            json_schema=json_schema,
            thinking_budget=thinking_budget,
            timeout_s=timeout_s,
            num_retries=num_retries,
            inference_retry_index=inference_retry_index,
            local_exclusive_admission=local_exclusive_admission,
            enforce_responsiveness=enforce_responsiveness,
        )
    )


async def agenerate(
    contents: str | list[Any],
    context: str,
    temperature: float = 0.3,
    max_output_tokens: int = 8192 * 2,
    system_instruction: str | None = None,
    json_output: bool = False,
    *,
    json_schema: dict | None = None,
    thinking_budget: int | None = None,
    timeout_s: float | None = None,
) -> str:
    if json_schema is not None:
        json_output = True
    result = await agenerate_with_result(
        contents,
        context,
        temperature,
        max_output_tokens,
        system_instruction,
        json_output,
        json_schema=json_schema,
        thinking_budget=thinking_budget,
        timeout_s=timeout_s,
    )
    return _text_result(
        result,
        json_output=json_output,
        json_schema=json_schema,
    )
