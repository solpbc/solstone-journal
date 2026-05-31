#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""OpenAI provider for agents and direct LLM generation.

This module provides the OpenAI provider for the ``journal providers check`` CLI
and run_generate/run_agenerate functions returning GenerateResult.

Common Parameters
-----------------
contents : str or list
    The content to send to the model.
model : str
    Model name, optionally with a reasoning effort suffix.
    Supported suffixes: ``-none``, ``-low``, ``-medium``, ``-high``, ``-xhigh``.
    Example: ``"gpt-5.2-high"`` sends ``reasoning={"effort": "high"}`` to the API.
    Without a suffix, ``reasoning`` is omitted (OpenAI model default).
max_output_tokens : int
    Maximum tokens for the model's response output.
system_instruction : str, optional
    System instruction for the model.
json_output : bool
    Whether to request JSON response format.
timeout_s : float, optional
    Request timeout in seconds.
**kwargs
    Additional provider-specific options.

Note: GPT-5+ reasoning models don't support custom temperature (fixed at 1.0).
"""

from __future__ import annotations

import os
import re
from typing import Any

from solstone.think.models import GPT_5, OPENAI_EFFORT_SUFFIXES
from solstone.think.providers._image import encode_image_part, is_image_part

from .shared import GenerateResult

# Agent configuration is now loaded via get_talent() in cortex.py

_SCHEMA_NAME_RE = re.compile(r"^[a-zA-Z0-9_-]{1,64}$")


def _parse_model_effort(model: str) -> tuple[str, str | None]:
    """Extract reasoning effort suffix from a model name.

    Returns (api_model, effort) where api_model has the suffix stripped
    and effort is the reasoning effort value (or None if no suffix).
    """
    for suffix in OPENAI_EFFORT_SUFFIXES:
        if model.endswith(suffix):
            return model[: -len(suffix)], suffix[1:]
    return model, None


# ---------------------------------------------------------------------------
# run_generate / run_agenerate functions
# ---------------------------------------------------------------------------

# Cache for OpenAI clients
_openai_client = None
_async_openai_client = None


def _get_openai_client():
    """Get or create sync OpenAI client."""
    global _openai_client
    if _openai_client is None:
        import openai

        api_key = os.getenv("OPENAI_API_KEY")
        if not api_key:
            raise ValueError("OPENAI_API_KEY not found in environment")
        _openai_client = openai.OpenAI(api_key=api_key)
    return _openai_client


def _get_async_openai_client():
    """Get or create async OpenAI client."""
    global _async_openai_client
    if _async_openai_client is None:
        import openai

        api_key = os.getenv("OPENAI_API_KEY")
        if not api_key:
            raise ValueError("OPENAI_API_KEY not found in environment")
        _async_openai_client = openai.AsyncOpenAI(api_key=api_key)
    return _async_openai_client


def _build_input(
    contents: Any,
    system_instruction: str | None = None,
) -> tuple[Any, str | None]:
    """Build OpenAI Responses input and system instructions."""
    if isinstance(contents, str):
        return contents, system_instruction
    if isinstance(contents, list):
        if contents and isinstance(contents[0], dict) and "role" in contents[0]:
            return contents, system_instruction
        if any(is_image_part(c) for c in contents):
            content = []
            for c in contents:
                if is_image_part(c):
                    media_type, b64 = encode_image_part(c)
                    content.append(
                        {
                            "type": "input_image",
                            "image_url": f"data:{media_type};base64,{b64}",
                            "detail": "auto",
                        }
                    )
                else:
                    content.append({"type": "input_text", "text": str(c)})
            return [{"role": "user", "content": content}], system_instruction
        return "\n".join(str(c) for c in contents), system_instruction
    return str(contents), system_instruction


def _derive_schema_name(schema: dict | None) -> str:
    """Return a valid schema name for OpenAI structured outputs."""
    if isinstance(schema, dict):
        title = schema.get("title")
        if isinstance(title, str) and title and _SCHEMA_NAME_RE.fullmatch(title):
            return title
    return "response"


def _normalize_finish_reason(response: Any) -> str | None:
    """Normalize OpenAI finish_reason to standard values.

    Returns normalized string: "stop", "max_tokens", "content_filter", or None.
    """
    if not response or not getattr(response, "status", None):
        return None

    status = response.status
    if status == "completed":
        return "stop"
    if status == "incomplete":
        incomplete_details = getattr(response, "incomplete_details", None)
        if (
            incomplete_details is not None
            and getattr(incomplete_details, "reason", None) == "content_filter"
        ):
            return "content_filter"
        return "max_tokens"
    if status == "failed":
        return "error"
    return status


def _extract_thinking(response: Any) -> list | None:
    """Extract reasoning summaries from Responses API output.

    Returns list of thinking block dicts or None if no reasoning.
    """
    if not hasattr(response, "output") or not response.output:
        return None

    thinking_blocks = []
    for item in response.output:
        if getattr(item, "type", None) != "reasoning":
            continue
        for summary in getattr(item, "summary", None) or []:
            text = getattr(summary, "text", None)
            if text:
                thinking_blocks.append({"summary": text})

    return thinking_blocks if thinking_blocks else None


def _resolved_model(response: Any, requested: str) -> str:
    """Return resolved response.model when it is a non-empty string, else requested."""
    resolved = getattr(response, "model", None)
    if isinstance(resolved, str) and resolved:
        return resolved
    return requested


def _extract_usage(response: Any) -> dict | None:
    """Extract normalized usage dict from OpenAI response."""
    if not response.usage:
        return None

    usage: dict[str, Any] = {
        "input_tokens": response.usage.input_tokens,
        "output_tokens": response.usage.output_tokens,
        "total_tokens": response.usage.total_tokens,
    }
    # Extract optional detail fields
    input_details = getattr(response.usage, "input_tokens_details", None)
    if input_details:
        cached = getattr(input_details, "cached_tokens", 0)
        if cached:
            usage["cached_tokens"] = cached
    output_details = getattr(response.usage, "output_tokens_details", None)
    if output_details:
        reasoning = getattr(output_details, "reasoning_tokens", 0)
        if reasoning:
            usage["reasoning_tokens"] = reasoning
    model_version = getattr(response, "model", None)
    if isinstance(model_version, str) and model_version:
        usage["model_version"] = model_version
    return usage


def run_generate(
    contents: Any,
    model: str = GPT_5,
    max_output_tokens: int = 8192 * 2,
    system_instruction: str | None = None,
    json_output: bool = False,
    json_schema: dict | None = None,
    timeout_s: float | None = None,
    **kwargs: Any,
) -> GenerateResult:
    """Generate text synchronously.

    Returns GenerateResult with text, usage, finish_reason, and thinking.
    See module docstring for parameter details.
    """
    client = _get_openai_client()
    input_content, instructions = _build_input(contents, system_instruction)

    # Parse effort suffix from model name (e.g., "gpt-5.2-high" → "gpt-5.2", "high")
    api_model, effort = _parse_model_effort(model)

    # Build request kwargs
    request_kwargs: dict[str, Any] = {
        "model": api_model,
        "input": input_content,
        "max_output_tokens": max_output_tokens,
    }
    if instructions is not None:
        request_kwargs["instructions"] = instructions
    if effort is not None:
        request_kwargs["reasoning"] = {"effort": effort}

    if json_schema is not None:
        request_kwargs["text"] = {
            "format": {
                "type": "json_schema",
                "name": _derive_schema_name(json_schema),
                "schema": json_schema,
                "strict": True,
            }
        }
    elif json_output:
        request_kwargs["text"] = {"format": {"type": "json_object"}}

    if timeout_s:
        request_kwargs["timeout"] = timeout_s

    response = client.responses.create(**request_kwargs)
    return GenerateResult(
        text=response.output_text or "",
        model=_resolved_model(response, model),
        usage=_extract_usage(response),
        finish_reason=_normalize_finish_reason(response),
        thinking=_extract_thinking(response),
    )


async def run_agenerate(
    contents: Any,
    model: str = GPT_5,
    max_output_tokens: int = 8192 * 2,
    system_instruction: str | None = None,
    json_output: bool = False,
    json_schema: dict | None = None,
    timeout_s: float | None = None,
    **kwargs: Any,
) -> GenerateResult:
    """Generate text asynchronously.

    Returns GenerateResult with text, usage, finish_reason, and thinking.
    See module docstring for parameter details.
    """
    client = _get_async_openai_client()
    input_content, instructions = _build_input(contents, system_instruction)

    # Parse effort suffix from model name (e.g., "gpt-5.2-high" → "gpt-5.2", "high")
    api_model, effort = _parse_model_effort(model)

    # Build request kwargs
    request_kwargs: dict[str, Any] = {
        "model": api_model,
        "input": input_content,
        "max_output_tokens": max_output_tokens,
    }
    if instructions is not None:
        request_kwargs["instructions"] = instructions
    if effort is not None:
        request_kwargs["reasoning"] = {"effort": effort}

    if json_schema is not None:
        request_kwargs["text"] = {
            "format": {
                "type": "json_schema",
                "name": _derive_schema_name(json_schema),
                "schema": json_schema,
                "strict": True,
            }
        }
    elif json_output:
        request_kwargs["text"] = {"format": {"type": "json_object"}}

    if timeout_s:
        request_kwargs["timeout"] = timeout_s

    response = await client.responses.create(**request_kwargs)
    return GenerateResult(
        text=response.output_text or "",
        model=_resolved_model(response, model),
        usage=_extract_usage(response),
        finish_reason=_normalize_finish_reason(response),
        thinking=_extract_thinking(response),
    )


def list_models() -> list[dict]:
    """List available OpenAI models.

    Returns
    -------
    list[dict]
        List of raw model info objects from the OpenAI API.
    """
    client = _get_openai_client()
    return [m.model_dump() for m in client.models.list()]


def validate_key(api_key: str) -> dict:
    """Validate an OpenAI API key by listing models.

    Creates a temporary client with the provided key. Never uses
    the cached client or environment variables.

    Returns {"valid": True} or {"valid": False, "error": "..."}.
    """
    try:
        import openai

        client = openai.OpenAI(api_key=api_key, timeout=10)
        list(client.models.list())
        return {"valid": True}
    except Exception as e:
        return {"valid": False, "error": str(e)}


__all__ = [
    "run_generate",
    "run_agenerate",
    "list_models",
    "validate_key",
]
