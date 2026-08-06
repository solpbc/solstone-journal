# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""One-record JSON stdin/stdout bridge to :func:`generate_with_result`.

This module deliberately owns protocol validation only. Provider selection,
attestation, journaling, and local-model state stay in ``think.models``.
"""

from __future__ import annotations

import asyncio
import base64
import io
import json
import math
import sys
from pathlib import Path
from typing import Any

from PIL import Image

from solstone.think.models import (
    agenerate_with_result,
    generate_with_result,
    resolve_provider,
)

_IMAGE_MIME_TYPES = frozenset({"image/png", "image/jpeg", "image/gif", "image/webp"})
_SESSION_LINE_LIMIT = 64 * 1024 * 1024


def _is_int(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _is_number(value: Any) -> bool:
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and math.isfinite(value)
    )


def _decode_contents(value: Any) -> list[Any]:
    if not isinstance(value, list) or not value:
        raise ValueError("contents must be a non-empty array")
    decoded: list[Any] = []
    for index, part in enumerate(value):
        if not isinstance(part, dict):
            raise ValueError(f"contents[{index}] must be an object")
        kind = part.get("type")
        if kind == "text":
            if set(part) != {"type", "text"} or not isinstance(part.get("text"), str):
                raise ValueError(f"contents[{index}] text part is invalid")
            decoded.append(part["text"])
            continue
        if kind != "image":
            raise ValueError(f"contents[{index}] has an unknown type")
        if set(part) != {"type", "data", "mime_type"}:
            raise ValueError(f"contents[{index}] image part has unknown fields")
        data, mime_type = part.get("data"), part.get("mime_type")
        if not isinstance(data, str) or not isinstance(mime_type, str):
            raise ValueError(f"contents[{index}] image part has the wrong type")
        if mime_type not in _IMAGE_MIME_TYPES:
            raise ValueError(f"contents[{index}] has an unsupported image MIME type")
        try:
            raw = base64.b64decode(data, validate=True)
            image = Image.open(io.BytesIO(raw))
            image.load()
        except Exception as exc:
            raise ValueError(f"contents[{index}] image is invalid") from exc
        decoded.append(image)
    return decoded


_GENERATE_CONTRACT_PATH = (
    Path(__file__).parents[2] / "core/fixtures/generate_contract.json"
)


def _generate_contract() -> dict[str, Any]:
    with _GENERATE_CONTRACT_PATH.open(encoding="utf-8") as handle:
        return json.load(handle)


def _v2_protocol_error(detail: str, *, request_id: str | None = None) -> dict[str, Any]:
    contract = _generate_contract()
    return {
        "schema": contract["schema_identifiers"]["error"],
        "id": request_id,
        "reason": "malformed-request",
        "detail": detail,
    }


def _write_v2_error(error: dict[str, Any], exit_code: int) -> None:
    sys.stderr.write(json.dumps(error, allow_nan=False) + "\n")
    raise SystemExit(exit_code)


def _v2_internal_error(request_id: str | None) -> dict[str, Any]:
    return {
        "schema": _generate_contract()["schema_identifiers"]["error"],
        "id": request_id,
        "reason": "internal-failure",
        "detail": "failed to encode provider result",
    }


def _v2_request_kwargs(request: dict[str, Any]) -> tuple[dict[str, Any], str | None]:
    contract = _generate_contract()
    allowed = frozenset(contract["request"]["fields"])
    unknown = set(request) - allowed
    if unknown:
        raise ValueError(f"unknown request field: {sorted(unknown)[0]}")
    if request.get("schema") != contract["schema_identifiers"]["request"]:
        raise ValueError("request schema is not supported")
    if not isinstance(request.get("context"), str):
        raise ValueError("context has the wrong type")
    request_id = request.get("id")
    if request_id is not None and not isinstance(request_id, str):
        raise ValueError("id has the wrong type")
    defaults = contract["request"]["defaults"]
    kwargs: dict[str, Any] = {
        "contents": _decode_contents(request.get("contents")),
        "context": request["context"],
        "temperature": request.get("temperature", defaults["temperature"]),
        "max_output_tokens": request.get(
            "max_output_tokens", defaults["max_output_tokens"]
        ),
        "system_instruction": request.get(
            "system_instruction", defaults["system_instruction"]
        ),
        "json_output": request.get("json_output", defaults["json_output"]),
        "json_schema": request.get("json_schema", defaults["json_schema"]),
        "thinking_budget": request.get("thinking_budget", defaults["thinking_budget"]),
        "timeout_s": request.get("timeout_s", defaults["timeout_s"]),
        "num_retries": request.get("transport_retries", defaults["transport_retries"]),
        "inference_retry_index": request.get(
            "attempt_index", defaults["attempt_index"]
        ),
        "local_exclusive_admission": request.get(
            "exclusive_admission", defaults["exclusive_admission"]
        ),
        "enforce_responsiveness": request.get(
            "enforce_responsiveness", defaults["enforce_responsiveness"]
        ),
    }
    if not _is_number(kwargs["temperature"]):
        raise ValueError("temperature has the wrong type")
    if not _is_int(kwargs["max_output_tokens"]):
        raise ValueError("max_output_tokens has the wrong type")
    if kwargs["system_instruction"] is not None and not isinstance(
        kwargs["system_instruction"], str
    ):
        raise ValueError("system_instruction has the wrong type")
    if not isinstance(kwargs["json_output"], bool):
        raise ValueError("json_output has the wrong type")
    if kwargs["json_schema"] is not None and not isinstance(
        kwargs["json_schema"], dict
    ):
        raise ValueError("json_schema has the wrong type")
    for name in ("thinking_budget", "num_retries"):
        if kwargs[name] is not None and not _is_int(kwargs[name]):
            raise ValueError(f"{name} has the wrong type")
    if kwargs["timeout_s"] is not None and not _is_number(kwargs["timeout_s"]):
        raise ValueError("timeout_s has the wrong type")
    if not _is_int(kwargs["inference_retry_index"]):
        raise ValueError("attempt_index has the wrong type")
    for name in ("local_exclusive_admission", "enforce_responsiveness"):
        if not isinstance(kwargs[name], bool):
            raise ValueError(f"{name} has the wrong type")
    return kwargs, request_id


def _v2_exception_details(exc: Exception) -> tuple[str, str]:
    for vector in _generate_contract()["conformance_vectors"]:
        source = vector.get("source", {})
        if source.get("exception") == type(exc).__name__:
            response = vector["response"]
            return response["reason"], response["detail"]
    for vector in _generate_contract()["conformance_vectors"]:
        if vector.get("id") == "refused-provider-response-invalid":
            response = vector["response"]
            return response["reason"], response["detail"]
    raise RuntimeError("generate contract has no provider-response-invalid vector")


def _v2_refusal(
    exc: Exception, request_id: str | None, provider: str | None
) -> dict[str, Any]:
    contract = _generate_contract()
    reason, detail = _v2_exception_details(exc)
    reason_code = getattr(exc, "reason_code", None)
    entry = next(
        (item for item in contract["reason_codes"] if item["code"] == reason_code),
        None,
    )
    if entry is None:
        classification = contract["unknown_member"]
    else:
        classification = entry
    return {
        "schema": contract["schema_identifiers"]["response"],
        "id": request_id,
        "outcome": "refused",
        "reason": reason,
        "reason_code": reason_code if isinstance(reason_code, str) else None,
        "retryable": classification["retryable"],
        "blocking": classification["blocking"],
        "reset_at_ms": getattr(exc, "reset_at_ms", None),
        "provider": provider,
        "detail": detail,
    }


def _v2_hints_applied(request: dict[str, Any], result: dict[str, Any]) -> list[str]:
    inference = result.get("inference")
    hints: list[str] = []
    if request.get("attempt_index", 0) and inference is not None:
        hints.append("attempt_index")
    if request.get("exclusive_admission", False) and inference is not None:
        hints.append("exclusive_admission")
    return hints


def _v2_generated(
    result: dict[str, Any],
    request_id: str | None,
    resolved_model: str,
    request: dict[str, Any],
) -> dict[str, Any]:
    if not isinstance(result.get("text"), str):
        raise RuntimeError("provider result is invalid")
    contract = _generate_contract()
    response = {
        "schema": contract["schema_identifiers"]["response"],
        "id": request_id,
        "outcome": "generated",
        "text": result["text"],
        "model": result.get("model") or resolved_model,
        "usage": result.get("usage") or {},
        "finish_reason": result.get("finish_reason") or "unknown",
        "thinking": result.get("thinking"),
        "schema_validation": result.get("schema_validation"),
        "input_budget": result.get("input_budget"),
        "request_budget": result.get("request_budget"),
        "inference": result.get("inference"),
    }
    hints_applied = _v2_hints_applied(request, result)
    if hints_applied:
        response["hints_applied"] = hints_applied
    return response


def _main_v2_one_shot() -> None:
    try:
        raw = sys.stdin.read()
        request = json.loads(raw)
        if not isinstance(request, dict):
            raise ValueError("request must be an object")
        kwargs, request_id = _v2_request_kwargs(request)
    except (json.JSONDecodeError, ValueError) as exc:
        error = _v2_protocol_error(
            "stdin is not valid JSON"
            if isinstance(exc, json.JSONDecodeError)
            else str(exc)
        )
        _write_v2_error(error, _generate_contract()["exit_codes"]["malformed_request"])
    try:
        provider, model = resolve_provider("generate")
        result = generate_with_result(**kwargs)
    except Exception as exc:
        try:
            response = _v2_refusal(exc, request_id, locals().get("provider"))
        except Exception:
            _write_v2_error(
                _v2_internal_error(request_id),
                _generate_contract()["exit_codes"]["internal_failure"],
            )
    else:
        try:
            response = _v2_generated(result, request_id, model, request)
        except Exception:
            _write_v2_error(
                _v2_internal_error(request_id),
                _generate_contract()["exit_codes"]["internal_failure"],
            )
    try:
        encoded = json.dumps(response, allow_nan=False)
    except Exception:
        _write_v2_error(
            _v2_internal_error(request_id),
            _generate_contract()["exit_codes"]["internal_failure"],
        )
    try:
        sys.stdout.write(encoded + "\n")
    except Exception:
        _write_v2_error(
            _v2_internal_error(request_id),
            _generate_contract()["exit_codes"]["internal_failure"],
        )


def _is_session_terminal(record: dict[str, Any]) -> bool:
    contract = _generate_contract()
    terminal = contract["framing"]["session"]["terminal"]
    if record.get("schema") != terminal["schema"]:
        return False
    if set(record) != set(terminal["fields"]):
        raise ValueError("terminal record has unknown fields")
    return True


def _write_v2_response(response: dict[str, Any]) -> None:
    encoded = json.dumps(response, allow_nan=False)
    sys.stdout.write(encoded + "\n")
    sys.stdout.flush()


async def _cancel_session_tasks(tasks: set[asyncio.Task[None]]) -> None:
    for task in tasks:
        task.cancel()
    if tasks:
        await asyncio.gather(*tasks, return_exceptions=True)


async def _execute_session_request(
    request: dict[str, Any],
    kwargs: dict[str, Any],
    request_id: str,
    semaphore: asyncio.Semaphore,
    aborting: asyncio.Event,
) -> None:
    async with semaphore:
        if aborting.is_set():
            return
        provider: str | None = None
        try:
            provider, model = resolve_provider("generate")
            result = await agenerate_with_result(**kwargs)
        except asyncio.CancelledError:
            raise
        except Exception as exc:
            try:
                response = _v2_refusal(exc, request_id, provider)
            except Exception:
                _write_v2_error(
                    _v2_internal_error(request_id),
                    _generate_contract()["exit_codes"]["internal_failure"],
                )
        else:
            try:
                response = _v2_generated(result, request_id, model, request)
            except Exception:
                _write_v2_error(
                    _v2_internal_error(request_id),
                    _generate_contract()["exit_codes"]["internal_failure"],
                )
        if not aborting.is_set():
            try:
                _write_v2_response(response)
            except Exception:
                _write_v2_error(
                    _v2_internal_error(request_id),
                    _generate_contract()["exit_codes"]["internal_failure"],
                )


async def _main_v2_session(max_in_flight: int) -> None:
    loop = asyncio.get_running_loop()
    reader = asyncio.StreamReader(limit=_SESSION_LINE_LIMIT)
    protocol = asyncio.StreamReaderProtocol(reader)
    transport, _ = await loop.connect_read_pipe(lambda: protocol, sys.stdin.buffer)
    semaphore = asyncio.Semaphore(max_in_flight)
    aborting = asyncio.Event()
    tasks: set[asyncio.Task[None]] = set()

    try:
        while True:
            raw = await reader.readline()
            if not raw:
                aborting.set()
                await _cancel_session_tasks(tasks)
                return
            try:
                record = json.loads(raw)
                if not isinstance(record, dict):
                    raise ValueError("request must be an object")
                if _is_session_terminal(record):
                    await asyncio.gather(*tasks)
                    return
                kwargs, request_id = _v2_request_kwargs(record)
                if request_id is None:
                    raise ValueError("id is required in a session request")
            except (json.JSONDecodeError, ValueError) as exc:
                aborting.set()
                await _cancel_session_tasks(tasks)
                detail = (
                    "stdin is not valid JSON"
                    if isinstance(exc, json.JSONDecodeError)
                    else str(exc)
                )
                _write_v2_error(
                    _v2_protocol_error(detail),
                    _generate_contract()["exit_codes"]["malformed_request"],
                )
            task = asyncio.create_task(
                _execute_session_request(
                    record, kwargs, request_id, semaphore, aborting
                )
            )
            tasks.add(task)
            task.add_done_callback(tasks.discard)
    finally:
        transport.close()


def _session_max_in_flight(arguments: list[str]) -> int:
    contract = _generate_contract()
    session = contract["framing"]["session"]
    concurrency = session["concurrency"]
    if len(arguments) != 3 or arguments[:2] != [
        session["selector"],
        concurrency["flag"],
    ]:
        raise ValueError(
            f"expected {session['selector']} {concurrency['flag']} <positive integer>"
        )
    try:
        max_in_flight = int(arguments[2])
    except ValueError as exc:
        raise ValueError("max-in-flight must be a positive integer") from exc
    if max_in_flight < concurrency["minimum"]:
        raise ValueError("max-in-flight is below the minimum")
    return max_in_flight


def main() -> None:
    arguments = sys.argv[1:]
    if arguments == ["--contract"]:
        sys.stdout.write(_GENERATE_CONTRACT_PATH.read_text(encoding="utf-8"))
        return
    if arguments == ["--one-shot"]:
        _main_v2_one_shot()
        return
    session_selector = _generate_contract()["framing"]["session"]["selector"]
    if arguments and arguments[0] == session_selector:
        try:
            max_in_flight = _session_max_in_flight(arguments)
        except ValueError as exc:
            _write_v2_error(
                _v2_protocol_error(str(exc)),
                _generate_contract()["exit_codes"]["malformed_request"],
            )
        asyncio.run(_main_v2_session(max_in_flight))
        return
    _write_v2_error(
        _v2_protocol_error("expected --contract, --one-shot, or --session"),
        _generate_contract()["exit_codes"]["malformed_request"],
    )


if __name__ == "__main__":
    main()
