# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for think.runner and logs tract integration."""

import os
import signal
import subprocess
import time
from io import StringIO
from unittest.mock import Mock, call

import pytest

from solstone.think.runner import ManagedProcess, run_task


@pytest.fixture
def journal_path(tmp_path, monkeypatch):
    """Set up a temporary journal path."""
    journal = tmp_path / "journal"
    journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    yield journal


def _managed_for_process(process):
    return ManagedProcess(
        process=process,
        name="test",
        log_writer=Mock(),
        cmd=["test"],
        _threads=[],
        ref="ref",
        _start_time=time.time(),
        _callosum=None,
    )


def test_terminate_uses_process_group(monkeypatch):
    killpg = Mock()
    monkeypatch.setattr("solstone.think.runner.os.getpgid", lambda pid: 456)
    monkeypatch.setattr("solstone.think.runner.os.killpg", killpg)

    graceful_process = Mock()
    graceful_process.pid = 123
    graceful_process.wait.return_value = -15
    graceful_process.returncode = -15
    graceful = _managed_for_process(graceful_process)

    assert graceful.terminate(timeout=2) == -15
    graceful_process.terminate.assert_called_once_with()
    graceful_process.wait.assert_called_once_with(timeout=2)
    graceful_process.kill.assert_not_called()
    killpg.assert_called_once_with(456, signal.SIGTERM)

    killpg.reset_mock()
    timeout_process = Mock()
    timeout_process.pid = 124
    timeout_process.wait.side_effect = [
        subprocess.TimeoutExpired(cmd=["test"], timeout=2),
        -9,
    ]
    timeout_process.returncode = -9
    timeout = _managed_for_process(timeout_process)

    with pytest.raises(subprocess.TimeoutExpired):
        timeout.terminate(timeout=2)

    timeout_process.terminate.assert_called_once_with()
    timeout_process.kill.assert_called_once_with()
    assert timeout_process.wait.call_args_list == [call(timeout=2), call()]
    assert killpg.call_args_list == [
        call(456, signal.SIGTERM),
        call(456, signal.SIGKILL),
    ]


def test_managed_process_has_ref_and_pid(journal_path, mock_callosum):
    """Test that ManagedProcess exposes ref and pid."""
    managed = ManagedProcess.spawn(["echo", "test"])

    # Verify ref and pid are accessible
    assert managed.ref is not None
    assert isinstance(managed.ref, str)
    assert managed.pid > 0
    assert isinstance(managed.pid, int)
    assert managed.name == "echo"  # Derived from cmd[0]

    # Wait and cleanup
    managed.wait()
    managed.cleanup()


def test_managed_process_uses_ref_as_ref(journal_path, mock_callosum):
    """Test that ref becomes the ref when provided."""
    ref = "1730476800123"
    managed = ManagedProcess.spawn(["echo", "test"], ref=ref)

    # Verify ref matches ref
    assert managed.ref == ref
    assert managed.name == "echo"

    # Wait and cleanup
    managed.wait()
    managed.cleanup()


def test_logs_tract_exec_event(journal_path, mock_callosum):
    """Test that exec event is emitted when process starts."""
    from solstone.think.callosum import CallosumConnection

    received = []
    listener = CallosumConnection()
    listener.start(callback=lambda msg: received.append(msg))

    # Spawn process
    managed = ManagedProcess.spawn(["echo", "hello"])

    # Find exec event
    exec_events = [msg for msg in received if msg.get("event") == "exec"]
    assert len(exec_events) >= 1

    exec_event = exec_events[0]
    assert exec_event["tract"] == "logs"
    assert exec_event["event"] == "exec"
    assert exec_event["ref"] == managed.ref
    assert exec_event["name"] == "echo"
    assert exec_event["pid"] == managed.pid
    assert exec_event["cmd"] == ["echo", "hello"]
    assert "log_path" in exec_event

    # Wait and cleanup
    managed.wait()
    managed.cleanup()
    listener.stop()


def test_logs_tract_line_event(journal_path, mock_callosum):
    """Test that line events are emitted for stdout/stderr."""
    from solstone.think import callosum

    received = []
    listener = callosum.CallosumConnection()
    listener.start(callback=lambda msg: received.append(msg))

    # Spawn process that outputs text
    managed = ManagedProcess.spawn(["echo", "hello logs tract"])

    # Wait for process and cleanup threads before checking events
    managed.wait()
    managed.cleanup()

    # Find line events
    line_events = [msg for msg in received if msg.get("event") == "line"]
    assert len(line_events) >= 1

    # Verify line event structure
    line_event = line_events[0]
    assert line_event["tract"] == "logs"
    assert line_event["event"] == "line"
    assert line_event["ref"] == managed.ref
    assert line_event["name"] == "echo"
    assert line_event["pid"] == managed.pid
    assert line_event["stream"] in ["stdout", "stderr"]
    assert "line" in line_event
    assert "hello logs tract" in line_event["line"]

    # Stop listener
    listener.stop()


