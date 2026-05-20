# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""CLI subprocess runner for AI provider tool agents.

Spawns provider CLI tools (claude, codex, opencode) in JSON streaming mode
and translates their JSONL output into our standard Event format.

Each provider module implements a translate() function that converts
provider-specific JSONL events into our Event TypedDicts. The CLIRunner
handles subprocess lifecycle, stdin piping, and event emission.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import shutil
import signal
from pathlib import Path
from typing import Any, Callable

from solstone.think.providers.shared import JSONEventCallback, safe_raw
from solstone.think.utils import get_project_root, now_ms

LOG = logging.getLogger("solstone.think.providers.cli")

_PROJECT_ROOT = Path(get_project_root())
_TIMEOUT_LOG_DIR: Path = Path("/tmp")

_QUOTA_TOKENS = ("QUOTA_EXHAUSTED", "TerminalQuotaError")
_RETRY_DELAY_RE = re.compile(r'"?retryDelayMs"?\s*[:=]\s*"?([0-9]+(?:\.[0-9]+)?)')
_READ_BUDGET_TOOL_NAMES: tuple[str, ...] = (
    "read_file",
    "glob",
    "list_directory",
    "grep_search",
)
_READ_BUDGET_TOOLS: frozenset[str] = frozenset(_READ_BUDGET_TOOL_NAMES)


class QuotaExhaustedError(Exception):
    """Raised when a provider CLI reports quota exhaustion."""

    def __init__(self, message: str, retry_delay_ms: int | None = None) -> None:
        super().__init__(message)
        self.retry_delay_ms = retry_delay_ms


def _quota_error_from_text(text: str) -> QuotaExhaustedError | None:
    if not any(token in text for token in _QUOTA_TOKENS):
        return None

    retry_delay_ms: int | None = None
    match = _RETRY_DELAY_RE.search(text)
    if match:
        retry_delay_ms = int(float(match.group(1)))

    message = text.strip() or "Provider quota exhausted"
    return QuotaExhaustedError(message, retry_delay_ms)


async def _drain_line(stream: asyncio.StreamReader) -> None:
    """Drain a single overlong line from the stream by consuming it in chunks."""
    while True:
        try:
            await stream.readline()
            return
        except asyncio.LimitOverrunError as exc:
            await stream.readexactly(exc.consumed)


# ---------------------------------------------------------------------------
# Prompt Assembly
# ---------------------------------------------------------------------------


def cogitate_sol_tool_hint(tool_name: str) -> str:
    """Return the shell-tool hint for non-write cogitate runs."""
    return (
        "When the instructions tell you to run `sol ...` commands, invoke them "
        f"through the `{tool_name}` tool. Example: "
        f'`{tool_name}(command="sol call activities list")`. '
        "Do not invent or call a tool literally named `sol`."
    )


def assemble_prompt(
    config: dict[str, Any],
    *,
    sol_tool_name: str | None = None,
) -> tuple[str, str | None]:
    """Combine config fields into a single prompt string and system instruction.

    Joins transcript, extra_context, user_instruction, and prompt with
    double newlines. Returns the system_instruction separately for CLIs
    that support --system-prompt (Claude); callers for other CLIs should
    prepend it to the prompt body.

    Args:
        config: Agent config dict with prompt, transcript, etc.

    Returns:
        Tuple of (prompt_body, system_instruction).
        system_instruction may be None.
    """
    parts = []
    for key in ("transcript", "extra_context", "user_instruction", "prompt"):
        value = config.get(key)
        if value:
            parts.append(value)

    prompt_body = "\n\n".join(parts) if parts else ""
    system_instruction = config.get("system_instruction") or None
    if sol_tool_name:
        hint = cogitate_sol_tool_hint(sol_tool_name)
        if system_instruction:
            system_instruction = f"{system_instruction}\n\n{hint}"
        else:
            system_instruction = hint
    if config.get("read_scope"):
        scope_hint = (
            "Limit filesystem reads to today's segment dir unless the task explicitly requires broader history. "
            "If you need broader scope, state what and why in your reasoning."
        )
        if system_instruction:
            system_instruction = f"{system_instruction}\n\n{scope_hint}"
        else:
            system_instruction = scope_hint
    return prompt_body, system_instruction


# ---------------------------------------------------------------------------
# Thinking Aggregator
# ---------------------------------------------------------------------------


