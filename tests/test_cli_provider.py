# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for think.providers.cli — CLI subprocess runner infrastructure."""

import asyncio
import json
import os
from pathlib import Path
from unittest.mock import AsyncMock, patch

import pytest

from solstone.think.providers.cli import (
    CLIRunner,
    QuotaExhaustedError,
    ThinkingAggregator,
    assemble_prompt,
    build_cogitate_env,
    cogitate_sol_tool_hint,
)
from solstone.think.providers.shared import JSONEventCallback, safe_raw

# ---------------------------------------------------------------------------
# assemble_prompt
# ---------------------------------------------------------------------------


class TestAssemblePrompt:
    def test_all_fields(self):
        config = {
            "transcript": "Speaker A: hello",
            "extra_context": "Today is Monday",
            "user_instruction": "Summarize the transcript",
            "prompt": "What happened?",
            "system_instruction": "You are a helpful assistant",
        }
        body, system = assemble_prompt(config)
        assert "Speaker A: hello" in body
        assert "Today is Monday" in body
        assert "Summarize the transcript" in body
        assert "What happened?" in body
        assert system == "You are a helpful assistant"
        # Parts joined with double newlines
        assert body.count("\n\n") == 3

    def test_prompt_only(self):
        config = {"prompt": "hello"}
        body, system = assemble_prompt(config)
        assert body == "hello"
        assert system is None

    def test_empty_config(self):
        body, system = assemble_prompt({})
        assert body == ""
        assert system is None

    def test_skips_empty_values(self):
        config = {
            "transcript": "",
            "extra_context": None,
            "user_instruction": "Do something",
            "prompt": "Go",
        }
        body, system = assemble_prompt(config)
        assert body == "Do something\n\nGo"
        assert system is None

    def test_system_instruction_empty_string(self):
        config = {"prompt": "test", "system_instruction": ""}
        _, system = assemble_prompt(config)
        assert system is None

    def test_cogitate_sol_tool_hint_names_provider_tool_name(self):
        for tool_name in ("Bash", "run_shell_command", "bash"):
            hint = cogitate_sol_tool_hint(tool_name)
            assert tool_name in hint
            assert "Do not invent or call a tool literally named `sol`." in hint
            assert 'command="sol call activities list"' in hint

    def test_assemble_prompt_appends_sol_tool_hint_when_provided(self):
        body, system = assemble_prompt(
            {"prompt": "hello", "system_instruction": "Base system"},
            sol_tool_name="Bash",
        )

        assert body == "hello"
        assert system is not None
        assert system.startswith("Base system")
        assert "through the `Bash` tool" in system

    def test_assemble_prompt_does_not_append_hint_when_not_provided(self):
        body, system = assemble_prompt(
            {"prompt": "hello", "system_instruction": "Base system"},
            sol_tool_name=None,
        )

        assert body == "hello"
        assert system == "Base system"

    def test_assemble_prompt_appends_read_scope_hint(self):
        body, system = assemble_prompt(
            {
                "prompt": "hello",
                "system_instruction": "Base system",
                "read_scope": ["chronicle/<day>"],
            },
            sol_tool_name="run_shell_command",
        )

        assert body == "hello"
        assert system is not None
        assert "through the `run_shell_command` tool" in system
        assert "Limit filesystem reads to today's segment dir" in system


# ---------------------------------------------------------------------------
# ThinkingAggregator
# ---------------------------------------------------------------------------


class TestThinkingAggregator:
    def _make_aggregator(self):
        """Create aggregator with event capture."""
        events = []
        cb = JSONEventCallback(events.append)
        agg = ThinkingAggregator(cb, model="test-model")
        return agg, events

    def test_accumulate_and_flush_as_thinking(self):
        agg, events = self._make_aggregator()
        agg.accumulate("hello ")
        agg.accumulate("world")
        agg.flush_as_thinking(raw_events=[{"type": "message"}])

        assert len(events) == 1
        assert events[0]["event"] == "thinking"
        assert events[0]["summary"] == "hello world"
        assert events[0]["model"] == "test-model"
        assert events[0]["raw"] == [{"type": "message"}]

    def test_flush_thinking_empty_buffer_is_noop(self):
        agg, events = self._make_aggregator()
        agg.flush_as_thinking()
        assert len(events) == 0

    def test_flush_thinking_whitespace_only_is_noop(self):
        agg, events = self._make_aggregator()
        agg.accumulate("   ")
        agg.flush_as_thinking()
        assert len(events) == 0

    def test_flush_as_result(self):
        agg, events = self._make_aggregator()
        agg.accumulate("final answer")
        result = agg.flush_as_result()
        assert result == "final answer"
        # No events emitted for result flush
        assert len(events) == 0
        # Buffer is cleared
        assert agg.flush_as_result() == ""

    def test_multiple_thinking_flushes(self):
        """Simulate text -> tool -> text -> tool -> text pattern."""
        agg, events = self._make_aggregator()

        # First text chunk (before first tool call)
        agg.accumulate("Let me check...")
        agg.flush_as_thinking()

        # Second text chunk (between tool calls)
        agg.accumulate("Now let me verify...")
        agg.flush_as_thinking()

        # Final text (the result)
        agg.accumulate("The answer is 42")
        result = agg.flush_as_result()

        assert len(events) == 2
        assert events[0]["summary"] == "Let me check..."
        assert events[1]["summary"] == "Now let me verify..."
        assert result == "The answer is 42"

    def test_has_content(self):
        agg, _ = self._make_aggregator()
        assert not agg.has_content
        agg.accumulate("x")
        assert agg.has_content
        agg.flush_as_result()
        assert not agg.has_content

    def test_no_raw_events(self):
        agg, events = self._make_aggregator()
        agg.accumulate("thinking")
        agg.flush_as_thinking()
        assert "raw" not in events[0]

    def test_strips_whitespace(self):
        agg, events = self._make_aggregator()
        agg.accumulate("  padded  ")
        agg.flush_as_thinking()
        assert events[0]["summary"] == "padded"


