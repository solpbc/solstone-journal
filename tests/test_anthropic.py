# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import asyncio
import base64
import importlib
import io
import json
import sys
import types
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock

import pytest
from PIL import Image

from solstone.think.models import (
    CLAUDE_SONNET_4,
    IncompleteJSONError,
    _validate_json_response,
)


async def run_main(mod, argv, stdin_data=None):
    sys.argv = argv
    if stdin_data:
        import io

        sys.stdin = io.StringIO(stdin_data)
    await mod.main_async()


def _png_bytes(size: tuple[int, int] = (4, 3)) -> bytes:
    image = Image.new("RGB", size, color="red")
    buf = io.BytesIO()
    image.save(buf, format="PNG", compress_level=1)
    return buf.getvalue()


def _decoded_image(b64: str) -> Image.Image:
    return Image.open(io.BytesIO(base64.b64decode(b64)))


class DummyMessages:
    async def create(self, **kwargs):
        DummyMessages.kwargs = kwargs
        return SimpleNamespace(content=[SimpleNamespace(type="text", text="ok")])


class MockThinkingBlock:
    """Mock ThinkingBlock that passes isinstance checks."""

    type = "thinking"

    def __init__(self, thinking: str, signature: str = "mock-signature"):
        self.thinking = thinking
        self.signature = signature


class MockRedactedThinkingBlock:
    """Mock RedactedThinkingBlock that passes isinstance checks."""

    type = "redacted_thinking"

    def __init__(self, data: str):
        self.data = data


class DummyMessagesWithThinking:
    async def create(self, **kwargs):
        DummyMessagesWithThinking.kwargs = kwargs
        # Return response with both thinking and text content
        return SimpleNamespace(
            content=[
                MockThinkingBlock("I'm thinking about this...", "test-signature-123"),
                SimpleNamespace(type="text", text="ok"),
            ]
        )


class DummyMessagesWithRedactedThinking:
    async def create(self, **kwargs):
        DummyMessagesWithRedactedThinking.kwargs = kwargs
        # Return response with redacted thinking
        return SimpleNamespace(
            content=[
                MockRedactedThinkingBlock("encrypted-data-xyz"),
                SimpleNamespace(type="text", text="ok"),
            ]
        )


class DummyMessagesError:
    async def create(self, **kwargs):
        DummyMessagesError.kwargs = kwargs
        raise Exception("boo")


def _setup_anthropic_stub(
    monkeypatch, error=False, with_thinking=False, with_redacted_thinking=False
):
    # Create mock Anthropic client
    anthropic_stub = types.ModuleType("anthropic")
    anthropic_constants_stub = types.ModuleType("anthropic._constants")
    anthropic_types_stub = types.ModuleType("anthropic.types")

    class DummyClient:
        def __init__(self, **kwargs):
            if with_redacted_thinking:
                self.messages = DummyMessagesWithRedactedThinking()
            elif with_thinking:
                self.messages = DummyMessagesWithThinking()
            elif error:
                self.messages = DummyMessagesError()
            else:
                self.messages = DummyMessages()

    class DummyBadRequestError(Exception):
        pass

    anthropic_stub.Anthropic = DummyClient
    anthropic_stub.AsyncAnthropic = DummyClient  # Add async version
    anthropic_stub.BadRequestError = DummyBadRequestError
    anthropic_constants_stub.MODEL_NONSTREAMING_TOKENS = {
        "claude-opus-4-20250514": 8192,
        "claude-opus-4-0": 8192,
        "claude-4-opus-20250514": 8192,
        "anthropic.claude-opus-4-20250514-v1:0": 8192,
        "claude-opus-4@20250514": 8192,
        "claude-opus-4-1-20250805": 8192,
        "anthropic.claude-opus-4-1-20250805-v1:0": 8192,
        "claude-opus-4-1@20250805": 8192,
    }

    # Add types to the types module
    anthropic_types_stub.Message = SimpleNamespace
    anthropic_types_stub.MessageParam = dict
    anthropic_types_stub.ToolParam = dict
    anthropic_types_stub.ToolUseBlock = SimpleNamespace
    # Use our mock classes for isinstance checks
    anthropic_types_stub.ThinkingBlock = MockThinkingBlock
    anthropic_types_stub.RedactedThinkingBlock = MockRedactedThinkingBlock

    # Add types as a submodule
    anthropic_stub.types = anthropic_types_stub

    # These anthropic* entries must be installed via monkeypatch.setitem so the
    # stub is unconditionally torn down. Raw sys.modules assignments leak across
    # workers and can cross-skip tests/test_provider_error_classification.py
    # under nondeterministic xdist distribution.
    monkeypatch.setitem(sys.modules, "anthropic", anthropic_stub)
    monkeypatch.setitem(sys.modules, "anthropic._constants", anthropic_constants_stub)
    monkeypatch.setitem(sys.modules, "anthropic.types", anthropic_types_stub)