class ThinkingAggregator:
    """Buffers assistant text between tool calls for thinking/result classification.

    All assistant text that arrives between tool calls is treated as "thinking".
    Only the final text after all tool activity completes is the "result".

    Usage:
        agg = ThinkingAggregator(callback, model)
        # As text arrives:
        agg.accumulate("some text")
        # When a tool_start arrives, flush buffered text as thinking:
        agg.flush_as_thinking(raw_events=[...])
        # When done (no more tool calls), get the final result:
        result = agg.flush_as_result()
    """

    def __init__(
        self,
        callback: JSONEventCallback,
        model: str | None = None,
    ) -> None:
        self._buffer: list[str] = []
        self._callback = callback
        self._model = model

    def accumulate(self, text: str) -> None:
        """Add text to the buffer."""
        if text:
            self._buffer.append(text)

    def flush_as_thinking(self, raw_events: list[dict[str, Any]] | None = None) -> None:
        """Emit buffered text as a thinking event and clear the buffer.

        Does nothing if the buffer is empty.
        """
        text = "".join(self._buffer).strip()
        self._buffer.clear()
        if not text:
            return

        event: dict[str, Any] = {
            "event": "thinking",
            "summary": text,
            "ts": now_ms(),
        }
        if self._model:
            event["model"] = self._model
        if raw_events:
            event["raw"] = safe_raw(raw_events)
        self._callback.emit(event)

    def flush_as_result(self) -> str:
        """Return buffered text as the final result and clear the buffer."""
        text = "".join(self._buffer).strip()
        self._buffer.clear()
        return text

    @property
    def has_content(self) -> bool:
        """Whether the buffer has any content."""
        return bool(self._buffer)


# ---------------------------------------------------------------------------
# CLI Runner
# ---------------------------------------------------------------------------