class _MockStderr:
    """Async iterator yielding pre-set stderr lines."""

    def __init__(self, lines: list[bytes]):
        self._lines = lines
        self._index = 0

    def __aiter__(self):
        return self

    async def __anext__(self):
        if self._index >= len(self._lines):
            raise StopAsyncIteration
        line = self._lines[self._index]
        self._index += 1
        return line


class _MockStdout:
    """Async iterator yielding pre-set stdout lines, with readline support."""

    def __init__(self, lines: list[bytes]):
        self._lines = lines
        self._index = 0

    async def readline(self):
        if self._index >= len(self._lines):
            return b""
        line = self._lines[self._index]
        self._index += 1
        return line

    def __aiter__(self):
        return self

    async def __anext__(self):
        if self._index >= len(self._lines):
            raise StopAsyncIteration
        line = self._lines[self._index]
        self._index += 1
        return line


def _make_process(stdout_lines, stderr_lines, return_code):
    """Create a mock process with given stdout/stderr/exit code."""
    process = AsyncMock()
    process.stdout = _MockStdout(stdout_lines)
    process.stderr = _MockStderr(stderr_lines)
    process.stdin = AsyncMock()
    process.stdin.write = lambda _data: None
    process.stdin.close = lambda: None
    process.kill = lambda: None
    process.wait = AsyncMock(return_value=return_code)
    return process


class HangingStdout:
    async def readline(self):
        future = asyncio.get_running_loop().create_future()
        return await future


class _DelayedStdout:
    """Stdout mock that waits before yielding each line."""

    def __init__(self, lines: list[bytes], delay_seconds: float):
        self._lines = lines
        self._delay_seconds = delay_seconds
        self._index = 0

    async def readline(self):
        await asyncio.sleep(self._delay_seconds)
        if self._index >= len(self._lines):
            return b""
        line = self._lines[self._index]
        self._index += 1
        return line


class _FirstEmitThenHangStdout:
    """Stdout mock that emits one line and then hangs forever."""

    def __init__(self, first_line: bytes, delay_seconds: float = 0.0):
        self._first_line = first_line
        self._delay_seconds = delay_seconds
        self._emitted = False

    async def readline(self):
        if not self._emitted:
            self._emitted = True
            if self._delay_seconds:
                await asyncio.sleep(self._delay_seconds)
            return self._first_line
        future = asyncio.get_running_loop().create_future()
        return await future


