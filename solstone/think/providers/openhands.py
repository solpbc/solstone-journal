# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Unified OpenHands/LiteLLM transport for personal cloud thinking.

OpenHands and LiteLLM are installed on demand, so this module must stay importable
without either package present. Keep all OpenHands/LiteLLM imports inside the
functions that use them.
"""

from __future__ import annotations

import asyncio
import json
import logging
import math
import os
import re
import shutil
import sys
import threading
import traceback
import uuid
from collections.abc import Callable
from contextlib import contextmanager
from datetime import datetime, timezone
from email.utils import parsedate_to_datetime
from pathlib import Path
from typing import Any

from solstone.log_policy import apply_http_logging_policy, snapshot_root_logging
from solstone.think.cogitate_contract import (
    capabilities_for_access_tier,
    expects_emit_final,
)
from solstone.think.cogitate_policy import (
    _FALLBACK_USD_PER_TOKEN,
    CONTEXT_FINAL_FRAC,
    CONTEXT_WARN_FRAC,
    COST_WARN_FRAC,
    DEFAULT_READ_CALL_BUDGET,
    DEFAULT_RUN_COST_CAP_USD,
    MAX_TURNS,
    MAX_TURNS_HEADROOM,
    TURN_WARN_FRACS,
    CogitatePolicy,
    resolve_read_scope,
)
from solstone.think.providers.cli import (
    ProviderKeyMissingError,
    QuotaExhaustedError,
    assemble_prompt,
)
from solstone.think.providers.local_admission import (
    LocalAdmissionCancelled,
    LocalSlotLease,
)
from solstone.think.providers.local_server import LOCAL_MIN_CONTEXT_TOKENS
from solstone.think.providers.shared import (
    USAGE_KEYS,
    GenerateResult,
    JSONEventCallback,
    classify_provider_error,
    safe_raw,
)
from solstone.think.utils import get_journal, get_project_root, now_ms

LOG = logging.getLogger("solstone.think.providers.openhands")

_MODEL_PREFIXES = {
    "anthropic": "anthropic",
    "openai": "openai",
    "google": "gemini",
}
_API_KEY_ENV = {
    "anthropic": "ANTHROPIC_API_KEY",
    "openai": "OPENAI_API_KEY",
    "google": "GOOGLE_API_KEY",
}
_KNOWN_MODEL_PREFIXES = frozenset({"anthropic", "openai", "google", "gemini", "local"})
_SHELL_STDOUT_CAP = 6000
_SHELL_STDERR_CAP = 6000
_SHELL_TIMEOUT_SECONDS = 30
_COST_WARNING_TEXT = "Cost calculation failed"
_LOCAL_OUTPUT_RESERVE_TOKENS = LOCAL_MIN_CONTEXT_TOKENS // 4
_LOCAL_CONDENSER_MAX_TOKENS = LOCAL_MIN_CONTEXT_TOKENS * 11 // 16
_LOCAL_CONDENSER_KEEP_FIRST = 4
_GENERATE_NUM_RETRIES = 2
_GEMINI_MAX_OUTPUT_TOKENS = 65_535
_ANTHROPIC_THINKING_BUFFER = 1_000
_SCHEMA_NAME_RE = re.compile(r"^[a-zA-Z0-9_-]{1,64}$")


@contextmanager
def _openhands_import_policy():
    """Contain OpenHands import side effects and irrelevant LiteLLM warnings."""
    root_baseline = snapshot_root_logging()
    os.environ.setdefault("OPENHANDS_SUPPRESS_BANNER", "1")
    litellm_log = logging.getLogger("LiteLLM")
    prior_litellm_level = litellm_log.level
    litellm_log.setLevel(logging.ERROR)
    try:
        yield
    finally:
        litellm_log.setLevel(prior_litellm_level)
        apply_http_logging_policy(root_baseline)


def _prefixed_model(provider: str, model: str) -> str:
    if provider == "local":
        base_model = str(model)
        if base_model.startswith("openai/"):
            return base_model
        return f"openai/{base_model}"

    prefix = _MODEL_PREFIXES[provider]
    base_model = str(model)
    if "/" in base_model:
        candidate_prefix, candidate_model = base_model.split("/", 1)
        if candidate_prefix in _KNOWN_MODEL_PREFIXES:
            base_model = candidate_model
    return f"{prefix}/{base_model}"


def _resolve_provider_key(provider: str, api_key: str | None = None) -> str:
    """Resolve the effective cloud key or raise before any provider request.

    Explicit ``api_key`` (probe path) wins over env. A None/blank/whitespace
    effective key raises ProviderKeyMissingError pointing the owner at Thinking.
    """

    env_key = _API_KEY_ENV[provider]
    effective = api_key if api_key is not None else os.getenv(env_key)
    if effective is None or not effective.strip():
        from solstone.think.providers import PROVIDER_METADATA

        label = PROVIDER_METADATA[provider]["label"]
        raise ProviderKeyMissingError(
            provider,
            env_key,
            f"{label} is missing its API key. Open Thinking to add "
            f"credentials before trying again.",
        )
    return effective


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


def _session_identity(value: Any) -> tuple[str, uuid.UUID]:
    if not value:
        conversation_id = uuid.uuid4()
        return str(conversation_id), conversation_id

    session_id = str(value)
    try:
        return session_id, uuid.UUID(session_id)
    except ValueError:
        return session_id, uuid.uuid5(
            uuid.NAMESPACE_URL,
            f"solstone:cogitate:{session_id}",
        )


def _build_llm(provider: str, model: str) -> Any:
    from openhands.sdk import LLM

    if provider == "local":
        from solstone.think.providers.local_endpoint import resolve_local_endpoint
        from solstone.think.services.spp_transport import confidential_egress_base_url

        endpoint = resolve_local_endpoint()
        if not endpoint.is_bundled:
            base_url = confidential_egress_base_url(endpoint.base_url)
            return LLM(
                model=f"openai/{endpoint.served_model_id}",
                base_url=f"{base_url}/v1",
                api_key=endpoint.credential or "EMPTY",
                native_tool_calling=False,
                timeout=LLM_TIMEOUT_S,
                num_retries=LLM_NUM_RETRIES,
                input_cost_per_token=0,
                output_cost_per_token=0,
                litellm_extra_body={"chat_template_kwargs": {"enable_thinking": False}},
            )

        from solstone.think.providers import local_server

        server = local_server.connect()
        return LLM(
            model=f"openai/{server.served_model_id}",
            base_url=f"http://127.0.0.1:{server.port}/v1",
            api_key="EMPTY",
            native_tool_calling=False,
            timeout=LLM_TIMEOUT_S,
            num_retries=LLM_NUM_RETRIES,
            max_input_tokens=local_server.LOCAL_MIN_CONTEXT_TOKENS,
            max_output_tokens=_LOCAL_OUTPUT_RESERVE_TOKENS,
            input_cost_per_token=0,
            output_cost_per_token=0,
            litellm_extra_body={"chat_template_kwargs": {"enable_thinking": False}},
        )

    if provider not in _MODEL_PREFIXES:
        raise ValueError(f"Unsupported OpenHands provider: {provider}")

    llm_kwargs: dict[str, Any] = {
        "model": _prefixed_model(provider, model),
        "api_key": _resolve_provider_key(provider),
        "native_tool_calling": True,
        "timeout": LLM_TIMEOUT_S,
        "num_retries": LLM_NUM_RETRIES,
    }
    if provider == "openai":
        llm_kwargs["reasoning_summary"] = "auto"
        llm_kwargs["enable_encrypted_reasoning"] = True
    return LLM(**llm_kwargs)


def _parse_openai_effort(model: str) -> tuple[str, str | None]:
    """Split Solstone's optional OpenAI reasoning-effort model suffix."""
    from solstone.think.models import OPENAI_EFFORT_SUFFIXES

    for suffix in OPENAI_EFFORT_SUFFIXES:
        if model.endswith(suffix):
            return model[: -len(suffix)], suffix[1:]
    return model, None


