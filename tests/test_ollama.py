# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Unit tests for the Ollama (Local) provider."""

import asyncio
import os
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from solstone.think.models import OLLAMA_FLASH, OLLAMA_LITE, OLLAMA_PRO


def _ollama_provider():
    import importlib

    return importlib.reload(importlib.import_module("solstone.think.providers.ollama"))


# ---------------------------------------------------------------------------
# _strip_model_prefix
# ---------------------------------------------------------------------------


class TestStripModelPrefix:
    def test_strips_ollama_local_prefix(self):
        provider = _ollama_provider()
        assert provider._strip_model_prefix("ollama-local/qwen3.5:9b") == "qwen3.5:9b"

    def test_strips_prefix_from_complex_name(self):
        provider = _ollama_provider()
        assert (
            provider._strip_model_prefix("ollama-local/qwen3.5:35b-a3b-bf16")
            == "qwen3.5:35b-a3b-bf16"
        )

    def test_no_prefix_passthrough(self):
        provider = _ollama_provider()
        assert provider._strip_model_prefix("llama3.1:8b") == "llama3.1:8b"


# ---------------------------------------------------------------------------
# _build_messages
# ---------------------------------------------------------------------------


class TestBuildMessages:
    def test_string_contents(self):
        provider = _ollama_provider()
        msgs = provider._build_messages("hello")
        assert msgs == [{"role": "user", "content": "hello"}]

    def test_string_with_system(self):
        provider = _ollama_provider()
        msgs = provider._build_messages("hello", system_instruction="be helpful")
        assert msgs == [
            {"role": "system", "content": "be helpful"},
            {"role": "user", "content": "hello"},
        ]

    def test_list_of_strings(self):
        provider = _ollama_provider()
        msgs = provider._build_messages(["line1", "line2"])
        assert msgs == [{"role": "user", "content": "line1\nline2"}]

    def test_list_of_dicts_passthrough(self):
        provider = _ollama_provider()
        input_msgs = [
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "hello"},
        ]
        msgs = provider._build_messages(input_msgs)
        assert msgs == input_msgs

    def test_list_of_dicts_with_system(self):
        provider = _ollama_provider()
        input_msgs = [{"role": "user", "content": "hi"}]
        msgs = provider._build_messages(input_msgs, system_instruction="sys")
        assert msgs == [
            {"role": "system", "content": "sys"},
            {"role": "user", "content": "hi"},
        ]

    def test_non_string_contents(self):
        provider = _ollama_provider()
        msgs = provider._build_messages(42)
        assert msgs == [{"role": "user", "content": "42"}]


# ---------------------------------------------------------------------------
# _build_request_body
# ---------------------------------------------------------------------------


class TestBuildRequestBody:
    def test_basic_body(self):
        provider = _ollama_provider()
        body = provider._build_request_body(
            model="qwen3.5:9b",
            messages=[{"role": "user", "content": "hi"}],
            temperature=0.3,
            max_output_tokens=1024,
            json_output=False,
            thinking_budget=None,
        )
        assert body["model"] == "qwen3.5:9b"
        assert body["stream"] is False
        assert body["options"]["temperature"] == 0.3
        assert body["options"]["num_predict"] == 1024
        assert body["think"] is False

    def test_thinking_enabled(self):
        provider = _ollama_provider()
        body = provider._build_request_body(
            "m",
            [{"role": "user", "content": "hi"}],
            0.3,
            1024,
            False,
            thinking_budget=4096,
        )
        assert body["think"] is True

    def test_thinking_disabled_zero(self):
        provider = _ollama_provider()
        body = provider._build_request_body(
            "m",
            [{"role": "user", "content": "hi"}],
            0.3,
            1024,
            False,
            thinking_budget=0,
        )
        assert body["think"] is False

    def test_json_output(self):
        provider = _ollama_provider()
        body = provider._build_request_body(
            "m", [{"role": "user", "content": "hi"}], 0.3, 1024, True, None
        )
        assert body["format"] == "json"

    def test_json_schema_dict(self):
        provider = _ollama_provider()
        schema = {"type": "object"}
        body = provider._build_request_body(
            "m",
            [{"role": "user", "content": "hi"}],
            0.3,
            1024,
            True,
            None,
            schema,
        )
        assert body["format"] == schema

    def test_no_json_output(self):
        provider = _ollama_provider()
        body = provider._build_request_body(
            "m", [{"role": "user", "content": "hi"}], 0.3, 1024, False, None
        )
        assert "format" not in body


