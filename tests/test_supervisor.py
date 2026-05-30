# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import asyncio
import importlib
import io
import json
import logging
import os
import signal
import socket
import subprocess
import sys
import threading
import time
from unittest.mock import MagicMock

import psutil
import pytest


def test_sd_notify_no_socket_is_noop(monkeypatch):
    from solstone.think.supervisor import _sd_notify

    monkeypatch.delenv("NOTIFY_SOCKET", raising=False)
    _sd_notify("READY=1")


def test_sd_notify_sends_payload(tmp_path, monkeypatch):
    from solstone.think.supervisor import _sd_notify

    sock_path = tmp_path / "notify.sock"
    with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as listener:
        listener.bind(str(sock_path))
        listener.settimeout(1)
        monkeypatch.setenv("NOTIFY_SOCKET", str(sock_path))

        _sd_notify("READY=1")

        assert listener.recv(1024) == b"READY=1"


def test_start_sense(tmp_path, mock_callosum, monkeypatch):
    """Test that sense launches correctly."""
    mod = importlib.import_module("solstone.think.supervisor")

    started = []

    class DummyProc:
        def __init__(self):
            self.stdout = io.StringIO()
            self.stderr = io.StringIO()
            self.pid = 12345

        def terminate(self):
            pass

        def wait(self, timeout=None):
            pass

    def fake_popen(
        cmd,
        stdin=None,
        stdout=None,
        stderr=None,
        text=False,
        bufsize=-1,
        process_group=None,
        env=None,
        **_kwargs,
    ):
        proc = DummyProc()
        started.append((cmd, stdout, stderr))
        return proc

    monkeypatch.setattr(mod.subprocess, "Popen", fake_popen)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Test start_sense()
    sense_proc = mod.start_sense()
    assert sense_proc is not None
    assert any(cmd == ["journal", "sense", "-v"] for cmd, _, _ in started)

    # Check that stdout and stderr capture pipes
    for cmd, stdout, stderr in started:
        assert stdout == subprocess.PIPE
        assert stderr == subprocess.PIPE


def test_launch_process_records_service_state(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    mod._SERVICE_STATE.clear()

    process = MagicMock()
    process.pid = 12345
    managed = mod.RunnerManagedProcess(
        process=process,
        name="unit",
        log_writer=MagicMock(),
        cmd=["journal", "sense"],
        _threads=[],
        ref="ref-1",
        _start_time=100.0,
        _callosum=None,
    )

    def fake_spawn(cmd, *, ref=None, callosum=None, day=None):
        assert cmd == ["journal", "sense"]
        assert ref == "ref-1"
        assert day is None
        return managed

    monkeypatch.setattr(mod.RunnerManagedProcess, "spawn", fake_spawn)

    result = mod._launch_process(
        "unit",
        ["journal", "sense"],
        restart=True,
        shutdown_timeout=7,
        ref="ref-1",
    )

    assert result is managed
    assert isinstance(result, mod.RunnerManagedProcess)
    assert mod._SERVICE_STATE["unit"] == {
        "restart": True,
        "shutdown_timeout": 7,
    }


def test_parse_args_remote_flag():
    """Test that parse_args includes --remote flag."""
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))

    parser = mod.parse_args()
    args = parser.parse_args(["--remote", "https://server/ingest/key"])

    assert args.remote == "https://server/ingest/key"


def test_parse_args_remote_flag_optional():
    """Test that --remote is optional."""
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))

    parser = mod.parse_args()
    args = parser.parse_args([])

    assert args.remote is None


def test_parse_args_lifecycle_verb_hint(monkeypatch, capsys):
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))
    monkeypatch.setattr(sys, "argv", ["sol", "supervisor", "stop"])

    parser = mod.parse_args()
    with pytest.raises(SystemExit) as exc_info:
        parser.parse_args(["stop"])

    captured = capsys.readouterr()
    assert exc_info.value.code == 2
    assert (
        "journal supervisor is the server-launch command (takes a port). "
        "For lifecycle, use: journal service <verb>. "
        "Did you mean: journal service stop ?"
    ) in captured.err


def test_shutdown_stops_in_reverse_order(monkeypatch):
    """Shutdown stops services in reverse order."""
    mod = importlib.import_module("solstone.think.supervisor")
    operations = []

    class MockManaged:
        def __init__(self, name):
            self.name = name
            self.terminate = MagicMock(
                side_effect=lambda timeout=None: operations.append(
                    ("terminate", self.name, timeout)
                )
            )
            self.cleanup = MagicMock(
                side_effect=lambda: operations.append(("cleanup", self.name))
            )

    procs = [
        MockManaged("convey"),
        MockManaged("sense"),
        MockManaged("cortex"),
    ]
    mod._SERVICE_STATE.clear()
    for managed in procs:
        mod._SERVICE_STATE[managed.name] = {
            "restart": True,
            "shutdown_timeout": 15,
        }

    for managed in reversed(procs):
        mod._stop_process(managed)

    assert operations == [
        ("terminate", "cortex", 15),
        ("cleanup", "cortex"),
        ("terminate", "sense", 15),
        ("cleanup", "sense"),
        ("terminate", "convey", 15),
        ("cleanup", "convey"),
    ]


def test_graceful_shutdown_calls_stop_process_for_each_managed_proc(
    tmp_path, monkeypatch
):
    """The main shutdown path stops managed services in reverse startup order."""
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.delenv("SOL_SUPERVISOR_SPAWNED", raising=False)
    monkeypatch.setattr(
        sys,
        "argv",
        ["supervisor", "0", "--no-daily", "--no-schedule"],
    )
    monkeypatch.setattr(mod, "run_pending_tasks", lambda *a, **k: (0, 0))
    monkeypatch.setattr(mod, "_sweep_orphaned_sol_processes", lambda *_a, **_k: 0)
    monkeypatch.setattr(mod.time, "sleep", lambda _seconds: None)
    monkeypatch.setattr(mod, "start_callosum_in_process", lambda: None)
    monkeypatch.setattr(mod, "stop_callosum_in_process", lambda: None)
    monkeypatch.setattr(mod, "wait_for_convey_ready", lambda _proc: True)
    monkeypatch.setattr(mod, "_maybe_submit_startup_digest", lambda *, no_cortex: None)

    class FakeCallosumConnection:
        def __init__(self, *args, **kwargs):
            pass

        def start(self, *args, **kwargs):
            pass

        def emit(self, *args, **kwargs):
            pass

        def stop(self):
            pass

    monkeypatch.setattr(mod, "CallosumConnection", FakeCallosumConnection)

    procs = []
    for name in ["convey", "sense", "cortex", "spl"]:
        managed = _TaskManagedStub(cmd=["journal", name])
        managed.name = name
        procs.append(managed)

    monkeypatch.setattr(
        mod,
        "start_convey_server",
        lambda verbose, debug=False, port=0: (procs[0], 5015),
    )
    monkeypatch.setattr(mod, "start_sense", lambda: procs[1])
    monkeypatch.setattr(mod, "start_cortex_server", lambda: procs[2])
    monkeypatch.setattr(mod, "start_spl_service", lambda: procs[3])

    stop_order = []
    monkeypatch.setattr(
        mod,
        "_stop_process",
        lambda managed: stop_order.append(managed.name),
    )

    def interrupt_supervise(coro):
        coro.close()
        raise KeyboardInterrupt

    monkeypatch.setattr(mod.asyncio, "run", interrupt_supervise)

    try:
        mod.main()
    finally:
        os.environ.pop("SOL_SUPERVISOR_SPAWNED", None)

    assert stop_order == ["spl", "cortex", "sense", "convey"]