def _generate_token_budget(
    provider: str,
    max_output_tokens: int,
    thinking_budget: int | None,
) -> int:
    """Return the provider-facing output ceiling for a generate call."""
    if provider == "google":
        total = max_output_tokens + max(0, thinking_budget or 0)
        if total > _GEMINI_MAX_OUTPUT_TOKENS:
            LOG.warning(
                "Clamping Gemini token budget: max_output_tokens=%s "
                "thinking_budget=%s total=%s clamp=%s",
                max_output_tokens,
                thinking_budget,
                total,
                _GEMINI_MAX_OUTPUT_TOKENS,
            )
            return _GEMINI_MAX_OUTPUT_TOKENS
        return total
    if provider == "anthropic" and thinking_budget and thinking_budget > 0:
        return max(
            max_output_tokens,
            thinking_budget + _ANTHROPIC_THINKING_BUFFER + 1,
        )
    return max_output_tokens


def _build_generate_llm(
    provider: str,
    model: str,
    *,
    max_output_tokens: int,
    thinking_budget: int | None,
    timeout_s: float,
    api_key: str | None = None,
    num_retries: int = _GENERATE_NUM_RETRIES,
) -> tuple[Any, str]:
    """Build a stateless OpenHands LLM for one direct generation request."""
    from openhands.sdk import LLM

    if provider not in _MODEL_PREFIXES:
        raise ValueError(f"Unsupported OpenHands provider: {provider}")

    api_model = model
    effort = None
    if provider == "openai":
        api_model, effort = _parse_openai_effort(model)

    return (
        LLM(
            model=_prefixed_model(provider, api_model),
            api_key=_resolve_provider_key(provider, api_key),
            timeout=max(1, math.ceil(timeout_s)),
            num_retries=num_retries,
            max_output_tokens=_generate_token_budget(
                provider,
                max_output_tokens,
                thinking_budget,
            ),
            # OpenHands defaults are agent-oriented: high reasoning, a 200k
            # Anthropic thinking budget, prompt-cache breakpoints, and encrypted
            # reasoning. Generate calls opt into reasoning explicitly instead.
            reasoning_effort=effort,
            extended_thinking_budget=None,
            caching_prompt=False,
            prompt_cache_retention=None,
            enable_encrypted_reasoning=False,
            openrouter_site_url="",
            openrouter_app_name="",
        ),
        api_model,
    )


def _data_url(part: Any) -> str:
    from solstone.think.providers._image import encode_image_part

    media_type, payload = encode_image_part(part)
    return f"data:{media_type};base64,{payload}"


def _message_content(value: Any) -> list[Any]:
    """Convert Solstone's generate input shapes to OpenHands content blocks."""
    from openhands.sdk.llm import ImageContent, TextContent

    from solstone.think.providers._image import is_image_part

    if is_image_part(value):
        return [ImageContent(image_urls=[_data_url(value)])]
    if isinstance(value, list | tuple):
        blocks: list[Any] = []
        for item in value:
            blocks.extend(_message_content(item))
        return blocks
    if isinstance(value, dict):
        part_type = str(value.get("type") or "")
        if part_type in {"text", "input_text", "output_text"}:
            return [TextContent(text=str(value.get("text") or ""))]
        if part_type in {"image_url", "input_image"}:
            image_url = value.get("image_url")
            if isinstance(image_url, dict):
                image_url = image_url.get("url")
            if isinstance(image_url, str) and image_url:
                return [ImageContent(image_urls=[image_url])]
    return [TextContent(text=str(value))]


def _generate_messages(
    contents: Any,
    system_instruction: str | None,
) -> list[Any]:
    from openhands.sdk.llm import Message, TextContent

    messages: list[Any] = []
    if system_instruction:
        messages.append(
            Message(role="system", content=[TextContent(text=system_instruction)])
        )

    if (
        isinstance(contents, list)
        and contents
        and isinstance(contents[0], dict)
        and "role" in contents[0]
    ):
        for item in contents:
            if not isinstance(item, dict):
                raise ValueError("role-based generate contents must contain messages")
            role = str(item.get("role") or "user")
            if role == "model":
                role = "assistant"
            if role not in {"user", "system", "assistant", "tool"}:
                raise ValueError(f"Unknown message role: {role!r}")
            messages.append(
                Message(role=role, content=_message_content(item.get("content", "")))
            )
    else:
        messages.append(Message(role="user", content=_message_content(contents)))
    return messages


def _schema_name(schema: dict | None) -> str:
    title = schema.get("title") if isinstance(schema, dict) else None
    if isinstance(title, str) and _SCHEMA_NAME_RE.fullmatch(title):
        return title
    return "response"


def _generate_call_kwargs(
    provider: str,
    model: str,
    *,
    temperature: float | None,
    json_output: bool,
    json_schema: dict | None,
    thinking_budget: int | None,
    responses_api: bool,
) -> dict[str, Any]:
    kwargs: dict[str, Any] = {}
    # Preserve each direct provider's accepted parameter contract. OpenAI's
    # reasoning models reject temperature, and Anthropic rejects it while
    # extended thinking is enabled (plus a few model-specific cases).
    include_temperature = provider == "google"
    if provider == "anthropic" and not (thinking_budget and thinking_budget > 0):
        from solstone.think.models import model_supports

        include_temperature = model_supports(model, "temperature")
    if temperature is not None and include_temperature:
        kwargs["temperature"] = temperature

    if provider == "google" and thinking_budget is not None:
        kwargs["thinking"] = {
            "type": "enabled" if thinking_budget > 0 else "disabled",
            "budget_tokens": max(0, thinking_budget),
        }
    elif provider == "anthropic" and thinking_budget and thinking_budget > 0:
        kwargs["thinking"] = {
            "type": "enabled",
            "budget_tokens": thinking_budget,
        }

    if json_schema is not None:
        schema_format = {
            "type": "json_schema",
            "name": _schema_name(json_schema),
            "schema": json_schema,
            "strict": True,
        }
        if responses_api:
            kwargs["text"] = {"format": schema_format}
        else:
            kwargs["response_format"] = {
                "type": "json_schema",
                "json_schema": schema_format,
            }
    elif json_output:
        if responses_api:
            kwargs["text"] = {"format": {"type": "json_object"}}
        else:
            kwargs["response_format"] = {"type": "json_object"}
    return kwargs


def _get(value: Any, key: str, default: Any = None) -> Any:
    if isinstance(value, dict):
        return value.get(key, default)
    return getattr(value, key, default)


def _response_text(response: Any) -> str:
    parts = []
    for content in response.message.content:
        text = getattr(content, "text", None)
        if isinstance(text, str):
            parts.append(text)
    return "".join(parts)


def _response_thinking(response: Any) -> list[dict[str, Any]] | None:
    message = response.message
    blocks: list[dict[str, Any]] = []
    for block in message.thinking_blocks:
        thinking = getattr(block, "thinking", None)
        if isinstance(thinking, str) and thinking:
            item: dict[str, Any] = {"summary": thinking}
            signature = getattr(block, "signature", None)
            if signature:
                item["signature"] = signature
            blocks.append(item)
            continue
        data = getattr(block, "data", None)
        if data:
            blocks.append({"summary": "[redacted]", "redacted_data": data})

    if not blocks and message.reasoning_content:
        blocks.append({"summary": message.reasoning_content})
    reasoning_item = message.responses_reasoning_item
    if not blocks and reasoning_item is not None:
        blocks.extend(
            {"summary": summary}
            for summary in reasoning_item.summary
            if isinstance(summary, str) and summary
        )
    return blocks or None


