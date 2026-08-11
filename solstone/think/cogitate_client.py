# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Thin async client for the native one-shot cogitate runtime."""

from __future__ import annotations

import asyncio
import json
import logging
import os
import signal
import subprocess
import uuid
from collections.abc import Callable
from functools import lru_cache
from pathlib import Path
from typing import Any

from solstone.think import core_handshake
from solstone.think.providers.cli import QuotaExhaustedError
from solstone.think.providers.shared import (
    CANNED_GENERATE_MAX_OUTPUT_TOKENS,
    CANNED_GENERATE_NUM_RETRIES,
    CANNED_GENERATE_PROMPT,
    CANNED_GENERATE_THINKING_BUDGET,
    CANNED_GENERATE_TIMEOUT_S,
    classify_provider_error,
)
from solstone.think.utils import get_journal, now_ms

LOG = logging.getLogger(__name__)

_REQUEST_SCHEMA = "solstone-cogitate-request-v2"
_STDERR_DETAIL_CAP_CHARS = 4_000
MAX_TURNS = 60
DEFAULT_RUN_COST_CAP_USD = 1.00
DEFAULT_READ_CALL_BUDGET = 200
_GENERATE_API_KEY_OVERRIDE = "SOLSTONE_GENERATE_API_KEY_OVERRIDE"
_GENERATE_MODEL_OVERRIDE = "SOLSTONE_GENERATE_MODEL_OVERRIDE"
_GENERATE_PROVIDER_OVERRIDE = "SOLSTONE_GENERATE_PROVIDER_OVERRIDE"


@lru_cache(maxsize=1)
def _native_binary() -> Path:
    """Resolve the handshaken solstone-core helper executable."""
    handshake = core_handshake.check_solstone_core_handshake()
    if handshake.status != "ok":
        detail = handshake.message or "unknown solstone-core handshake failure"
        raise RuntimeError(f"cogitate requires solstone-core: {detail}")
    return core_handshake.helper_path_for_executable()


def _request(config: dict[str, Any], *, context_window: int | None) -> dict[str, Any]:
    """Encode prepared talent config as the native cogitate v2 request."""
    diagnostic = config.get("diagnostic") is True
    session_id = config.get("session_id")
    if not session_id:
        session_id = str(uuid.uuid4())
    resolved_context_window = (
        context_window if context_window is not None else config.get("context_window")
    )
    return {
        "schema": _REQUEST_SCHEMA,
        "access_tier": "diagnostic"
        if diagnostic
        else str(config.get("access_tier", "normal")),
        "outbound_approval": config.get("outbound_approval"),
        "diagnostic": diagnostic,
        "talent_instruction": config.get("user_instruction") or None,
        "sol_tool_name": None if diagnostic else "sol",
        "read_scope": config.get("read_scope") or [],
        "output_path": (
            str(config["output_path"]) if config.get("output_path") else None
        ),
        "schedule": config.get("schedule") or None,
        "max_turns": int(config.get("max_turns", MAX_TURNS) or MAX_TURNS),
        "cost_cap_usd": float(
            config.get("max_run_cost_usd", DEFAULT_RUN_COST_CAP_USD)
            or DEFAULT_RUN_COST_CAP_USD
        ),
        "context_window": resolved_context_window,
        "timeout_ms": int(
            float(config.get("timeout_seconds", 600) or 600) * 1_000
        ),
        "read_call_budget": int(
            config.get("read_call_budget", DEFAULT_READ_CALL_BUDGET)
            or DEFAULT_READ_CALL_BUDGET
        ),
        "model": str(config["model"]),
        "correlation_id": str(session_id),
        "initial_prompt": str(config.get("prompt") or ""),
        "journal_root": str(Path(get_journal()).resolve()),
        "dry_run": config.get("dry_run") is True,
    }


def _run_native_command(arguments: list[str], *, input_text: str | None = None) -> str:
    """Run a handshaken cogitate command and return its stdout."""
    try:
        completed = subprocess.run(
            [str(_native_binary()), "cogitate", *arguments],
            input=input_text,
            text=True,
            capture_output=True,
            check=False,
        )
    except OSError as exc:
        raise RuntimeError(f"cogitate native command could not start: {exc}") from exc
    if completed.returncode != 0:
        detail = completed.stderr.strip()
        message = f"cogitate native command exited with code {completed.returncode}"
        if detail:
            message = f"{message}: {detail}"
        raise RuntimeError(message)
    return completed.stdout