# ---------------------------------------------------------------------------
# _normalize_finish_reason
# ---------------------------------------------------------------------------


class TestNormalizeFinishReason:
    def test_stop(self):
        provider = _ollama_provider()
        assert (
            provider._normalize_finish_reason({"done": True, "done_reason": "stop"})
            == "stop"
        )

    def test_length_to_max_tokens(self):
        provider = _ollama_provider()
        assert (
            provider._normalize_finish_reason({"done": True, "done_reason": "length"})
            == "max_tokens"
        )

    def test_done_no_reason(self):
        provider = _ollama_provider()
        assert provider._normalize_finish_reason({"done": True}) == "stop"

    def test_not_done(self):
        provider = _ollama_provider()
        assert provider._normalize_finish_reason({"done": False}) is None

    def test_unknown_passthrough(self):
        provider = _ollama_provider()
        assert (
            provider._normalize_finish_reason({"done": True, "done_reason": "other"})
            == "other"
        )


# ---------------------------------------------------------------------------
# _extract_usage
# ---------------------------------------------------------------------------


class TestExtractUsage:
    def test_normal_usage(self):
        provider = _ollama_provider()
        result = provider._extract_usage(
            {
                "prompt_eval_count": 100,
                "eval_count": 50,
            }
        )
        assert result == {
            "input_tokens": 100,
            "output_tokens": 50,
            "total_tokens": 150,
        }

    def test_missing_fields(self):
        provider = _ollama_provider()
        result = provider._extract_usage({})
        assert result == {
            "input_tokens": 0,
            "output_tokens": 0,
            "total_tokens": 0,
        }


# ---------------------------------------------------------------------------
# _extract_thinking
# ---------------------------------------------------------------------------


class TestExtractThinking:
    def test_with_thinking(self):
        provider = _ollama_provider()
        result = provider._extract_thinking(
            {"message": {"content": "4", "thinking": "Let me calculate..."}}
        )
        assert result == [{"summary": "Let me calculate..."}]

    def test_no_thinking(self):
        provider = _ollama_provider()
        result = provider._extract_thinking({"message": {"content": "4"}})
        assert result is None

    def test_empty_thinking(self):
        provider = _ollama_provider()
        result = provider._extract_thinking(
            {"message": {"content": "4", "thinking": "   "}}
        )
        assert result is None

    def test_no_message(self):
        provider = _ollama_provider()
        result = provider._extract_thinking({})
        assert result is None


# ---------------------------------------------------------------------------
# _parse_response
# ---------------------------------------------------------------------------


class TestParseResponse:
    def test_full_response(self):
        provider = _ollama_provider()
        data = {
            "message": {
                "role": "assistant",
                "content": "Hello!",
                "thinking": "Reasoning...",
            },
            "done": True,
            "done_reason": "stop",
            "prompt_eval_count": 25,
            "eval_count": 10,
        }
        result = provider._parse_response(data)
        assert result["text"] == "Hello!"
        assert result["finish_reason"] == "stop"
        assert result["usage"]["input_tokens"] == 25
        assert result["usage"]["output_tokens"] == 10
        assert result["thinking"] == [{"summary": "Reasoning..."}]

    def test_no_thinking(self):
        provider = _ollama_provider()
        data = {
            "message": {"role": "assistant", "content": "4"},
            "done": True,
            "done_reason": "stop",
            "prompt_eval_count": 10,
            "eval_count": 2,
        }
        result = provider._parse_response(data)
        assert result["text"] == "4"
        assert result["thinking"] is None


