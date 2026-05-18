#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Anthropic Claude provider for agents and direct LLM generation.

This module provides the Anthropic Claude provider for the ``sol providers check`` CLI
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
import traceback
from pathlib import Path
from typing import Any, Callable

from anthropic import AsyncAnthropic
from anthropic._constants import MODEL_NONSTREAMING_TOKENS
from anthropic.types import (
    Message,
    MessageParam,
    RedactedThinkingBlock,
    ThinkingBlock,
)

from solstone.think.models import CLAUDE_SONNET_4
from solstone.think.providers._image import encode_image_part, is_image_part
from solstone.think.utils import now_ms

from .cli import (
    CLIRunner,
    QuotaExhaustedError,
    ThinkingAggregator,
    assemble_prompt,
    build_cogitate_env,
    check_cli_binary,
)
from .shared import (
    GenerateResult,
    JSONEventCallback,
    classify_provider_error,
    safe_raw,
)

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


def _compute_thinking_params(max_tokens: int) -> tuple[int, int]:
    """Compute thinking budget and adjusted max_tokens.

    Returns (thinking_budget, adjusted_max_tokens) ensuring:
    - thinking_budget >= _MIN_THINKING_BUDGET
    - thinking_budget < adjusted_max_tokens
    """
    # Budget is the lesser of default or what fits in max_tokens
    thinking_budget = min(_DEFAULT_THINKING_BUDGET, max(max_tokens - 1000, 0))

    # Ensure minimum thinking budget
    if thinking_budget < _MIN_THINKING_BUDGET:
        thinking_budget = _MIN_THINKING_BUDGET
        # Increase max_tokens to accommodate thinking + output
        max_tokens = max(max_tokens, thinking_budget + 1000)

    return thinking_budget, max_tokens


def _resolve_agent_thinking_params(
    max_output_tokens: int, thinking_budget_config: int | None
) -> tuple[int, int]:
    """Resolve thinking budget and max tokens for agent run.

    Args:
        max_output_tokens: Maximum output tokens from config.
        thinking_budget_config: Explicit thinking budget from config, or None.

    Returns:
        Tuple of (thinking_budget, effective_max_tokens).
        If thinking_budget_config is provided and > 0, uses it directly.
        Otherwise computes defaults via _compute_thinking_params.
    """
    if thinking_budget_config is not None and thinking_budget_config > 0:
        return thinking_budget_config, max_output_tokens
    return _compute_thinking_params(max_output_tokens)


def _translate_claude(
    event: dict[str, Any],
    aggregator: ThinkingAggregator,
    callback: JSONEventCallback,
    pending_tools: dict[str, dict[str, Any]],
    result_meta: dict[str, Any],
) -> str | None:
    """Translate a Claude CLI JSONL event into our Event format.

    Args:
        event: Raw parsed JSON event from Claude CLI stdout.
        aggregator: ThinkingAggregator for text buffering.
        callback: JSONEventCallback for emitting events.
        pending_tools: Mutable dict tracking active tool calls (tool_use_id -> {tool, args}).
        result_meta: Mutable dict for storing cost/usage from result event.

    Returns:
        Session ID string from init events, None otherwise.
    """
    event_type = event.get("type")

    if event_type == "system":
        if event.get("subtype") == "init":
            return event.get("session_id")

    elif event_type == "assistant":
        message = event.get("message", {})
        content_blocks = message.get("content", [])

        # Two-pass: text/thinking first, then tool_use
        tool_use_blocks = []
        for block in content_blocks:
            block_type = block.get("type")
            if block_type == "text":
                aggregator.accumulate(block.get("text", ""))
            elif block_type == "thinking":
                thinking_event: dict[str, Any] = {
                    "event": "thinking",
                    "summary": block.get("thinking", ""),
                    "raw": safe_raw([event]),
                }
                if aggregator._model:
                    thinking_event["model"] = aggregator._model
                callback.emit(thinking_event)
            elif block_type == "tool_use":
                tool_use_blocks.append(block)

        for block in tool_use_blocks:
            aggregator.flush_as_thinking(raw_events=[event])

            tool_id = block.get("id", "")
            tool_name = block.get("name", "")
            tool_args = block.get("input", {})

            pending_tools[tool_id] = {"tool": tool_name, "args": tool_args}

            callback.emit(
                {
                    "event": "tool_start",
                    "tool": tool_name,
                    "args": tool_args,
                    "call_id": tool_id,
                    "raw": safe_raw([event]),
                }
            )

    elif event_type == "user":
        message = event.get("message", {})
        content_blocks = message.get("content", [])

        for block in content_blocks:
            if block.get("type") == "tool_result":
                tool_use_id = block.get("tool_use_id", "")
                tool_info = pending_tools.pop(tool_use_id, {})

                callback.emit(
                    {
                        "event": "tool_end",
                        "tool": tool_info.get("tool", ""),
                        "args": tool_info.get("args"),
                        "result": block.get("content", ""),
                        "call_id": tool_use_id,
                        "raw": safe_raw([event]),
                    }
                )

    elif event_type == "result":
        result_meta["cost_usd"] = event.get("total_cost_usd")
        usage = event.get("usage")
        if usage:
            input_tokens = usage.get("input_tokens") or 0
            output_tokens = usage.get("output_tokens") or 0
            usage_dict: dict[str, Any] = {
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
                "total_tokens": input_tokens + output_tokens,
            }
            cache_creation = usage.get("cache_creation_input_tokens")
            if cache_creation:
                usage_dict["cache_creation_tokens"] = cache_creation
            cache_read = usage.get("cache_read_input_tokens")
            if cache_read:
                usage_dict["cached_tokens"] = cache_read
            result_meta["usage"] = usage_dict

    return None


