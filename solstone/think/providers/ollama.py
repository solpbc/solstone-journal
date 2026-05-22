#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Ollama (Local) provider for LLM generation and tool-calling agents.

This module provides the Ollama provider for run_generate/run_agenerate
(text generation) and run_cogitate (tool-calling agents).

**Generation** uses Ollama's native ``/api/chat`` endpoint via ``httpx``
for reliable control over the ``think`` parameter, which the OpenAI-compatible
endpoint silently ignores on models like Qwen3.5.

**Cogitate** uses the OpenCode CLI (``opencode run --format json``) as a
subprocess, following the same CLIRunner + translate pattern as the Google,
OpenAI, and Anthropic providers. OpenCode connects to local Ollama via its
OpenAI-compatible endpoint and handles tool execution internally.

Common Parameters
-----------------
contents : str or list
    The content to send to the model.
model : str
    Model name with ``ollama-local/`` prefix (e.g., ``ollama-local/qwen3.5:9b``).
    The prefix is stripped before sending to the Ollama API.
temperature : float
    Temperature for generation (default: 0.3).
max_output_tokens : int
    Maximum tokens for the model's response output.
system_instruction : str, optional
    System instruction for the model.
json_output : bool
    Whether to request JSON response format.
thinking_budget : int, optional
    Token budget for model thinking. When > 0, enables Ollama's ``think``
    parameter. When None or 0, thinking is explicitly disabled.
timeout_s : float, optional
    Request timeout in seconds.
**kwargs
    Additional provider-specific options (absorbed for forward compatibility).

