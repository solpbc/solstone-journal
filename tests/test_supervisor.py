# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import asyncio
import importlib
import io
import json
import logging
import os
import socket
import subprocess
import sys
import threading
import time
from datetime import date
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock

import psutil
import pytest

from solstone.think.maint import MaintTask, MaintTaskResult
from solstone.think.processing import (
    DISPLAY_POWERSAVE_UNAVAILABLE,
    DisplayPowersaveReading,
    DisplayPowersaveSettings,
    GateSettings,
    ProcessingSettings,
    TimeWindowSettings,
)
from solstone.think.providers.artifact_proof import ReadinessOutcome
from tests.helpers.module_mocks import (
    capturing_thread_constructor,
    module_mock,
)


@pytest.fixture(autouse=True)
def _speakers_analyze_installation_ready(monkeypatch, tmp_path):
    from tests.helpers.speakers_analyze import install_enter_generation_stub

    install_enter_generation_stub(monkeypatch, tmp_path)


def _mlx_readiness(
    *,
    model_installed: bool = True,
    ram_sufficient: bool = True,
    platform_supported: bool = True,
    package_available: bool = True,
    runtime_dir: str = "/tmp/snap",
    model_id: str = "mlx-model",
) -> ReadinessOutcome:
    ready = (
        model_installed and ram_sufficient and platform_supported and package_available
    )
    return ReadinessOutcome(
        provider="local",
        status="ready" if ready else "missing-or-mismatched",
        reason_code="ready" if ready else "manifest_missing",
        target={"model_id": model_id},
        install={
            "install_state": "idle",
            "install_error": None,
            "error_code": None,
            "attempt_id": None,
            "progress_bytes_received": None,
            "progress_bytes_total": None,
            "last_transition_at": None,
            "last_progress_at": None,
        },
        host={
            "platform_supported": platform_supported,
            "package_available": package_available,
            "ram_sufficient": ram_sufficient,
        },
        artifacts={
            "model_installed": model_installed,
            "runtime_dir": runtime_dir,
        },
        proof={},
    )


@pytest.fixture(autouse=True)
def _default_thinking_engine_selected(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    monkeypatch.setattr(mod, "no_thinking_engine_chosen", lambda: False)


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
    monkeypatch.setenv("SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_ID", "generation")
    monkeypatch.setenv("SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_FD", "42")
    monkeypatch.setenv("SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_TOKEN", "123")

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

    def fake_spawn(cmd, *, ref=None, callosum=None, day=None, env=None):
        assert cmd == ["journal", "sense"]
        assert ref == "ref-1"
        assert day is None
        assert env["SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_ID"] == "generation"
        assert env["SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_FD"] == "42"
        assert env["SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_TOKEN"] == "123"
        return managed

    monkeypatch.setattr(mod.RunnerManagedProcess, "spawn", fake_spawn)

    result = mod._launch_process(
        "unit",
        ["journal", "sense"],
        restart=True,
        shutdown_timeout=7,
        ref="ref-1",
        env=os.environ.copy(),
    )

    assert result is managed
    assert isinstance(result, mod.RunnerManagedProcess)
    assert mod._SERVICE_STATE["unit"] == {
        "restart": True,
        "shutdown_timeout": 7,
    }


def test_launch_process_records_uptime_without_restart(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    mod._SERVICE_STATE.clear()
    mod._RESTART_POLICIES.clear()
    monkeypatch.setattr(mod, "_supervisor_ref", None)
    monkeypatch.setattr(mod, "_supervisor_start", None)
    monkeypatch.setattr(mod, "_task_queue", None)
    monkeypatch.setattr(mod, "_callosum_server", None)
    monkeypatch.setattr(mod.scheduler, "collect_status", lambda: [])

    clock = {"now": 100.0}
    monkeypatch.setattr(mod.time, "time", lambda: clock["now"])

    process = MagicMock()
    process.pid = 12345
    process.poll.return_value = None
    managed = mod.RunnerManagedProcess(
        process=process,
        name="parakeet-server",
        log_writer=MagicMock(),
        cmd=["parakeet-server"],
        _threads=[],
        ref="ref-1",
        _start_time=100.0,
        _callosum=None,
    )
    monkeypatch.setattr(
        mod.RunnerManagedProcess,
        "spawn",
        lambda *_args, **_kwargs: managed,
    )

    result = mod._launch_process(
        "parakeet-server",
        ["parakeet-server"],
        restart=False,
        ref="ref-1",
    )
    clock["now"] = 137.0

    status = mod.collect_status([result])

    assert status["services"] == [
        {
            "name": "parakeet-server",
            "ref": "ref-1",
            "pid": 12345,
            "uptime_seconds": 37,
        }
    ]


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


def test_parse_args_app_supervised_flag():
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))

    parser = mod.parse_args()
    args = parser.parse_args(
        ["5016", "--app-supervised", "--no-daily", "--no-schedule"]
    )

    assert args.port == 5016
    assert args.app_supervised is True
    assert args.no_daily is True
    assert args.no_schedule is True


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
    from solstone.think import speakers_analyze_installation as installation

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.delenv("SOL_SUPERVISOR_SPAWNED", raising=False)
    monkeypatch.setattr(
        sys,
        "argv",
        ["supervisor", "0", "--no-daily", "--no-schedule"],
    )
    monkeypatch.setattr(mod, "run_pending_tasks", lambda *a, **k: [])
    monkeypatch.setattr(mod, "_sweep_orphaned_sol_processes", lambda *_a, **_k: 0)
    monkeypatch.setattr(mod.time, "sleep", lambda _seconds: None)
    monkeypatch.setattr(mod, "start_callosum_in_process", lambda: None)
    monkeypatch.setattr(mod, "stop_callosum_in_process", lambda **_kwargs: None)
    monkeypatch.setattr(mod, "wait_for_convey_ready", lambda _proc: True)

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

    lifecycle_order = []

    class FakeGeneration:
        generation_id = "test-generation"

        def release(self):
            lifecycle_order.append("release-generation")

    monkeypatch.setattr(
        installation,
        "enter_speakers_analyze_generation",
        lambda **_kwargs: FakeGeneration(),
    )
    stop_order = []
    monkeypatch.setattr(
        mod,
        "_stop_process",
        lambda managed, **_kwargs: (
            stop_order.append(managed.name),
            lifecycle_order.append(f"stop-{managed.name}"),
        ),
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
    assert lifecycle_order[-1] == "release-generation"
    assert lifecycle_order[:-1] == [
        "stop-spl",
        "stop-cortex",
        "stop-sense",
        "stop-convey",
    ]


@pytest.mark.parametrize(
    ("convey_accepting", "expected_ready"),
    [(True, True), (False, False)],
)
def test_supervisor_readiness_marker_requires_started_convey_accepting(
    tmp_path, monkeypatch, convey_accepting, expected_ready
):
    """A started Convey process must still accept before supervisor marks ready."""
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.delenv("SOL_SUPERVISOR_SPAWNED", raising=False)
    monkeypatch.setattr(
        sys,
        "argv",
        ["supervisor", "0", "--no-daily", "--no-schedule"],
    )
    monkeypatch.setattr(mod, "run_pending_tasks", lambda *a, **k: [])
    monkeypatch.setattr(mod, "_sweep_orphaned_sol_processes", lambda *_a, **_k: 0)
    monkeypatch.setattr(mod.time, "sleep", lambda _seconds: None)
    monkeypatch.setattr(mod, "start_callosum_in_process", lambda: None)
    monkeypatch.setattr(mod, "stop_callosum_in_process", lambda **_kwargs: None)
    monkeypatch.setattr(mod, "wait_for_convey_ready", lambda _proc: True)
    monkeypatch.setattr(mod, "is_solstone_up", lambda timeout=1.0: convey_accepting)
    monkeypatch.setattr(mod, "is_local_provider_needed", lambda: False)
    monkeypatch.setattr(mod, "read_service_port", lambda _name: 5015)

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
    monkeypatch.setattr(mod, "_stop_process", lambda managed, **_kwargs: None)

    events: list[str] = []
    monkeypatch.setattr(mod, "signal_ready", lambda: events.append("ready"))

    def interrupt_supervise(coro):
        coro.close()
        raise KeyboardInterrupt

    monkeypatch.setattr(mod.asyncio, "run", interrupt_supervise)

    try:
        mod.main()
    finally:
        os.environ.pop("SOL_SUPERVISOR_SPAWNED", None)

    assert ("ready" in events) is expected_ready


def _run_supervisor_main_for_shutdown_knobs(tmp_path, monkeypatch, *, argv):
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.delenv("SOL_SUPERVISOR_SPAWNED", raising=False)
    monkeypatch.delenv("SOLSTONE_APP_SUPERVISED", raising=False)
    monkeypatch.setattr(sys, "argv", argv)
    monkeypatch.setattr(mod, "run_pending_tasks", lambda *a, **k: [])
    monkeypatch.setattr(mod, "_sweep_orphaned_sol_processes", lambda *_a, **_k: 0)
    monkeypatch.setattr(mod.time, "sleep", lambda _seconds: None)
    monkeypatch.setattr(mod, "start_callosum_in_process", lambda: None)
    monkeypatch.setattr(mod, "is_local_provider_needed", lambda: False)

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

    managed = _TaskManagedStub(cmd=["journal", "sense"])
    managed.name = "sense"
    monkeypatch.setattr(mod, "start_sense", lambda: managed)

    events: list[str] = []
    captures: dict[str, list] = {
        "task_shutdown": [],
        "stop_process": [],
        "callosum_join": [],
    }

    class FakeTaskQueue:
        def __init__(self, *args, **kwargs):
            self.caps = {}

        @staticmethod
        def get_command_name(cmd):
            return mod._command_partition(cmd)

        def set_cap(self, cmd_name, seconds):
            self.caps[cmd_name] = seconds

        def set_ready(self):
            pass

        def shutdown(self, *, timeout):
            captures["task_shutdown"].append(timeout)

    monkeypatch.setattr(mod, "TaskQueue", FakeTaskQueue)
    monkeypatch.setattr(mod, "signal_ready", lambda: events.append("ready"))

    def record_watcher_start():
        events.append("watcher")
        assert mod._managed_procs == [managed]

    monkeypatch.setattr(mod, "start_parent_death_watcher", record_watcher_start)
    monkeypatch.setattr(
        mod,
        "_stop_process",
        lambda proc, *, timeout_cap=None: captures["stop_process"].append(
            (proc.name, timeout_cap)
        ),
    )
    monkeypatch.setattr(
        mod,
        "stop_callosum_in_process",
        lambda *, join_timeout=5.0: captures["callosum_join"].append(join_timeout),
    )
    exit_now = MagicMock()
    monkeypatch.setattr(mod.os, "_exit", exit_now)

    def interrupt_supervise(coro):
        events.append("run")
        coro.close()
        raise KeyboardInterrupt

    monkeypatch.setattr(mod.asyncio, "run", interrupt_supervise)

    try:
        mod.main()
    finally:
        os.environ.pop("SOL_SUPERVISOR_SPAWNED", None)

    return mod, captures, events, exit_now


def test_app_supervised_main_uses_watcher_and_compressed_shutdown_knobs(
    tmp_path, monkeypatch
):
    from solstone.think import install_guard, service

    reconcile = MagicMock()
    install_wrappers = MagicMock()
    monkeypatch.setattr(service, "reconcile_installed_unit", reconcile)
    monkeypatch.setattr(install_guard, "install_wrappers", install_wrappers)

    mod, captures, events, exit_now = _run_supervisor_main_for_shutdown_knobs(
        tmp_path,
        monkeypatch,
        argv=[
            "supervisor",
            "0",
            "--app-supervised",
            "--no-daily",
            "--no-schedule",
            "--no-convey",
            "--no-cortex",
            "--no-spl",
        ],
    )

    assert events == ["ready", "watcher", "run"]
    assert captures["task_shutdown"] == [mod.APP_SUPERVISED_TASK_DRAIN_S]
    assert captures["stop_process"] == [("sense", mod.APP_SUPERVISED_CHILD_STOP_S)]
    assert captures["callosum_join"] == [mod.APP_SUPERVISED_CALLOSUM_JOIN_S]
    exit_now.assert_not_called()
    reconcile.assert_not_called()
    install_wrappers.assert_not_called()


def test_default_main_uses_default_shutdown_knobs(tmp_path, monkeypatch):
    mod, captures, events, _exit_now = _run_supervisor_main_for_shutdown_knobs(
        tmp_path,
        monkeypatch,
        argv=[
            "supervisor",
            "0",
            "--no-daily",
            "--no-schedule",
            "--no-convey",
            "--no-cortex",
            "--no-spl",
        ],
    )

    assert events == ["ready", "run"]
    assert captures["task_shutdown"] == [10]
    assert captures["stop_process"] == [("sense", None)]
    assert captures["callosum_join"] == [5.0]


def test_get_command_name():
    """Test command name extraction for queue serialization."""
    mod = importlib.import_module("solstone.think.supervisor")
    get = mod.TaskQueue.get_command_name

    # sol X -> X
    assert get(["journal", "indexer", "--rescan"]) == "indexer"
    assert get(["sol", "insight", "20240101"]) == "insight"
    assert get(["journal", "think", "--day", "20240101"]) == "daily"
    assert get(["journal", "brain", "renew-prerequisites"]) == "brain"
    assert get(["journal", "maintenance", "list"]) == "maintenance"
    assert get(["journal", "maintenance", "run", "foo:bar"]) == "maintenance:foo:bar"
    assert get(["journal", "maintenance", "run", "baz:qux"]) == "maintenance:baz:qux"
    assert get(["journal", "maintenance", "run", "foo:bar"]) == get(
        ["journal", "maintenance", "run", "foo:bar"]
    )
    assert get(["journal", "maintenance", "run", "foo:bar"]) != get(
        ["journal", "maintenance", "run", "baz:qux"]
    )

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
        ["journal", "indexer", "--rescan"],
        ["journal", "sense", "--day", "20260101"],
        ["journal", "maintenance", "list"],
        ["journal", "maintenance", "run", "foo:bar"],
    ],
)
def test_command_partition_matches_task_queue_get_command_name(cmd):
    mod = importlib.import_module("solstone.think.supervisor")
    runner = importlib.import_module("solstone.think.runner")

    assert runner._command_partition(cmd) == mod.TaskQueue.get_command_name(cmd)


