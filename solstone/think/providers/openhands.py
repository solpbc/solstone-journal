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
import tempfile
import threading
import traceback
import uuid
from collections.abc import Callable, Iterator
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass
from datetime import datetime, timezone
from email.utils import parsedate_to_datetime
from pathlib import Path
from typing import Any

from solstone.log_policy import apply_http_logging_policy, snapshot_root_logging
from solstone.think.cogitate_contract import (
    capabilities_for_access_tier,
    cogitate_journal_command_list,
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
    CANNED_GENERATE_MAX_OUTPUT_TOKENS,
    CANNED_GENERATE_NUM_RETRIES,
    CANNED_GENERATE_PROMPT,
    CANNED_GENERATE_THINKING_BUDGET,
    CANNED_GENERATE_TIMEOUT_S,
    PROVIDER_ERROR_TEXT_CAP_CHARS,
    USAGE_KEYS,
    JSONEventCallback,
    classify_provider_error,
    safe_raw,
)
from solstone.think.responsiveness import (
    NON_RESPONSIVE_OUTPUT_FRAGMENT,
    NON_RESPONSIVE_RAW_OUTPUT_CAP_CHARS,
    NON_RESPONSIVE_REASON_CODE,
    classify_output_responsiveness,
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
_OPENHANDS_CONVERSATION_LOGGER = "openhands.sdk.conversation.impl.local_conversation"
_OPENHANDS_MAX_ITERATIONS_PREFIX = "Agent reached maximum iterations limit "
_LOCAL_CONDENSER_KEEP_FIRST = 4
_GENERATE_NUM_RETRIES = 2
_GEMINI_MAX_OUTPUT_TOKENS = 65_535
_ANTHROPIC_THINKING_BUFFER = 1_000
_SCHEMA_NAME_RE = re.compile(r"^[a-zA-Z0-9_-]{1,64}$")
_GENERATE_API_KEY_OVERRIDE = "SOLSTONE_GENERATE_API_KEY_OVERRIDE"
_GENERATE_MODEL_OVERRIDE = "SOLSTONE_GENERATE_MODEL_OVERRIDE"
_GENERATE_PROVIDER_OVERRIDE = "SOLSTONE_GENERATE_PROVIDER_OVERRIDE"


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


def _local_output_reserve_tokens(window: int) -> int:
    return window // 4


_LOCAL_TOKENIZER_DIVERGENCE_FACTOR = 1.125


def _local_condenser_max_tokens(window: int) -> int:
    """Modeled input boundary with a measured client/server tokenizer margin.

    This factor is not a system/tool overhead term. The condenser token count
    already includes the system prompt and tool schemas. It is a safety factor
    for one production observation (n=1): 12,437 served tokens vs 11,237
    LiteLLM-estimated tokens, or +10.7%, rounded upward.
    """

    modeled_input_budget = window - _local_output_reserve_tokens(window)
    return math.floor(modeled_input_budget / _LOCAL_TOKENIZER_DIVERGENCE_FACTOR)


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


def _is_diagnostic_cogitate(config: dict[str, Any]) -> bool:
    return config.get("diagnostic") is True


def _effective_access_tier(config: dict[str, Any]) -> str:
    if _is_diagnostic_cogitate(config):
        return "diagnostic"
    return str(config.get("access_tier", "normal"))


def _llm_num_retries(config: dict[str, Any]) -> int:
    if _is_diagnostic_cogitate(config):
        return CANNED_GENERATE_NUM_RETRIES
    return LLM_NUM_RETRIES


def _cogitate_budgets(config: dict[str, Any]) -> tuple[int, float, float]:
    max_turns = int(config.get("max_turns", MAX_TURNS) or MAX_TURNS)
    cost_cap = float(
        config.get("max_run_cost_usd", DEFAULT_RUN_COST_CAP_USD)
        or DEFAULT_RUN_COST_CAP_USD
    )
    timeout_seconds = float(config.get("timeout_seconds", 600) or 600)
    return max_turns, cost_cap, timeout_seconds


def _build_llm(
    provider: str,
    model: str,
    *,
    num_retries: int | None = None,
    endpoint: Any | None = None,
    served_window: int | None = None,
) -> Any:
    from openhands.sdk import LLM

    retry_count = LLM_NUM_RETRIES if num_retries is None else num_retries
    if provider == "local":
        from solstone.think.services.spp_transport import confidential_egress_base_url

        if endpoint is None:
            raise ValueError("Resolved local endpoint is required to build local LLM.")
        if not endpoint.is_bundled:
            base_url = confidential_egress_base_url(endpoint.base_url)
            llm_kwargs: dict[str, Any] = {
                "model": f"openai/{endpoint.served_model_id}",
                "base_url": f"{base_url}/v1",
                "api_key": endpoint.credential or "EMPTY",
                "native_tool_calling": False,
                "timeout": LLM_TIMEOUT_S,
                "num_retries": retry_count,
                "retry_min_wait": 1,
                "retry_max_wait": 2,
                "retry_multiplier": 1.0,
                "input_cost_per_token": 0,
                "output_cost_per_token": 0,
            }
            if served_window is not None and served_window >= LOCAL_MIN_CONTEXT_TOKENS:
                llm_kwargs["max_input_tokens"] = served_window
                llm_kwargs["max_output_tokens"] = _local_output_reserve_tokens(
                    served_window
                )
                if endpoint.is_confidential:
                    llm_kwargs["litellm_extra_body"] = {
                        "chat_template_kwargs": {"enable_thinking": False}
                    }
            return LLM(**llm_kwargs)

        from solstone.think.providers import local_server

        server = local_server.connect()
        return LLM(
            model=f"openai/{server.served_model_id}",
            base_url=f"http://127.0.0.1:{server.port}/v1",
            api_key="EMPTY",
            native_tool_calling=False,
            timeout=LLM_TIMEOUT_S,
            num_retries=retry_count,
            max_input_tokens=local_server.LOCAL_MIN_CONTEXT_TOKENS,
            max_output_tokens=_local_output_reserve_tokens(
                local_server.LOCAL_MIN_CONTEXT_TOKENS
            ),
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
        "num_retries": retry_count,
    }
    if provider == "openai":
        llm_kwargs["reasoning_summary"] = "auto"
        llm_kwargs["enable_encrypted_reasoning"] = True
    return LLM(**llm_kwargs)


def _build_local_condenser(llm: Any, *, max_tokens: int) -> Any:
    """LLM-summarizing condenser for an explicitly resolved local window.

    Reuses the agent's own LLM (shared usage_id is accepted in
    openhands-sdk 1.27.1) so there is no separate summarization endpoint.
    """
    from openhands.sdk.context.condenser import LLMSummarizingCondenser

    return LLMSummarizingCondenser(
        llm=llm,
        max_tokens=max_tokens,
        keep_first=_LOCAL_CONDENSER_KEEP_FIRST,
    )


def _build_cogitate_agent(
    *,
    llm: Any,
    condenser_max_tokens: int | None,
    tool_specs: list[Any],
    include_default_tools: list[Any],
    system_prompt: str,
) -> Any:
    from openhands.sdk import Agent

    condenser = (
        _build_local_condenser(llm, max_tokens=condenser_max_tokens)
        if condenser_max_tokens is not None
        else None
    )
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
                "Single command-line invocation to run directly, without a shell: "
                "use `sol`/`sol call ...` for normal journal access; run approved "
                "`journal` families ("
                + cogitate_journal_command_list()
                + ") directly as `journal <family> ...`, never prefixed with "
                "`sol` or `sol call`."
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
            "Run one policy-approved command directly, without a shell: use "
            "`sol`/`sol call ...` for normal journal access; run approved "
            "`journal` families ("
            + cogitate_journal_command_list()
            + ") directly as `journal <family> ...`, never prefixed with "
            "`sol` or `sol call`."
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


def _non_responsive_raw_payload(
    output: str, matched_signal: str | None
) -> list[dict[str, Any]]:
    payload: dict[str, Any] = {
        "reason_code": NON_RESPONSIVE_REASON_CODE,
        "non_responsive_output": output[:NON_RESPONSIVE_RAW_OUTPUT_CAP_CHARS],
    }
    if matched_signal is not None:
        payload["non_responsive_matched_signal"] = matched_signal
    return safe_raw([payload])


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


@dataclass
class _DiagnosticMaxIterationsDemotionState:
    demoted: bool = False


@contextmanager
def _demote_diagnostic_max_iterations_error() -> Iterator[
    _DiagnosticMaxIterationsDemotionState
]:
    state = _DiagnosticMaxIterationsDemotionState()

    class _MaxIterationsFilter(logging.Filter):
        def filter(self, record: logging.LogRecord) -> bool:
            if record.levelno == logging.ERROR and record.getMessage().startswith(
                _OPENHANDS_MAX_ITERATIONS_PREFIX
            ):
                record.levelno = logging.INFO
                record.levelname = "INFO"
                state.demoted = True
            return True

    logger = logging.getLogger(_OPENHANDS_CONVERSATION_LOGGER)
    max_iterations_filter = _MaxIterationsFilter()
    try:
        logger.addFilter(max_iterations_filter)
        yield state
    finally:
        logger.removeFilter(max_iterations_filter)


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
    provider = str(config["provider"])
    model = str(config["model"])
    effective_on_event = on_event
    byo_endpoint = None
    byo_served_window = None
    if provider == "local":
        from solstone.think.providers.local_endpoint import (
            resolve_endpoint_served_window,
            resolve_local_endpoint,
            wrap_on_event_redacting,
        )

        byo_endpoint = resolve_local_endpoint()
        if not byo_endpoint.is_bundled:
            byo_served_window = resolve_endpoint_served_window(byo_endpoint)
        if not byo_endpoint.is_bundled and byo_endpoint.credential:
            effective_on_event = wrap_on_event_redacting(
                on_event,
                byo_endpoint.credential,
            )
    callback = JSONEventCallback(effective_on_event)

    llm: Any | None = None
    usage_start: dict[str, int] | None = None
    persistence_tmpdir: tempfile.TemporaryDirectory[str] | None = None
    try:
        with _openhands_import_policy():
            from openhands.sdk import Conversation
            from openhands.sdk.tool.registry import register_tool
            from openhands.sdk.tool.spec import Tool

        diagnostic = _is_diagnostic_cogitate(config)
        wants_emit_final = expects_emit_final(config)
        max_turns, cost_cap, timeout_seconds = _cogitate_budgets(config)
        session_id, conversation_id = _session_identity(config.get("session_id"))
        prompt_body, system_instruction = assemble_prompt(
            config,
            sol_tool_name=None if diagnostic else "sol",
            diagnostic=diagnostic,
        )
        allowed_roots = _resolve_allowed_roots(config)
        access_tier = _effective_access_tier(config)
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
        llm = _build_llm(
            provider,
            model,
            num_retries=_llm_num_retries(config),
            endpoint=byo_endpoint,
            served_window=byo_served_window,
        )
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

        condenser_max_tokens = None
        if byo_endpoint is not None:
            if byo_endpoint.is_bundled:
                condenser_max_tokens = _local_condenser_max_tokens(
                    LOCAL_MIN_CONTEXT_TOKENS
                )
            elif (
                byo_served_window is not None
                and byo_served_window >= LOCAL_MIN_CONTEXT_TOKENS
            ):
                condenser_max_tokens = _local_condenser_max_tokens(byo_served_window)
        agent = _build_cogitate_agent(
            llm=llm,
            condenser_max_tokens=condenser_max_tokens,
            tool_specs=tool_specs,
            include_default_tools=default_tools,
            system_prompt=system_instruction,
        )

        if diagnostic:
            persistence_tmpdir = tempfile.TemporaryDirectory(
                prefix="solstone-cogitate-diagnostic-"
            )
            persistence_dir = Path(persistence_tmpdir.name)
        else:
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
        wall_clock_s = _wall_clock_deadline_s(timeout_seconds)
        wall_clock_exceeded = False
        demotion_state = _DiagnosticMaxIterationsDemotionState()
        with ExitStack() as stack:
            stack.enter_context(_suppress_litellm_cost_warnings())
            if diagnostic:
                demotion_state = stack.enter_context(
                    _demote_diagnostic_max_iterations_error()
                )
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

        if demotion_state.demoted:
            # Verbose-only at the default CLI log level; the demoted SDK line
            # still renders.
            LOG.info(
                "Readiness probe reached the iteration limit derived from its "
                "deliberate turn budget; the preceding SDK max-iterations line is "
                "expected here and was recorded at INFO rather than ERROR."
            )

        if sol_executor is not None:
            terminal_error = sol_executor.take_terminal_error()
            if terminal_error is not None:
                conversation.close()
                raise terminal_error

        result = translator.result()
        non_responsive = False
        non_responsive_raw: list[dict[str, Any]] | None = None
        if isinstance(result, str) and result.strip():
            responsiveness = classify_output_responsiveness(result)
            if responsiveness.non_responsive:
                non_responsive = True
                non_responsive_raw = _non_responsive_raw_payload(
                    result[:NON_RESPONSIVE_RAW_OUTPUT_CAP_CHARS],
                    responsiveness.matched_signal,
                )
                result = None
        usage = _usage_delta(usage_start, llm)
        if wall_clock_exceeded:
            has_partial = bool(result and result.strip())
            if non_responsive:
                error_text = (
                    "wall_clock_exceeded: cogitate run exceeded its wall-clock "
                    f"deadline after producing {NON_RESPONSIVE_OUTPUT_FRAGMENT}"
                )
            else:
                error_text = (
                    "wall_clock_exceeded: cogitate run exceeded its wall-clock "
                    "deadline and was force-finished with a partial result preserved"
                    if has_partial
                    else "wall_clock_exceeded: cogitate run exceeded its wall-clock "
                    "deadline and was force-finished before emitting a final result"
                )
            conversation.close()
            error_event: dict[str, Any] = {
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
            if non_responsive_raw is not None:
                error_event["raw"] = non_responsive_raw
            callback.emit(error_event)
            return None if non_responsive else result
        if translator._cost_force_stopped or translator.max_turns_exhausted:
            reason_code = (
                "token_budget_exceeded"
                if translator._cost_force_stopped
                else "max_turns_exhausted"
            )
            has_partial = bool(result and result.strip())
            if non_responsive:
                if reason_code == "token_budget_exceeded":
                    error_text = (
                        "token_budget_exceeded: cogitate run reached its per-run "
                        f"resource budget after producing {NON_RESPONSIVE_OUTPUT_FRAGMENT}"
                    )
                else:
                    error_text = (
                        "max_turns_exhausted: cogitate run reached its turn budget "
                        f"after producing {NON_RESPONSIVE_OUTPUT_FRAGMENT}"
                    )
            elif reason_code == "token_budget_exceeded":
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
            error_event = {
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
            if non_responsive_raw is not None:
                error_event["raw"] = non_responsive_raw
            callback.emit(error_event)
            return None if non_responsive else result
        execution_status = _conversation_execution_status(conversation)
        if execution_status in {"stuck", "paused"}:
            has_partial = bool(result and result.strip())
            if non_responsive:
                error_text = (
                    "agent_stuck: cogitate run was interrupted/stuck after producing "
                    f"{NON_RESPONSIVE_OUTPUT_FRAGMENT}"
                )
            else:
                error_text = (
                    "agent_stuck: cogitate run was interrupted/stuck with a partial "
                    "result preserved"
                    if has_partial
                    else "agent_stuck: cogitate run was interrupted/stuck before "
                    "emitting a final result"
                )
            conversation.close()
            error_event = {
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
            if non_responsive_raw is not None:
                error_event["raw"] = non_responsive_raw
            callback.emit(error_event)
            return None if non_responsive else result
        if non_responsive:
            callback.emit(
                {
                    "event": "error",
                    "error": (
                        f"{NON_RESPONSIVE_REASON_CODE}: cogitate run produced "
                        f"{NON_RESPONSIVE_OUTPUT_FRAGMENT}"
                    ),
                    "reason_code": NON_RESPONSIVE_REASON_CODE,
                    "provider": provider,
                    "usage": usage,
                    "terminal": True,
                    "cli_session_id": str(conversation_id),
                    "ts": now_ms(),
                    "raw": non_responsive_raw,
                }
            )
            return None
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
                redact_exception_credential,
            )

            local_endpoint = byo_endpoint
            if not local_endpoint.is_bundled:
                reason_code = classify_byo_cogitate_error(provider_exc)
                redact_exception_credential(exc, local_endpoint.credential)
                if reason_code:
                    setattr(exc, "reason_code", reason_code)
                    setattr(provider_exc, "reason_code", reason_code)
        reason_code = reason_code or classify_provider_error(provider_exc, provider)
        error_text = str(exc)[:PROVIDER_ERROR_TEXT_CAP_CHARS]
        trace_text = traceback.format_exc()
        if local_endpoint is not None:
            fixed_copy = local_endpoint_reason_copy(reason_code)
            if fixed_copy:
                error_text = fixed_copy
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
    finally:
        if persistence_tmpdir is not None:
            persistence_tmpdir.cleanup()


def _validation_reason(exc: BaseException, provider: str) -> str:
    return getattr(exc, "reason_code", None) or classify_provider_error(exc, provider)


def _probe(
    provider: str,
    model: str | None,
    api_key: str,
) -> None:
    from solstone.think import generate_client

    overrides = {_GENERATE_API_KEY_OVERRIDE: api_key}
    if model is not None:
        overrides.update(
            {
                _GENERATE_PROVIDER_OVERRIDE: provider,
                _GENERATE_MODEL_OVERRIDE: model,
            }
        )
    generate_client.generate_with_result(
        CANNED_GENERATE_PROMPT,
        "settings.cloud.validate_key",
        temperature=0,
        max_output_tokens=CANNED_GENERATE_MAX_OUTPUT_TOKENS,
        system_instruction=None,
        json_output=False,
        thinking_budget=CANNED_GENERATE_THINKING_BUDGET,
        timeout_s=CANNED_GENERATE_TIMEOUT_S,
        num_retries=CANNED_GENERATE_NUM_RETRIES,
        child_environment=overrides,
    )


def validate_key(provider: str, api_key: str) -> dict:
    """Verify a personal cloud key through the native generate transport."""
    try:
        _probe(provider, None, api_key)
        return {"valid": True}
    except Exception as exc:
        reason = _validation_reason(exc, provider)
        # A 404 or quota response proves the endpoint accepted the credential;
        # model selection performs the definitive, model-specific probe next.
        if reason in {"model_not_found", "provider_quota_exceeded"}:
            return {"valid": True, "probe_reason_code": reason}
        return {"valid": False, "error": str(exc), "reason_code": reason}


def validate_model(provider: str, model: str, api_key: str) -> dict:
    """Verify that a personal cloud key can run the selected model natively."""
    try:
        _probe(provider, model, api_key)
        return {"valid": True}
    except Exception as exc:
        return {
            "valid": False,
            "error": str(exc),
            "reason_code": _validation_reason(exc, provider),
        }
