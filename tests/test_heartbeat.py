# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import argparse
import os

import pytest


@pytest.fixture
def journal_path(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "health").mkdir()
    return tmp_path


@pytest.fixture
def heartbeat_mocks(monkeypatch):
    state = {"run_calls": [], "append_calls": []}

    def fake_run_recipe_pass(today):
        state["run_calls"].append(today)
        return {"fired": [], "escalated_targets": [], "data_source_errors": []}

    def fake_append_steward_event(event, **fields):
        state["append_calls"].append((event, fields))

    monkeypatch.setattr(
        "solstone.think.heartbeat.setup_cli",
        lambda parser: argparse.Namespace(force=False),
    )
    monkeypatch.setattr(
        "solstone.think.heartbeat.run_recipe_pass",
        fake_run_recipe_pass,
    )
    monkeypatch.setattr(
        "solstone.think.heartbeat.append_steward_event",
        fake_append_steward_event,
    )
    return state


def test_heartbeat_main_is_callable():
    """solstone.think.heartbeat.main is a callable function."""
    from solstone.think.heartbeat import main

    assert callable(main)


def test_pid_guard_live_process_exits_zero(journal_path, heartbeat_mocks):
    """When PID file contains current process PID, main() exits 0 without a pass."""
    import solstone.think.heartbeat as mod

    pid_file = journal_path / "health" / "heartbeat.pid"
    pid_file.write_text(str(os.getpid()))

    with pytest.raises(SystemExit) as exc_info:
        mod.main()
    assert exc_info.value.code == 0
    assert heartbeat_mocks["run_calls"] == []
    assert heartbeat_mocks["append_calls"] == []


def test_pid_guard_dead_process_removes_stale_pid(journal_path, heartbeat_mocks):
    """When PID file contains a dead PID, main() removes it and runs the pass."""
    import solstone.think.heartbeat as mod

    pid_file = journal_path / "health" / "heartbeat.pid"
    dead_pid = 99999999
    try:
        os.kill(dead_pid, 0)
        pytest.skip("PID 99999999 is unexpectedly alive")
    except ProcessLookupError:
        pass

    pid_file.write_text(str(dead_pid))

    with pytest.raises(SystemExit) as exc_info:
        mod.main()
    assert exc_info.value.code == 0
    assert len(heartbeat_mocks["run_calls"]) == 1
    assert not pid_file.exists()


def test_pid_file_created_and_removed_on_success(
    journal_path, heartbeat_mocks, monkeypatch
):
    """PID file exists during execution and is removed after main() completes."""
    import solstone.think.heartbeat as mod

    pid_file = journal_path / "health" / "heartbeat.pid"
    pid_during_run = []

    def capture_pid_run(today):
        pid_during_run.append(pid_file.exists())
        if pid_file.exists():
            pid_during_run.append(pid_file.read_text().strip())
        return {"fired": [], "escalated_targets": [], "data_source_errors": []}

    monkeypatch.setattr(mod, "run_recipe_pass", capture_pid_run)

    with pytest.raises(SystemExit):
        mod.main()

    assert pid_during_run[0] is True
    assert pid_during_run[1] == str(os.getpid())
    assert not pid_file.exists()


def test_pid_file_removed_on_error(journal_path, heartbeat_mocks, monkeypatch):
    """PID file is removed when the deterministic pass raises."""
    import solstone.think.heartbeat as mod

    pid_file = journal_path / "health" / "heartbeat.pid"

    def fail_run(today):
        raise RuntimeError("boom")

    monkeypatch.setattr(mod, "run_recipe_pass", fail_run)

    with pytest.raises(SystemExit) as exc_info:
        mod.main()
    assert exc_info.value.code == 1
    assert not pid_file.exists()
    content = (journal_path / "health" / "heartbeat.log").read_text()
    assert "outcome=error" in content


def test_log_run_appends_line(journal_path):
    """_log_run appends a correctly formatted line to heartbeat.log."""
    import time

    from solstone.think.heartbeat import _log_run

    health_dir = journal_path / "health"
    start_time = time.monotonic() - 5

    _log_run(health_dir, start_time, "success")

    log_file = health_dir / "heartbeat.log"
    assert log_file.exists()
    content = log_file.read_text()
    assert content.endswith("\n")
    line = content.strip()
    assert "duration=" in line
    assert "outcome=success" in line


def test_log_written_after_successful_run(journal_path, heartbeat_mocks):
    """After a successful main() run, heartbeat.log has a success entry."""
    import solstone.think.heartbeat as mod

    with pytest.raises(SystemExit) as exc_info:
        mod.main()
    assert exc_info.value.code == 0

    log_file = journal_path / "health" / "heartbeat.log"
    assert log_file.exists()
    content = log_file.read_text()
    assert "outcome=success" in content


def test_pass_runs_once_and_no_cortex_path(journal_path, heartbeat_mocks):
    """A normal run invokes one deterministic pass and has no cortex symbols."""
    import solstone.think.heartbeat as mod

    with pytest.raises(SystemExit) as exc_info:
        mod.main()
    assert exc_info.value.code == 0

    assert len(heartbeat_mocks["run_calls"]) == 1
    assert not hasattr(mod, "cortex_request")
    assert not hasattr(mod, "wait_for_uses")