def test_command_partition_groups_importer_paths():
    runner = importlib.import_module("solstone.think.runner")

    assert (
        runner._command_partition(
            [
                "journal",
                "importer",
                "/tmp/imports/first/source.m4a",
                "20260101_120000",
            ]
        )
        == "importer"
    )
    assert (
        runner._command_partition(
            [
                "journal",
                "importer",
                "/tmp/imports/second/source.m4a",
                "20260101_121500",
            ]
        )
        == "importer"
    )


def _fresh_task_queue(mod, *, on_queue_change=None):
    mod._task_queue = mod.TaskQueue(on_queue_change=on_queue_change)
    mod._supervisor_callosum = None
    return mod._task_queue


def _capture_thread_starts(monkeypatch, mod):
    spawned = []
    monkeypatch.setattr(
        mod,
        "threading",
        module_mock(
            mod.threading,
            Thread=capturing_thread_constructor(
                spawned,
                capture=lambda thread: thread._args,
            ),
        ),
    )
    return spawned


class _CaptureTaskQueue:
    def __init__(self):
        self.submissions = []

    def submit(self, cmd, day=None):
        self.submissions.append({"cmd": cmd, "day": day})


def _supervisor_processing_settings(
    mode: str,
    *,
    display_powersave_enabled: bool = False,
) -> ProcessingSettings:
    return ProcessingSettings(
        mode=mode,
        gate=GateSettings(
            time_window=TimeWindowSettings(
                enabled=True,
                start="02:00",
                end="06:00",
            ),
            display_powersave=DisplayPowersaveSettings(
                enabled=display_powersave_enabled
            ),
        ),
    )


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


def test_handle_segment_observed_deferred_live_submits_nothing(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    capture = _CaptureTaskQueue()
    monkeypatch.setattr(mod, "_task_queue", capture)
    monkeypatch.setattr(
        mod,
        "load_processing_settings",
        lambda: _supervisor_processing_settings("deferred"),
    )
    mod._flush_state.update(
        {
            "last_segment_ts": 123.0,
            "day": "20260526",
            "segment": "110000_300",
            "stream": "archon",
            "flushed": True,
        }
    )
    before = dict(mod._flush_state)

    mod._handle_segment_observed(
        {
            "tract": "observe",
            "event": "observed",
            "day": "20260527",
            "segment": "120000_300",
        }
    )

    assert capture.submissions == []
    assert dict(mod._flush_state) == before


def test_handle_segment_observed_no_engine_submits_nothing(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    capture = _CaptureTaskQueue()
    monkeypatch.setattr(mod, "_task_queue", capture)
    monkeypatch.setattr(
        mod,
        "load_processing_settings",
        lambda: _supervisor_processing_settings("realtime"),
    )
    monkeypatch.setattr(mod, "no_thinking_engine_chosen", lambda: True)
    mod._flush_state.update(
        {
            "last_segment_ts": 123.0,
            "day": "20260526",
            "segment": "110000_300",
            "stream": "archon",
            "flushed": True,
        }
    )
    before = dict(mod._flush_state)

    mod._handle_segment_observed(
        {
            "tract": "observe",
            "event": "observed",
            "day": "20260527",
            "segment": "120000_300",
        }
    )

    assert capture.submissions == []
    assert dict(mod._flush_state) == before


def test_handle_segment_observed_reads_processing_mode_fresh(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    capture = _CaptureTaskQueue()
    monkeypatch.setattr(mod, "_task_queue", capture)
    modes = ["realtime", "deferred"]

    def load_settings():
        return _supervisor_processing_settings(modes.pop(0))

    monkeypatch.setattr(mod, "load_processing_settings", load_settings)

    mod._handle_segment_observed(
        {
            "tract": "observe",
            "event": "observed",
            "day": "20260527",
            "segment": "120000_300",
        }
    )
    after_realtime = dict(mod._flush_state)
    mod._handle_segment_observed(
        {
            "tract": "observe",
            "event": "observed",
            "day": "20260527",
            "segment": "120500_300",
        }
    )

    assert len(capture.submissions) == 1
    assert capture.submissions[0]["cmd"][-1] == "--live"
    assert dict(mod._flush_state) == after_realtime


def test_check_segment_flush_deferred_guard_submits_nothing(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    capture = _CaptureTaskQueue()
    monkeypatch.setattr(mod, "_task_queue", capture)
    monkeypatch.setattr(
        mod,
        "load_processing_settings",
        lambda: _supervisor_processing_settings("deferred"),
    )
    mod._flush_state.update(
        {
            "last_segment_ts": 1.0,
            "day": "20260527",
            "segment": "120000_300",
            "stream": None,
            "flushed": False,
        }
    )

    mod._check_segment_flush()

    assert capture.submissions == []
    assert mod._flush_state["flushed"] is False


def test_check_segment_flush_no_engine_guard_submits_nothing(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    capture = _CaptureTaskQueue()
    monkeypatch.setattr(mod, "_task_queue", capture)
    monkeypatch.setattr(
        mod,
        "load_processing_settings",
        lambda: _supervisor_processing_settings("realtime"),
    )
    monkeypatch.setattr(mod, "no_thinking_engine_chosen", lambda: True)
    mod._flush_state.update(
        {
            "last_segment_ts": 1.0,
            "day": "20260527",
            "segment": "120000_300",
            "stream": None,
            "flushed": False,
        }
    )

    mod._check_segment_flush()

    assert capture.submissions == []
    assert mod._flush_state["flushed"] is False


def test_check_segment_flush_realtime_submits(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    capture = _CaptureTaskQueue()
    monkeypatch.setattr(mod, "_task_queue", capture)
    monkeypatch.setattr(
        mod,
        "load_processing_settings",
        lambda: _supervisor_processing_settings("realtime"),
    )
    monkeypatch.setattr(mod.time, "time", lambda: 10_000.0)
    mod._flush_state.update(
        {
            "last_segment_ts": 1.0,
            "day": "20260527",
            "segment": "120000_300",
            "stream": None,
            "flushed": False,
        }
    )

    mod._check_segment_flush()

    assert len(capture.submissions) == 1
    assert "--flush" in capture.submissions[0]["cmd"]
    assert mod._flush_state["flushed"] is True


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


def test_reactive_task_caps_values():
    mod = importlib.import_module("solstone.think.supervisor")

    assert mod.REACTIVE_TASK_CAPS == {
        "daily": 21600,
        "segment": 4500,
        "indexer": 7200,
        "importer": 3600,
    }


def test_register_baseline_caps_sets_explicit_caps():
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)

    mod.register_baseline_caps(queue)

    backup_partition = mod.TaskQueue.get_command_name(
        ["journal", "maintenance", "run", "backup:run"]
    )
    expected = {
        "daily": 21600,
        "segment": 4500,
        "indexer": 7200,
        "importer": 3600,
        backup_partition: 49 * 60 * 60,
    }
    for name, seconds in expected.items():
        assert queue._effective_cap(name) == seconds
        assert queue._effective_cap(name) != mod.DEFAULT_TASK_MAX_RUNTIME


def test_from_scratch_reprocess_resolves_to_daily():
    mod = importlib.import_module("solstone.think.supervisor")

    assert (
        mod.TaskQueue.get_command_name(
            ["journal", "think", "-v", "--day", "20260527", "--from-scratch"]
        )
        == "daily"
    )


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

    spawned = _capture_thread_starts(monkeypatch, mod)

    # First request - should run immediately
    msg1 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["journal", "indexer", "--rescan"],
    }
    mod._handle_task_request(msg1)

    assert "indexer" in mod._task_queue._running
    assert len(spawned) == 1

    # Second request (different args) - should be queued
    msg2 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["journal", "indexer", "--rescan-full"],
    }
    mod._handle_task_request(msg2)

    assert len(spawned) == 1  # No new spawn
    assert "indexer" in mod._task_queue._queues
    assert len(mod._task_queue._queues["indexer"]) == 1
    # Queue entries are {refs, cmd} dicts (refs is a list for coalescing)
    assert mod._task_queue._queues["indexer"][0]["cmd"] == [
        "journal",
        "indexer",
        "--rescan-full",
    ]
    assert len(mod._task_queue._queues["indexer"][0]["refs"]) == 1


def test_task_queue_dedupe_exact_match(monkeypatch):
    """Test that exact same command is deduped in queue."""
    mod = importlib.import_module("solstone.think.supervisor")

    # Create fresh task queue (no callback to avoid callosum events)
    mod._task_queue = mod.TaskQueue(on_queue_change=None)

    _capture_thread_starts(monkeypatch, mod)

    # First request - runs
    msg1 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["journal", "indexer", "--rescan"],
    }
    mod._handle_task_request(msg1)

    # Second request (same cmd) - queued
    msg2 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["journal", "indexer", "--rescan"],
    }
    mod._handle_task_request(msg2)

    assert len(mod._task_queue._queues["indexer"]) == 1

    # Third request (same cmd again) - deduped, not added
    msg3 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["journal", "indexer", "--rescan"],
    }
    mod._handle_task_request(msg3)

    assert len(mod._task_queue._queues["indexer"]) == 1  # Still just 1


def test_task_queue_different_commands_independent(monkeypatch):
    """Test that different commands have independent queues."""
    mod = importlib.import_module("solstone.think.supervisor")

    # Create fresh task queue (no callback to avoid callosum events)
    mod._task_queue = mod.TaskQueue(on_queue_change=None)

    spawned = _capture_thread_starts(monkeypatch, mod)

    # Indexer request - runs
    msg1 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["journal", "indexer", "--rescan"],
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
            {"refs": ["queued-ref"], "cmd": ["journal", "indexer", "--rescan-full"]}
        ]
    }

    spawned = _capture_thread_starts(monkeypatch, mod)

    # Process queue
    mod._task_queue._process_next("indexer")

    # Should have spawned the queued task with its refs list
    assert len(spawned) == 1
    assert spawned[0][0] == ["queued-ref"]  # refs list preserved from queue
    assert spawned[0][1] == ["journal", "indexer", "--rescan-full"]  # cmd
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

    spawned = _capture_thread_starts(monkeypatch, mod)

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

    spawned = _capture_thread_starts(monkeypatch, mod)

    # Request with caller-provided ref
    msg = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["journal", "indexer", "--rescan"],
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

    _capture_thread_starts(monkeypatch, mod)

    # First request runs immediately
    msg1 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["journal", "indexer", "--rescan"],
        "ref": "first-ref",
    }
    mod._handle_task_request(msg1)

    # Second request gets queued with its own ref
    msg2 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["journal", "indexer", "--rescan-full"],
        "ref": "second-ref",
    }
    mod._handle_task_request(msg2)

    # Verify queued entry has the caller's ref in refs list
    assert len(mod._task_queue._queues["indexer"]) == 1
    assert mod._task_queue._queues["indexer"][0]["refs"] == ["second-ref"]
    assert mod._task_queue._queues["indexer"][0]["cmd"] == [
        "journal",
        "indexer",
        "--rescan-full",
    ]