def test_logs_tract_exit_event(journal_path, mock_callosum):
    """Test that exit event is emitted when process completes."""
    from solstone.think.callosum import CallosumConnection

    received = []
    listener = CallosumConnection()
    listener.start(callback=lambda msg: received.append(msg))

    # Spawn and wait for process
    managed = ManagedProcess.spawn(["echo", "test"])
    managed.wait()
    managed.cleanup()

    # Find exit event
    exit_events = [msg for msg in received if msg.get("event") == "exit"]
    assert len(exit_events) >= 1

    exit_event = exit_events[0]
    assert exit_event["tract"] == "logs"
    assert exit_event["event"] == "exit"
    assert exit_event["ref"] == managed.ref
    assert exit_event["name"] == "echo"
    assert exit_event["pid"] == managed.pid
    assert exit_event["exit_code"] == 0
    assert "duration_ms" in exit_event
    assert exit_event["duration_ms"] >= 0
    assert exit_event["cmd"] == ["echo", "test"]
    assert "log_path" in exit_event

    listener.stop()


def test_logs_tract_all_events_have_common_fields(journal_path, mock_callosum):
    """Test that all logs tract events have process, name, and pid."""
    from solstone.think.callosum import CallosumConnection

    received = []
    listener = CallosumConnection()
    listener.start(callback=lambda msg: received.append(msg))

    # Run a process
    managed = ManagedProcess.spawn(["echo", "test"])
    managed.wait()
    managed.cleanup()

    # Filter to only logs tract events
    logs_events = [msg for msg in received if msg.get("tract") == "logs"]
    assert len(logs_events) >= 3  # exec, line, exit

    # Verify common fields in all events
    for event in logs_events:
        assert "ref" in event
        assert "name" in event
        assert "pid" in event
        assert "ts" in event  # Auto-added by Callosum
        assert event["ref"] == managed.ref
        assert event["name"] == "echo"
        assert event["pid"] == managed.pid

    listener.stop()


def test_run_task_emits_logs_tract_events(journal_path, mock_callosum):
    """Test that run_task function emits logs tract events."""
    from solstone.think.callosum import CallosumConnection

    received = []
    listener = CallosumConnection()
    listener.start(callback=lambda msg: received.append(msg))

    # Run task
    success, exit_code, log_path = run_task(["echo", "run_task test"])

    # Verify success
    assert success is True
    assert exit_code == 0
    assert log_path.exists()

    # Verify events were emitted
    logs_events = [msg for msg in received if msg.get("tract") == "logs"]
    event_types = [msg["event"] for msg in logs_events]

    assert "exec" in event_types
    assert "line" in event_types
    assert "exit" in event_types

    listener.stop()


def test_ref_links_to_task_tract(journal_path, mock_callosum):
    """Test that providing ref links logs to task tract."""
    from solstone.think.callosum import CallosumConnection

    received = []
    listener = CallosumConnection()
    listener.start(callback=lambda msg: received.append(msg))

    ref = "1730476800999"
    managed = ManagedProcess.spawn(["echo", "linked"], ref=ref)
    managed.wait()
    managed.cleanup()

    # Verify all logs events use ref as process
    logs_events = [msg for msg in received if msg.get("tract") == "logs"]
    assert len(logs_events) >= 3

    for event in logs_events:
        assert event["ref"] == ref

    listener.stop()


def test_error_exit_code_in_exit_event(journal_path, mock_callosum):
    """Test that non-zero exit codes are captured in exit event."""
    from solstone.think.callosum import CallosumConnection

    received = []
    listener = CallosumConnection()
    listener.start(callback=lambda msg: received.append(msg))

    # Run process that exits with error
    managed = ManagedProcess.spawn(["sh", "-c", "exit 42"])
    exit_code = managed.wait()
    managed.cleanup()

    # Verify exit code
    assert exit_code == 42

    # Find exit event
    exit_events = [msg for msg in received if msg.get("event") == "exit"]
    assert len(exit_events) >= 1

    exit_event = exit_events[0]
    assert exit_event["exit_code"] == 42

    listener.stop()


def test_spawned_process_sees_eof_on_stdin(journal_path, mock_callosum):
    r_fd, w_fd = os.pipe()
    saved_stdin = os.dup(0)

    try:
        os.write(w_fd, b"smuggled\n")
        os.close(w_fd)
        w_fd = -1
        os.dup2(r_fd, 0)
        os.close(r_fd)
        r_fd = -1

        managed = ManagedProcess.spawn(["sh", "-c", "read x; echo got=$x"])
        exit_code = managed.wait()
        managed.cleanup()

        content = managed.log_writer.path.read_text()
        assert exit_code == 0
        assert "got=" in content
        assert "got=smuggled" not in content
    finally:
        os.dup2(saved_stdin, 0)
        os.close(saved_stdin)
        if r_fd != -1:
            os.close(r_fd)
        if w_fd != -1:
            os.close(w_fd)


