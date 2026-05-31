#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Gemini provider for agents and direct LLM generation.

This module provides the Google Gemini provider for the ``journal providers check`` CLI
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
from typing import Any

from google import genai
from google.genai import types

from solstone.think.models import GEMINI_FLASH

from .shared import GenerateResult

# Vertex's `maxOutputTokens` accepts 1..65535 inclusive — exactly 65536 returns
# 400 INVALID_ARGUMENT ("supported range is from 1 (inclusive) to 65536
# (exclusive)"). The clamp logic in _build_generate_config sums max_output_tokens
# + thinking_budget into this single field, so the bound applies to the total.
GEMINI_MAX_OUTPUT_TOKENS = 65535
_DEFAULT_MAX_TOKENS = 8192
_DEFAULT_MODEL = GEMINI_FLASH

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


def _active_backend() -> str | None:
    """Return the currently-active Google backend ("aistudio" | "vertex"), or None.

    Mirrors the resolution order in :func:`get_or_create_client` so callers can
    decide backend-specific behavior (e.g. alias remapping) without duplicating
    config-vs-env-vs-cache logic. Returns ``None`` when no backend can be
    determined (no config override, no API key) — callers should treat that as
    "unknown" rather than guessing.
    """
    from solstone.think.utils import get_config

    configured = get_config().get("providers", {}).get("google_backend", "auto")
    if configured in ("aistudio", "vertex"):
        return configured
    api_key = os.getenv("GOOGLE_API_KEY")
    if api_key:
        return _get_effective_backend(api_key)
    return None


# Google publishes the `gemini-*-latest` aliases on AI Studio
# (`generativelanguage.googleapis.com`) but Vertex AI's publisher-model registry
# only honors them for the flash tiers — `gemini-pro-latest` 404s on Vertex.
# AI Studio is the default path for the vast majority of solstone users, so we
# keep `-latest` as the canonical model identifier across the codebase (preserves
# Google's auto-upgrade for those users) and remap only on the Vertex hop.
# When bumping the Vertex target, list models from a Vertex-backed client and
# pick the most recent `gemini-<N>.<M>-pro[-preview]` that resolves cleanly —
# we deliberately track the latest preview here rather than retreating to GA,
# matching the spirit of the `-latest` rail we're substituting for.
_VERTEX_MODEL_ALIASES: dict[str, str] = {
    "gemini-pro-latest": "gemini-3.1-pro-preview",
}


def _resolve_model_for_vertex(model: str) -> str:
    """Substitute Vertex-resolvable identifiers for AI-Studio-only aliases.

    No-op unless the active backend is Vertex AND the model is a known-broken
    alias on Vertex. See :data:`_VERTEX_MODEL_ALIASES` for the mapping and the
    rationale comment above it.
    """
    if model not in _VERTEX_MODEL_ALIASES:
        return model
    if _active_backend() != "vertex":
        return model
    return _VERTEX_MODEL_ALIASES[model]


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
    usage: dict[str, Any] = {
        "input_tokens": getattr(metadata, "prompt_token_count", 0),
        "output_tokens": getattr(metadata, "candidates_token_count", 0),
        "total_tokens": getattr(metadata, "total_token_count", 0),
    }
    model_version = getattr(response, "model_version", None)
    if isinstance(model_version, str) and model_version:
        usage["model_version"] = model_version
    # Only include optional fields if non-zero
    cached = getattr(metadata, "cached_content_token_count", 0)
    if cached:
        usage["cached_tokens"] = cached
    reasoning = getattr(metadata, "thoughts_token_count", 0)
    if reasoning:
        usage["reasoning_tokens"] = reasoning
    return usage


def _resolved_model(response: Any, requested: str) -> str:
    """Return resolved response.model_version when it is a non-empty string, else requested."""
    resolved = getattr(response, "model_version", None)
    if isinstance(resolved, str) and resolved:
        return resolved
    return requested


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


def _summarize_contents(contents: Any) -> str:
    """One-line, PII-free fingerprint of the contents passed to generate_content.

    Captures part count, types, and image sizes — enough to correlate a 400 with
    the call shape without leaking text bodies. Used only by error logging.
    """
    if isinstance(contents, str):
        return f"str(len={len(contents)})"
    if not isinstance(contents, list):
        return f"other({type(contents).__name__})"
    parts: list[str] = []
    for item in contents:
        if isinstance(item, str):
            parts.append(f"s{len(item)}")
        elif isinstance(item, dict) and "role" in item:
            parts.append(f"d({item.get('role')})")
        elif isinstance(item, types.Part):
            mime = getattr(getattr(item, "inline_data", None), "mime_type", None)
            parts.append(f"p({mime or '?'})")
        elif isinstance(item, types.Content):
            parts.append(f"c({item.role})")
        else:
            size = getattr(item, "size", None)
            if size is not None:
                parts.append(f"i({size[0]}x{size[1]})")
            else:
                parts.append(type(item).__name__[:6])
    return f"list[{len(parts)}]={','.join(parts)}"


def _log_provider_error(
    exc: BaseException,
    *,
    model: str,
    contents: Any,
    config: types.GenerateContentConfig,
) -> None:
    """Emit a structured log line when generate_content raises a ClientError.

    Captures the SDK-returned error body (which Cloud Monitoring does not surface)
    so we can categorize 400s offline. WARNING level so the line lands in
    solstone's normal log streams without changing existing exception propagation.
    """
    from google.genai import errors as _genai_errors

    if not isinstance(exc, _genai_errors.APIError):
        return

    status_code = getattr(exc, "code", None) or getattr(exc, "status_code", None)
    json_schema_present = getattr(config, "response_json_schema", None) is not None
    thinking_cfg = getattr(config, "thinking_config", None)
    thinking_budget = (
        getattr(thinking_cfg, "thinking_budget", None) if thinking_cfg else None
    )

    logging.getLogger(__name__).warning(
        "google_provider_error status=%s model=%s backend=%s "
        "contents=%s json=%s schema=%s thinking_budget=%s max_output_tokens=%s "
        "body=%s",
        status_code,
        model,
        _active_backend() or "unknown",
        _summarize_contents(contents),
        bool(getattr(config, "response_mime_type", "")),
        json_schema_present,
        thinking_budget,
        getattr(config, "max_output_tokens", None),
        str(exc)[:2000],
    )


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
    model = _resolve_model_for_vertex(model)
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

    try:
        response = client.models.generate_content(
            model=model,
            contents=contents,
            config=config,
        )
    except Exception as exc:
        _log_provider_error(exc, model=model, contents=contents, config=config)
        raise

    return GenerateResult(
        text=_extract_response_text(response),
        model=_resolved_model(response, model),
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
    model = _resolve_model_for_vertex(model)
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

    try:
        response = await client.aio.models.generate_content(
            model=model,
            contents=contents,
            config=config,
        )
    except Exception as exc:
        _log_provider_error(exc, model=model, contents=contents, config=config)
        raise

    return GenerateResult(
        text=_extract_response_text(response),
        model=_resolved_model(response, model),
        usage=_extract_usage(response),
        finish_reason=_normalize_finish_reason(response),
        thinking=_extract_thinking(response),
    )


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
    "run_generate",
    "run_agenerate",
    "get_or_create_client",
    "_detect_backend",
    "_get_effective_backend",
    "_active_backend",
    "_resolve_model_for_vertex",
    "list_models",
    "validate_key",
    "validate_vertex_credentials",
]