class TestCLIRunnerExitCode:
    """Tests for CLIRunner handling of non-zero exit codes."""

    def test_quota_exhausted_stderr_raises_quota_error(self):
        """CLI quota stderr raises QuotaExhaustedError before generic exit handling."""
        events = []
        callback = JSONEventCallback(events.append)
        aggregator = ThinkingAggregator(callback, model="test-model")

        process = _make_process(
            stdout_lines=[],
            stderr_lines=[
                b'TerminalQuotaError: quota exhausted {"retryDelayMs": 120000}\n'
            ],
            return_code=1,
        )

        runner = CLIRunner(
            cmd=["fakecli", "--json"],
            prompt_text="test",
            translate=lambda _e, _a, _c: None,
            callback=callback,
            aggregator=aggregator,
        )

        with (
            patch(
                "solstone.think.providers.cli.asyncio.create_subprocess_exec",
                AsyncMock(return_value=process),
            ),
            patch(
                "solstone.think.providers.cli.shutil.which",
                return_value="/usr/bin/fakecli",
            ),
            pytest.raises(QuotaExhaustedError, match="quota exhausted") as exc_info,
        ):
            asyncio.run(runner.run())

        assert exc_info.value.retry_delay_ms == 120000
        # CLIRunner should NOT emit error events — that's the caller's job
        error_events = [e for e in events if e.get("event") == "error"]
        assert len(error_events) == 0

    def test_quota_exhausted_stdout_raises_quota_error(self):
        events = []
        callback = JSONEventCallback(events.append)
        aggregator = ThinkingAggregator(callback, model="test-model")
        process = _make_process(
            stdout_lines=[b'{"error":"QUOTA_EXHAUSTED","retryDelayMs":42}\n'],
            stderr_lines=[],
            return_code=1,
        )
        runner = CLIRunner(
            cmd=["fakecli", "--json"],
            prompt_text="test",
            translate=lambda _e, _a, _c: None,
            callback=callback,
            aggregator=aggregator,
        )

        with (
            patch(
                "solstone.think.providers.cli.asyncio.create_subprocess_exec",
                AsyncMock(return_value=process),
            ),
            patch(
                "solstone.think.providers.cli.shutil.which",
                return_value="/usr/bin/fakecli",
            ),
            pytest.raises(QuotaExhaustedError) as exc_info,
        ):
            asyncio.run(runner.run())

        assert exc_info.value.retry_delay_ms == 42

    def test_nonzero_exit_with_output_returns_result(self):
        """CLI exits with error but produced output → return result + warning."""
        events = []
        callback = JSONEventCallback(events.append)
        aggregator = ThinkingAggregator(callback, model="test-model")

        # translate accumulates text from stdout events
        def translate(event, agg, cb):
            if event.get("type") == "text":
                agg.accumulate(event["content"])
            return None

        process = _make_process(
            stdout_lines=[b'{"type": "text", "content": "The answer is 42"}\n'],
            stderr_lines=[b"Warning: something went wrong\n"],
            return_code=1,
        )

        runner = CLIRunner(
            cmd=["fakecli", "--json"],
            prompt_text="test",
            translate=translate,
            callback=callback,
            aggregator=aggregator,
        )

        with (
            patch(
                "solstone.think.providers.cli.asyncio.create_subprocess_exec",
                AsyncMock(return_value=process),
            ),
            patch(
                "solstone.think.providers.cli.shutil.which",
                return_value="/usr/bin/fakecli",
            ),
        ):
            result = asyncio.run(runner.run())

        assert result == "The answer is 42"
        warning_events = [e for e in events if e.get("event") == "warning"]
        assert len(warning_events) == 1
        assert "code 1" in warning_events[0]["message"]
        assert "something went wrong" in warning_events[0]["stderr"]

    def test_zero_exit_empty_result_ok(self):
        """CLI exits 0 with no output → return empty string, no error."""
        events = []
        callback = JSONEventCallback(events.append)
        aggregator = ThinkingAggregator(callback, model="test-model")

        process = _make_process(
            stdout_lines=[],
            stderr_lines=[],
            return_code=0,
        )

        runner = CLIRunner(
            cmd=["fakecli", "--json"],
            prompt_text="test",
            translate=lambda _e, _a, _c: None,
            callback=callback,
            aggregator=aggregator,
        )

        with (
            patch(
                "solstone.think.providers.cli.asyncio.create_subprocess_exec",
                AsyncMock(return_value=process),
            ),
            patch(
                "solstone.think.providers.cli.shutil.which",
                return_value="/usr/bin/fakecli",
            ),
        ):
            result = asyncio.run(runner.run())

        assert result == ""
        assert not [e for e in events if e.get("event") in ("error", "warning")]

    def test_env_dict_used_directly_without_merge(self):
        events = []
        callback = JSONEventCallback(events.append)
        aggregator = ThinkingAggregator(callback, model="test-model")
        provided_env = {"PATH": "/custom/bin"}
        sentinel_key = "CLIRUNNER_TEST_LEAK"
        captured_env = None

        async def create_subprocess_exec(*args, **kwargs):
            nonlocal captured_env
            captured_env = kwargs["env"]
            return _make_process([], [], 0)

        runner = CLIRunner(
            cmd=["fakecli", "--json"],
            prompt_text="test",
            translate=lambda _e, _a, _c: None,
            callback=callback,
            aggregator=aggregator,
            env=provided_env,
        )

        os.environ[sentinel_key] = "should-not-leak"
        try:
            with (
                patch(
                    "solstone.think.providers.cli.asyncio.create_subprocess_exec",
                    AsyncMock(side_effect=create_subprocess_exec),
                ),
                patch(
                    "solstone.think.providers.cli.shutil.which",
                    return_value="/usr/bin/fakecli",
                ),
            ):
                asyncio.run(runner.run())
        finally:
            os.environ.pop(sentinel_key, None)

        assert captured_env == provided_env
        assert captured_env is provided_env
        assert sentinel_key not in captured_env

    def test_cwd_passed_to_create_subprocess_exec(self):
        events = []
        callback = JSONEventCallback(events.append)
        aggregator = ThinkingAggregator(callback, model="test-model")
        captured_cwd = None
        captured_process_group = None

        async def create_subprocess_exec(*args, **kwargs):
            nonlocal captured_cwd, captured_process_group
            captured_cwd = kwargs["cwd"]
            captured_process_group = kwargs["process_group"]
            return _make_process([], [], 0)

        runner = CLIRunner(
            cmd=["fakecli", "--json"],
            prompt_text="test",
            translate=lambda _e, _a, _c: None,
            callback=callback,
            aggregator=aggregator,
            cwd=Path("/tmp"),
        )

        with (
            patch(
                "solstone.think.providers.cli.asyncio.create_subprocess_exec",
                AsyncMock(side_effect=create_subprocess_exec),
            ),
            patch(
                "solstone.think.providers.cli.shutil.which",
                return_value="/usr/bin/fakecli",
            ),
        ):
            asyncio.run(runner.run())

        assert captured_cwd == "/tmp"
        assert captured_process_group == 0

    def test_read_tool_budget_overflow_terminates_process_group(self):
        events = []
        callback = JSONEventCallback(events.append)
        aggregator = ThinkingAggregator(callback, model="test-model")
        stdout_lines = [
            (json.dumps({"type": "tool_use", "tool_name": "read_file"}) + "\n").encode(
                "utf-8"
            )
            for _ in range(201)
        ]
        process = _make_process(stdout_lines, [], 0)
        translated = []

        def translate(event, _agg, _cb):
            translated.append(event)
            return None

        runner = CLIRunner(
            cmd=["fakecli", "--json"],
            prompt_text="test",
            translate=translate,
            callback=callback,
            aggregator=aggregator,
            read_call_budget=200,
        )
        runner._terminate_process_group = AsyncMock()

        with (
            patch(
                "solstone.think.providers.cli.asyncio.create_subprocess_exec",
                AsyncMock(return_value=process),
            ),
            patch(
                "solstone.think.providers.cli.shutil.which",
                return_value="/usr/bin/fakecli",
            ),
            pytest.raises(RuntimeError) as exc_info,
        ):
            asyncio.run(runner.run())

        assert "tool_budget_exhausted" in str(exc_info.value)
        assert "(201/200)" in str(exc_info.value)
        assert len(translated) == 200
        exhausted = [
            event for event in events if event["event"] == "tool_budget_exhausted"
        ]
        assert exhausted == [
            {
                "event": "tool_budget_exhausted",
                "tool": "read_file",
                "budget": 200,
                "count": 201,
                "read_tools": ["read_file", "glob", "list_directory", "grep_search"],
                "ts": exhausted[0]["ts"],
            }
        ]
        runner._terminate_process_group.assert_called_once_with(process)


