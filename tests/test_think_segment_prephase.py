# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Regression witnesses for the daily segment-think pre-phase."""

import asyncio
import importlib
import json
import logging
from pathlib import Path

import pytest

from tests.test_think_segment import _segment_configs, _write_sense_output

DAY = "20240115"
STREAM = "default"
ACTIVE_SEGMENT = "090000_300"
IDLE_SEGMENT = "090500_300"
FACET = "work"


class NullCallosumConnection:
    def __init__(self, *args, **kwargs) -> None:
        pass

    def start(self, callback=None) -> None:
        return None

    def emit(self, *args, **kwargs) -> None:
        return None

    def stop(self) -> None:
        return None


def _read_jsonl(path: Path) -> list[dict]:
    return [
        json.loads(line)
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]


def _event_name(event: dict) -> str:
    return str(event.get("event") or event.get("type") or "")


def _active_sense() -> dict:
    return {
        "density": "active",
        "content_type": "coding",
        "activity_summary": "Writing tests",
        "entities": [],
        "facets": [{"facet": FACET, "level": "high"}],
        "recommend": {},
    }


def _idle_sense() -> dict:
    return {
        "density": "idle",
        "content_type": "idle",
        "activity_summary": "",
        "entities": [],
        "facets": [],
        "recommend": {},
    }


def _seed_segment(
    journal: Path,
    day: str,
    segment: str,
    sense_json: dict | None = None,
) -> Path:
    segment_dir = journal / "chronicle" / day / STREAM / segment
    (segment_dir / "talents").mkdir(parents=True, exist_ok=True)
    (segment_dir / "screen.jsonl").write_text(
        json.dumps({"timestamp": f"{day}T09:00:00"}) + "\n",
        encoding="utf-8",
    )
    _write_sense_output(segment_dir, sense_json or _active_sense())
    return segment_dir


def _patch_main_runtime(monkeypatch: pytest.MonkeyPatch) -> None:
    from solstone.think import thinking as think

    monkeypatch.setattr(think, "CallosumConnection", NullCallosumConnection)
    monkeypatch.setattr(think, "check_callosum_available", lambda: True)


def test_daily_health_log_keeps_segment_events_out(journal_copy, monkeypatch):
    mod = importlib.import_module("solstone.think.thinking")

    def mock_run_command(cmd, day):
        return True

    def mock_run_queued_command(cmd, day, timeout=600):
        return True

    def mock_run_daily_prompts(day, verbose, **kwargs):
        return (5, 0, [], set())

    _patch_main_runtime(monkeypatch)
    monkeypatch.setattr(mod, "run_command", mock_run_command)
    monkeypatch.setattr(mod, "run_queued_command", mock_run_queued_command)
    monkeypatch.setattr(mod, "run_daily_prompts", mock_run_daily_prompts)
    monkeypatch.setattr("sys.argv", ["sol think", "--day", "20240101"])

    mod.main()

    health_dir = journal_copy / "chronicle" / "20240101" / "health"
    daily_files = sorted(health_dir.glob("*_daily.jsonl"))
    assert len(daily_files) == 1

    events = _read_jsonl(daily_files[0])
    assert any(
        event.get("phase") == "segment_think"
        and _event_name(event).startswith("phase.")
        for event in events
    )
    assert not [
        event
        for event in events
        if _event_name(event).startswith(("talent.", "activity."))
    ]


def test_segment_health_log_receives_segment_talent_events(tmp_path, monkeypatch):
    from solstone.think import thinking as think

    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    _seed_segment(journal, DAY, ACTIVE_SEGMENT)

    segments = think.cluster_segments(DAY)
    assert any(segment["key"] == ACTIVE_SEGMENT for segment in segments)

    _patch_main_runtime(monkeypatch)
    monkeypatch.setattr(
        think,
        "get_talent_configs",
        lambda schedule=None, **kwargs: _segment_configs("sense"),
    )
    monkeypatch.setattr(
        think,
        "cortex_request",
        lambda prompt, name, config=None: f"agent-{name}",
    )
    monkeypatch.setattr(
        think,
        "wait_for_uses",
        lambda agent_ids, timeout=600: ({aid: "finish" for aid in agent_ids}, []),
    )
    monkeypatch.setattr("sys.argv", ["sol think", "--segments", "--day", DAY])

    with pytest.raises(SystemExit) as excinfo:
        think.main()

    assert excinfo.value.code == 0
    health_dir = journal / "chronicle" / DAY / "health"
    segment_files = sorted(health_dir.glob("*_segment.jsonl"))
    assert len(segment_files) == 1
    assert any(
        _event_name(event).startswith("talent.")
        for event in _read_jsonl(segment_files[0])
    )
    assert not list(health_dir.glob("*_daily.jsonl"))