# ---------------------------------------------------------------------------
# run_generate
# ---------------------------------------------------------------------------


def _make_ollama_response(
    content="Hello!",
    thinking=None,
    done=True,
    done_reason="stop",
    prompt_eval_count=10,
    eval_count=5,
):
    """Build a mock native Ollama /api/chat response dict."""
    message = {"role": "assistant", "content": content}
    if thinking is not None:
        message["thinking"] = thinking
    return {
        "model": "qwen3.5:9b",
        "message": message,
        "done": done,
        "done_reason": done_reason,
        "prompt_eval_count": prompt_eval_count,
        "eval_count": eval_count,
    }


class TestRunGenerate:
    def test_basic_generation(self):
        provider = _ollama_provider()
        mock_response = MagicMock()
        mock_response.json.return_value = _make_ollama_response()
        mock_response.raise_for_status = MagicMock()

        with patch.object(provider, "_get_client") as mock_get:
            mock_client = MagicMock()
            mock_client.post.return_value = mock_response
            mock_get.return_value = mock_client

            result = provider.run_generate("hello", model=OLLAMA_FLASH)

        assert result["text"] == "Hello!"
        assert result["finish_reason"] == "stop"
        assert result["usage"]["input_tokens"] == 10

    def test_model_prefix_stripped(self):
        provider = _ollama_provider()
        mock_response = MagicMock()
        mock_response.json.return_value = _make_ollama_response()
        mock_response.raise_for_status = MagicMock()

        with patch.object(provider, "_get_client") as mock_get:
            mock_client = MagicMock()
            mock_client.post.return_value = mock_response
            mock_get.return_value = mock_client

            provider.run_generate("hello", model="ollama-local/qwen3.5:9b")

        call_kwargs = mock_client.post.call_args
        body = call_kwargs.kwargs["json"]
        assert body["model"] == "qwen3.5:9b"

    def test_thinking_enabled(self):
        provider = _ollama_provider()
        mock_response = MagicMock()
        mock_response.json.return_value = _make_ollama_response(thinking="Reasoning...")
        mock_response.raise_for_status = MagicMock()

        with patch.object(provider, "_get_client") as mock_get:
            mock_client = MagicMock()
            mock_client.post.return_value = mock_response
            mock_get.return_value = mock_client

            result = provider.run_generate(
                "hello", model=OLLAMA_FLASH, thinking_budget=4096
            )

        call_kwargs = mock_client.post.call_args
        body = call_kwargs.kwargs["json"]
        assert body["think"] is True
        assert result["thinking"] == [{"summary": "Reasoning..."}]

    def test_thinking_disabled_when_none(self):
        provider = _ollama_provider()
        mock_response = MagicMock()
        mock_response.json.return_value = _make_ollama_response()
        mock_response.raise_for_status = MagicMock()

        with patch.object(provider, "_get_client") as mock_get:
            mock_client = MagicMock()
            mock_client.post.return_value = mock_response
            mock_get.return_value = mock_client

            provider.run_generate("hello", model=OLLAMA_FLASH, thinking_budget=None)

        call_kwargs = mock_client.post.call_args
        body = call_kwargs.kwargs["json"]
        assert body["think"] is False

    def test_thinking_disabled_when_zero(self):
        provider = _ollama_provider()
        mock_response = MagicMock()
        mock_response.json.return_value = _make_ollama_response()
        mock_response.raise_for_status = MagicMock()

        with patch.object(provider, "_get_client") as mock_get:
            mock_client = MagicMock()
            mock_client.post.return_value = mock_response
            mock_get.return_value = mock_client

            provider.run_generate("hello", model=OLLAMA_FLASH, thinking_budget=0)

        call_kwargs = mock_client.post.call_args
        body = call_kwargs.kwargs["json"]
        assert body["think"] is False

    def test_json_output(self):
        provider = _ollama_provider()
        mock_response = MagicMock()
        mock_response.json.return_value = _make_ollama_response(
            content='{"key": "value"}'
        )
        mock_response.raise_for_status = MagicMock()

        with patch.object(provider, "_get_client") as mock_get:
            mock_client = MagicMock()
            mock_client.post.return_value = mock_response
            mock_get.return_value = mock_client

            provider.run_generate("hello", model=OLLAMA_FLASH, json_output=True)

        call_kwargs = mock_client.post.call_args
        body = call_kwargs.kwargs["json"]
        assert body["format"] == "json"

    def test_json_schema_dict(self):
        provider = _ollama_provider()
        mock_response = MagicMock()
        mock_response.json.return_value = _make_ollama_response(
            content='{"key": "value"}'
        )
        mock_response.raise_for_status = MagicMock()
        schema = {"type": "object"}

        with patch.object(provider, "_get_client") as mock_get:
            mock_client = MagicMock()
            mock_client.post.return_value = mock_response
            mock_get.return_value = mock_client

            provider.run_generate("hello", model=OLLAMA_FLASH, json_schema=schema)

        call_kwargs = mock_client.post.call_args
        body = call_kwargs.kwargs["json"]
        assert body["format"] == schema

    def test_system_instruction(self):
        provider = _ollama_provider()
        mock_response = MagicMock()
        mock_response.json.return_value = _make_ollama_response()
        mock_response.raise_for_status = MagicMock()

        with patch.object(provider, "_get_client") as mock_get:
            mock_client = MagicMock()
            mock_client.post.return_value = mock_response
            mock_get.return_value = mock_client

            provider.run_generate(
                "hello", model=OLLAMA_FLASH, system_instruction="be concise"
            )

        call_kwargs = mock_client.post.call_args
        body = call_kwargs.kwargs["json"]
        messages = body["messages"]
        assert messages[0] == {"role": "system", "content": "be concise"}
        assert messages[1] == {"role": "user", "content": "hello"}

    def test_structured_messages_body(self):
        provider = _ollama_provider()
        mock_response = MagicMock()
        mock_response.json.return_value = _make_ollama_response()
        mock_response.raise_for_status = MagicMock()
        input_messages = [
            {"role": "user", "content": "first"},
            {"role": "assistant", "content": "second"},
            {"role": "user", "content": "third"},
        ]

        with patch.object(provider, "_get_client") as mock_get:
            mock_client = MagicMock()
            mock_client.post.return_value = mock_response
            mock_get.return_value = mock_client

            provider.run_generate(
                input_messages,
                model=OLLAMA_FLASH,
                system_instruction="be concise",
            )

        call_kwargs = mock_client.post.call_args
        body = call_kwargs.kwargs["json"]
        assert body["messages"] == [
            {"role": "system", "content": "be concise"},
            *input_messages,
        ]

    def test_timeout(self):
        provider = _ollama_provider()
        mock_response = MagicMock()
        mock_response.json.return_value = _make_ollama_response()
        mock_response.raise_for_status = MagicMock()

        with patch.object(provider, "_get_client") as mock_get:
            mock_client = MagicMock()
            mock_client.post.return_value = mock_response
            mock_get.return_value = mock_client

            provider.run_generate("hello", model=OLLAMA_FLASH, timeout_s=30.0)

        call_kwargs = mock_client.post.call_args
        assert call_kwargs.kwargs["timeout"] == 30.0


