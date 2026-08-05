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


def main() -> None:
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


if __name__ == "__main__":
    main()
