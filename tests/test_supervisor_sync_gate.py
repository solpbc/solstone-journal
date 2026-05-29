# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import importlib
import json
import os
import sys
import time
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest

from solstone.think import sync_check


def _set_identity(monkeypatch):
    monkeypatch.setattr(sync_check, "_MACHINE_ID", None)
    monkeypatch.setattr(sync_check, "get_machine_id", lambda: "self-machine-1234")
    monkeypatch.setattr(sync_check, "get_self_hostname_sanitized", lambda: "self-host")
    monkeypatch.setattr(sync_check, "_solstone_version", lambda: "test-version")


def _write_foreign(journal, *, host="other-host", mtime=None, machine_id=None):
    sync_dir = journal / "health" / "sync"
    sync_dir.mkdir(parents=True, exist_ok=True)
    path = sync_dir / f"{host}.check"
    path.write_text(
        json.dumps(
            {
                "schema": sync_check.SCHEMA_VERSION,
                "machine_id": (
                    machine_id if machine_id is not None else f"{host}-machine"
                ),
                "hostname": host,
                "pid": 456,
                "wall_time": "2026-05-11T00:00:00Z",
                "solstone_version": "test-version",
                "journal_path": "/foreign/journal",
            }
        )
        + "\n",
        encoding="utf-8",
    )
    if mtime is not None:
        os.utime(path, (mtime, mtime))
    return path


def _load_supervisor(tmp_path, monkeypatch, argv=None):
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.delenv("SOL_SUPERVISOR_SPAWNED", raising=False)
    monkeypatch.setattr(sys, "argv", argv or ["supervisor"])
    monkeypatch.setattr(mod.time, "sleep", lambda _seconds: None)
    monkeypatch.setattr(mod, "run_pending_tasks", lambda *args, **kwargs: (0, 0))
    monkeypatch.setattr(mod, "_sweep_orphaned_sol_processes", lambda *_a, **_k: 0)
    return mod


def test_startup_check_clean_journal_proceeds_past_flock(tmp_path, monkeypatch):
    _set_identity(monkeypatch)
    mod = _load_supervisor(tmp_path, monkeypatch)

    def stop_after_probe():
        raise SystemExit(0)

    monkeypatch.setattr(mod, "start_callosum_in_process", stop_after_probe)

    with pytest.raises(SystemExit) as exc:
        mod.main()

    assert exc.value.code == 0
    assert (tmp_path / "health" / "supervisor.pid").read_text().strip() == str(
        os.getpid()
    )


def test_startup_check_same_machine_old_hostname_proceeds_past_flock(
    tmp_path, monkeypatch, capsys
):
    _set_identity(monkeypatch)
    _write_foreign(
        tmp_path,
        host="old-host",
        machine_id="self-machine-1234",
        mtime=time.time() - 5,
    )
    mod = _load_supervisor(tmp_path, monkeypatch)

    def stop_after_probe():
        raise SystemExit(0)

    monkeypatch.setattr(mod, "start_callosum_in_process", stop_after_probe)

    with pytest.raises(SystemExit) as exc:
        mod.main()

    assert exc.value.code == 0
    captured = capsys.readouterr()
    assert "Refusing to start" not in captured.err
    assert (tmp_path / "health" / "supervisor.pid").read_text().strip() == str(
        os.getpid()
    )


def test_startup_check_live_foreign_exits_1_prints_message_no_pid(
    tmp_path, monkeypatch, capsys
):
    _set_identity(monkeypatch)
    _write_foreign(tmp_path, mtime=time.time() - 5)
    mod = _load_supervisor(tmp_path, monkeypatch)

    with pytest.raises(SystemExit) as exc:
        mod.main()

    assert exc.value.code == 1
    captured = capsys.readouterr()
    assert "Refusing to start" in captured.err
    assert "one service per journal" in captured.err
    assert not (tmp_path / "health" / "supervisor.pid").exists()


def test_mid_run_foreign_heartbeat_sets_shutdown_emits_event_returns(
    tmp_path, monkeypatch
):
    _set_identity(monkeypatch)
    _write_foreign(tmp_path, mtime=time.time() - 5)
    mod = _load_supervisor(tmp_path, monkeypatch)
    callosum = MagicMock()
    monkeypatch.setattr(mod, "_supervisor_callosum", callosum)
    monkeypatch.setattr(mod, "_last_sync_tick", 0.0)
    monkeypatch.setattr(mod, "_last_sync_snapshot", None)
    monkeypatch.setattr(mod, "_sync_conflict_shutdown", False)
    monkeypatch.setattr(mod, "shutdown_requested", False)

    assert mod._run_sync_tick(time.time()) is False

    assert mod.shutdown_requested is True
    assert mod._sync_conflict_shutdown is True
    callosum.emit.assert_called_once()
    assert callosum.emit.call_args.args[:2] == ("supervisor", "sync_conflict")
    assert callosum.emit.call_args.kwargs["hostname"] == "other-host"


def test_finally_clears_self_heartbeat_on_normal_shutdown(tmp_path, monkeypatch):
    _set_identity(monkeypatch)
    mod = _load_supervisor(
        tmp_path,
        monkeypatch,
        [
            "supervisor",
            "0",
            "--no-daily",
            "--no-schedule",
            "--no-convey",
            "--no-cortex",
            "--no-link",
        ],
    )
    monkeypatch.setattr(mod, "start_callosum_in_process", lambda: None)
    monkeypatch.setattr(mod, "stop_callosum_in_process", lambda: None)
    monkeypatch.setattr(mod, "start_sense", lambda: SimpleNamespace(name="sense"))
    monkeypatch.setattr(mod, "_stop_process", lambda _managed: None)

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

    def interrupt_supervise(coro):
        coro.close()
        raise KeyboardInterrupt

    monkeypatch.setattr(mod.asyncio, "run", interrupt_supervise)

    try:
        mod.main()
    finally:
        os.environ.pop("SOL_SUPERVISOR_SPAWNED", None)

    assert not sync_check._self_heartbeat_path(tmp_path).exists()


def test_finally_does_not_clear_self_heartbeat_after_mid_run_conflict(
    tmp_path, monkeypatch
):
    _set_identity(monkeypatch)
    mod = _load_supervisor(
        tmp_path,
        monkeypatch,
        [
            "supervisor",
            "0",
            "--no-daily",
            "--no-schedule",
            "--no-convey",
            "--no-cortex",
            "--no-link",
        ],
    )
    monkeypatch.setattr(mod, "start_callosum_in_process", lambda: None)
    monkeypatch.setattr(mod, "stop_callosum_in_process", lambda: None)
    monkeypatch.setattr(mod, "start_sense", lambda: SimpleNamespace(name="sense"))
    monkeypatch.setattr(mod, "_stop_process", lambda _managed: None)

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

    def interrupt_supervise(coro):
        coro.close()
        mod._sync_conflict_shutdown = True
        raise KeyboardInterrupt

    monkeypatch.setattr(mod.asyncio, "run", interrupt_supervise)

    try:
        with pytest.raises(SystemExit) as exc:
            mod.main()
    finally:
        os.environ.pop("SOL_SUPERVISOR_SPAWNED", None)

    assert exc.value.code == 2
    assert sync_check._self_heartbeat_path(tmp_path).exists()