# ---------------------------------------------------------------------------
# run_agenerate
# ---------------------------------------------------------------------------


class TestRunAgenerate:
    def test_async_generation(self):
        provider = _ollama_provider()
        mock_response = MagicMock()
        mock_response.json.return_value = _make_ollama_response()
        mock_response.raise_for_status = MagicMock()

        with patch.object(provider, "_get_async_client") as mock_get:
            mock_client = MagicMock()
            mock_client.post = AsyncMock(return_value=mock_response)
            mock_get.return_value = mock_client

            result = asyncio.run(provider.run_agenerate("hello", model=OLLAMA_FLASH))

        assert result["text"] == "Hello!"
        assert result["finish_reason"] == "stop"

    def test_async_json_schema_dict(self):
        provider = _ollama_provider()
        mock_response = MagicMock()
        mock_response.json.return_value = _make_ollama_response(
            content='{"key": "value"}'
        )
        mock_response.raise_for_status = MagicMock()
        schema = {"type": "object"}

        with patch.object(provider, "_get_async_client") as mock_get:
            mock_client = MagicMock()
            mock_client.post = AsyncMock(return_value=mock_response)
            mock_get.return_value = mock_client

            asyncio.run(
                provider.run_agenerate("hello", model=OLLAMA_FLASH, json_schema=schema)
            )

        call_kwargs = mock_client.post.call_args
        body = call_kwargs.kwargs["json"]
        assert body["format"] == schema