def _response_usage(response: Any) -> dict[str, Any] | None:
    token_usage = response.metrics.accumulated_token_usage
    if token_usage is None:
        return None
    usage: dict[str, Any] = {
        "input_tokens": token_usage.prompt_tokens,
        "output_tokens": token_usage.completion_tokens,
        "total_tokens": token_usage.prompt_tokens + token_usage.completion_tokens,
    }
    if token_usage.cache_read_tokens:
        usage["cached_tokens"] = token_usage.cache_read_tokens
    if token_usage.cache_write_tokens:
        usage["cache_creation_tokens"] = token_usage.cache_write_tokens
    if token_usage.reasoning_tokens:
        usage["reasoning_tokens"] = token_usage.reasoning_tokens
    if not any(usage.values()):
        return None
    raw_model = _get(response.raw_response, "model")
    if isinstance(raw_model, str) and raw_model:
        usage["model_version"] = raw_model
    return usage


def _normalize_finish_reason(response: Any) -> str | None:
    raw = response.raw_response
    choices = _get(raw, "choices")
    if choices:
        reason = _get(choices[0], "finish_reason")
    else:
        status = _get(raw, "status")
        if status == "completed":
            return "stop"
        if status == "incomplete":
            details = _get(raw, "incomplete_details")
            reason = _get(details, "reason", "max_tokens")
        elif status == "failed":
            return "error"
        else:
            reason = status
    if not reason:
        return None
    normalized = str(reason).strip().lower()
    return {
        "end_turn": "stop",
        "stop_sequence": "stop",
        "length": "max_tokens",
        "max_output_tokens": "max_tokens",
    }.get(normalized, normalized)


def _generate_result(response: Any, requested_model: str) -> GenerateResult:
    raw_model = _get(response.raw_response, "model")
    return GenerateResult(
        text=_response_text(response),
        model=raw_model
        if isinstance(raw_model, str) and raw_model
        else requested_model,
        usage=_response_usage(response),
        finish_reason=_normalize_finish_reason(response),
        thinking=_response_thinking(response),
    )


def _run_generate(
    contents: Any,
    model: str,
    *,
    provider: str,
    temperature: float | None,
    max_output_tokens: int,
    system_instruction: str | None,
    json_output: bool,
    thinking_budget: int | None,
    json_schema: dict | None,
    timeout_s: float,
    api_key: str | None = None,
    num_retries: int = _GENERATE_NUM_RETRIES,
) -> GenerateResult:
    with _openhands_import_policy():
        llm, _ = _build_generate_llm(
            provider,
            model,
            max_output_tokens=max_output_tokens,
            thinking_budget=thinking_budget,
            timeout_s=timeout_s,
            api_key=api_key,
            num_retries=num_retries,
        )
        messages = _generate_messages(contents, system_instruction)
    # Direct OpenAI is intentionally Responses-first even for custom model ids,
    # matching Solstone's previous native OpenAI contract. Other providers use
    # chat completion through LiteLLM's provider translation.
    responses_api = provider == "openai"
    call_kwargs = _generate_call_kwargs(
        provider,
        model,
        temperature=temperature,
        json_output=json_output,
        json_schema=json_schema,
        thinking_budget=thinking_budget,
        responses_api=responses_api,
    )
    response = (
        llm.responses(messages, **call_kwargs)
        if responses_api
        else llm.completion(messages, **call_kwargs)
    )
    return _generate_result(response, model)


async def _run_agenerate(
    contents: Any,
    model: str,
    *,
    provider: str,
    temperature: float | None,
    max_output_tokens: int,
    system_instruction: str | None,
    json_output: bool,
    thinking_budget: int | None,
    json_schema: dict | None,
    timeout_s: float,
) -> GenerateResult:
    with _openhands_import_policy():
        llm, _ = _build_generate_llm(
            provider,
            model,
            max_output_tokens=max_output_tokens,
            thinking_budget=thinking_budget,
            timeout_s=timeout_s,
        )
        messages = _generate_messages(contents, system_instruction)
    responses_api = provider == "openai"
    call_kwargs = _generate_call_kwargs(
        provider,
        model,
        temperature=temperature,
        json_output=json_output,
        json_schema=json_schema,
        thinking_budget=thinking_budget,
        responses_api=responses_api,
    )
    response = (
        await llm.aresponses(messages, **call_kwargs)
        if responses_api
        else await llm.acompletion(messages, **call_kwargs)
    )
    return _generate_result(response, model)


def _build_local_condenser(llm: Any) -> Any:
    """LLM-summarizing condenser for the bundled-local floor window.

    Reuses the agent's own LLM (shared usage_id is accepted in
    openhands-sdk 1.27.1) so there is no separate summarization endpoint.
    """
    from openhands.sdk.context.condenser import LLMSummarizingCondenser

    return LLMSummarizingCondenser(
        llm=llm,
        max_tokens=_LOCAL_CONDENSER_MAX_TOKENS,
        keep_first=_LOCAL_CONDENSER_KEEP_FIRST,
    )


def _build_cogitate_agent(
    *,
    llm: Any,
    is_bundled_local: bool,
    tool_specs: list[Any],
    include_default_tools: list[Any],
    system_prompt: str,
) -> Any:
    from openhands.sdk import Agent

    condenser = _build_local_condenser(llm) if is_bundled_local else None
    return Agent(
        llm=llm,
        tools=tool_specs,
        include_default_tools=include_default_tools,
        system_prompt=system_prompt,
        condenser=condenser,
    )


# Lazy cache for the openhands-derived Sol* classes. The classes have to
# live at module level (i.e. without `<locals>` in their __qualname__ and
# discoverable as attributes on this module) — openhands-sdk persists tool
# events to disk and re-validates them via `Event.model_validate_json`,
# which walks `Action.__subclasses__()` and rejects any subclass whose
# qualname contains "<locals>" with "Local classes not supported". A
# `_build_sol_tools()` that defined the classes inline poisoned the entire
# Action subclass pool and crashed the stuck_detector's event re-read.
# We can't define the classes at literal module level because openhands-sdk
# is installed on demand and may not be importable at import time; instead
# we define them inside `_ensure_sol_types()` on first use and promote them
# into the module namespace.
_SOL_TYPES: dict[str, Any] = {}