class CLIRunner:
    """Spawn a CLI subprocess and translate its JSONL output to our events.

    The runner pipes a prompt to stdin, reads JSONL from stdout line by line,
    and calls a provider-specific translate function for each line.

    Args:
        cmd: Command to run (e.g., ["claude", "-p", "-", ...]).
        prompt_text: Text to pipe to stdin.
        translate: Function that receives (raw_event_dict, aggregator, callback)
            and emits our Event types. Must return the cli_session_id from the
            init event (or None for non-init events).
        callback: JSONEventCallback for emitting events.
        aggregator: ThinkingAggregator for text buffering.
        cwd: Working directory for the subprocess. Defaults to project root.
        env: Optional complete environment for the subprocess (used as-is, not merged). When None, inherits os.environ.
        timeout: Subprocess timeout in seconds. Default 600.
        first_event_timeout: Timeout for first stdout line in seconds. Default 90.
        read_call_budget: Optional combined budget for native read-tool calls.
    """

    def __init__(
        self,
        cmd: list[str],
        prompt_text: str,
        translate: Callable[
            [dict[str, Any], ThinkingAggregator, JSONEventCallback],
            str | None,
        ],
        callback: JSONEventCallback,
        aggregator: ThinkingAggregator,
        cwd: Path | None = None,
        env: dict[str, str] | None = None,
        provider: str = "",
        timeout: int = 600,
        first_event_timeout: int = 90,
        read_call_budget: int | None = None,
    ) -> None:
        self.cmd = cmd
        self.prompt_text = prompt_text
        self.translate = translate
        self.callback = callback
        self.aggregator = aggregator
        self.cwd = cwd or _PROJECT_ROOT
        self.env = env
        self.provider = provider
        self.timeout = timeout
        self.first_event_timeout = first_event_timeout
        self._timed_out_waiting_for_first_event = False
        self._already_retried_first_event: bool = False
        self._quota_error: QuotaExhaustedError | None = None
        self._read_call_budget = read_call_budget
        self._read_call_count = 0
        self._read_budget_tools = _READ_BUDGET_TOOLS
        self.cli_session_id: str | None = None

    async def run(self) -> str:
        """Spawn the CLI process, stream events, and return the final result.

        Returns:
            The final result text from the agent.

        Raises:
            RuntimeError: If the CLI binary is not found or process fails.
        """
        binary = self.cmd[0]
        if not shutil.which(binary):
            raise RuntimeError(
                f"CLI tool '{binary}' not found. Install it and ensure it's on PATH."
            )

        proc_env = self.env if self.env is not None else os.environ.copy()

        LOG.info("Spawning CLI: %s (cwd=%s)", " ".join(self.cmd), self.cwd)

        self._quota_error = None
        process: asyncio.subprocess.Process | None = None
        stderr_task: asyncio.Task[None] | None = None
        stderr_lines: list[str] = []
        self._timed_out_waiting_for_first_event = False

        try:
            process = await self._spawn_process(proc_env)
            self._send_prompt(process)
            stderr_task = asyncio.create_task(
                self._read_stderr(process, stderr_lines, binary)
            )

            try:
                try:
                    await asyncio.wait_for(
                        self._process_stdout(process),
                        timeout=self.timeout,
                    )
                except asyncio.TimeoutError:
                    if (
                        self._timed_out_waiting_for_first_event
                        and not self._already_retried_first_event
                    ):
                        LOG.warning(
                            "CLI first-event timed out after %ss, retrying once",
                            self.first_event_timeout,
                        )
                        self._already_retried_first_event = True
                        try:
                            process.kill()
                        except ProcessLookupError:
                            pass
                        await self._terminate_process_group(process)
                        await stderr_task
                        self._write_timeout_log(
                            which_timeout="first_event",
                            timeout_seconds=self.first_event_timeout,
                            proc_env=proc_env,
                            cmd=self.cmd,
                            cwd=str(self.cwd),
                            stderr_lines=stderr_lines,
                        )

                        process = await self._spawn_process(proc_env)
                        self._send_prompt(process)

                        stderr_lines = []
                        stderr_task = asyncio.create_task(
                            self._read_stderr(process, stderr_lines, binary)
                        )
                        self._timed_out_waiting_for_first_event = False
                        await asyncio.wait_for(
                            self._process_stdout(process),
                            timeout=self.timeout,
                        )
                    else:
                        raise
            except asyncio.TimeoutError:
                timeout_seconds = (
                    self.first_event_timeout
                    if self._timed_out_waiting_for_first_event
                    else self.timeout
                )
                which_timeout = (
                    "first_event"
                    if self._timed_out_waiting_for_first_event
                    else "full_run"
                )
                LOG.error("CLI process timed out after %ss, killing", timeout_seconds)
                try:
                    process.kill()
                except ProcessLookupError:
                    pass
                await self._terminate_process_group(process)
                await stderr_task
                self._write_timeout_log(
                    which_timeout=which_timeout,
                    timeout_seconds=timeout_seconds,
                    proc_env=proc_env,
                    cmd=self.cmd,
                    cwd=str(self.cwd),
                    stderr_lines=stderr_lines,
                )
                stderr_tail = "\n".join(stderr_lines[-20:])
                error_message = (
                    f"CLI process timed out after {timeout_seconds}s. "
                    f"Stderr tail:\n{stderr_tail}\n"
                    "Check that the CLI tool is installed and authenticated."
                )
                self.callback.emit(
                    {
                        "event": "error",
                        "error": error_message,
                        "reason_code": "chat_timeout",
                        "provider": self.provider,
                        "ts": now_ms(),
                    }
                )
                raise RuntimeError(error_message)

            if self._quota_error:
                raise self._quota_error

            await stderr_task
            if self._quota_error:
                raise self._quota_error

            return_code = await process.wait()
            result = self.aggregator.flush_as_result()

            if return_code != 0:
                stderr_text = "\n".join(stderr_lines[-20:])  # Last 20 lines
                if result:
                    # CLI failed but produced output — warn and return what we got
                    LOG.warning(
                        "CLI process exited with code %d but produced output. Stderr: %s",
                        return_code,
                        stderr_text,
                    )
                    self.callback.emit(
                        {
                            "event": "warning",
                            "message": f"CLI exited with code {return_code}",
                            "stderr": stderr_text,
                            "ts": now_ms(),
                        }
                    )
                else:
                    # CLI failed with no output — this is an error.
                    # Don't emit error event here; the caller's exception
                    # handler is responsible for error event emission.
                    LOG.error(
                        "CLI process exited with code %d: %s",
                        return_code,
                        stderr_text,
                    )
                    raise RuntimeError(
                        f"CLI process exited with code {return_code}. Stderr: {stderr_text}"
                    )

            return result
        finally:
            if process and process.returncode is None:
                await self._terminate_process_group(process)
            if stderr_task:
                await stderr_task

    async def _spawn_process(
        self, proc_env: dict[str, str]
    ) -> asyncio.subprocess.Process:
        return await asyncio.create_subprocess_exec(
            *self.cmd,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            limit=1024 * 1024,
            cwd=str(self.cwd),
            env=proc_env,
            process_group=0,
        )

    def _send_prompt(self, process: asyncio.subprocess.Process) -> None:
        if process.stdin:
            process.stdin.write(self.prompt_text.encode("utf-8"))
            process.stdin.close()

    async def _read_stderr(
        self,
        process: asyncio.subprocess.Process,
        stderr_lines: list[str],
        binary: str,
    ) -> None:
        if not process.stderr:
            return
        async for raw_line in process.stderr:
            line = raw_line.decode("utf-8", errors="replace").rstrip()
            if not line:
                continue
            stderr_lines.append(line)
            LOG.debug("[%s stderr] %s", binary, line)
            quota_error = _quota_error_from_text(line)
            if quota_error:
                self._quota_error = quota_error
                await self._terminate_process_group(process)
                return

    async def _terminate_process_group(
        self,
        process: asyncio.subprocess.Process,
        *,
        grace_seconds: float = 2.0,
    ) -> None:
        if process.returncode is not None:
            return

        try:
            pgid = os.getpgid(process.pid)
        except ProcessLookupError:
            pgid = None

        if pgid is not None:
            try:
                os.killpg(pgid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        else:
            try:
                process.terminate()
            except ProcessLookupError:
                pass

        try:
            await asyncio.wait_for(process.wait(), timeout=grace_seconds)
            return
        except asyncio.TimeoutError:
            pass

        if pgid is not None:
            try:
                os.killpg(pgid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        try:
            process.kill()
        except ProcessLookupError:
            pass
        await process.wait()

    async def _process_stdout(self, process: asyncio.subprocess.Process) -> None:
        """Read and translate JSONL lines from stdout."""
        if not process.stdout:
            return

        async def _process_line(raw_line: bytes) -> None:
            line = raw_line.decode("utf-8", errors="replace").strip()
            if not line:
                return

            quota_error = _quota_error_from_text(line)
            if quota_error:
                self._quota_error = quota_error
                raise quota_error

            try:
                event_data = json.loads(line)
            except json.JSONDecodeError:
                LOG.warning("Non-JSON stdout line: %s", line[:200])
                return

            tool_name = event_data.get("tool_name")
            if (
                event_data.get("type") == "tool_use"
                and tool_name in self._read_budget_tools
            ):
                self._read_call_count += 1
                if (
                    self._read_call_budget is not None
                    and self._read_call_count > self._read_call_budget
                ):
                    self.callback.emit(
                        {
                            "event": "tool_budget_exhausted",
                            "tool": tool_name,
                            "budget": self._read_call_budget,
                            "count": self._read_call_count,
                            "read_tools": list(_READ_BUDGET_TOOL_NAMES),
                            "ts": now_ms(),
                        }
                    )
                    await self._terminate_process_group(process)
                    raise RuntimeError(
                        "tool_budget_exhausted: read tool call budget exceeded "
                        f"({self._read_call_count}/{self._read_call_budget})"
                    )

            try:
                session_id = self.translate(event_data, self.aggregator, self.callback)
                if session_id:
                    self.cli_session_id = session_id
            except QuotaExhaustedError:
                raise
            except Exception:
                LOG.exception("Error translating CLI event: %s", line[:200])

        try:
            first_line = await asyncio.wait_for(
                process.stdout.readline(),
                timeout=self.first_event_timeout,
            )
        except asyncio.TimeoutError:
            self._timed_out_waiting_for_first_event = True
            raise
        if not first_line:
            return
        await _process_line(first_line)

        while True:
            try:
                raw_line = await process.stdout.readline()
            except asyncio.LimitOverrunError as exc:
                LOG.warning(
                    "CLI stdout line exceeded buffer limit (%d bytes consumed before limit); "
                    "draining and emitting truncated tool_end",
                    exc.consumed,
                )
                await _drain_line(process.stdout)
                self.callback.emit(
                    {
                        "event": "tool_end",
                        "tool": "bash",
                        "result": "[output truncated: too large to process — try a more targeted query]",
                        "ts": now_ms(),
                    }
                )
                continue
            if not raw_line:
                break
            await _process_line(raw_line)

    def _write_timeout_log(
        self,
        *,
        which_timeout: str,
        timeout_seconds: int,
        proc_env: dict[str, str],
        cmd: list[str],
        cwd: str | None,
        stderr_lines: list[str],
    ) -> Path | None:
        """Write a postmortem log for a CLI timeout."""

        timestamp_ms = now_ms()
        path = _TIMEOUT_LOG_DIR / f"gemini-cogitate-timeout-{timestamp_ms}.log"
        env_keys = ", ".join(sorted(set(proc_env.keys())))
        stderr_text = "\n".join(stderr_lines)
        content = "\n".join(
            [
                f"timestamp_ms: {timestamp_ms}",
                f"which_timeout: {which_timeout}",
                f"timeout_seconds: {timeout_seconds}",
                (f"already_retried_first_event: {self._already_retried_first_event}"),
                f"cmd: {cmd!r}",
                f"cwd: {cwd if cwd is not None else 'None'}",
                f"env_keys: {env_keys}",
                "stderr (full):",
                stderr_text,
            ]
        )

        try:
            with open(path, "w", encoding="utf-8") as log_file:
                log_file.write(content)
            os.chmod(str(path), 0o600)
        except OSError as exc:
            LOG.warning("Could not write timeout log to %s: %s", path, exc)
            return None
        return path


# ---------------------------------------------------------------------------
# CLI Binary Check
# ---------------------------------------------------------------------------


def check_cli_binary(name: str) -> str:
    """Check that a CLI binary is available on PATH.

    Args:
        name: Binary name (e.g., "claude", "codex", "opencode").

    Returns:
        The full path to the binary.

    Raises:
        RuntimeError: If the binary is not found.
    """
    path = shutil.which(name)
    if not path:
        raise RuntimeError(
            f"CLI tool '{name}' not found on PATH. "
            f"Install it and ensure it's accessible."
        )
    return path


# ---------------------------------------------------------------------------
# Cogitate Environment
# ---------------------------------------------------------------------------


_BASE_ALLOWLIST = [
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "PWD",
    "TERM",
    "TMPDIR",
    "TZ",
    "LANG",
    "LC_*",
    "XDG_*",
    "SOLSTONE_*",
    "SOL_*",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
]

_PROVIDER_ALLOWLIST: dict[str, list[str]] = {
    "anthropic": ["ANTHROPIC_*", "CLAUDE_*"],
    "openai": ["OPENAI_*"],
}


def _matches_env_pattern(key: str, pattern: str) -> bool:
    if pattern.endswith("*"):
        return key.startswith(pattern[:-1])
    return key == pattern


def build_cogitate_env(provider_name: str) -> dict[str, str]:
    """Build environment dict for a cogitate CLI subprocess.

    The child environment is built from an allowlist. Patterns ending in ``*``
    match the prefix before the star, including the exact prefix string.
    No other glob characters are supported.

    Args:
        provider_name: Provider name (``anthropic`` or ``openai``).

    Returns:
        Filtered environment for the provider CLI.
    """
    from solstone.think.providers import PROVIDER_METADATA
    from solstone.think.utils import get_config

    if provider_name not in _PROVIDER_ALLOWLIST:
        valid = ", ".join(sorted(_PROVIDER_ALLOWLIST))
        raise ValueError(f"Unsupported cogitate provider: {provider_name!r} ({valid})")

    config = get_config()
    providers_config = config.get("providers", {})
    auth_config = providers_config.get("auth", {})
    auth_mode = auth_config.get(provider_name, "platform")
    env_key = PROVIDER_METADATA[provider_name]["env_key"]
    allowlist = _BASE_ALLOWLIST + _PROVIDER_ALLOWLIST[provider_name]
    env = {
        key: value
        for key, value in os.environ.items()
        if any(_matches_env_pattern(key, pattern) for pattern in allowlist)
    }

    if auth_mode == "platform":
        env.pop(env_key, None)

    return env


__all__ = [
    "CLIRunner",
    "QuotaExhaustedError",
    "ThinkingAggregator",
    "assemble_prompt",
    "build_cogitate_env",
    "check_cli_binary",
]