# ---------------------------------------------------------------------------
# _translate_opencode
# ---------------------------------------------------------------------------


def _make_test_harness():
    """Create a callback/aggregator pair for testing _translate_opencode."""
    from solstone.think.providers.cli import ThinkingAggregator
    from solstone.think.providers.shared import JSONEventCallback

    events = []
    cb = JSONEventCallback(lambda e: events.append(e))
    aggregator = ThinkingAggregator(cb, "qwen3.5:9b")
    return events, cb, aggregator


class TestTranslateOpencode:
    def test_step_start_returns_session_id(self):
        provider = _ollama_provider()
        events, cb, aggregator = _make_test_harness()
        event = {
            "type": "step_start",
            "sessionID": "ses_abc123",
            "part": {"type": "step-start"},
        }
        usage = {}

        result = provider._translate_opencode(event, aggregator, cb, usage)

        assert result == "ses_abc123"
        assert events == []

    def test_text_accumulates(self):
        provider = _ollama_provider()
        events, cb, aggregator = _make_test_harness()
        event = {
            "type": "text",
            "part": {"type": "text", "text": "Hello world"},
        }
        usage = {}

        result = provider._translate_opencode(event, aggregator, cb, usage)

        assert result is None
        assert aggregator.has_content
        assert aggregator.flush_as_result() == "Hello world"

    def test_tool_use_emits_start_and_end(self):
        provider = _ollama_provider()
        events, cb, aggregator = _make_test_harness()
        event = {
            "type": "tool_use",
            "part": {
                "type": "tool",
                "tool": "bash",
                "callID": "call_xyz",
                "state": {
                    "status": "completed",
                    "input": {"command": "echo hello"},
                    "output": "hello\n",
                },
            },
        }
        usage = {}

        result = provider._translate_opencode(event, aggregator, cb, usage)

        assert result is None
        assert len(events) == 2
        assert events[0]["event"] == "tool_start"
        assert events[0]["tool"] == "bash"
        assert events[0]["args"] == {"command": "echo hello"}
        assert events[0]["call_id"] == "call_xyz"
        assert events[1]["event"] == "tool_end"
        assert events[1]["tool"] == "bash"
        assert events[1]["result"] == "hello\n"
        assert events[1]["call_id"] == "call_xyz"

    def test_tool_use_flushes_thinking(self):
        provider = _ollama_provider()
        events, cb, aggregator = _make_test_harness()
        usage = {}

        # Accumulate some text first
        aggregator.accumulate("Let me run a command...")

        # Then a tool use event
        event = {
            "type": "tool_use",
            "part": {
                "type": "tool",
                "tool": "bash",
                "callID": "call_1",
                "state": {
                    "status": "completed",
                    "input": {"command": "ls"},
                    "output": "file.txt\n",
                },
            },
        }
        provider._translate_opencode(event, aggregator, cb, usage)

        # First event should be thinking (flushed), then tool_start, tool_end
        assert len(events) == 3
        assert events[0]["event"] == "thinking"
        assert events[0]["summary"] == "Let me run a command..."
        assert events[1]["event"] == "tool_start"
        assert events[2]["event"] == "tool_end"

    def test_step_finish_captures_usage(self):
        provider = _ollama_provider()
        events, cb, aggregator = _make_test_harness()
        event = {
            "type": "step_finish",
            "part": {
                "type": "step-finish",
                "reason": "stop",
                "tokens": {
                    "total": 14681,
                    "input": 14649,
                    "output": 32,
                    "reasoning": 0,
                    "cache": {"write": 0, "read": 0},
                },
            },
        }
        usage = {}

        result = provider._translate_opencode(event, aggregator, cb, usage)

        assert result is None
        assert usage["input_tokens"] == 14649
        assert usage["output_tokens"] == 32
        assert usage["total_tokens"] == 14681

    def test_step_finish_accumulates_usage_across_steps(self):
        provider = _ollama_provider()
        events, cb, aggregator = _make_test_harness()
        usage = {}

        # First step
        event1 = {
            "type": "step_finish",
            "part": {
                "type": "step-finish",
                "reason": "tool-calls",
                "tokens": {"total": 100, "input": 80, "output": 20, "reasoning": 0},
            },
        }
        provider._translate_opencode(event1, aggregator, cb, usage)

        # Second step
        event2 = {
            "type": "step_finish",
            "part": {
                "type": "step-finish",
                "reason": "stop",
                "tokens": {"total": 150, "input": 120, "output": 30, "reasoning": 0},
            },
        }
        provider._translate_opencode(event2, aggregator, cb, usage)

        assert usage["input_tokens"] == 200
        assert usage["output_tokens"] == 50
        assert usage["total_tokens"] == 250

    def test_unknown_event_ignored(self):
        provider = _ollama_provider()
        events, cb, aggregator = _make_test_harness()
        event = {"type": "unknown_type", "part": {}}
        usage = {}

        result = provider._translate_opencode(event, aggregator, cb, usage)

        assert result is None
        assert events == []


