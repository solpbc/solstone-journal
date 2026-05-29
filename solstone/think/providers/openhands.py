# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""OpenHands provider facade for cogitate runs.

OpenHands and LiteLLM are installed on demand, so this module must stay importable
without either package present. Keep all OpenHands/LiteLLM imports inside the
functions that use them.
"""

from __future__ import annotations

import json
import logging
import os
import sys
import traceback
import uuid
from collections.abc import Callable
from contextlib import contextmanager
from datetime import datetime, timezone
from email.utils import parsedate_to_datetime
from importlib import import_module
from pathlib import Path
from typing import Any

from solstone.think.cogitate_policy import (
    DEFAULT_READ_CALL_BUDGET,
    MAX_TURNS,
    CogitatePolicy,
    MaxTurnsExhausted,
    resolve_read_scope,
)
from solstone.think.providers.cli import QuotaExhaustedError, assemble_prompt
from solstone.think.providers.shared import (
    USAGE_KEYS,
    JSONEventCallback,
    classify_provider_error,
    safe_raw,
)
from solstone.think.utils import get_journal, get_project_root, now_ms

_GENERATE_MODULES = {
    "anthropic": "solstone.think.providers.anthropic",
    "openai": "solstone.think.providers.openai",
    "google": "solstone.think.providers.google",
}

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
        from solstone.think.providers import local_server

        model_id = str(model)
        if model_id.startswith("openai/"):
            model_id = model_id[len("openai/") :]
        server = local_server.ensure_running(model_id)
        return LLM(
            model=f"openai/{model_id}",
            base_url=f"http://127.0.0.1:{server.port}/v1",
            api_key="EMPTY",
            native_tool_calling=False,
            input_cost_per_token=0,
            chat_template_kwargs={"enable_thinking": False},
        )

    if provider not in _MODEL_PREFIXES:
        raise ValueError(f"Unsupported OpenHands provider: {provider}")

    llm_kwargs: dict[str, Any] = {
        "model": _prefixed_model(provider, model),
        "api_key": os.getenv(_API_KEY_ENV[provider]),
        "native_tool_calling": True,
    }
    if provider == "openai":
        llm_kwargs["reasoning_summary"] = "auto"
        llm_kwargs["enable_encrypted_reasoning"] = True
    return LLM(**llm_kwargs)


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
        command: str = Field(description="Shell command to run.")

    class SolObservation(Observation):
        pass

    class SolExecutor(ToolExecutor):
        def __init__(
            self,
            *,
            policy: CogitatePolicy,
            callback: JSONEventCallback,
            write: bool,
            read_call_budget: int,
        ) -> None:
            self.policy = policy
            self.callback = callback
            self.write = write
            self.read_call_budget = read_call_budget
            self.read_call_count = 0
            self._budget_exhausted_emitted = False

        def __call__(self, action: Any, conversation: Any = None) -> Any:
            del conversation

            command = str(action.command)
            ok, reason = self.policy.check(
                "run_shell_command",
                {"command": command},
            )
            if not ok:
                return SolObservation.from_text(reason, is_error=True)

            if not self.write:
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

            result = _run_shell_command(command)
            return SolObservation.from_text(result["text"], is_error=result["is_error"])

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
    write: bool,
    read_call_budget: int,
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
        write=write,
        read_call_budget=read_call_budget,
    )
    tool = sol_tool_cls(
        description="Run a sol shell command after policy approval.",
        action_type=sol_action,
        observation_type=sol_observation,
        executor=executor,
        annotations=tool_annotations(
            title="sol",
            readOnlyHint=not write,
            destructiveHint=write,
            idempotentHint=not write,
            openWorldHint=False,
        ),
    )
    return [tool], executor


def _run_shell_command(command: str) -> dict[str, Any]:
    import subprocess

    try:
        completed = subprocess.run(
            ["bash", "-lc", command],
            text=True,
            capture_output=True,
            timeout=_SHELL_TIMEOUT_SECONDS,
            check=False,
        )
    except FileNotFoundError:
        return {"text": "command_not_found: bash", "is_error": True}
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
        parts.append(f"timeout: run_shell_command exceeded {_SHELL_TIMEOUT_SECONDS}s")
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
        provider: str,
        model: str,
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
        self.provider = provider
        self.model = model
        self.max_turns = max_turns
        self.expects_emit_final = expects_emit_final
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
        self._max_turns_event_emitted = False

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
                "ts": now_ms(),
            }
        )

    def _handle_conversation_error_event(self, event: Any) -> None:
        if getattr(event, "code", None) != "MaxIterationsReached":
            return
        self.max_turns_exhausted = True
        self.emit_max_turns_exhausted()

    def emit_max_turns_exhausted(self) -> None:
        if self._max_turns_event_emitted:
            return
        self.callback.emit(
            {
                "event": "max_turns_exhausted",
                "max_turns": self.max_turns,
                "ts": now_ms(),
            }
        )
        self._max_turns_event_emitted = True

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


async def run_cogitate(
    config: dict[str, Any],
    on_event: Callable[[dict], None] | None = None,
) -> str | None:
    """Run a cogitate prompt through OpenHands SDK."""
    callback = JSONEventCallback(on_event)
    provider = str(config["provider"])
    model = str(config["model"])

    try:
        from openhands.sdk import Agent, Conversation
        from openhands.sdk.tool.registry import register_tool
        from openhands.sdk.tool.spec import Tool

        write = bool(config.get("write"))
        expects_emit_final = bool(config.get("output_path")) or config.get(
            "schedule"
        ) in {"daily", "weekly", "activity"}
        max_turns = int(config.get("max_turns", MAX_TURNS) or MAX_TURNS)
        session_id, conversation_id = _session_identity(config.get("session_id"))
        prompt_body, system_instruction = assemble_prompt(
            config,
            sol_tool_name="sol" if not write else None,
        )
        allowed_roots = _resolve_allowed_roots(config)
        policy = CogitatePolicy(write=write, allowed_roots=allowed_roots)
        read_call_budget = int(
            config.get("read_call_budget", DEFAULT_READ_CALL_BUDGET) or 0
        )
        llm = _build_llm(provider, model)
        usage_start = _usage_snapshot(llm)
        sol_tools, _executor = _build_sol_tools(
            policy=policy,
            callback=callback,
            write=write,
            read_call_budget=read_call_budget,
        )
        # openhands-sdk v1.23 resolves Agent.tools by spec name via the
        # registry; passing ToolDefinition instances directly fails pydantic
        # validation. Re-register the per-run SolTool instance (its executor
        # closure captures this run's policy / callback / budget) and
        # reference it by name.
        register_tool("sol", sol_tools[0])
        tool_specs = [Tool(name="sol")]
        default_tools = ["FinishTool"]
        if expects_emit_final:
            from .emit_final_tool import build_emit_final_tools

            emit_final_tools = build_emit_final_tools()
            register_tool("emit_final", emit_final_tools[0])
            tool_specs.append(Tool(name="emit_final"))
            default_tools = []

        agent = Agent(
            llm=llm,
            tools=tool_specs,
            include_default_tools=default_tools,
            system_prompt=system_instruction,
        )

        journal = Path(get_journal())
        persistence_dir = journal / ".cache" / "cogitate-history" / session_id
        persistence_dir.mkdir(parents=True, exist_ok=True)
        translator = _OpenHandsTranslator(
            callback=callback,
            provider=provider,
            model=_prefixed_model(provider, model),
            max_turns=max_turns,
            expects_emit_final=expects_emit_final,
        )
        conversation = Conversation(
            agent=agent,
            workspace=str(get_project_root()),
            persistence_dir=str(persistence_dir),
            conversation_id=conversation_id,
            callbacks=[translator.on_event],
            token_callbacks=[translator.on_token],
            max_iteration_per_run=max_turns,
            stuck_detection=True,
            visualizer=None,
        )
        conversation.send_message(prompt_body)
        with _suppress_litellm_cost_warnings():
            await conversation.arun()

        if translator.max_turns_exhausted:
            raise MaxTurnsExhausted(
                f"max_turns_exhausted: OpenHands cogitate exceeded {max_turns} turns"
            )

        result = translator.result()
        callback.emit(
            {
                "event": "finish",
                "result": result,
                "usage": _usage_delta(usage_start, llm),
                "cli_session_id": str(conversation_id),
                "ts": now_ms(),
            }
        )
        return result
    except QuotaExhaustedError:
        raise
    except MaxTurnsExhausted:
        raise
    except Exception as exc:
        provider_exc = _unwrap_provider_exception(exc)
        if classify_provider_error(provider_exc, provider) == "provider_quota_exceeded":
            raise QuotaExhaustedError(
                str(provider_exc), _retry_delay_ms(provider_exc)
            ) from exc
        callback.emit(
            {
                "event": "error",
                "error": str(exc),
                "reason_code": classify_provider_error(provider_exc, provider),
                "provider": provider,
                "trace": traceback.format_exc(),
                "ts": now_ms(),
            }
        )
        setattr(exc, "_evented", True)
        raise


def run_generate(contents: Any, model: str, **kwargs: Any) -> Any:
    provider = kwargs.pop("provider")
    module = import_module(_GENERATE_MODULES[provider])
    return module.run_generate(contents=contents, model=model, **kwargs)


async def run_agenerate(contents: Any, model: str, **kwargs: Any) -> Any:
    provider = kwargs.pop("provider")
    module = import_module(_GENERATE_MODULES[provider])
    return await module.run_agenerate(contents=contents, model=model, **kwargs)


def list_models(provider: str) -> list[dict]:
    module = import_module(_GENERATE_MODULES[provider])
    return module.list_models()


def validate_key(provider: str, api_key: str) -> dict:
    module = import_module(_GENERATE_MODULES[provider])
    return module.validate_key(api_key)