class TestCLIRunnerFirstEventTimeout:
    def test_first_event_timeout_includes_stderr(self):
        events = []
        callback = JSONEventCallback(events.append)
        aggregator = ThinkingAggregator(callback, model="test-model")

        process = _make_process([], [b"Please authenticate first\n"], 0)
        process.stdout = HangingStdout()  # Override with hanging version

        runner = CLIRunner(
            cmd=["fakecli", "--json"],
            prompt_text="test prompt",
            translate=lambda _event, _agg, _cb: None,
            callback=callback,
            aggregator=aggregator,
            timeout=5,
            first_event_timeout=0.1,
        )
        # Force single-shot behavior to keep this test focused on the give-up
        # surface; the new retry contract is covered in
        # TestCLIRunnerFirstEventRetry.
        runner._already_retried_first_event = True

        with (
            patch(
                "solstone.think.providers.cli.asyncio.create_subprocess_exec",
                AsyncMock(return_value=process),
            ),
            patch(
                "solstone.think.providers.cli.shutil.which",
                return_value="/usr/bin/fakecli",
            ),
            pytest.raises(RuntimeError) as exc_info,
        ):
            asyncio.run(runner.run())

        message = str(exc_info.value)
        assert "authenticate" in message.lower()
        assert "Check that the CLI tool is installed and authenticated." in message

        error_events = [event for event in events if event.get("event") == "error"]
        assert len(error_events) == 1
        assert "Please authenticate first" in error_events[0]["error"]