def test_task_queue_coalesces_refs_on_dedupe(monkeypatch):
    """Test that duplicate queued requests coalesce their refs."""
    mod = importlib.import_module("solstone.think.supervisor")

    # Create fresh task queue (no callback to avoid callosum events)
    mod._task_queue = mod.TaskQueue(on_queue_change=None)

    _capture_thread_starts(monkeypatch, mod)

    # First request runs immediately
    msg1 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["journal", "indexer", "--rescan"],
        "ref": "first-ref",
    }
    mod._handle_task_request(msg1)

    # Second request (same cmd) gets queued
    msg2 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["journal", "indexer", "--rescan"],
        "ref": "second-ref",
    }
    mod._handle_task_request(msg2)

    # Third request (same cmd) should coalesce its ref into existing queue entry
    msg3 = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["journal", "indexer", "--rescan"],
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
                "cmd": ["journal", "indexer", "--rescan"],
            }
        ]
    }

    spawned = _capture_thread_starts(monkeypatch, mod)

    # Process queue
    mod._task_queue._process_next("indexer")

    # Should spawn with all three refs
    assert len(spawned) == 1
    assert spawned[0][0] == ["ref-A", "ref-B", "ref-C"]  # all refs passed
    assert spawned[0][1] == ["journal", "indexer", "--rescan"]


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

    spawned = _capture_thread_starts(monkeypatch, mod)

    mod._task_queue._running = {"indexer": {"ref": "stale-ref", "thread": dead_thread}}
    mod._task_queue._queues = {
        "indexer": [
            {"refs": ["queued-ref"], "cmd": ["journal", "indexer", "--rescan-full"]}
        ]
    }

    # Submit a new indexer task — should detect stale state and start immediately
    msg = {
        "tract": "supervisor",
        "event": "request",
        "cmd": ["journal", "indexer", "--rescan-new"],
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


BENIGN_LLAMA_LOAD_LOG = (
    "2026-06-12T12:00:00+00:00 [llama-server:stderr] "
    "llama_model_loader: loading model tensors\n"
    "2026-06-12T12:00:02+00:00 [llama-server:stderr] "
    "common_init_from_params: setting dry_penalty_last_n to ctx_size = 16384\n"
)


def _local_artifacts(local_install, binary, gguf, mmproj, *, backend="vulkan"):
    return local_install.LocalArtifacts(
        backend=backend,
        backend_reason=f"test {backend}",
        binary_path=binary,
        lib_dir=None,
        gguf_path=gguf,
        mmproj_path=mmproj,
    )


def _cuda_local_artifacts(local_install, binary, lib_dir, gguf, mmproj):
    return local_install.LocalArtifacts(
        backend="cuda",
        backend_reason="test cuda",
        binary_path=binary,
        lib_dir=lib_dir,
        gguf_path=gguf,
        mmproj_path=mmproj,
    )


class _FakeReservation:
    def __init__(self, port: int = 2468):
        self.port = port
        self.released = False
        self.closed = False

    def release_for_spawn(self) -> int:
        self.released = True
        self.closed = True
        return self.port

    def close(self) -> None:
        self.closed = True


def _native_launch_plan_for_test(plan, port: int, *, mlx_interpreter_path=None):
    """Return the native-plan contract used by supervisor launch tests."""
    if plan.backend == "mlx":
        assert mlx_interpreter_path is not None
        return {
            "outcome": "launch",
            "argv": [
                str(mlx_interpreter_path),
                "--host",
                "127.0.0.1",
                "--port",
                str(port),
                "--model",
                str(plan.runtime_dir),
            ],
            "context_tokens": 0,
            "parallel_slots": 0,
            "prompt_cache_mib": 0,
            "extra_env": {},
        }

    assert plan.binary_path is not None
    assert plan.model_path is not None
    assert plan.context_tokens is not None
    assert plan.parallel_slots is not None
    prompt_cache_mib = 2048 if plan.context_tokens >= 32768 else 0
    argv = [
        str(plan.binary_path),
        "-m",
        str(plan.model_path),
        "--alias",
        plan.model_id,
        "--host",
        "127.0.0.1",
        "--port",
        str(port),
        "--jinja",
        "--n-gpu-layers",
        "999",
        "-c",
        str(plan.context_tokens * plan.parallel_slots),
        "--parallel",
        str(plan.parallel_slots),
        "--kv-unified",
        "--cache-ram",
        str(prompt_cache_mib),
        "--no-context-shift",
        "--device",
        "CUDA0" if plan.backend == "cuda" else "Vulkan0",
    ]
    if plan.mmproj_path is not None:
        argv.extend(["--mmproj", str(plan.mmproj_path)])
    extra_env = {}
    if plan.backend == "cuda" and plan.gpu_index is not None:
        extra_env["CUDA_VISIBLE_DEVICES"] = str(plan.gpu_index)
        if plan.lib_dir is not None:
            inherited = os.environ.get("LD_LIBRARY_PATH")
            extra_env["LD_LIBRARY_PATH"] = (
                f"{plan.lib_dir}:{inherited}" if inherited else str(plan.lib_dir)
            )
    elif plan.gpu_index is not None:
        extra_env["GGML_VK_VISIBLE_DEVICES"] = str(plan.gpu_index)
    return {
        "outcome": "launch",
        "argv": argv,
        "context_tokens": plan.context_tokens,
        "parallel_slots": plan.parallel_slots,
        "prompt_cache_mib": prompt_cache_mib,
        "extra_env": extra_env,
    }


def _mlx_launch_plan(mod, runtime_dir: Path, *, model_id: str = "mlx-model"):
    return mod.LocalServerLaunchPlan(
        backend="mlx",
        desired_fingerprint_json='{"provider":"local"}',
        desired_fingerprint_sha256="fp-local",
        runtime_dir=runtime_dir,
        model_id=model_id,
    )


def _vulkan_launch_plan(
    mod,
    binary: Path,
    gguf: Path,
    mmproj: Path | None,
    *,
    vram_mib: int = 6390,
):
    from solstone.think.providers import local_server

    tier = local_server.select_server_tier(vram_mib)
    return mod.LocalServerLaunchPlan(
        backend="vulkan",
        desired_fingerprint_json='{"provider":"local"}',
        desired_fingerprint_sha256="fp-local",
        binary_path=binary,
        model_path=gguf,
        mmproj_path=mmproj,
        gpu_index=1,
        gpu_name="NVIDIA GeForce GTX 1660 Ti",
        gpu_vram_mib=vram_mib,
        vram_before_mib=512,
        context_tokens=tier.context_tokens,
        parallel_slots=tier.parallel_slots,
        visible_devices_env="GGML_VK_VISIBLE_DEVICES",
        backend_reason="test vulkan",
    )


def _cuda_launch_plan(
    mod,
    binary: Path,
    gguf: Path,
    mmproj: Path,
    lib_dir: Path,
    *,
    tiering_memory_mib: int | None = 20000,
    visible_device: str = "0",
):
    from solstone.think.providers import local_server

    tier = local_server.select_server_tier(tiering_memory_mib or 0)
    return mod.LocalServerLaunchPlan(
        backend="cuda",
        desired_fingerprint_json='{"provider":"local"}',
        desired_fingerprint_sha256="fp-local",
        binary_path=binary,
        model_path=gguf,
        mmproj_path=mmproj,
        lib_dir=lib_dir,
        gpu_index=int(visible_device),
        gpu_vram_mib=tiering_memory_mib,
        context_tokens=tier.context_tokens,
        parallel_slots=tier.parallel_slots,
        visible_devices_env="CUDA_VISIBLE_DEVICES",
        backend_reason="test cuda",
    )


def _requestable_cuda_plan(mod, tmp_path):
    return _cuda_launch_plan(
        mod,
        tmp_path / "llama-server",
        tmp_path / "model.gguf",
        tmp_path / "mmproj.gguf",
        tmp_path / "cuda-lib",
    )


def test_request_local_launch_plan_requires_successful_handshake(tmp_path):
    mod = importlib.import_module("solstone.think.supervisor")
    from solstone.think import core_handshake

    with pytest.raises(RuntimeError, match="handshake failure"):
        mod._request_local_launch_plan(
            _requestable_cuda_plan(mod, tmp_path),
            4010,
            handshake_checker=lambda: core_handshake.CoreHandshakeResult(
                "fail", "handshake failure"
            ),
        )


def test_request_local_launch_plan_rejects_nonzero_exit(tmp_path):
    mod = importlib.import_module("solstone.think.supervisor")
    from solstone.think import core_handshake

    with pytest.raises(RuntimeError, match="failed: native failure"):
        mod._request_local_launch_plan(
            _requestable_cuda_plan(mod, tmp_path),
            4010,
            handshake_checker=lambda: core_handshake.CoreHandshakeResult("ok"),
            helper_locator=lambda: Path("/tmp/solstone-core"),
            runner=lambda *_args, **_kwargs: SimpleNamespace(
                stdout="", stderr="native failure", returncode=1
            ),
        )


def test_request_local_launch_plan_rejects_native_rejection(tmp_path):
    mod = importlib.import_module("solstone.think.supervisor")
    from solstone.think import core_handshake

    with pytest.raises(RuntimeError, match="rejected: missing binary"):
        mod._request_local_launch_plan(
            _requestable_cuda_plan(mod, tmp_path),
            4010,
            handshake_checker=lambda: core_handshake.CoreHandshakeResult("ok"),
            helper_locator=lambda: Path("/tmp/solstone-core"),
            runner=lambda *_args, **_kwargs: SimpleNamespace(
                stdout='{"outcome":"rejected","reason":"missing binary"}',
                stderr="",
                returncode=0,
            ),
        )


def test_request_local_launch_plan_returns_native_launch_outcome(tmp_path):
    mod = importlib.import_module("solstone.think.supervisor")
    from solstone.think import core_handshake

    expected = {
        "outcome": "launch",
        "argv": ["/tmp/llama-server"],
        "context_tokens": 32768,
        "parallel_slots": 2,
        "prompt_cache_mib": 2048,
        "extra_env": {"CUDA_VISIBLE_DEVICES": "0"},
    }
    calls = []

    def runner(argv, **kwargs):
        calls.append((argv, kwargs))
        return SimpleNamespace(stdout=json.dumps(expected), stderr="", returncode=0)

    outcome = mod._request_local_launch_plan(
        _requestable_cuda_plan(mod, tmp_path),
        4010,
        handshake_checker=lambda: core_handshake.CoreHandshakeResult("ok"),
        helper_locator=lambda: Path("/tmp/solstone-core"),
        runner=runner,
    )

    assert outcome == expected
    assert calls[0][0] == ["/tmp/solstone-core", "local", "plan"]
    assert json.loads(calls[0][1]["input"])["nvidia_probe"]["vram_mib"] == 20000


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

    def fake_spawn(cmd, *, ref=None, callosum=None, day=None, env=None):
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


def test_exit_status_for_code_maps_empty_sentinel():
    from solstone.think.supervisor import _exit_status_for_code
    from solstone.think.utils import EXIT_EMPTY

    assert _exit_status_for_code(0) == "ok"
    assert _exit_status_for_code(EXIT_EMPTY) == "empty"
    assert _exit_status_for_code(1) == "error"
    assert _exit_status_for_code(75) == "error"


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

    def fake_spawn(cmd, *, ref=None, callosum=None, day=None, env=None):
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


def test_run_task_records_attempt_and_outcome_on_spawn(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    callosum = MagicMock()
    record_attempt = MagicMock()
    record_outcome = MagicMock(return_value=SimpleNamespace(entered_backoff=False))

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

    def fake_spawn(cmd, *, ref=None, callosum=None, day=None, env=None):
        managed.cmd = cmd
        managed.ref = ref
        return managed

    monkeypatch.setattr(mod, "CallosumConnection", FakeCallosum)
    monkeypatch.setattr(mod.RunnerManagedProcess, "spawn", fake_spawn)
    monkeypatch.setattr(mod, "record_attempt", record_attempt)
    monkeypatch.setattr(mod, "record_outcome", record_outcome)
    monkeypatch.setattr(queue, "_process_next", MagicMock())

    cmd = ["journal", "think", "-v", "--day", "20250101"]
    queue._run_task(["ref-1"], cmd, "daily", "20250101")

    record_attempt.assert_called_once()
    assert record_attempt.call_args.args == (cmd, "20250101", "ref-1")
    assert isinstance(record_attempt.call_args.kwargs["started_at"], float)
    record_outcome.assert_called_once()
    assert record_outcome.call_args.args == (cmd, "20250101", "ref-1")
    assert record_outcome.call_args.kwargs["exit_status"] == "ok"
    assert isinstance(record_outcome.call_args.kwargs["ended_at"], float)


def test_run_task_spawn_failure_does_not_record_attempt_or_outcome(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    callosum = MagicMock()
    record_attempt = MagicMock()
    record_outcome = MagicMock()
    process_next = MagicMock()

    class FakeCallosum:
        def start(self, callback=None):
            return None

        def emit(self, *args, **kwargs):
            return callosum.emit(*args, **kwargs)

        def stop(self):
            return callosum.stop()

    def fake_spawn(cmd, *, ref=None, callosum=None, day=None, env=None):
        raise RuntimeError("spawn failed")

    monkeypatch.setattr(mod, "CallosumConnection", FakeCallosum)
    monkeypatch.setattr(mod.RunnerManagedProcess, "spawn", fake_spawn)
    monkeypatch.setattr(mod, "record_attempt", record_attempt)
    monkeypatch.setattr(mod, "record_outcome", record_outcome)
    monkeypatch.setattr(queue, "_process_next", process_next)

    cmd = ["journal", "think", "-v", "--day", "20250101"]
    queue._run_task(["ref-1"], cmd, "daily", "20250101")

    record_attempt.assert_not_called()
    record_outcome.assert_not_called()
    callosum.stop.assert_called_once()
    process_next.assert_called_once_with("daily")


def test_submit_coalesce_does_not_record_attempt(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    monkeypatch.setattr(mod, "record_attempt", MagicMock())
    _capture_thread_starts(monkeypatch, mod)
    cmd = ["journal", "think", "-v", "--day", "20250101"]

    queue.submit(cmd, ref="running", day="20250101")
    queue.submit(cmd, ref="queued", day="20250101")
    queue.submit(cmd, ref="coalesced", day="20250101")

    mod.record_attempt.assert_not_called()
    assert queue._queues["daily"][0]["refs"] == ["queued", "coalesced"]


def test_handle_task_request_skip_does_not_record(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    managed = _TaskManagedStub(
        cmd=["journal", "importer", "--sync", "plaud"], start_time=100.0
    )
    queue._active["active-ref"] = managed
    queue.set_cap("importer", 50)

    monkeypatch.setattr(mod, "_task_queue", queue)
    monkeypatch.setattr(mod, "_supervisor_callosum", MagicMock())
    monkeypatch.setattr(mod, "record_attempt", MagicMock())
    monkeypatch.setattr(mod.time, "time", lambda: 150.0)

    mod._handle_task_request(
        {
            "tract": "supervisor",
            "event": "request",
            "cmd": ["journal", "importer", "--sync", "plaud"],
            "ref": "requested-ref",
        }
    )

    mod.record_attempt.assert_not_called()


def test_run_task_emits_backoff_notification_once(monkeypatch):
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

    def fake_spawn(cmd, *, ref=None, callosum=None, day=None, env=None):
        managed.cmd = cmd
        managed.ref = ref
        return managed

    monkeypatch.setattr(mod, "CallosumConnection", FakeCallosum)
    monkeypatch.setattr(mod.RunnerManagedProcess, "spawn", fake_spawn)
    monkeypatch.setattr(mod, "record_attempt", MagicMock())
    record_outcome = MagicMock(
        return_value=SimpleNamespace(
            entered_backoff=True,
            day="20250101",
            attempts=3,
            consecutive_non_completion=3,
            last_outcome="timeout",
        )
    )
    monkeypatch.setattr(mod, "record_outcome", record_outcome)
    monkeypatch.setattr(queue, "_process_next", MagicMock())
    cmd = ["journal", "think", "-v", "--day", "20250101"]

    queue._run_task(["ref-1"], cmd, "daily", "20250101")

    emitted = [call_args.args[:2] for call_args in callosum.emit.call_args_list]
    assert ("storage", "warning") in emitted
    assert ("notification", "show") in emitted

    callosum.emit.reset_mock()
    record_outcome.return_value = SimpleNamespace(entered_backoff=False)
    queue._run_task(["ref-2"], cmd, "daily", "20250101")

    emitted = [call_args.args[:2] for call_args in callosum.emit.call_args_list]
    assert ("storage", "warning") not in emitted
    assert ("notification", "show") not in emitted


def test_run_task_nudges_after_uncompleted_daily_only(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    callosum = MagicMock()
    nudge = MagicMock()

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

    def fake_spawn(cmd, *, ref=None, callosum=None, day=None, env=None):
        managed.cmd = cmd
        managed.ref = ref
        return managed

    monkeypatch.setattr(mod, "CallosumConnection", FakeCallosum)
    monkeypatch.setattr(mod.RunnerManagedProcess, "spawn", fake_spawn)
    monkeypatch.setattr(mod, "record_attempt", MagicMock())
    record_outcome = MagicMock()
    monkeypatch.setattr(mod, "record_outcome", record_outcome)
    monkeypatch.setattr(mod, "_nudge_catchup_drain", nudge)
    monkeypatch.setattr(queue, "_process_next", MagicMock())

    daily_cmd = ["journal", "think", "-v", "--day", "20250101"]
    record_outcome.return_value = SimpleNamespace(
        recorded=True,
        command_kind=mod.KIND_DAILY_CATCHUP,
        completed=False,
        entered_backoff=False,
    )
    queue._cap_terminated.add("ref-timeout")
    queue._run_task(["ref-timeout"], daily_cmd, "daily", "20250101")

    assert record_outcome.call_args.kwargs["exit_status"] == "timeout"
    nudge.assert_called_once_with(exclude_today=True)

    nudge.reset_mock()
    record_outcome.return_value = SimpleNamespace(
        recorded=True,
        command_kind=mod.KIND_DAILY_CATCHUP,
        completed=True,
        entered_backoff=False,
    )
    queue._run_task(["ref-complete"], daily_cmd, "daily", "20250101")
    nudge.assert_not_called()

    nudge.reset_mock()
    record_outcome.return_value = SimpleNamespace(
        recorded=True,
        command_kind="segment",
        completed=False,
        entered_backoff=False,
    )
    segment_cmd = [
        "journal",
        "think",
        "-v",
        "--day",
        "20250101",
        "--segment",
        "120000_300",
    ]
    queue._run_task(["ref-segment"], segment_cmd, "segment", "20250101")
    nudge.assert_not_called()


def test_handle_supervisor_drain_routes_exclude_today(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    calls = []

    class FakeDateTime:
        @staticmethod
        def now():
            return SimpleNamespace(date=lambda: date(2025, 1, 2))

    def fake_drain(*args, **kwargs):
        calls.append((args, kwargs))

    monkeypatch.setattr(mod, "datetime", FakeDateTime)
    monkeypatch.setattr(mod, "run_catchup_drain", fake_drain)

    mod._handle_supervisor_drain(
        {
            "tract": "supervisor",
            "event": "drain",
            "day": "20250101",
            "exclude_today": True,
        }
    )
    assert calls == [((), {"force_days": {"20250101"}})]

    calls.clear()
    mod._handle_supervisor_drain(
        {"tract": "supervisor", "event": "drain", "exclude_today": True}
    )
    assert calls == [((), {"exclude": {"20250102"}})]

    calls.clear()
    mod._handle_supervisor_drain({"tract": "supervisor", "event": "drain"})
    assert calls == [((), {})]


def test_startup_catchup_drain_reconciles_before_drain(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    order = []

    def reconcile():
        order.append("reconcile")
        return []

    def drain():
        order.append("drain")

    monkeypatch.setattr(mod, "reconcile_interrupted_attempts", reconcile)
    monkeypatch.setattr(mod, "run_catchup_drain", drain)

    mod._startup_catchup_drain()

    assert order == ["reconcile", "drain"]


def test_run_catchup_drain_no_engine_submits_nothing(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    capture = _CaptureTaskQueue()
    monkeypatch.setattr(mod, "_task_queue", capture)
    monkeypatch.setattr(mod, "no_thinking_engine_chosen", lambda: True)
    monkeypatch.setattr(
        mod,
        "updated_days",
        lambda **_kwargs: pytest.fail("drain should not enumerate days"),
    )

    assert mod.run_catchup_drain() == []
    assert capture.submissions == []


def test_run_gate_tick_deferred_open_runs_catchup_drain(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    drain = MagicMock()
    monkeypatch.setattr(
        mod,
        "load_processing_settings",
        lambda: _supervisor_processing_settings("deferred"),
    )
    monkeypatch.setattr(
        mod,
        "evaluate_drain_gate",
        lambda settings, now, reading: SimpleNamespace(open=True),
    )
    monkeypatch.setattr(mod, "run_catchup_drain", drain)
    mod._last_gate_tick = 0.0

    mod._run_gate_tick(60.0)

    drain.assert_called_once_with()


def test_run_gate_tick_deferred_closed_skips_catchup_drain(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    drain = MagicMock()
    monkeypatch.setattr(
        mod,
        "load_processing_settings",
        lambda: _supervisor_processing_settings("deferred"),
    )
    monkeypatch.setattr(
        mod,
        "evaluate_drain_gate",
        lambda settings, now, reading: SimpleNamespace(open=False),
    )
    monkeypatch.setattr(mod, "run_catchup_drain", drain)
    mod._last_gate_tick = 0.0

    mod._run_gate_tick(60.0)

    drain.assert_not_called()


def _write_catchup_state(journal: Path, records: dict[str, dict]) -> None:
    """Write catchup-state.json entries keyed '<day>:<kind>'."""
    health = journal / "health"
    health.mkdir(parents=True, exist_ok=True)
    (health / "catchup-state.json").write_text(
        json.dumps({"version": 1, "entries": records}), encoding="utf-8"
    )


def _catchup_record(day: str, kind: str, **overrides) -> dict:
    record = {
        "day": day,
        "command_kind": kind,
        "attempts": 1,
        "consecutive_non_completion": 1,
        "last_attempt_at": 0,
        "last_outcome": "timeout",
        "next_retry_at": 0,
        "entered_backoff_at": None,
        "notified_at": None,
        "fingerprint": "fp",
        "active": None,
        "reason_code": None,
        "timeout_seconds": None,
        "bounded": None,
    }
    record.update(overrides)
    return record


def _arm_catchup_retry_tick(mod, monkeypatch, *, mode="realtime", remote=False):
    """Put the tick past its cadence gate with a seeded watermark."""
    monkeypatch.setattr(
        mod, "load_processing_settings", lambda: _supervisor_processing_settings(mode)
    )
    monkeypatch.setattr(mod, "_is_remote_mode", remote)
    mod._last_catchup_retry_tick = 0.0


def test_read_catchup_retry_expiries_uses_later_of_both_kinds(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_catchup_state(
        tmp_path,
        {
            "20260101:daily-catchup": _catchup_record(
                "20260101", "daily-catchup", next_retry_at=100.0
            ),
            "20260101:segment-repair": _catchup_record(
                "20260101", "segment-repair", next_retry_at=250.0
            ),
        },
    )

    assert mod._read_catchup_retry_expiries() == [250.0]


def test_read_catchup_retry_expiries_skips_active_and_unretried(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_catchup_state(
        tmp_path,
        {
            # Active on one kind: cannot become eligible by expiry alone.
            "20260101:daily-catchup": _catchup_record(
                "20260101", "daily-catchup", next_retry_at=100.0, active="pid-1"
            ),
            "20260101:segment-repair": _catchup_record(
                "20260101", "segment-repair", next_retry_at=250.0
            ),
            # No backoff: already eligible, so no future crossing to detect.
            "20260102:daily-catchup": _catchup_record("20260102", "daily-catchup"),
            # KIND_SEGMENT never carries a meaningful retry and is not gated on.
            "20260103:segment": _catchup_record(
                "20260103", "segment", next_retry_at=400.0
            ),
        },
    )

    assert mod._read_catchup_retry_expiries() == []


def test_catchup_retry_tick_seeds_watermark_without_draining(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    drain = MagicMock()
    monkeypatch.setattr(mod, "run_catchup_drain", drain)
    _arm_catchup_retry_tick(mod, monkeypatch)
    mod._catchup_retry_watermark = 0.0

    now = time.time()
    _write_catchup_state(
        tmp_path,
        {
            "20260101:daily-catchup": _catchup_record(
                "20260101", "daily-catchup", next_retry_at=now - 10
            )
        },
    )

    mod._run_catchup_retry_tick(now)

    # _startup_catchup_drain() already covered days expired before boot.
    drain.assert_not_called()
    assert mod._catchup_retry_watermark == now


def test_catchup_retry_tick_drains_excluding_today(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    drain = MagicMock()
    monkeypatch.setattr(mod, "run_catchup_drain", drain)
    _arm_catchup_retry_tick(mod, monkeypatch)

    now = time.time()
    mod._catchup_retry_watermark = now - 60
    _write_catchup_state(
        tmp_path,
        {
            "20260101:daily-catchup": _catchup_record(
                "20260101", "daily-catchup", next_retry_at=now - 10
            )
        },
    )

    mod._run_catchup_retry_tick(now)

    today_str = date.today().strftime("%Y%m%d")
    drain.assert_called_once_with(exclude={today_str})


def test_catchup_retry_tick_fires_once_per_expiry(tmp_path, monkeypatch):
    """A day that re-enters backoff waits for its new expiry, not the next tick."""
    mod = importlib.import_module("solstone.think.supervisor")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    drain = MagicMock()
    monkeypatch.setattr(mod, "run_catchup_drain", drain)
    _arm_catchup_retry_tick(mod, monkeypatch)

    now = time.time()
    mod._catchup_retry_watermark = now - 60
    _write_catchup_state(
        tmp_path,
        {
            "20260101:daily-catchup": _catchup_record(
                "20260101", "daily-catchup", next_retry_at=now - 10
            )
        },
    )

    mod._run_catchup_retry_tick(now)
    mod._last_catchup_retry_tick = 0.0
    mod._run_catchup_retry_tick(now + 60)

    assert drain.call_count == 1


@pytest.mark.parametrize(
    "mode,remote", [("deferred", False), ("realtime", True), ("deferred", True)]
)
def test_catchup_retry_tick_skips_deferred_and_remote(
    tmp_path, monkeypatch, mode, remote
):
    mod = importlib.import_module("solstone.think.supervisor")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    drain = MagicMock()
    monkeypatch.setattr(mod, "run_catchup_drain", drain)
    _arm_catchup_retry_tick(mod, monkeypatch, mode=mode, remote=remote)

    now = time.time()
    mod._catchup_retry_watermark = now - 60
    _write_catchup_state(
        tmp_path,
        {
            "20260101:daily-catchup": _catchup_record(
                "20260101", "daily-catchup", next_retry_at=now - 10
            )
        },
    )

    mod._run_catchup_retry_tick(now)

    drain.assert_not_called()


def test_catchup_retry_tick_without_expiry_does_not_fingerprint(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    drain = MagicMock()
    monkeypatch.setattr(mod, "run_catchup_drain", drain)
    monkeypatch.setattr(
        "solstone.think.catchup_state.read_raw_input_fingerprint",
        lambda day: pytest.fail("expiry gate must not hash raw input"),
    )
    _arm_catchup_retry_tick(mod, monkeypatch)

    now = time.time()
    mod._catchup_retry_watermark = now - 60
    _write_catchup_state(
        tmp_path,
        {
            "20260101:daily-catchup": _catchup_record(
                "20260101", "daily-catchup", next_retry_at=now + 3600
            )
        },
    )

    mod._run_catchup_retry_tick(now)

    drain.assert_not_called()
    assert mod._catchup_retry_watermark == now


def test_catchup_retry_tick_submits_expired_day_through_drain(tmp_path, monkeypatch):
    """End to end: the real drain submits the expired day and skips the backed-off one."""
    mod = importlib.import_module("solstone.think.supervisor")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    capture = _CaptureTaskQueue()
    monkeypatch.setattr(mod, "_task_queue", capture)
    monkeypatch.setattr(mod, "no_thinking_engine_chosen", lambda: False)
    monkeypatch.setattr(mod, "updated_days", lambda **_kw: ["20260101", "20260102"])
    monkeypatch.setattr(
        "solstone.think.catchup_state.read_raw_input_fingerprint", lambda day: "fp"
    )
    _arm_catchup_retry_tick(mod, monkeypatch)

    now = time.time()
    mod._catchup_retry_watermark = now - 60
    _write_catchup_state(
        tmp_path,
        {
            "20260101:daily-catchup": _catchup_record(
                "20260101", "daily-catchup", next_retry_at=now - 10
            ),
            # Still inside its backoff window, fingerprint unchanged.
            "20260102:daily-catchup": _catchup_record(
                "20260102", "daily-catchup", next_retry_at=now + 3600
            ),
        },
    )

    mod._run_catchup_retry_tick(now)

    assert capture.submissions == [
        {"cmd": ["journal", "think", "-v", "--day", "20260101"], "day": "20260101"}
    ]


def test_catchup_retry_tick_respects_drain_engine_gate(tmp_path, monkeypatch):
    """The re-fire routes through run_catchup_drain, so its gates still apply."""
    mod = importlib.import_module("solstone.think.supervisor")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    capture = _CaptureTaskQueue()
    monkeypatch.setattr(mod, "_task_queue", capture)
    monkeypatch.setattr(mod, "no_thinking_engine_chosen", lambda: True)
    monkeypatch.setattr(
        mod,
        "updated_days",
        lambda **_kw: pytest.fail("drain should not enumerate days"),
    )
    _arm_catchup_retry_tick(mod, monkeypatch)

    now = time.time()
    mod._catchup_retry_watermark = now - 60
    _write_catchup_state(
        tmp_path,
        {
            "20260101:daily-catchup": _catchup_record(
                "20260101", "daily-catchup", next_retry_at=now - 10
            )
        },
    )

    mod._run_catchup_retry_tick(now)

    assert capture.submissions == []


def test_catchup_retry_tick_throttles_to_cadence(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    settings = MagicMock(
        side_effect=AssertionError("cadence gate should short-circuit")
    )
    monkeypatch.setattr(mod, "load_processing_settings", settings)
    mod._last_catchup_retry_tick = 100.0

    mod._run_catchup_retry_tick(100.0 + mod.CATCHUP_RETRY_TICK_INTERVAL_S - 1)

    settings.assert_not_called()


def test_run_gate_tick_realtime_skips_catchup_drain(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    drain = MagicMock()
    evaluate = MagicMock(return_value=SimpleNamespace(open=True))
    monkeypatch.setattr(
        mod,
        "load_processing_settings",
        lambda: _supervisor_processing_settings("realtime"),
    )
    monkeypatch.setattr(mod, "evaluate_drain_gate", evaluate)
    monkeypatch.setattr(mod, "run_catchup_drain", drain)
    mod._last_gate_tick = 0.0

    mod._run_gate_tick(60.0)

    evaluate.assert_not_called()
    drain.assert_not_called()


def test_run_gate_tick_throttles_catchup_drain(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    drain = MagicMock()
    monkeypatch.setattr(
        mod,
        "load_processing_settings",
        lambda: _supervisor_processing_settings("deferred"),
    )
    monkeypatch.setattr(
        mod,
        "evaluate_drain_gate",
        lambda settings, now, reading: SimpleNamespace(open=True),
    )
    monkeypatch.setattr(mod, "run_catchup_drain", drain)
    mod._last_gate_tick = 0.0

    mod._run_gate_tick(60.0)
    mod._run_gate_tick(119.0)
    mod._run_gate_tick(121.0)

    assert drain.call_count == 2


def test_run_gate_tick_disabled_display_powersave_does_not_poll(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    drain = MagicMock()
    poll = MagicMock(side_effect=AssertionError("poll should not be called"))

    def evaluate(settings, now, reading):
        assert reading == DISPLAY_POWERSAVE_UNAVAILABLE
        return SimpleNamespace(open=True)

    monkeypatch.setattr(mod, "_is_remote_mode", False)
    monkeypatch.setattr(
        mod,
        "load_processing_settings",
        lambda: _supervisor_processing_settings("deferred"),
    )
    monkeypatch.setattr(mod, "poll_display_powersave", poll)
    monkeypatch.setattr(mod, "evaluate_drain_gate", evaluate)
    monkeypatch.setattr(mod, "run_catchup_drain", drain)
    mod._last_gate_tick = 0.0

    mod._run_gate_tick(60.0)

    poll.assert_not_called()
    drain.assert_called_once_with()


def test_run_gate_tick_enabled_display_powersave_polls(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    reading = DisplayPowersaveReading(available=True, asleep=True, debounced=True)
    poll = MagicMock(return_value=reading)
    captured = {}

    def evaluate(settings, now, display_reading):
        captured["reading"] = display_reading
        return SimpleNamespace(open=False)

    monkeypatch.setattr(mod, "_is_remote_mode", False)
    monkeypatch.setattr(
        mod,
        "load_processing_settings",
        lambda: _supervisor_processing_settings(
            "deferred",
            display_powersave_enabled=True,
        ),
    )
    monkeypatch.setattr(mod, "poll_display_powersave", poll)
    monkeypatch.setattr(mod, "evaluate_drain_gate", evaluate)
    monkeypatch.setattr(mod, "run_catchup_drain", MagicMock())
    mod._last_gate_tick = 0.0

    mod._run_gate_tick(60.0)

    poll.assert_called_once()
    assert isinstance(poll.call_args.args[0], float)
    assert captured["reading"] == reading


def test_supervise_resets_display_powersave_monitor_on_entry(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    reset = MagicMock()
    monkeypatch.setattr(mod, "reset_display_powersave_monitor", reset)
    monkeypatch.setattr(mod, "shutdown_requested", True)

    asyncio.run(mod.supervise(daily=False, schedule=False, procs=[]))

    reset.assert_called_once_with()


def test_supervise_logs_tick_step_failure_and_continues(caplog, monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    monkeypatch.setattr(mod, "shutdown_requested", False)
    monkeypatch.setattr(mod, "_task_queue", None)
    monkeypatch.setattr(mod, "_supervisor_callosum", None)
    monkeypatch.setattr(mod, "_last_tick_step_failure", None)
    monkeypatch.setattr(mod, "reset_display_powersave_monitor", lambda: None)
    monkeypatch.setattr(mod, "_run_sync_tick", lambda _now: True)

    flush_calls = []
    monkeypatch.setattr(
        mod, "_check_segment_flush", lambda: flush_calls.append("flush")
    )

    scheduler_calls = 0

    def check_schedule():
        nonlocal scheduler_calls
        scheduler_calls += 1
        if scheduler_calls == 1:
            raise Exception("schedule boom")

    monkeypatch.setattr(mod.scheduler, "check", check_schedule)

    async def stop_after_two_ticks(_seconds):
        if scheduler_calls >= 2:
            mod.shutdown_requested = True

    monkeypatch.setattr(mod.asyncio, "sleep", stop_after_two_ticks)

    with caplog.at_level(logging.DEBUG):
        asyncio.run(mod.supervise(daily=False, schedule=True, procs=[]))

    errors = [
        record
        for record in caplog.records
        if record.levelno == logging.ERROR and "scheduler_check" in record.message
    ]
    assert len(errors) == 1
    assert errors[0].exc_info is not None
    assert scheduler_calls == 2
    assert len(flush_calls) == 2


def test_supervise_propagates_cancelled_error_from_guarded_step(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    monkeypatch.setattr(mod, "shutdown_requested", False)
    monkeypatch.setattr(mod, "_task_queue", None)
    monkeypatch.setattr(mod, "_supervisor_callosum", None)
    monkeypatch.setattr(mod, "reset_display_powersave_monitor", lambda: None)
    monkeypatch.setattr(mod, "_check_segment_flush", lambda: None)
    monkeypatch.setattr(mod, "_run_sync_tick", lambda _now: True)
    monkeypatch.setattr(
        mod.scheduler,
        "check",
        lambda: (_ for _ in ()).throw(asyncio.CancelledError()),
    )

    with pytest.raises(asyncio.CancelledError):
        asyncio.run(mod.supervise(daily=False, schedule=True, procs=[]))


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

    def fake_spawn(cmd, *, ref=None, callosum=None, day=None, env=None):
        return managed

    monkeypatch.setattr(mod, "CallosumConnection", FakeCallosum)
    monkeypatch.setattr(mod.RunnerManagedProcess, "spawn", fake_spawn)
    monkeypatch.setattr(mod, "_start_termination_thread", MagicMock())

    queue._run_task(["ref-1"], ["sol", "import"], "import")

    assert queue._history[0]["exit_status"] == "timeout"


def test_handle_task_request_opt_in_queues_differing_active_command(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = _fresh_task_queue(mod)
    spawned = _capture_thread_starts(monkeypatch, mod)
    active_cmd = ["journal", "think", "-v", "--day", "20250115"]
    incoming_cmd = ["journal", "think", "-v", "--day", "20250115", "--from-scratch"]
    partition = mod.TaskQueue.get_command_name(active_cmd)
    queue._active["active-ref"] = _TaskManagedStub(cmd=active_cmd, start_time=100.0)
    queue._running[partition] = {
        "ref": "active-ref",
        "thread": None,
        "scheduler_name": None,
    }

    mod._handle_task_request(
        {
            "tract": "supervisor",
            "event": "request",
            "cmd": incoming_cmd,
            "ref": "requested-ref",
            "day": "20250115",
            "queue_if_active_cmd_differs": True,
        }
    )

    assert queue._queues.get(partition) == [
        {
            "refs": ["requested-ref"],
            "cmd": incoming_cmd,
            "day": "20250115",
            "scheduler_name": None,
        }
    ]
    assert spawned == []


def test_handle_task_request_importer_opt_in_queues_different_file(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = _fresh_task_queue(mod)
    spawned = _capture_thread_starts(monkeypatch, mod)
    active_cmd = [
        "journal",
        "importer",
        "/tmp/imports/20260101_120000/source.m4a",
        "20260101_120000",
    ]
    incoming_cmd = [
        "journal",
        "importer",
        "/tmp/imports/20260101_121500/source.m4a",
        "20260101_121500",
    ]
    partition = mod.TaskQueue.get_command_name(active_cmd)
    queue._active["active-ref"] = _TaskManagedStub(cmd=active_cmd, start_time=100.0)
    queue._running[partition] = {
        "ref": "active-ref",
        "thread": None,
        "scheduler_name": None,
    }

    mod._handle_task_request(
        {
            "tract": "supervisor",
            "event": "request",
            "cmd": incoming_cmd,
            "ref": "requested-ref",
            "queue_if_active_cmd_differs": True,
        }
    )

    assert queue._queues.get(partition) == [
        {
            "refs": ["requested-ref"],
            "cmd": incoming_cmd,
            "day": None,
            "scheduler_name": None,
        }
    ]
    assert spawned == []


def test_handle_task_request_opt_in_skips_identical_active_command(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = _fresh_task_queue(mod)
    spawned = _capture_thread_starts(monkeypatch, mod)
    active_cmd = ["journal", "think", "-v", "--day", "20250115"]
    partition = mod.TaskQueue.get_command_name(active_cmd)
    queue._active["active-ref"] = _TaskManagedStub(cmd=active_cmd, start_time=100.0)
    queue._running[partition] = {
        "ref": "active-ref",
        "thread": None,
        "scheduler_name": None,
    }
    queue.set_cap(partition, 50)
    callosum = MagicMock()

    monkeypatch.setattr(mod, "_supervisor_callosum", callosum)
    monkeypatch.setattr(mod.time, "time", lambda: 150.0)

    mod._handle_task_request(
        {
            "tract": "supervisor",
            "event": "request",
            "cmd": active_cmd,
            "ref": "requested-ref",
            "day": "20250115",
            "queue_if_active_cmd_differs": True,
        }
    )

    callosum.emit.assert_called_once_with(
        "supervisor",
        "skipped",
        reason="still_running",
        ref="requested-ref",
        active_ref="active-ref",
        cmd=active_cmd,
        scheduler_name=None,
    )
    assert queue._queues == {}
    assert spawned == []


def test_handle_task_request_without_opt_in_skips_differing_active_command(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = _fresh_task_queue(mod)
    spawned = _capture_thread_starts(monkeypatch, mod)
    active_cmd = ["journal", "think", "-v", "--day", "20250115"]
    incoming_cmd = ["journal", "think", "-v", "--day", "20250115", "--from-scratch"]
    partition = mod.TaskQueue.get_command_name(active_cmd)
    queue._active["active-ref"] = _TaskManagedStub(cmd=active_cmd, start_time=100.0)
    queue._running[partition] = {
        "ref": "active-ref",
        "thread": None,
        "scheduler_name": None,
    }
    queue.set_cap(partition, 50)
    callosum = MagicMock()

    monkeypatch.setattr(mod, "_supervisor_callosum", callosum)
    monkeypatch.setattr(mod.time, "time", lambda: 150.0)

    mod._handle_task_request(
        {
            "tract": "supervisor",
            "event": "request",
            "cmd": incoming_cmd,
            "ref": "requested-ref",
            "day": "20250115",
        }
    )

    callosum.emit.assert_called_once_with(
        "supervisor",
        "skipped",
        reason="still_running",
        ref="requested-ref",
        active_ref="active-ref",
        cmd=incoming_cmd,
        scheduler_name=None,
    )
    assert queue._queues == {}
    assert spawned == []


def test_handle_task_request_importer_without_opt_in_skips_different_file(
    monkeypatch,
):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = _fresh_task_queue(mod)
    spawned = _capture_thread_starts(monkeypatch, mod)
    active_cmd = [
        "journal",
        "importer",
        "/tmp/imports/20260101_120000/source.m4a",
        "20260101_120000",
    ]
    incoming_cmd = [
        "journal",
        "importer",
        "/tmp/imports/20260101_121500/source.m4a",
        "20260101_121500",
    ]
    partition = mod.TaskQueue.get_command_name(active_cmd)
    queue._active["active-ref"] = _TaskManagedStub(cmd=active_cmd, start_time=100.0)
    queue._running[partition] = {
        "ref": "active-ref",
        "thread": None,
        "scheduler_name": None,
    }
    queue.set_cap(partition, 50)
    callosum = MagicMock()

    monkeypatch.setattr(mod, "_supervisor_callosum", callosum)
    monkeypatch.setattr(mod.time, "time", lambda: 150.0)

    mod._handle_task_request(
        {
            "tract": "supervisor",
            "event": "request",
            "cmd": incoming_cmd,
            "ref": "requested-ref",
        }
    )

    callosum.emit.assert_called_once_with(
        "supervisor",
        "skipped",
        reason="still_running",
        ref="requested-ref",
        active_ref="active-ref",
        cmd=incoming_cmd,
        scheduler_name=None,
    )
    assert queue._queues == {}
    assert spawned == []


def test_handle_task_request_active_skip_logs_one_warning(caplog, monkeypatch):
    """Pre-fix baseline had zero warning records for this branch.

    Command:
    `hop check make test-only TEST="tests/test_supervisor.py -k opt_in_skips_identical"`
    """
    mod = importlib.import_module("solstone.think.supervisor")
    queue = _fresh_task_queue(mod)
    active_cmd = [
        "journal",
        "importer",
        "/tmp/imports/20260101_120000/source.m4a",
        "20260101_120000",
    ]
    incoming_cmd = [
        "journal",
        "importer",
        "/tmp/imports/20260101_121500/source.m4a",
        "20260101_121500",
    ]
    partition = mod.TaskQueue.get_command_name(active_cmd)
    queue._active["active-ref"] = _TaskManagedStub(cmd=active_cmd, start_time=100.0)
    queue._running[partition] = {
        "ref": "active-ref",
        "thread": None,
        "scheduler_name": None,
    }
    queue.set_cap(partition, 50)

    monkeypatch.setattr(mod.time, "time", lambda: 150.0)
    caplog.set_level(logging.WARNING)

    mod._handle_task_request(
        {
            "tract": "supervisor",
            "event": "request",
            "cmd": incoming_cmd,
            "ref": "requested-ref",
            "scheduler_name": "import-start",
        }
    )

    warnings = [
        record for record in caplog.records if record.levelno == logging.WARNING
    ]
    assert len(warnings) == 1
    message = warnings[0].getMessage()
    assert "cmd_name=importer" in message
    assert "ref=requested-ref" in message
    assert "active_ref=active-ref" in message
    assert "reason=still_running" in message
    assert "scheduler_name=import-start" in message


def test_handle_task_request_without_task_queue_logs_one_warning(caplog, monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    monkeypatch.setattr(mod, "_task_queue", None)
    caplog.set_level(logging.WARNING)

    mod._handle_task_request(
        {
            "tract": "supervisor",
            "event": "request",
            "cmd": ["journal", "importer", "/tmp/source.m4a", "20260101_120000"],
            "ref": "requested-ref",
            "scheduler_name": "import-start",
        }
    )

    warnings = [
        record for record in caplog.records if record.levelno == logging.WARNING
    ]
    assert len(warnings) == 1
    message = warnings[0].getMessage()
    assert "task_queue_unavailable" in message
    assert "ref=requested-ref" in message
    assert "scheduler_name=import-start" in message
    assert (
        "cmd=['journal', 'importer', '/tmp/source.m4a', '20260101_120000']" in message
    )


def test_handle_task_request_skips_still_running(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    managed = _TaskManagedStub(
        cmd=["journal", "importer", "--sync", "plaud"], start_time=100.0
    )
    queue._active["active-ref"] = managed
    queue.set_cap("importer", 50)
    callosum = MagicMock()

    monkeypatch.setattr(mod, "_task_queue", queue)
    monkeypatch.setattr(mod, "_supervisor_callosum", callosum)
    monkeypatch.setattr(mod.time, "time", lambda: 150.0)

    mod._handle_task_request(
        {
            "tract": "supervisor",
            "event": "request",
            "cmd": ["journal", "importer", "--sync", "plaud"],
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
        cmd=["journal", "importer", "--sync", "plaud"],
        scheduler_name="sync-plaud",
    )
    assert queue._queues == {}


def test_handle_task_request_skips_wedged(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    managed = _TaskManagedStub(
        cmd=["journal", "importer", "--sync", "plaud"], start_time=100.0
    )
    queue._active["active-ref"] = managed
    queue.set_cap("importer", 50)
    callosum = MagicMock()

    monkeypatch.setattr(mod, "_task_queue", queue)
    monkeypatch.setattr(mod, "_supervisor_callosum", callosum)
    monkeypatch.setattr(mod.time, "time", lambda: 201.0)

    mod._handle_task_request(
        {
            "tract": "supervisor",
            "event": "request",
            "cmd": ["journal", "importer", "--sync", "plaud"],
            "ref": "requested-ref",
        }
    )

    assert callosum.emit.call_args.kwargs["reason"] == "wedged"
    assert callosum.emit.call_args.kwargs["active_ref"] == "active-ref"


def test_task_queue_shutdown_terminates_active_tasks():
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    first = _TaskManagedStub(cmd=["sol", "import"])
    second = _TaskManagedStub(cmd=["journal", "indexer"])
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
    second = _TaskManagedStub(cmd=["journal", "indexer"])
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
        cmd=["journal", "importer", "--sync", "plaud", "--save"],
        start_time=100.0,
    )
    queue._active["ref-1"] = managed
    queue.set_cap("importer", 50)

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
        "Task importer (cmd=journal importer --sync plaud --save, ref=ref-1) exceeded "
        "max_runtime of 50s (elapsed=100s); terminating"
    ) in caplog.text


def test_collect_task_status_reports_default_cap(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    managed = _TaskManagedStub(cmd=["journal", "providers"], start_time=100.0)
    queue._active["ref-1"] = managed
    monkeypatch.setattr(mod.time, "time", lambda: 112.0)

    assert queue.collect_task_status() == [
        {
            "ref": "ref-1",
            "name": "providers",
            "duration_seconds": 12,
            "max_runtime_seconds": mod.DEFAULT_TASK_MAX_RUNTIME,
            "slow": False,
            "stuck": False,
        }
    ]


def test_collect_task_status_under_cap(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    managed = _TaskManagedStub(cmd=["journal", "providers"], start_time=100.0)
    queue._active["ref-1"] = managed
    queue.set_cap("providers", 300)
    monkeypatch.setattr(mod.time, "time", lambda: 112.0)

    status = queue.collect_task_status()

    assert status[0]["max_runtime_seconds"] == 300
    assert status[0]["slow"] is False
    assert status[0]["stuck"] is False


def test_collect_task_status_slow_under_cap(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    managed = _TaskManagedStub(cmd=["journal", "providers"], start_time=100.0)
    queue._active["ref-1"] = managed
    queue.set_cap("providers", 15)
    monkeypatch.setattr(mod.time, "time", lambda: 112.0)

    status = queue.collect_task_status()

    assert status[0]["max_runtime_seconds"] == 15
    assert status[0]["slow"] is True
    assert status[0]["stuck"] is False


def test_collect_task_status_over_cap(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    managed = _TaskManagedStub(cmd=["journal", "providers"], start_time=100.0)
    queue._active["ref-1"] = managed
    queue.set_cap("providers", 5)
    monkeypatch.setattr(mod.time, "time", lambda: 112.0)

    status = queue.collect_task_status()

    assert status[0]["max_runtime_seconds"] == 5
    assert status[0]["slow"] is True
    assert status[0]["stuck"] is True


def test_collect_task_status_default_cap_stuck(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    managed = _TaskManagedStub(cmd=["journal", "providers"], start_time=100.0)
    queue._active["ref-1"] = managed
    monkeypatch.setattr(
        mod.time, "time", lambda: 100.0 + mod.DEFAULT_TASK_MAX_RUNTIME + 5
    )

    status = queue.collect_task_status()

    assert status[0]["max_runtime_seconds"] == mod.DEFAULT_TASK_MAX_RUNTIME
    assert status[0]["stuck"] is True


def test_collect_task_status_snapshots_active_under_lock():
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)

    for index in range(25):
        managed = _TaskManagedStub(cmd=["journal", "providers"], start_time=100.0)
        managed.is_running = MagicMock(side_effect=lambda: time.sleep(0.0001) or True)
        queue._active[f"ref-{index}"] = managed

    stop = threading.Event()
    thread_errors = []

    def mutate_active():
        index = 0
        try:
            while not stop.is_set():
                ref = f"bg-{index}"
                managed = _TaskManagedStub(cmd=["journal", "providers"])
                with queue._lock:
                    queue._active[ref] = managed
                time.sleep(0)
                with queue._lock:
                    queue._active.pop(ref, None)
                index += 1
        except BaseException as exc:
            thread_errors.append(exc)

    thread = threading.Thread(target=mutate_active)
    thread.start()
    try:
        for _ in range(100):
            queue.collect_task_status()
    finally:
        stop.set()
        thread.join(timeout=2)

    assert not thread.is_alive()
    assert thread_errors == []


def test_enforce_deadlines_terminates_stopped_task(caplog, monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")

    class StoppedProcess:
        def __init__(self, pid):
            self.pid = pid

        def status(self):
            return mod.psutil.STATUS_STOPPED

    monkeypatch.setattr(mod.psutil, "Process", StoppedProcess)
    terminate = MagicMock()
    monkeypatch.setattr(mod, "_start_termination_thread", terminate)
    queue = mod.TaskQueue(on_queue_change=None)
    managed = _TaskManagedStub(cmd=["sol", "import"], start_time=100.0)
    queue.set_cap("import", 300)
    queue._active["ref-1"] = managed
    caplog.set_level(logging.WARNING)

    queue.enforce_deadlines(110.0)
    terminate.assert_not_called()

    queue.enforce_deadlines(110.0)

    terminate.assert_called_once_with("ref-1", managed, timeout=2.0, reason="stopped")
    assert "stopped" in caplog.text


def test_enforce_deadlines_does_not_probe_status_under_lock(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    managed = _TaskManagedStub(cmd=["sol", "import"], start_time=100.0)
    managed.process.pid = 123
    queue._active["ref-1"] = managed
    queue.set_cap("import", 300)

    class ProbeProcess:
        def __init__(self, pid):
            assert pid == 123

        def status(self):
            acquired = queue._lock.acquire(blocking=False)
            assert acquired
            queue._lock.release()
            return mod.psutil.STATUS_RUNNING

    monkeypatch.setattr(mod.psutil, "Process", ProbeProcess)
    monkeypatch.setattr(mod, "_start_termination_thread", MagicMock())

    queue.enforce_deadlines(110.0)


def test_enforce_deadlines_skips_stopped_probe_for_new_cap_kill(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    managed = _TaskManagedStub(cmd=["sol", "import"], start_time=100.0)
    queue._active["ref-1"] = managed
    queue.set_cap("import", 50)
    probe = MagicMock()
    terminate = MagicMock()
    monkeypatch.setattr(mod.psutil, "Process", probe)
    monkeypatch.setattr(mod, "_start_termination_thread", terminate)

    queue.enforce_deadlines(200.0)

    probe.assert_not_called()
    terminate.assert_called_once_with("ref-1", managed, timeout=2.0, reason="cap")


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


def test_enforce_deadlines_terminates_uncapped_at_default(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)
    managed = _TaskManagedStub(cmd=["sol", "import"], start_time=100.0)
    queue._active["ref-1"] = managed

    def terminate_now(key, managed_arg, timeout, reason):
        assert key == "ref-1"
        assert managed_arg is managed
        assert timeout == 2.0
        assert reason == "cap"
        managed_arg.terminate(timeout=timeout)

    monkeypatch.setattr(mod, "_start_termination_thread", terminate_now)
    queue.enforce_deadlines(100.0 + mod.DEFAULT_TASK_MAX_RUNTIME + 1)

    managed.terminate.assert_called_once_with(timeout=2.0)


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


def test_start_local_server_launches_mlx_server_on_darwin(
    tmp_path, monkeypatch, capsys
):
    mod = importlib.import_module("solstone.think.supervisor")
    from solstone.think.providers import local_server, local_vulkan

    monkeypatch.setattr(sys, "platform", "darwin")
    gpu_gate = MagicMock(side_effect=AssertionError("darwin must not probe Vulkan"))
    monkeypatch.setattr(local_vulkan, "detect_gpus", gpu_gate)
    mod._SERVICE_STATE.clear()
    runtime_dir = tmp_path / "gemma4" / "variant-1120"
    written_ports = []
    spawned = []
    spawned_envs = []
    managed = _TaskManagedStub(cmd=[])
    managed.name = "mlx-vlm-server"
    managed.process.returncode = None

    plan = _mlx_launch_plan(
        mod,
        runtime_dir,
        model_id="gemma-4-26b-a4b-it-mlx-4bit",
    )
    monkeypatch.setattr(
        mod,
        "write_service_port",
        lambda service, port: written_ports.append((service, port)),
    )
    monkeypatch.setattr(local_server, "_probe_health", lambda port: ("ready", None))
    monkeypatch.setattr(mod, "_request_local_launch_plan", _native_launch_plan_for_test)

    def fake_spawn(cmd, *, ref=None, callosum=None, day=None, env=None):
        spawned.append(cmd)
        spawned_envs.append(env)
        managed.cmd = cmd
        managed.ref = ref
        return managed

    monkeypatch.setattr(mod.RunnerManagedProcess, "spawn", fake_spawn)

    result = mod.start_local_server(plan, _FakeReservation())

    assert result.status == "ready"
    assert result.reason_code == "probe-ready"
    assert result.managed is managed
    assert written_ports == []
    assert spawned == [
        [
            str(Path(sys.executable).with_name("mlx-vlm-server")),
            "--host",
            "127.0.0.1",
            "--port",
            "2468",
            "--model",
            str(runtime_dir),
        ]
    ]
    assert "0.0.0.0" not in spawned[0]
    assert "--n-gpu-layers" not in spawned[0]
    assert "-c" not in spawned[0]
    assert "--device" not in spawned[0]
    assert "Vulkan0" not in spawned[0]
    assert spawned_envs == [None]
    assert "--draft-model" not in spawned[0]
    assert "--draft-kind" not in spawned[0]
    assert mod._SERVICE_STATE["mlx-vlm-server"]["restart"] is False
    assert mod.LOCAL_MODEL_WARMING_UP_COPY in capsys.readouterr().out
    gpu_gate.assert_not_called()


def test_start_local_server_skips_when_mlx_not_installed_on_darwin(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    from solstone.think.providers import mlx_install

    monkeypatch.setattr(sys, "platform", "darwin")
    monkeypatch.setattr(
        mlx_install,
        "target_fingerprint",
        lambda: {"provider": "local", "backend": "mlx"},
    )
    monkeypatch.setattr(
        mlx_install,
        "inspect_readiness",
        lambda: _mlx_readiness(model_installed=False),
    )
    launch = MagicMock()
    monkeypatch.setattr(mod, "_launch_process", launch)

    observation = mod._observe_mlx_local_provider_truth()

    assert observation.phase == "artifact-not-ready"
    assert observation.plan is None
    launch.assert_not_called()


def test_start_local_server_skips_when_mlx_memory_blocked_on_darwin(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    from solstone.think.providers import mlx_install

    monkeypatch.setattr(sys, "platform", "darwin")
    monkeypatch.setattr(
        mlx_install,
        "target_fingerprint",
        lambda: {"provider": "local", "backend": "mlx"},
    )
    monkeypatch.setattr(
        mlx_install,
        "inspect_readiness",
        lambda: _mlx_readiness(ram_sufficient=False),
    )
    launch = MagicMock()
    monkeypatch.setattr(mod, "_launch_process", launch)

    observation = mod._observe_mlx_local_provider_truth()

    assert observation.phase == "artifact-not-ready"
    assert observation.plan is None
    launch.assert_not_called()


@pytest.mark.parametrize(
    ("vram_mib", "expected_context_tokens", "expected_parallel", "expected_cache_ram"),
    [
        (6390, None, "1", "0"),
        (16000, 32768, "2", "2048"),
    ],
)
def test_start_local_server_launches_llama_server_key_and_cmd(
    tmp_path,
    monkeypatch,
    capsys,
    vram_mib: int,
    expected_context_tokens: int | None,
    expected_parallel: str,
    expected_cache_ram: str,
):
    mod = importlib.import_module("solstone.think.supervisor")
    from solstone.think.providers import local_install, local_server, local_vulkan

    expected_context_tokens = (
        local_server.LOCAL_MIN_CONTEXT_TOKENS
        if expected_context_tokens is None
        else expected_context_tokens
    )
    expected_launched_context_tokens = expected_context_tokens * int(expected_parallel)
    mod._SERVICE_STATE.clear()
    binary = tmp_path / "llama-server"
    # ensure_artifacts_installed always resolves artifacts under the selected
    # model's directory; the spawn guard rejects anything else, so the stub must
    # return realistic in-model-dir paths.
    model_artifact_dir = local_install.model_dir(mod.LOCAL_MODEL)
    gguf = model_artifact_dir / "model.gguf"
    mmproj = model_artifact_dir / "mmproj.gguf"
    written_ports = []
    written_context_windows = []
    spawned = []
    spawned_envs = []
    managed = _TaskManagedStub(cmd=[])
    managed.name = "llama-server"
    managed.process.returncode = None
    log_path = tmp_path / "llama-server.log"
    log_path.write_text(BENIGN_LLAMA_LOAD_LOG, encoding="utf-8")
    managed.log_writer = type("LogWriter", (), {"path": log_path})()

    plan = _vulkan_launch_plan(mod, binary, gguf, mmproj, vram_mib=vram_mib)
    monkeypatch.setattr(local_vulkan, "device_local_used_mib", lambda index: 512)
    monkeypatch.setattr(
        mod,
        "write_service_port",
        lambda service, port: written_ports.append((service, port)),
    )
    monkeypatch.setattr(
        local_server,
        "write_local_context_window",
        lambda tokens: written_context_windows.append(tokens),
    )
    monkeypatch.setattr(local_server, "_probe_health", lambda port: ("ready", None))
    monkeypatch.setattr(local_server, "fetch_props", lambda port: None)
    monkeypatch.setattr(mod, "_request_local_launch_plan", _native_launch_plan_for_test)

    def fake_spawn(cmd, *, ref=None, callosum=None, day=None, env=None):
        spawned.append(cmd)
        spawned_envs.append(env)
        managed.cmd = cmd
        managed.ref = ref
        return managed

    monkeypatch.setattr(mod.RunnerManagedProcess, "spawn", fake_spawn)

    result = mod.start_local_server(plan, _FakeReservation())

    assert result.status == "ready"
    assert result.reason_code == "probe-ready"
    assert result.managed is managed
    assert written_ports == []
    assert written_context_windows == []
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
            "--n-gpu-layers",
            "999",
            "-c",
            str(expected_launched_context_tokens),
            "--parallel",
            expected_parallel,
            "--kv-unified",
            "--cache-ram",
            expected_cache_ram,
            "--no-context-shift",
            "--device",
            "Vulkan0",
            "--mmproj",
            str(mmproj),
        ]
    ]
    assert spawned_envs[0]["GGML_VK_VISIBLE_DEVICES"] == "1"
    assert "0.0.0.0" not in spawned[0]
    assert mod._SERVICE_STATE["llama-server"]["restart"] is False
    assert mod.LOCAL_MODEL_WARMING_UP_COPY in capsys.readouterr().out


def test_log_context_assertion(caplog):
    mod = importlib.import_module("solstone.think.supervisor")
    from solstone.think.providers import local_server

    def plan_for(tier):
        return mod.LocalServerLaunchPlan(
            backend="vulkan",
            desired_fingerprint_json='{"provider":"local"}',
            desired_fingerprint_sha256="fp-local",
            context_tokens=tier.context_tokens,
            parallel_slots=tier.parallel_slots,
        )

    floor = plan_for(local_server._FLOOR_TIER)
    capable = plan_for(local_server._CAPABLE_TIER)
    capable_n_ctx = capable.context_tokens * capable.parallel_slots

    with caplog.at_level(logging.INFO):
        mod._log_context_assertion(floor, 16384, 1)
    assert not any(record.levelno >= logging.WARNING for record in caplog.records)

    caplog.clear()
    with caplog.at_level(logging.INFO):
        mod._log_context_assertion(capable, capable_n_ctx, 2)
    assert not any(record.levelno >= logging.WARNING for record in caplog.records)

    caplog.clear()
    with caplog.at_level(logging.WARNING):
        mod._log_context_assertion(capable, capable_n_ctx, 1)
    assert any("context MISMATCH" in record.message for record in caplog.records)
    assert any("slots MISMATCH" in record.message for record in caplog.records)

    caplog.clear()
    with caplog.at_level(logging.INFO):
        mod._log_context_assertion(capable, capable_n_ctx, None)
    assert not any(record.levelno >= logging.WARNING for record in caplog.records)
    assert any("context OK" in record.message for record in caplog.records)
    assert any("slot count not reported" in record.message for record in caplog.records)

    caplog.clear()
    with caplog.at_level(logging.WARNING):
        mod._log_context_assertion(capable, 12345, 2)
    assert any(record.levelno == logging.WARNING for record in caplog.records)

    caplog.clear()
    with caplog.at_level(logging.INFO):
        mod._log_context_assertion(capable, None, None)
    assert not any(record.levelno >= logging.WARNING for record in caplog.records)
    assert any(
        "context assertion skipped" in record.message for record in caplog.records
    )


def test_start_local_server_skips_missing_artifacts(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")

    readiness = ReadinessOutcome(
        provider="local",
        status="missing-or-mismatched",
        reason_code="manifest_missing",
        target={},
        install={"install_state": "idle"},
        host={},
        artifacts={},
        proof={},
    )
    launch = MagicMock()
    monkeypatch.setattr(mod, "_launch_process", launch)

    observation = mod._readiness_block_observation(
        provider="local",
        readiness=readiness,
        fingerprint_json='{"provider":"local"}',
        fingerprint_sha256_value="fp-local",
        boot_required=True,
    )

    assert observation is not None
    assert observation.phase == "artifact-not-ready"
    assert observation.reason_code == "artifact-missing"
    launch.assert_not_called()


def _configure_linux_llama_start(
    mod,
    tmp_path,
    monkeypatch,
    *,
    log_text: str,
    poll_return=None,
):
    from solstone.think.providers import local_install, local_server, local_vulkan

    mod._SERVICE_STATE.clear()
    binary = tmp_path / "llama-server"
    model_artifact_dir = local_install.model_dir(mod.LOCAL_MODEL)
    gguf = model_artifact_dir / "model.gguf"
    log_path = tmp_path / "llama-server.log"
    log_path.write_text(log_text, encoding="utf-8")
    managed = _TaskManagedStub(cmd=[])
    managed.name = "llama-server"
    managed.process.returncode = poll_return
    managed.process.poll = MagicMock(return_value=poll_return)
    managed.log_writer = type("LogWriter", (), {"path": log_path})()
    spawned: list[list[str]] = []
    spawned_envs: list[dict[str, str] | None] = []
    plan = _vulkan_launch_plan(mod, binary, gguf, None, vram_mib=6390)

    monkeypatch.setattr(local_vulkan, "device_local_used_mib", lambda index: 512)
    monkeypatch.setattr(mod, "write_service_port", lambda _service, _port: None)
    monkeypatch.setattr(
        local_server, "write_local_context_window", lambda _tokens: None
    )
    monkeypatch.setattr(local_server, "_probe_health", lambda _port: ("ready", None))
    monkeypatch.setattr(local_server, "fetch_props", lambda _port: None)
    monkeypatch.setattr(mod, "_request_local_launch_plan", _native_launch_plan_for_test)

    def fake_spawn(cmd, *, ref=None, callosum=None, day=None, env=None):
        spawned.append(cmd)
        spawned_envs.append(env)
        managed.cmd = cmd
        managed.ref = ref
        return managed

    monkeypatch.setattr(mod.RunnerManagedProcess, "spawn", fake_spawn)
    return managed, spawned, spawned_envs, plan


def test_start_local_server_skips_without_hardware_gpu(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    from solstone.think.providers import local_vulkan

    launch = MagicMock()
    monkeypatch.setattr(mod, "_launch_process", launch)

    observation = mod.ProviderTruthObservation(
        provider="local",
        phase="host-blocked",
        reason_code="gpu-unavailable",
        desired_fingerprint_json='{"provider":"local"}',
        desired_fingerprint_sha256="fp-local",
        boot_required=True,
        detail={
            "readiness_status": "host-ineligible",
            "readiness_reason_code": "gpu_unavailable",
            "devices": mod._format_vulkan_devices([], local_vulkan),
        },
    )

    assert observation.phase == "host-blocked"
    assert observation.reason_code == "gpu-unavailable"
    launch.assert_not_called()


def test_start_local_server_benign_load_log_returns_ready(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    managed, _spawned, spawned_envs, plan = _configure_linux_llama_start(
        mod, tmp_path, monkeypatch, log_text=BENIGN_LLAMA_LOAD_LOG
    )

    result = mod.start_local_server(plan, _FakeReservation())

    assert result.status == "ready"
    assert result.managed is managed
    assert spawned_envs[0]["GGML_VK_VISIBLE_DEVICES"] == "1"
    managed.terminate.assert_not_called()


def test_start_local_server_process_exit_during_warmup_fails_closed(
    tmp_path, monkeypatch
):
    mod = importlib.import_module("solstone.think.supervisor")
    managed, _spawned, _envs, plan = _configure_linux_llama_start(
        mod,
        tmp_path,
        monkeypatch,
        log_text="2026-06-12T12:00:00+00:00 [llama-server:stderr] loading\n",
        poll_return=1,
    )

    result = mod.start_local_server(plan, _FakeReservation())

    assert result.status == "exited"
    assert result.reason_code == "process-exited"
    assert result.managed is managed
    managed.terminate.assert_not_called()
    managed.cleanup.assert_not_called()


def test_start_local_server_deadline_with_live_process_returns_managed(
    tmp_path, monkeypatch
):
    mod = importlib.import_module("solstone.think.supervisor")
    monkeypatch.setattr(mod, "LOCAL_SERVER_READY_TIMEOUT_S", 0.0)
    managed, _spawned, _envs, plan = _configure_linux_llama_start(
        mod,
        tmp_path,
        monkeypatch,
        log_text="2026-06-12T12:00:00+00:00 [llama-server:stderr] loading\n",
    )

    result = mod.start_local_server(plan, _FakeReservation())

    assert result.status == "warmup-timeout"
    assert result.reason_code == "warmup-timeout"
    assert result.managed is managed
    managed.terminate.assert_not_called()
    managed.cleanup.assert_not_called()


def _configure_cuda_llama_start(
    mod,
    tmp_path,
    monkeypatch,
    *,
    probe,
    ready: bool = True,
):
    from solstone.think.providers import local_install, local_server

    mod._SERVICE_STATE.clear()
    binary = tmp_path / "llama-server"
    model_artifact_dir = local_install.model_dir(mod.LOCAL_MODEL)
    gguf = model_artifact_dir / "model.gguf"
    mmproj = model_artifact_dir / "mmproj.gguf"
    lib_dir = tmp_path / "cuda-lib"
    lib_dir.mkdir()
    managed = _TaskManagedStub(cmd=[])
    managed.name = "llama-server"
    managed.process.returncode = None
    written_ports = []
    written_context_windows = []
    spawned: list[list[str]] = []
    spawned_envs: list[dict[str, str] | None] = []
    plan = _cuda_launch_plan(
        mod,
        binary,
        gguf,
        mmproj,
        lib_dir,
        tiering_memory_mib=probe.tiering_memory_mib,
        visible_device=str(probe.index if probe.index is not None else 0),
    )

    monkeypatch.setattr(
        mod,
        "write_service_port",
        lambda service, port: written_ports.append((service, port)),
    )
    monkeypatch.setattr(
        local_server,
        "write_local_context_window",
        lambda tokens: written_context_windows.append(tokens),
    )
    monkeypatch.setattr(
        local_server,
        "_probe_health",
        lambda _port: (
            (local_server.STATE_READY, None)
            if ready
            else (local_server.STATE_FAILED, "loading")
        ),
    )
    monkeypatch.setattr(local_server, "fetch_props", lambda _port: None)
    monkeypatch.setattr(mod, "_request_local_launch_plan", _native_launch_plan_for_test)

    def fake_spawn(cmd, *, ref=None, callosum=None, day=None, env=None):
        spawned.append(cmd)
        spawned_envs.append(env)
        managed.cmd = cmd
        managed.ref = ref
        return managed

    monkeypatch.setattr(mod.RunnerManagedProcess, "spawn", fake_spawn)
    return (
        managed,
        spawned,
        spawned_envs,
        written_ports,
        written_context_windows,
        lib_dir,
        plan,
    )


def test_start_local_server_cuda_launches_llama_server_cmd_and_env(
    tmp_path, monkeypatch
):
    mod = importlib.import_module("solstone.think.supervisor")
    from solstone.think.providers import local_cuda

    monkeypatch.setenv("LD_LIBRARY_PATH", "/existing/lib")
    probe = local_cuda.NvidiaProbe(
        index=0,
        compute_cap="sm_121",
        driver_cuda_version=13,
        vram_mib=20000,
        tiering_memory_mib=20000,
        memory_source=local_cuda.MEMORY_SOURCE_NVIDIA_VRAM,
        detected=True,
    )
    (
        managed,
        spawned,
        spawned_envs,
        written_ports,
        written_context_windows,
        lib_dir,
        plan,
    ) = _configure_cuda_llama_start(mod, tmp_path, monkeypatch, probe=probe)

    result = mod.start_local_server(plan, _FakeReservation())

    assert result.status == "ready"
    assert result.managed is managed
    assert written_ports == []
    assert written_context_windows == []
    assert len(spawned) == 1
    cmd = spawned[0]
    assert cmd[cmd.index("--device") + 1] == "CUDA0"
    assert cmd[cmd.index("-c") + 1] == str(plan.context_tokens * plan.parallel_slots)
    assert cmd[cmd.index("--host") + 1] == "127.0.0.1"
    assert cmd[cmd.index("--n-gpu-layers") + 1] == "999"
    assert "--mmproj" in cmd
    assert "0.0.0.0" not in cmd
    assert spawned_envs[0]["CUDA_VISIBLE_DEVICES"] == "0"
    assert spawned_envs[0]["LD_LIBRARY_PATH"] == f"{lib_dir}:/existing/lib"


def test_start_local_server_cuda_missing_vram_uses_floor_tier(
    tmp_path, monkeypatch, caplog
):
    mod = importlib.import_module("solstone.think.supervisor")
    from solstone.think.providers import local_cuda, local_server

    probe = local_cuda.NvidiaProbe(
        index=0,
        compute_cap="sm_121",
        driver_cuda_version=13,
        vram_mib=None,
        tiering_memory_mib=None,
        memory_source=local_cuda.MEMORY_SOURCE_UNAVAILABLE,
        detected=True,
    )
    (
        managed,
        spawned,
        _spawned_envs,
        _ports,
        written_context_windows,
        _lib_dir,
        plan,
    ) = _configure_cuda_llama_start(mod, tmp_path, monkeypatch, probe=probe)

    with caplog.at_level(logging.INFO):
        result = mod.start_local_server(plan, _FakeReservation())

    assert result.status == "ready"
    assert result.managed is managed
    assert written_context_windows == []
    assert spawned[0][spawned[0].index("-c") + 1] == str(
        local_server.LOCAL_MIN_CONTEXT_TOKENS
    )
    assert any(
        "local server backend=cuda" in record.message for record in caplog.records
    )


def test_start_local_server_cuda_unified_memory_uses_capable_tier(
    tmp_path, monkeypatch, caplog
):
    mod = importlib.import_module("solstone.think.supervisor")
    from solstone.think.providers import local_cuda

    probe = local_cuda.NvidiaProbe(
        index=0,
        compute_cap="sm_121",
        driver_cuda_version=13,
        vram_mib=None,
        tiering_memory_mib=26724,
        memory_source=local_cuda.MEMORY_SOURCE_SYSTEM_AVAILABLE,
        detected=True,
    )
    (
        managed,
        spawned,
        _spawned_envs,
        _ports,
        written_context_windows,
        _lib_dir,
        plan,
    ) = _configure_cuda_llama_start(mod, tmp_path, monkeypatch, probe=probe)

    with caplog.at_level(logging.INFO):
        result = mod.start_local_server(plan, _FakeReservation())

    assert result.status == "ready"
    assert result.managed is managed
    assert written_context_windows == []
    assert spawned[0][spawned[0].index("-c") + 1] == str(
        plan.context_tokens * plan.parallel_slots
    )
    assert any(
        "local server backend=cuda" in record.message for record in caplog.records
    )


def test_start_local_server_cuda_low_unified_memory_uses_floor_tier(
    tmp_path, monkeypatch
):
    mod = importlib.import_module("solstone.think.supervisor")
    from solstone.think.providers import local_cuda, local_server

    probe = local_cuda.NvidiaProbe(
        index=0,
        compute_cap="sm_121",
        driver_cuda_version=13,
        vram_mib=None,
        tiering_memory_mib=8000,
        memory_source=local_cuda.MEMORY_SOURCE_SYSTEM_AVAILABLE,
        detected=True,
    )
    (
        _managed,
        spawned,
        _spawned_envs,
        _ports,
        written_context_windows,
        _lib_dir,
        plan,
    ) = _configure_cuda_llama_start(mod, tmp_path, monkeypatch, probe=probe)

    mod.start_local_server(plan, _FakeReservation())

    assert written_context_windows == []
    assert spawned[0][spawned[0].index("-c") + 1] == str(
        local_server.LOCAL_MIN_CONTEXT_TOKENS
    )


def test_start_local_server_cuda_warmup_timeout_fails_closed(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    from solstone.think.providers import local_cuda

    monkeypatch.setattr(mod, "LOCAL_SERVER_READY_TIMEOUT_S", 0.0)
    probe = local_cuda.NvidiaProbe(
        index=0,
        compute_cap="sm_121",
        driver_cuda_version=13,
        vram_mib=20000,
        tiering_memory_mib=20000,
        memory_source=local_cuda.MEMORY_SOURCE_NVIDIA_VRAM,
        detected=True,
    )
    (
        managed,
        _spawned,
        _spawned_envs,
        _ports,
        _contexts,
        _lib_dir,
        plan,
    ) = _configure_cuda_llama_start(
        mod, tmp_path, monkeypatch, probe=probe, ready=False
    )
    terminated = []

    def terminate_managed(managed_arg, timeout, *, reason):
        terminated.append((managed_arg, timeout, reason))

    monkeypatch.setattr(mod, "_terminate_managed", terminate_managed)

    result = mod.start_local_server(plan, _FakeReservation())

    assert result.status == "warmup-timeout"
    assert result.reason_code == "warmup-timeout"
    assert result.managed is managed
    assert terminated == []
    managed.cleanup.assert_not_called()


def test_supervisor_provider_start_lifecycle_events_are_ignored():
    mod = importlib.import_module("solstone.think.supervisor")

    mod._handle_callosum_message({"tract": "supervisor", "event": "start_local"})
    mod._handle_callosum_message({"tract": "supervisor", "event": "start_parakeet"})

    assert not hasattr(mod, "_request_provider_runtime_retry")


def test_supervisor_provider_start_lifecycle_handlers_are_absent():
    mod = importlib.import_module("solstone.think.supervisor")

    assert not hasattr(mod, "_handle_supervisor_start_local")
    assert not hasattr(mod, "_handle_supervisor_start_parakeet")
    assert not hasattr(mod, "_request_local_server_start")
    assert not hasattr(mod, "_request_parakeet_server_start")


def test_handle_runner_exits_reports_llama_server_to_reconciler(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    mod._SERVICE_STATE.clear()
    mod._RESTART_POLICIES.clear()
    monkeypatch.setattr(mod.time, "time", lambda: 100.0)
    monkeypatch.setattr(mod, "shutdown_requested", False)

    managed = _TaskManagedStub(cmd=["/tmp/llama-server", "-m", "/tmp/model.gguf"])
    managed.name = "llama-server"
    managed.process.poll.return_value = 1
    managed.process.returncode = 1
    state = mod.ProviderRuntimeState("local")
    state.generation = 2
    state.desired_fingerprint = "fp-local"
    state.latest_phase = "ready"
    state.latest_plan = _vulkan_launch_plan(
        mod,
        Path("/tmp/llama-server"),
        Path("/tmp/model.gguf"),
        None,
        vram_mib=8000,
    )
    writes = []

    mod._SERVICE_STATE["llama-server"] = {
        "restart": True,
        "shutdown_timeout": 12,
    }
    monkeypatch.setattr(
        mod,
        "_provider_runtime_states",
        {
            "local": state,
            "parakeet": mod.ProviderRuntimeState("parakeet"),
        },
    )
    monkeypatch.setattr(
        mod,
        "_recovery_state",
        {
            "local": mod.ProviderRecoveryState(),
            "parakeet": mod.ProviderRecoveryState(),
        },
    )
    monkeypatch.setattr(
        mod,
        "_write_provider_runtime",
        lambda state_arg, **kwargs: writes.append((state_arg, kwargs)),
    )

    def fake_launch(name, cmd, *, restart=False, shutdown_timeout=15, ref=None):
        raise AssertionError("provider-owned exits must not use generic restart")

    monkeypatch.setattr(mod, "_launch_process", fake_launch)
    monkeypatch.setattr(mod, "_supervisor_callosum", None)

    procs = [managed]
    asyncio.run(mod.handle_runner_exits(procs))

    assert procs == []
    assert state.generation == 3
    assert state.latest_phase == "stopped"
    assert state.latest_plan is None
    assert state.next_truth_at == 0.0
    assert mod._recovery_state["local"].down_generation == 3
    assert mod._SERVICE_STATE["llama-server"] == {
        "restart": True,
        "shutdown_timeout": 12,
    }
    assert writes[-1][1]["phase"] == "stopped"
    assert writes[-1][1]["reason_code"] == "process-exited"


def test_supervisor_singleton_lock_acquired(tmp_path, monkeypatch):
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))
    from solstone.think import speakers_analyze_installation as installation

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "health").mkdir(parents=True, exist_ok=True)
    monkeypatch.setattr(sys, "argv", ["supervisor"])
    order = []

    def stop_after_lock():
        order.append("callosum")
        raise SystemExit(0)

    class FakeGeneration:
        generation_id = "test-generation"

        def release(self):
            order.append("release")

    # Skip maint discovery/subprocess runs — unrelated to lock acquisition and
    # slow enough on a fresh tmp_path to blow the 5s pytest-timeout under load.
    monkeypatch.setattr(mod, "run_pending_tasks", lambda *a, **k: [])
    monkeypatch.setattr(
        mod, "_sweep_orphaned_sol_processes", lambda *_a, **_k: order.append("sweep")
    )
    monkeypatch.setattr(
        installation,
        "enter_speakers_analyze_generation",
        lambda **_kwargs: order.append("entry") or FakeGeneration(),
    )
    monkeypatch.setattr(
        mod,
        "write_self_heartbeat",
        lambda journal: order.append("heartbeat"),
    )
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
    assert start_time == psutil.Process(os.getpid()).create_time()
    assert mod.is_supervisor_up() is True
    assert order == ["sweep", "entry", "heartbeat", "callosum", "release"]


def test_supervisor_blocks_before_callosum_on_blocking_maint_failure(
    tmp_path, monkeypatch, capsys
):
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "health").mkdir(parents=True, exist_ok=True)
    monkeypatch.setattr(sys, "argv", ["supervisor"])
    task = MaintTask(
        app="thinking",
        name="001_migrate_provider_install_state",
        script_path=Path("/dummy.py"),
        retry_on_next_start=True,
        blocks_supervisor_start=True,
    )
    state_file = tmp_path / "maint" / "thinking" / f"{task.name}.jsonl"
    result = MaintTaskResult(
        task=task,
        success=False,
        exit_code=7,
        state_file=state_file,
    )
    monkeypatch.setattr(mod, "run_pending_tasks", lambda *a, **k: [result])
    monkeypatch.setattr(mod, "_sweep_orphaned_sol_processes", lambda *_a, **_k: 0)
    start_mock = MagicMock()
    monkeypatch.setattr(mod, "start_callosum_in_process", start_mock)

    with pytest.raises(SystemExit) as exc:
        mod.main()

    assert exc.value.code == 1
    start_mock.assert_not_called()
    captured = capsys.readouterr()
    assert "thinking:001_migrate_provider_install_state" in captured.err
    assert str(state_file) in captured.err
    assert "retry-on-next-start" in captured.err


def test_supervisor_generation_entry_failure_exits_78_before_launch(
    tmp_path, monkeypatch, capsys
):
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))
    from solstone.think import speakers_analyze_installation as installation

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "health").mkdir(parents=True, exist_ok=True)
    monkeypatch.setattr(sys, "argv", ["supervisor"])
    monkeypatch.setattr(mod, "run_pending_tasks", MagicMock())
    monkeypatch.setattr(mod, "_sweep_orphaned_sol_processes", lambda *_a, **_k: 0)
    heartbeat = MagicMock()
    start_callosum = MagicMock()
    start_sense = MagicMock()
    monkeypatch.setattr(mod, "write_self_heartbeat", heartbeat)
    monkeypatch.setattr(mod, "start_callosum_in_process", start_callosum)
    monkeypatch.setattr(mod, "start_sense", start_sense)
    message = "speakers-analyze generation lease is already held"
    monkeypatch.setattr(
        installation,
        "enter_speakers_analyze_generation",
        lambda **_kwargs: (_ for _ in ()).throw(RuntimeError(message)),
    )

    with pytest.raises(SystemExit) as exc:
        mod.main()

    assert exc.value.code == 78
    captured = capsys.readouterr()
    assert captured.err.count(message) == 1
    heartbeat.assert_not_called()
    start_callosum.assert_not_called()
    start_sense.assert_not_called()


def test_supervisor_partial_startup_failure_after_entry_releases_generation(
    tmp_path, monkeypatch
):
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))
    from solstone.think import speakers_analyze_installation as installation

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "health").mkdir(parents=True, exist_ok=True)
    monkeypatch.setattr(sys, "argv", ["supervisor"])
    monkeypatch.setattr(mod, "run_pending_tasks", MagicMock())
    monkeypatch.setattr(mod, "_sweep_orphaned_sol_processes", lambda *_a, **_k: 0)
    released = []

    class FakeGeneration:
        generation_id = "test-generation"

        def release(self):
            released.append("release")

    monkeypatch.setattr(
        installation,
        "enter_speakers_analyze_generation",
        lambda **_kwargs: FakeGeneration(),
    )
    monkeypatch.setattr(
        mod,
        "write_self_heartbeat",
        lambda journal: (_ for _ in ()).throw(RuntimeError("heartbeat failed")),
    )
    start_callosum = MagicMock()
    monkeypatch.setattr(mod, "start_callosum_in_process", start_callosum)

    with pytest.raises(RuntimeError, match="heartbeat failed"):
        mod.main()

    assert released == ["release"]
    start_callosum.assert_not_called()


def test_supervisor_continues_after_successful_blocking_maint_task(
    tmp_path, monkeypatch
):
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "health").mkdir(parents=True, exist_ok=True)
    monkeypatch.setattr(sys, "argv", ["supervisor"])
    task = MaintTask(
        app="thinking",
        name="001_migrate_provider_install_state",
        script_path=Path("/dummy.py"),
        retry_on_next_start=True,
        blocks_supervisor_start=True,
    )
    result = MaintTaskResult(
        task=task,
        success=True,
        exit_code=0,
        state_file=tmp_path / "maint" / "thinking" / f"{task.name}.jsonl",
    )
    monkeypatch.setattr(mod, "run_pending_tasks", lambda *a, **k: [result])
    monkeypatch.setattr(mod, "_sweep_orphaned_sol_processes", lambda *_a, **_k: 0)

    def stop_after_callosum():
        raise SystemExit(0)

    start_mock = MagicMock(side_effect=stop_after_callosum)
    monkeypatch.setattr(mod, "start_callosum_in_process", start_mock)

    with pytest.raises(SystemExit) as exc:
        mod.main()

    assert exc.value.code == 0
    start_mock.assert_called_once()


def test_supervisor_singleton_lock_blocked(tmp_path, monkeypatch, capsys):
    import fcntl

    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))
    from solstone.think import speakers_analyze_installation as installation

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.delenv("INVOCATION_ID", raising=False)
    health_dir = tmp_path / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    lock_file = open(health_dir / "supervisor.lock", "w")
    fcntl.flock(lock_file, fcntl.LOCK_EX | fcntl.LOCK_NB)
    (health_dir / "supervisor.pid").write_text("12345")
    monkeypatch.setattr(sys, "argv", ["supervisor"])

    start_mock = MagicMock()
    entry_mock = MagicMock()
    monkeypatch.setattr(installation, "enter_speakers_analyze_generation", entry_mock)
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
    entry_mock.assert_not_called()
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


def test_is_supervisor_up_boundary_at_shared_tolerance(tmp_path, monkeypatch):
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    health_dir = tmp_path / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    (health_dir / "supervisor.pid").write_text(str(os.getpid()))
    create_time = psutil.Process(os.getpid()).create_time()
    within = create_time + mod.START_TIME_TOLERANCE_S - 0.1
    beyond = create_time + mod.START_TIME_TOLERANCE_S + 0.1

    (health_dir / "supervisor.start_time").write_text(str(within))
    assert mod.is_supervisor_up() is True

    (health_dir / "supervisor.start_time").write_text(str(beyond))
    assert mod.is_supervisor_up() is False