def _ensure_sol_types() -> dict[str, Any]:
    if _SOL_TYPES:
        return _SOL_TYPES

    from openhands.sdk.tool import ToolAnnotations, ToolDefinition, ToolExecutor
    from openhands.sdk.tool.schema import Action, Observation
    from pydantic import Field

    class SolAction(Action):
        command: str = Field(
            description=(
                "Single `sol` or approved `journal` command-line invocation to "
                "run directly, without a shell."
            )
        )

    class SolObservation(Observation):
        pass

    class SolExecutor(ToolExecutor):
        def __init__(
            self,
            *,
            policy: CogitatePolicy,
            callback: JSONEventCallback,
            read_call_budget: int,
            slot_lease: LocalSlotLease | None = None,
        ) -> None:
            self.policy = policy
            self.callback = callback
            self.read_call_budget = read_call_budget
            self.slot_lease = slot_lease
            self.read_call_count = 0
            self._budget_exhausted_emitted = False
            self._conversation: Any | None = None
            self._terminal_error: Exception | None = None
            self._terminal_error_lock = threading.Lock()
            self._slot_cycle_lock = threading.Lock()

        def bind_conversation(self, conversation: Any) -> None:
            self._conversation = conversation

        def take_terminal_error(self) -> Exception | None:
            with self._terminal_error_lock:
                error = self._terminal_error
                self._terminal_error = None
                return error

        def interrupt(self) -> None:
            if self.slot_lease is not None:
                self.slot_lease.cancel_pending_reacquire()

        def __call__(self, action: Any, conversation: Any = None) -> Any:
            command = str(action.command)
            decision = self.policy.classify_command(command)
            if not decision.allowed:
                return SolObservation.from_text(decision.reason, is_error=True)

            self.read_call_count += 1
            if self.read_call_count > self.read_call_budget:
                if not self._budget_exhausted_emitted:
                    self.callback.emit(
                        {
                            "event": "tool_budget_exhausted",
                            "tool": "sol",
                            "budget": self.read_call_budget,
                            "count": self.read_call_count,
                            "ts": now_ms(),
                        }
                    )
                    self._budget_exhausted_emitted = True
                return SolObservation.from_text(
                    "tool_budget_exhausted: read-call budget exceeded",
                    is_error=True,
                )

            assert decision.argv is not None
            if self.slot_lease is None:
                result = _run_command(decision.argv)
                return SolObservation.from_text(
                    result["text"], is_error=result["is_error"]
                )

            with self._slot_cycle_lock:
                self.slot_lease.yield_slot()
                result: dict[str, Any] | None = None
                command_error: Exception | None = None
                try:
                    result = _run_command(decision.argv)
                except Exception as exc:
                    command_error = exc
                try:
                    self.slot_lease.reacquire()
                except LocalAdmissionCancelled:
                    if result is not None:
                        return SolObservation.from_text(
                            result["text"], is_error=result["is_error"]
                        )
                    return SolObservation.from_text(
                        "local_admission_cancelled: cogitate run interrupted "
                        "before reacquiring local inference",
                        is_error=True,
                    )
                except Exception as exc:
                    self._store_terminal_error(exc)
                    live_conversation = conversation or self._conversation
                    if live_conversation is not None:
                        live_conversation.interrupt()
                    return SolObservation.from_text(str(exc), is_error=True)
                if command_error is not None:
                    raise command_error
                assert result is not None
            return SolObservation.from_text(result["text"], is_error=result["is_error"])

        def _store_terminal_error(self, error: Exception) -> None:
            with self._terminal_error_lock:
                self._terminal_error = error

    class SolTool(ToolDefinition[SolAction, SolObservation]):
        name = "sol"

        @classmethod
        def create(cls, *args: Any, **kwargs: Any) -> list[Any]:
            del args, kwargs
            return []

    # Promote the closure-defined classes onto this module so they look
    # module-level to openhands-sdk's serialization machinery. Without
    # this, `__qualname__` carries `<locals>` and re-deserializing tool
    # events fails inside stuck_detector with
    # "Local classes not supported".
    module = sys.modules[__name__]
    for cls in (SolAction, SolObservation, SolExecutor, SolTool):
        cls.__module__ = __name__
        cls.__qualname__ = cls.__name__
        setattr(module, cls.__name__, cls)

    _SOL_TYPES.update(
        SolAction=SolAction,
        SolObservation=SolObservation,
        SolExecutor=SolExecutor,
        SolTool=SolTool,
        ToolAnnotations=ToolAnnotations,
    )
    return _SOL_TYPES


def _build_sol_tools(
    *,
    policy: CogitatePolicy,
    callback: JSONEventCallback,
    read_call_budget: int,
    slot_lease: LocalSlotLease | None = None,
) -> tuple[list[Any], Any]:
    types = _ensure_sol_types()
    sol_action = types["SolAction"]
    sol_observation = types["SolObservation"]
    sol_executor_cls = types["SolExecutor"]
    sol_tool_cls = types["SolTool"]
    tool_annotations = types["ToolAnnotations"]

    executor = sol_executor_cls(
        policy=policy,
        callback=callback,
        read_call_budget=read_call_budget,
        slot_lease=slot_lease,
    )
    tool = sol_tool_cls(
        description=(
            "Run one policy-approved `sol` or `journal` command-line invocation "
            "directly, without a shell."
        ),
        action_type=sol_action,
        observation_type=sol_observation,
        executor=executor,
        annotations=tool_annotations(
            title="sol",
            readOnlyHint=True,
            destructiveHint=False,
            idempotentHint=True,
            openWorldHint=False,
        ),
    )
    return [tool], executor


def _run_command(argv: list[str]) -> dict[str, Any]:
    import subprocess

    executable = Path(sys.executable).parent / argv[0]
    resolved = str(executable) if executable.exists() else shutil.which(argv[0])
    if not resolved:
        return {"text": f"command_not_found: {argv[0]}", "is_error": True}

    try:
        completed = subprocess.run(
            [resolved, *argv[1:]],
            text=True,
            capture_output=True,
            timeout=_SHELL_TIMEOUT_SECONDS,
            check=False,
        )
    except FileNotFoundError:
        return {"text": f"command_not_found: {argv[0]}", "is_error": True}
    except subprocess.TimeoutExpired as exc:
        output = exc.stdout or ""
        error = exc.stderr or ""
        text = _format_shell_output(
            stdout=str(output),
            stderr=str(error),
            returncode=None,
            timed_out=True,
        )
        return {"text": text, "is_error": True}
    except PermissionError as exc:
        return {"text": f"permission_denied: {exc}", "is_error": True}
    except OSError as exc:
        return {"text": str(exc), "is_error": True}

    text = _format_shell_output(
        stdout=completed.stdout or "",
        stderr=completed.stderr or "",
        returncode=completed.returncode,
        timed_out=False,
    )
    return {"text": text, "is_error": completed.returncode != 0}


def _format_shell_output(
    *,
    stdout: str,
    stderr: str,
    returncode: int | None,
    timed_out: bool,
) -> str:
    parts: list[str] = []
    if stdout:
        parts.append(f"stdout:\n{_truncate_output(stdout, _SHELL_STDOUT_CAP)}")
    if stderr:
        parts.append(f"stderr:\n{_truncate_output(stderr, _SHELL_STDERR_CAP)}")
    if timed_out:
        parts.append(f"timeout: command exceeded {_SHELL_TIMEOUT_SECONDS}s")
    elif returncode is not None and returncode != 0:
        parts.append(f"exit_code: {returncode}")
    if not parts:
        return "ok"
    return "\n\n".join(parts)


def _truncate_output(text: str, cap: int) -> str:
    if len(text) <= cap:
        return text
    return f"{text[:cap]}\n... [truncated]"