def _setup_claude_cli_stub(
    monkeypatch,
    provider_mod,
    *,
    error=False,
    with_thinking=False,
    with_redacted_thinking=False,
):
    monkeypatch.setattr(
        provider_mod.bundled,
        "resolve_bundled_binary",
        lambda _name: Path("/usr/bin/claude"),
    )

    class DummyCLIRunner:
        def __init__(
            self,
            cmd,
            prompt_text,
            translate,
            callback,
            aggregator,
            cwd=None,
            env=None,
            timeout=600,
        ):
            self.translate = translate
            self.callback = callback
            self.aggregator = aggregator
            self.cli_session_id = None

        async def run(self):
            if error:
                raise RuntimeError("boo")

            raw_events = [
                {
                    "type": "system",
                    "subtype": "init",
                    "session_id": "test-session-abc123",
                }
            ]
            if with_thinking:
                raw_events.append(
                    {
                        "type": "assistant",
                        "message": {
                            "content": [
                                {
                                    "type": "thinking",
                                    "thinking": "I'm thinking about this...",
                                }
                            ]
                        },
                    }
                )
            if with_redacted_thinking:
                raw_events.append(
                    {
                        "type": "assistant",
                        "message": {
                            "content": [{"type": "thinking", "thinking": "[redacted]"}]
                        },
                    }
                )
            raw_events.append(
                {
                    "type": "assistant",
                    "message": {"content": [{"type": "text", "text": "ok"}]},
                }
            )

            for raw_event in raw_events:
                session_id = self.translate(raw_event, self.aggregator, self.callback)
                if session_id:
                    self.cli_session_id = session_id

            return self.aggregator.flush_as_result()

    monkeypatch.setattr(provider_mod, "CLIRunner", DummyCLIRunner)


def _setup_openhands_cogitate_stub(
    monkeypatch,
    *,
    error=False,
    with_thinking=False,
    with_redacted_thinking=False,
):
    from solstone.think.providers import openhands as openhands_provider

    async def fake_run_cogitate(config, on_event=None):
        if error:
            raise RuntimeError("boo")
        if on_event:
            if with_thinking:
                on_event(
                    {
                        "event": "thinking",
                        "summary": "I'm thinking about this...",
                        "signature": "test-signature-123",
                        "redacted_data": None,
                        "model": config.get("model"),
                    }
                )
            if with_redacted_thinking:
                on_event(
                    {
                        "event": "thinking",
                        "summary": "[redacted]",
                        "signature": None,
                        "redacted_data": "encrypted-data-xyz",
                        "model": config.get("model"),
                    }
                )
            on_event({"event": "text_delta", "delta": "ok"})
            on_event({"event": "finish", "result": "ok"})
        return "ok"

    monkeypatch.setattr(openhands_provider, "run_cogitate", fake_run_cogitate)