Environment Variables
---------------------
OLLAMA_BASE_URL : str
    Base URL for the Ollama server (default: ``http://localhost:11434``).
"""

from __future__ import annotations

import logging
import os
import traceback
from pathlib import Path
from typing import Any, Callable

import httpx

from solstone.think.models import OLLAMA_FLASH
from solstone.think.utils import now_ms

from .cli import CLIRunner, QuotaExhaustedError, ThinkingAggregator, assemble_prompt
from .shared import GenerateResult, JSONEventCallback, classify_provider_error, safe_raw

LOG = logging.getLogger("solstone.think.providers.ollama")

_OLLAMA_LOCAL_PREFIX = "ollama-local/"
_DEFAULT_BASE_URL = "http://localhost:11434"
_DEFAULT_TIMEOUT = 120.0

# ---------------------------------------------------------------------------
# Client management
# ---------------------------------------------------------------------------

_sync_client: httpx.Client | None = None
_async_client: httpx.AsyncClient | None = None


def _get_base_url() -> str:
    """Get Ollama base URL from environment or default."""
    return os.getenv("OLLAMA_BASE_URL", _DEFAULT_BASE_URL).rstrip("/")


def _get_client() -> httpx.Client:
    """Get or create cached sync httpx client."""
    global _sync_client
    if _sync_client is None:
        _sync_client = httpx.Client(
            base_url=_get_base_url(),
            timeout=_DEFAULT_TIMEOUT,
        )
    return _sync_client


def _get_async_client() -> httpx.AsyncClient:
    """Get or create cached async httpx client."""
    global _async_client
    if _async_client is None:
        _async_client = httpx.AsyncClient(
            base_url=_get_base_url(),
            timeout=_DEFAULT_TIMEOUT,
        )
    return _async_client


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _strip_model_prefix(model: str) -> str:
    """Strip the ``ollama-local/`` prefix for the Ollama API.

    The Ollama API expects bare model names like ``qwen3.5:9b``, but
    Solstone uses the ``ollama-local/`` prefix for provider routing.
    """
    if model.startswith(_OLLAMA_LOCAL_PREFIX):
        return model[len(_OLLAMA_LOCAL_PREFIX) :]
    return model


def _build_messages(
    contents: Any,
    system_instruction: str | None = None,
) -> list[dict[str, str]]:
    """Convert contents and system instruction to chat messages.

    Parameters
    ----------
    contents
        String, list of strings, or list of message dicts with ``role`` keys.
    system_instruction
        Optional system prompt, prepended as a system message.

    Returns
    -------
    list[dict[str, str]]
        Messages in ``[{role, content}, ...]`` format.
    """
    messages: list[dict[str, str]] = []

    if system_instruction:
        messages.append({"role": "system", "content": system_instruction})

    if isinstance(contents, str):
        messages.append({"role": "user", "content": contents})
    elif isinstance(contents, list):
        if contents and isinstance(contents[0], dict) and "role" in contents[0]:
            messages.extend(contents)
        else:
            messages.append(
                {"role": "user", "content": "\n".join(str(c) for c in contents)}
            )
    else:
        messages.append({"role": "user", "content": str(contents)})

    return messages


def _build_request_body(
    model: str,
    messages: list[dict[str, str]],
    temperature: float,
    max_output_tokens: int,
    json_output: bool,
    thinking_budget: int | None,
    json_schema: dict | None = None,
) -> dict[str, Any]:
    """Build the native Ollama /api/chat request body.

    Parameters
    ----------
    model
        Bare model name (prefix already stripped).
    messages
        Chat messages list.
    temperature
        Sampling temperature.
    max_output_tokens
        Maximum response tokens (``num_predict`` in Ollama).
    json_output
        Whether to request JSON response format.
    thinking_budget
        Thinking token budget; > 0 enables, None/0 disables.

    Returns
    -------
    dict
        Request body for ``POST /api/chat``.
    """
    body: dict[str, Any] = {
        "model": model,
        "messages": messages,
        "stream": False,
        "options": {
            "temperature": temperature,
            "num_predict": max_output_tokens,
        },
    }

    # Thinking control: this is the reason we use the native API.
    # The OpenAI-compat endpoint ignores this parameter.
    if thinking_budget is not None and thinking_budget > 0:
        body["think"] = True
    else:
        body["think"] = False

    if json_schema is not None:
        body["format"] = json_schema
    elif json_output:
        body["format"] = "json"

    return body


def _normalize_finish_reason(data: dict[str, Any]) -> str | None:
    """Normalize Ollama's done_reason to standard values.

    Returns ``"stop"``, ``"max_tokens"``, or None.
    """
    if not data.get("done"):
        return None

    reason = data.get("done_reason", "")
    if reason == "stop":
        return "stop"
    elif reason == "length":
        return "max_tokens"
    elif reason:
        return reason
    return "stop"  # done=True with no reason implies normal completion


def _extract_usage(data: dict[str, Any]) -> dict[str, int]:
    """Extract normalized usage dict from native Ollama response.

    Ollama uses ``prompt_eval_count`` and ``eval_count`` instead of the
    OpenAI-style ``prompt_tokens`` / ``completion_tokens``.
    """
    input_tokens = data.get("prompt_eval_count", 0) or 0
    output_tokens = data.get("eval_count", 0) or 0
    return {
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": input_tokens + output_tokens,
    }


def _extract_thinking(data: dict[str, Any]) -> list | None:
    """Extract thinking content from native Ollama response.

    The native API returns a ``thinking`` field on the message when
    thinking is enabled.
    """
    message = data.get("message", {})
    thinking = message.get("thinking")
    if thinking and isinstance(thinking, str) and thinking.strip():
        return [{"summary": thinking.strip()}]
    return None


def _parse_response(data: dict[str, Any]) -> GenerateResult:
    """Parse the native Ollama /api/chat response into GenerateResult."""
    message = data.get("message", {})
    text = message.get("content", "")

    return GenerateResult(
        text=text,
        usage=_extract_usage(data),
        finish_reason=_normalize_finish_reason(data),
        thinking=_extract_thinking(data),
    )


# ---------------------------------------------------------------------------
# run_generate / run_agenerate
# ---------------------------------------------------------------------------


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
    """Generate text synchronously via local Ollama.

    Returns GenerateResult with text, usage, finish_reason, and thinking.
    See module docstring for parameter details.
    """
    client = _get_client()
    api_model = _strip_model_prefix(model)
    messages = _build_messages(contents, system_instruction)
    body = _build_request_body(
        api_model,
        messages,
        temperature,
        max_output_tokens,
        json_output,
        thinking_budget,
        json_schema,
    )

    response = client.post(
        "/api/chat",
        json=body,
        timeout=timeout_s or _DEFAULT_TIMEOUT,
    )
    response.raise_for_status()
    return _parse_response(response.json())


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
    """Generate text asynchronously via local Ollama.

    Returns GenerateResult with text, usage, finish_reason, and thinking.
    See module docstring for parameter details.
    """
    client = _get_async_client()
    api_model = _strip_model_prefix(model)
    messages = _build_messages(contents, system_instruction)
    body = _build_request_body(
        api_model,
        messages,
        temperature,
        max_output_tokens,
        json_output,
        thinking_budget,
        json_schema,
    )

    response = await client.post(
        "/api/chat",
        json=body,
        timeout=timeout_s or _DEFAULT_TIMEOUT,
    )
    response.raise_for_status()
    return _parse_response(response.json())


# ---------------------------------------------------------------------------
# run_cogitate via OpenCode CLI
# ---------------------------------------------------------------------------


def _translate_opencode(
    event: dict[str, Any],
    aggregator: ThinkingAggregator,
    callback: JSONEventCallback,
    usage_out: dict[str, Any],
) -> str | None:
    """Translate an OpenCode JSONL event into our standard Event types.

    Args:
        event: Raw JSONL event dict from ``opencode run --format json``.
        aggregator: ThinkingAggregator for buffering text.
        callback: JSONEventCallback for emitting events.
        usage_out: Mutable dict to receive usage stats from step_finish events.

    Returns:
        The CLI session ID from step_start events, or None.
    """
    event_type = event.get("type")
    part = event.get("part", {})

    # -- step_start: capture session ID ------------------------------------
    if event_type == "step_start":
        return event.get("sessionID")

    # -- text: accumulate assistant text -----------------------------------
    if event_type == "text":
        text = part.get("text", "")
        if text:
            aggregator.accumulate(text)
        return None

    # -- tool_use: emit tool_start + tool_end ------------------------------
    # OpenCode reports tools as already completed, so we emit both events
    # back-to-back from a single JSONL line.
    if event_type == "tool_use":
        aggregator.flush_as_thinking(raw_events=[event])

        tool_name = part.get("tool", "")
        call_id = part.get("callID", "")
        state = part.get("state", {})
        tool_input = state.get("input", {})
        tool_output = state.get("output", "")

        callback.emit(
            {
                "event": "tool_start",
                "tool": tool_name,
                "args": tool_input,
                "call_id": call_id,
                "raw": safe_raw([event]),
                "ts": now_ms(),
            }
        )
        callback.emit(
            {
                "event": "tool_end",
                "tool": tool_name,
                "args": tool_input,
                "result": tool_output,
                "call_id": call_id,
                "raw": safe_raw([event]),
                "ts": now_ms(),
            }
        )
        return None

    # -- step_finish: capture usage ----------------------------------------
    if event_type == "step_finish":
        tokens = part.get("tokens")
        if tokens and usage_out is not None:
            input_tokens = tokens.get("input", 0)
            output_tokens = tokens.get("output", 0)
            total_tokens = tokens.get("total", 0)
            # Accumulate across steps (OpenCode emits one per turn)
            usage_out["input_tokens"] = usage_out.get("input_tokens", 0) + input_tokens
            usage_out["output_tokens"] = (
                usage_out.get("output_tokens", 0) + output_tokens
            )
            usage_out["total_tokens"] = usage_out.get("total_tokens", 0) + total_tokens
            reasoning = tokens.get("reasoning", 0)
            if reasoning:
                usage_out["reasoning_tokens"] = (
                    usage_out.get("reasoning_tokens", 0) + reasoning
                )
            cache = tokens.get("cache", {})
            cached_read = cache.get("read", 0)
            if cached_read:
                usage_out["cached_tokens"] = (
                    usage_out.get("cached_tokens", 0) + cached_read
                )
        return None

    # Unknown event type — log and skip
    LOG.debug("Unknown OpenCode CLI event type: %s", event_type)
    return None


def _build_opencode_env() -> dict[str, str]:
    """Build environment dict for the OpenCode subprocess.

    Sets ``OPENAI_API_KEY`` to a placeholder if not already set, since
    OpenCode's OpenAI-compatible provider requires it even for local Ollama.
    """
    env = os.environ.copy()
    if not env.get("OPENAI_API_KEY"):
        env["OPENAI_API_KEY"] = "ollama"
    return env


async def run_cogitate(
    config: dict[str, Any],
    on_event: Callable[[dict], None] | None = None,
) -> str:
    """Run a prompt with tool-calling support via OpenCode CLI + local Ollama.

    Uses the OpenCode CLI as a subprocess agent, which connects to the local
    Ollama instance and provides built-in tools (bash, read, glob, grep, etc.).

    Args:
        config: Complete configuration dictionary including prompt, system_instruction,
            user_instruction, extra_context, model, etc.
        on_event: Optional event callback
    """
    model = _strip_model_prefix(config.get("model", OLLAMA_FLASH))
    session_id = config.get("session_id")
    callback = JSONEventCallback(on_event)

    try:
        # Check that OpenCode CLI is available
        import shutil

        if not shutil.which("opencode"):
            raise RuntimeError(
                "Cogitate requires OpenCode CLI (opencode). "
                "Install from https://opencode.ai and configure it with a local "
                "Ollama provider. Generate works without it."
            )

        # Assemble prompt from config fields
        prompt_body, system_instruction = assemble_prompt(
            config,
            sol_tool_name="bash" if not config.get("write") else None,
        )

        # OpenCode has no --system-prompt flag; prepend to prompt body
        if system_instruction:
            prompt_body = system_instruction + "\n\n" + prompt_body

        # Build CLI command.
        # --title skips OpenCode's title-generation LLM call (avoids delays).
        agent_name = config.get("name", "sol-agent")
        cmd = [
            "opencode",
            "run",
            "--format",
            "json",
            "--title",
            agent_name,
            "-m",
            f"ollama/{model}",
        ]

        # Resume from previous session if continuing
        if session_id:
            cmd.extend(["--session", session_id])

        # Mutable container for usage accumulation
        usage: dict[str, Any] = {}

        def translate(
            event: dict[str, Any], agg: ThinkingAggregator, cb: JSONEventCallback
        ) -> str | None:
            return _translate_opencode(event, agg, cb, usage)

        aggregator = ThinkingAggregator(callback, model=model)
        cwd_value = config.get("cwd")
        runner = CLIRunner(
            cmd=cmd,
            prompt_text=prompt_body,
            translate=translate,
            callback=callback,
            aggregator=aggregator,
            cwd=Path(cwd_value) if cwd_value else None,
            env=_build_opencode_env(),
            # Local models are slower than cloud APIs; allow more time for
            # the first event (model loading + initial inference).
            first_event_timeout=120,
        )
        runner.provider = "ollama"

        result = await runner.run()

        # Emit finish event (CLIRunner does not emit one)
        finish_event: dict[str, Any] = {
            "event": "finish",
            "result": result,
            "ts": now_ms(),
        }
        if usage:
            finish_event["usage"] = usage
        if runner.cli_session_id:
            finish_event["cli_session_id"] = runner.cli_session_id
        callback.emit(finish_event)
        return result
    except QuotaExhaustedError:
        raise
    except Exception as exc:
        callback.emit(
            {
                "event": "error",
                "error": str(exc),
                "reason_code": classify_provider_error(exc, "ollama"),
                "provider": "ollama",
                "trace": traceback.format_exc(),
            }
        )
        setattr(exc, "_evented", True)
        raise


# ---------------------------------------------------------------------------
# list_models / validate_key
# ---------------------------------------------------------------------------


def list_models(provider: str) -> list[dict]:
    """List available models from the local Ollama instance.

    Returns
    -------
    list[dict]
        List of model info dicts from the Ollama ``/api/tags`` endpoint.
    """
    del provider
    client = _get_client()
    response = client.get("/api/tags")
    response.raise_for_status()
    return response.json().get("models", [])


def validate_key(provider: str, api_key: str) -> dict:
    """Check that the local Ollama instance is reachable.

    The ``provider`` parameter is accepted for registry dispatch compatibility.
    The ``api_key`` parameter is ignored — Ollama requires no authentication.
    Connectivity is validated by hitting the version endpoint.

    Returns ``{"valid": True}`` if reachable, ``{"valid": False, "error": "..."}``
    if not.
    """
    del provider, api_key
    try:
        base_url = _get_base_url()
        response = httpx.get(f"{base_url}/api/version", timeout=5)
        response.raise_for_status()
        return {"valid": True}
    except Exception as e:
        return {"valid": False, "error": str(e)}


__all__ = [
    "run_generate",
    "run_agenerate",
    "run_cogitate",
    "list_models",
    "validate_key",
]
