# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the generator output pipeline.

Tests cover:
- Basic output generation via NDJSON protocol
- Hook invocation with correct context
- Generators without hooks
"""

import importlib
import io
import json
from pathlib import Path
from unittest.mock import MagicMock

from solstone.think.utils import day_path
from tests.conftest import copytree_tracked

FIXTURES = Path("tests/fixtures")


def copy_day(tmp_path: Path, monkeypatch) -> Path:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    dest = day_path("20240101")
    src = FIXTURES / "journal" / "chronicle" / "20240101"
    copytree_tracked(src, dest)
    return dest


# Mock result must be >= MIN_INPUT_CHARS (50) to generate output
MOCK_RESULT = {
    "text": "## Meeting Summary\n\nTeam standup at 9am with Alice and Bob discussing project status.",
    "usage": {"input_tokens": 100, "output_tokens": 50},
}


def run_generator_with_config(mod, config: dict, monkeypatch) -> list[dict]:
    """Run generator with NDJSON config and capture output events."""
    # Mock argv to prevent argparse from seeing pytest args
    monkeypatch.setattr("sys.argv", ["sol"])

    # Mock stdin with config
    stdin_data = json.dumps(config) + "\n"
    monkeypatch.setattr("sys.stdin", io.StringIO(stdin_data))

    # Capture stdout
    captured_output = io.StringIO()
    monkeypatch.setattr("sys.stdout", captured_output)

    # Run main
    mod.main()

    # Parse output events
    events = []
    captured_output.seek(0)
    for line in captured_output:
        line = line.strip()
        if line:
            events.append(json.loads(line))

    return events


def _write_generator_file(
    tmp_path: Path,
    name: str,
    metadata: dict,
    body: str = "Test prompt",
) -> None:
    (tmp_path / f"{name}.md").write_text(
        f"{json.dumps(metadata, indent=2)}\n\n{body}\n",
        encoding="utf-8",
    )


def _write_schema_file(tmp_path: Path, name: str, schema: dict) -> None:
    (tmp_path / name).write_text(json.dumps(schema, indent=2), encoding="utf-8")


def test_generate_output_ndjson(tmp_path, monkeypatch):
    """Test basic output generation via NDJSON protocol."""
    mod = importlib.import_module("solstone.think.talents")
    copy_day(tmp_path, monkeypatch)

    import solstone.think.talent as talent

    monkeypatch.setattr(talent, "TALENT_DIR", tmp_path)

    test_generator = tmp_path / "test_gen.md"
    test_generator.write_text(
        '{\n  "type": "generate",\n  "schedule": "daily",\n  "priority": 10,\n  "output": "md",\n  "load": {"transcripts": true, "percepts": true}\n}\n\nTest prompt'
    )

    # Mock the underlying generation function in models
    from solstone.think import models

    monkeypatch.setattr(
        models,
        "generate_with_result",
        lambda *a, **k: MOCK_RESULT,
    )
    monkeypatch.setenv("GOOGLE_API_KEY", "x")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    config = {
        "name": "test_gen",
        "day": "20240101",
        "output": "md",
        "provider": "google",
        "model": "gemini-2.0-flash",
    }

    events = run_generator_with_config(mod, config, monkeypatch)

    # Should have start and finish events
    assert len(events) >= 2
    assert events[0]["event"] == "start"
    assert events[0]["name"] == "test_gen"

    # Find finish event
    finish_events = [e for e in events if e["event"] == "finish"]
    assert len(finish_events) == 1
    assert finish_events[0]["result"] == MOCK_RESULT["text"]


def test_dispatcher_passes_json_schema(tmp_path, monkeypatch):
    """Test that generator execution forwards json_schema to the model layer."""
    mod = importlib.import_module("solstone.think.talents")
    copy_day(tmp_path, monkeypatch)

    from solstone.think import models, talent

    monkeypatch.setattr(talent, "TALENT_DIR", tmp_path)
    schema = {"type": "object", "properties": {"summary": {"type": "string"}}}
    _write_schema_file(tmp_path, "schema.json", schema)
    _write_generator_file(
        tmp_path,
        "schema_gen",
        {
            "type": "generate",
            "schedule": "daily",
            "priority": 10,
            "output": "json",
            "schema": "schema.json",
            "load": {"transcripts": True, "percepts": True},
        },
    )

    mock_generate = MagicMock(
        return_value={
            "text": '{"summary":"ok"}',
            "usage": {"input_tokens": 10, "output_tokens": 5},
        }
    )
    monkeypatch.setattr(models, "generate_with_result", mock_generate)
    monkeypatch.setenv("GOOGLE_API_KEY", "x")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    events = run_generator_with_config(
        mod,
        {
            "name": "schema_gen",
            "day": "20240101",
            "output": "json",
            "provider": "google",
            "model": "gemini-2.0-flash",
        },
        monkeypatch,
    )

    assert mock_generate.call_args.kwargs["json_schema"] == schema
    finish_events = [e for e in events if e["event"] == "finish"]
    assert len(finish_events) == 1


def test_dispatcher_omits_json_schema_when_absent(tmp_path, monkeypatch):
    """Test that generator execution passes json_schema=None when absent."""
    mod = importlib.import_module("solstone.think.talents")
    copy_day(tmp_path, monkeypatch)

    from solstone.think import models, talent

    monkeypatch.setattr(talent, "TALENT_DIR", tmp_path)
    _write_generator_file(
        tmp_path,
        "plain_gen",
        {
            "type": "generate",
            "schedule": "daily",
            "priority": 10,
            "output": "md",
            "load": {"transcripts": True, "percepts": True},
        },
    )

    mock_generate = MagicMock(return_value=MOCK_RESULT)
    monkeypatch.setattr(models, "generate_with_result", mock_generate)
    monkeypatch.setenv("GOOGLE_API_KEY", "x")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    run_generator_with_config(
        mod,
        {
            "name": "plain_gen",
            "day": "20240101",
            "output": "md",
            "provider": "google",
            "model": "gemini-2.0-flash",
        },
        monkeypatch,
    )

    assert mock_generate.call_args.kwargs["json_schema"] is None


def test_finish_event_includes_schema_validation(tmp_path, monkeypatch):
    """Test that finish events surface schema_validation when returned."""
    mod = importlib.import_module("solstone.think.talents")
    copy_day(tmp_path, monkeypatch)

    from solstone.think import models, talent

    monkeypatch.setattr(talent, "TALENT_DIR", tmp_path)
    schema = {"type": "object", "properties": {"summary": {"type": "string"}}}
    validation = {"valid": True, "errors": []}
    _write_schema_file(tmp_path, "schema.json", schema)
    _write_generator_file(
        tmp_path,
        "schema_validation_gen",
        {
            "type": "generate",
            "schedule": "daily",
            "priority": 10,
            "output": "json",
            "schema": "schema.json",
            "load": {"transcripts": True, "percepts": True},
        },
    )

    monkeypatch.setattr(
        models,
        "generate_with_result",
        MagicMock(
            return_value={
                "text": '{"summary":"ok"}',
                "usage": {"input_tokens": 10, "output_tokens": 5},
                "schema_validation": validation,
            }
        ),
    )
    monkeypatch.setenv("GOOGLE_API_KEY", "x")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    events = run_generator_with_config(
        mod,
        {
            "name": "schema_validation_gen",
            "day": "20240101",
            "output": "json",
            "provider": "google",
            "model": "gemini-2.0-flash",
        },
        monkeypatch,
    )

    finish_events = [e for e in events if e["event"] == "finish"]
    assert len(finish_events) == 1
    assert finish_events[0]["schema_validation"] == validation


def test_finish_event_omits_schema_validation_when_absent(tmp_path, monkeypatch):
    """Test that finish events omit schema_validation when not returned."""
    mod = importlib.import_module("solstone.think.talents")
    copy_day(tmp_path, monkeypatch)

    from solstone.think import models, talent

    monkeypatch.setattr(talent, "TALENT_DIR", tmp_path)
    _write_generator_file(
        tmp_path,
        "no_schema_validation_gen",
        {
            "type": "generate",
            "schedule": "daily",
            "priority": 10,
            "output": "md",
            "load": {"transcripts": True, "percepts": True},
        },
    )

    monkeypatch.setattr(
        models,
        "generate_with_result",
        MagicMock(return_value=MOCK_RESULT),
    )
    monkeypatch.setenv("GOOGLE_API_KEY", "x")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    events = run_generator_with_config(
        mod,
        {
            "name": "no_schema_validation_gen",
            "day": "20240101",
            "output": "md",
            "provider": "google",
            "model": "gemini-2.0-flash",
        },
        monkeypatch,
    )

    finish_events = [e for e in events if e["event"] == "finish"]
    assert len(finish_events) == 1
    assert "schema_validation" not in finish_events[0]


def test_generate_hook_invoked_with_context(tmp_path, monkeypatch):
    """Test that hooks receive correct context including span flag."""
    mod = importlib.import_module("solstone.think.talents")
    copy_day(tmp_path, monkeypatch)

    import solstone.think.talent as talent

    monkeypatch.setattr(talent, "TALENT_DIR", tmp_path)

    hook_file = tmp_path / "test_hook.py"
    hook_file.write_text("""