class TestCLIRunnerFirstEventRetry:
    @staticmethod
    def _translate_text_event(event, agg, cb):
        if event.get("type") == "text":
            agg.accumulate(event["content"])
            cb.emit({"event": "text", "text": event["content"]})
        return None

    def test_short_first_event_timeout_with_slow_first_emit_raises(
        self, monkeypatch, tmp_path
    ):
        monkeypatch.setattr("solstone.think.providers.cli._TIMEOUT_LOG_DIR", tmp_path)
        events = []
        callback = JSONEventCallback(events.append)
        aggregator = ThinkingAggregator(callback, model="test-model")
        slow_line = b'{"type": "text", "content": "slow"}\n'
        process_one = _make_process([], [], 0)
        process_one.stdout = _DelayedStdout([slow_line], delay_seconds=0.5)
        process_two = _make_process([], [], 0)
        process_two.stdout = _DelayedStdout([slow_line], delay_seconds=0.5)
        runner = CLIRunner(
            cmd=["fakecli", "--json"],
            prompt_text="test",
            translate=self._translate_text_event,
            callback=callback,
            aggregator=aggregator,
            timeout=5,
            first_event_timeout=0.05,
        )

        with (
            patch(
                "solstone.think.providers.cli.asyncio.create_subprocess_exec",
                AsyncMock(side_effect=[process_one, process_two]),
            ) as mock_create,
            patch(
                "solstone.think.providers.cli.shutil.which",
                return_value="/usr/bin/fakecli",
            ),
            pytest.raises(RuntimeError),
        ):
            asyncio.run(runner.run())

        assert mock_create.call_count == 2

    def test_first_event_timeout_with_headroom_succeeds(self, monkeypatch, tmp_path):
        monkeypatch.setattr("solstone.think.providers.cli._TIMEOUT_LOG_DIR", tmp_path)
        events = []
        callback = JSONEventCallback(events.append)
        aggregator = ThinkingAggregator(callback, model="test-model")
        line = b'{"type": "text", "content": "headroom"}\n'
        process = _make_process([], [], 0)
        process.stdout = _DelayedStdout([line], delay_seconds=0.05)
        runner = CLIRunner(
            cmd=["fakecli", "--json"],
            prompt_text="test",
            translate=self._translate_text_event,
            callback=callback,
            aggregator=aggregator,
            timeout=5,
            first_event_timeout=1.0,
        )

        with (
            patch(
                "solstone.think.providers.cli.asyncio.create_subprocess_exec",
                AsyncMock(return_value=process),
            ) as mock_create,
            patch(
                "solstone.think.providers.cli.shutil.which",
                return_value="/usr/bin/fakecli",
            ),
        ):
            result = asyncio.run(runner.run())

        assert result == "headroom"
        assert mock_create.call_count == 1
        assert runner._already_retried_first_event is False

    def test_first_event_timeout_triggers_one_retry(self, monkeypatch, tmp_path):
        monkeypatch.setattr("solstone.think.providers.cli._TIMEOUT_LOG_DIR", tmp_path)
        events = []
        callback = JSONEventCallback(events.append)
        aggregator = ThinkingAggregator(callback, model="test-model")
        hanging_proc_1 = _make_process([], [], 0)
        hanging_proc_1.stdout = HangingStdout()
        hanging_proc_2 = _make_process([], [], 0)
        hanging_proc_2.stdout = HangingStdout()
        runner = CLIRunner(
            cmd=["fakecli", "--json"],
            prompt_text="test",
            translate=self._translate_text_event,
            callback=callback,
            aggregator=aggregator,
            timeout=5,
            first_event_timeout=0.05,
        )

        with (
            patch(
                "solstone.think.providers.cli.asyncio.create_subprocess_exec",
                AsyncMock(side_effect=[hanging_proc_1, hanging_proc_2]),
            ) as mock_create,
            patch(
                "solstone.think.providers.cli.shutil.which",
                return_value="/usr/bin/fakecli",
            ),
            pytest.raises(RuntimeError),
        ):
            asyncio.run(runner.run())

        assert mock_create.call_count == 2
        assert runner._already_retried_first_event is True

    def test_retry_succeeds_when_second_spawn_emits(self, monkeypatch, tmp_path):
        monkeypatch.setattr("solstone.think.providers.cli._TIMEOUT_LOG_DIR", tmp_path)
        events = []
        callback = JSONEventCallback(events.append)
        aggregator = ThinkingAggregator(callback, model="test-model")
        hanging_proc = _make_process([], [], 0)
        hanging_proc.stdout = HangingStdout()
        healthy_proc = _make_process([], [], 0)
        healthy_proc.stdout = _DelayedStdout(
            [b'{"type": "text", "content": "retry ok"}\n'],
            delay_seconds=0.01,
        )
        runner = CLIRunner(
            cmd=["fakecli", "--json"],
            prompt_text="test",
            translate=self._translate_text_event,
            callback=callback,
            aggregator=aggregator,
            timeout=5,
            first_event_timeout=0.05,
        )

        with (
            patch(
                "solstone.think.providers.cli.asyncio.create_subprocess_exec",
                AsyncMock(side_effect=[hanging_proc, healthy_proc]),
            ) as mock_create,
            patch(
                "solstone.think.providers.cli.shutil.which",
                return_value="/usr/bin/fakecli",
            ),
        ):
            result = asyncio.run(runner.run())

        assert result == "retry ok"
        assert mock_create.call_count == 2
        assert runner._already_retried_first_event is True
        assert [event for event in events if event.get("event") == "text"]

    def test_timeout_log_redacts_env_values_and_prompt(self, monkeypatch, tmp_path):
        monkeypatch.setattr("solstone.think.providers.cli._TIMEOUT_LOG_DIR", tmp_path)
        events = []
        callback = JSONEventCallback(events.append)
        aggregator = ThinkingAggregator(callback, model="test-model")
        hanging_proc_1 = _make_process([], [], 0)
        hanging_proc_1.stdout = HangingStdout()
        hanging_proc_2 = _make_process([], [], 0)
        hanging_proc_2.stdout = HangingStdout()
        runner = CLIRunner(
            cmd=["fakecli", "--json"],
            prompt_text="prompt-do-not-leak-67890",
            translate=self._translate_text_event,
            callback=callback,
            aggregator=aggregator,
            env={"FAKE_KEY": "do-not-leak-me-12345"},
            timeout=5,
            first_event_timeout=0.05,
        )

        with (
            patch(
                "solstone.think.providers.cli.asyncio.create_subprocess_exec",
                AsyncMock(side_effect=[hanging_proc_1, hanging_proc_2]),
            ),
            patch(
                "solstone.think.providers.cli.shutil.which",
                return_value="/usr/bin/fakecli",
            ),
            pytest.raises(RuntimeError),
        ):
            asyncio.run(runner.run())

        files = list(tmp_path.glob("gemini-cogitate-timeout-*.log"))
        assert len(files) == 2
        for file_path in files:
            content = file_path.read_text()
            assert "FAKE_KEY" in content
            assert "do-not-leak-me-12345" not in content
            assert "prompt-do-not-leak-67890" not in content
            assert file_path.stat().st_mode & 0o777 == 0o600

    def test_full_run_timeout_writes_log(self, monkeypatch, tmp_path):
        monkeypatch.setattr("solstone.think.providers.cli._TIMEOUT_LOG_DIR", tmp_path)
        events = []
        callback = JSONEventCallback(events.append)
        aggregator = ThinkingAggregator(callback, model="test-model")
        process = _make_process([], [], 0)
        process.stdout = _FirstEmitThenHangStdout(
            b'{"type": "text", "content": "first line"}\n'
        )
        runner = CLIRunner(
            cmd=["fakecli", "--json"],
            prompt_text="test",
            translate=self._translate_text_event,
            callback=callback,
            aggregator=aggregator,
            timeout=0.05,
            first_event_timeout=1.0,
        )

        with (
            patch(
                "solstone.think.providers.cli.asyncio.create_subprocess_exec",
                AsyncMock(return_value=process),
            ),
            patch(
                "solstone.think.providers.cli.shutil.which",
                return_value="/usr/bin/fakecli",
            ),
            pytest.raises(RuntimeError),
        ):
            asyncio.run(runner.run())

        files = list(tmp_path.glob("gemini-cogitate-timeout-*.log"))
        assert len(files) == 1
        assert "which_timeout: full_run" in files[0].read_text()