class _OpenHandsTranslator:
    def __init__(
        self,
        *,
        callback: JSONEventCallback,
        llm: Any,
        provider: str,
        model: str,
        cost_cap: float,
        max_turns: int = MAX_TURNS,
        expects_emit_final: bool = False,
    ) -> None:
        from openhands.sdk.event import (
            ActionEvent,
            AgentErrorEvent,
            MessageEvent,
            ObservationEvent,
        )
        from openhands.sdk.event.conversation_error import ConversationErrorEvent

        self.callback = callback
        self.llm = llm
        self.provider = provider
        self.model = model
        self.cost_cap = cost_cap
        self.max_turns = max_turns
        self.expects_emit_final = expects_emit_final
        self.conversation: Any = None
        self.ActionEvent = ActionEvent
        self.AgentErrorEvent = AgentErrorEvent
        self.ConversationErrorEvent = ConversationErrorEvent
        self.MessageEvent = MessageEvent
        self.ObservationEvent = ObservationEvent
        self.tool_calls: dict[str, dict[str, Any]] = {}
        self.emit_final_content: str | None = None
        self.finish_message: str | None = None
        self.final_message: str | None = None
        self.max_turns_exhausted = False
        self._wrapup_nudged = False
        self._final_turn_armed = False
        self._cost_force_stopped = False
        self._observed_turns: int = 0
        self._seen_response_ids: set[str] = set()
        self._turn_warnings_fired: set[float] = set()
        self._turn_final_armed: bool = False
        self._turn_force_stopped: bool = False

    def on_event(self, event: Any) -> None:
        if isinstance(event, self.ActionEvent):
            self._handle_action_event(event)
            return
        if isinstance(event, self.ObservationEvent):
            self._handle_observation_event(event)
            return
        if isinstance(event, self.MessageEvent):
            self._handle_message_event(event)
            return
        if isinstance(event, self.AgentErrorEvent):
            self._handle_agent_error_event(event)
            return
        if isinstance(event, self.ConversationErrorEvent):
            self._handle_conversation_error_event(event)

    def on_token(self, chunk: Any) -> None:
        delta = _extract_token_delta(chunk)
        if not delta:
            return
        self.callback.emit(
            {
                "event": "text_delta",
                "delta": delta,
                "model": self.model,
                "ts": now_ms(),
            }
        )

    def _handle_action_event(self, event: Any) -> None:
        raw = _raw_event(event)
        self._emit_reasoning(event, raw)

        tool_name = str(getattr(event, "tool_name", "") or "")
        if not tool_name:
            return

        args = _tool_arguments(event)
        call_id = str(getattr(event, "tool_call_id", "") or "")
        if _is_emit_final_action(tool_name, event, args):
            self.emit_final_content = _emit_final_content(event, args)
            return
        if _is_finish_action(tool_name, event, args):
            self.finish_message = _finish_message(event, args)
            return

        self._check_resource_ceiling()
        response_id = str(getattr(event, "llm_response_id", "") or "")
        self._check_turn_budget(response_id)
        self.tool_calls[call_id] = {"tool": tool_name, "args": args}
        self.callback.emit(
            {
                "event": "tool_start",
                "tool": tool_name,
                "args": args,
                "call_id": call_id,
                "raw": raw,
                "ts": now_ms(),
            }
        )

    def _finish_tool_name(self) -> str:
        return "emit_final" if self.expects_emit_final else "finish"

    def _run_cost(self) -> float:
        metrics = getattr(self.llm, "metrics", None)
        cost = float(getattr(metrics, "accumulated_cost", 0.0) or 0.0)
        if cost > 0.0:
            return cost
        usage = getattr(metrics, "accumulated_token_usage", None)
        if usage is None:
            return 0.0
        prompt = int(getattr(usage, "prompt_tokens", 0) or 0)
        cache_read = int(getattr(usage, "cache_read_tokens", 0) or 0)
        completion = int(getattr(usage, "completion_tokens", 0) or 0)
        fresh = max(0, prompt - cache_read) + completion
        return fresh * _FALLBACK_USD_PER_TOKEN

    def _context_fraction(self) -> float | None:
        window = getattr(self.llm, "effective_max_input_tokens", None)
        if not window or window <= 0:
            return None
        metrics = getattr(self.llm, "metrics", None)
        usage = getattr(metrics, "accumulated_token_usage", None)
        per_turn = int(getattr(usage, "per_turn_token", 0) or 0)
        return per_turn / window

    def _check_resource_ceiling(self) -> None:
        if self.conversation is None or self._cost_force_stopped:
            return

        # Stage 3: the armed last turn did not finish -> hard backstop.
        if self._final_turn_armed:
            self.conversation.pause()
            self._cost_force_stopped = True
            return

        cost = self._run_cost()
        context_frac = self._context_fraction()
        finish_tool = self._finish_tool_name()

        # Stage 2: at the cap -> arm exactly one more turn.
        if cost >= self.cost_cap or (
            context_frac is not None and context_frac >= CONTEXT_FINAL_FRAC
        ):
            self.conversation.send_message(
                f"Resource budget reached: this is the final turn. Stop gathering "
                f"more context or using tools, and call {finish_tool} now with the "
                f"best result available."
            )
            self._final_turn_armed = True
            self._wrapup_nudged = True
            return

        # Stage 1: approaching the cap -> one wrap-up nudge.
        if not self._wrapup_nudged and (
            cost >= COST_WARN_FRAC * self.cost_cap
            or (context_frac is not None and context_frac >= CONTEXT_WARN_FRAC)
        ):
            self.conversation.send_message(
                f"Resource budget warning: this run is approaching its per-run "
                f"resource budget. Finish useful work now and call {finish_tool} "
                f"with the best complete result you can produce."
            )
            self._wrapup_nudged = True

    def _check_turn_budget(self, response_id: str) -> None:
        if self.conversation is None or self._turn_force_stopped:
            return

        # A parallel/duplicate action from an already-counted response is the
        # same turn; dedupe before the armed check so an arming turn cannot
        # immediately force-stop itself.
        if response_id and response_id in self._seen_response_ids:
            return

        # Stage 3: a new non-final turn after the ultimatum -> hard backstop.
        if self._turn_final_armed:
            self.conversation.pause()
            self._turn_force_stopped = True
            self.max_turns_exhausted = True
            return

        if response_id:
            self._seen_response_ids.add(response_id)
        self._observed_turns += 1

        used = self._observed_turns
        limit = self.max_turns
        remaining = limit - used
        finish_tool = self._finish_tool_name()

        # Stage 2: one or fewer turns remains; threshold warnings collapse here.
        if used >= limit - 1:
            self.conversation.send_message(
                f"Turn budget reached: this is your last turn. Stop gathering more "
                f"context or using tools, and call {finish_tool} now with the best "
                f"result available."
            )
            self._turn_final_armed = True
            return

        # Stage 1: threshold warnings, each latched once.
        for frac in TURN_WARN_FRACS:
            if frac not in self._turn_warnings_fired and used >= math.ceil(
                frac * limit
            ):
                percent = int(frac * 100)
                if percent == 50:
                    instruction = (
                        "Start converging on the final result and call "
                        f"{finish_tool} as soon as useful work is complete."
                    )
                elif percent == 75:
                    instruction = (
                        "Stop broad gathering; use the remaining turns only for "
                        f"synthesis and final checks, then call {finish_tool}."
                    )
                else:
                    instruction = (
                        "Finish now unless one more tool call is essential; call "
                        f"{finish_tool} with the best complete result available."
                    )
                self.conversation.send_message(
                    f"Turn budget warning: you've used {percent}% of your turn "
                    f"budget so far: {used} of {limit} turns, {remaining} turns "
                    f"left. {instruction}"
                )
                self._turn_warnings_fired.add(frac)

    def _emit_reasoning(self, event: Any, raw: list[dict[str, Any]]) -> None:
        reasoning_content = getattr(event, "reasoning_content", None)
        if isinstance(reasoning_content, str) and reasoning_content.strip():
            self._emit_thinking(reasoning_content.strip(), raw=raw)

        for block in getattr(event, "thinking_blocks", []) or []:
            summary = _text_from_attr(block, "thinking")
            signature = _text_from_attr(block, "signature") or None
            redacted_data = _text_from_attr(block, "data") or None
            if summary or redacted_data or signature:
                self._emit_thinking(
                    summary,
                    signature=signature,
                    redacted_data=redacted_data,
                    raw=raw,
                )

        item = getattr(event, "responses_reasoning_item", None)
        if item is not None:
            summary = _reasoning_item_summary(item)
            redacted_data = _text_from_attr(item, "encrypted_content") or None
            if summary or redacted_data:
                self._emit_thinking(
                    summary,
                    redacted_data=redacted_data,
                    raw=raw,
                )

    def _emit_thinking(
        self,
        summary: str,
        *,
        signature: str | None = None,
        redacted_data: str | None = None,
        raw: list[dict[str, Any]] | None = None,
    ) -> None:
        event: dict[str, Any] = {
            "event": "thinking",
            "summary": summary,
            "model": self.model,
            "signature": signature,
            "redacted_data": redacted_data,
            "ts": now_ms(),
        }
        if raw is not None:
            event["raw"] = raw
        self.callback.emit(event)

    def _handle_observation_event(self, event: Any) -> None:
        call_id = str(getattr(event, "tool_call_id", "") or "")
        paired = self.tool_calls.pop(call_id, {})
        tool_name = paired.get("tool") or str(getattr(event, "tool_name", "") or "")
        args = paired.get("args")
        self.callback.emit(
            {
                "event": "tool_end",
                "tool": tool_name,
                "args": args,
                "result": _observation_text(getattr(event, "observation", None)),
                "call_id": call_id,
                "raw": _raw_event(event),
                "ts": now_ms(),
            }
        )

    def _handle_message_event(self, event: Any) -> None:
        source = getattr(event, "source", None)
        text = _message_event_text(event)
        if source == "agent" and text:
            self.final_message = text

    def _handle_agent_error_event(self, event: Any) -> None:
        message = str(getattr(event, "error", "") or "")
        self.callback.emit(
            {
                "event": "error",
                "error": message,
                "reason_code": classify_provider_error(
                    RuntimeError(message),
                    self.provider,
                ),
                "provider": self.provider,
                "trace": "",
                "raw": _raw_event(event),
                "terminal": False,
                "ts": now_ms(),
            }
        )

    def _handle_conversation_error_event(self, event: Any) -> None:
        if getattr(event, "code", None) != "MaxIterationsReached":
            return
        self.max_turns_exhausted = True

    def result(self) -> str | None:
        if self.expects_emit_final:
            return self.emit_final_content
        return self.finish_message or self.final_message


