#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Anthropic Claude provider for agents and direct LLM generation.

This module provides the Anthropic Claude provider for the ``journal providers check`` CLI
and run_generate/run_agenerate functions returning GenerateResult.

Common Parameters
-----------------
contents : str or list
    The content to send to the model.
model : str
    Model name to use.
temperature : float
    Temperature for generation (default: 0.3).
max_output_tokens : int
    Maximum tokens for the model's response output.
system_instruction : str, optional
    System instruction for the model.
json_output : bool
    Whether to request JSON response format.
thinking_budget : int, optional
    Token budget for model thinking.
timeout_s : float, optional
    Request timeout in seconds.
**kwargs
    Additional provider-specific options.
"""

from __future__ import annotations

import logging
import os
from typing import Any

from anthropic import AsyncAnthropic
from anthropic._constants import MODEL_NONSTREAMING_TOKENS
from anthropic.types import (
    Message,
    MessageParam,
    RedactedThinkingBlock,
    ThinkingBlock,
)

from solstone.think.models import CLAUDE_SONNET_4, model_supports
from solstone.think.providers._image import encode_image_part, is_image_part

from .shared import GenerateResult

# Default values are now handled internally
_DEFAULT_MODEL = CLAUDE_SONNET_4

logger = logging.getLogger(__name__)

_DEFAULT_MAX_TOKENS = 8096 * 2
_MIN_THINKING_BUDGET = 1024  # Anthropic minimum
_DEFAULT_THINKING_BUDGET = 10000
_MAX_TOKENS_BUFFER = (
    1000  # Anthropic rejects requests where max_tokens <= thinking.budget_tokens.
)
_NONSTREAMING_TIME_CAP_TOKENS = (
    21_333  # SDK time formula: 60*60*max_tokens/128_000 > 600 ≈ 21,333.
)
# anthropic._constants.MODEL_NONSTREAMING_TOKENS adds per-model non-streaming caps on top of this threshold.


# ---------------------------------------------------------------------------
# run_generate / run_agenerate functions
# ---------------------------------------------------------------------------


def _resolved_model(response: Any, requested: str) -> str:
    """Return resolved response.model when it is a non-empty string, else requested."""
    resolved = getattr(response, "model", None)
    if isinstance(resolved, str) and resolved:
        return resolved
    return requested


def _extract_usage_dict(response: Any) -> dict[str, Any] | None:
    """Extract usage dict from Anthropic response.

    Returns normalized usage dict or None if usage unavailable.
    """
    if not hasattr(response, "usage") or not response.usage:
        return None

    usage = response.usage
    input_tokens = getattr(usage, "input_tokens", 0)
    output_tokens = getattr(usage, "output_tokens", 0)
    usage_dict: dict[str, Any] = {
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": input_tokens + output_tokens,
    }
    # Add cache tokens if available and non-zero
    cache_creation = getattr(usage, "cache_creation_input_tokens", None)
    if cache_creation:
        usage_dict["cache_creation_tokens"] = cache_creation
    cache_read = getattr(usage, "cache_read_input_tokens", None)
    if cache_read:
        usage_dict["cached_tokens"] = cache_read
    model_version = getattr(response, "model", None)
    if isinstance(model_version, str) and model_version:
        usage_dict["model_version"] = model_version
    return usage_dict


def _normalize_finish_reason(stop_reason: str | None) -> str | None:
    """Normalize Anthropic stop_reason to standard values.

    Returns normalized string: "stop", "max_tokens", or None.
    """
    if not stop_reason:
        return None

    reason = stop_reason.lower()
    if reason == "end_turn":
        return "stop"
    elif reason == "max_tokens":
        return "max_tokens"
    elif reason == "stop_sequence":
        return "stop"
    else:
        return reason


def _extract_text_and_thinking(response: Any) -> tuple[str, list | None]:
    """Extract text and thinking blocks from Anthropic response.

    Returns tuple of (text, thinking_blocks).
    """
    text = ""
    thinking_blocks = []

    for block in response.content:
        if getattr(block, "type", None) == "text":
            text += block.text
        elif isinstance(block, ThinkingBlock):
            thinking_blocks.append(
                {
                    "summary": block.thinking,
                    "signature": block.signature,
                }
            )
        elif isinstance(block, RedactedThinkingBlock):
            thinking_blocks.append(
                {
                    "summary": "[redacted]",
                    "redacted_data": block.data,
                }
            )

    return text, thinking_blocks if thinking_blocks else None


def _adjust_budget_for_thinking(request_kwargs: dict[str, Any]) -> None:
    """Lift max_tokens when thinking budget would otherwise collide with Anthropic validation."""
    thinking = request_kwargs.get("thinking")
    if not thinking:
        return

    budget_tokens = thinking.get("budget_tokens")
    if not budget_tokens or budget_tokens <= 0:
        return

    max_tokens = request_kwargs["max_tokens"]
    minimum_max_tokens = budget_tokens + _MAX_TOKENS_BUFFER + 1
    if max_tokens <= budget_tokens + _MAX_TOKENS_BUFFER:
        logger.info(
            "Adjusted Anthropic max_tokens for thinking budget: %s -> %s",
            max_tokens,
            minimum_max_tokens,
        )
        # Anthropic requires max_tokens > thinking.budget_tokens; lift rather than clamp thinking so the caller's stated output floor is preserved.
        request_kwargs["max_tokens"] = minimum_max_tokens


def _requires_streaming(model: str, max_tokens: int) -> bool:
    """Return whether the Anthropic SDK would require streaming for this request."""
    if max_tokens > _NONSTREAMING_TIME_CAP_TOKENS:
        return True
    cap = MODEL_NONSTREAMING_TOKENS.get(model)
    if cap is not None and max_tokens > cap:
        return True
    return False


def _send_message(client: Any, request_kwargs: dict[str, Any]) -> Message:
    """Dispatch sync message requests via create or stream based on Anthropic limits."""
    if _requires_streaming(request_kwargs["model"], request_kwargs["max_tokens"]):
        with client.messages.stream(**request_kwargs) as stream:
            return stream.get_final_message()
    return client.messages.create(**request_kwargs)


async def _asend_message(client: Any, request_kwargs: dict[str, Any]) -> Message:
    """Dispatch async message requests via create or stream based on Anthropic limits."""
    if _requires_streaming(request_kwargs["model"], request_kwargs["max_tokens"]):
        async with client.messages.stream(**request_kwargs) as stream:
            return await stream.get_final_message()
    return await client.messages.create(**request_kwargs)


# Cache for Anthropic clients
_anthropic_client = None
_async_anthropic_client = None


def _get_anthropic_client():
    """Get or create sync Anthropic client."""
    global _anthropic_client
    if _anthropic_client is None:
        from anthropic import Anthropic

        api_key = os.getenv("ANTHROPIC_API_KEY")
        if not api_key:
            raise ValueError("ANTHROPIC_API_KEY not found in environment")
        _anthropic_client = Anthropic(api_key=api_key)
    return _anthropic_client


def _get_async_anthropic_client():
    """Get or create async Anthropic client."""
    global _async_anthropic_client
    if _async_anthropic_client is None:
        api_key = os.getenv("ANTHROPIC_API_KEY")
        if not api_key:
            raise ValueError("ANTHROPIC_API_KEY not found in environment")
        _async_anthropic_client = AsyncAnthropic(api_key=api_key)
    return _async_anthropic_client


def _convert_contents_to_messages(contents: Any) -> list[MessageParam]:
    """Convert contents to Anthropic messages format."""
    # Handle different content formats
    if isinstance(contents, str):
        return [{"role": "user", "content": contents}]
    elif isinstance(contents, list):
        # Check if it's already in messages format
        if contents and isinstance(contents[0], dict) and "role" in contents[0]:
            return contents
        elif any(is_image_part(c) for c in contents):
            content = []
            for c in contents:
                if is_image_part(c):
                    media_type, b64 = encode_image_part(c)
                    content.append(
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": media_type,
                                "data": b64,
                            },
                        }
                    )
                else:
                    content.append({"type": "text", "text": str(c)})
            return [{"role": "user", "content": content}]
        else:
            # List of content parts - combine into single user message
            combined = "\n".join(str(c) for c in contents)
            return [{"role": "user", "content": combined}]
    else:
        return [{"role": "user", "content": str(contents)}]


def run_generate(
    contents: Any,
    model: str = _DEFAULT_MODEL,
    temperature: float = 0.3,
    max_output_tokens: int = 8192 * 2,
    system_instruction: str | None = None,
    json_output: bool = False,
    thinking_budget: int | None = None,
    json_schema: dict | None = None,
    timeout_s: float | None = None,
    **kwargs: Any,
) -> GenerateResult:
    """Generate text synchronously.

    Returns GenerateResult with text, usage, finish_reason, and thinking.
    See module docstring for parameter details.
    """
    client = _get_anthropic_client()
    messages = _convert_contents_to_messages(contents)

    # Handle JSON output by adding to system instruction
    system = system_instruction or ""
    if json_schema is None and json_output:
        json_instruction = "Respond with valid JSON only. No explanation or markdown."
        system = f"{system}\n\n{json_instruction}" if system else json_instruction

    # Build request kwargs
    request_kwargs: dict[str, Any] = {
        "model": model,
        "max_tokens": max_output_tokens,
        "messages": messages,
    }

    if system:
        request_kwargs["system"] = system

    # Note: Anthropic doesn't support temperature with thinking enabled
    # Configure thinking if budget is provided
    if thinking_budget and thinking_budget > 0:
        request_kwargs["thinking"] = {
            "type": "enabled",
            "budget_tokens": thinking_budget,
        }
    elif model_supports(model, "temperature"):
        request_kwargs["temperature"] = temperature
    else:
        # Some Anthropic reasoning models reject the temperature parameter.
        pass
    _adjust_budget_for_thinking(request_kwargs)

    if timeout_s:
        request_kwargs["timeout"] = timeout_s

    if json_schema is not None:
        request_kwargs["output_config"] = {
            "format": {
                "type": "json_schema",
                "schema": json_schema,
            }
        }
        response = _send_message(client, request_kwargs)
        text, thinking = _extract_text_and_thinking(response)
    else:
        response = _send_message(client, request_kwargs)
        text, thinking = _extract_text_and_thinking(response)

    finish_reason = _normalize_finish_reason(response.stop_reason)
    return GenerateResult(
        text=text,
        model=_resolved_model(response, model),
        usage=_extract_usage_dict(response),
        finish_reason=finish_reason,
        thinking=thinking,
    )


async def run_agenerate(
    contents: Any,
    model: str = _DEFAULT_MODEL,
    temperature: float = 0.3,
    max_output_tokens: int = 8192 * 2,
    system_instruction: str | None = None,
    json_output: bool = False,
    thinking_budget: int | None = None,
    json_schema: dict | None = None,
    timeout_s: float | None = None,
    **kwargs: Any,
) -> GenerateResult:
    """Generate text asynchronously.

    Returns GenerateResult with text, usage, finish_reason, and thinking.
    See module docstring for parameter details.
    """
    client = _get_async_anthropic_client()
    messages = _convert_contents_to_messages(contents)

    # Handle JSON output by adding to system instruction
    system = system_instruction or ""
    if json_schema is None and json_output:
        json_instruction = "Respond with valid JSON only. No explanation or markdown."
        system = f"{system}\n\n{json_instruction}" if system else json_instruction

    # Build request kwargs
    request_kwargs: dict[str, Any] = {
        "model": model,
        "max_tokens": max_output_tokens,
        "messages": messages,
    }

    if system:
        request_kwargs["system"] = system

    # Note: Anthropic doesn't support temperature with thinking enabled
    # Configure thinking if budget is provided
    if thinking_budget and thinking_budget > 0:
        request_kwargs["thinking"] = {
            "type": "enabled",
            "budget_tokens": thinking_budget,
        }
    elif model_supports(model, "temperature"):
        request_kwargs["temperature"] = temperature
    else:
        # Some Anthropic reasoning models reject the temperature parameter.
        pass
    _adjust_budget_for_thinking(request_kwargs)

    if timeout_s:
        request_kwargs["timeout"] = timeout_s

    if json_schema is not None:
        request_kwargs["output_config"] = {
            "format": {
                "type": "json_schema",
                "schema": json_schema,
            }
        }
        response = await _asend_message(client, request_kwargs)
        text, thinking = _extract_text_and_thinking(response)
    else:
        response = await _asend_message(client, request_kwargs)
        text, thinking = _extract_text_and_thinking(response)

    finish_reason = _normalize_finish_reason(response.stop_reason)
    return GenerateResult(
        text=text,
        model=_resolved_model(response, model),
        usage=_extract_usage_dict(response),
        finish_reason=finish_reason,
        thinking=thinking,
    )


def list_models() -> list[dict]:
    """List available Anthropic models.

    Returns
    -------
    list[dict]
        List of raw model info objects from the Anthropic API.
    """
    client = _get_anthropic_client()
    return [m.model_dump() for m in client.models.list()]


def validate_key(api_key: str) -> dict:
    """Validate an Anthropic API key by listing models.

    Creates a temporary client with the provided key. Never uses
    the cached client or environment variables.

    Returns {"valid": True} or {"valid": False, "error": "..."}.
    """
    try:
        from anthropic import Anthropic

        client = Anthropic(api_key=api_key, timeout=10)
        list(client.models.list(limit=1))
        return {"valid": True}
    except Exception as e:
        return {"valid": False, "error": str(e)}


__all__ = [
    "run_generate",
    "run_agenerate",
    "list_models",
    "validate_key",
]
