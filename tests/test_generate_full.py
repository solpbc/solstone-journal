# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the generator output pipeline.

Tests cover:
- Basic output generation via NDJSON protocol
- Hook invocation with correct context
- Generators without hooks
"""

import asyncio
import importlib
import io
import json
import logging
import os
import threading
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path
from unittest.mock import MagicMock

from solstone.think.responsiveness import (
    NON_RESPONSIVE_RAW_OUTPUT_CAP_CHARS,
    NON_RESPONSIVE_REASON_CODE,
)
from solstone.think.utils import day_path
from tests.conftest import copytree_tracked

FIXTURES = Path("tests/fixtures")


def copy_day(tmp_path: Path, monkeypatch) -> Path:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    config_dir = tmp_path / "config"
    config_dir.mkdir(parents=True, exist_ok=True)
    (config_dir / "journal.json").write_text(
        json.dumps(
            {
                "providers": {
                    "active": {
                        "provider": "google",
                        "model": "gemini-2.0-flash",
                    }
                }
            }
        ),
        encoding="utf-8",
    )
    dest = day_path("20240101")
    src = FIXTURES / "journal" / "chronicle" / "20240101"
    copytree_tracked(src, dest)
    return dest


# Mock result must be >= MIN_INPUT_CHARS (50) to generate output
MOCK_RESULT = {
    "text": "## Meeting Summary\n\nTeam standup at 9am with Alice and Bob discussing project status.",
    "usage": {"input_tokens": 100, "output_tokens": 50},
}
_NON_RESPONSIVE_REFUSAL = "I cannot describe this screen."
_BRAIN_NOW = datetime.now(timezone.utc)


def _install_native_brain_binary(monkeypatch) -> None:
    from solstone.think.providers import brain_state

    binary = Path(__file__).resolve().parents[1] / "core/target/debug/solstone-core"
    assert binary.is_file()
    monkeypatch.setattr(brain_state, "_native_binary", lambda **_kwargs: binary)


def _brain_component() -> dict:
    return {
        "status": "ok",
        "observed_at": _BRAIN_NOW.isoformat(),
        "expires_at": (_BRAIN_NOW + timedelta(days=1)).isoformat(),
    }


def _ready_brain_outcome() -> dict:
    component = _brain_component
    return {
        "configuration": component(),
        "lane_prerequisites": component(),
        "generate": component(),
        "cogitate": component(),
    }


def _write_ready_brain_record(
    tmp_path: Path,
    *,
    model: str = "gemini-3.5-flash",
) -> None:
    from solstone.think.providers.brain_state import (
        begin_brain_refresh,
        finish_brain_refresh,
    )

    config_dir = tmp_path / "config"
    config_dir.mkdir(parents=True, exist_ok=True)
    (config_dir / "journal.json").write_text(
        json.dumps(
            {
                "providers": {
                    "active": {
                        "provider": "google",
                        "model": model,
                    }
                },
                "env": {"GOOGLE_API_KEY": "test-key"},
            }
        ),
        encoding="utf-8",
    )
    permit = begin_brain_refresh(_BRAIN_NOW, journal_path=tmp_path)
    assert permit is not None
    finish_brain_refresh(
        permit,
        _ready_brain_outcome(),
        _BRAIN_NOW,
        journal_path=tmp_path,
    )


def _live_generate_progress_threads() -> list[threading.Thread]:
    return [
        thread
        for thread in threading.enumerate()
        if thread.name.startswith("generate-progress-") and thread.is_alive()
    ]


def run_generator_with_config(mod, config: dict, monkeypatch) -> list[dict]:
    """Run generator with NDJSON config and capture output events."""
    config_path = Path(os.environ["SOLSTONE_JOURNAL"]) / "config" / "journal.json"
    if not config_path.exists():
        config_path.parent.mkdir(parents=True, exist_ok=True)
        config_path.write_text(
            json.dumps(
                {
                    "providers": {
                        "active": {
                            "provider": "google",
                            "model": "gemini-2.0-flash",
                        }
                    }
                }
            ),
            encoding="utf-8",
        )
    # Mock argv to prevent argparse from seeing pytest args
    monkeypatch.setattr("sys.argv", ["sol"])

    # Mock stdin with config
    request = {
        key: value for key, value in config.items() if key not in {"provider", "model"}
    }
    stdin_data = json.dumps(request) + "\n"
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


def _non_responsive_generate_result(text: str = _NON_RESPONSIVE_REFUSAL) -> dict:
    return {
        "text": text,
        "model": "provider-model",
        "finish_reason": "stop",
        "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2},
    }


def _install_generate_provider(
    monkeypatch, outcome, *, provider: str = "google"
) -> None:
    from solstone.think import models

    provider_module = MagicMock()
    if isinstance(outcome, list):
        provider_module.run_generate.side_effect = outcome
    else:
        provider_module.run_generate.return_value = outcome
    monkeypatch.setattr(
        models,
        "resolve_provider",
        lambda _interface: (provider, "provider-model"),
    )
    monkeypatch.setattr(
        "solstone.think.providers.get_provider_module",
        lambda _provider: provider_module,
    )


def _run_generate_failure(
    tmp_path: Path,
    monkeypatch,
    side_effect: Exception,
) -> list[dict]:
    mod = importlib.import_module("solstone.think.talents")
    _install_native_brain_binary(monkeypatch)
    copy_day(tmp_path, monkeypatch)
    _write_ready_brain_record(tmp_path)

    import solstone.think.talent as talent

    monkeypatch.setattr(talent, "TALENT_DIR", tmp_path)
    _write_generator_file(
        tmp_path,
        "missing_model_day_gen",
        {
            "type": "generate",
            "schedule": "daily",
            "priority": 10,
            "output": "md",
            "load": {"transcripts": True, "percepts": True},
        },
    )
    provider_module = MagicMock()
    provider_module.run_generate.side_effect = side_effect
    monkeypatch.setattr(
        "solstone.think.providers.get_provider_module",
        lambda _provider: provider_module,
    )
    monkeypatch.setenv("GOOGLE_API_KEY", "test-key")

    return run_generator_with_config(
        mod,
        {
            "name": "missing_model_day_gen",
            "day": "20240101",
            "output": "md",
        },
        monkeypatch,
    )


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


def test_execute_generate_blank_expected_output_emits_terminal_no_output(
    tmp_path, monkeypatch
):
    """Blank post-provider output for an output-path talent emits no_output."""
    from solstone.think import models
    from solstone.think.talents import _execute_generate

    output_path = tmp_path / "out.md"
    output_path.write_text("old output", encoding="utf-8")
    monkeypatch.setattr(
        models,
        "generate_with_result",
        lambda **kwargs: {"text": "   ", "usage": {"input_tokens": 1}},
    )
    events: list[dict] = []

    asyncio.run(
        _execute_generate(
            {
                "provider": "google",
                "model": "gemini-2.0-flash",
                "name": "blank_gen",
                "prompt": "x",
                "output": "md",
                "output_path": str(output_path),
            },
            events.append,
        )
    )

    assert [event["event"] for event in events] == ["error"]
    assert events[0]["reason_code"] == "no_output"
    assert events[0]["terminal"] is True
    assert output_path.read_text(encoding="utf-8") == "old output"


def test_generate_progress_interval_stays_below_half_watchdog_cap():
    from solstone.convey import chat
    from solstone.think import talents

    assert (
        talents._GENERATE_PROGRESS_INTERVAL_S
        < min(chat._WATCHDOG_TIMEOUTS.values()) / 2
    ), (
        "generate progress heartbeat must stay strictly below half of the "
        "shortest watchdog timeout so one missed tick cannot trip the cap; "
        "strictly less than the cap is insufficient"
    )


def test_execute_generate_blocking_call_emits_progress_and_count(monkeypatch):
    from solstone.think import models, talents
    from solstone.think.talents import _execute_generate

    monkeypatch.setattr(talents, "_GENERATE_PROGRESS_INTERVAL_S", 0.01)

    def slow_generate(**_kwargs):
        time.sleep(0.035)
        return {"text": "done", "usage": {"input_tokens": 1}}

    monkeypatch.setattr(models, "generate_with_result", slow_generate)
    events: list[dict] = []

    asyncio.run(
        _execute_generate(
            {
                "provider": "google",
                "model": "gemini-2.0-flash",
                "name": "progress_gen",
                "prompt": "x",
            },
            events.append,
        )
    )

    progress_events = [event for event in events if event["event"] == "progress"]
    assert len(progress_events) >= 2
    assert all(event["phase"] == "generate" for event in progress_events)
    assert all("summary" not in event for event in progress_events)
    assert events[-1]["event"] == "finish"
    assert events[-1]["generate_progress_count"] == len(progress_events)
    assert not _live_generate_progress_threads()


def test_execute_generate_heartbeat_thread_stops_on_terminal_and_raise_paths(
    monkeypatch,
):
    from solstone.think import models, talents
    from solstone.think.models import IncompleteJSONError, ProviderResponseInvalidError
    from solstone.think.talents import _execute_generate

    monkeypatch.setattr(talents, "_GENERATE_PROGRESS_INTERVAL_S", 0.01)

    async def run_case(config: dict, generate) -> list[dict]:
        monkeypatch.setattr(models, "generate_with_result", generate)
        events: list[dict] = []
        try:
            await _execute_generate(config, events.append)
        finally:
            assert not _live_generate_progress_threads()
        return events

    base_config = {
        "provider": "google",
        "model": "gemini-2.0-flash",
        "name": "thread_cleanup",
        "prompt": "x",
    }

    normal_events = asyncio.run(run_case(base_config, lambda **_kwargs: {"text": "ok"}))
    assert normal_events[-1]["event"] == "finish"

    def raise_runtime(**_kwargs):
        raise RuntimeError("boom")

    try:
        asyncio.run(run_case(base_config, raise_runtime))
    except RuntimeError:
        pass
    else:
        raise AssertionError("expected RuntimeError")

    def provider_invalid(**_kwargs):
        raise ProviderResponseInvalidError("malformed", model="model")

    provider_events = asyncio.run(run_case(base_config, provider_invalid))
    assert provider_events[-1]["reason_code"] == "provider_response_invalid"

    calls = 0

    def retry_provider_invalid(**_kwargs):
        nonlocal calls
        calls += 1
        if calls == 1:
            raise IncompleteJSONError("length", '{"partial":')
        raise ProviderResponseInvalidError("malformed", model="model")

    retry_events = asyncio.run(
        run_case(
            {
                **base_config,
                "provider": "local",
                "model": "local-model",
                "output": "json",
            },
            retry_provider_invalid,
        )
    )
    assert retry_events[-1]["reason_code"] == "provider_response_invalid"
    assert retry_events[-1]["retries"] == 1


def test_execute_generate_heartbeat_emit_failure_is_logged_and_continues(
    monkeypatch, caplog
):
    from solstone.think import models, talents
    from solstone.think.talents import _execute_generate

    monkeypatch.setattr(talents, "_GENERATE_PROGRESS_INTERVAL_S", 0.01)

    def slow_generate(**_kwargs):
        time.sleep(0.035)
        return {"text": "done"}

    monkeypatch.setattr(models, "generate_with_result", slow_generate)
    events: list[dict] = []
    failed_once = False

    def emit_event(event: dict) -> None:
        nonlocal failed_once
        if event["event"] == "progress" and not failed_once:
            failed_once = True
            raise RuntimeError("transient emit failure")
        events.append(event)

    caplog.set_level(logging.ERROR, logger="solstone.think.talents")
    asyncio.run(
        _execute_generate(
            {
                "provider": "google",
                "model": "gemini-2.0-flash",
                "name": "progress_failure",
                "prompt": "x",
            },
            emit_event,
        )
    )

    progress_events = [event for event in events if event["event"] == "progress"]
    assert failed_once is True
    assert progress_events
    assert events[-1]["event"] == "finish"
    assert events[-1]["generate_progress_count"] == len(progress_events)
    assert "generate progress heartbeat failed talent=progress_failure" in caplog.text


def test_execute_generate_provider_blank_records_runtime_failure(
    tmp_path,
    monkeypatch,
):
    from solstone.think.providers.brain_state import inspect_brain_state
    from solstone.think.talents import _execute_generate

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _install_native_brain_binary(monkeypatch)
    _write_ready_brain_record(tmp_path)
    output_path = tmp_path / "out.md"
    output_path.write_text("old output", encoding="utf-8")
    provider_module = MagicMock()
    provider_module.run_generate.return_value = {
        "text": "   ",
        "model": "gemini-3.5-flash",
        "finish_reason": "stop",
        "usage": {"input_tokens": 1, "output_tokens": 0, "total_tokens": 1},
    }
    monkeypatch.setattr(
        "solstone.think.providers.get_provider_module",
        lambda _provider: provider_module,
    )
    events: list[dict] = []

    asyncio.run(
        _execute_generate(
            {
                "provider": "google",
                "model": "gemini-3.5-flash",
                "name": "provider_blank_gen",
                "prompt": "x",
                "output": "md",
                "output_path": str(output_path),
            },
            events.append,
        )
    )

    assert [event["event"] for event in events] == ["error"]
    assert events[0]["reason_code"] == "provider_response_invalid"
    assert events[0]["terminal"] is True
    assert output_path.read_text(encoding="utf-8") == "old output"
    provider_module.run_generate.assert_called_once()

    inspection = inspect_brain_state(datetime.now(timezone.utc), journal_path=tmp_path)
    record = inspection["record"]
    assert record is not None
    assert record["reason_code"] == "provider_response_invalid"
    assert record["evidence"]["generate"]["reason_code"] == (
        "provider_response_invalid"
    )


def test_execute_generate_non_responsive_emits_terminal_error(tmp_path, monkeypatch):
    from solstone.think.talents import _execute_generate

    output_path = tmp_path / "out.md"
    output_path.write_text("old output", encoding="utf-8")
    _install_generate_provider(monkeypatch, _non_responsive_generate_result())
    events = []

    asyncio.run(
        _execute_generate(
            {
                "name": "test_non_responsive",
                "prompt": "x",
                "output": "md",
                "output_path": str(output_path),
            },
            events.append,
        )
    )

    assert [event["event"] for event in events] == ["error"]
    assert events[0]["terminal"] is True
    assert events[0]["reason_code"] == NON_RESPONSIVE_REASON_CODE
    assert output_path.read_text(encoding="utf-8") == "old output"


def test_execute_generate_non_responsive_retry_emits_retries_one(
    tmp_path,
    monkeypatch,
):
    from solstone.think.talents import _execute_generate

    capacity_error = RuntimeError("capacity exhausted")
    capacity_error.reason_code = "local_capacity_exhausted"
    output_path = tmp_path / "out.md"
    _install_generate_provider(
        monkeypatch,
        [capacity_error, _non_responsive_generate_result()],
        provider="local",
    )
    events = []

    asyncio.run(
        _execute_generate(
            {
                "name": "test_non_responsive_retry",
                "provider": "local",
                "prompt": "x",
                "output": "md",
                "output_path": str(output_path),
            },
            events.append,
        )
    )

    assert [event["event"] for event in events] == ["error"]
    assert events[0]["reason_code"] == NON_RESPONSIVE_REASON_CODE
    assert events[0]["retries"] == 1
    assert not output_path.exists()


def test_execute_generate_non_responsive_terminal_event_carries_safe_raw(
    tmp_path,
    monkeypatch,
):
    from solstone.think.talents import _execute_generate

    raw_output = _NON_RESPONSIVE_REFUSAL + " " + ("overflow " * 200)
    _install_generate_provider(
        monkeypatch,
        _non_responsive_generate_result(raw_output),
    )
    events = []

    asyncio.run(
        _execute_generate(
            {
                "name": "test_non_responsive_raw",
                "prompt": "x",
                "output": "md",
                "output_path": str(tmp_path / "out.md"),
            },
            events.append,
        )
    )

    raw = events[0]["raw"]
    assert isinstance(raw, list)
    assert len(raw) == 1
    assert raw[0]["reason_code"] == NON_RESPONSIVE_REASON_CODE
    assert (
        raw[0]["non_responsive_output"]
        == raw_output[:NON_RESPONSIVE_RAW_OUTPUT_CAP_CHARS]
    )
    assert raw[0]["non_responsive_matched_signal"] == "i cannot"


def test_generate_model_not_found_records_runtime_failure(tmp_path, monkeypatch):
    from litellm.exceptions import NotFoundError

    from solstone.think.providers.brain_state import inspect_brain_state

    events = _run_generate_failure(
        tmp_path,
        monkeypatch,
        NotFoundError(
            "model not found",
            model="gemini-3.5-flash",
            llm_provider="gemini",
        ),
    )

    error_events = [event for event in events if event["event"] == "error"]
    assert len(error_events) == 1
    assert error_events[0]["reason_code"] == "model_not_found"
    assert error_events[0]["provider"] == "google"
    assert [event for event in events if event["event"] == "finish"] == []

    inspection = inspect_brain_state(datetime.now(timezone.utc), journal_path=tmp_path)
    record = inspection["record"]
    assert record is not None
    assert record["reason_code"] == "model_not_found"
    assert record["evidence"]["generate"]["reason_code"] == "model_not_found"


def test_generate_marked_transport_404_records_runtime_failure(tmp_path, monkeypatch):
    from solstone.think.providers.brain_state import inspect_brain_state
    from solstone.think.providers.shared import mark_cloud_model_request

    class ProviderCatalogError(Exception):
        status_code = 404

    exc = ProviderCatalogError("missing model")
    mark_cloud_model_request(exc)

    events = _run_generate_failure(tmp_path, monkeypatch, exc)

    error_events = [event for event in events if event["event"] == "error"]
    assert len(error_events) == 1
    assert error_events[0]["reason_code"] == "model_not_found"
    assert error_events[0]["provider"] == "google"
    assert [event for event in events if event["event"] == "finish"] == []

    inspection = inspect_brain_state(datetime.now(timezone.utc), journal_path=tmp_path)
    record = inspection["record"]
    assert record is not None
    assert record["reason_code"] == "model_not_found"
    assert record["evidence"]["generate"]["reason_code"] == "model_not_found"


def test_execute_generate_provider_blank_rejected_when_config_switches_in_flight(
    tmp_path,
    monkeypatch,
):
    from solstone.think.providers import brain_state as brain_state_module
    from solstone.think.providers.brain_state import (
        brain_state_path,
        inspect_brain_state,
    )
    from solstone.think.talents import _execute_generate

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _install_native_brain_binary(monkeypatch)
    _write_ready_brain_record(tmp_path, model="gemini-3.5-flash")
    f1_record = inspect_brain_state(datetime.now(timezone.utc), journal_path=tmp_path)[
        "record"
    ]
    assert f1_record is not None
    f1_sha = f1_record["fingerprint_sha256"]

    original_record_failure = brain_state_module.record_brain_runtime_failure
    recorded_expected_fingerprints: list[str] = []

    def spy_record_brain_runtime_failure(*args, **kwargs):
        recorded_expected_fingerprints.append(kwargs["expected_fingerprint_sha256"])
        return original_record_failure(*args, **kwargs)

    monkeypatch.setattr(
        brain_state_module,
        "record_brain_runtime_failure",
        spy_record_brain_runtime_failure,
    )

    f2_snapshot: dict[str, object] = {}
    provider_module = MagicMock()

    def switch_to_f2_then_blank(*args, **kwargs):
        assert kwargs["model"] == "gemini-3.5-flash"
        _write_ready_brain_record(tmp_path, model="gemini-3.1-flash-lite")
        f2_snapshot["bytes"] = brain_state_path(journal_path=tmp_path).read_bytes()
        f2_record = inspect_brain_state(
            datetime.now(timezone.utc), journal_path=tmp_path
        )["record"]
        assert f2_record is not None
        assert f2_record["fingerprint_sha256"] != f1_sha
        f2_snapshot["record"] = f2_record
        return {
            "text": "   ",
            "model": "gemini-3.5-flash",
            "finish_reason": "stop",
            "usage": {"input_tokens": 1, "output_tokens": 0, "total_tokens": 1},
        }

    provider_module.run_generate.side_effect = switch_to_f2_then_blank
    monkeypatch.setattr(
        "solstone.think.providers.get_provider_module",
        lambda _provider: provider_module,
    )
    output_path = tmp_path / "out.md"
    output_path.write_text("old output", encoding="utf-8")
    events: list[dict] = []

    asyncio.run(
        _execute_generate(
            {
                "provider": "google",
                "model": "gemini-3.5-flash",
                "name": "provider_blank_in_flight_switch",
                "prompt": "x",
                "output": "md",
                "output_path": str(output_path),
            },
            events.append,
        )
    )

    assert [event["event"] for event in events] == ["error"]
    assert events[0]["reason_code"] == "provider_response_invalid"
    assert events[0]["terminal"] is True
    provider_module.run_generate.assert_called_once()
    assert recorded_expected_fingerprints == [f1_sha]

    assert brain_state_path(journal_path=tmp_path).read_bytes() == f2_snapshot["bytes"]
    current_record = inspect_brain_state(
        datetime.now(timezone.utc), journal_path=tmp_path
    )["record"]
    assert current_record == f2_snapshot["record"]
    assert current_record is not None
    assert current_record["reason_code"] is None
    assert current_record["evidence"]["generate"]["status"] == "ok"
    assert (
        current_record["evidence"]["generate"].get("reason_code")
        != "provider_response_invalid"
    )


def test_execute_generate_schema_invalid_emits_terminal_error(tmp_path, monkeypatch):
    from solstone.think import models
    from solstone.think.talents import _execute_generate

    output_path = tmp_path / "out.json"
    validation = {
        "valid": False,
        "errors": [
            {
                "path": "/summary",
                "constraint": "type",
                "message": "42 is not of type 'string'",
            }
        ],
    }
    monkeypatch.setattr(
        models,
        "generate_with_result",
        lambda **kwargs: {
            "text": '{"summary": 42}',
            "usage": {"input_tokens": 1, "output_tokens": 3},
            "schema_validation": validation,
        },
    )
    events: list[dict] = []

    asyncio.run(
        _execute_generate(
            {
                "provider": "google",
                "model": "gemini-2.0-flash",
                "name": "schema_bad",
                "prompt": "x",
                "output": "json",
                "output_path": str(output_path),
                "json_schema": {
                    "type": "object",
                    "required": ["summary"],
                    "properties": {"summary": {"type": "string"}},
                },
            },
            events.append,
        )
    )

    assert [event["event"] for event in events] == ["error"]
    assert events[0]["reason_code"] == "schema_invalid"
    assert events[0]["terminal"] is True
    assert events[0]["schema_validation"] == validation
    assert events[0]["schema_validation"]["valid"] is False
    assert not output_path.exists()


def test_execute_generate_schema_invalid_preserves_existing_output(
    tmp_path, monkeypatch
):
    from solstone.think import models
    from solstone.think.talents import _execute_generate

    output_path = tmp_path / "out.json"
    original = b'{"summary": "old"}'
    output_path.write_bytes(original)
    validation = {
        "valid": False,
        "errors": [
            {
                "path": "/summary",
                "constraint": "type",
                "message": "42 is not of type 'string'",
            }
        ],
    }
    monkeypatch.setattr(
        models,
        "generate_with_result",
        lambda **kwargs: {
            "text": '{"summary": 42}',
            "usage": {"input_tokens": 1, "output_tokens": 3},
            "schema_validation": validation,
        },
    )
    events: list[dict] = []

    asyncio.run(
        _execute_generate(
            {
                "provider": "google",
                "model": "gemini-2.0-flash",
                "name": "schema_bad_existing",
                "prompt": "x",
                "output": "json",
                "output_path": str(output_path),
                "json_schema": {
                    "type": "object",
                    "required": ["summary"],
                    "properties": {"summary": {"type": "string"}},
                },
            },
            events.append,
        )
    )

    assert output_path.read_bytes() == original
    assert [event["event"] for event in events] == ["error"]
    assert events[0]["reason_code"] == "schema_invalid"
    assert events[0]["terminal"] is True


def test_execute_generate_schema_clean_writes_file_and_clean_provenance(
    tmp_path, monkeypatch
):
    from solstone.think import models
    from solstone.think.talent_provenance import read_provenance
    from solstone.think.talents import _execute_generate

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    output_path = tmp_path / "chronicle" / "20240101" / "talents" / "schema_clean.json"
    validation = {"valid": True, "errors": []}
    monkeypatch.setattr(
        models,
        "generate_with_result",
        lambda **kwargs: {
            "text": '{"summary": "ok"}',
            "usage": {"input_tokens": 1, "output_tokens": 3},
            "schema_validation": validation,
        },
    )
    events: list[dict] = []

    asyncio.run(
        _execute_generate(
            {
                "provider": "google",
                "model": "gemini-2.0-flash",
                "name": "schema_clean",
                "prompt": "x",
                "day": "20240101",
                "schedule": "daily",
                "output": "json",
                "output_path": str(output_path),
                "json_schema": {
                    "type": "object",
                    "required": ["summary"],
                    "properties": {"summary": {"type": "string"}},
                },
            },
            events.append,
        )
    )

    assert [event["event"] for event in events] == ["finish"]
    assert not [event for event in events if event["event"] == "error"]
    assert output_path.read_text(encoding="utf-8") == '{"summary": "ok"}'
    provenance = read_provenance(output_path)
    assert provenance is not None
    assert provenance["output_path"] == "chronicle/20240101/talents/schema_clean.json"


def test_execute_generate_md_output_without_schema_still_writes(tmp_path, monkeypatch):
    from solstone.think import models
    from solstone.think.talents import _execute_generate

    output_path = tmp_path / "out.md"
    monkeypatch.setattr(
        models,
        "generate_with_result",
        lambda **kwargs: {
            "text": "plain markdown",
            "usage": {"input_tokens": 1, "output_tokens": 3},
        },
    )
    events: list[dict] = []

    asyncio.run(
        _execute_generate(
            {
                "provider": "google",
                "model": "gemini-2.0-flash",
                "name": "md_gen",
                "prompt": "x",
                "output": "md",
                "output_path": str(output_path),
            },
            events.append,
        )
    )

    assert [event["event"] for event in events] == ["finish"]
    assert output_path.read_text(encoding="utf-8") == "plain markdown"


def test_execute_generate_json_empty_post_hook_result_finishes_without_write(
    tmp_path, monkeypatch
):
    from solstone.think import models, talents
    from solstone.think.talents import _execute_generate

    output_path = tmp_path / "out.json"
    monkeypatch.setattr(
        models,
        "generate_with_result",
        lambda **kwargs: {
            "text": '{"summary": "ok"}',
            "usage": {"input_tokens": 1, "output_tokens": 3},
            "schema_validation": {"valid": True, "errors": []},
        },
    )
    monkeypatch.setattr(
        talents, "load_post_hook", lambda config: lambda result, ctx: ""
    )
    events: list[dict] = []

    asyncio.run(
        _execute_generate(
            {
                "provider": "google",
                "model": "gemini-2.0-flash",
                "name": "empty_hook",
                "prompt": "x",
                "output": "json",
                "output_path": str(output_path),
                "json_schema": {
                    "type": "object",
                    "required": ["summary"],
                    "properties": {"summary": {"type": "string"}},
                },
            },
            events.append,
        )
    )

    assert [event["event"] for event in events] == ["finish"]
    assert not [event for event in events if event["event"] == "error"]
    assert not output_path.exists()


def test_execute_generate_json_without_schema_nonparseable_emits_schema_invalid(
    tmp_path, monkeypatch
):
    from solstone.think import models
    from solstone.think.talents import _execute_generate

    output_path = tmp_path / "out.json"
    monkeypatch.setattr(
        models,
        "generate_with_result",
        lambda **kwargs: {
            "text": "not json",
            "usage": {"input_tokens": 1, "output_tokens": 3},
        },
    )
    events: list[dict] = []

    asyncio.run(
        _execute_generate(
            {
                "provider": "google",
                "model": "gemini-2.0-flash",
                "name": "json_no_schema_bad",
                "prompt": "x",
                "output": "json",
                "output_path": str(output_path),
            },
            events.append,
        )
    )

    assert [event["event"] for event in events] == ["error"]
    assert events[0]["reason_code"] == "schema_invalid"
    assert events[0]["terminal"] is True
    assert events[0]["error"] == "talent output failed JSON schema validation"
    assert not output_path.exists()


def test_execute_generate_invalid_raw_repaired_by_hook_still_finishes(
    tmp_path, monkeypatch
):
    from solstone.think import models, talents
    from solstone.think.talents import _execute_generate

    output_path = tmp_path / "out.json"
    repaired = '{"summary": "repaired"}'
    validation = {
        "valid": False,
        "errors": [
            {
                "path": "",
                "constraint": "json_parse",
                "message": "Expecting value",
            }
        ],
    }
    monkeypatch.setattr(
        models,
        "generate_with_result",
        lambda **kwargs: {
            "text": "not json",
            "usage": {"input_tokens": 1, "output_tokens": 3},
            "schema_validation": validation,
        },
    )
    # Pulse/steward repair hooks depend on this raw-invalid/result-valid quadrant.
    monkeypatch.setattr(
        talents,
        "load_post_hook",
        lambda config: lambda result, ctx: repaired,
    )
    events: list[dict] = []

    asyncio.run(
        _execute_generate(
            {
                "provider": "google",
                "model": "gemini-2.0-flash",
                "name": "repaired_hook",
                "prompt": "x",
                "output": "json",
                "output_path": str(output_path),
                "json_schema": {
                    "type": "object",
                    "required": ["summary"],
                    "properties": {"summary": {"type": "string"}},
                },
            },
            events.append,
        )
    )

    assert [event["event"] for event in events] == ["finish"]
    assert not [event for event in events if event["event"] == "error"]
    assert output_path.read_text(encoding="utf-8") == repaired


def test_no_output_does_not_log_day_ok(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.talents")
    copy_day(tmp_path, monkeypatch)

    import solstone.think.talent as talent

    monkeypatch.setattr(talent, "TALENT_DIR", tmp_path)
    _write_generator_file(
        tmp_path,
        "blank_day_gen",
        {
            "type": "generate",
            "schedule": "daily",
            "priority": 10,
            "output": "md",
            "load": {"transcripts": True, "percepts": True},
        },
    )

    from solstone.think import models

    monkeypatch.setattr(
        models,
        "generate_with_result",
        lambda *a, **k: {"text": "   ", "usage": {"input_tokens": 1}},
    )
    monkeypatch.setenv("GOOGLE_API_KEY", "x")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    events = run_generator_with_config(
        mod,
        {
            "name": "blank_day_gen",
            "day": "20240101",
            "output": "md",
            "provider": "google",
            "model": "gemini-2.0-flash",
        },
        monkeypatch,
    )

    error_events = [e for e in events if e["event"] == "error"]
    assert len(error_events) == 1
    assert error_events[0]["reason_code"] == "no_output"
    assert [e for e in events if e["event"] == "finish"] == []

    task_log = tmp_path / "chronicle" / "20240101" / "task_log.txt"
    log_text = task_log.read_text(encoding="utf-8") if task_log.exists() else ""
    assert "talent blank_day_gen ok" not in log_text


def test_schema_invalid_does_not_log_day_ok(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.talents")
    copy_day(tmp_path, monkeypatch)

    import solstone.think.talent as talent

    monkeypatch.setattr(talent, "TALENT_DIR", tmp_path)
    _write_generator_file(
        tmp_path,
        "schema_invalid_day_gen",
        {
            "type": "generate",
            "schedule": "daily",
            "priority": 10,
            "output": "json",
            "schema": "schema_invalid_day_gen.schema.json",
            "load": {"transcripts": True, "percepts": True},
        },
    )
    _write_schema_file(
        tmp_path,
        "schema_invalid_day_gen.schema.json",
        {
            "type": "object",
            "required": ["summary"],
            "properties": {"summary": {"type": "string"}},
        },
    )

    from solstone.think import models

    monkeypatch.setattr(
        models,
        "generate_with_result",
        lambda *a, **k: {
            "text": '{"summary": 42}',
            "usage": {"input_tokens": 1, "output_tokens": 3},
            "schema_validation": {
                "valid": False,
                "errors": [
                    {
                        "path": "/summary",
                        "constraint": "type",
                        "message": "42 is not of type 'string'",
                    }
                ],
            },
        },
    )
    monkeypatch.setenv("GOOGLE_API_KEY", "x")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    events = run_generator_with_config(
        mod,
        {
            "name": "schema_invalid_day_gen",
            "day": "20240101",
            "output": "json",
            "provider": "google",
            "model": "gemini-2.0-flash",
        },
        monkeypatch,
    )

    error_events = [e for e in events if e["event"] == "error"]
    assert len(error_events) == 1
    assert error_events[0]["reason_code"] == "schema_invalid"
    assert [e for e in events if e["event"] == "finish"] == []

    task_log = tmp_path / "chronicle" / "20240101" / "task_log.txt"
    log_text = task_log.read_text(encoding="utf-8") if task_log.exists() else ""
    assert "talent schema_invalid_day_gen ok" not in log_text

    output_path = (
        tmp_path / "chronicle" / "20240101" / "talents" / "schema_invalid_day_gen.json"
    )
    assert not output_path.exists()


def test_execute_generate_blank_without_output_path_still_finishes(
    tmp_path, monkeypatch
):
    """Non-output-path generate runs preserve the old blank-finish behavior."""
    from solstone.think import models
    from solstone.think.talents import _execute_generate

    monkeypatch.setattr(
        models,
        "generate_with_result",
        lambda **kwargs: {"text": "", "usage": {"input_tokens": 1}},
    )
    events: list[dict] = []

    asyncio.run(
        _execute_generate(
            {
                "provider": "google",
                "model": "gemini-2.0-flash",
                "name": "blank_no_output_path",
                "prompt": "x",
            },
            events.append,
        )
    )

    assert [event["event"] for event in events] == ["finish"]
    assert events[0]["result"] == ""


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


def test_finish_event_includes_input_budget(tmp_path, monkeypatch):
    """Test that finish events surface bundled-local input_budget when returned."""
    mod = importlib.import_module("solstone.think.talents")
    copy_day(tmp_path, monkeypatch)

    from solstone.think import models, talent

    monkeypatch.setattr(talent, "TALENT_DIR", tmp_path)
    _write_generator_file(
        tmp_path,
        "input_budget_gen",
        {
            "type": "generate",
            "schedule": "daily",
            "priority": 10,
            "output": "md",
            "load": {"transcripts": True, "percepts": True},
        },
    )
    input_budget = {
        "clipped": True,
        "dropped_chars": 1234,
        "dropped_entries": 3,
        "budget_tokens": 12032,
    }

    monkeypatch.setattr(
        models,
        "generate_with_result",
        MagicMock(return_value={**MOCK_RESULT, "input_budget": input_budget}),
    )
    monkeypatch.setenv("GOOGLE_API_KEY", "x")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    events = run_generator_with_config(
        mod,
        {
            "name": "input_budget_gen",
            "day": "20240101",
            "output": "md",
            "provider": "google",
            "model": "gemini-2.0-flash",
        },
        monkeypatch,
    )

    finish_events = [e for e in events if e["event"] == "finish"]
    assert len(finish_events) == 1
    assert finish_events[0]["input_budget"] == input_budget


def test_finish_event_includes_request_budget(tmp_path, monkeypatch):
    """Test that finish events surface local request_budget when returned."""
    mod = importlib.import_module("solstone.think.talents")
    copy_day(tmp_path, monkeypatch)

    from solstone.think import models, talent

    monkeypatch.setattr(talent, "TALENT_DIR", tmp_path)
    _write_generator_file(
        tmp_path,
        "request_budget_gen",
        {
            "type": "generate",
            "schedule": "daily",
            "priority": 10,
            "output": "md",
            "load": {"transcripts": True, "percepts": True},
        },
    )
    request_budget = {
        "window": 32768,
        "slots": 2,
        "estimated_prompt_tokens": 100,
        "image_tokens": 0,
        "clamped_max_tokens": 512,
        "requested_max_output_tokens": 4096,
    }
    monkeypatch.setattr(
        models,
        "generate_with_result",
        MagicMock(return_value={**MOCK_RESULT, "request_budget": request_budget}),
    )
    monkeypatch.setenv("GOOGLE_API_KEY", "x")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    events = run_generator_with_config(
        mod,
        {
            "name": "request_budget_gen",
            "day": "20240101",
            "output": "md",
            "provider": "google",
            "model": "gemini-2.0-flash",
        },
        monkeypatch,
    )

    finish_events = [e for e in events if e["event"] == "finish"]
    assert len(finish_events) == 1
    assert finish_events[0]["request_budget"] == request_budget


def test_finish_event_omits_input_budget_when_absent(tmp_path, monkeypatch):
    """Test that finish events omit input_budget when the provider did not return it."""
    mod = importlib.import_module("solstone.think.talents")
    copy_day(tmp_path, monkeypatch)

    from solstone.think import models, talent

    monkeypatch.setattr(talent, "TALENT_DIR", tmp_path)
    _write_generator_file(
        tmp_path,
        "no_input_budget_gen",
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
            "name": "no_input_budget_gen",
            "day": "20240101",
            "output": "md",
            "provider": "google",
            "model": "gemini-2.0-flash",
        },
        monkeypatch,
    )

    finish_events = [e for e in events if e["event"] == "finish"]
    assert len(finish_events) == 1
    assert "input_budget" not in finish_events[0]


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


def test_generate_hook_error_emits_terminal_hook_error(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.talents")
    copy_day(tmp_path, monkeypatch)

    import solstone.think.talent as talent

    monkeypatch.setattr(talent, "TALENT_DIR", tmp_path)
    _write_generator_file(
        tmp_path,
        "hook_error_gen",
        {
            "type": "generate",
            "schedule": "daily",
            "priority": 10,
            "output": "md",
            "hook": {"post": "hook_error_gen"},
            "load": {"transcripts": True, "percepts": True},
        },
    )

    hook_file = tmp_path / "hook_error_gen.py"
    hook_file.write_text("""