_OVERSIZE = object()  # sentinel for oversize line in _MockStdoutWithOversize


class _MockStdoutWithOversize:
    """Stdout mock that raises LimitOverrunError on a specific readline() call."""

    def __init__(self, lines: list):
        # lines entries are either bytes or the sentinel OVERSIZE
        self._lines = lines
        self._index = 0
        self._draining_oversize = False

    async def readline(self):
        if self._draining_oversize:
            self._draining_oversize = False
            return b"x" * 1024 * 1024 + b"\n"
        if self._index >= len(self._lines):
            return b""
        entry = self._lines[self._index]
        self._index += 1
        if entry is _OVERSIZE:
            self._draining_oversize = True
            raise asyncio.LimitOverrunError(
                "Separator is not found, and chunk exceed the limit", 1024 * 1024
            )
        return entry

    async def readexactly(self, n: int) -> bytes:
        return b"x" * n

    def __aiter__(self):
        return self

    async def __anext__(self):
        val = await self.readline()
        if val == b"":
            raise StopAsyncIteration
        return val


class TestCLIRunnerOversizedOutput:
    """CLIRunner recovers from LimitOverrunError in the stdout loop."""

    def test_oversized_line_emits_tool_end_and_continues(self):
        """Oversize line → synthetic tool_end emitted + subsequent line processed."""
        import json

        normal_line_1 = json.dumps({"event": "text", "text": "hello"}).encode() + b"\n"
        normal_line_2 = json.dumps({"event": "text", "text": "world"}).encode() + b"\n"

        events = []
        callback = JSONEventCallback(events.append)
        aggregator = ThinkingAggregator(callback, model="test-model")

        process = AsyncMock()
        process.stdout = _MockStdoutWithOversize(
            [
                normal_line_1,
                _OVERSIZE,
                normal_line_2,
            ]
        )
        process.stderr = _MockStderr([])
        process.stdin = AsyncMock()
        process.stdin.write = lambda _data: None
        process.stdin.close = lambda: None
        process.kill = lambda: None
        process.wait = AsyncMock(return_value=0)

        # translate just forwards text events as-is
        def translate(event_data, agg, cb):
            if event_data.get("event") == "text":
                cb.emit({"event": "text", "text": event_data["text"]})
            return None

        runner = CLIRunner(
            cmd=["fakecli", "--json"],
            prompt_text="test",
            translate=translate,
            callback=callback,
            aggregator=aggregator,
        )

        with (
            patch(
                "solstone.think.providers.cli.asyncio.create_subprocess_exec",
                AsyncMock(return_value=process),
            ),
            patch(
                "solstone.think.providers.cli.shutil.which",
                return_value="/usr/bin/fakecli",
            ),
        ):
            asyncio.run(runner.run())

        event_types = [e["event"] for e in events]
        # tool_end should be emitted
        assert "tool_end" in event_types, f"Expected tool_end in events: {events}"

        # the tool_end result should indicate truncation
        tool_end_events = [e for e in events if e["event"] == "tool_end"]
        assert len(tool_end_events) == 1
        assert "truncated" in tool_end_events[0]["result"]

        # the normal line after the oversize error should also be processed
        text_events = [e for e in events if e["event"] == "text"]
        texts = [e["text"] for e in text_events]
        assert "world" in texts, f"Expected 'world' in text events: {texts}"