def test_existing_segment_talent_output_prevents_second_llm_run(
    tmp_path,
    monkeypatch,
):
    from solstone.think import talents

    out = (
        tmp_path
        / "chronicle"
        / DAY
        / STREAM
        / ACTIVE_SEGMENT
        / "talents"
        / "entities.md"
    )
    events: list[dict] = []
    called: list[int] = []

    async def fake_execute(config, emit_event):
        called.append(1)
        output_path = Path(config["output_path"])
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_text("FRESH", encoding="utf-8")
        emit_event({"event": "finish", "ts": 0, "result": "FRESH"})

    monkeypatch.setattr(talents, "_execute_generate", fake_execute)
    monkeypatch.setattr(talents, "_run_pre_hooks", lambda config: {})

    config = {
        "type": "generate",
        "name": "entities",
        "provider": "google",
        "model": "x",
        "prompt": "think about this segment",
        "output_path": str(out),
        "refresh": False,
        "schedule": "segment",
    }

    # Lode-level analogue of test_talent_fallback's guard: the second run is
    # the healthy-day segment re-think and must use the cached output.
    asyncio.run(talents._run_talent(config, events.append, dry_run=False))
    assert len(called) == 1
    assert out.exists()

    second_events: list[dict] = []
    asyncio.run(talents._run_talent(config, second_events.append, dry_run=False))

    finish_events = [event for event in second_events if event.get("event") == "finish"]
    assert len(called) == 1
    assert finish_events[-1]["result"] == "FRESH"
    assert "usage" not in finish_events[-1]


def test_activity_replay_dedupes_records_and_preserves_non_refresh(
    tmp_path,
    monkeypatch,
):
    from solstone.think import thinking as think
    from solstone.think.activities import make_activity_id
    from solstone.think.activity_state_machine import ActivityStateMachine

    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    _seed_segment(journal, DAY, ACTIVE_SEGMENT, _active_sense())
    _seed_segment(journal, DAY, IDLE_SEGMENT, _idle_sense())

    activity_calls: list[dict] = []

    monkeypatch.setattr(
        think,
        "get_talent_configs",
        lambda schedule=None, **kwargs: _segment_configs("sense"),
    )
    monkeypatch.setattr(
        think,
        "cortex_request",
        lambda prompt, name, config=None: f"agent-{name}",
    )
    monkeypatch.setattr(
        think,
        "wait_for_uses",
        lambda agent_ids, timeout=600: ({aid: "finish" for aid in agent_ids}, []),
    )
    monkeypatch.setattr(
        think,
        "run_activity_prompts",
        lambda **kwargs: activity_calls.append(kwargs) or True,
    )
    monkeypatch.setattr(think, "_callosum", None)
    monkeypatch.setattr(think, "_jsonl", None)

    for _ in range(2):
        state_machine = ActivityStateMachine()
        for segment in (ACTIVE_SEGMENT, IDLE_SEGMENT):
            think.run_segment_sense(
                DAY,
                segment,
                refresh=False,
                verbose=False,
                stream=STREAM,
                state_machine=state_machine,
            )

    record_path = journal / "facets" / FACET / "activities" / f"{DAY}.jsonl"
    records = _read_jsonl(record_path)
    activity_id = make_activity_id("coding", ACTIVE_SEGMENT)
    matching = [record for record in records if record.get("id") == activity_id]

    assert len(matching) == 1
    assert len(activity_calls) == 2
    # run_activity_prompts has no parent-side output-exists short-circuit; the
    # child _run_talent cache guard covered above prevents the actual re-fire.
    assert activity_calls[-1]["refresh"] is False


def test_segments_mode_zero_segment_noop(tmp_path, monkeypatch, caplog):
    from solstone.think import thinking as think

    day = "20240116"
    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    _patch_main_runtime(monkeypatch)
    monkeypatch.setattr("sys.argv", ["sol think", "--segments", "--day", day])

    caplog.set_level(logging.INFO)
    with pytest.raises(SystemExit) as excinfo:
        think.main()

    assert excinfo.value.code == 0
    assert f"No segments found for {day}" in caplog.text
