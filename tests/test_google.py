# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import asyncio
import importlib
import json
import sys
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock

from solstone.think.models import GEMINI_FLASH
from solstone.think.providers import google as google_provider
from solstone.think.providers.google import (
    _extract_finish_reason,
    _format_completion_message,
)
from tests.conftest import setup_google_genai_stub


async def run_main(mod, argv, stdin_data=None):
    sys.argv = argv
    if stdin_data:
        import io

        sys.stdin = io.StringIO(stdin_data)
    await mod.main_async()


def _assert_structured_contents(contents):
    assert [content.role for content in contents] == ["user", "model", "user"]
    assert [[part.text for part in content.parts] for content in contents] == [
        ["first"],
        ["second"],
        ["third"],
    ]


def test_google_main(monkeypatch, tmp_path, capsys):
    mod = importlib.reload(importlib.import_module("solstone.think.talents"))

    journal = tmp_path / "journal"
    journal.mkdir()

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    async def fake_run_cogitate(config, on_event=None):
        if on_event:
            on_event({"event": "text_delta", "delta": "ok"})
            on_event({"event": "finish", "result": "ok"})
        return "ok"

    monkeypatch.setattr(google_provider, "run_cogitate", fake_run_cogitate)

    ndjson_input = json.dumps(
        {
            "name": "exec",
            "prompt": "hello",
            "provider": "google",
            "model": GEMINI_FLASH,
            "tools": ["search_insights"],
        }
    )
    asyncio.run(run_main(mod, ["sol think.talents"], stdin_data=ndjson_input))

    out_lines = capsys.readouterr().out.strip().splitlines()
    events = [json.loads(line) for line in out_lines]
    assert events[0]["event"] == "start"
    assert isinstance(events[0]["ts"], int)
    assert "hello" in events[0]["prompt"]
    assert events[0]["name"] == "exec"
    assert events[0]["model"] == GEMINI_FLASH
    assert events[-1]["event"] == "finish"
    assert isinstance(events[-1]["ts"], int)
    assert events[-1]["result"] == "ok"

    # Journal logging is now handled by cortex, not by agents directly
    # So we don't check for journal files here


# ---------------------------------------------------------------------------
# Tests for finish reason extraction and formatting
# ---------------------------------------------------------------------------


def test_extract_finish_reason_with_enum():
    """Test extracting finish_reason from enum-style response."""

    class MockEnum:
        name = "STOP"

    candidate = SimpleNamespace(finish_reason=MockEnum())
    response = SimpleNamespace(candidates=[candidate])
    assert _extract_finish_reason(response) == "STOP"


def test_extract_finish_reason_with_string():
    """Test extracting finish_reason when it's already a string."""
    candidate = SimpleNamespace(finish_reason="MAX_TOKENS")
    response = SimpleNamespace(candidates=[candidate])
    assert _extract_finish_reason(response) == "MAX_TOKENS"


def test_extract_finish_reason_no_candidates():
    """Test extracting finish_reason when no candidates exist."""
    response = SimpleNamespace(candidates=[])
    assert _extract_finish_reason(response) is None

    response = SimpleNamespace()
    assert _extract_finish_reason(response) is None


def test_format_completion_message_stop_with_tools():
    """Test message for STOP with tool calls."""
    msg = _format_completion_message("STOP", had_tool_calls=True)
    assert msg == "Completed via tools."


def test_format_completion_message_stop_no_tools():
    """Test message for STOP without tool calls."""
    msg = _format_completion_message("STOP", had_tool_calls=False)
    assert msg == "Completed."


def test_format_completion_message_max_tokens():
    """Test message for MAX_TOKENS finish reason."""
    msg = _format_completion_message("MAX_TOKENS", had_tool_calls=False)
    assert msg == "Reached token limit."


def test_format_completion_message_safety():
    """Test message for safety-related finish reasons."""
    msg = _format_completion_message("SAFETY", had_tool_calls=False)
    assert msg == "Blocked by safety filters."

    msg = _format_completion_message("PROHIBITED_SAFETY", had_tool_calls=False)
    assert msg == "Blocked by safety filters."


def test_format_completion_message_tool_errors():
    """Test message for tool-related error finish reasons."""
    msg = _format_completion_message("UNEXPECTED_TOOL_CALL", had_tool_calls=True)
    assert msg == "Tool execution incomplete."

    msg = _format_completion_message("MALFORMED_FUNCTION_CALL", had_tool_calls=False)
    assert msg == "Tool execution incomplete."


def test_format_completion_message_unknown():
    """Test message for unknown finish reasons."""
    msg = _format_completion_message("SOME_NEW_REASON", had_tool_calls=False)
    assert msg == "Completed (some_new_reason)."


def test_format_completion_message_none():
    """Test message when finish_reason is None."""
    msg = _format_completion_message(None, had_tool_calls=False)
    assert msg == "Completed (unknown)."