@lru_cache(maxsize=1)
def load_talent_contract() -> dict[str, Any]:
    """Load the native talent capability contract once per process."""
    try:
        contract = json.loads(_run_native_command(["--talent-contract"]))
    except json.JSONDecodeError as exc:
        raise RuntimeError("cogitate talent contract was not valid JSON") from exc
    if not isinstance(contract, dict):
        raise RuntimeError("cogitate talent contract was not an object")
    tiers = contract.get("tiers")
    if not isinstance(tiers, list) or any(
        not isinstance(tier, dict)
        or not isinstance(tier.get("name"), str)
        or not isinstance(tier.get("talent_facing"), bool)
        for tier in tiers
    ):
        raise RuntimeError("cogitate talent contract had invalid tiers")
    return contract


def render_dry_run_details(
    config: dict[str, Any], *, context_window: int | None = None
) -> dict[str, str | bool | None]:
    """Return native-composed prompt and finalization details from a dry run."""
    request = _request({**config, "dry_run": True}, context_window=context_window)
    output = _run_native_command(
        ["--one-shot"], input_text=f"{json.dumps(request, allow_nan=False)}\n"
    )
    try:
        events = [json.loads(line) for line in output.splitlines() if line.strip()]
    except json.JSONDecodeError as exc:
        raise RuntimeError("cogitate dry-run emitted invalid NDJSON") from exc
    if len(events) != 1 or not isinstance(events[0], dict):
        raise RuntimeError("cogitate dry-run did not emit exactly one event")
    event = events[0]
    rendered_prompt = event.get("rendered_prompt")
    if (
        event.get("event") != "dry_run"
        or event.get("terminal") is not True
        or not isinstance(rendered_prompt, dict)
        or not isinstance(rendered_prompt.get("initial_prompt"), str)
        or not isinstance(event.get("expects_emit_final"), bool)
        or (
            rendered_prompt.get("system_instruction") is not None
            and not isinstance(rendered_prompt.get("system_instruction"), str)
        )
    ):
        raise RuntimeError("cogitate dry-run event had no rendered prompt")
    return {
        "initial_prompt": rendered_prompt["initial_prompt"],
        "system_instruction": rendered_prompt["system_instruction"],
        "expects_emit_final": event["expects_emit_final"],
    }


def _validation_reason(exc: BaseException, provider: str) -> str:
    return getattr(exc, "reason_code", None) or classify_provider_error(exc, provider)


def _probe(provider: str, model: str | None, api_key: str) -> None:
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


def validate_key(provider: str, api_key: str) -> dict[str, Any]:
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


def validate_model(provider: str, model: str, api_key: str) -> dict[str, Any]:
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


def _emit_terminal_error(
    on_event: Callable[[dict[str, Any]], None] | None,
    *,
    error: str,
    provider: str,
    reason_code: str,
) -> None:
    if on_event is None:
        return
    on_event(
        {
            "event": "error",
            "error": error,
            "reason_code": reason_code,
            "provider": provider,
            "terminal": True,
            "ts": now_ms(),
        }
    )


def _stderr_detail(lines: list[str]) -> str:
    detail = "\n".join(lines).strip()
    if len(detail) <= _STDERR_DETAIL_CAP_CHARS:
        return detail
    return detail[-_STDERR_DETAIL_CAP_CHARS:]


async def _read_stderr(
    process: asyncio.subprocess.Process, stderr_lines: list[str]
) -> None:
    if process.stderr is None:
        return
    async for raw_line in process.stderr:
        line = raw_line.decode("utf-8", errors="replace").rstrip()
        if line:
            stderr_lines.append(line)
            LOG.debug("[solstone-core cogitate stderr] %s", line)


async def _terminate_process_group(process: asyncio.subprocess.Process) -> None:
    if process.returncode is not None:
        return
    try:
        pgid = os.getpgid(process.pid)
    except ProcessLookupError:
        pgid = None
    try:
        if pgid is not None:
            os.killpg(pgid, signal.SIGTERM)
        else:
            process.terminate()
    except ProcessLookupError:
        pass
    try:
        await asyncio.wait_for(process.wait(), timeout=2)
        return
    except asyncio.TimeoutError:
        pass
    try:
        if pgid is not None:
            os.killpg(pgid, signal.SIGKILL)
        else:
            process.kill()
    except ProcessLookupError:
        pass
    await process.wait()