# ---------------------------------------------------------------------------
# run_cogitate
# ---------------------------------------------------------------------------


class TestRunCogitate:
    def test_basic_cogitate(self):
        provider = _ollama_provider()

        class MockCLIRunner:
            last_instance = None

            def __init__(self, **kwargs):
                self.kwargs = kwargs
                self.cmd = kwargs["cmd"]
                self.prompt_text = kwargs["prompt_text"]
                self.cli_session_id = "ses_test123"
                self.run = AsyncMock(return_value="test result")
                MockCLIRunner.last_instance = self

        with (
            patch("shutil.which", return_value="/usr/bin/opencode"),
            patch("solstone.think.providers.ollama.CLIRunner", MockCLIRunner),
        ):
            events = []
            asyncio.run(
                provider.run_cogitate(
                    {"prompt": "hello", "model": OLLAMA_FLASH},
                    lambda e: events.append(e),
                )
            )

        instance = MockCLIRunner.last_instance
        assert "opencode" in instance.cmd
        assert "--format" in instance.cmd
        assert "json" in instance.cmd
        assert "-m" in instance.cmd
        m_idx = instance.cmd.index("-m")
        assert instance.cmd[m_idx + 1] == "ollama/qwen3.5:9b"

        # Should emit finish event
        finish_events = [e for e in events if e.get("event") == "finish"]
        assert len(finish_events) == 1
        assert finish_events[0]["result"] == "test result"
        assert finish_events[0]["cli_session_id"] == "ses_test123"

    def test_cogitate_strips_model_prefix(self):
        provider = _ollama_provider()

        class MockCLIRunner:
            last_instance = None

            def __init__(self, **kwargs):
                self.cmd = kwargs["cmd"]
                self.prompt_text = kwargs["prompt_text"]
                self.cli_session_id = None
                self.run = AsyncMock(return_value="ok")
                MockCLIRunner.last_instance = self

        with (
            patch("shutil.which", return_value="/usr/bin/opencode"),
            patch("solstone.think.providers.ollama.CLIRunner", MockCLIRunner),
        ):
            asyncio.run(
                provider.run_cogitate(
                    {"prompt": "test", "model": "ollama-local/qwen3.5:35b-a3b-bf16"},
                    lambda e: None,
                )
            )

        cmd = MockCLIRunner.last_instance.cmd
        m_idx = cmd.index("-m")
        assert cmd[m_idx + 1] == "ollama/qwen3.5:35b-a3b-bf16"

    def test_cogitate_session_resume(self):
        provider = _ollama_provider()

        class MockCLIRunner:
            last_instance = None

            def __init__(self, **kwargs):
                self.cmd = kwargs["cmd"]
                self.prompt_text = kwargs["prompt_text"]
                self.cli_session_id = None
                self.run = AsyncMock(return_value="ok")
                MockCLIRunner.last_instance = self

        with (
            patch("shutil.which", return_value="/usr/bin/opencode"),
            patch("solstone.think.providers.ollama.CLIRunner", MockCLIRunner),
        ):
            asyncio.run(
                provider.run_cogitate(
                    {
                        "prompt": "continue",
                        "model": OLLAMA_FLASH,
                        "session_id": "ses_previous",
                    },
                    lambda e: None,
                )
            )

        cmd = MockCLIRunner.last_instance.cmd
        assert "--session" in cmd
        s_idx = cmd.index("--session")
        assert cmd[s_idx + 1] == "ses_previous"

    def test_cogitate_prepends_system_instruction(self):
        provider = _ollama_provider()

        class MockCLIRunner:
            last_instance = None

            def __init__(self, **kwargs):
                self.prompt_text = kwargs["prompt_text"]
                self.cmd = kwargs["cmd"]
                self.cli_session_id = None
                self.run = AsyncMock(return_value="ok")
                MockCLIRunner.last_instance = self

        with (
            patch("shutil.which", return_value="/usr/bin/opencode"),
            patch("solstone.think.providers.ollama.CLIRunner", MockCLIRunner),
        ):
            asyncio.run(
                provider.run_cogitate(
                    {
                        "prompt": "user prompt",
                        "system_instruction": "be helpful",
                        "model": OLLAMA_FLASH,
                    },
                    lambda e: None,
                )
            )

        prompt = MockCLIRunner.last_instance.prompt_text
        assert prompt.startswith("be helpful")
        assert "user prompt" in prompt

    def test_cogitate_emits_error_on_failure(self):
        provider = _ollama_provider()

        class MockCLIRunner:
            def __init__(self, **kwargs):
                self.cmd = kwargs["cmd"]
                self.prompt_text = kwargs["prompt_text"]
                self.cli_session_id = None
                self.run = AsyncMock(side_effect=RuntimeError("CLI not found"))

        events = []
        with (
            patch("shutil.which", return_value="/usr/bin/opencode"),
            patch("solstone.think.providers.ollama.CLIRunner", MockCLIRunner),
        ):
            with pytest.raises(RuntimeError, match="CLI not found"):
                asyncio.run(
                    provider.run_cogitate(
                        {"prompt": "test", "model": OLLAMA_FLASH},
                        lambda e: events.append(e),
                    )
                )

        error_events = [e for e in events if e.get("event") == "error"]
        assert len(error_events) == 1
        assert "CLI not found" in error_events[0]["error"]

    def test_cogitate_raises_when_opencode_not_installed(self):
        provider = _ollama_provider()

        events = []
        with patch("shutil.which", return_value=None):
            with pytest.raises(RuntimeError, match="Cogitate requires OpenCode CLI"):
                asyncio.run(
                    provider.run_cogitate(
                        {"prompt": "test", "model": OLLAMA_FLASH},
                        lambda e: events.append(e),
                    )
                )

        error_events = [e for e in events if e.get("event") == "error"]
        assert len(error_events) == 1
        assert "OpenCode CLI" in error_events[0]["error"]