def test_pass_event_persisted(journal_path, heartbeat_mocks, monkeypatch):
    """A successful run records the deterministic pass result."""
    import solstone.think.heartbeat as mod

    monkeypatch.setattr(
        mod,
        "run_recipe_pass",
        lambda today: {
            "fired": [],
            "escalated_targets": [],
            "data_source_errors": [],
        },
    )

    with pytest.raises(SystemExit) as exc_info:
        mod.main()
    assert exc_info.value.code == 0

    assert heartbeat_mocks["append_calls"] == [
        (
            "pass",
            {
                "fired": [],
                "escalated_targets": [],
                "data_source_errors": [],
            },
        )
    ]


def test_recency_check_skips_recent_heartbeat(journal_path, heartbeat_mocks):
    """When heartbeat.log has a recent success, main() exits 0 without a pass."""
    from datetime import datetime

    import solstone.think.heartbeat as mod

    # Write a recent success entry
    log_file = journal_path / "health" / "heartbeat.log"
    recent_ts = datetime.now().isoformat(timespec="seconds")
    log_file.write_text(f"{recent_ts} duration=5s outcome=success\n")

    with pytest.raises(SystemExit) as exc_info:
        mod.main()
    assert exc_info.value.code == 0
    assert heartbeat_mocks["run_calls"] == []
    assert heartbeat_mocks["append_calls"] == []


def test_recency_check_runs_after_old_heartbeat(journal_path, heartbeat_mocks):
    """When heartbeat.log success is older than the window, main() runs the pass."""
    from datetime import datetime, timedelta

    import solstone.think.heartbeat as mod

    # Write an old success entry (24 hours ago)
    log_file = journal_path / "health" / "heartbeat.log"
    old_ts = (datetime.now() - timedelta(hours=24)).isoformat(timespec="seconds")
    log_file.write_text(f"{old_ts} duration=5s outcome=success\n")

    with pytest.raises(SystemExit):
        mod.main()
    assert len(heartbeat_mocks["run_calls"]) == 1


def test_force_flag_bypasses_recency_check(journal_path, heartbeat_mocks, monkeypatch):
    """--force runs full check even with a recent success."""
    import solstone.think.heartbeat as mod

    monkeypatch.setattr(
        "solstone.think.heartbeat.setup_cli",
        lambda parser: argparse.Namespace(force=True),
    )

    # Write a recent success entry
    from datetime import datetime

    log_file = journal_path / "health" / "heartbeat.log"
    recent_ts = datetime.now().isoformat(timespec="seconds")
    log_file.write_text(f"{recent_ts} duration=5s outcome=success\n")

    with pytest.raises(SystemExit):
        mod.main()
    assert len(heartbeat_mocks["run_calls"]) == 1


def test_last_success_time_parses_log(journal_path):
    """_last_success_time returns the timestamp of the most recent success."""
    from solstone.think.heartbeat import _last_success_time

    health_dir = journal_path / "health"
    log_file = health_dir / "heartbeat.log"
    log_file.write_text(
        "2026-03-19T08:00:00 duration=120s outcome=success\n"
        "2026-03-19T12:00:00 duration=5s outcome=error\n"
        "2026-03-19T14:00:00 duration=90s outcome=success\n"
    )

    result = _last_success_time(health_dir)
    assert result is not None
    assert result.hour == 14
    assert result.day == 19


def test_last_success_time_returns_none_for_no_log(journal_path):
    """_last_success_time returns None when no log file exists."""
    from solstone.think.heartbeat import _last_success_time

    result = _last_success_time(journal_path / "health")
    assert result is None


def test_last_success_time_returns_none_for_no_successes(journal_path):
    """_last_success_time returns None when log has no success entries."""
    from solstone.think.heartbeat import _last_success_time

    health_dir = journal_path / "health"
    log_file = health_dir / "heartbeat.log"
    log_file.write_text(
        "2026-03-19T08:00:00 duration=5s outcome=error\n"
        "2026-03-19T12:00:00 duration=5s outcome=error\n"
    )

    result = _last_success_time(health_dir)
    assert result is None


def test_think_emit_daily_complete_shape(monkeypatch):
    """think.emit('daily_complete', ...) calls _callosum.emit with correct tract and fields."""
    from unittest.mock import Mock

    import solstone.think.thinking as think_mod

    mock_conn = Mock()
    monkeypatch.setattr(think_mod, "_callosum", mock_conn)

    think_mod.emit(
        "daily_complete", day="20260318", success=3, failed=0, duration_ms=5000
    )

    mock_conn.emit.assert_called_once_with(
        "think",
        "daily_complete",
        day="20260318",
        success=3,
        failed=0,
        duration_ms=5000,
    )


def test_think_emit_noop_without_callosum(monkeypatch):
    """think.emit() does nothing when _callosum is None."""
    import solstone.think.thinking as think_mod

    monkeypatch.setattr(think_mod, "_callosum", None)
    think_mod.emit("daily_complete", day="20260318")