def test_get_command_name():
    """Test command name extraction for queue serialization."""
    mod = importlib.import_module("solstone.think.supervisor")
    get = mod.TaskQueue.get_command_name

    # sol X -> X
    assert get(["sol", "indexer", "--rescan"]) == "indexer"
    assert get(["sol", "insight", "20240101"]) == "insight"
    assert get(["journal", "think", "--day", "20240101"]) == "daily"

    # Other commands -> basename
    assert get(["/usr/bin/python", "script.py"]) == "python"
    assert get(["custom-tool"]) == "custom-tool"

    # Empty -> unknown
    assert get([]) == "unknown"


@pytest.mark.parametrize(
    "cmd",
    [
        ["journal", "think", "--day", "20260527"],
        ["journal", "think", "--day", "20260527", "--segment", "120000_300"],
        [
            "journal",
            "think",
            "--day",
            "20260527",
            "--segment",
            "120000_300",
            "--stream",
            "screen",
        ],
        [
            "journal",
            "think",
            "--day",
            "20260527",
            "--segment",
            "120000_300",
            "--flush",
        ],
        ["journal", "think", "--day", "20260527", "--segments"],
        [
            "journal",
            "think",
            "--activity",
            "activity-id",
            "--facet",
            "work",
            "--day",
            "20260527",
        ],
        ["journal", "think", "--weekly", "-v"],
        ["journal", "think"],
        ["sol", "indexer", "--rescan"],
        ["journal", "sense", "--day", "20260101"],
    ],
)
def test_command_partition_matches_task_queue_get_command_name(cmd):
    mod = importlib.import_module("solstone.think.supervisor")
    runner = importlib.import_module("solstone.think.runner")

    assert runner._command_partition(cmd) == mod.TaskQueue.get_command_name(cmd)


def _fresh_task_queue(mod, *, on_queue_change=None):
    mod._task_queue = mod.TaskQueue(on_queue_change=on_queue_change)
    mod._supervisor_callosum = None
    return mod._task_queue


def _capture_thread_starts(monkeypatch, mod):
    spawned = []

    def fake_thread_start(self):
        spawned.append(self._args)

    monkeypatch.setattr(mod.threading.Thread, "start", fake_thread_start)
    return spawned


class _CaptureTaskQueue:
    def __init__(self):
        self.submissions = []

    def submit(self, cmd, day=None):
        self.submissions.append({"cmd": cmd, "day": day})