async def run_cogitate(
    config: dict[str, Any],
    on_event: Callable[[dict[str, Any]], None] | None = None,
    *,
    context_window: int | None = None,
) -> str | None:
    """Run one native cogitate request and relay its NDJSON events."""
    provider = str(config.get("provider") or "")
    process: asyncio.subprocess.Process | None = None
    stderr_task: asyncio.Task[None] | None = None
    stderr_lines: list[str] = []
    terminal_seen = False
    result: str | None = None

    try:
        try:
            binary = _native_binary()
        except Exception as exc:
            message = f"cogitate native command unavailable: {exc}"
            _emit_terminal_error(
                on_event,
                error=message,
                provider=provider,
                reason_code="native_runtime_unavailable",
            )
            setattr(exc, "_evented", True)
            raise

        try:
            process = await asyncio.create_subprocess_exec(
                str(binary),
                "cogitate",
                "--one-shot",
                stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                limit=1024 * 1024,
                process_group=0,
            )
        except OSError as exc:
            message = f"cogitate native command could not start: {exc}"
            _emit_terminal_error(
                on_event,
                error=message,
                provider=provider,
                reason_code="native_runtime_unavailable",
            )
            error = RuntimeError(message)
            setattr(error, "_evented", True)
            raise error from exc

        stderr_task = asyncio.create_task(_read_stderr(process, stderr_lines))
        if process.stdin is None:
            message = "cogitate native command did not expose stdin"
            _emit_terminal_error(
                on_event,
                error=message,
                provider=provider,
                reason_code="native_runtime_unavailable",
            )
            error = RuntimeError(message)
            setattr(error, "_evented", True)
            raise error
        try:
            payload = json.dumps(
                _request(config, context_window=context_window), allow_nan=False
            )
            process.stdin.write((payload + "\n").encode("utf-8"))
            await process.stdin.drain()
            process.stdin.close()
        except Exception as exc:
            detail = str(exc) or type(exc).__name__
            message = f"cogitate native command could not send request: {detail}"
            _emit_terminal_error(
                on_event,
                error=message,
                provider=provider,
                reason_code="native_runtime_incomplete",
            )
            setattr(exc, "_evented", True)
            raise

        if process.stdout is not None:
            async for raw_line in process.stdout:
                line = raw_line.decode("utf-8", errors="replace").strip()
                if not line:
                    continue
                try:
                    event = json.loads(line)
                except json.JSONDecodeError:
                    LOG.warning("cogitate native command emitted invalid NDJSON")
                    continue
                if not isinstance(event, dict):
                    LOG.warning("cogitate native command emitted a non-object event")
                    continue

                event_kind = event.get("event")
                if (
                    event_kind == "error"
                    and event.get("terminal") is True
                    and event.get("reason_code") == "provider_quota_exceeded"
                ):
                    terminal_seen = True
                    raise QuotaExhaustedError(
                        str(event.get("error") or "Provider quota exhausted")
                    )

                # Forward compatible unknown event kinds deliberately relay unchanged.
                if isinstance(event_kind, str):
                    if on_event is not None:
                        on_event(event)
                    if event.get("terminal") is True:
                        terminal_seen = True
                    if event_kind == "finish" and isinstance(event.get("result"), str):
                        result = event["result"]
                else:
                    LOG.warning(
                        "cogitate native command emitted an event without a kind"
                    )

        return_code = await process.wait()
        if stderr_task is not None:
            await stderr_task
            stderr_task = None
        if not terminal_seen:
            detail = _stderr_detail(stderr_lines)
            message = (
                "cogitate native command exited with code "
                f"{return_code} without a terminal event"
            )
            if detail:
                message = f"{message}: {detail}"
            _emit_terminal_error(
                on_event,
                error=message,
                provider=provider,
                reason_code="native_runtime_incomplete",
            )
            if return_code != 0:
                error = RuntimeError(message)
                setattr(error, "_evented", True)
                raise error
        return result
    finally:
        if process is not None and process.returncode is None:
            await _terminate_process_group(process)
        if stderr_task is not None:
            await stderr_task