class TestRunGenerateJsonSchema:
    def test_structured_messages_sync_mapped_to_google_contents(self, monkeypatch):
        setup_google_genai_stub(monkeypatch, with_thinking=False)
        sys.modules.pop("solstone.think.providers.google", None)
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.google")
        )

        mock_client = MagicMock()
        mock_client.models.generate_content.return_value = SimpleNamespace(
            text="[]",
            candidates=[],
            usage_metadata=None,
        )
        monkeypatch.setattr(
            provider, "get_or_create_client", lambda _client=None: mock_client
        )
        messages = [
            {"role": "user", "content": "first"},
            {"role": "assistant", "content": "second"},
            {"role": "user", "content": "third"},
        ]

        provider.run_generate(messages, model=GEMINI_FLASH)

        contents = mock_client.models.generate_content.call_args.kwargs["contents"]
        _assert_structured_contents(contents)

    def test_structured_messages_async_mapped_to_google_contents(self, monkeypatch):
        setup_google_genai_stub(monkeypatch, with_thinking=False)
        sys.modules.pop("solstone.think.providers.google", None)
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.google")
        )

        mock_client = MagicMock()
        mock_client.aio.models.generate_content = AsyncMock(
            return_value=SimpleNamespace(
                text="[]",
                candidates=[],
                usage_metadata=None,
            )
        )
        monkeypatch.setattr(
            provider, "get_or_create_client", lambda _client=None: mock_client
        )
        messages = [
            {"role": "user", "content": "first"},
            {"role": "assistant", "content": "second"},
            {"role": "user", "content": "third"},
        ]

        asyncio.run(provider.run_agenerate(messages, model=GEMINI_FLASH))

        contents = mock_client.aio.models.generate_content.call_args.kwargs["contents"]
        _assert_structured_contents(contents)

    def test_no_schema_kwargs_unchanged(self, monkeypatch):
        setup_google_genai_stub(monkeypatch, with_thinking=False)
        sys.modules.pop("solstone.think.providers.google", None)
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.google")
        )

        mock_client = MagicMock()
        mock_client.models.generate_content.return_value = SimpleNamespace(
            text="[]",
            candidates=[],
            usage_metadata=None,
        )
        monkeypatch.setattr(
            provider, "get_or_create_client", lambda _client=None: mock_client
        )

        provider.run_generate("hello", model=GEMINI_FLASH, json_output=True)

        config = mock_client.models.generate_content.call_args.kwargs["config"]
        assert config.response_mime_type == "application/json"
        assert getattr(config, "response_json_schema", None) is None

    def test_with_schema_adds_json_schema(self, monkeypatch):
        setup_google_genai_stub(monkeypatch, with_thinking=False)
        sys.modules.pop("solstone.think.providers.google", None)
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.google")
        )

        schema = {"type": "object"}
        mock_client = MagicMock()
        mock_client.models.generate_content.return_value = SimpleNamespace(
            text="[]",
            candidates=[],
            usage_metadata=None,
        )
        monkeypatch.setattr(
            provider, "get_or_create_client", lambda _client=None: mock_client
        )

        provider.run_generate(
            "hello", model=GEMINI_FLASH, json_output=True, json_schema=schema
        )

        config = mock_client.models.generate_content.call_args.kwargs["config"]
        assert config.response_mime_type == "application/json"
        assert config.response_json_schema == schema

    def test_async_no_schema_kwargs_unchanged(self, monkeypatch):
        setup_google_genai_stub(monkeypatch, with_thinking=False)
        sys.modules.pop("solstone.think.providers.google", None)
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.google")
        )

        mock_client = MagicMock()
        mock_client.aio.models.generate_content = AsyncMock(
            return_value=SimpleNamespace(
                text="[]",
                candidates=[],
                usage_metadata=None,
            )
        )
        monkeypatch.setattr(
            provider, "get_or_create_client", lambda _client=None: mock_client
        )

        asyncio.run(
            provider.run_agenerate("hello", model=GEMINI_FLASH, json_output=True)
        )

        config = mock_client.aio.models.generate_content.call_args.kwargs["config"]
        assert config.response_mime_type == "application/json"
        assert getattr(config, "response_json_schema", None) is None

    def test_async_with_schema_adds_json_schema(self, monkeypatch):
        setup_google_genai_stub(monkeypatch, with_thinking=False)
        sys.modules.pop("solstone.think.providers.google", None)
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.google")
        )

        schema = {"type": "object"}
        mock_client = MagicMock()
        mock_client.aio.models.generate_content = AsyncMock(
            return_value=SimpleNamespace(
                text="[]",
                candidates=[],
                usage_metadata=None,
            )
        )
        monkeypatch.setattr(
            provider, "get_or_create_client", lambda _client=None: mock_client
        )

        asyncio.run(
            provider.run_agenerate(
                "hello",
                model=GEMINI_FLASH,
                json_output=True,
                json_schema=schema,
            )
        )

        config = mock_client.aio.models.generate_content.call_args.kwargs["config"]
        assert config.response_mime_type == "application/json"
        assert config.response_json_schema == schema
