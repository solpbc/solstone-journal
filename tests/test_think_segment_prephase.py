# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Regression witnesses for the daily segment-think pre-phase."""

import asyncio
import importlib
import json
import logging
import os
import subprocess
import threading
import time
from io import StringIO
from pathlib import Path
from unittest.mock import Mock

import pytest

from solstone.think.providers import fanout_policy
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


def _write_jsonl(path: Path, events: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        for event in events:
            handle.write(json.dumps(event) + "\n")


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


def _complete_segment_progress():
    from solstone.think.pipeline_health import SEGMENT_FLOOR_TALENTS, SegmentProgress

    floor = frozenset(SEGMENT_FLOOR_TALENTS)
    return SegmentProgress(
        sensed=True,
        density="active",
        change_class=None,
        dispatched=floor,
        completed=floor,
        unconfigured=frozenset(),
        capped=frozenset(),
    )


def _append_segment_terminal(
    journal: Path,
    *,
    name: str,
    event: str = "talent.complete",
    ts: int = 100,
    stream: str = STREAM,
) -> None:
    health_dir = journal / "chronicle" / DAY / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    path = health_dir / "001_segment.jsonl"
    with path.open("a", encoding="utf-8") as handle:
        handle.write(
            json.dumps(
                {
                    "event": event,
                    "ts": ts,
                    "mode": "segment",
                    "day": DAY,
                    "segment": ACTIVE_SEGMENT,
                    "stream": stream,
                    "name": name,
                }
            )
            + "\n"
        )


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
    monkeypatch.setattr(
        mod, "run_bounded_phase", lambda cmd, day, timeout=None: (True, False)
    )
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


def _forbid_slot_discovery(monkeypatch, mod):
    """Assert the non-local path never probes the local server."""
    del mod

    def _unreachable() -> int:
        raise AssertionError("slot discovery must not run for non-local defaults")

    monkeypatch.setattr(fanout_policy, "read_server_parallel_slots", _unreachable)


def _pin_describe_non_local(monkeypatch, mod):
    """Pin the describe default to its CPU formula.

    The fixture journal resolves observe.* to google, but these tests assert an
    exact -j value; stub the predicate so they do not silently depend on that.
    """
    monkeypatch.setattr(fanout_policy, "_describe_uses_local", lambda: False)
    _forbid_slot_discovery(monkeypatch, mod)


def test_sense_repair_prephase_uses_default_describe_jobs(journal_copy, monkeypatch):
    mod = importlib.import_module("solstone.think.thinking")
    bounded_calls = []
    daily_called = []

    _pin_describe_non_local(monkeypatch, mod)

    monkeypatch.setattr(fanout_policy.os, "cpu_count", lambda: 16)
    assert fanout_policy.default_describe_jobs() == 4
    monkeypatch.setattr(fanout_policy.os, "cpu_count", lambda: 7)
    assert fanout_policy.default_describe_jobs() == 1
    monkeypatch.setattr(fanout_policy.os, "cpu_count", lambda: 8)
    assert fanout_policy.default_describe_jobs() == 2

    def fake_bounded(cmd, day, timeout=None):
        bounded_calls.append((cmd, day, timeout))
        return (True, False)

    def fake_daily(day, verbose, **kwargs):
        daily_called.append(day)
        return (5, 0, [], set())

    _patch_main_runtime(monkeypatch)
    monkeypatch.setattr(mod, "run_bounded_phase", fake_bounded)
    monkeypatch.setattr(mod, "run_command", lambda cmd, day: True)
    monkeypatch.setattr(mod, "run_queued_command", lambda cmd, day, timeout=600: True)
    monkeypatch.setattr(mod, "run_daily_prompts", fake_daily)
    monkeypatch.setattr("sys.argv", ["sol think", "--day", "20240101"])

    mod.main()

    assert daily_called == ["20240101"]
    assert bounded_calls[0] == (
        ["journal", "sense", "--day", "20240101", "-j", "2"],
        "20240101",
        mod.DEFAULT_TASK_MAX_RUNTIME,
    )


def test_daily_segment_prephase_timeout_is_nonfatal(journal_copy, monkeypatch):
    mod = importlib.import_module("solstone.think.thinking")
    bounded_calls = []
    command_calls = []
    daily_called = []

    def fake_bounded(cmd, day, timeout=None):
        bounded_calls.append((cmd, day, timeout))
        return (False, True)

    def fake_command(cmd, day):
        command_calls.append(cmd)
        return True

    def fake_daily(day, verbose, **kwargs):
        daily_called.append(day)
        return (5, 0, [], set())

    _patch_main_runtime(monkeypatch)
    _pin_describe_non_local(monkeypatch, mod)
    monkeypatch.setattr(fanout_policy.os, "cpu_count", lambda: 16)
    monkeypatch.setattr(mod, "run_bounded_phase", fake_bounded)
    monkeypatch.setattr(mod, "run_command", fake_command)
    monkeypatch.setattr(mod, "run_queued_command", lambda cmd, day, timeout=600: True)
    monkeypatch.setattr(mod, "run_daily_prompts", fake_daily)
    monkeypatch.setattr("sys.argv", ["sol think", "--day", "20240101"])

    mod.main()

    health_dir = journal_copy / "chronicle" / "20240101" / "health"
    daily_files = sorted(health_dir.glob("*_daily.jsonl"))
    assert len(daily_files) == 1
    events = _read_jsonl(daily_files[0])
    segment_completes = [
        event
        for event in events
        if _event_name(event) == "phase.complete"
        and event.get("phase") == "segment_think"
    ]
    assert len(segment_completes) == 1
    complete = segment_completes[0]
    assert complete["success"] is False
    assert complete["reason_code"] == "wall_clock_exceeded"
    assert complete["timeout_seconds"] == 1800
    assert complete["bounded"] is True

    assert daily_called
    assert bounded_calls == [
        (
            ["journal", "sense", "--day", "20240101", "-j", "4"],
            "20240101",
            mod.DEFAULT_TASK_MAX_RUNTIME,
        ),
        (
            ["journal", "think", "--segments", "--day", "20240101"],
            "20240101",
            mod.DEFAULT_TASK_MAX_RUNTIME,
        ),
        (
            ["journal", "journal-stats"],
            "20240101",
            mod.JOURNAL_STATS_MAX_RUNTIME,
        ),
    ]
    assert mod.DEFAULT_TASK_MAX_RUNTIME == 1800
    assert mod.JOURNAL_STATS_MAX_RUNTIME == 600
    assert command_calls == []


def test_daily_segment_prephase_records_yield_and_progressing_repair(
    journal_copy, monkeypatch, caplog
):
    from solstone.think import catchup_state
    from solstone.think import thinking as mod

    day = "20240131"
    _seed_segment(journal_copy, day, ACTIVE_SEGMENT)

    def fake_bounded(cmd, day_arg, timeout=None):
        if cmd[:2] == ["journal", "sense"]:
            return (True, False)
        if cmd[:3] == ["journal", "think", "--segments"]:
            _write_jsonl(
                journal_copy / "chronicle" / day / "health" / "001_segment.jsonl",
                [
                    {
                        "event": "sense.complete",
                        "ts": 1,
                        "mode": "segment",
                        "day": day,
                        "segment": ACTIVE_SEGMENT,
                        "stream": STREAM,
                        "density": "active",
                    },
                    *[
                        {
                            "event": "talent.complete",
                            "ts": index + 2,
                            "mode": "segment",
                            "day": day,
                            "segment": ACTIVE_SEGMENT,
                            "stream": STREAM,
                            "name": name,
                        }
                        for index, name in enumerate(mod.SEGMENT_FLOOR_TALENTS)
                    ],
                ],
            )
            return (False, True)
        return (True, False)

    def fake_daily(day, verbose, **kwargs):
        return (1, 0, [], set())

    _patch_main_runtime(monkeypatch)
    monkeypatch.setattr(mod, "run_bounded_phase", fake_bounded)
    monkeypatch.setattr(mod, "run_queued_command", lambda cmd, day, timeout=600: True)
    monkeypatch.setattr(mod, "run_daily_prompts", fake_daily)
    monkeypatch.setattr("sys.argv", ["sol think", "--day", day])

    caplog.set_level(logging.WARNING)
    mod.main()

    health_dir = journal_copy / "chronicle" / day / "health"
    daily_files = sorted(health_dir.glob("*_daily.jsonl"))
    events = _read_jsonl(daily_files[0])
    segment_complete = next(
        event
        for event in events
        if _event_name(event) == "phase.complete"
        and event.get("phase") == "segment_think"
    )
    assert segment_complete["success"] is False
    assert segment_complete["cleared"] == 1
    assert segment_complete["remaining"] == 0

    record = catchup_state.read_day_record(day, catchup_state.KIND_SEGMENT_REPAIR)
    assert record["last_outcome"] == catchup_state.PROGRESSING_OUTCOME
    assert record["cleared"] == 1
    assert record["remaining"] == 0
    assert record["exit_reason"] == catchup_state.SEGMENT_REPAIR_TIMEOUT_REASON
    assert "Segment-think repair exceeded its 1800s budget" in caplog.text
    assert "yield: 1 cleared, 0 remaining" in caplog.text


def test_daily_sense_prephase_timeout_records_disposition(journal_copy, monkeypatch):
    mod = importlib.import_module("solstone.think.thinking")
    daily_called = []

    def fake_bounded(cmd, day, timeout=None):
        if cmd[:2] == ["journal", "sense"]:
            return (False, True)
        return (True, False)

    def fake_daily(day, verbose, **kwargs):
        daily_called.append(day)
        return (5, 0, [], set())

    _patch_main_runtime(monkeypatch)
    monkeypatch.setattr(mod, "run_bounded_phase", fake_bounded)
    monkeypatch.setattr(mod, "run_command", lambda cmd, day: True)
    monkeypatch.setattr(mod, "run_queued_command", lambda cmd, day, timeout=600: True)
    monkeypatch.setattr(mod, "run_daily_prompts", fake_daily)
    monkeypatch.setattr("sys.argv", ["sol think", "--day", "20240101"])

    mod.main()

    health_dir = journal_copy / "chronicle" / "20240101" / "health"
    daily_files = sorted(health_dir.glob("*_daily.jsonl"))
    assert len(daily_files) == 1
    events = _read_jsonl(daily_files[0])
    completes = [
        e
        for e in events
        if _event_name(e) == "phase.complete" and e.get("phase") == "sense_repair"
    ]
    assert len(completes) == 1
    complete = completes[0]
    assert complete["success"] is False
    assert complete["reason_code"] == "wall_clock_exceeded"
    assert complete["timeout_seconds"] == mod.DEFAULT_TASK_MAX_RUNTIME
    assert complete["bounded"] is True
    # Pipeline continued past the sense pre-phase into the daily prompts.
    assert daily_called


def test_daily_journal_stats_postphase_timeout_records_disposition(
    journal_copy, monkeypatch
):
    mod = importlib.import_module("solstone.think.thinking")
    daily_called = []

    def fake_bounded(cmd, day, timeout=None):
        if cmd[:2] == ["journal", "journal-stats"]:
            return (False, True)
        return (True, False)

    def fake_daily(day, verbose, **kwargs):
        daily_called.append(day)
        return (5, 0, [], set())

    _patch_main_runtime(monkeypatch)
    monkeypatch.setattr(mod, "run_bounded_phase", fake_bounded)
    monkeypatch.setattr(mod, "run_command", lambda cmd, day: True)
    monkeypatch.setattr(mod, "run_queued_command", lambda cmd, day, timeout=600: True)
    monkeypatch.setattr(mod, "run_daily_prompts", fake_daily)
    monkeypatch.setattr("sys.argv", ["sol think", "--day", "20240101"])

    # main() returning normally proves the post-phase did not wedge the pipeline.
    mod.main()

    health_dir = journal_copy / "chronicle" / "20240101" / "health"
    daily_files = sorted(health_dir.glob("*_daily.jsonl"))
    assert len(daily_files) == 1
    events = _read_jsonl(daily_files[0])
    completes = [
        e
        for e in events
        if _event_name(e) == "phase.complete" and e.get("phase") == "journal_stats"
    ]
    assert len(completes) == 1
    complete = completes[0]
    assert complete["success"] is False
    assert complete["reason_code"] == "wall_clock_exceeded"
    assert complete["timeout_seconds"] == mod.JOURNAL_STATS_MAX_RUNTIME
    assert complete["bounded"] is True
    assert daily_called


def test_daily_postphase_emits_mixed_storage_warnings(journal_copy, monkeypatch):
    mod = importlib.import_module("solstone.think.thinking")
    callosum = importlib.import_module("solstone.think.callosum")
    retention = importlib.import_module("solstone.think.retention")
    sent = []
    warnings = [
        {
            "level": "warning",
            "type": "disk_percent",
            "message": "disk warning",
            "current": 95.0,
            "threshold": 80,
        },
        {
            "level": "warning",
            "type": "offload_stalled",
            "message": "offload warning",
            "current": None,
            "threshold": None,
        },
    ]

    def fake_daily(day, verbose, **kwargs):
        return (5, 0, [], set())

    def fake_send(tract, event, **fields):
        sent.append((tract, event, fields))
        return True

    _patch_main_runtime(monkeypatch)
    monkeypatch.setattr(
        mod, "run_bounded_phase", lambda cmd, day, timeout=None: (True, False)
    )
    monkeypatch.setattr(mod, "run_command", lambda cmd, day: True)
    monkeypatch.setattr(mod, "run_queued_command", lambda cmd, day, timeout=600: True)
    monkeypatch.setattr(mod, "run_daily_prompts", fake_daily)
    monkeypatch.setattr(retention, "compute_storage_summary", lambda: object())
    monkeypatch.setattr(
        retention,
        "check_storage_health",
        lambda summary, journal_path: warnings,
    )
    monkeypatch.setattr(callosum, "callosum_send", fake_send)
    monkeypatch.setattr("sys.argv", ["sol think", "--day", "20240101"])

    mod.main()

    storage = [item for item in sent if item[:2] == ("storage", "warning")]
    notification = [item for item in sent if item[:2] == ("notification", "show")]
    assert storage == [
        (
            "storage",
            "warning",
            {
                "level": "warning",
                "type": "disk_percent",
                "message": "disk warning",
                "current": 95.0,
                "threshold": 80,
            },
        ),
        (
            "storage",
            "warning",
            {
                "level": "warning",
                "type": "offload_stalled",
                "message": "offload warning",
                "current": None,
                "threshold": None,
            },
        ),
    ]
    assert notification == [
        (
            "notification",
            "show",
            {
                "title": "Storage Warning",
                "message": "disk warning",
                "action": "/app/settings#storage",
            },
        )
    ]


def test_daily_segment_prephase_failure_has_no_timeout_reason(
    journal_copy, monkeypatch
):
    mod = importlib.import_module("solstone.think.thinking")
    daily_called = []

    def fake_daily(day, verbose, **kwargs):
        daily_called.append(day)
        return (5, 0, [], set())

    _patch_main_runtime(monkeypatch)
    monkeypatch.setattr(
        mod, "run_bounded_phase", lambda cmd, day, timeout=None: (False, False)
    )
    monkeypatch.setattr(mod, "run_command", lambda cmd, day: True)
    monkeypatch.setattr(mod, "run_queued_command", lambda cmd, day, timeout=600: True)
    monkeypatch.setattr(mod, "run_daily_prompts", fake_daily)
    monkeypatch.setattr("sys.argv", ["sol think", "--day", "20240101"])

    mod.main()

    health_dir = journal_copy / "chronicle" / "20240101" / "health"
    daily_files = sorted(health_dir.glob("*_daily.jsonl"))
    assert len(daily_files) == 1
    events = _read_jsonl(daily_files[0])
    segment_completes = [
        event
        for event in events
        if _event_name(event) == "phase.complete"
        and event.get("phase") == "segment_think"
    ]
    assert len(segment_completes) == 1
    complete = segment_completes[0]
    assert complete["success"] is False
    assert "reason_code" not in complete
    assert "timeout_seconds" not in complete
    assert "bounded" not in complete
    assert daily_called


def test_run_bounded_phase_timeout_with_exit_zero_propagates_failure(
    journal_copy, monkeypatch
):
    mod = importlib.import_module("solstone.think.thinking")
    log_path = journal_copy / "x.log"
    fake = Mock()
    fake.log_writer = Mock()
    fake.log_writer.path = log_path
    fake.name = "test"
    fake.wait.side_effect = subprocess.TimeoutExpired(cmd=["x"], timeout=0.01)
    fake.terminate.return_value = 0
    fake.cleanup.return_value = None
    monkeypatch.setattr(
        "solstone.think.runner.ManagedProcess.spawn", lambda *a, **k: fake
    )

    result = mod.run_bounded_phase(
        ["journal", "think", "--segments", "--day", DAY],
        DAY,
        timeout=0.01,
    )

    assert result == (False, True)


def test_run_bounded_phase_preserves_generation_env_for_batch_sense(
    journal_copy, monkeypatch, tmp_path
):
    mod = importlib.import_module("solstone.think.thinking")
    captured = {}
    fd = os.open(tmp_path / "generation.lock", os.O_RDWR | os.O_CREAT, 0o600)
    monkeypatch.setenv("SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_ID", "generation")
    monkeypatch.setenv("SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_FD", str(fd))
    monkeypatch.setenv("SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_TOKEN", "123")

    class FakePopen:
        def __init__(self, *args, **kwargs):
            captured["args"] = args
            captured["kwargs"] = kwargs
            self.pid = 1234
            self.stdout = StringIO("")
            self.stderr = StringIO("")
            self.returncode = 0

        def wait(self, timeout=None):
            return 0

        def poll(self):
            return self.returncode

        def terminate(self):
            self.returncode = -15

        def kill(self):
            self.returncode = -9

    monkeypatch.setattr("solstone.think.runner.subprocess.Popen", FakePopen)

    try:
        result = mod.run_bounded_phase(
            ["journal", "sense", "--day", DAY],
            DAY,
            timeout=mod.DEFAULT_TASK_MAX_RUNTIME,
        )
    finally:
        os.close(fd)

    assert result == (True, False)
    assert captured["kwargs"]["pass_fds"] == (fd,)
    assert captured["kwargs"]["env"] is None


def test_segment_health_log_receives_segment_talent_events(tmp_path, monkeypatch):
    from solstone.think import thinking as think

    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    _seed_segment(journal, DAY, ACTIVE_SEGMENT)

    segments = think.cluster_segments(DAY)
    assert any(segment["key"] == ACTIVE_SEGMENT for segment in segments)

    _patch_main_runtime(monkeypatch)
    # A tmp journal carries no local artifacts, so the real predicate resolves
    # non-local and the default must never probe the server.
    _forbid_slot_discovery(monkeypatch, think)
    monkeypatch.setattr(
        think,
        "get_talent_configs",
        lambda schedule=None, **kwargs: _segment_configs("sense"),
    )
    monkeypatch.setattr(
        think,
        "cortex_request",
        lambda prompt, name, config=None, **kwargs: f"agent-{name}",
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


def test_select_segment_repair_targets_skips_complete_and_unsensed(monkeypatch):
    from solstone.think import thinking as think

    complete = {
        "key": "090000_300",
        "stream": STREAM,
        "data_state": {"screen": "analyzed"},
    }
    incomplete = {
        "key": "090500_300",
        "stream": STREAM,
        "data_state": {"screen": "analyzed"},
    }
    raw_blocked = {
        "key": "091000_300",
        "stream": STREAM,
        "data_state": {"screen": "pending"},
    }
    segments = [complete, incomplete, raw_blocked]
    monkeypatch.setattr(
        think,
        "read_segment_progress",
        lambda day: {(STREAM, complete["key"]): _complete_segment_progress()},
    )

    selected, counts = think._select_segment_repair_targets(
        DAY,
        segments,
        force_all=False,
    )

    assert selected == [incomplete]
    assert counts == {
        "total": 3,
        "selected": 1,
        "complete": 1,
        "raw_blocked": 1,
    }


def test_select_segment_repair_targets_force_all_preserves_refresh_semantics(
    monkeypatch,
):
    from solstone.think import thinking as think

    segments = [
        {
            "key": "090000_300",
            "stream": STREAM,
            "data_state": {"screen": "analyzed"},
        },
        {
            "key": "090500_300",
            "stream": STREAM,
            "data_state": {"screen": "pending"},
        },
    ]
    monkeypatch.setattr(think, "read_segment_progress", lambda day: {})

    selected, counts = think._select_segment_repair_targets(
        DAY,
        segments,
        force_all=True,
    )

    assert selected == segments
    assert counts == {
        "total": 2,
        "selected": 2,
        "complete": 0,
        "raw_blocked": 0,
    }


def test_raw_media_pending_skip_becomes_repair_selectable_after_describe_output(
    monkeypatch,
):
    from solstone.think import thinking as think

    pending = {
        "key": "091000_300",
        "stream": STREAM,
        "data_state": {"screen": "pending"},
    }
    sensed = {
        "key": pending["key"],
        "stream": STREAM,
        "data_state": {"screen": "analyzed"},
    }
    monkeypatch.setattr(think, "read_segment_progress", lambda day: {})

    selected, counts = think._select_segment_repair_targets(
        DAY,
        [pending],
        force_all=False,
    )
    assert selected == []
    assert counts == {
        "total": 1,
        "selected": 0,
        "complete": 0,
        "raw_blocked": 1,
    }

    selected, counts = think._select_segment_repair_targets(
        DAY,
        [sensed],
        force_all=False,
    )
    assert selected == [sensed]
    assert counts == {
        "total": 1,
        "selected": 1,
        "complete": 0,
        "raw_blocked": 0,
    }


def test_media_terminal_no_sense_complete_is_selected_and_dispatched(monkeypatch):
    from solstone.think import thinking as think
    from solstone.think.pipeline_health import classify_segment_completion

    segment = {
        "key": "090000_300",
        "stream": STREAM,
        "start": "09:00",
        "end": "09:05",
        "data_state": {"screen": "analyzed"},
    }
    monkeypatch.setattr(think, "read_segment_progress", lambda day: {})

    completion = classify_segment_completion([segment], {})
    assert completion.blockers == [
        {
            "segment": segment["key"],
            "dimension": "not_thought",
            "detail": "no_sense_complete",
        }
    ]

    selected, counts = think._select_segment_repair_targets(
        DAY,
        [segment],
        force_all=False,
    )
    assert selected == [segment]
    assert counts == {
        "total": 1,
        "selected": 1,
        "complete": 0,
        "raw_blocked": 0,
    }

    dispatched = []

    def fake_run_segment_sense(**kwargs):
        dispatched.append((kwargs["stream"], kwargs["segment"]))
        return (1, 0, [])

    monkeypatch.setattr(think, "run_segment_sense", fake_run_segment_sense)
    monkeypatch.setattr(think, "resolve_predecessor", lambda *args: None)

    success, failed = think._run_segment_repair_batch(
        day=DAY,
        segments=selected,
        refresh=False,
        verbose=False,
        max_concurrency=1,
        segment_workers=1,
        timeout=None,
        skip_activity_prompts=False,
        skip_talents=frozenset(),
    )

    assert (success, failed) == (1, 0)
    assert dispatched == [(STREAM, segment["key"])]


def test_run_segment_repair_batch_respects_worker_bound(monkeypatch):
    from solstone.think import thinking as think

    segments = [
        {"key": f"090{i}00_300", "stream": STREAM, "start": "09:00", "end": "09:05"}
        for i in range(4)
    ]
    lock = threading.Lock()
    barrier = threading.Barrier(2, timeout=1.0)
    current = 0
    peak = 0

    def fake_run_segment_sense(**kwargs):
        nonlocal current, peak
        with lock:
            current += 1
            peak = max(peak, current)
        try:
            barrier.wait()
        except threading.BrokenBarrierError:
            pass
        time.sleep(0.02)
        with lock:
            current -= 1
        return (1, 0, [])

    monkeypatch.setattr(think, "run_segment_sense", fake_run_segment_sense)
    monkeypatch.setattr(think, "resolve_predecessor", lambda *args: None)

    success, failed = think._run_segment_repair_batch(
        day=DAY,
        segments=segments,
        refresh=False,
        verbose=False,
        max_concurrency=2,
        segment_workers=2,
        timeout=None,
        skip_activity_prompts=False,
        skip_talents=frozenset(),
    )

    assert (success, failed) == (4, 0)
    assert peak == 2


def test_segments_mode_targets_incomplete_tail_and_replays_full_day(
    tmp_path,
    monkeypatch,
):
    from solstone.think import thinking as think
    from solstone.think import utils as think_utils

    day = "20240117"
    journal = tmp_path / "journal"
    (journal / "chronicle" / day).mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setenv("SOL_SKIP_SUPERVISOR_CHECK", "1")
    think_utils._journal_path_cache = None
    _patch_main_runtime(monkeypatch)

    complete = {
        "key": "090000_300",
        "stream": STREAM,
        "start": "09:00",
        "end": "09:05",
        "data_state": {"screen": "analyzed"},
    }
    incomplete = {
        "key": "090500_300",
        "stream": STREAM,
        "start": "09:05",
        "end": "09:10",
        "data_state": {"screen": "analyzed"},
    }
    raw_blocked = {
        "key": "091000_300",
        "stream": STREAM,
        "start": "09:10",
        "end": "09:15",
        "data_state": {"screen": "pending"},
    }
    segments = [complete, incomplete, raw_blocked]
    calls: list[dict] = []
    replay_calls: list[dict] = []

    def fake_run_segment_sense(**kwargs):
        calls.append(kwargs)
        return (1, 0, [])

    monkeypatch.setattr(think, "cluster_segments", lambda day: segments)
    monkeypatch.setattr(
        think,
        "read_segment_progress",
        lambda day: {(STREAM, complete["key"]): _complete_segment_progress()},
    )
    monkeypatch.setattr(think, "run_segment_sense", fake_run_segment_sense)
    monkeypatch.setattr(think, "resolve_predecessor", lambda *args: None)
    monkeypatch.setattr(
        think,
        "_replay_activity_state_for_segments",
        lambda **kwargs: replay_calls.append(kwargs),
    )
    monkeypatch.setattr(
        "sys.argv",
        ["sol think", "--segments", "--day", day, "--segment-workers", "1"],
    )

    with pytest.raises(SystemExit) as excinfo:
        think.main()

    assert excinfo.value.code == 0
    assert [call["segment"] for call in calls] == [incomplete["key"]]
    assert calls[0]["state_machine"] is None
    assert calls[0]["skip_activity_prompts"] is True
    assert replay_calls[0]["segments"] == segments


def test_segments_mode_complete_day_noops_without_dispatch(tmp_path, monkeypatch):
    from solstone.think import thinking as think
    from solstone.think import utils as think_utils

    day = "20240118"
    journal = tmp_path / "journal"
    (journal / "chronicle" / day).mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setenv("SOL_SKIP_SUPERVISOR_CHECK", "1")
    think_utils._journal_path_cache = None
    _patch_main_runtime(monkeypatch)

    segment = {
        "key": "090000_300",
        "stream": STREAM,
        "start": "09:00",
        "end": "09:05",
        "data_state": {"screen": "analyzed"},
    }
    calls: list[dict] = []
    replay_calls: list[dict] = []
    monkeypatch.setattr(think, "cluster_segments", lambda day: [segment])
    monkeypatch.setattr(
        think,
        "read_segment_progress",
        lambda day: {(STREAM, segment["key"]): _complete_segment_progress()},
    )
    monkeypatch.setattr(
        think,
        "run_segment_sense",
        lambda **kwargs: calls.append(kwargs) or (1, 0, []),
    )
    monkeypatch.setattr(
        think,
        "_replay_activity_state_for_segments",
        lambda **kwargs: replay_calls.append(kwargs),
    )
    monkeypatch.setattr(
        "sys.argv",
        ["sol think", "--segments", "--day", day, "--segment-workers", "1"],
    )

    with pytest.raises(SystemExit) as excinfo:
        think.main()

    assert excinfo.value.code == 0
    assert calls == []
    assert replay_calls == []


def test_existing_segment_talent_output_prevents_second_llm_run(
    tmp_path,
    monkeypatch,
):
    from solstone.think import models, talents

    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    out = journal / "chronicle" / DAY / STREAM / ACTIVE_SEGMENT / "talents" / "flow.md"
    events: list[dict] = []
    called: list[str] = []

    def fake_generate_with_result(**kwargs):
        called.append("generate")
        return {"text": "FRESH", "usage": {"output_tokens": 500}}

    monkeypatch.setattr(models, "generate_with_result", fake_generate_with_result)
    monkeypatch.setattr(talents, "_run_pre_hooks", lambda config: {})

    config = {
        "type": "generate",
        "name": "flow",
        "provider": "google",
        "model": "x",
        "prompt": "think about this segment",
        "day": DAY,
        "segment": ACTIVE_SEGMENT,
        "stream": STREAM,
        "output_path": str(out),
        "output": "md",
        "refresh": False,
        "schedule": "segment",
    }

    # This test's analogue of test_talent_fallback's guard: the second run is
    # the healthy-day segment re-think and must use the cached output.
    asyncio.run(talents._run_talent(config, events.append, dry_run=False))
    assert len(called) == 1
    assert out.exists()

    _append_segment_terminal(journal, name="flow")

    second_events: list[dict] = []
    asyncio.run(talents._run_talent(config, second_events.append, dry_run=False))

    finish_events = [event for event in second_events if event.get("event") == "finish"]
    assert len(called) == 1
    assert finish_events[-1]["result"] == "FRESH"
    assert finish_events[-1]["cache_hit"] is True
    assert finish_events[-1]["output_changed"] is False
    assert "usage" not in finish_events[-1]

    changed_config = dict(config)
    changed_config["model"] = "y"
    third_events: list[dict] = []
    asyncio.run(talents._run_talent(changed_config, third_events.append, dry_run=False))
    assert len(called) == 2
    third_finish = [event for event in third_events if event.get("event") == "finish"][
        -1
    ]
    assert third_finish["cache_hit"] is False

    transcript_changed_config = dict(changed_config)
    transcript_changed_config["transcript"] = "new source content from the segment"
    fourth_events: list[dict] = []
    asyncio.run(
        talents._run_talent(
            transcript_changed_config,
            fourth_events.append,
            dry_run=False,
        )
    )
    assert len(called) == 3
    fourth_finish = [
        event for event in fourth_events if event.get("event") == "finish"
    ][-1]
    assert fourth_finish["cache_hit"] is False


def test_provenance_reuse_requires_successful_latest_terminal(
    tmp_path,
    monkeypatch,
):
    from solstone.think import models, talents

    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    out = (
        journal / "chronicle" / DAY / STREAM / ACTIVE_SEGMENT / "talents" / "screen.md"
    )
    called: list[str] = []

    def fake_generate_with_result(**kwargs):
        called.append("generate")
        return {"text": "FRESH", "usage": {"output_tokens": 500}}

    monkeypatch.setattr(models, "generate_with_result", fake_generate_with_result)
    monkeypatch.setattr(talents, "_run_pre_hooks", lambda config: {})

    config = {
        "type": "generate",
        "name": "screen",
        "provider": "google",
        "model": "x",
        "prompt": "think about this segment",
        "day": DAY,
        "segment": ACTIVE_SEGMENT,
        "stream": STREAM,
        "output_path": str(out),
        "output": "md",
        "refresh": False,
        "schedule": "segment",
    }

    asyncio.run(talents._run_talent(config, lambda event: None, dry_run=False))
    _append_segment_terminal(journal, name="screen", ts=100)
    _append_segment_terminal(journal, name="screen", event="talent.fail", ts=200)

    events: list[dict] = []
    asyncio.run(talents._run_talent(config, events.append, dry_run=False))

    assert len(called) == 2
    finish = [event for event in events if event.get("event") == "finish"][-1]
    assert finish["cache_hit"] is False


def test_refresh_identical_output_regenerates_without_output_changed(
    tmp_path,
    monkeypatch,
):
    from solstone.think import models, talents

    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    out = (
        journal / "chronicle" / DAY / STREAM / ACTIVE_SEGMENT / "talents" / "screen.md"
    )
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text("SAME", encoding="utf-8")
    called: list[str] = []

    def fake_generate_with_result(**kwargs):
        called.append("generate")
        return {"text": "SAME", "usage": {"output_tokens": 500}}

    monkeypatch.setattr(models, "generate_with_result", fake_generate_with_result)
    monkeypatch.setattr(talents, "_run_pre_hooks", lambda config: {})

    events: list[dict] = []
    asyncio.run(
        talents._run_talent(
            {
                "type": "generate",
                "name": "screen",
                "provider": "google",
                "model": "x",
                "prompt": "think about this segment",
                "day": DAY,
                "segment": ACTIVE_SEGMENT,
                "stream": STREAM,
                "output_path": str(out),
                "output": "md",
                "refresh": True,
                "schedule": "segment",
            },
            events.append,
            dry_run=False,
        )
    )

    finish = [event for event in events if event.get("event") == "finish"][-1]
    assert len(called) == 1
    assert finish["cache_hit"] is False
    assert finish["output_changed"] is False


def test_json_cache_reuse_requires_current_schema_validation(
    tmp_path,
    monkeypatch,
):
    from solstone.think import models, talents
    from solstone.think.talent_provenance import output_digest, write_provenance

    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    out = (
        journal / "chronicle" / DAY / STREAM / ACTIVE_SEGMENT / "talents" / "sense.json"
    )
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text('{"bad": true}', encoding="utf-8")

    config = {
        "type": "generate",
        "name": "sense",
        "provider": "google",
        "model": "x",
        "prompt": "think about this segment",
        "day": DAY,
        "segment": ACTIVE_SEGMENT,
        "stream": STREAM,
        "output_path": str(out),
        "output": "json",
        "json_schema": {
            "type": "object",
            "required": ["ok"],
            "properties": {"ok": {"type": "boolean"}},
        },
        "refresh": False,
        "schedule": "segment",
    }
    runtime_schema = talents.hydrate_runtime_enums(config["json_schema"])
    output_sha256, output_size = output_digest(out)
    write_provenance(
        out,
        identity_hash=talents._identity_hash(config, runtime_schema),
        output_sha256=output_sha256,
        output_size=output_size,
        provider="google",
        model="x",
        generation_params=talents._generation_params(config),
        completed_at_ms=100,
        use_id="seed",
        identity_fields={"name": "sense"},
    )
    _append_segment_terminal(journal, name="sense", ts=100)

    called: list[str] = []

    def fake_generate_with_result(**kwargs):
        called.append("generate")
        return {
            "text": '{"ok": true}',
            "usage": {"output_tokens": 500},
            "schema_validation": {"valid": True, "errors": []},
        }

    monkeypatch.setattr(models, "generate_with_result", fake_generate_with_result)
    monkeypatch.setattr(talents, "_run_pre_hooks", lambda config: {})

    events: list[dict] = []
    asyncio.run(talents._run_talent(config, events.append, dry_run=False))

    assert len(called) == 1
    finish = [event for event in events if event.get("event") == "finish"][-1]
    assert finish["cache_hit"] is False
    assert out.read_text(encoding="utf-8") == '{"ok": true}'


def test_identity_hash_ignores_source_count_vocabulary():
    from solstone.think import talents

    config = {
        "type": "generate",
        "name": "sense",
        "provider": "google",
        "model": "x",
        "prompt": "think about this segment",
        "day": DAY,
        "segment": ACTIVE_SEGMENT,
        "stream": STREAM,
        "output": "json",
        "json_schema": {
            "type": "object",
            "required": ["ok"],
            "properties": {"ok": {"type": "boolean"}},
        },
        "schedule": "segment",
        "sources": {
            "transcripts": True,
            "percepts": False,
            "talents": {"sense": True},
        },
        "source_counts": {"transcripts": 1, "percepts": 0, "agents": 1},
    }
    runtime_schema = talents.hydrate_runtime_enums(config["json_schema"])
    renamed_counts = {
        **config,
        "source_counts": {"transcripts": 1, "percepts": 0, "talents": 1},
    }

    assert talents._identity_hash(config, runtime_schema) == talents._identity_hash(
        renamed_counts,
        runtime_schema,
    )


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
        lambda prompt, name, config=None, **kwargs: f"agent-{name}",
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
    health_log = journal / "chronicle" / DAY / "health" / "activity_test.jsonl"
    writer = think.ThinkingJSONLWriter(str(health_log))
    monkeypatch.setattr(think, "_jsonl", writer)

    try:
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

        changed = _active_sense()
        changed["activity_summary"] = "Writing tests with new context"
        _write_sense_output(
            journal / "chronicle" / DAY / STREAM / ACTIVE_SEGMENT,
            changed,
        )
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
    finally:
        writer.close()

    record_path = journal / "facets" / FACET / "activities" / f"{DAY}.jsonl"
    records = _read_jsonl(record_path)
    activity_id = make_activity_id("coding", ACTIVE_SEGMENT)
    matching = [record for record in records if record.get("id") == activity_id]

    assert len(matching) == 1
    assert len(activity_calls) == 2
    assert activity_calls[0]["refresh"] is False
    assert activity_calls[1]["refresh"] is False
    health_events = _read_jsonl(health_log)
    assert any(event.get("event") == "activity.unchanged" for event in health_events)


def test_activity_replay_treats_malformed_sense_like_absent(
    tmp_path,
    monkeypatch,
    caplog,
):
    from solstone.think import thinking as think
    from solstone.think.activity_state_machine import ActivityStateMachine

    ending_segment = "091000_300"
    segments = [
        {"stream": STREAM, "key": ACTIVE_SEGMENT},
        {"stream": STREAM, "key": IDLE_SEGMENT},
        {"stream": STREAM, "key": ending_segment},
    ]

    monkeypatch.setattr(
        "solstone.think.activity_state_machine.time.time",
        lambda: 123.456,
    )
    monkeypatch.setattr(
        think,
        "run_activity_prompts",
        lambda **kwargs: True,
    )
    monkeypatch.setattr(think, "_callosum", None)
    state_machines: list[ActivityStateMachine] = []

    class CapturingActivityStateMachine(ActivityStateMachine):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, **kwargs)
            state_machines.append(self)

    monkeypatch.setattr(think, "ActivityStateMachine", CapturingActivityStateMachine)

    def run_case(journal: Path, *, malformed: bool) -> tuple[bytes, bytes]:
        start_index = len(state_machines)
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
        _seed_segment(journal, DAY, ACTIVE_SEGMENT, _active_sense())
        middle_dir = _seed_segment(journal, DAY, IDLE_SEGMENT, _active_sense())
        _seed_segment(journal, DAY, ending_segment, _idle_sense())
        middle_sense = middle_dir / "talents" / "sense.json"
        if malformed:
            _write_sense_output(
                middle_dir,
                {
                    "density": "active",
                    "activity_summary": "Malformed cached output",
                    "entities": [],
                    "facets": [{"facet": FACET, "level": "high"}],
                    "recommend": {},
                },
            )
        else:
            middle_sense.unlink()

        think._replay_activity_state_for_segments(
            day=DAY,
            segments=segments,
            refresh=False,
            verbose=False,
            max_concurrency=2,
            skip_activity_prompts=False,
        )
        record_path = journal / "facets" / FACET / "activities" / f"{DAY}.jsonl"
        state_machine = state_machines[start_index]
        state_snapshot = {
            "last_segment_key": state_machine.last_segment_key,
            "last_segment_day": state_machine.last_segment_day,
            "active": {
                facet: {k: v for k, v in entry.items() if k != "_change"}
                for facet, entry in state_machine.state.items()
            },
        }
        return record_path.read_bytes(), json.dumps(
            state_snapshot, sort_keys=True
        ).encode()

    caplog.set_level(logging.WARNING)
    malformed_records, malformed_state = run_case(
        tmp_path / "malformed", malformed=True
    )
    absent_records, absent_state = run_case(tmp_path / "absent", malformed=False)

    assert malformed_records == absent_records
    assert malformed_state == absent_state
    assert "missing required keys: content_type" in caplog.text


@pytest.mark.parametrize("missing_key", ["density", "content_type"])
def test_run_segment_sense_reports_invalid_sense_output(
    tmp_path,
    monkeypatch,
    caplog,
    missing_key,
):
    from solstone.think import thinking as think
    from solstone.think.activity_state_machine import ActivityStateMachine

    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    malformed = _active_sense()
    malformed.pop(missing_key)
    _seed_segment(journal, DAY, ACTIVE_SEGMENT, malformed)

    monkeypatch.setattr(
        think,
        "get_talent_configs",
        lambda schedule=None, **kwargs: _segment_configs("sense"),
    )
    monkeypatch.setattr(
        think,
        "cortex_request",
        lambda prompt, name, config=None, **kwargs: f"agent-{name}",
    )
    monkeypatch.setattr(
        think,
        "wait_for_uses",
        lambda agent_ids, timeout=600: ({aid: "finish" for aid in agent_ids}, []),
    )
    emitted: list[tuple[str, str, dict]] = []

    class CaptureCallosum:
        def emit(self, tract, event, **fields):
            emitted.append((tract, event, fields))

    monkeypatch.setattr(think, "_callosum", CaptureCallosum())
    caplog.set_level(logging.WARNING)

    success, failed, failed_names = think.run_segment_sense(
        DAY,
        ACTIVE_SEGMENT,
        refresh=False,
        verbose=False,
        stream=STREAM,
        state_machine=ActivityStateMachine(),
    )

    assert success == 1
    assert failed == 1
    assert failed_names == ["sense (output_invalid)"]
    assert f"missing required keys: {missing_key}" in caplog.text
    completed = [event for event in emitted if event[1] == "completed"]
    assert completed[-1][2]["failed"] == 1
    assert completed[-1][2]["failed_names"] == ["sense (output_invalid)"]


def test_cache_hit_priority_drain_suppresses_rescan_but_counts_segment_complete(
    tmp_path,
    monkeypatch,
):
    from solstone.think import thinking as think
    from solstone.think.pipeline_health import (
        read_completed_since,
        read_segment_progress,
    )

    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    output_path = (
        journal / "chronicle" / DAY / STREAM / ACTIVE_SEGMENT / "talents" / "screen.md"
    )
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text("cached", encoding="utf-8")

    use_id = "1700000000000"
    use_dir = journal / "talents" / "screen"
    use_dir.mkdir(parents=True, exist_ok=True)
    (use_dir / f"{use_id}.jsonl").write_text(
        json.dumps(
            {
                "event": "finish",
                "ts": 300,
                "use_id": use_id,
                "name": "screen",
                "result": "cached",
                "cache_hit": True,
                "output_changed": False,
                "completed_at_ms": 100,
            }
        )
        + "\n",
        encoding="utf-8",
    )

    health_log = journal / "chronicle" / DAY / "health" / "segment.jsonl"
    writer = think.ThinkingJSONLWriter(str(health_log))
    monkeypatch.setattr(think, "_jsonl", writer)
    monkeypatch.setattr(
        think,
        "wait_for_uses",
        lambda agent_ids, timeout=610: ({use_id: "finish"}, []),
    )
    rescan_calls: list[list[str]] = []
    monkeypatch.setattr(
        think,
        "run_queued_command",
        lambda cmd, day, timeout=600: rescan_calls.append(cmd) or True,
    )

    try:
        success, failed, failed_names = think._drain_priority_batch(
            [(use_id, "screen", {"type": "generate", "output": "md"}, None)],
            "segment",
            DAY,
            ACTIVE_SEGMENT,
            STREAM,
        )
    finally:
        writer.close()

    assert (success, failed, failed_names) == (1, 0, [])
    assert rescan_calls == []
    health_events = _read_jsonl(health_log)
    complete = [
        event for event in health_events if event.get("event") == "talent.complete"
    ][-1]
    assert complete["cache_hit"] is True
    assert complete["completed_at_ms"] == 100
    assert read_completed_since(DAY, 0).segments == ()
    assert "screen" in read_segment_progress(DAY)[(STREAM, ACTIVE_SEGMENT)].completed


def test_segments_mode_zero_segment_noop(tmp_path, monkeypatch, caplog):
    from solstone.think import thinking as think

    day = "20240116"
    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    _patch_main_runtime(monkeypatch)
    _forbid_slot_discovery(monkeypatch, think)
    monkeypatch.setattr("sys.argv", ["sol think", "--segments", "--day", day])

    caplog.set_level(logging.INFO)
    with pytest.raises(SystemExit) as excinfo:
        think.main()

    assert excinfo.value.code == 0
    assert f"No segments found for {day}" in caplog.text


def _patch_segment_run(monkeypatch, think, journal):
    """Seed a runnable segment and stub the talent machinery around it."""
    _seed_segment(journal, DAY, ACTIVE_SEGMENT)
    _patch_main_runtime(monkeypatch)
    monkeypatch.setattr(
        think,
        "get_talent_configs",
        lambda schedule=None, **kwargs: _segment_configs("sense"),
    )
    monkeypatch.setattr(
        think,
        "cortex_request",
        lambda prompt, name, config=None, **kwargs: f"agent-{name}",
    )
    monkeypatch.setattr(
        think,
        "wait_for_uses",
        lambda agent_ids, timeout=600: ({aid: "finish" for aid in agent_ids}, []),
    )


def test_segments_default_local_slot_fallback_logs_once_across_call_sites(
    tmp_path, monkeypatch, caplog
):
    """Discovery failure yields the floor value and logs the fallback once.

    ``--segments`` calls ``default_segment_workers()`` twice in one process:
    once in argument validation and once in the run path. The tmp journal has
    no ``health/local.port``, so discovery fails without any network I/O.
    """
    from solstone.think import thinking as think

    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    _patch_segment_run(monkeypatch, think, journal)
    monkeypatch.setattr(fanout_policy, "_segment_work_uses_local", lambda: True)
    monkeypatch.setattr(fanout_policy.os, "cpu_count", lambda: 12)
    monkeypatch.setattr("sys.argv", ["sol think", "--segments", "--day", DAY])

    assert not (journal / "health" / "local.port").exists()

    derived = []
    original_default = fanout_policy.default_segment_workers

    def spy_default() -> int:
        value = original_default()
        derived.append(value)
        return value

    monkeypatch.setattr(fanout_policy, "default_segment_workers", spy_default)

    caplog.set_level(logging.INFO)
    with pytest.raises(SystemExit) as excinfo:
        think.main()
    assert excinfo.value.code == 0

    # Validation (thinking.py:4161) and the run path (:4270) both call it.
    assert derived == [1, 1]

    messages = [record.getMessage() for record in caplog.records]
    fallbacks = [m for m in messages if "local_server_parallel_slots fallback" in m]
    assert fallbacks == [
        "local_server_parallel_slots fallback slots=1 port=None "
        "context_tokens=None source=default"
    ]

    caps = [m for m in messages if "default_segment_workers capped" in m]
    assert caps == [
        "default_segment_workers capped provider=local slots=1 formula=6 derived=1"
    ]


def test_segments_explicit_segment_workers_bypasses_local_default_at_call_site(
    tmp_path, monkeypatch
):
    """An explicit --segment-workers wins over a smaller derived default."""
    from solstone.think import thinking as think

    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    _patch_segment_run(monkeypatch, think, journal)

    # The derived default would be 1; the CLI asks for 6.
    monkeypatch.setattr(fanout_policy, "_segment_work_uses_local", lambda: True)
    monkeypatch.setattr(fanout_policy, "read_server_parallel_slots", lambda: 1)
    monkeypatch.setattr(fanout_policy.os, "cpu_count", lambda: 12)
    assert fanout_policy.default_segment_workers() == 1

    observed = []
    original_batch = think._run_segment_repair_batch

    def spy_batch(**kwargs):
        observed.append(kwargs["segment_workers"])
        return original_batch(**kwargs)

    monkeypatch.setattr(think, "_run_segment_repair_batch", spy_batch)
    monkeypatch.setattr(
        "sys.argv",
        ["sol think", "--segments", "--day", DAY, "--segment-workers", "6"],
    )

    with pytest.raises(SystemExit) as excinfo:
        think.main()

    assert excinfo.value.code == 0
    assert observed == [6]