def post_process(result, context):
    raise RuntimeError("hook boom")
""")

    from solstone.think import models

    monkeypatch.setattr(
        models,
        "generate_with_result",
        lambda *a, **k: MOCK_RESULT,
    )
    monkeypatch.setenv("GOOGLE_API_KEY", "x")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    events = run_generator_with_config(
        mod,
        {
            "name": "hook_error_gen",
            "day": "20240101",
            "output": "md",
            "provider": "google",
            "model": "gemini-2.0-flash",
        },
        monkeypatch,
    )

    assert [e for e in events if e["event"] == "finish"] == []
    error_events = [e for e in events if e["event"] == "error"]
    assert len(error_events) == 1
    assert error_events[0]["reason_code"] == "hook_error"
    assert error_events[0]["terminal"] is True
    assert (
        "post-hook 'hook_error_gen' failed for talent 'hook_error_gen'"
        in (error_events[0]["error"])
    )
    assert "hook boom" in error_events[0]["error"]


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
    config_dir = tmp_path / "config"
    config_dir.mkdir()
    (config_dir / "journal.json").write_text(
        json.dumps(
            {
                "providers": {
                    "active": {
                        "provider": "google",
                        "model": "gemini-2.0-flash",
                    }
                }
            }
        ),
        encoding="utf-8",
    )
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