# ---------------------------------------------------------------------------
# _build_opencode_env
# ---------------------------------------------------------------------------


class TestBuildOpencodeEnv:
    def test_sets_api_key_placeholder(self):
        provider = _ollama_provider()
        with patch.dict(os.environ, {}, clear=True):
            env = provider._build_opencode_env()
        assert env.get("OPENAI_API_KEY") == "ollama"

    def test_preserves_existing_api_key(self):
        provider = _ollama_provider()
        with patch.dict(os.environ, {"OPENAI_API_KEY": "real-key"}, clear=False):
            env = provider._build_opencode_env()
        assert env["OPENAI_API_KEY"] == "real-key"


# ---------------------------------------------------------------------------
# list_models / validate_key
# ---------------------------------------------------------------------------


class TestListModels:
    def test_returns_model_list(self):
        provider = _ollama_provider()
        mock_response = MagicMock()
        mock_response.json.return_value = {
            "models": [
                {"name": "qwen3.5:9b", "size": 6600000000},
                {"name": "llama3.1:8b", "size": 4900000000},
            ]
        }
        mock_response.raise_for_status = MagicMock()

        with patch.object(provider, "_get_client") as mock_get:
            mock_client = MagicMock()
            mock_client.get.return_value = mock_response
            mock_get.return_value = mock_client

            result = provider.list_models("ollama")

        assert len(result) == 2
        assert result[0]["name"] == "qwen3.5:9b"