def post_process(result, context):
    import json
    from pathlib import Path
    # Write context to file for test verification
    out_path = Path(context["output_path"]).parent / "context_captured.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    ctx_copy = {
        "day": context.get("day"),
        "segment": context.get("segment"),
        "span": context.get("span_mode"),
        "name": context.get("name"),
        "has_transcript": bool(context.get("transcript")),
        "has_hook": bool(context.get("hook")),  # Frontmatter fields now directly in config
    }
    with open(out_path, "w") as f:
        json.dump(ctx_copy, f)
    return None
""")

    test_generator = tmp_path / "hooked_gen.md"
    test_generator.write_text(
        '{\n  "type": "generate",\n  "title": "Hooked",\n  "schedule": "daily",\n  "priority": 10,\n  "output": "md",\n  "hook": {"post": "test_hook"},\n  "load": {"transcripts": true, "percepts": true}\n}\n\nTest prompt'
    )

    # Mock the underlying generation function in models
    from solstone.think import models

    monkeypatch.setattr(
        models,
        "generate_with_result",
        lambda *a, **k: MOCK_RESULT,
    )
    monkeypatch.setenv("GOOGLE_API_KEY", "x")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    config = {
        "name": "hooked_gen",
        "day": "20240101",
        "output": "md",
        "provider": "google",
        "model": "gemini-2.0-flash",
    }

    events = run_generator_with_config(mod, config, monkeypatch)

    # Should have start and finish events
    finish_events = [e for e in events if e["event"] == "finish"]
    assert len(finish_events) == 1

    # Read captured context
    captured_path = (
        tmp_path / "chronicle" / "20240101" / "talents" / "context_captured.json"
    )
    captured = json.loads(captured_path.read_text())

    assert captured["day"] == "20240101"
    assert captured["segment"] is None
    # span_mode is a bool in the new config structure
    assert captured["span"] is False
    assert captured["name"] == "hooked_gen"
    assert captured["has_transcript"] is True
    assert captured["has_hook"] is True  # Frontmatter fields now directly in config


def test_generate_without_hook_succeeds(tmp_path, monkeypatch):
    """Test that generators without hooks still work correctly."""
    mod = importlib.import_module("solstone.think.talents")
    copy_day(tmp_path, monkeypatch)

    import solstone.think.talent as talent

    monkeypatch.setattr(talent, "TALENT_DIR", tmp_path)

    test_generator = tmp_path / "nohook_gen.md"
    test_generator.write_text(
        '{\n  "type": "generate",\n  "schedule": "daily",\n  "priority": 10,\n  "output": "md",\n  "load": {"transcripts": true, "percepts": true}\n}\n\nNo hook prompt'
    )

    # Mock the underlying generation function in models
    from solstone.think import models

    monkeypatch.setattr(
        models,
        "generate_with_result",
        lambda *a, **k: MOCK_RESULT,
    )
    monkeypatch.setenv("GOOGLE_API_KEY", "x")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    config = {
        "name": "nohook_gen",
        "day": "20240101",
        "output": "md",
        "provider": "google",
        "model": "gemini-2.0-flash",
    }

    events = run_generator_with_config(mod, config, monkeypatch)

    # Should have start and finish events
    assert len(events) >= 2
    finish_events = [e for e in events if e["event"] == "finish"]
    assert len(finish_events) == 1
    assert finish_events[0]["result"] == MOCK_RESULT["text"]


def test_generate_error_event_on_missing_generator(tmp_path, monkeypatch):
    """Test that missing generator name emits error event."""
    mod = importlib.import_module("solstone.think.talents")
    copy_day(tmp_path, monkeypatch)

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    config = {
        "name": "nonexistent_generator",
        "day": "20240101",
        "output": "md",
    }

    events = run_generator_with_config(mod, config, monkeypatch)

    # Should have an error event
    error_events = [e for e in events if e["event"] == "error"]
    assert len(error_events) == 1
    assert "not found" in error_events[0]["error"].lower()


def test_generate_skipped_on_no_input(tmp_path, monkeypatch):
    """Test that generator emits skipped finish when no input."""
    mod = importlib.import_module("solstone.think.talents")

    # Create empty day directory (no transcripts)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")
    day_dir.mkdir(parents=True, exist_ok=True)

    import solstone.think.talent as talent

    monkeypatch.setattr(talent, "TALENT_DIR", tmp_path)

    test_generator = tmp_path / "empty_gen.md"
    test_generator.write_text(
        '{\n  "type": "generate",\n  "schedule": "daily",\n  "priority": 10,\n  "output": "md",\n  "load": {"transcripts": true, "percepts": true}\n}\n\nTest prompt'
    )

    monkeypatch.setenv("GOOGLE_API_KEY", "x")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    config = {
        "name": "empty_gen",
        "day": "20240101",
        "output": "md",
        "provider": "google",
        "model": "gemini-2.0-flash",
    }

    events = run_generator_with_config(mod, config, monkeypatch)

    # Should have start and finish with skipped
    finish_events = [e for e in events if e["event"] == "finish"]
    assert len(finish_events) == 1
    assert finish_events[0].get("skipped") == "no_input"


def test_cogitate_not_skipped_without_sources(tmp_path, monkeypatch):
    """Test that cogitate agents with day but no sources are not skipped."""
    mod = importlib.import_module("solstone.think.talents")

    # Create empty day directory (no transcripts)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")
    day_dir.mkdir(parents=True, exist_ok=True)

    import solstone.think.talent as talent

    monkeypatch.setattr(talent, "TALENT_DIR", tmp_path)

    test_agent = tmp_path / "test_cogitate.md"
    test_agent.write_text(
        '{\n  "type": "cogitate",\n  "schedule": "daily",\n  "priority": 10\n}\n\nTest prompt'
    )

    monkeypatch.setenv("GOOGLE_API_KEY", "x")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    config = mod.prepare_config(
        {
            "name": "test_cogitate",
            "day": "20240101",
        }
    )

    assert config.get("skip_reason") is None


def test_named_hook_resolution(tmp_path, monkeypatch):
    """Test that named hooks are resolved via load_post_hook."""
    from solstone.think.talent import load_post_hook

    # Config with named hook (new format)
    config = {"hook": {"post": "schedule"}}
    hook_fn = load_post_hook(config)

    # Should resolve to talent/schedule.py and be callable
    assert callable(hook_fn)
