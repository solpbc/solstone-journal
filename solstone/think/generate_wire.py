# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""One-record JSON stdin/stdout bridge to :func:`generate_with_result`.

This module deliberately owns protocol validation only. Provider selection,
attestation, journaling, and local-model state stay in ``think.models``.
"""

from __future__ import annotations

import base64
import io
import json
import logging
import math
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from PIL import Image

from solstone.think.models import (
    AttestationFailedError,
    AttestationNotVerifiedError,
    AttestationStaleError,
    IncompleteJSONError,
    IncompleteTextError,
    NoBrainConfiguredError,
    ProviderResponseInvalidError,
    SchemaValidationError,
    generate_with_result,
    resolve_provider,
)
from solstone.think.responsiveness import NonResponsiveOutputError

REQUEST_SCHEMA = "solstone-generate-request-v1"
RESPONSE_SCHEMA = "solstone-generate-response-v1"
ERROR_SCHEMA = "solstone-generate-error-v1"

_REQUEST_FIELDS = frozenset(
    {
        "schema",
        "contents",
        "context",
        "temperature",
        "max_output_tokens",
        "system_instruction",
        "json_output",
        "json_schema",
        "thinking_budget",
        "timeout_s",
        "num_retries",
        "inference_retry_index",
        "local_exclusive_admission",
        "enforce_responsiveness",
    }
)
_RESULT_FIELDS = frozenset(
    {
        "text",
        "model",
        "usage",
        "finish_reason",
        "thinking",
        "schema_validation",
        "input_budget",
        "request_budget",
        "inference",
    }
)
_IMAGE_MIME_TYPES = frozenset({"image/png", "image/jpeg", "image/gif", "image/webp"})


@dataclass(frozen=True)
class WireError(Exception):
    reason: str
    detail: str
    exit_code: int = 75

    def as_json(self) -> dict[str, str]:
        return {"schema": ERROR_SCHEMA, "reason": self.reason, "detail": self.detail}


def _malformed(detail: str) -> WireError:
    return WireError("malformed-request", detail, 64)


def _is_int(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _is_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def _decode_contents(value: Any) -> list[Any]:
    if not isinstance(value, list) or not value:
        raise _malformed("contents must be a non-empty array")
    decoded: list[Any] = []
    for index, part in enumerate(value):
        if not isinstance(part, dict):
            raise _malformed(f"contents[{index}] must be an object")
        kind = part.get("type")
        if kind == "text":
            if set(part) != {"type", "text"} or not isinstance(part.get("text"), str):
                raise _malformed(f"contents[{index}] text part is invalid")
            decoded.append(part["text"])
            continue
        if kind != "image":
            raise _malformed(f"contents[{index}] has an unknown type")
        if set(part) != {"type", "data", "mime_type"}:
            raise _malformed(f"contents[{index}] image part has unknown fields")
        data, mime_type = part.get("data"), part.get("mime_type")
        if not isinstance(data, str) or not isinstance(mime_type, str):
            raise _malformed(f"contents[{index}] image part has the wrong type")
        if mime_type not in _IMAGE_MIME_TYPES:
            raise _malformed(f"contents[{index}] has an unsupported image MIME type")
        try:
            raw = base64.b64decode(data, validate=True)
            image = Image.open(io.BytesIO(raw))
            image.load()
        except Exception as exc:
            raise _malformed(f"contents[{index}] image is invalid") from exc
        decoded.append(image)
    return decoded


def _request_kwargs(request: dict[str, Any]) -> dict[str, Any]:
    unknown = set(request) - _REQUEST_FIELDS
    if unknown:
        raise _malformed(f"unknown request field: {sorted(unknown)[0]}")
    if request.get("schema") != REQUEST_SCHEMA:
        raise _malformed("request schema is not supported")
    if not isinstance(request.get("context"), str):
        raise _malformed("context has the wrong type")
    kwargs: dict[str, Any] = {
        "contents": _decode_contents(request.get("contents")),
        "context": request["context"],
        "temperature": request.get("temperature", 0.3),
        "max_output_tokens": request.get("max_output_tokens", 16384),
        "system_instruction": request.get("system_instruction"),
        "json_output": request.get("json_output", False),
        "json_schema": request.get("json_schema"),
        "thinking_budget": request.get("thinking_budget"),
        "timeout_s": request.get("timeout_s"),
        "num_retries": request.get("num_retries"),
        "inference_retry_index": request.get("inference_retry_index", 0),
        "local_exclusive_admission": request.get("local_exclusive_admission", False),
        "enforce_responsiveness": request.get("enforce_responsiveness", True),
    }
    if not _is_number(kwargs["temperature"]):
        raise _malformed("temperature has the wrong type")
    if not _is_int(kwargs["max_output_tokens"]):
        raise _malformed("max_output_tokens has the wrong type")
    if kwargs["system_instruction"] is not None and not isinstance(kwargs["system_instruction"], str):
        raise _malformed("system_instruction has the wrong type")
    if not isinstance(kwargs["json_output"], bool):
        raise _malformed("json_output has the wrong type")
    if kwargs["json_schema"] is not None and not isinstance(kwargs["json_schema"], dict):
        raise _malformed("json_schema has the wrong type")
    for name in ("thinking_budget", "num_retries"):
        if kwargs[name] is not None and not _is_int(kwargs[name]):
            raise _malformed(f"{name} has the wrong type")
    if kwargs["timeout_s"] is not None and not _is_number(kwargs["timeout_s"]):
        raise _malformed("timeout_s has the wrong type")
    if not _is_int(kwargs["inference_retry_index"]):
        raise _malformed("inference_retry_index has the wrong type")
    for name in ("local_exclusive_admission", "enforce_responsiveness"):
        if not isinstance(kwargs[name], bool):
            raise _malformed(f"{name} has the wrong type")
    return kwargs


def _exception_to_wire_error(exc: Exception) -> WireError:
    # Children precede their common attestation base class.
    if isinstance(exc, AttestationFailedError):
        return WireError("attestation-failed", "provider attestation failed")
    if isinstance(exc, AttestationStaleError):
        return WireError("attestation-stale", "provider attestation is stale")
    if isinstance(exc, AttestationNotVerifiedError):
        return WireError("attestation-not-verified", "provider attestation is not verified")
    if isinstance(exc, NoBrainConfiguredError):
        return WireError("no-engine-configured", "no thinking engine is configured", 69)
    if isinstance(exc, IncompleteJSONError):
        return WireError("incomplete-json", "provider returned incomplete JSON")
    if isinstance(exc, IncompleteTextError):
        return WireError("incomplete-text", "provider returned incomplete text")
    if isinstance(exc, ProviderResponseInvalidError):
        return WireError("provider-response-invalid", "provider response is invalid")
    if isinstance(exc, SchemaValidationError):
        return WireError("schema-validation-failed", "provider response failed schema validation")
    if isinstance(exc, NonResponsiveOutputError):
        return WireError("non-responsive-output", "provider output was not responsive")
    return WireError("provider-response-invalid", "unexpected provider execution failure")


def handle_request(request: dict[str, Any]) -> dict[str, Any]:
    """Validate a decoded request and return a response envelope.

    ``WireError`` is intentionally the only protocol-level exception exposed to
    the thin CLI wrapper, making the function suitable for direct unit tests.
    """
    if not isinstance(request, dict):
        raise _malformed("request must be an object")
    try:
        result = generate_with_result(**_request_kwargs(request))
    except WireError:
        raise
    except Exception as exc:
        raise _exception_to_wire_error(exc) from exc
    if not isinstance(result, dict) or "text" not in result or not isinstance(result["text"], str):
        raise WireError("provider-response-invalid", "provider result is invalid")
    unknown = set(result) - _RESULT_FIELDS
    if unknown:
        raise WireError("provider-response-invalid", "provider result has unknown fields")
    response = {"schema": RESPONSE_SCHEMA, "result": result}
    try:
        json.dumps(response, allow_nan=False)
    except (TypeError, ValueError) as exc:
        raise WireError("provider-response-invalid", "provider result is not JSON serializable") from exc
    return response


def _configure_protocol_logging() -> None:
    """Keep provider logs out of the protocol streams.

    The shim has no interactive diagnostics surface; a null root handler is
    preferable to interleaving a provider warning with its single JSON record.
    """
    logging.basicConfig(handlers=[logging.NullHandler()], force=True)
    logging.disable(logging.CRITICAL)


def _main_v1() -> None:
    _configure_protocol_logging()
    try:
        raw = sys.stdin.read()
        request = json.loads(raw)
    except json.JSONDecodeError:
        error = _malformed("stdin is not valid JSON")
    except Exception:
        error = WireError("malformed-request", "failed to read request", 64)
    else:
        try:
            response = handle_request(request)
        except WireError as exc:
            error = exc
        else:
            sys.stdout.write(json.dumps(response, allow_nan=False) + "\n")
            return
    sys.stderr.write(json.dumps(error.as_json(), allow_nan=False) + "\n")
    raise SystemExit(error.exit_code)


_GENERATE_CONTRACT_PATH = Path(__file__).parents[2] / "core/fixtures/generate_contract.json"


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
        "max_output_tokens": request.get("max_output_tokens", defaults["max_output_tokens"]),
        "system_instruction": request.get("system_instruction", defaults["system_instruction"]),
        "json_output": request.get("json_output", defaults["json_output"]),
        "json_schema": request.get("json_schema", defaults["json_schema"]),
        "thinking_budget": request.get("thinking_budget", defaults["thinking_budget"]),
        "timeout_s": request.get("timeout_s", defaults["timeout_s"]),
        "num_retries": request.get("transport_retries", defaults["transport_retries"]),
        "inference_retry_index": request.get("attempt_index", defaults["attempt_index"]),
        "local_exclusive_admission": request.get("exclusive_admission", defaults["exclusive_admission"]),
        "enforce_responsiveness": request.get("enforce_responsiveness", defaults["enforce_responsiveness"]),
    }
    if not _is_number(kwargs["temperature"]):
        raise ValueError("temperature has the wrong type")
    if not _is_int(kwargs["max_output_tokens"]):
        raise ValueError("max_output_tokens has the wrong type")
    if kwargs["system_instruction"] is not None and not isinstance(kwargs["system_instruction"], str):
        raise ValueError("system_instruction has the wrong type")
    if not isinstance(kwargs["json_output"], bool):
        raise ValueError("json_output has the wrong type")
    if kwargs["json_schema"] is not None and not isinstance(kwargs["json_schema"], dict):
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


def _v2_refusal(exc: Exception, request_id: str | None, provider: str | None) -> dict[str, Any]:
    contract = _generate_contract()
    reason, detail = _v2_exception_details(exc)
    reason_code = getattr(exc, "reason_code", None)
    entry = next(
        (
            item
            for item in contract["reason_codes"]
            if item["code"] == reason_code
        ),
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


def _v2_generated(
    result: dict[str, Any], request_id: str | None, resolved_model: str
) -> dict[str, Any]:
    if not isinstance(result.get("text"), str):
        raise RuntimeError("provider result is invalid")
    contract = _generate_contract()
    return {
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


def _main_v2_one_shot() -> None:
    try:
        raw = sys.stdin.read()
        request = json.loads(raw)
        if not isinstance(request, dict):
            raise ValueError("request must be an object")
        kwargs, request_id = _v2_request_kwargs(request)
    except (json.JSONDecodeError, ValueError) as exc:
        error = _v2_protocol_error(
            "stdin is not valid JSON" if isinstance(exc, json.JSONDecodeError) else str(exc)
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
            response = _v2_generated(result, request_id, model)
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


def main() -> None:
    if sys.argv[1:] == ["--contract"]:
        sys.stdout.write(_GENERATE_CONTRACT_PATH.read_text(encoding="utf-8"))
        return
    if sys.argv[1:] == ["--one-shot"]:
        _main_v2_one_shot()
        return
    _main_v1()


if __name__ == "__main__":
    main()