def test_claude_main(monkeypatch, tmp_path, capsys):
    _setup_openhands_cogitate_stub(monkeypatch)
    mod = importlib.reload(importlib.import_module("solstone.think.talents"))

    journal = tmp_path / "journal"
    journal.mkdir()
    agents_dir = journal / "talents"
    agents_dir.mkdir()

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setenv("ANTHROPIC_API_KEY", "x")

    ndjson_input = json.dumps(
        {
            "name": "exec",
            "prompt": "hello",
            "provider": "anthropic",
            "model": CLAUDE_SONNET_4,
            "tools": ["search_insights"],
        }
    )
    asyncio.run(run_main(mod, ["sol solstone.think.talents"], stdin_data=ndjson_input))

    out_lines = capsys.readouterr().out.strip().splitlines()
    events = [json.loads(line) for line in out_lines]
    assert events[0]["event"] == "start"
    assert isinstance(events[0]["ts"], int)
    # Prompt includes system instruction prepended during enrichment
    assert "hello" in events[0]["prompt"]
    assert events[0]["name"] == "exec"
    assert events[0]["model"] == CLAUDE_SONNET_4
    assert events[-1]["event"] == "finish"
    assert isinstance(events[-1]["ts"], int)
    assert events[-1]["result"] == "ok"

    # Journal logging is now handled by cortex, not by agents directly
    # So we don't check for journal files here


def test_claude_outfile(monkeypatch, tmp_path, capsys):
    _setup_openhands_cogitate_stub(monkeypatch)
    mod = importlib.reload(importlib.import_module("solstone.think.talents"))

    journal = tmp_path / "journal"
    journal.mkdir()
    agents_dir = journal / "talents"
    agents_dir.mkdir()

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setenv("ANTHROPIC_API_KEY", "x")

    ndjson_input = json.dumps(
        {
            "name": "exec",
            "prompt": "hello",
            "provider": "anthropic",
            "model": CLAUDE_SONNET_4,
            "tools": ["search_insights"],
        }
    )
    asyncio.run(run_main(mod, ["sol solstone.think.talents"], stdin_data=ndjson_input))

    # Output file functionality was removed in NDJSON-only mode
    # Check stdout instead
    out_lines = capsys.readouterr().out.strip().splitlines()
    events = [json.loads(line) for line in out_lines]
    assert events[0]["event"] == "start"
    assert isinstance(events[0]["ts"], int)
    # Prompt includes system instruction prepended during enrichment
    assert "hello" in events[0]["prompt"]
    assert events[0]["name"] == "exec"
    assert events[0]["model"] == CLAUDE_SONNET_4
    assert events[-1]["event"] == "finish"
    assert isinstance(events[-1]["ts"], int)
    assert events[-1]["result"] == "ok"

    # Journal logging is now handled by cortex, not by agents directly
    # So we don't check for journal files here


def test_claude_thinking_events(monkeypatch, tmp_path, capsys):
    """Test that thinking events are properly emitted for Claude models."""
    _setup_openhands_cogitate_stub(monkeypatch, with_thinking=True)
    mod = importlib.reload(importlib.import_module("solstone.think.talents"))

    journal = tmp_path / "journal"
    journal.mkdir()
    agents_dir = journal / "talents"
    agents_dir.mkdir()

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setenv("ANTHROPIC_API_KEY", "x")

    ndjson_input = json.dumps(
        {
            "name": "exec",
            "prompt": "hello",
            "provider": "anthropic",
            "model": CLAUDE_SONNET_4,
            "tools": ["search_insights"],
        }
    )
    asyncio.run(run_main(mod, ["sol solstone.think.talents"], stdin_data=ndjson_input))

    out_lines = capsys.readouterr().out.strip().splitlines()
    events = [json.loads(line) for line in out_lines]

    # Check for thinking event
    thinking_events = [e for e in events if e.get("event") == "thinking"]
    assert len(thinking_events) == 1
    assert "I'm thinking about this..." in thinking_events[0]["summary"]

    # Check that regular events are still present
    assert events[0]["event"] == "start"
    assert events[-1]["event"] == "finish"
    assert events[-1]["result"] == "ok"


