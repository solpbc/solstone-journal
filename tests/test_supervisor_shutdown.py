# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for handle_shutdown's reap pass."""

from pathlib import Path
from unittest.mock import MagicMock

import pytest

import solstone.think.supervisor as supervisor


class FakeManaged:
    def __init__(self, name, exits_after_terminate=True):
        self.name = name
        self.process = MagicMock()
        self.process.pid = 12345
        self._running = True
        self._exits_after_terminate = exits_after_terminate
        self.process.terminate.side_effect = self._on_terminate
        self.process.kill.side_effect = self._on_kill

    def is_running(self):
        return self._running

    def _on_terminate(self):
        if self._exits_after_terminate:
            self._running = False

    def _on_kill(self):
        self._running = False


@pytest.fixture(autouse=True)
def _disable_llama_reap(monkeypatch):
    monkeypatch.setattr(supervisor, "_find_local_server_pids", lambda _journal: [])


def test_reap_terminates_and_kills_survivor(monkeypatch):
    well_behaved = FakeManaged("well", exits_after_terminate=True)
    stuck = FakeManaged("stuck", exits_after_terminate=False)
    monkeypatch.setattr(supervisor, "_managed_procs", [well_behaved, stuck])
    monkeypatch.setattr(supervisor, "shutdown_requested", False)
    times = iter([0.0, 3.5])
    monkeypatch.setattr(supervisor.time, "monotonic", lambda: next(times))
    monkeypatch.setattr(supervisor.time, "sleep", lambda _seconds: None)

    with pytest.raises(KeyboardInterrupt):
        supervisor.handle_shutdown(15, None)

    assert well_behaved.process.terminate.called
    assert stuck.process.terminate.called
    assert not well_behaved.process.kill.called
    assert stuck.process.kill.called


def test_reap_idempotent_on_second_call(monkeypatch):
    proc = FakeManaged("svc", exits_after_terminate=True)
    monkeypatch.setattr(supervisor, "_managed_procs", [proc])
    monkeypatch.setattr(supervisor, "shutdown_requested", False)
    monkeypatch.setattr(supervisor.time, "monotonic", lambda: 0.0)
    monkeypatch.setattr(supervisor.time, "sleep", lambda _seconds: None)

    with pytest.raises(KeyboardInterrupt):
        supervisor.handle_shutdown(15, None)
    assert proc.process.terminate.call_count == 1

    supervisor.handle_shutdown(15, None)

    assert proc.process.terminate.call_count == 1


def test_reap_empty_managed_procs(monkeypatch):
    monkeypatch.setattr(supervisor, "_managed_procs", [])
    monkeypatch.setattr(supervisor, "shutdown_requested", False)

    with pytest.raises(KeyboardInterrupt):
        supervisor.handle_shutdown(15, None)


def test_reap_swallows_oserror_on_kill(monkeypatch, caplog):
    bad = FakeManaged("bad", exits_after_terminate=False)
    bad.process.kill.side_effect = OSError("permission denied")
    monkeypatch.setattr(supervisor, "_managed_procs", [bad])
    monkeypatch.setattr(supervisor, "shutdown_requested", False)
    times = iter([0.0, 3.5])
    monkeypatch.setattr(supervisor.time, "monotonic", lambda: next(times))
    monkeypatch.setattr(supervisor.time, "sleep", lambda _seconds: None)
    caplog.set_level("ERROR")

    with pytest.raises(KeyboardInterrupt):
        supervisor.handle_shutdown(15, None)

    assert "shutdown: kill failed for bad" in caplog.text


def test_reap_terminates_local_server_after_managed_children(monkeypatch):
    proc = FakeManaged("svc", exits_after_terminate=True)
    order = []
    original_terminate = proc.process.terminate.side_effect

    def terminate_managed():
        order.append("managed")
        original_terminate()

    def find_local_servers(journal):
        order.append(("find", journal))
        return [444, 555]

    def terminate_pids(pids, grace):
        order.append(("llama", pids, grace))
        return len(pids)

    proc.process.terminate.side_effect = terminate_managed
    monkeypatch.setattr(supervisor, "_managed_procs", [proc])
    monkeypatch.setattr(supervisor, "shutdown_requested", False)
    monkeypatch.setattr(supervisor, "get_journal", lambda: "/journal/test")
    monkeypatch.setattr(supervisor, "_find_local_server_pids", find_local_servers)
    monkeypatch.setattr(supervisor, "_terminate_pids", terminate_pids)

    with pytest.raises(KeyboardInterrupt):
        supervisor.handle_shutdown(15, None)

    assert order == [
        "managed",
        ("find", Path("/journal/test").resolve()),
        ("llama", [444, 555], supervisor.LOCAL_SERVER_TERMINATE_GRACE_S),
    ]

    supervisor.handle_shutdown(15, None)

    assert order == [
        "managed",
        ("find", Path("/journal/test").resolve()),
        ("llama", [444, 555], supervisor.LOCAL_SERVER_TERMINATE_GRACE_S),
    ]