def test_handle_segment_observed_live_command_marks_live(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    capture = _CaptureTaskQueue()
    monkeypatch.setattr(mod, "_task_queue", capture)

    mod._handle_segment_observed(
        {
            "tract": "observe",
            "event": "observed",
            "day": "20260527",
            "segment": "120000_300",
        }
    )

    assert len(capture.submissions) == 1
    assert capture.submissions[0]["day"] == "20260527"
    assert "--live" in capture.submissions[0]["cmd"]


def test_handle_segment_observed_batch_submits_nothing(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    capture = _CaptureTaskQueue()
    monkeypatch.setattr(mod, "_task_queue", capture)

    mod._handle_segment_observed(
        {
            "tract": "observe",
            "event": "observed",
            "day": "20260527",
            "segment": "120000_300",
            "batch": True,
        }
    )

    assert len(capture.submissions) == 0


def test_handle_segment_observed_batch_leaves_flush_state_untouched(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    capture = _CaptureTaskQueue()
    monkeypatch.setattr(mod, "_task_queue", capture)

    mod._flush_state["last_segment_ts"] = 0.0
    mod._flush_state["day"] = None
    mod._flush_state["segment"] = None
    mod._flush_state["flushed"] = True

    mod._handle_segment_observed(
        {
            "tract": "observe",
            "event": "observed",
            "day": "20260527",
            "segment": "120000_300",
        }
    )

    assert len(capture.submissions) == 1
    live_state = {
        "day": mod._flush_state["day"],
        "segment": mod._flush_state["segment"],
        "flushed": mod._flush_state["flushed"],
        "last_segment_ts": mod._flush_state["last_segment_ts"],
    }
    assert live_state["day"] == "20260527"
    assert live_state["segment"] == "120000_300"
    assert live_state["flushed"] is False
    assert live_state["last_segment_ts"] > 0

    mod._handle_segment_observed(
        {
            "tract": "observe",
            "event": "observed",
            "day": "20260101",
            "segment": "090000_300",
            "batch": True,
        }
    )

    assert len(capture.submissions) == 1
    assert mod._flush_state["day"] == live_state["day"]
    assert mod._flush_state["segment"] == live_state["segment"]
    assert mod._flush_state["flushed"] == live_state["flushed"]
    assert mod._flush_state["last_segment_ts"] == live_state["last_segment_ts"]


def test_handle_segment_observed_live_stream_command(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    capture = _CaptureTaskQueue()
    monkeypatch.setattr(mod, "_task_queue", capture)

    mod._handle_segment_observed(
        {
            "tract": "observe",
            "event": "observed",
            "day": "20260527",
            "segment": "120000_300",
            "stream": "archon",
        }
    )

    assert len(capture.submissions) == 1
    cmd = capture.submissions[0]["cmd"]
    assert "--live" in cmd
    stream_index = cmd.index("--stream")
    assert cmd[stream_index + 1] == "archon"


def test_task_queue_daily_and_segment_run_independently(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = _fresh_task_queue(mod)
    _capture_thread_starts(monkeypatch, mod)

    queue.submit(["journal", "think", "--day", "20260527"], ref="daily-ref")
    queue.submit(
        ["journal", "think", "--day", "20260527", "--segment", "120000_300"],
        ref="segment-ref",
    )

    assert set(queue._running) == {"daily", "segment"}
    assert queue._queues == {}


def test_task_queue_segment_and_flush_run_independently(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = _fresh_task_queue(mod)
    _capture_thread_starts(monkeypatch, mod)

    queue.submit(
        ["journal", "think", "--day", "20260527", "--segment", "120000_300"],
        ref="segment-ref",
    )
    queue.submit(
        [
            "journal",
            "think",
            "--day",
            "20260527",
            "--segment",
            "120000_300",
            "--flush",
        ],
        ref="flush-ref",
    )

    assert set(queue._running) == {"segment", "flush"}
    assert queue._queues == {}


def test_task_queue_daily_and_activity_run_independently(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = _fresh_task_queue(mod)
    _capture_thread_starts(monkeypatch, mod)

    queue.submit(["journal", "think", "--day", "20260527"], ref="daily-ref")
    queue.submit(
        [
            "journal",
            "think",
            "--activity",
            "activity-id",
            "--facet",
            "work",
            "--day",
            "20260527",
        ],
        ref="activity-ref",
    )

    assert set(queue._running) == {"daily", "activity"}
    assert queue._queues == {}


def test_task_queue_daily_and_weekly_run_independently(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = _fresh_task_queue(mod)
    _capture_thread_starts(monkeypatch, mod)

    queue.submit(["journal", "think", "--day", "20260527"], ref="daily-ref")
    queue.submit(["journal", "think", "--weekly", "-v"], ref="weekly-ref")

    assert set(queue._running) == {"daily", "weekly"}
    assert queue._queues == {}


def test_task_queue_segments_plural_shares_segment_partition(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = _fresh_task_queue(mod)
    _capture_thread_starts(monkeypatch, mod)

    queue.submit(
        ["journal", "think", "--day", "20260527", "--segment", "120000_300"],
        ref="segment-ref",
    )
    queue.submit(
        ["journal", "think", "--day", "20260527", "--segments"],
        ref="segments-ref",
    )

    assert set(queue._running) == {"segment"}
    assert queue._queues["segment"][0]["refs"] == ["segments-ref"]
    assert queue._queues["segment"][0]["cmd"] == [
        "journal",
        "think",
        "--day",
        "20260527",
        "--segments",
    ]


def test_task_queue_flush_flag_precedes_segment_flag():
    runner = importlib.import_module("solstone.think.runner")

    assert (
        runner._command_partition(
            ["journal", "think", "--day", "20260527", "--segment", "00", "--flush"]
        )
        == "flush"
    )


def test_task_queue_within_mode_serialization_segment(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = _fresh_task_queue(mod)
    spawned = _capture_thread_starts(monkeypatch, mod)

    queue.submit(
        ["journal", "think", "--day", "20260527", "--segment", "120000_300"],
        ref="first-ref",
    )
    queue.submit(
        ["journal", "think", "--day", "20260527", "--segment", "120500_300"],
        ref="second-ref",
    )

    assert set(queue._running) == {"segment"}
    assert queue._running["segment"]["ref"] == "first-ref"
    assert set(queue._queues) == {"segment"}
    assert queue._queues["segment"][0]["refs"] == ["second-ref"]

    queue._process_next("segment")

    assert queue._running["segment"]["ref"] == "second-ref"
    assert queue._queues["segment"] == []
    assert spawned[-1][0] == ["second-ref"]


def test_task_queue_dedup_within_segment_partition(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = _fresh_task_queue(mod)
    _capture_thread_starts(monkeypatch, mod)
    cmd = ["journal", "think", "--day", "20260527", "--segment", "120000_300"]

    queue.submit(cmd, ref="first-ref")
    queue.submit(cmd, ref="second-ref")
    queue.submit(cmd, ref="third-ref")

    assert queue._running["segment"]["ref"] == "first-ref"
    assert len(queue._queues["segment"]) == 1
    assert queue._queues["segment"][0]["cmd"] == cmd
    assert queue._queues["segment"][0]["refs"] == ["second-ref", "third-ref"]


def test_task_queue_stale_thread_reclamation_per_partition(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    dead_thread = threading.Thread(target=lambda: None)
    dead_thread.start()
    dead_thread.join()

    queue = _fresh_task_queue(mod)
    _capture_thread_starts(monkeypatch, mod)

    queue.submit(["journal", "think", "--day", "20260527"], ref="old-daily-ref")
    queue.submit(
        ["journal", "think", "--day", "20260527", "--segment", "120000_300"],
        ref="segment-ref",
    )

    queue._running["daily"]["thread"] = dead_thread

    queue.submit(["journal", "think", "--day", "20260528"], ref="new-daily-ref")

    assert queue._running["daily"]["ref"] == "new-daily-ref"
    assert queue._running["segment"]["ref"] == "segment-ref"
    assert set(queue._running) == {"daily", "segment"}


def test_handle_task_request_routes_to_mode_partition(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = _fresh_task_queue(mod)
    _capture_thread_starts(monkeypatch, mod)

    queue.submit(["journal", "think", "--day", "20260527"], ref="daily-ref")
    mod._handle_task_request(
        {
            "tract": "supervisor",
            "event": "request",
            "cmd": ["journal", "think", "--day", "20260527", "--segment", "120000_300"],
            "ref": "segment-ref",
        }
    )

    assert queue._running["daily"]["ref"] == "daily-ref"
    assert queue._running["segment"]["ref"] == "segment-ref"
    assert queue._queues == {}


def test_scheduler_weekly_cap_registers_under_weekly(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    monkeypatch.setattr(
        mod.scheduler,
        "collect_runtime_caps",
        lambda: [(["journal", "think", "--weekly", "-v"], 60.0)],
    )

    for cmd, seconds in mod.scheduler.collect_runtime_caps():
        queue.set_cap(mod.TaskQueue.get_command_name(cmd), seconds)

    assert queue._caps == {"weekly": 60.0}


def test_queue_event_carries_mode_partition_name(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    events = []
    queue = _fresh_task_queue(
        mod,
        on_queue_change=lambda command, running, queued: events.append(
            (command, running, queued)
        ),
    )
    _capture_thread_starts(monkeypatch, mod)

    queue.submit(
        ["journal", "think", "--day", "20260527", "--segment", "120000_300"],
        ref="first-ref",
    )
    queue.submit(
        ["journal", "think", "--day", "20260527", "--segment", "120500_300"],
        ref="second-ref",
    )

    assert events[-1][0] == "segment"


def test_no_literal_think_queue_keys_in_source():
    mod = importlib.import_module("solstone.think.supervisor")

    assert mod.TaskQueue.get_command_name(
        ["journal", "think", "--day", "20260527"]
    ) != ("think")


def test_task_queue_same_command_queued(monkeypatch):
    """Test that same command is queued when already running."""
    mod = importlib.import_module("solstone.think.supervisor")

    # Create fresh task queue (no callback to avoid callosum events)
    mod._task_queue = mod.TaskQueue(on_queue_change=None)

    spawned = []

    def fake_thread_start(self):
        spawned.append(self._target.__name__)

    monkeypatch.setattr(mod.threading.Thread, "start", fake_thread_start)

    # First request - should run immediately
    msg1 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["sol", "indexer", "--rescan"],
    }
    mod._handle_task_request(msg1)

    assert "indexer" in mod._task_queue._running
    assert len(spawned) == 1

    # Second request (different args) - should be queued
    msg2 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["sol", "indexer", "--rescan-full"],
    }
    mod._handle_task_request(msg2)

    assert len(spawned) == 1  # No new spawn
    assert "indexer" in mod._task_queue._queues
    assert len(mod._task_queue._queues["indexer"]) == 1
    # Queue entries are {refs, cmd} dicts (refs is a list for coalescing)
    assert mod._task_queue._queues["indexer"][0]["cmd"] == [
        "sol",
        "indexer",
        "--rescan-full",
    ]
    assert len(mod._task_queue._queues["indexer"][0]["refs"]) == 1


def test_task_queue_dedupe_exact_match(monkeypatch):
    """Test that exact same command is deduped in queue."""
    mod = importlib.import_module("solstone.think.supervisor")

    # Create fresh task queue (no callback to avoid callosum events)
    mod._task_queue = mod.TaskQueue(on_queue_change=None)

    spawned = []

    def fake_thread_start(self):
        spawned.append(self._target.__name__)

    monkeypatch.setattr(mod.threading.Thread, "start", fake_thread_start)

    # First request - runs
    msg1 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["sol", "indexer", "--rescan"],
    }
    mod._handle_task_request(msg1)

    # Second request (same cmd) - queued
    msg2 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["sol", "indexer", "--rescan"],
    }
    mod._handle_task_request(msg2)

    assert len(mod._task_queue._queues["indexer"]) == 1

    # Third request (same cmd again) - deduped, not added
    msg3 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["sol", "indexer", "--rescan"],
    }
    mod._handle_task_request(msg3)

    assert len(mod._task_queue._queues["indexer"]) == 1  # Still just 1


def test_task_queue_different_commands_independent(monkeypatch):
    """Test that different commands have independent queues."""
    mod = importlib.import_module("solstone.think.supervisor")

    # Create fresh task queue (no callback to avoid callosum events)
    mod._task_queue = mod.TaskQueue(on_queue_change=None)

    spawned = []

    def fake_thread_start(self):
        spawned.append(self._target.__name__)

    monkeypatch.setattr(mod.threading.Thread, "start", fake_thread_start)

    # Indexer request - runs
    msg1 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["sol", "indexer", "--rescan"],
    }
    mod._handle_task_request(msg1)

    # Insight request - also runs (different command)
    msg2 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["sol", "insight", "20240101"],
    }
    mod._handle_task_request(msg2)

    assert len(spawned) == 2  # Both spawned
    assert "indexer" in mod._task_queue._running
    assert "insight" in mod._task_queue._running


def test_process_queue_spawns_next(monkeypatch):
    """Test that _process_next spawns next queued task."""
    mod = importlib.import_module("solstone.think.supervisor")

    # Create task queue with pre-set state
    mod._task_queue = mod.TaskQueue(on_queue_change=None)
    mod._task_queue._running = {"indexer": {"ref": "ref123", "thread": None}}
    mod._task_queue._queues = {
        "indexer": [
            {"refs": ["queued-ref"], "cmd": ["sol", "indexer", "--rescan-full"]}
        ]
    }

    spawned = []

    def fake_thread_start(self):
        spawned.append(self._args)  # Capture args (refs, cmd, cmd_name, callosum)

    monkeypatch.setattr(mod.threading.Thread, "start", fake_thread_start)

    # Process queue
    mod._task_queue._process_next("indexer")

    # Should have spawned the queued task with its refs list
    assert len(spawned) == 1
    assert spawned[0][0] == ["queued-ref"]  # refs list preserved from queue
    assert spawned[0][1] == ["sol", "indexer", "--rescan-full"]  # cmd
    assert spawned[0][2] == "indexer"  # cmd_name

    # Queue should be empty now
    assert mod._task_queue._queues["indexer"] == []


def test_process_queue_clears_running_when_empty(monkeypatch):
    """Test that _process_next clears running state when queue is empty."""
    mod = importlib.import_module("solstone.think.supervisor")

    # Create task queue with pre-set state (no queued tasks)
    mod._task_queue = mod.TaskQueue(on_queue_change=None)
    mod._task_queue._running = {"indexer": {"ref": "ref123", "thread": None}}
    mod._task_queue._queues = {"indexer": []}

    spawned = []

    def fake_thread_start(self):
        spawned.append(True)

    monkeypatch.setattr(mod.threading.Thread, "start", fake_thread_start)

    # Process queue
    mod._task_queue._process_next("indexer")

    # No spawn (queue was empty)
    assert len(spawned) == 0

    # Running state should be cleared
    assert "indexer" not in mod._task_queue._running


def test_task_request_uses_caller_provided_ref(monkeypatch):
    """Test that caller-provided ref is used and preserved through queue."""
    mod = importlib.import_module("solstone.think.supervisor")

    # Create fresh task queue (no callback to avoid callosum events)
    mod._task_queue = mod.TaskQueue(on_queue_change=None)

    spawned = []

    def fake_thread_start(self):
        spawned.append(self._args)  # Capture args (refs, cmd, cmd_name, callosum)

    monkeypatch.setattr(mod.threading.Thread, "start", fake_thread_start)

    # Request with caller-provided ref
    msg = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["sol", "indexer", "--rescan"],
        "ref": "my-custom-ref-123",
    }
    mod._handle_task_request(msg)

    # Should use the provided ref
    assert mod._task_queue._running["indexer"]["ref"] == "my-custom-ref-123"
    assert spawned[0][0] == ["my-custom-ref-123"]  # refs is a list


def test_task_queue_preserves_caller_ref(monkeypatch):
    """Test that queued requests preserve their caller-provided ref."""
    mod = importlib.import_module("solstone.think.supervisor")

    # Create fresh task queue (no callback to avoid callosum events)
    mod._task_queue = mod.TaskQueue(on_queue_change=None)

    spawned = []

    def fake_thread_start(self):
        spawned.append(self._args)

    monkeypatch.setattr(mod.threading.Thread, "start", fake_thread_start)

    # First request runs immediately
    msg1 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["sol", "indexer", "--rescan"],
        "ref": "first-ref",
    }
    mod._handle_task_request(msg1)

    # Second request gets queued with its own ref
    msg2 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["sol", "indexer", "--rescan-full"],
        "ref": "second-ref",
    }
    mod._handle_task_request(msg2)

    # Verify queued entry has the caller's ref in refs list
    assert len(mod._task_queue._queues["indexer"]) == 1
    assert mod._task_queue._queues["indexer"][0]["refs"] == ["second-ref"]
    assert mod._task_queue._queues["indexer"][0]["cmd"] == [
        "sol",
        "indexer",
        "--rescan-full",
    ]


def test_task_queue_coalesces_refs_on_dedupe(monkeypatch):
    """Test that duplicate queued requests coalesce their refs."""
    mod = importlib.import_module("solstone.think.supervisor")

    # Create fresh task queue (no callback to avoid callosum events)
    mod._task_queue = mod.TaskQueue(on_queue_change=None)

    spawned = []

    def fake_thread_start(self):
        spawned.append(self._args)

    monkeypatch.setattr(mod.threading.Thread, "start", fake_thread_start)

    # First request runs immediately
    msg1 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["sol", "indexer", "--rescan"],
        "ref": "first-ref",
    }
    mod._handle_task_request(msg1)

    # Second request (same cmd) gets queued
    msg2 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["sol", "indexer", "--rescan"],
        "ref": "second-ref",
    }
    mod._handle_task_request(msg2)

    # Third request (same cmd) should coalesce its ref into existing queue entry
    msg3 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["sol", "indexer", "--rescan"],
        "ref": "third-ref",
    }
    mod._handle_task_request(msg3)

    # Should still be just one queue entry
    assert len(mod._task_queue._queues["indexer"]) == 1
    # But it should have both refs
    assert mod._task_queue._queues["indexer"][0]["refs"] == [
        "second-ref",
        "third-ref",
    ]


def test_process_queue_spawns_with_multiple_refs(monkeypatch):
    """Test that dequeued task has all coalesced refs."""
    mod = importlib.import_module("solstone.think.supervisor")

    # Create task queue with pre-set state (queued task with multiple refs)
    mod._task_queue = mod.TaskQueue(on_queue_change=None)
    mod._task_queue._running = {"indexer": {"ref": "running-ref", "thread": None}}
    mod._task_queue._queues = {
        "indexer": [
            {
                "refs": ["ref-A", "ref-B", "ref-C"],
                "cmd": ["sol", "indexer", "--rescan"],
            }
        ]
    }

    spawned = []

    def fake_thread_start(self):
        spawned.append(self._args)

    monkeypatch.setattr(mod.threading.Thread, "start", fake_thread_start)

    # Process queue
    mod._task_queue._process_next("indexer")

    # Should spawn with all three refs
    assert len(spawned) == 1
    assert spawned[0][0] == ["ref-A", "ref-B", "ref-C"]  # all refs passed
    assert spawned[0][1] == ["sol", "indexer", "--rescan"]


def test_stale_queue_detected_on_submit(monkeypatch):
    """Test that a dead task thread is detected and cleared on next submit."""
    import threading

    mod = importlib.import_module("solstone.think.supervisor")

    mod._task_queue = mod.TaskQueue(on_queue_change=None)

    # Create a dead thread BEFORE monkeypatching Thread.start
    dead_thread = threading.Thread(target=lambda: None)
    dead_thread.start()
    dead_thread.join()
    assert not dead_thread.is_alive()

    spawned = []

    def fake_thread_start(self):
        spawned.append(self._target.__name__)

    monkeypatch.setattr(mod.threading.Thread, "start", fake_thread_start)

    mod._task_queue._running = {"indexer": {"ref": "stale-ref", "thread": dead_thread}}
    mod._task_queue._queues = {
        "indexer": [
            {"refs": ["queued-ref"], "cmd": ["sol", "indexer", "--rescan-full"]}
        ]
    }

    # Submit a new indexer task — should detect stale state and start immediately
    msg = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["sol", "indexer", "--rescan-new"],
        "ref": "new-ref",
    }
    mod._handle_task_request(msg)

    # Stale entry should have been cleared, new task started
    assert mod._task_queue._running["indexer"]["ref"] == "new-ref"
    assert len(spawned) == 1

    # Old queued entries should still be in queue (stale clear only removes _running)
    assert len(mod._task_queue._queues["indexer"]) == 1


class _TaskProcessStub:
    def __init__(self):
        self.poll = MagicMock(return_value=None)
        self.pid = 12345


class _TaskManagedStub:
    def __init__(self, *, cmd, start_time=100.0):
        self.name = "task"
        self.cmd = cmd
        self.start_time = start_time
        self.process = _TaskProcessStub()
        self.ref = "ref-1"
        self.terminate = MagicMock()
        self.cleanup = MagicMock()
        self.is_running = MagicMock(return_value=True)


def test_ensure_venv_bin_on_path_prepends_when_missing(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    monkeypatch.setenv("PATH", "/usr/bin")
    monkeypatch.setattr(sys, "executable", "/fake/venv/bin/python3")

    mod._ensure_venv_bin_on_path()

    parts = os.environ["PATH"].split(os.pathsep)
    assert parts[0] == "/fake/venv/bin"
    assert "/usr/bin" in parts[1:]


def test_ensure_venv_bin_on_path_idempotent(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    monkeypatch.setenv("PATH", "/usr/bin")
    monkeypatch.setattr(sys, "executable", "/fake/venv/bin/python3")

    mod._ensure_venv_bin_on_path()
    mod._ensure_venv_bin_on_path()

    parts = os.environ["PATH"].split(os.pathsep)
    assert parts.count("/fake/venv/bin") == 1


def test_taskqueue_set_cap_records_cap():
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)

    queue.set_cap("import", 1800)

    assert queue._caps["import"] == 1800


def test_task_queue_history_records_completion(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "health").mkdir(parents=True, exist_ok=True)

    queue = mod.TaskQueue(on_queue_change=None)
    callosum = MagicMock()

    class FakeCallosum:
        def start(self, callback=None):
            return None

        def emit(self, *args, **kwargs):
            return callosum.emit(*args, **kwargs)

        def stop(self):
            return None

    managed = MagicMock()
    managed.pid = 12345
    managed.wait.return_value = 0
    managed.cleanup = MagicMock()

    def fake_spawn(cmd, *, ref=None, callosum=None, day=None):
        managed.cmd = cmd
        managed.ref = ref
        return managed

    monkeypatch.setattr(mod, "CallosumConnection", FakeCallosum)
    monkeypatch.setattr(mod.RunnerManagedProcess, "spawn", fake_spawn)

    queue._run_task(
        ["ref-1"],
        ["journal", "heartbeat"],
        "heartbeat",
        None,
        "heartbeat",
    )

    assert list(queue._history) == [
        {
            "name": "heartbeat",
            "cmd": ["journal", "heartbeat"],
            "ref": "ref-1",
            "ended_at": queue._history[0]["ended_at"],
            "exit_status": "ok",
            "scheduler_name": "heartbeat",
        }
    ]


def test_scheduler_completion_updates_scheduler_json(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    health_dir = tmp_path / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    state_path = health_dir / "scheduler.json"
    state_path.write_text(
        '{"heartbeat": {"custom": "kept"}, "other": {"last_run": 1}}',
        encoding="utf-8",
    )

    mod._record_scheduler_completion(
        "heartbeat",
        ended_at=123.0,
        exit_status="ok",
        ref="ref-1",
        cmd=["journal", "heartbeat"],
    )

    data = json.loads(state_path.read_text(encoding="utf-8"))
    assert data["heartbeat"] == {
        "custom": "kept",
        "last_run": 123.0,
        "last_status": "ok",
        "last_ref": "ref-1",
    }
    assert data["other"] == {"last_run": 1}


def test_run_task_completes_when_scheduler_writeback_fails(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    callosum = MagicMock()

    class FakeCallosum:
        def start(self, callback=None):
            return None

        def emit(self, *args, **kwargs):
            return callosum.emit(*args, **kwargs)

        def stop(self):
            return callosum.stop()

    managed = MagicMock()
    managed.pid = 12345
    managed.wait.return_value = 0
    managed.cleanup = MagicMock()

    def fake_spawn(cmd, *, ref=None, callosum=None, day=None):
        managed.cmd = cmd
        managed.ref = ref
        return managed

    monkeypatch.setattr(mod, "CallosumConnection", FakeCallosum)
    monkeypatch.setattr(mod.RunnerManagedProcess, "spawn", fake_spawn)
    monkeypatch.setattr(
        mod,
        "_record_scheduler_completion",
        MagicMock(side_effect=OSError("disk full")),
    )
    process_next = MagicMock()
    monkeypatch.setattr(queue, "_process_next", process_next)

    queue._run_task(
        ["ref-1"],
        ["journal", "heartbeat"],
        "heartbeat",
        None,
        "heartbeat",
    )

    callosum.stop.assert_called_once()
    process_next.assert_called_once_with("heartbeat")


def test_record_scheduler_completion_serializes_concurrent_writes(
    tmp_path, monkeypatch
):
    mod = importlib.import_module("solstone.think.supervisor")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    threads = [
        threading.Thread(
            target=mod._record_scheduler_completion,
            args=(name,),
            kwargs={
                "ended_at": ended_at,
                "exit_status": "ok",
                "ref": f"ref-{name}",
                "cmd": ["sol", name],
            },
        )
        for name, ended_at in [("first", 101.0), ("second", 202.0)]
    ]

    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()

    state_path = tmp_path / "health" / "scheduler.json"
    data = json.loads(state_path.read_text(encoding="utf-8"))
    assert data["first"]["last_run"] == 101.0
    assert data["second"]["last_run"] == 202.0


def test_task_history_records_cap_kill_as_timeout(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    queue.set_cap("import", 50)
    callosum = MagicMock()

    class FakeCallosum:
        def start(self, callback=None):
            return None

        def emit(self, *args, **kwargs):
            return callosum.emit(*args, **kwargs)

        def stop(self):
            return None

    managed = MagicMock()
    managed.pid = 12345
    managed.cmd = ["sol", "import"]
    managed.ref = "ref-1"
    managed.start_time = 100.0
    managed.cleanup = MagicMock()

    def wait():
        queue.enforce_deadlines(200.0)
        return -15

    managed.wait.side_effect = wait

    def fake_spawn(cmd, *, ref=None, callosum=None, day=None):
        return managed

    monkeypatch.setattr(mod, "CallosumConnection", FakeCallosum)
    monkeypatch.setattr(mod.RunnerManagedProcess, "spawn", fake_spawn)
    monkeypatch.setattr(mod, "_start_termination_thread", MagicMock())

    queue._run_task(["ref-1"], ["sol", "import"], "import")

    assert queue._history[0]["exit_status"] == "timeout"


def test_handle_task_request_skips_still_running(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    managed = _TaskManagedStub(cmd=["sol", "import"], start_time=100.0)
    queue._active["active-ref"] = managed
    queue.set_cap("import", 50)
    callosum = MagicMock()

    monkeypatch.setattr(mod, "_task_queue", queue)
    monkeypatch.setattr(mod, "_supervisor_callosum", callosum)
    monkeypatch.setattr(mod.time, "time", lambda: 150.0)

    mod._handle_task_request(
        {
            "tract": "supervisor",
            "event": "request",
            "cmd": ["sol", "import", "--sync", "plaud"],
            "ref": "requested-ref",
            "scheduler_name": "sync-plaud",
        }
    )

    callosum.emit.assert_called_once_with(
        "supervisor",
        "skipped",
        reason="still_running",
        ref="requested-ref",
        active_ref="active-ref",
        cmd=["sol", "import", "--sync", "plaud"],
        scheduler_name="sync-plaud",
    )
    assert queue._queues == {}


def test_handle_task_request_skips_wedged(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    managed = _TaskManagedStub(cmd=["sol", "import"], start_time=100.0)
    queue._active["active-ref"] = managed
    queue.set_cap("import", 50)
    callosum = MagicMock()

    monkeypatch.setattr(mod, "_task_queue", queue)
    monkeypatch.setattr(mod, "_supervisor_callosum", callosum)
    monkeypatch.setattr(mod.time, "time", lambda: 201.0)

    mod._handle_task_request(
        {
            "tract": "supervisor",
            "event": "request",
            "cmd": ["sol", "import", "--sync", "plaud"],
            "ref": "requested-ref",
        }
    )

    assert callosum.emit.call_args.kwargs["reason"] == "wedged"
    assert callosum.emit.call_args.kwargs["active_ref"] == "active-ref"


def test_task_queue_shutdown_terminates_active_tasks():
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    first = _TaskManagedStub(cmd=["sol", "import"])
    second = _TaskManagedStub(cmd=["sol", "indexer"])
    queue._active = {"first": first, "second": second}

    assert queue.shutdown() == 2

    first.terminate.assert_called_once_with(timeout=10.0)
    second.terminate.assert_called_once_with(timeout=10.0)


def test_task_queue_shutdown_empty_is_noop():
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)

    assert queue.shutdown() == 0


def test_task_queue_shutdown_continues_after_timeout():
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    first = _TaskManagedStub(cmd=["sol", "import"])
    second = _TaskManagedStub(cmd=["sol", "indexer"])
    first.terminate.side_effect = subprocess.TimeoutExpired(
        cmd=["sol", "import"], timeout=10
    )
    queue._active = {"first": first, "second": second}

    assert queue.shutdown() == 2

    first.terminate.assert_called_once_with(timeout=10.0)
    second.terminate.assert_called_once_with(timeout=10.0)


def test_enforce_deadlines_terminates_when_elapsed_exceeds_cap(caplog, monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    managed = _TaskManagedStub(
        cmd=["sol", "import", "--sync", "plaud", "--save"],
        start_time=100.0,
    )
    queue._active["ref-1"] = managed
    queue.set_cap("import", 50)

    def terminate_now(key, managed_arg, timeout, reason):
        assert key == "ref-1"
        assert managed_arg is managed
        assert timeout == 2.0
        assert reason == "cap"
        managed_arg.terminate(timeout=timeout)

    monkeypatch.setattr(mod, "_start_termination_thread", terminate_now)
    caplog.set_level("WARNING")
    queue.enforce_deadlines(200.0)

    managed.terminate.assert_called_once_with(timeout=2.0)
    assert (
        "Task import (cmd=sol import --sync plaud --save, ref=ref-1) exceeded "
        "max_runtime of 50s (elapsed=100s); terminating"
    ) in caplog.text


def test_collect_task_status_no_cap(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    managed = _TaskManagedStub(cmd=["sol", "providers"], start_time=100.0)
    queue._active["ref-1"] = managed
    monkeypatch.setattr(mod.time, "time", lambda: 112.0)

    assert queue.collect_task_status() == [
        {
            "ref": "ref-1",
            "name": "providers",
            "duration_seconds": 12,
            "max_runtime_seconds": None,
            "stuck": False,
        }
    ]


def test_collect_task_status_under_cap(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    managed = _TaskManagedStub(cmd=["sol", "providers"], start_time=100.0)
    queue._active["ref-1"] = managed
    queue.set_cap("providers", 300)
    monkeypatch.setattr(mod.time, "time", lambda: 112.0)

    status = queue.collect_task_status()

    assert status[0]["max_runtime_seconds"] == 300
    assert status[0]["stuck"] is False


def test_collect_task_status_over_cap(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    managed = _TaskManagedStub(cmd=["sol", "providers"], start_time=100.0)
    queue._active["ref-1"] = managed
    queue.set_cap("providers", 5)
    monkeypatch.setattr(mod.time, "time", lambda: 112.0)

    status = queue.collect_task_status()

    assert status[0]["max_runtime_seconds"] == 5
    assert status[0]["stuck"] is True


def test_enforce_deadlines_terminates_stopped_task(caplog, monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    proc = subprocess.Popen(["sh", "-c", "kill -STOP $$; sleep 60"])
    try:
        child = psutil.Process(proc.pid)
        for _ in range(30):
            if child.status() == psutil.STATUS_STOPPED:
                break
            time.sleep(0.1)
        else:
            pytest.fail("subprocess did not enter stopped state")

        queue = mod.TaskQueue(on_queue_change=None)
        managed = _TaskManagedStub(cmd=["sleep"], start_time=time.time())
        managed.process.pid = proc.pid
        queue._caps["sleep"] = 60
        queue._active["ref-1"] = managed
        terminate = MagicMock()
        monkeypatch.setattr(mod, "_start_termination_thread", terminate)
        caplog.set_level(logging.WARNING)

        queue.enforce_deadlines(time.time())
        terminate.assert_not_called()

        queue.enforce_deadlines(time.time())

        terminate.assert_called_once_with(
            "ref-1", managed, timeout=2.0, reason="stopped"
        )
        assert "stopped" in caplog.text
    finally:
        try:
            os.kill(proc.pid, signal.SIGCONT)
        except ProcessLookupError:
            pass
        try:
            proc.terminate()
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)


def test_terminate_managed_logs_timeout(caplog):
    mod = importlib.import_module("solstone.think.supervisor")
    managed = _TaskManagedStub(cmd=["sol", "import"], start_time=100.0)
    managed.terminate.side_effect = subprocess.TimeoutExpired(
        cmd=managed.cmd, timeout=3
    )

    caplog.set_level("WARNING")
    mod._terminate_managed(managed, 3, reason="test")

    managed.terminate.assert_called_once_with(timeout=3)
    assert "task did not terminate within 3.0s for test" in caplog.text


def test_enforce_deadlines_noop_when_no_cap():
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    managed = _TaskManagedStub(cmd=["sol", "import"], start_time=100.0)
    queue._active["ref-1"] = managed

    queue.enforce_deadlines(10_000.0)

    managed.terminate.assert_not_called()


def test_restart_service_uses_single_termination_path(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    managed = _TaskManagedStub(cmd=["journal", "sense"], start_time=100.0)
    managed.name = "sense"
    managed.ref = "ref-sense"
    mod._managed_procs = [managed]
    mod._SERVICE_STATE.clear()
    mod._SERVICE_STATE["sense"] = {
        "restart": False,
        "shutdown_timeout": 7,
    }

    def terminate_now(key, managed_arg, timeout, reason):
        assert key == "sense"
        assert managed_arg is managed
        assert timeout == 7
        assert reason == "restart"
        managed_arg.terminate(timeout=timeout)

    monkeypatch.setattr(mod, "_start_termination_thread", terminate_now)

    assert mod._restart_service("sense") is True
    managed.terminate.assert_called_once_with(timeout=7)
    assert mod._SERVICE_STATE["sense"]["restart"] is True


def test_stop_process_uses_service_shutdown_timeout():
    mod = importlib.import_module("solstone.think.supervisor")
    managed = _TaskManagedStub(cmd=["journal", "spl"], start_time=100.0)
    managed.name = "spl"
    mod._SERVICE_STATE.clear()
    mod._SERVICE_STATE["spl"] = {
        "restart": True,
        "shutdown_timeout": 9,
    }

    mod._stop_process(managed)

    managed.terminate.assert_called_once_with(timeout=9)
    managed.cleanup.assert_called_once_with()


def test_start_local_server_launches_llama_server_key_and_cmd(
    tmp_path, monkeypatch, capsys
):
    mod = importlib.import_module("solstone.think.supervisor")
    from solstone.think.providers import local_install, local_server

    mod._SERVICE_STATE.clear()
    binary = tmp_path / "llama-server"
    # ensure_artifacts_installed always resolves artifacts under the selected
    # model's directory; the spawn guard rejects anything else, so the stub must
    # return realistic in-model-dir paths.
    model_artifact_dir = local_install.model_dir(mod.LOCAL_MODEL)
    gguf = model_artifact_dir / "model.gguf"
    mmproj = model_artifact_dir / "mmproj.gguf"
    written_ports = []
    spawned = []
    managed = _TaskManagedStub(cmd=[])
    managed.name = "llama-server"
    managed.process.returncode = None

    monkeypatch.setattr(
        local_install,
        "ensure_artifacts_installed",
        lambda model_id: (binary, gguf, mmproj),
    )
    monkeypatch.setattr(mod, "find_available_port", lambda: 2468)
    monkeypatch.setattr(
        mod,
        "write_service_port",
        lambda service, port: written_ports.append((service, port)),
    )
    monkeypatch.setattr(local_server, "_probe_health", lambda port: ("ready", None))

    def fake_spawn(cmd, *, ref=None, callosum=None, day=None):
        spawned.append(cmd)
        managed.cmd = cmd
        managed.ref = ref
        return managed

    monkeypatch.setattr(mod.RunnerManagedProcess, "spawn", fake_spawn)

    result = mod.start_local_server()

    assert result is managed
    assert written_ports == [("local", 2468)]
    assert spawned == [
        [
            str(binary),
            "-m",
            str(gguf),
            "--alias",
            mod.LOCAL_MODEL,
            "--host",
            "127.0.0.1",
            "--port",
            "2468",
            "--jinja",
            "--mmproj",
            str(mmproj),
        ]
    ]
    assert "0.0.0.0" not in spawned[0]
    assert mod._SERVICE_STATE["llama-server"]["restart"] is True
    assert mod.LOCAL_MODEL_WARMING_UP_COPY in capsys.readouterr().out


def test_start_local_server_skips_missing_artifacts(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    from solstone.think.providers import local_install

    monkeypatch.setattr(
        local_install,
        "ensure_artifacts_installed",
        lambda model_id: (_ for _ in ()).throw(RuntimeError("missing")),
    )
    launch = MagicMock()
    monkeypatch.setattr(mod, "_launch_process", launch)

    assert mod.start_local_server() is None
    launch.assert_not_called()


def test_handle_runner_exits_restarts_llama_server_by_managed_name(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    mod._SERVICE_STATE.clear()
    mod._RESTART_POLICIES.clear()
    monkeypatch.setattr(mod.time, "time", lambda: 100.0)
    monkeypatch.setattr(mod, "shutdown_requested", False)

    managed = _TaskManagedStub(cmd=["/tmp/llama-server", "-m", "/tmp/model.gguf"])
    managed.name = "llama-server"
    managed.process.poll.return_value = 1
    managed.process.returncode = 1
    replacement = _TaskManagedStub(cmd=managed.cmd)
    replacement.name = "llama-server"
    launched = []

    mod._SERVICE_STATE["llama-server"] = {
        "restart": True,
        "shutdown_timeout": 12,
    }

    def fake_launch(name, cmd, *, restart=False, shutdown_timeout=15, ref=None):
        launched.append((name, cmd, restart, shutdown_timeout))
        return replacement

    monkeypatch.setattr(mod, "_launch_process", fake_launch)
    monkeypatch.setattr(mod, "_supervisor_callosum", None)

    procs = [managed]
    asyncio.run(mod.handle_runner_exits(procs))

    assert launched == [
        ("llama-server", managed.cmd, True, 12),
    ]
    assert procs == [replacement]


def test_supervisor_singleton_lock_acquired(tmp_path, monkeypatch):
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "health").mkdir(parents=True, exist_ok=True)
    monkeypatch.setattr(sys, "argv", ["supervisor"])

    def stop_after_lock():
        raise SystemExit(0)

    # Skip maint discovery/subprocess runs — unrelated to lock acquisition and
    # slow enough on a fresh tmp_path to blow the 5s pytest-timeout under load.
    monkeypatch.setattr(mod, "run_pending_tasks", lambda *a, **k: (0, 0))
    monkeypatch.setattr(mod, "_sweep_orphaned_sol_processes", lambda *_a, **_k: 0)
    monkeypatch.setattr(mod.time, "sleep", lambda _seconds: None)
    monkeypatch.setattr(mod, "start_callosum_in_process", stop_after_lock)

    with pytest.raises(SystemExit) as exc:
        mod.main()

    assert exc.value.code == 0
    assert (tmp_path / "health" / "supervisor.lock").exists()
    assert (tmp_path / "health" / "supervisor.pid").read_text().strip() == str(
        os.getpid()
    )
    start_time = float(
        (tmp_path / "health" / "supervisor.start_time").read_text().strip()
    )
    assert start_time > 0


def test_supervisor_singleton_lock_blocked(tmp_path, monkeypatch, capsys):
    import fcntl

    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.delenv("INVOCATION_ID", raising=False)
    health_dir = tmp_path / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    lock_file = open(health_dir / "supervisor.lock", "w")
    fcntl.flock(lock_file, fcntl.LOCK_EX | fcntl.LOCK_NB)
    (health_dir / "supervisor.pid").write_text("12345")
    monkeypatch.setattr(sys, "argv", ["supervisor"])

    start_mock = MagicMock()
    monkeypatch.setattr(mod, "start_callosum_in_process", start_mock)

    try:
        with pytest.raises(SystemExit) as exc:
            mod.main()
    finally:
        lock_file.close()

    assert exc.value.code == 1
    output = capsys.readouterr().out
    assert "Supervisor already running" in output
    assert "PID 12345" in output
    start_mock.assert_not_called()


def test_supervisor_singleton_lock_blocked_under_systemd_exits_cleanly(
    tmp_path, monkeypatch, capsys
):
    import fcntl

    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setenv("INVOCATION_ID", "test-invocation-uuid")
    health_dir = tmp_path / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    lock_file = open(health_dir / "supervisor.lock", "w")
    fcntl.flock(lock_file, fcntl.LOCK_EX | fcntl.LOCK_NB)
    (health_dir / "supervisor.pid").write_text("12345")
    monkeypatch.setattr(sys, "argv", ["supervisor"])

    start_mock = MagicMock()
    monkeypatch.setattr(mod, "start_callosum_in_process", start_mock)

    try:
        with pytest.raises(SystemExit) as exc:
            mod.main()
    finally:
        lock_file.close()

    assert exc.value.code == 0
    output = capsys.readouterr().out
    assert (
        "Supervisor already running (PID 12345) - exiting cleanly under "
        "systemd activation"
    ) in output
    start_mock.assert_not_called()


def test_supervisor_singleton_lock_blocked_with_health(tmp_path, monkeypatch, capsys):
    import fcntl

    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.delenv("INVOCATION_ID", raising=False)
    health_dir = tmp_path / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    lock_file = open(health_dir / "supervisor.lock", "w")
    fcntl.flock(lock_file, fcntl.LOCK_EX | fcntl.LOCK_NB)
    (health_dir / "supervisor.pid").write_text("12345")
    (health_dir / "callosum.sock").touch()
    monkeypatch.setattr(sys, "argv", ["supervisor"])

    start_mock = MagicMock()
    health_mock = MagicMock(return_value=0)
    monkeypatch.setattr(mod, "start_callosum_in_process", start_mock)
    monkeypatch.setattr("solstone.think.health_cli.health_check", health_mock)

    try:
        with pytest.raises(SystemExit) as exc:
            mod.main()
    finally:
        lock_file.close()

    assert exc.value.code == 1
    output = capsys.readouterr().out
    assert "Supervisor already running" in output
    assert "PID 12345" in output
    health_mock.assert_called_once_with()
    start_mock.assert_not_called()


def test_supervisor_singleton_lock_blocked_with_health_under_systemd_skips_health_check(
    tmp_path, monkeypatch, capsys
):
    import fcntl

    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setenv("INVOCATION_ID", "test-invocation-uuid")
    health_dir = tmp_path / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    lock_file = open(health_dir / "supervisor.lock", "w")
    fcntl.flock(lock_file, fcntl.LOCK_EX | fcntl.LOCK_NB)
    (health_dir / "supervisor.pid").write_text("12345")
    (health_dir / "callosum.sock").touch()
    monkeypatch.setattr(sys, "argv", ["supervisor"])

    start_mock = MagicMock()
    health_mock = MagicMock(return_value=0)
    monkeypatch.setattr(mod, "start_callosum_in_process", start_mock)
    monkeypatch.setattr("solstone.think.health_cli.health_check", health_mock)

    try:
        with pytest.raises(SystemExit) as exc:
            mod.main()
    finally:
        lock_file.close()

    assert exc.value.code == 0
    output = capsys.readouterr().out
    assert (
        "Supervisor already running (PID 12345) - exiting cleanly under "
        "systemd activation"
    ) in output
    health_mock.assert_not_called()
    start_mock.assert_not_called()


def test_is_supervisor_up_without_pid_file(tmp_path, monkeypatch):
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "health").mkdir(parents=True, exist_ok=True)

    assert mod.is_supervisor_up() is False


def test_is_supervisor_up_with_dead_pid(tmp_path, monkeypatch):
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    health_dir = tmp_path / "health"
    health_dir.mkdir(parents=True, exist_ok=True)

    proc = subprocess.Popen(["true"])
    proc.wait()
    (health_dir / "supervisor.pid").write_text(str(proc.pid))

    assert mod.is_supervisor_up() is False


def test_is_supervisor_up_with_live_pid_missing_start_time(tmp_path, monkeypatch):
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    health_dir = tmp_path / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    (health_dir / "supervisor.pid").write_text(str(os.getpid()))

    assert mod.is_supervisor_up() is False


def test_is_supervisor_up_with_live_pid_mismatched_start_time(tmp_path, monkeypatch):
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    health_dir = tmp_path / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    (health_dir / "supervisor.pid").write_text(str(os.getpid()))
    create_time = psutil.Process(os.getpid()).create_time()
    (health_dir / "supervisor.start_time").write_text(str(create_time + 60))

    assert mod.is_supervisor_up() is False


def test_is_supervisor_up_with_matching_process_identity(tmp_path, monkeypatch):
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    health_dir = tmp_path / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    (health_dir / "supervisor.pid").write_text(str(os.getpid()))
    (health_dir / "supervisor.start_time").write_text(
        str(psutil.Process(os.getpid()).create_time())
    )

    assert mod.is_supervisor_up() is True