class TestValidateKey:
    def test_reachable(self):
        provider = _ollama_provider()

        with patch("httpx.get") as mock_get:
            mock_response = MagicMock()
            mock_response.raise_for_status = MagicMock()
            mock_response.json.return_value = {"version": "0.18.3"}
            mock_get.return_value = mock_response

            result = provider.validate_key("ollama", "ignored")

        assert result == {"valid": True}

    def test_unreachable(self):
        provider = _ollama_provider()

        with patch("httpx.get") as mock_get:
            mock_get.side_effect = httpx.ConnectError("Connection refused")

            result = provider.validate_key("ollama", "ignored")

        assert result["valid"] is False
        assert "Connection refused" in result["error"]


# ---------------------------------------------------------------------------
# Model constants
# ---------------------------------------------------------------------------

import httpx


class TestModelConstants:
    def test_default_models_have_prefix(self):
        assert OLLAMA_PRO.startswith("ollama-local/")
        assert OLLAMA_FLASH.startswith("ollama-local/")
        assert OLLAMA_LITE.startswith("ollama-local/")

    def test_get_model_provider(self):
        from solstone.think.models import get_model_provider

        assert get_model_provider(OLLAMA_PRO) == "ollama"
        assert get_model_provider(OLLAMA_FLASH) == "ollama"
        assert get_model_provider(OLLAMA_LITE) == "ollama"

    def test_provider_defaults_exist(self):
        from solstone.think.models import PROVIDER_DEFAULTS

        assert "ollama" in PROVIDER_DEFAULTS
        assert 1 in PROVIDER_DEFAULTS["ollama"]
        assert 2 in PROVIDER_DEFAULTS["ollama"]
        assert 3 in PROVIDER_DEFAULTS["ollama"]

    def test_calc_token_cost_zero(self):
        from solstone.think.models import calc_token_cost

        result = calc_token_cost(
            {
                "model": OLLAMA_FLASH,
                "usage": {"input_tokens": 100, "output_tokens": 50},
            }
        )
        assert result is not None
        assert result["total_cost"] == 0.0

    def test_provider_registry(self):
        from solstone.think.providers import PROVIDER_METADATA, PROVIDER_REGISTRY

        assert "ollama" in PROVIDER_REGISTRY
        assert "ollama" in PROVIDER_METADATA
        assert PROVIDER_METADATA["ollama"]["label"] == "Ollama (Local)"
        assert PROVIDER_METADATA["ollama"]["env_key"] == ""