def _raw_event(event: Any) -> list[dict[str, Any]]:
    if hasattr(event, "model_dump"):
        try:
            return safe_raw([event.model_dump(mode="json")])
        except Exception:
            pass
    return safe_raw([{"type": event.__class__.__name__, "repr": repr(event)}])


def _tool_arguments(event: Any) -> dict[str, Any]:
    tool_call = getattr(event, "tool_call", None)
    raw_arguments = getattr(tool_call, "arguments", None)
    if isinstance(raw_arguments, dict):
        return dict(raw_arguments)
    if isinstance(raw_arguments, str):
        try:
            value = json.loads(raw_arguments)
        except json.JSONDecodeError:
            return {"raw_arguments": raw_arguments}
        return value if isinstance(value, dict) else {"raw_arguments": raw_arguments}

    action = getattr(event, "action", None)
    if hasattr(action, "model_dump"):
        try:
            return action.model_dump(mode="json")
        except Exception:
            pass
    return {}


def _is_finish_action(tool_name: str, event: Any, args: dict[str, Any]) -> bool:
    if tool_name == "finish":
        return True
    action = getattr(event, "action", None)
    if action is not None and action.__class__.__name__ == "FinishAction":
        return True
    return "message" in args and tool_name.endswith("finish")


def _is_emit_final_action(tool_name: str, event: Any, args: dict[str, Any]) -> bool:
    if tool_name == "emit_final":
        return True
    action = getattr(event, "action", None)
    if action is not None and action.__class__.__name__ == "EmitFinalAction":
        return True
    return "content" in args and tool_name.endswith("emit_final")


def _finish_message(event: Any, args: dict[str, Any]) -> str:
    action = getattr(event, "action", None)
    message = getattr(action, "message", None)
    if isinstance(message, str):
        return message
    value = args.get("message")
    return value if isinstance(value, str) else ""


def _emit_final_content(event: Any, args: dict[str, Any]) -> str:
    action = getattr(event, "action", None)
    content = getattr(action, "content", None)
    if isinstance(content, str):
        return content
    value = args.get("content")
    return value if isinstance(value, str) else ""


def _text_from_attr(value: Any, attr: str) -> str:
    text = getattr(value, attr, None)
    return text if isinstance(text, str) else ""


def _reasoning_item_summary(item: Any) -> str:
    summary = getattr(item, "summary", None)
    if isinstance(summary, str):
        return summary
    if isinstance(summary, list):
        parts: list[str] = []
        for entry in summary:
            if isinstance(entry, str):
                parts.append(entry)
                continue
            text = getattr(entry, "text", None) or getattr(entry, "summary", None)
            if isinstance(text, str):
                parts.append(text)
        return "\n".join(part for part in parts if part)
    content = getattr(item, "content", None)
    return content if isinstance(content, str) else ""


def _observation_text(observation: Any) -> str:
    text = getattr(observation, "text", None)
    if isinstance(text, str):
        return text
    content = getattr(observation, "content", None)
    if isinstance(content, list):
        return "".join(_content_text(item) for item in content)
    return "" if observation is None else str(observation)


def _message_event_text(event: Any) -> str:
    message = getattr(event, "llm_message", None)
    content = getattr(message, "content", None)
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "".join(_content_text(item) for item in content)
    return ""


def _content_text(item: Any) -> str:
    if isinstance(item, str):
        return item
    text = getattr(item, "text", None)
    return text if isinstance(text, str) else ""


def _extract_token_delta(chunk: Any) -> str:
    choices = _get_value(chunk, "choices")
    if not choices:
        return ""
    choice = choices[0]
    delta = _get_value(choice, "delta")
    content = _get_value(delta, "content")
    return content if isinstance(content, str) else ""


def _get_value(value: Any, key: str) -> Any:
    if isinstance(value, dict):
        return value.get(key)
    return getattr(value, key, None)


def _usage_snapshot(llm: Any) -> dict[str, int]:
    metrics = getattr(llm, "metrics", None)
    usage = getattr(metrics, "accumulated_token_usage", None)
    token_usages = getattr(metrics, "token_usages", None) or []
    return {
        "input_tokens": int(getattr(usage, "prompt_tokens", 0) or 0),
        "output_tokens": int(getattr(usage, "completion_tokens", 0) or 0),
        "cached_tokens": int(getattr(usage, "cache_read_tokens", 0) or 0),
        "cache_creation_tokens": int(getattr(usage, "cache_write_tokens", 0) or 0),
        "reasoning_tokens": int(getattr(usage, "reasoning_tokens", 0) or 0),
        "requests": len(token_usages),
    }


def _usage_delta(start: dict[str, int], llm: Any) -> dict[str, int]:
    end = _usage_snapshot(llm)
    usage = {
        key: max(0, end.get(key, 0) - start.get(key, 0))
        for key in (
            "input_tokens",
            "output_tokens",
            "cached_tokens",
            "cache_creation_tokens",
            "reasoning_tokens",
            "requests",
        )
    }
    usage["total_tokens"] = usage["input_tokens"] + usage["output_tokens"]
    return {key: value for key, value in usage.items() if key in USAGE_KEYS}


def _unwrap_provider_exception(exc: BaseException) -> BaseException:
    cause = exc.__cause__
    if cause is not None:
        return cause
    context = exc.__context__
    return context if context is not None else exc


def _retry_delay_ms(exc: BaseException) -> int | None:
    response = getattr(exc, "response", None)
    headers = getattr(response, "headers", None)
    if not headers:
        return None
    retry_after = headers.get("retry-after") or headers.get("Retry-After")
    if retry_after is None:
        return None

    try:
        return int(float(str(retry_after).strip()) * 1000)
    except ValueError:
        pass

    try:
        retry_at = parsedate_to_datetime(str(retry_after))
    except (TypeError, ValueError):
        return None
    if retry_at.tzinfo is None:
        retry_at = retry_at.replace(tzinfo=timezone.utc)
    delay = retry_at - datetime.now(timezone.utc)
    return max(0, int(delay.total_seconds() * 1000))


@contextmanager
def _suppress_litellm_cost_warnings() -> Any:
    class _CostWarningFilter(logging.Filter):
        def filter(self, record: logging.LogRecord) -> bool:
            return _COST_WARNING_TEXT not in record.getMessage()

    loggers = [
        logging.getLogger("litellm"),
        logging.getLogger("LiteLLM"),
    ]
    filters: list[tuple[logging.Logger, logging.Filter]] = []
    try:
        for logger in loggers:
            warning_filter = _CostWarningFilter()
            logger.addFilter(warning_filter)
            filters.append((logger, warning_filter))
        yield
    finally:
        for logger, warning_filter in filters:
            logger.removeFilter(warning_filter)


def _conversation_execution_status(conversation: Any) -> str | None:
    try:
        state = conversation.state
    except AttributeError:
        return None
    if state is None:
        return None
    try:
        status = state.execution_status
    except AttributeError:
        return None
    if status is None:
        return None
    try:
        value = status.value
    except AttributeError:
        value = status
    return value if isinstance(value, str) else None


