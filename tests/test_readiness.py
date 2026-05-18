# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import logging
import os
import threading
from pathlib import Path

import psutil

from solstone.think import readiness


def _health_dir(tmp_path: Path) -> Path:
    health_dir = tmp_path / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    return health_dir


def _write_identity(
    tmp_path: Path, *, pid: int | None = None, start_time: float | None = None
) -> tuple[int, float]:
    pid = os.getpid() if pid is None else pid
    start_time = psutil.Process(pid).create_time() if start_time is None else start_time
    health_dir = _health_dir(tmp_path)
    (health_dir / "supervisor.pid").write_text(str(pid))
    (health_dir / "supervisor.start_time").write_text(str(start_time))
    return pid, start_time


def _write_marker(tmp_path: Path, payload: dict) -> None:
    marker = tmp_path / readiness.MARKER_RELATIVE_PATH
    marker.write_text(json.dumps(payload))


def test_signal_ready_writes_authoritative_marker(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _pid, start_time = _write_identity(tmp_path)

    readiness.signal_ready(
        {"pid": 1, "ready_at": 2.0, "start_time": 3.0, "caller": "kept"}
    )

    payload = json.loads((tmp_path / readiness.MARKER_RELATIVE_PATH).read_text())
    assert payload["pid"] == os.getpid()
    assert payload["start_time"] == start_time
    assert payload["ready_at"] != 2.0
    assert payload["caller"] == "kept"


def test_signal_ready_falls_back_when_start_time_missing(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(readiness.time, "time", lambda: 123.5)

    readiness.signal_ready()

    payload = json.loads((tmp_path / readiness.MARKER_RELATIVE_PATH).read_text())
    assert payload["ready_at"] == 123.5
    assert payload["start_time"] == 123.5


def test_clear_ready_removes_marker_and_tolerates_missing(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _health_dir(tmp_path)
    marker = tmp_path / readiness.MARKER_RELATIVE_PATH
    marker.write_text("{}")

    readiness.clear_ready()
    readiness.clear_ready()

    assert not marker.exists()


def test_wait_ready_returns_valid_payload(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    pid, start_time = _write_identity(tmp_path)
    _write_marker(
        tmp_path,
        {"pid": pid, "ready_at": 50.0, "start_time": start_time, "state": "ready"},
    )

    payload = readiness.wait_ready(timeout=0.1)

    assert payload is not None
    assert payload["state"] == "ready"


def test_wait_ready_times_out_without_marker(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_identity(tmp_path)

    assert readiness.wait_ready(timeout=0.1) is None


def test_wait_ready_ignores_malformed_marker(tmp_path, monkeypatch, caplog):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_identity(tmp_path)
    marker = tmp_path / readiness.MARKER_RELATIVE_PATH
    marker.write_text("{")

    with caplog.at_level(logging.WARNING):
        assert readiness.wait_ready(timeout=0.1) is None

    assert "Readiness marker" in caplog.text


def test_wait_ready_ignores_pid_mismatch(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    pid, start_time = _write_identity(tmp_path)
    _write_marker(
        tmp_path, {"pid": pid + 1, "ready_at": 50.0, "start_time": start_time}
    )

    assert readiness.wait_ready(timeout=0.1) is None


def test_wait_ready_rejects_reused_pid(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    pid, start_time = _write_identity(tmp_path)
    _write_marker(tmp_path, {"pid": pid, "ready_at": 50.0, "start_time": start_time})

    class FakeProcess:
        def __init__(self, process_pid: int) -> None:
            assert process_pid == pid

        def create_time(self) -> float:
            return start_time + 10.0

    monkeypatch.setattr(readiness.psutil, "Process", FakeProcess)

    assert readiness.wait_ready(timeout=0.1) is None


def test_wait_ready_observes_marker_written_mid_flight(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(readiness, "_POLL_INTERVAL_S", 0.02)
    _write_identity(tmp_path)
    timer = threading.Timer(
        0.02, readiness.signal_ready, kwargs={"payload": {"stage": "ready"}}
    )

    try:
        timer.start()
        payload = readiness.wait_ready(timeout=1.0)
    finally:
        timer.cancel()
        timer.join()

    assert payload is not None
    assert payload["stage"] == "ready"
