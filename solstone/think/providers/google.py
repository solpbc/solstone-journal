#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Gemini provider for agents and direct LLM generation.

This module provides the Google Gemini provider for the ``sol providers check`` CLI
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
    Provider-specific options (client).
"""

from __future__ import annotations

import logging
import os
import traceback
from pathlib import Path
from typing import Any, Callable

from google import genai
from google.genai import errors as google_errors
from google.genai import types

from solstone.think.cogitate_policy import resolve_read_scope
from solstone.think.models import GEMINI_FLASH
from solstone.think.utils import get_journal, get_project_root, now_ms

from .cli import QuotaExhaustedError, assemble_prompt
from .google_tools import (
    DEFAULT_READ_CALL_BUDGET,
    MAX_TURNS,
    CogitatePolicy,
    MaxTurnsExhausted,
    build_tool_declarations,
    glob,
    grep_search,
    list_directory,
    load_history,
    read_file,
    run_shell_command,
    save_history,
)
from .shared import (
    GenerateResult,
    JSONEventCallback,
    classify_provider_error,
)

GEMINI_MAX_OUTPUT_TOKENS = 65536
_DEFAULT_MAX_TOKENS = 8192
_DEFAULT_MODEL = GEMINI_FLASH

logger = logging.getLogger(__name__)

_READ_TOOL_NAMES = frozenset(
    {
        "read_file",
        "glob",
        "list_directory",
        "grep_search",
    }
)

# Backend detection cache
_detected_backend: str | None = None


def _structured_to_google_contents(
    messages: list[dict[str, str]],
) -> list[types.Content]:
    """Map role/content dicts to Gemini-native Content objects."""
    mapped: list[types.Content] = []
    for msg in messages:
        role = msg["role"]
        if role == "user":
            google_role = "user"
        elif role == "assistant":
            google_role = "model"
        else:
            raise ValueError(f"Unknown message role: {role!r}")
        mapped.append(
            types.Content(
                role=google_role,
                parts=[types.Part(text=msg["content"])],
            )
        )
    return mapped


# ---------------------------------------------------------------------------
# Client and helper functions for generate/agenerate
# ---------------------------------------------------------------------------


def _probe_backend(api_key: str) -> str:
    """Probe AI Studio endpoint to classify key type.

    Returns ``"aistudio"`` when the key works against the AI Studio models
    endpoint (HTTP 200) or ``"vertex"`` otherwise. Network errors default
    to ``"aistudio"`` for backward compatibility.
    """
    try:
        import httpx

        resp = httpx.get(
            "https://generativelanguage.googleapis.com/v1beta/models",
            params={"key": api_key},
            timeout=5,
        )
        return "aistudio" if resp.status_code == 200 else "vertex"
    except Exception:
        return "aistudio"


def _detect_backend(api_key: str) -> str:
    """Return cached backend detection result, probing on first call."""
    global _detected_backend
    if _detected_backend is not None:
        return _detected_backend
    _detected_backend = _probe_backend(api_key)
    return _detected_backend


def _get_effective_backend(api_key: str) -> str:
    """Return effective backend, checking config override before cache.

    Reads ``providers.google_backend`` from journal config. Values
    ``"aistudio"`` or ``"vertex"`` bypass detection; ``"auto"`` (the default
    when the key is absent) uses :func:`_detect_backend`.
    """
    from solstone.think.utils import get_config

    configured = get_config().get("providers", {}).get("google_backend", "auto")
    if configured in ("aistudio", "vertex"):
        return configured
    return _detect_backend(api_key)


def get_or_create_client(client: genai.Client | None = None) -> genai.Client:
    """Get existing client or create new one.

    For Vertex AI backend, uses service account credentials from config
    or falls back to GOOGLE_APPLICATION_CREDENTIALS env var.
    For AI Studio / auto-detect, uses GOOGLE_API_KEY.
    """
    if client is not None:
        return client

    from solstone.think.utils import get_config

    config = get_config()
    providers_config = config.get("providers", {})

    http_options = types.HttpOptions(retry_options=types.HttpRetryOptions(attempts=8))

    api_key = os.getenv("GOOGLE_API_KEY")

    # Determine backend
    configured_backend = providers_config.get("google_backend", "auto")
    if configured_backend == "vertex":
        backend = "vertex"
    elif configured_backend == "aistudio":
        backend = "aistudio"
    elif api_key:
        backend = _get_effective_backend(api_key)
    else:
        raise ValueError("GOOGLE_API_KEY not found in environment")

    if backend == "vertex":
        creds_path = providers_config.get("vertex_credentials")

        client_kwargs: dict[str, Any] = {
            "vertexai": True,
            "http_options": http_options,
        }

        if creds_path and os.path.exists(creds_path):
            import json as _json

            from google.oauth2.service_account import Credentials

            creds = Credentials.from_service_account_file(
                creds_path,
                scopes=["https://www.googleapis.com/auth/cloud-platform"],
            )
            client_kwargs["credentials"] = creds
            with open(creds_path, encoding="utf-8") as _f:
                _sa_data = _json.load(_f)
            if "project_id" in _sa_data:
                client_kwargs["project"] = _sa_data["project_id"]
        elif not os.getenv("GOOGLE_APPLICATION_CREDENTIALS"):
            raise ValueError(
                "Vertex AI backend requires service account credentials. "
                "Configure in Settings or set GOOGLE_APPLICATION_CREDENTIALS."
            )
        # else: GOOGLE_APPLICATION_CREDENTIALS is set, SDK auto-discovers

        client = genai.Client(**client_kwargs)
    else:
        # AI Studio path
        if not api_key:
            raise ValueError("GOOGLE_API_KEY not found in environment")
        client = genai.Client(
            api_key=api_key,
            vertexai=False,
            http_options=http_options,
        )

    return client


def _compute_agent_thinking_params(
    max_output_tokens: int, thinking_budget: int | None
) -> tuple[int, int]:
    """Compute total tokens and effective thinking budget for agent run.

    Args:
        max_output_tokens: Maximum output tokens from config.
        thinking_budget: Thinking budget from config, or None for dynamic.

    Returns:
        Tuple of (total_tokens, effective_thinking_budget).
        total_tokens = max_output_tokens + (thinking_budget or 0)
        effective_thinking_budget = thinking_budget if provided, else -1 (dynamic)
    """
    total_tokens = max_output_tokens + (thinking_budget or 0)
    effective_thinking_budget = thinking_budget if thinking_budget is not None else -1
    return total_tokens, effective_thinking_budget


def _build_generate_config(
    temperature: float,
    max_output_tokens: int,
    system_instruction: str | None,
    json_output: bool,
    thinking_budget: int | None,
    json_schema: dict | None = None,
    timeout_s: float | None = None,
) -> types.GenerateContentConfig:
    """Build the GenerateContentConfig.

    Note: Gemini's max_output_tokens is actually the total budget (thinking + output).
    We compute this internally: total = max_output_tokens + thinking_budget.
    """
    # Compute total tokens: output + thinking budget
    total_tokens = max_output_tokens + (thinking_budget or 0)
    if total_tokens > GEMINI_MAX_OUTPUT_TOKENS:
        clamped_max_output = min(max_output_tokens, GEMINI_MAX_OUTPUT_TOKENS)
        clamped_thinking = max(0, GEMINI_MAX_OUTPUT_TOKENS - clamped_max_output)
        logging.getLogger(__name__).warning(
            "Clamping Gemini token budget: max_output_tokens=%s thinking_budget=%s "
            "clamped_max_output_tokens=%s clamped_thinking_budget=%s",
            max_output_tokens,
            thinking_budget,
            clamped_max_output,
            clamped_thinking,
        )
        thinking_budget = clamped_thinking
        total_tokens = clamped_max_output + clamped_thinking

    config_args: dict[str, Any] = {
        "temperature": temperature,
        "max_output_tokens": total_tokens,
    }

    if system_instruction:
        config_args["system_instruction"] = system_instruction

    if json_output:
        config_args["response_mime_type"] = "application/json"
        if json_schema is not None:
            config_args["response_json_schema"] = json_schema

    # Set thinking config when caller explicitly specified a budget.
    # thinking_budget=0 must explicitly disable thinking (not omit config),
    # otherwise Gemini applies its own default budget consuming output tokens.
    if thinking_budget is not None:
        config_args["thinking_config"] = types.ThinkingConfig(
            thinking_budget=thinking_budget
        )

    if timeout_s:
        # Convert seconds to milliseconds for the SDK
        timeout_ms = int(timeout_s * 1000)
        config_args["http_options"] = types.HttpOptions(timeout=timeout_ms)

    return types.GenerateContentConfig(**config_args)


def _extract_response_text(response: Any) -> str:
    """Extract text from response.

    Returns response.text if available, or a friendly completion message
    if the response is empty. Raises on safety filter blocks.

    Parameters
    ----------
    response
        The response from the model.
    """
    if response is None:
        raise ValueError("No response from model")

    # Check for error conditions in candidates
    finish_reason = _extract_finish_reason(response)
    if finish_reason and "SAFETY" in finish_reason.upper():
        raise ValueError(f"Response blocked by safety filters: {finish_reason}")

    # Extract text, or generate friendly message if empty
    text = response.text if response.text else ""
    if text:
        return text

    # Empty text - generate user-friendly completion message
    return _format_completion_message(finish_reason, had_tool_calls=False)


def _normalize_finish_reason(response: Any) -> str | None:
    """Normalize finish_reason to standard values.

    Returns normalized string: "stop", "max_tokens", "safety", or None.
    """
    raw = _extract_finish_reason(response)
    if not raw:
        return None

    # Normalize (handle both enum names and string values)
    reason = raw.upper().replace("FINISHREASON.", "")

    if reason == "STOP":
        return "stop"
    elif reason == "MAX_TOKENS":
        return "max_tokens"
    elif "SAFETY" in reason:
        return "safety"
    elif reason == "RECITATION":
        return "recitation"
    else:
        return reason.lower()


def _extract_usage(response: Any) -> dict | None:
    """Extract normalized usage dict from response."""
    if not hasattr(response, "usage_metadata") or not response.usage_metadata:
        return None

    metadata = response.usage_metadata
    usage: dict[str, int] = {
        "input_tokens": getattr(metadata, "prompt_token_count", 0),
        "output_tokens": getattr(metadata, "candidates_token_count", 0),
        "total_tokens": getattr(metadata, "total_token_count", 0),
    }
    # Only include optional fields if non-zero
    cached = getattr(metadata, "cached_content_token_count", 0)
    if cached:
        usage["cached_tokens"] = cached
    reasoning = getattr(metadata, "thoughts_token_count", 0)
    if reasoning:
        usage["reasoning_tokens"] = reasoning
    return usage


def _extract_thinking(response: Any) -> list | None:
    """Extract thinking blocks from response.

    Returns list of ThinkingBlock dicts or None if no thinking.
    """
    if not hasattr(response, "candidates") or not response.candidates:
        return None

    thinking_blocks = []
    for candidate in response.candidates:
        if not candidate.content or not candidate.content.parts:
            continue
        for part in candidate.content.parts:
            if getattr(part, "thought", False) and getattr(part, "text", None):
                thinking_blocks.append({"summary": part.text})

    return thinking_blocks if thinking_blocks else None


def _extract_finish_reason(response: Any) -> str | None:
    """Extract finish_reason from response candidates.

    Returns the finish_reason string (e.g., "STOP", "MAX_TOKENS") or None
    if not available.
    """
    if not hasattr(response, "candidates") or not response.candidates:
        return None

    candidate = response.candidates[0]
    if hasattr(candidate, "finish_reason") and candidate.finish_reason:
        # Convert enum to string if needed
        reason = candidate.finish_reason
        if hasattr(reason, "name"):
            return reason.name
        return str(reason)
    return None


def _format_completion_message(finish_reason: str | None, had_tool_calls: bool) -> str:
    """Create a user-friendly completion message based on finish reason.

    Parameters
    ----------
    finish_reason
        The finish_reason from the response (e.g., "STOP", "MAX_TOKENS").
    had_tool_calls
        Whether tool calls were executed during this run.

    Returns
    -------
    str
        A concise, user-friendly completion message.
    """
    if not finish_reason:
        finish_reason = "UNKNOWN"

    # Normalize finish reason (handle both enum names and string values)
    reason = finish_reason.upper().replace("FINISHREASON.", "")

    if reason == "STOP":
        if had_tool_calls:
            return "Completed via tools."
        return "Completed."
    elif reason == "MAX_TOKENS":
        return "Reached token limit."
    elif "SAFETY" in reason:
        return "Blocked by safety filters."
    elif reason == "RECITATION":
        return "Stopped due to recitation."
    elif "TOOL" in reason or "FUNCTION" in reason:
        # UNEXPECTED_TOOL_CALL, MALFORMED_FUNCTION_CALL, etc.
        return "Tool execution incomplete."
    else:
        # Unknown reason - include it for debugging
        return f"Completed ({reason.lower()})."


def _log_empty_response_diagnostics(
    response: Any, finish_reason: str | None, had_tool_calls: bool
) -> None:
    """Log diagnostic information when response.text is empty.

    Helps debug intermittent empty response issues with Gemini models.
    """
    # Build diagnostic info
    diag = {
        "finish_reason": finish_reason,
        "had_tool_calls": had_tool_calls,
        "has_candidates": hasattr(response, "candidates") and bool(response.candidates),
    }

    if hasattr(response, "candidates") and response.candidates:
        candidate = response.candidates[0]
        diag["has_content"] = candidate.content is not None
        if candidate.content:
            diag["has_parts"] = bool(getattr(candidate.content, "parts", None))
            if hasattr(candidate.content, "parts") and candidate.content.parts:
                diag["num_parts"] = len(candidate.content.parts)
                # Check what types of parts we have
                part_types = []
                for part in candidate.content.parts:
                    if getattr(part, "thought", False):
                        part_types.append("thinking")
                    elif getattr(part, "text", None):
                        part_types.append("text")
                    elif hasattr(part, "function_call"):
                        part_types.append("function_call")
                    elif hasattr(part, "function_response"):
                        part_types.append("function_response")
                    else:
                        part_types.append("other")
                diag["part_types"] = part_types

    # Check for AFC history (indicates tools were auto-called)
    if hasattr(response, "automatic_function_calling_history"):
        afc_history = response.automatic_function_calling_history
        diag["afc_history_length"] = len(afc_history) if afc_history else 0

    logger.info(f"Empty response.text diagnostics: {diag}")


# ---------------------------------------------------------------------------
# run_generate / run_agenerate functions
# ---------------------------------------------------------------------------


def run_generate(
    contents: str | list[Any],
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
    client = kwargs.get("client")

    client = get_or_create_client(client)
    if isinstance(contents, str):
        contents = [contents]
    elif (
        isinstance(contents, list)
        and contents
        and isinstance(contents[0], dict)
        and "role" in contents[0]
    ):
        contents = _structured_to_google_contents(contents)
    config = _build_generate_config(
        temperature=temperature,
        max_output_tokens=max_output_tokens,
        system_instruction=system_instruction,
        json_output=json_output,
        thinking_budget=thinking_budget,
        json_schema=json_schema,
        timeout_s=timeout_s,
    )

    response = client.models.generate_content(
        model=model,
        contents=contents,
        config=config,
    )

    return GenerateResult(
        text=_extract_response_text(response),
        usage=_extract_usage(response),
        finish_reason=_normalize_finish_reason(response),
        thinking=_extract_thinking(response),
    )


async def run_agenerate(
    contents: str | list[Any],
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
    client = kwargs.get("client")

    client = get_or_create_client(client)
    if isinstance(contents, str):
        contents = [contents]
    elif (
        isinstance(contents, list)
        and contents
        and isinstance(contents[0], dict)
        and "role" in contents[0]
    ):
        contents = _structured_to_google_contents(contents)
    config = _build_generate_config(
        temperature=temperature,
        max_output_tokens=max_output_tokens,
        system_instruction=system_instruction,
        json_output=json_output,
        thinking_budget=thinking_budget,
        json_schema=json_schema,
        timeout_s=timeout_s,
    )

    response = await client.aio.models.generate_content(
        model=model,
        contents=contents,
        config=config,
    )

    return GenerateResult(
        text=_extract_response_text(response),
        usage=_extract_usage(response),
        finish_reason=_normalize_finish_reason(response),
        thinking=_extract_thinking(response),
    )


# ---------------------------------------------------------------------------
# Agent functions
# ---------------------------------------------------------------------------


def _cogitate_history_path(session_id: str | None) -> Path | None:
    if not session_id:
        return None
    return Path(get_journal()) / ".cache" / "cogitate-history" / f"{session_id}.json"


def _resolve_allowed_roots(config: dict[str, Any]) -> list[Path]:
    journal = Path(get_journal()).resolve()
    project_root = Path(get_project_root()).resolve()
    day = config.get("day") or ""
    span = int(config.get("read_scope_span", 0) or 0)
    scope_roots: list[Path] = []
    for scope in resolve_read_scope(config, day, span=span):
        scope_path = Path(scope).expanduser()
        if not scope_path.is_absolute():
            scope_path = journal / scope_path
        scope_roots.append(scope_path.resolve())
    return [journal, project_root, *scope_roots]


def _extract_retry_delay_ms(exc: BaseException) -> int | None:
    details = getattr(exc, "details", None)
    if not isinstance(details, dict):
        return None
    error = details.get("error")
    if not isinstance(error, dict):
        return None
    for item in error.get("details", []) or []:
        if not isinstance(item, dict):
            continue
        retry_delay = item.get("retryDelay")
        if not isinstance(retry_delay, str):
            continue
        value = retry_delay.strip()
        if value.endswith("s"):
            value = value[:-1]
        try:
            return int(float(value) * 1000)
        except ValueError:
            return None
    return None


def _raise_quota_if_needed(exc: google_errors.ClientError) -> None:
    if getattr(exc, "code", None) != 429 and getattr(exc, "status", None) != (
        "RESOURCE_EXHAUSTED"
    ):
        return
    message = str(exc) or getattr(exc, "message", "") or "Provider quota exhausted"
    raise QuotaExhaustedError(message, _extract_retry_delay_ms(exc)) from exc


def _build_cogitate_config(
    system_instruction: str | None,
) -> types.GenerateContentConfig:
    return types.GenerateContentConfig(
        system_instruction=system_instruction,
        tools=[types.Tool(function_declarations=build_tool_declarations())],
        tool_config=types.ToolConfig(
            function_calling_config=types.FunctionCallingConfig(
                mode=types.FunctionCallingConfigMode.AUTO
            )
        ),
        automatic_function_calling=types.AutomaticFunctionCallingConfig(disable=True),
        thinking_config=types.ThinkingConfig(
            include_thoughts=True,
            thinking_budget=-1,
        ),
    )


def _iter_response_parts(chunk: Any) -> list[Any]:
    parts: list[Any] = []
    for candidate in getattr(chunk, "candidates", []) or []:
        content = getattr(candidate, "content", None)
        if content is None:
            continue
        parts.extend(getattr(content, "parts", []) or [])
    return parts


def _call_id(tool_name: str, function_call: Any, turn_index: int, index: int) -> str:
    existing = getattr(function_call, "id", None)
    if existing:
        return str(existing)
    return f"{tool_name}-{turn_index}-{index}"


def _dispatch_google_tool(
    tool_name: str,
    args: dict[str, Any],
    allowed_roots: list[Path],
) -> dict[str, Any]:
    if tool_name == "read_file":
        return read_file(str(args.get("file_path", "")), allowed_roots=allowed_roots)
    if tool_name == "glob":
        return glob(
            str(args.get("pattern", "")),
            path=str(args.get("path", ".")),
            allowed_roots=allowed_roots,
        )
    if tool_name == "list_directory":
        return list_directory(
            str(args.get("dir_path", "")),
            allowed_roots=allowed_roots,
        )
    if tool_name == "grep_search":
        return grep_search(
            str(args.get("pattern", "")),
            path=str(args.get("path", ".")),
            include=str(args.get("include", "")),
            allowed_roots=allowed_roots,
        )
    if tool_name == "run_shell_command":
        return run_shell_command(str(args.get("command", "")))
    return {"error": f"unknown_tool: {tool_name}"}


def _emit_tool_budget_exhausted(
    callback: JSONEventCallback,
    tool_name: str,
    budget: int,
    count: int,
) -> None:
    callback.emit(
        {
            "event": "tool_budget_exhausted",
            "tool": tool_name,
            "budget": budget,
            "count": count,
            "read_tools": sorted(_READ_TOOL_NAMES),
            "ts": now_ms(),
        }
    )


def _raise_tool_budget_exhausted(count: int, budget: int) -> None:
    raise RuntimeError(
        f"tool_budget_exhausted: read tool call budget exceeded ({count}/{budget})"
    )


async def run_cogitate(
    config: dict[str, Any],
    on_event: Callable[[dict], None] | None = None,
) -> str:
    """Run a prompt with tool-calling support via Google Gemini SDK.

    Args:
        config: Complete configuration dictionary including prompt, system_instruction,
            user_instruction, extra_context, model, etc.
        on_event: Optional event callback
    """
    model = config.get("model", _DEFAULT_MODEL)
    session_id = config.get("session_id")
    callback = JSONEventCallback(on_event)

    try:
        prompt_body, system_instruction = assemble_prompt(
            config,
            sol_tool_name="run_shell_command" if not config.get("write") else None,
        )
        client = get_or_create_client(config.get("client"))
        sdk_config = _build_cogitate_config(system_instruction)
        history_path = _cogitate_history_path(session_id)
        history = load_history(history_path) if history_path else []
        chat = client.chats.create(
            model=model,
            config=sdk_config,
            history=history,
        )
        allowed_roots = _resolve_allowed_roots(config)
        policy = CogitatePolicy(
            write=bool(config.get("write")), allowed_roots=allowed_roots
        )
        read_call_budget = int(
            config.get("read_call_budget", DEFAULT_READ_CALL_BUDGET) or 0
        )
        read_call_count = 0
        usage: dict[str, Any] = {}

        next_message: str | list[types.Part] = prompt_body
        result_parts: list[str] = []
        for turn_index in range(MAX_TURNS):
            function_calls: list[dict[str, Any]] = []
            stream = chat.send_message_stream(next_message)
            for chunk in stream:
                chunk_usage = _extract_usage(chunk)
                if chunk_usage:
                    usage = chunk_usage
                for part in _iter_response_parts(chunk):
                    text = getattr(part, "text", None)
                    is_thought = bool(getattr(part, "thought", False))
                    if is_thought and isinstance(text, str) and text.strip():
                        callback.emit(
                            {
                                "event": "thinking",
                                "summary": text.strip(),
                                "model": model,
                                "ts": now_ms(),
                            }
                        )
                    if not is_thought and isinstance(text, str) and text:
                        result_parts.append(text)
                        callback.emit(
                            {
                                "event": "text_delta",
                                "delta": text,
                                "model": model,
                                "ts": now_ms(),
                            }
                        )
                    function_call = getattr(part, "function_call", None)
                    if function_call is not None:
                        tool_name = str(getattr(function_call, "name", "") or "")
                        args = dict(getattr(function_call, "args", {}) or {})
                        function_calls.append(
                            {
                                "tool": tool_name,
                                "args": args,
                                "call_id": _call_id(
                                    tool_name,
                                    function_call,
                                    turn_index,
                                    len(function_calls),
                                ),
                            }
                        )

            if history_path:
                save_history(history_path, chat.get_history(curated=True))

            if not function_calls:
                result = "".join(result_parts).strip()
                finish_event: dict[str, Any] = {
                    "event": "finish",
                    "result": result,
                    "ts": now_ms(),
                }
                if usage:
                    finish_event["usage"] = usage
                if session_id:
                    finish_event["cli_session_id"] = session_id
                callback.emit(finish_event)
                return result

            response_parts: list[types.Part] = []
            for function_call in function_calls:
                tool_name = function_call["tool"]
                args = function_call["args"]
                call_id = function_call["call_id"]
                callback.emit(
                    {
                        "event": "tool_start",
                        "tool": tool_name,
                        "args": args,
                        "call_id": call_id,
                        "ts": now_ms(),
                    }
                )
                if tool_name in _READ_TOOL_NAMES:
                    read_call_count += 1
                    if read_call_count > read_call_budget:
                        _emit_tool_budget_exhausted(
                            callback,
                            tool_name,
                            read_call_budget,
                            read_call_count,
                        )
                        _raise_tool_budget_exhausted(
                            read_call_count,
                            read_call_budget,
                        )
                allowed, reason = policy.check(tool_name, args)
                if allowed:
                    tool_result = _dispatch_google_tool(tool_name, args, allowed_roots)
                else:
                    tool_result = {"error": reason}
                callback.emit(
                    {
                        "event": "tool_end",
                        "tool": tool_name,
                        "args": args,
                        "call_id": call_id,
                        "result": tool_result,
                        "ts": now_ms(),
                    }
                )
                response_parts.append(
                    types.Part.from_function_response(
                        name=tool_name,
                        response=tool_result,
                    )
                )
            next_message = response_parts

        callback.emit(
            {
                "event": "max_turns_exhausted",
                "max_turns": MAX_TURNS,
                "ts": now_ms(),
            }
        )
        raise MaxTurnsExhausted(
            f"max_turns_exhausted: Google cogitate exceeded {MAX_TURNS} turns"
        )
    except google_errors.ClientError as exc:
        _raise_quota_if_needed(exc)
        callback.emit(
            {
                "event": "error",
                "error": str(exc),
                "reason_code": classify_provider_error(exc, "google"),
                "provider": "google",
                "trace": traceback.format_exc(),
            }
        )
        setattr(exc, "_evented", True)
        raise
    except google_errors.APIError as exc:
        callback.emit(
            {
                "event": "error",
                "error": str(exc),
                "reason_code": classify_provider_error(exc, "google"),
                "provider": "google",
                "trace": traceback.format_exc(),
            }
        )
        setattr(exc, "_evented", True)
        raise
    except QuotaExhaustedError:
        raise
    except MaxTurnsExhausted:
        raise
    except RuntimeError as exc:
        if str(exc).startswith("tool_budget_exhausted:"):
            raise
        callback.emit(
            {
                "event": "error",
                "error": str(exc),
                "reason_code": classify_provider_error(exc, "google"),
                "provider": "google",
                "trace": traceback.format_exc(),
            }
        )
        setattr(exc, "_evented", True)
        raise
    except Exception as exc:
        callback.emit(
            {
                "event": "error",
                "error": str(exc),
                "reason_code": classify_provider_error(exc, "google"),
                "provider": "google",
                "trace": traceback.format_exc(),
            }
        )
        setattr(exc, "_evented", True)
        raise


def list_models() -> list[dict]:
    """List available Google Gemini models.

    Returns
    -------
    list[dict]
        List of raw model info objects from the Google Gemini API.
    """
    client = get_or_create_client()
    return [m.model_dump() for m in client.models.list()]


def validate_key(api_key: str) -> dict:
    """Validate a Google API key by listing models.

    Creates a temporary client with the provided key. Never uses
    the cached client or environment variables.

    Returns {"valid": True, "backend": "aistudio"|"vertex"} or
    {"valid": False, "error": "..."}.
    """
    global _detected_backend
    try:
        # Probe backend for this specific key (always probes, bypasses cache).
        backend = _probe_backend(api_key)

        client_kwargs = {
            "api_key": api_key,
            "http_options": types.HttpOptions(timeout=10000),
            "vertexai": backend == "vertex",
        }

        client = genai.Client(**client_kwargs)
        list(client.models.list(config={"page_size": 1}))
        _detected_backend = backend  # only cache after successful validation
        return {"valid": True, "backend": backend}
    except Exception as e:
        return {"valid": False, "error": str(e)}


def validate_vertex_credentials(
    creds_path: str,
) -> dict:
    """Validate Vertex AI service account credentials by listing models.

    Creates a temporary client with the provided SA credentials.

    Returns {"valid": True, "email": "..."} or {"valid": False, "error": "..."}.
    """
    try:
        import json as _json

        from google.oauth2.service_account import Credentials

        creds = Credentials.from_service_account_file(
            creds_path,
            scopes=["https://www.googleapis.com/auth/cloud-platform"],
        )
        client_kwargs: dict[str, Any] = {
            "vertexai": True,
            "credentials": creds,
            "http_options": types.HttpOptions(timeout=10000),
        }
        with open(creds_path, encoding="utf-8") as _f:
            _sa_data = _json.load(_f)
        if "project_id" in _sa_data:
            client_kwargs["project"] = _sa_data["project_id"]

        client = genai.Client(**client_kwargs)
        list(client.models.list(config={"page_size": 1}))
        return {"valid": True, "email": creds.service_account_email}
    except Exception as e:
        return {"valid": False, "error": str(e)}


__all__ = [
    "run_cogitate",
    "run_generate",
    "run_agenerate",
    "get_or_create_client",
    "_detect_backend",
    "_get_effective_backend",
    "list_models",
    "validate_key",
    "validate_vertex_credentials",
]