def test_claude_redacted_thinking_events(monkeypatch, tmp_path, capsys):
    """Test that redacted thinking events are properly handled."""
    _setup_openhands_cogitate_stub(monkeypatch, with_redacted_thinking=True)
    mod = importlib.reload(importlib.import_module("solstone.think.talents"))

    journal = tmp_path / "journal"
    journal.mkdir()
    agents_dir = journal / "talents"
    agents_dir.mkdir()

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setenv("ANTHROPIC_API_KEY", "x")

    ndjson_input = json.dumps(
        {
            "name": "exec",
            "prompt": "hello",
            "provider": "anthropic",
            "model": CLAUDE_SONNET_4,
            "tools": ["search_insights"],
        }
    )
    asyncio.run(run_main(mod, ["sol solstone.think.talents"], stdin_data=ndjson_input))

    out_lines = capsys.readouterr().out.strip().splitlines()
    events = [json.loads(line) for line in out_lines]

    # Check for redacted thinking event
    thinking_events = [e for e in events if e.get("event") == "thinking"]
    assert len(thinking_events) == 1
    assert thinking_events[0]["summary"] == "[redacted]"

    # Check that regular events are still present
    assert events[0]["event"] == "start"
    assert events[-1]["event"] == "finish"


def test_claude_outfile_error(monkeypatch, tmp_path, capsys):
    _setup_openhands_cogitate_stub(monkeypatch, error=True)
    mod = importlib.reload(importlib.import_module("solstone.think.talents"))

    journal = tmp_path / "journal"
    journal.mkdir()
    agents_dir = journal / "talents"
    agents_dir.mkdir()

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setenv("ANTHROPIC_API_KEY", "x")

    ndjson_input = json.dumps(
        {
            "name": "exec",
            "prompt": "hello",
            "provider": "anthropic",
            "model": CLAUDE_SONNET_4,
            "tools": ["search_insights"],
        }
    )
    asyncio.run(run_main(mod, ["sol solstone.think.talents"], stdin_data=ndjson_input))

    # Error events should be written to stdout
    out_lines = capsys.readouterr().out.strip().splitlines()
    if out_lines:  # May be empty if error is raised before any output
        events = [json.loads(line) for line in out_lines if line]
        if events:
            assert any(e["event"] == "error" for e in events)