# ---------------------------------------------------------------------------
# safe_raw
# ---------------------------------------------------------------------------


class TestSafeRaw:
    def test_small_event_returned_unchanged(self):
        events = [{"type": "tool_use", "tool_name": "read_file", "tool_id": "t1"}]
        assert safe_raw(events) is events

    def test_large_event_trimmed(self):
        big_output = "x" * 20_000
        events = [
            {
                "type": "tool_result",
                "tool_id": "t1",
                "output": big_output,
                "extra_field": "value",
            }
        ]
        result = safe_raw(events)
        assert result is not events
        # Should keep only structural keys
        assert result[0] == {"type": "tool_result", "tool_id": "t1"}
        # Last element is the trimmed metadata
        meta = result[-1]["_raw_trimmed"]
        assert meta["limit"] == 16_384
        assert meta["original_bytes"] > 16_384

    def test_custom_limit(self):
        events = [{"type": "message", "content": "a" * 200}]
        # Under custom limit
        assert safe_raw(events, limit=1024) is events
        # Over custom limit
        result = safe_raw(events, limit=50)
        assert result is not events
        assert result[-1]["_raw_trimmed"]["limit"] == 50

    def test_structural_keys_preserved(self):
        events = [
            {
                "type": "tool_use",
                "id": "abc",
                "tool_id": "t1",
                "tool_name": "search",
                "role": "assistant",
                "event_type": "message",
                "timestamp": 12345,
                "big_content": "z" * 20_000,
            }
        ]
        result = safe_raw(events)
        kept = result[0]
        assert kept == {
            "type": "tool_use",
            "id": "abc",
            "tool_id": "t1",
            "tool_name": "search",
            "role": "assistant",
            "event_type": "message",
            "timestamp": 12345,
        }

    def test_multiple_events(self):
        events = [
            {"type": "msg", "data": "a" * 10_000},
            {"type": "msg", "data": "b" * 10_000},
        ]
        result = safe_raw(events)
        assert len(result) == 3  # 2 trimmed events + 1 metadata
        assert result[0] == {"type": "msg"}
        assert result[1] == {"type": "msg"}
        assert "_raw_trimmed" in result[2]


# ---------------------------------------------------------------------------
# build_cogitate_env
# ---------------------------------------------------------------------------


def test_build_cogitate_env_allowlist_anthropic():
    config = {"providers": {"auth": {"anthropic": "api_key"}}}
    with (
        patch.dict(
            os.environ,
            {
                "PATH": "/bin",
                "HOME": "/home/test",
                "ANTHROPIC_API_KEY": "sk-ant",
                "CLAUDE_CONFIG_DIR": "/tmp/claude",
                "OPENAI_API_KEY": "sk-oai",
                "GOOGLE_API_KEY": "gk",
                "HTTPS_PROXY": "http://proxy",
            },
            clear=True,
        ),
        patch("solstone.think.utils.get_config", return_value=config),
    ):
        env = build_cogitate_env("anthropic")

    assert env["ANTHROPIC_API_KEY"] == "sk-ant"
    assert env["CLAUDE_CONFIG_DIR"] == "/tmp/claude"
    assert env["PATH"] == "/bin"
    assert "OPENAI_API_KEY" not in env
    assert "GOOGLE_API_KEY" not in env
    assert "HTTPS_PROXY" not in env


