# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Thin async client for the native one-shot cogitate runtime."""

from __future__ import annotations

import asyncio
import json
import logging
import os
import signal
import uuid
from collections.abc import Callable
from functools import lru_cache
from pathlib import Path
from typing import Any

from solstone.think import core_handshake
from solstone.think.cogitate_policy import (
    DEFAULT_READ_CALL_BUDGET,
    DEFAULT_RUN_COST_CAP_USD,
    MAX_TURNS,
)
from solstone.think.providers.cli import QuotaExhaustedError
from solstone.think.utils import get_journal, now_ms

LOG = logging.getLogger(__name__)

_REQUEST_SCHEMA = "solstone-cogitate-request-v2"
_STDERR_DETAIL_CAP_CHARS = 4_000


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