class TestRunGenerateJsonSchema:
    def test_run_generate_records_resolved_model_version(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        mock_response = SimpleNamespace(
            content=[SimpleNamespace(type="text", text="ok")],
            usage=SimpleNamespace(input_tokens=10, output_tokens=5),
            stop_reason="end_turn",
            model="claude-haiku-4-5-20251001",
        )
        mock_client.messages.create.return_value = mock_response
        monkeypatch.setattr(provider, "_get_anthropic_client", lambda: mock_client)

        result = provider.run_generate(
            "hello",
            model="claude-haiku-4-5",
            max_output_tokens=100,
        )

        assert result["model"] == "claude-haiku-4-5-20251001"
        assert result["usage"]["model_version"] == "claude-haiku-4-5-20251001"

    def test_run_generate_model_version_falls_back_to_requested(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        mock_response = SimpleNamespace(
            content=[SimpleNamespace(type="text", text="ok")],
            usage=SimpleNamespace(input_tokens=10, output_tokens=5),
            stop_reason="end_turn",
        )
        mock_client.messages.create.return_value = mock_response
        monkeypatch.setattr(provider, "_get_anthropic_client", lambda: mock_client)

        result = provider.run_generate(
            "hello",
            model="claude-haiku-4-5",
            max_output_tokens=100,
        )

        assert result["model"] == "claude-haiku-4-5"
        assert "model_version" not in result["usage"]

    def test_translate_claude_captures_resolved_model_from_assistant_event(self):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        result_meta = {}

        provider._translate_claude(
            {
                "type": "assistant",
                "message": {
                    "model": "claude-haiku-4-5-20251001",
                    "content": [],
                },
            },
            MagicMock(),
            MagicMock(),
            {},
            result_meta,
        )
        provider._translate_claude(
            {
                "type": "result",
                "usage": {"input_tokens": 10, "output_tokens": 5},
                "total_cost_usd": 0.01,
            },
            MagicMock(),
            MagicMock(),
            {},
            result_meta,
        )

        assert result_meta["usage"]["model_version"] == "claude-haiku-4-5-20251001"

        result_meta = {}
        provider._translate_claude(
            {"type": "assistant", "message": {"content": []}},
            MagicMock(),
            MagicMock(),
            {},
            result_meta,
        )
        provider._translate_claude(
            {
                "type": "result",
                "usage": {"input_tokens": 10, "output_tokens": 5},
                "total_cost_usd": 0.01,
            },
            MagicMock(),
            MagicMock(),
            {},
            result_meta,
        )

        assert "model_version" not in result_meta["usage"]

    def test_structured_messages_passthrough(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        mock_response = MagicMock()
        mock_response.content = [SimpleNamespace(type="text", text="ok")]
        mock_response.usage = None
        mock_response.stop_reason = "end_turn"
        mock_client.messages.create.return_value = mock_response
        monkeypatch.setattr(provider, "_get_anthropic_client", lambda: mock_client)
        messages = [
            {"role": "user", "content": "first"},
            {"role": "assistant", "content": "second"},
            {"role": "user", "content": "third"},
        ]

        provider.run_generate(messages, system_instruction="base")

        call_kwargs = mock_client.messages.create.call_args.kwargs
        assert call_kwargs["messages"] == messages
        assert call_kwargs["system"] == "base"

    def test_image_parts_build_anthropic_blocks(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        mock_response = MagicMock()
        mock_response.content = [SimpleNamespace(type="text", text="ok")]
        mock_response.usage = None
        mock_response.stop_reason = "end_turn"
        mock_client.messages.create.return_value = mock_response
        monkeypatch.setattr(provider, "_get_anthropic_client", lambda: mock_client)
        image = Image.new("RGB", (5, 4), color="blue")

        provider.run_generate(["before", image, "after"])

        content = mock_client.messages.create.call_args.kwargs["messages"][0]["content"]
        assert [block["type"] for block in content] == ["text", "image", "text"]
        assert content[0]["text"] == "before"
        assert content[2]["text"] == "after"
        source = content[1]["source"]
        assert source["type"] == "base64"
        assert source["media_type"] == "image/png"
        decoded = _decoded_image(source["data"])
        assert decoded.size == image.size
        assert decoded.format == "PNG"

    def test_png_bytes_part_builds_anthropic_image_block(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        mock_response = MagicMock()
        mock_response.content = [SimpleNamespace(type="text", text="ok")]
        mock_response.usage = None
        mock_response.stop_reason = "end_turn"
        mock_client.messages.create.return_value = mock_response
        monkeypatch.setattr(provider, "_get_anthropic_client", lambda: mock_client)
        data = _png_bytes((6, 3))

        provider.run_generate(["prompt", data])

        content = mock_client.messages.create.call_args.kwargs["messages"][0]["content"]
        source = content[1]["source"]
        assert source["media_type"] == "image/png"
        decoded = _decoded_image(source["data"])
        assert decoded.size == (6, 3)
        assert decoded.format == "PNG"

    def test_bad_bytes_raise_before_create(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        monkeypatch.setattr(provider, "_get_anthropic_client", lambda: mock_client)

        with pytest.raises(ValueError) as exc_info:
            provider.run_generate(["prompt", b"not-an-image"])

        assert "bytes" in str(exc_info.value)
        assert "not-an-image" in str(exc_info.value)
        assert mock_client.messages.create.call_count == 0

    def test_cmyk_image_raises_before_create(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        monkeypatch.setattr(provider, "_get_anthropic_client", lambda: mock_client)
        image = Image.new("CMYK", (2, 2))

        with pytest.raises(ValueError) as exc_info:
            provider.run_generate(["prompt", image])

        assert "Image" in str(exc_info.value)
        assert "CMYK" in str(exc_info.value)
        assert mock_client.messages.create.call_count == 0

    def test_no_schema_keeps_prompt_append(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        mock_response = MagicMock()
        mock_response.content = [SimpleNamespace(type="text", text="{}")]
        mock_response.usage = None
        mock_response.stop_reason = "end_turn"
        mock_client.messages.create.return_value = mock_response
        monkeypatch.setattr(provider, "_get_anthropic_client", lambda: mock_client)

        provider.run_generate(
            "hello",
            json_output=True,
            system_instruction="base",
        )

        call_kwargs = mock_client.messages.create.call_args.kwargs
        assert call_kwargs["system"].endswith(
            "Respond with valid JSON only. No explanation or markdown."
        )

    def test_with_schema_uses_output_config(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        mock_response = MagicMock()
        mock_response.content = [SimpleNamespace(type="text", text="{}")]
        mock_response.usage = None
        mock_response.stop_reason = "end_turn"
        mock_client.messages.create.return_value = mock_response
        monkeypatch.setattr(provider, "_get_anthropic_client", lambda: mock_client)
        schema = {"type": "object"}

        provider.run_generate(
            "hello",
            system_instruction="base",
            json_schema=schema,
        )

        call_kwargs = mock_client.messages.create.call_args.kwargs
        assert call_kwargs["output_config"] == {
            "format": {"type": "json_schema", "schema": schema}
        }
        assert "tools" not in call_kwargs
        assert "tool_choice" not in call_kwargs
        assert call_kwargs["system"] == "base"

    def test_structured_messages_with_schema_uses_output_config(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        mock_response = MagicMock()
        mock_response.content = [SimpleNamespace(type="text", text="{}")]
        mock_response.usage = None
        mock_response.stop_reason = "end_turn"
        mock_client.messages.create.return_value = mock_response
        monkeypatch.setattr(provider, "_get_anthropic_client", lambda: mock_client)
        schema = {"type": "object"}
        messages = [
            {"role": "user", "content": "first"},
            {"role": "assistant", "content": "second"},
            {"role": "user", "content": "third"},
        ]

        provider.run_generate(
            messages,
            system_instruction="base",
            json_schema=schema,
        )

        call_kwargs = mock_client.messages.create.call_args.kwargs
        assert call_kwargs["messages"] == messages
        assert call_kwargs["output_config"] == {
            "format": {"type": "json_schema", "schema": schema}
        }
        assert "tools" not in call_kwargs
        assert "tool_choice" not in call_kwargs
        assert call_kwargs["system"] == "base"

    def test_async_with_schema_uses_output_config(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        mock_client.messages.create = AsyncMock()
        mock_response = MagicMock()
        mock_response.content = [SimpleNamespace(type="text", text="{}")]
        mock_response.usage = None
        mock_response.stop_reason = "end_turn"
        mock_client.messages.create.return_value = mock_response
        monkeypatch.setattr(
            provider, "_get_async_anthropic_client", lambda: mock_client
        )
        schema = {"type": "object"}

        asyncio.run(
            provider.run_agenerate(
                "hello",
                system_instruction="base",
                json_schema=schema,
            )
        )

        call_kwargs = mock_client.messages.create.call_args.kwargs
        assert call_kwargs["output_config"] == {
            "format": {"type": "json_schema", "schema": schema}
        }
        assert "tools" not in call_kwargs
        assert "tool_choice" not in call_kwargs
        assert call_kwargs["system"] == "base"

    def test_schema_max_tokens_still_surfaces_incomplete_json(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        mock_response = MagicMock()
        mock_response.content = [SimpleNamespace(type="text", text="{}")]
        mock_response.usage = None
        mock_response.stop_reason = "max_tokens"
        mock_client.messages.create.return_value = mock_response
        monkeypatch.setattr(provider, "_get_anthropic_client", lambda: mock_client)

        result = provider.run_generate("hello", json_schema={"type": "object"})

        assert result["finish_reason"] == "max_tokens"
        with pytest.raises(IncompleteJSONError):
            _validate_json_response(result, True)

    def test_async_multi_image_parts_preserve_order(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        mock_client.messages.create = AsyncMock()
        mock_response = MagicMock()
        mock_response.content = [SimpleNamespace(type="text", text="ok")]
        mock_response.usage = None
        mock_response.stop_reason = "end_turn"
        mock_client.messages.create.return_value = mock_response
        monkeypatch.setattr(
            provider, "_get_async_anthropic_client", lambda: mock_client
        )
        first = Image.new("RGB", (3, 2), color="red")
        second = Image.new("RGB", (4, 5), color="green")

        asyncio.run(provider.run_agenerate(["prompt", first, second]))

        content = mock_client.messages.create.call_args.kwargs["messages"][0]["content"]
        assert [block["type"] for block in content] == ["text", "image", "image"]
        assert content[0]["text"] == "prompt"
        first_source = content[1]["source"]
        second_source = content[2]["source"]
        assert first_source["media_type"] == "image/png"
        assert second_source["media_type"] == "image/png"
        first_decoded = _decoded_image(first_source["data"])
        second_decoded = _decoded_image(second_source["data"])
        assert first_decoded.size == first.size
        assert second_decoded.size == second.size
        assert first_decoded.format == "PNG"
        assert second_decoded.format == "PNG"


def _make_response(content=None, stop_reason="end_turn"):
    response = MagicMock()
    response.content = (
        content if content is not None else [SimpleNamespace(type="text", text="ok")]
    )
    response.usage = None
    response.stop_reason = stop_reason
    return response


def _make_stream_cm(final_message, *, enter_side_effect=None):
    cm = MagicMock()
    if enter_side_effect is None:
        stream = MagicMock()
        stream.get_final_message = MagicMock(return_value=final_message)
        cm.__enter__ = MagicMock(return_value=stream)
    else:
        cm.__enter__ = MagicMock(side_effect=enter_side_effect)
    cm.__exit__ = MagicMock(return_value=False)
    return cm


def _make_async_stream_cm(final_message):
    cm = MagicMock()
    stream = MagicMock()
    stream.get_final_message = AsyncMock(return_value=final_message)
    cm.__aenter__ = AsyncMock(return_value=stream)
    cm.__aexit__ = AsyncMock(return_value=False)
    return cm


class TestBudgetAdjustment:
    def test_lifts_max_tokens_when_collides_with_thinking_budget(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        mock_client.messages.create.return_value = _make_response()
        monkeypatch.setattr(provider, "_get_anthropic_client", lambda: mock_client)

        provider.run_generate(
            "hello",
            thinking_budget=4096,
            max_output_tokens=4096,
        )

        call_kwargs = mock_client.messages.create.call_args.kwargs
        assert call_kwargs["max_tokens"] == 4096 + 1000 + 1
        assert call_kwargs["thinking"]["budget_tokens"] == 4096

    def test_leaves_max_tokens_when_buffer_satisfied(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        mock_client.messages.create.return_value = _make_response()
        monkeypatch.setattr(provider, "_get_anthropic_client", lambda: mock_client)

        provider.run_generate(
            "hello",
            thinking_budget=4096,
            max_output_tokens=8192,
        )

        call_kwargs = mock_client.messages.create.call_args.kwargs
        assert call_kwargs["max_tokens"] == 8192

    def test_no_adjustment_without_thinking(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        mock_client.messages.create.return_value = _make_response()
        monkeypatch.setattr(provider, "_get_anthropic_client", lambda: mock_client)

        provider.run_generate(
            "hello",
            thinking_budget=0,
            max_output_tokens=4096,
        )

        call_kwargs = mock_client.messages.create.call_args.kwargs
        assert call_kwargs["max_tokens"] == 4096
        assert "thinking" not in call_kwargs

    def test_async_lifts_max_tokens_when_collides(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        mock_client.messages.create = AsyncMock(return_value=_make_response())
        monkeypatch.setattr(
            provider, "_get_async_anthropic_client", lambda: mock_client
        )

        asyncio.run(
            provider.run_agenerate(
                "hello",
                thinking_budget=4096,
                max_output_tokens=4096,
            )
        )

        call_kwargs = mock_client.messages.create.call_args.kwargs
        assert call_kwargs["max_tokens"] == 4096 + 1000 + 1
        assert call_kwargs["thinking"]["budget_tokens"] == 4096


class TestStreamingDispatch:
    def test_streams_when_max_tokens_exceeds_time_formula(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        mock_client.messages.stream.return_value = _make_stream_cm(_make_response())
        monkeypatch.setattr(provider, "_get_anthropic_client", lambda: mock_client)

        provider.run_generate(
            "hello",
            model="claude-sonnet-4-5",
            max_output_tokens=49152,
        )

        assert mock_client.messages.stream.call_count == 1
        assert mock_client.messages.create.call_count == 0

    def test_uses_create_below_threshold(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        mock_client.messages.create.return_value = _make_response()
        monkeypatch.setattr(provider, "_get_anthropic_client", lambda: mock_client)

        provider.run_generate(
            "hello",
            model="claude-sonnet-4-5",
            max_output_tokens=8192,
        )

        assert mock_client.messages.create.call_count == 1
        assert mock_client.messages.stream.call_count == 0

    def test_streams_when_per_model_cap_exceeded(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        mock_client.messages.stream.return_value = _make_stream_cm(_make_response())
        monkeypatch.setattr(provider, "_get_anthropic_client", lambda: mock_client)

        provider.run_generate(
            "hello",
            model="claude-opus-4-1-20250805",
            max_output_tokens=16384,
        )

        assert mock_client.messages.stream.call_count == 1
        assert mock_client.messages.create.call_count == 0

    def test_async_streams_when_max_tokens_exceeds_time_formula(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        mock_client.messages.stream.return_value = _make_async_stream_cm(
            _make_response()
        )
        monkeypatch.setattr(
            provider, "_get_async_anthropic_client", lambda: mock_client
        )

        asyncio.run(
            provider.run_agenerate(
                "hello",
                model="claude-sonnet-4-5",
                max_output_tokens=49152,
            )
        )

        assert mock_client.messages.stream.call_count == 1
        assert mock_client.messages.create.call_count == 0

    def test_async_uses_create_below_threshold(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        mock_client.messages.create = AsyncMock(return_value=_make_response())
        monkeypatch.setattr(
            provider, "_get_async_anthropic_client", lambda: mock_client
        )

        asyncio.run(
            provider.run_agenerate(
                "hello",
                model="claude-sonnet-4-5",
                max_output_tokens=8192,
            )
        )

        assert mock_client.messages.create.call_count == 1
        assert mock_client.messages.stream.call_count == 0

    def test_interaction_thinking_and_streaming(self, monkeypatch):
        provider = importlib.reload(
            importlib.import_module("solstone.think.providers.anthropic")
        )
        mock_client = MagicMock()
        mock_client.messages.stream.return_value = _make_stream_cm(_make_response())
        monkeypatch.setattr(provider, "_get_anthropic_client", lambda: mock_client)

        provider.run_generate(
            "hello",
            model="claude-sonnet-4-5",
            thinking_budget=24576,
            max_output_tokens=24576,
        )

        assert mock_client.messages.stream.call_count == 1
        call_kwargs = mock_client.messages.stream.call_args.kwargs
        assert call_kwargs["max_tokens"] == 25577