def test_build_cogitate_env_allowlist_openai():
    config = {"providers": {"auth": {"openai": "api_key"}}}
    with (
        patch.dict(
            os.environ,
            {
                "PATH": "/bin",
                "OPENAI_API_KEY": "sk-oai",
                "OPENAI_ORG_ID": "org",
                "ANTHROPIC_API_KEY": "sk-ant",
                "GOOGLE_API_KEY": "gk",
                "NODE_OPTIONS": "--inspect",
            },
            clear=True,
        ),
        patch("solstone.think.utils.get_config", return_value=config),
    ):
        env = build_cogitate_env("openai")

    assert env["OPENAI_API_KEY"] == "sk-oai"
    assert env["OPENAI_ORG_ID"] == "org"
    assert "ANTHROPIC_API_KEY" not in env
    assert "GOOGLE_API_KEY" not in env
    assert "NODE_OPTIONS" not in env


class TestBuildCogitateEnv:
    """Tests for build_cogitate_env — API key stripping for CLI subprocesses."""

    def test_default_strips_key(self):
        """No auth config → default platform mode → key removed."""
        config = {"providers": {}}
        with (
            patch.dict(os.environ, {"ANTHROPIC_API_KEY": "sk-secret"}, clear=False),
            patch("solstone.think.utils.get_config", return_value=config),
        ):
            env = build_cogitate_env("anthropic")
        assert "ANTHROPIC_API_KEY" not in env

    def test_explicit_platform_strips_key(self):
        """auth.anthropic = "platform" → key removed."""
        config = {"providers": {"auth": {"anthropic": "platform"}}}
        with (
            patch.dict(os.environ, {"ANTHROPIC_API_KEY": "sk-secret"}, clear=False),
            patch("solstone.think.utils.get_config", return_value=config),
        ):
            env = build_cogitate_env("anthropic")
        assert "ANTHROPIC_API_KEY" not in env

    def test_api_key_mode_preserves_key(self):
        """auth.anthropic = "api_key" → key preserved."""
        config = {"providers": {"auth": {"anthropic": "api_key"}}}
        with (
            patch.dict(os.environ, {"ANTHROPIC_API_KEY": "sk-secret"}, clear=False),
            patch("solstone.think.utils.get_config", return_value=config),
        ):
            env = build_cogitate_env("anthropic")
        assert env["ANTHROPIC_API_KEY"] == "sk-secret"

    def test_missing_auth_section_strips_key(self):
        """No providers section at all → safe default, key removed."""
        config = {}
        with (
            patch.dict(os.environ, {"OPENAI_API_KEY": "sk-openai"}, clear=False),
            patch("solstone.think.utils.get_config", return_value=config),
        ):
            env = build_cogitate_env("openai")
        assert "OPENAI_API_KEY" not in env

    def test_other_env_vars_preserved(self):
        """Non-API-key vars are never stripped."""
        config = {"providers": {}}
        with (
            patch.dict(
                os.environ,
                {"ANTHROPIC_API_KEY": "sk-secret", "HOME": "/home/test"},
                clear=False,
            ),
            patch("solstone.think.utils.get_config", return_value=config),
        ):
            env = build_cogitate_env("anthropic")
        assert env["HOME"] == "/home/test"

    def test_per_provider_independence(self):
        """Each provider's auth mode is independent."""
        config = {
            "providers": {
                "auth": {
                    "anthropic": "api_key",
                    "openai": "platform",
                }
            }
        }
        with (
            patch.dict(
                os.environ,
                {"ANTHROPIC_API_KEY": "sk-ant", "OPENAI_API_KEY": "sk-oai"},
                clear=False,
            ),
            patch("solstone.think.utils.get_config", return_value=config),
        ):
            ant_env = build_cogitate_env("anthropic")
            oai_env = build_cogitate_env("openai")
        assert ant_env["ANTHROPIC_API_KEY"] == "sk-ant"
        assert "OPENAI_API_KEY" not in oai_env

    def test_non_google_key_unaffected_by_vertex(self):
        """Google backend settings do not affect non-Google CLI envs."""
        config = {
            "providers": {
                "google_backend": "vertex",
                "auth": {"anthropic": "api_key"},
            }
        }
        with (
            patch.dict(os.environ, {"ANTHROPIC_API_KEY": "sk-ant"}, clear=True),
            patch("solstone.think.utils.get_config", return_value=config),
        ):
            env = build_cogitate_env("anthropic")
        assert "GOOGLE_GENAI_USE_VERTEXAI" not in env
        assert env["ANTHROPIC_API_KEY"] == "sk-ant"