# Bound per-call LLM time and retries explicitly. The SDK defaults
# (num_retries=5, timeout=300s) can stack to ~25-30 min of retry churn on a
# single bad call. NOTE: LLM.timeout is forwarded to
# litellm_completion(timeout=...) but does NOT reliably bound a mid-stream idle
# gap on streaming cogitate calls — the asyncio wall-clock wrap in
# run_cogitate() remains the real backstop for that class of stall.
LLM_TIMEOUT_S = 300
LLM_NUM_RETRIES = 2

# Seconds subtracted from a talent's timeout_seconds to derive the in-process
# wall-clock deadline, so the in-process force-finish completes well before
# Cortex's process-kill Timer (cortex.py:355-365), which fires at
# timeout_seconds then SIGTERMs and waits 10s.
WALL_CLOCK_GRACE_S = 30.0


def _wall_clock_deadline_s(timeout_seconds: float) -> float:
    """In-process wall-clock deadline, strictly inside ``timeout_seconds``.

    The deadline is ``timeout_seconds - WALL_CLOCK_GRACE_S``. When that is
    non-positive (a talent configured a ``timeout_seconds`` at or below the
    grace), fall back to half the talent budget so the deadline is always
    positive and strictly less than ``timeout_seconds``.
    """
    deadline = timeout_seconds - WALL_CLOCK_GRACE_S
    if deadline <= 0:
        deadline = timeout_seconds / 2
    return deadline


async def run_cogitate(
    config: dict[str, Any],
    on_event: Callable[[dict], None] | None = None,
    *,
    slot_lease: LocalSlotLease | None = None,
) -> str | None:
    """Run a cogitate prompt through OpenHands SDK."""
    callback = JSONEventCallback(on_event)
    provider = str(config["provider"])
    model = str(config["model"])

    llm: Any | None = None
    usage_start: dict[str, int] | None = None
    try:
        with _openhands_import_policy():
            from openhands.sdk import Conversation
            from openhands.sdk.tool.registry import register_tool
            from openhands.sdk.tool.spec import Tool

        wants_emit_final = expects_emit_final(config)
        max_turns = int(config.get("max_turns", MAX_TURNS) or MAX_TURNS)
        cost_cap = float(
            config.get("max_run_cost_usd", DEFAULT_RUN_COST_CAP_USD)
            or DEFAULT_RUN_COST_CAP_USD
        )
        session_id, conversation_id = _session_identity(config.get("session_id"))
        prompt_body, system_instruction = assemble_prompt(
            config,
            sol_tool_name="sol",
        )
        allowed_roots = _resolve_allowed_roots(config)
        access_tier = str(config.get("access_tier", "normal"))
        outbound_approval = config.get("outbound_approval")
        caps = capabilities_for_access_tier(access_tier)
        policy = CogitatePolicy(
            allowed_roots=allowed_roots,
            access_tier=access_tier,
            outbound_approval=outbound_approval,
        )
        read_call_budget = int(
            config.get("read_call_budget", DEFAULT_READ_CALL_BUDGET) or 0
        )
        journal = Path(get_journal())
        llm = _build_llm(provider, model)
        usage_start = _usage_snapshot(llm)
        tool_specs = []
        sol_executor = None
        if caps.sol:
            sol_tools, sol_executor = _build_sol_tools(
                policy=policy,
                callback=callback,
                read_call_budget=read_call_budget,
                slot_lease=slot_lease,
            )
            # openhands-sdk v1.23 resolves Agent.tools by spec name via the
            # registry; passing ToolDefinition instances directly fails pydantic
            # validation. Re-register the per-run SolTool instance (its executor
            # closure captures this run's policy / callback / budget) and
            # reference it by name.
            register_tool("sol", sol_tools[0])
            tool_specs.append(Tool(name="sol"))
        from .read_tools import build_read_tools

        if caps.reads:
            read_tools = build_read_tools(
                journal=journal,
                read_call_budget=read_call_budget,
            )
            for read_tool in read_tools:
                register_tool(read_tool.name, read_tool)
                tool_specs.append(Tool(name=read_tool.name))
        default_tools = ["FinishTool"]
        if wants_emit_final:
            from .emit_final_tool import build_emit_final_tools

            emit_final_tools = build_emit_final_tools()
            register_tool("emit_final", emit_final_tools[0])
            tool_specs.append(Tool(name="emit_final"))
            default_tools = []

        from solstone.think.providers.local_endpoint import resolve_local_endpoint

        is_bundled_local = provider == "local" and resolve_local_endpoint().is_bundled
        agent = _build_cogitate_agent(
            llm=llm,
            is_bundled_local=is_bundled_local,
            tool_specs=tool_specs,
            include_default_tools=default_tools,
            system_prompt=system_instruction,
        )

        persistence_dir = journal / ".cache" / "cogitate-history" / session_id
        persistence_dir.mkdir(parents=True, exist_ok=True)
        translator = _OpenHandsTranslator(
            callback=callback,
            llm=llm,
            provider=provider,
            model=_prefixed_model(provider, model),
            cost_cap=cost_cap,
            max_turns=max_turns,
            expects_emit_final=wants_emit_final,
        )
        conversation = Conversation(
            agent=agent,
            workspace=str(get_project_root()),
            persistence_dir=str(persistence_dir),
            conversation_id=conversation_id,
            callbacks=[translator.on_event],
            token_callbacks=[translator.on_token],
            max_iteration_per_run=max_turns + MAX_TURNS_HEADROOM,
            stuck_detection=True,
            visualizer=None,
        )
        translator.conversation = conversation
        if sol_executor is not None:
            sol_executor.bind_conversation(conversation)
        conversation.send_message(prompt_body)
        timeout_seconds = float(config.get("timeout_seconds", 600) or 600)
        wall_clock_s = _wall_clock_deadline_s(timeout_seconds)
        wall_clock_exceeded = False
        with _suppress_litellm_cost_warnings():
            run_task = asyncio.ensure_future(conversation.arun())
            _done, pending = await asyncio.wait({run_task}, timeout=wall_clock_s)
            if run_task in pending:
                wall_clock_exceeded = True
                run_task.cancel()
                try:
                    await run_task
                except asyncio.CancelledError:
                    pass
                except Exception:
                    LOG.exception(
                        "cogitate arun raised while force-finishing on the "
                        "wall-clock deadline"
                    )
            else:
                # arun completed (or raised) within the deadline. asyncio.wait
                # captures any exception on the task rather than propagating it,
                # so re-raise here to keep the existing QuotaExhaustedError /
                # generic except-Exception classification path unchanged.
                run_task.result()

        if sol_executor is not None:
            terminal_error = sol_executor.take_terminal_error()
            if terminal_error is not None:
                conversation.close()
                raise terminal_error

        result = translator.result()
        usage = _usage_delta(usage_start, llm)
        if wall_clock_exceeded:
            has_partial = bool(result and result.strip())
            error_text = (
                "wall_clock_exceeded: cogitate run exceeded its wall-clock "
                "deadline and was force-finished with a partial result preserved"
                if has_partial
                else "wall_clock_exceeded: cogitate run exceeded its wall-clock "
                "deadline and was force-finished before emitting a final result"
            )
            conversation.close()
            callback.emit(
                {
                    "event": "error",
                    "error": error_text,
                    "reason_code": "wall_clock_exceeded",
                    "provider": provider,
                    "result": result,
                    "usage": usage,
                    "terminal": True,
                    "cli_session_id": str(conversation_id),
                    "ts": now_ms(),
                }
            )
            return result
        if translator._cost_force_stopped or translator.max_turns_exhausted:
            reason_code = (
                "token_budget_exceeded"
                if translator._cost_force_stopped
                else "max_turns_exhausted"
            )
            has_partial = bool(result and result.strip())
            if reason_code == "token_budget_exceeded":
                error_text = (
                    "token_budget_exceeded: cogitate run reached its per-run "
                    "resource budget and was force-finished with a partial result "
                    "preserved"
                    if has_partial
                    else "token_budget_exceeded: cogitate run reached its per-run "
                    "resource budget and was force-finished before emitting a final "
                    "result"
                )
            else:
                error_text = (
                    "max_turns_exhausted: cogitate run reached its turn budget and "
                    "was force-finished with a partial result preserved"
                    if has_partial
                    else "max_turns_exhausted: cogitate run reached its turn budget "
                    "and was force-finished before emitting a final result"
                )
            conversation.close()
            callback.emit(
                {
                    "event": "error",
                    "error": error_text,
                    "reason_code": reason_code,
                    "provider": provider,
                    "result": result,
                    "usage": usage,
                    "terminal": True,
                    "cli_session_id": str(conversation_id),
                    "ts": now_ms(),
                }
            )
            return result
        execution_status = _conversation_execution_status(conversation)
        if execution_status in {"stuck", "paused"}:
            has_partial = bool(result and result.strip())
            error_text = (
                "agent_stuck: cogitate run was interrupted/stuck with a partial "
                "result preserved"
                if has_partial
                else "agent_stuck: cogitate run was interrupted/stuck before "
                "emitting a final result"
            )
            conversation.close()
            callback.emit(
                {
                    "event": "error",
                    "error": error_text,
                    "reason_code": "agent_stuck",
                    "provider": provider,
                    "result": result,
                    "usage": usage,
                    "terminal": True,
                    "cli_session_id": str(conversation_id),
                    "ts": now_ms(),
                }
            )
            return result
        if wants_emit_final and not (result and result.strip()):
            callback.emit(
                {
                    "event": "error",
                    "error": (
                        "no_output: expects-final cogitate run finished without "
                        "emitting a final result"
                    ),
                    "reason_code": "no_output",
                    "provider": provider,
                    "terminal": True,
                    "ts": now_ms(),
                }
            )
            return None
        callback.emit(
            {
                "event": "finish",
                "result": result,
                "usage": usage,
                "cli_session_id": str(conversation_id),
                "ts": now_ms(),
            }
        )
        return result
    except QuotaExhaustedError:
        raise
    except Exception as exc:
        from solstone.think.talents import TalentHookError

        if isinstance(exc, TalentHookError):
            raise

        provider_exc = _unwrap_provider_exception(exc)
        reason_code = None
        local_endpoint = None
        if provider == "local":
            from solstone.think.providers.local_endpoint import (
                classify_byo_cogitate_error,
                local_endpoint_reason_copy,
                redact_local_endpoint_credential,
                resolve_local_endpoint,
            )

            local_endpoint = resolve_local_endpoint()
            if not local_endpoint.is_bundled:
                reason_code = classify_byo_cogitate_error(provider_exc)
                if reason_code:
                    setattr(exc, "reason_code", reason_code)
                    setattr(provider_exc, "reason_code", reason_code)
        reason_code = reason_code or classify_provider_error(provider_exc, provider)
        error_text = str(exc)
        trace_text = traceback.format_exc()
        if local_endpoint is not None:
            fixed_copy = local_endpoint_reason_copy(reason_code)
            if fixed_copy:
                error_text = fixed_copy
            if not local_endpoint.is_bundled:
                error_text = redact_local_endpoint_credential(
                    error_text, local_endpoint
                )
                trace_text = redact_local_endpoint_credential(
                    trace_text, local_endpoint
                )
        if reason_code == "provider_quota_exceeded":
            raise QuotaExhaustedError(
                str(provider_exc), _retry_delay_ms(provider_exc)
            ) from exc
        error_event = {
            "event": "error",
            "error": error_text,
            "reason_code": reason_code,
            "provider": provider,
            "trace": trace_text,
        }
        if usage_start is not None and llm is not None:
            error_event["usage"] = _usage_delta(usage_start, llm)
        error_event["ts"] = now_ms()
        callback.emit(error_event)
        setattr(exc, "_evented", True)
        raise