async def run_cogitate(
    config: dict[str, Any],
    on_event: Callable[[dict], None] | None = None,
) -> str:
    """Run a prompt with tool-calling support via Claude CLI subprocess.

    Spawns the Claude CLI in streaming JSON mode and translates its
    JSONL output into our standard Event format.

    Args:
        config: Complete configuration dictionary including prompt, system_instruction,
            user_instruction, extra_context, model, etc.
        on_event: Optional event callback
    """
    model = config.get("model", _DEFAULT_MODEL)
    session_id = config.get("session_id")

    callback = JSONEventCallback(on_event)

    try:
        check_cli_binary("claude")

        prompt_body, system_instruction = assemble_prompt(
            config,
            sol_tool_name="Bash" if not config.get("write") else None,
        )

        cmd = [
            "claude",
            "-p",
            "-",
            "--verbose",
            "--output-format",
            "stream-json",
            "--permission-mode",
            "plan",
            "--model",
            model,
        ]

        # Restrict tool access unless write mode is enabled
        if not config.get("write"):
            cmd.extend(["--allowedTools", "Bash(sol *)"])

        if system_instruction:
            cmd.extend(["--system-prompt", system_instruction])

        if session_id:
            cmd.extend(["--resume", session_id])

        aggregator = ThinkingAggregator(callback, model=model)
        pending_tools: dict[str, dict[str, Any]] = {}
        result_meta: dict[str, Any] = {}

        def translate(
            event: dict[str, Any],
            agg: ThinkingAggregator,
            cb: JSONEventCallback,
        ) -> str | None:
            return _translate_claude(event, agg, cb, pending_tools, result_meta)

        cwd_value = config.get("cwd")
        runner = CLIRunner(
            cmd=cmd,
            prompt_text=prompt_body,
            translate=translate,
            callback=callback,
            aggregator=aggregator,
            cwd=Path(cwd_value) if cwd_value else None,
            env=build_cogitate_env("anthropic"),
        )
        runner.provider = "anthropic"

        result = await runner.run()

        # Build finish event with usage from result meta
        usage_dict = result_meta.get("usage")

        callback.emit(
            {
                "event": "finish",
                "result": result,
                "cli_session_id": runner.cli_session_id,
                "usage": usage_dict,
                "ts": now_ms(),
            }
        )

        return result
    except QuotaExhaustedError:
        raise
    except Exception as exc:
        callback.emit(
            {
                "event": "error",
                "error": str(exc),
                "reason_code": classify_provider_error(exc, "anthropic"),
                "provider": "anthropic",
                "trace": traceback.format_exc(),
                "ts": now_ms(),
            }
        )
        setattr(exc, "_evented", True)
        raise


# ---------------------------------------------------------------------------
# run_generate / run_agenerate functions
# ---------------------------------------------------------------------------


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
    else:
        request_kwargs["temperature"] = temperature
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
    else:
        request_kwargs["temperature"] = temperature
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
    "run_cogitate",
    "run_generate",
    "run_agenerate",
    "list_models",
    "validate_key",
]
