# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for NDJSON-only input in think.talents."""

import asyncio
import json
import sys
from io import StringIO
from unittest.mock import MagicMock, patch

import pytest

from solstone.think.models import GPT_5


@pytest.fixture
def mock_journal(tmp_path, monkeypatch):
    """Set up a temporary journal directory."""
    journal_path = tmp_path / "journal"
    journal_path.mkdir()
    agents_path = journal_path / "talents"
    agents_path.mkdir()

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal_path))
    return journal_path


async def mock_run_cogitate(config, on_event=None):
    """Mock run_cogitate function for testing."""
    prompt = config.get("prompt", "")
    provider = config.get("provider", "")
    model = config.get("model", "")
    name = config.get("name", "chat")

    if on_event:
        on_event(
            {
                "event": "start",
                "prompt": prompt,
                "provider": provider,
                "model": model,
                "name": name,
                "ts": 1234567890,
            }
        )
        on_event(
            {
                "event": "finish",
                "result": f"Response to: {prompt}",
                "ts": 1234567891,
            }
        )
    return f"Response to: {prompt}"


def mock_prepare_config(request: dict) -> dict:
    """Mock prepare_config that passes through request with minimal additions."""
    config = dict(request)
    # Add required fields if not present
    if "name" not in config:
        config["name"] = "chat"
    if "provider" not in config:
        config["provider"] = "google"
    if "model" not in config:
        config["model"] = "gpt-5-mini"
    if "type" not in config:
        config["type"] = "cogitate"
    # Add empty meta for hooks
    config["meta"] = {}
    return config


def mock_all_providers(monkeypatch):
    """Mock the registered cogitate provider module with mock_run_cogitate.

    Cloud providers route through the OpenHands facade for cogitate, so one mock
    covers openai, anthropic, and google registry lookups.
    """
    mock_module = MagicMock()
    mock_module.run_cogitate = mock_run_cogitate
    monkeypatch.setitem(sys.modules, "solstone.think.providers.openhands", mock_module)

    monkeypatch.setitem(sys.modules, "agents", MagicMock())

    # Mock prepare_config to avoid needing real agent configs
    monkeypatch.setattr("solstone.think.talents.prepare_config", mock_prepare_config)


def test_ndjson_single_request(mock_journal, monkeypatch, capsys):
    """Test processing a single NDJSON request from stdin."""
    ndjson_input = json.dumps(
        {
            "prompt": "What is 2+2?",
            "provider": "openai",
            "name": "chat",
            "model": GPT_5,
            "max_output_tokens": 100,
        }
    )

    monkeypatch.setattr("sys.stdin", StringIO(ndjson_input))

    mock_args = MagicMock()
    mock_args.verbose = False
    mock_args.dry_run = False

    mock_all_providers(monkeypatch)

    from solstone.think.talents import main_async

    with patch("solstone.think.talents.setup_cli", return_value=mock_args):
        asyncio.run(main_async())

    captured = capsys.readouterr()
    lines = captured.out.strip().split("\n")

    events = [json.loads(line) for line in lines if line]

    assert events

    start_event = events[0]
    assert start_event["event"] == "start"
    # Prompt includes system instruction prepended during enrichment
    assert "What is 2+2?" in start_event["prompt"]
    assert start_event["provider"] == "openai"
    assert start_event["model"] == GPT_5

    finish_events = [e for e in events if e["event"] == "finish"]
    assert finish_events


def test_ndjson_multiple_requests(mock_journal, monkeypatch, capsys):
    """Test processing multiple NDJSON requests from stdin."""
    requests = [
        {
            "prompt": "First question",
            "provider": "openai",
        },
        {
            "prompt": "Second question",
            "provider": "anthropic",
            "model": "claude-3",
        },
        {
            "prompt": "Third question",
            "provider": "google",
            "name": "technical",
        },
    ]

    ndjson_input = "\n".join(json.dumps(r) for r in requests)

    monkeypatch.setattr("sys.stdin", StringIO(ndjson_input))

    mock_args = MagicMock()
    mock_args.verbose = False
    mock_args.dry_run = False

    mock_all_providers(monkeypatch)

    from solstone.think.talents import main_async

    with patch("solstone.think.talents.setup_cli", return_value=mock_args):
        asyncio.run(main_async())

    captured = capsys.readouterr()
    lines = [line for line in captured.out.strip().split("\n") if line]

    assert len(lines) >= 6

    events = [json.loads(line) for line in lines]
    start_events = [e for e in events if e["event"] == "start"]

    assert len(start_events) == 3
    # Prompts include system instruction prepended during enrichment
    assert "First question" in start_events[0]["prompt"]
    assert "Second question" in start_events[1]["prompt"]
    assert start_events[1]["provider"] == "anthropic"
    assert "Third question" in start_events[2]["prompt"]
    assert start_events[2]["name"] == "technical"


def test_ndjson_invalid_json(mock_journal, monkeypatch, capsys):
    """Test handling of invalid JSON in NDJSON input."""
    ndjson_input = """{"prompt": "Valid request", "provider": "openai"}
not valid json
{"prompt": "Another valid request", "provider": "openai"}"""

    monkeypatch.setattr("sys.stdin", StringIO(ndjson_input))

    mock_args = MagicMock()
    mock_args.verbose = False
    mock_args.dry_run = False

    mock_all_providers(monkeypatch)

    from solstone.think.talents import main_async

    with patch("solstone.think.talents.setup_cli", return_value=mock_args):
        asyncio.run(main_async())

    captured = capsys.readouterr()
    lines = [line for line in captured.out.strip().split("\n") if line]

    events = [json.loads(line) for line in lines]

    error_events = [e for e in events if e["event"] == "error"]
    assert len(error_events) == 1
    assert "Invalid JSON" in error_events[0]["error"]

    start_events = [e for e in events if e["event"] == "start"]
    assert len(start_events) == 2


def test_ndjson_missing_prompt(mock_journal, monkeypatch, capsys):
    """Test handling of NDJSON request without required 'prompt' field."""
    ndjson_input = json.dumps(
        {
            "provider": "openai",
            "model": GPT_5,
        }
    )

    monkeypatch.setattr("sys.stdin", StringIO(ndjson_input))

    mock_args = MagicMock()
    mock_args.verbose = False
    mock_args.dry_run = False

    mock_all_providers(monkeypatch)

    from solstone.think.talents import main_async

    with patch("solstone.think.talents.setup_cli", return_value=mock_args):
        asyncio.run(main_async())

    captured = capsys.readouterr()
    lines = [line for line in captured.out.strip().split("\n") if line]

    assert len(lines) >= 1
    error_event = json.loads(lines[0])
    assert error_event["event"] == "error"
    assert "prompt" in error_event["error"].lower()  # Error mentions prompt


def test_ndjson_empty_lines(mock_journal, monkeypatch, capsys):
    """Test that empty lines in NDJSON input are ignored."""
    ndjson_input = """{"prompt": "First", "provider": "openai"}

{"prompt": "Second", "provider": "openai"}

"""

    monkeypatch.setattr("sys.stdin", StringIO(ndjson_input))

    mock_args = MagicMock()
    mock_args.verbose = False
    mock_args.dry_run = False

    mock_all_providers(monkeypatch)

    from solstone.think.talents import main_async

    with patch("solstone.think.talents.setup_cli", return_value=mock_args):
        asyncio.run(main_async())

    captured = capsys.readouterr()
    lines = [line for line in captured.out.strip().split("\n") if line]

    events = [json.loads(line) for line in lines]
    start_events = [e for e in events if e["event"] == "start"]

    assert len(start_events) == 2