def run_generate(
    contents: Any,
    model: str,
    *,
    provider: str,
    temperature: float = 0.3,
    max_output_tokens: int = 8192 * 2,
    system_instruction: str | None = None,
    json_output: bool = False,
    thinking_budget: int | None = None,
    json_schema: dict | None = None,
    timeout_s: float = 120,
    **kwargs: Any,
) -> GenerateResult:
    if kwargs:
        unknown = ", ".join(sorted(kwargs))
        raise TypeError(f"Unsupported generate options: {unknown}")
    return _run_generate(
        contents,
        model,
        provider=provider,
        temperature=temperature,
        max_output_tokens=max_output_tokens,
        system_instruction=system_instruction,
        json_output=json_output,
        thinking_budget=thinking_budget,
        json_schema=json_schema,
        timeout_s=timeout_s,
    )


async def run_agenerate(
    contents: Any,
    model: str,
    *,
    provider: str,
    temperature: float = 0.3,
    max_output_tokens: int = 8192 * 2,
    system_instruction: str | None = None,
    json_output: bool = False,
    thinking_budget: int | None = None,
    json_schema: dict | None = None,
    timeout_s: float = 120,
    **kwargs: Any,
) -> GenerateResult:
    if kwargs:
        unknown = ", ".join(sorted(kwargs))
        raise TypeError(f"Unsupported generate options: {unknown}")
    return await _run_agenerate(
        contents,
        model,
        provider=provider,
        temperature=temperature,
        max_output_tokens=max_output_tokens,
        system_instruction=system_instruction,
        json_output=json_output,
        thinking_budget=thinking_budget,
        json_schema=json_schema,
        timeout_s=timeout_s,
    )


def _exception_chain(exc: BaseException) -> list[BaseException]:
    chain: list[BaseException] = []
    current: BaseException | None = exc
    while current is not None and current not in chain:
        chain.append(current)
        current = current.__cause__ or current.__context__
    return chain


def _model_not_found(exc: BaseException) -> bool:
    for item in _exception_chain(exc):
        status = getattr(item, "status_code", None)
        if status is None:
            status = getattr(getattr(item, "response", None), "status_code", None)
        if status == 404 or "notfound" in type(item).__name__.lower():
            return True
    return False


def _validation_reason(exc: BaseException, provider: str) -> str:
    if _model_not_found(exc):
        return "model_not_found"
    for item in _exception_chain(exc):
        reason = classify_provider_error(item, provider)
        if reason != "unknown":
            return reason
    return "unknown"


def _probe(provider: str, model: str, api_key: str) -> None:
    _run_generate(
        "Reply OK",
        model,
        provider=provider,
        temperature=None,
        max_output_tokens=8,
        system_instruction=None,
        json_output=False,
        thinking_budget=None,
        json_schema=None,
        timeout_s=10,
        api_key=api_key,
        num_retries=0,
    )


def validate_key(provider: str, api_key: str) -> dict:
    """Verify a personal cloud key through the same transport used at runtime."""
    from solstone.think.models import default_model_for_provider

    try:
        _probe(provider, default_model_for_provider(provider), api_key)
        return {"valid": True}
    except Exception as exc:
        reason = _validation_reason(exc, provider)
        # A 404 or quota response proves the endpoint accepted the credential;
        # model selection performs the definitive, model-specific probe next.
        if reason in {"model_not_found", "provider_quota_exceeded"}:
            return {"valid": True, "probe_reason_code": reason}
        return {"valid": False, "error": str(exc), "reason_code": reason}


def validate_model(provider: str, model: str, api_key: str) -> dict:
    """Verify that a personal cloud key can actually run the selected model."""
    try:
        _probe(provider, model, api_key)
        return {"valid": True}
    except Exception as exc:
        return {
            "valid": False,
            "error": str(exc),
            "reason_code": _validation_reason(exc, provider),
        }