def test_process_creates_health_log(journal_path, mock_callosum):
    """Test that process output is logged to health directory."""
    managed = ManagedProcess.spawn(["echo", "logged output"])
    ref = managed.ref
    managed.wait()
    managed.cleanup()

    # Verify log file was created with {ref}_{name}.log format
    from datetime import datetime

    day = datetime.now().strftime("%Y%m%d")
    log_path = journal_path / "chronicle" / day / "health" / f"{ref}_echo.log"

    assert log_path.exists()
    content = log_path.read_text()
    assert "logged output" in content

    # Verify day-level symlink exists
    day_symlink = journal_path / "chronicle" / day / "health" / "echo.log"
    assert day_symlink.is_symlink()
    assert day_symlink.resolve() == log_path.resolve()

    # Verify journal-level symlink exists
    journal_symlink = journal_path / "health" / "echo.log"
    assert journal_symlink.is_symlink()
    assert journal_symlink.resolve() == log_path.resolve()


def test_process_day_override(journal_path, mock_callosum):
    """Test that day parameter overrides log directory placement."""
    target_day = "20240101"
    managed = ManagedProcess.spawn(["echo", "day test"], day=target_day)
    ref = managed.ref
    managed.wait()
    managed.cleanup()

    # Log should be in target day, not today
    log_path = journal_path / "chronicle" / target_day / "health" / f"{ref}_echo.log"
    assert log_path.exists()
    content = log_path.read_text()
    assert "day test" in content

    # Today's health directory should NOT have this log
    from datetime import datetime

    today = datetime.now().strftime("%Y%m%d")
    if today != target_day:
        today_log = journal_path / today / "health" / f"{ref}_echo.log"
        assert not today_log.exists()

    # Day-level symlink in target day
    day_symlink = journal_path / "chronicle" / target_day / "health" / "echo.log"
    assert day_symlink.is_symlink()
    assert day_symlink.resolve() == log_path.resolve()

    # Journal-level symlink points to target day
    journal_symlink = journal_path / "health" / "echo.log"
    assert journal_symlink.is_symlink()
    assert journal_symlink.resolve() == log_path.resolve()


def test_run_task_day_override(journal_path, mock_callosum):
    """Test that run_task passes day through to log placement."""
    target_day = "20240201"
    success, exit_code, log_path = run_task(["echo", "task day test"], day=target_day)

    assert success
    assert exit_code == 0
    assert target_day in str(log_path)
    assert log_path.exists()


@pytest.mark.parametrize(
    ("cmd", "expected_name"),
    [
        (["journal", "think", "--day", "20240115"], "daily"),
        (
            ["journal", "think", "--day", "20240115", "--segment", "120000_300"],
            "segment",
        ),
        (["journal", "think", "--weekly"], "weekly"),
        (
            [
                "journal",
                "think",
                "--activity",
                "id",
                "--facet",
                "work",
                "--day",
                "20240115",
            ],
            "activity",
        ),
        (
            [
                "journal",
                "think",
                "--day",
                "20240115",
                "--segment",
                "120000_300",
                "--flush",
            ],
            "flush",
        ),
        (["journal", "think", "--day", "20240115", "--segments"], "segment"),
        (["journal", "indexer", "--rescan"], "indexer"),
    ],
)
def test_command_name_derivation(
    journal_path, mock_callosum, monkeypatch, cmd, expected_name
):
    """Commands produce mode-aware log names."""
    captured_kwargs = {}

    class FakePopen:
        def __init__(self, *args, **kwargs):
            captured_kwargs.update(kwargs)
            self.pid = 4321
            self.stdout = StringIO("")
            self.stderr = StringIO("")
            self.returncode = 0

        def wait(self, timeout=None):
            self.returncode = 0
            return 0

        def poll(self):
            return self.returncode

        def terminate(self):
            self.returncode = -15

        def kill(self):
            self.returncode = -9

    monkeypatch.setattr("solstone.think.runner.subprocess.Popen", FakePopen)

    managed = ManagedProcess.spawn(cmd, ref="testref")

    assert set(captured_kwargs) == {
        "bufsize",
        "env",
        "process_group",
        "stderr",
        "stdin",
        "stdout",
        "text",
    }
    assert captured_kwargs["process_group"] == 0
    assert managed.name == expected_name
    assert managed.log_writer.path.name == f"testref_{expected_name}.log"

    managed.wait()
    managed.cleanup()
