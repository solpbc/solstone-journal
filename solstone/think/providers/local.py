# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Bundled local provider backed by llama-server on 127.0.0.1.

The module must remain importable before the local runtime or GGUF files exist.
Network clients and daemon startup are created only inside provider functions.
"""

from __future__ import annotations

import asyncio
import logging
import traceback
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

from solstone.think.models import LOCAL_MODEL
from solstone.think.providers._image import encode_image_part, is_image_part
from solstone.think.providers.shared import (
    GenerateResult,
    classify_provider_error,
    safe_raw,
)

LOG = logging.getLogger(__name__)

_DEFAULT_TIMEOUT = 120.0
_LOCAL_PREFIX = "local/"


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


LOCAL_MODEL_SPECS: dict[str, LocalModelSpec] = {
    LOCAL_MODEL: LocalModelSpec(
        model_id=LOCAL_MODEL,
        repo="Qwen/Qwen2.5-Coder-7B-Instruct-GGUF",
        filename="qwen2.5-coder-7b-instruct-q4_k_m.gguf",
        revision="main",
        sha256="509287f78cb4d4cf6b3843734733b914b2c158e43e22a7f4bf5e963800894d3c",
        size_bytes=4_683_073_536,
        min_ram_bytes=12 * 1024**3,
    ),
}


class LocalProviderError(RuntimeError):
    """Local provider failure with a recovery reason code."""

    def __init__(self, reason_code: str, message: str) -> None:
        super().__init__(message)
        self.reason_code = reason_code


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


def _build_request_body(
    model_id: str,
    messages: list[dict[str, Any]],
    temperature: float,
    max_output_tokens: int,
    json_output: bool,
    json_schema: dict | None,
) -> dict[str, Any]:
    body: dict[str, Any] = {
        "model": model_id,
        "messages": messages,
        "temperature": temperature,
        "max_tokens": max_output_tokens,
        "stream": False,
    }
    if json_schema is not None:
        body["response_format"] = {
            "type": "json_schema",
            "json_schema": {
                "name": "local_schema",
                "schema": json_schema,
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
    return {
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": total_tokens,
    }


def _parse_response(data: dict[str, Any], requested_model: str) -> GenerateResult:
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
        model=data.get("model")
        if isinstance(data.get("model"), str)
        else requested_model,
        usage=_extract_usage(data),
        finish_reason=choice.get("finish_reason"),
        thinking=None,
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
    del thinking_budget, kwargs
    from solstone.think.providers import local_server

    model_id = normalize_model_id(model)
    messages = _build_messages(contents, system_instruction)
    server = local_server.connect()
    body = _build_request_body(
        model_id,
        messages,
        temperature,
        max_output_tokens,
        json_output,
        json_schema,
    )

    import httpx

    response = httpx.post(
        f"{server.base_url}/v1/chat/completions",
        json=body,
        timeout=timeout_s or _DEFAULT_TIMEOUT,
    )
    response.raise_for_status()
    return _parse_response(response.json(), model_id)


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
    return await asyncio.to_thread(
        run_generate,
        contents,
        model,
        temperature,
        max_output_tokens,
        system_instruction,
        json_output,
        thinking_budget,
        json_schema,
        timeout_s,
        **kwargs,
    )


async def run_cogitate(
    config: dict[str, Any],
    on_event: Callable[[dict], None] | None = None,
) -> str:
    from solstone.think.providers import local_server, openhands

    config = {**config, "model": normalize_model_id(config.get("model", LOCAL_MODEL))}
    try:
        local_server.connect()
        return await openhands.run_cogitate(config, on_event=on_event)
    except Exception as exc:
        if on_event and not getattr(exc, "_evented", False):
            reason_code = getattr(exc, "reason_code", None) or classify_provider_error(
                exc, "local"
            )
            on_event(
                {
                    "event": "error",
                    "error": str(exc),
                    "reason_code": reason_code,
                    "provider": "local",
                    "trace": traceback.format_exc(),
                    "raw": safe_raw([{"reason_code": reason_code}]),
                }
            )
            setattr(exc, "_evented", True)
        raise


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
    "LocalModelSpec",
    "LocalProviderError",
    "normalize_model_id",
    "run_generate",
    "run_agenerate",
    "run_cogitate",
    "list_models",
    "validate_key",
]
