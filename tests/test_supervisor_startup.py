# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import importlib
import itertools
from types import SimpleNamespace
from unittest import mock

import pytest

import solstone.think.utils as utils


def test_task_queue_defers_submit_when_not_ready(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None, ready=False)

    started = []

    def fake_thread_start(self):
        started.append(self._args)

    monkeypatch.setattr(mod.threading.Thread, "start", fake_thread_start)

    ref = queue.submit(
        ["sol", "indexer", "--rescan"], ref="pending-ref", day="20260418"
    )

    assert ref == "pending-ref"
    assert started == []
    assert queue._pending == [
        {
            "refs": ["pending-ref"],
            "cmd": ["sol", "indexer", "--rescan"],
            "day": "20260418",
            "scheduler_name": None,
        }
    ]
    assert queue.collect_queue_counts() == {"pending": 1}


def test_task_queue_set_ready_drains_in_submission_order(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None, ready=False)

    started = []

    def fake_thread_start(self):
        started.append(self._args)

    monkeypatch.setattr(mod.threading.Thread, "start", fake_thread_start)

    queue.submit(["sol", "indexer", "--rescan"], ref="ref-1")
    queue.submit(["sol", "insight", "20260418"], ref="ref-2")
    queue.submit(["sol", "heartbeat"], ref="ref-3")

    queue.set_ready()

    assert [args[0] for args in started] == [["ref-1"], ["ref-2"], ["ref-3"]]
    assert [args[1] for args in started] == [
        ["sol", "indexer", "--rescan"],
        ["sol", "insight", "20260418"],
        ["sol", "heartbeat"],
    ]
    assert queue._pending == []


def test_task_queue_set_ready_dedupes_same_cmd_in_pending(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None, ready=False)

    started = []

    def fake_thread_start(self):
        started.append(self._args)

    monkeypatch.setattr(mod.threading.Thread, "start", fake_thread_start)

    queue.submit(["sol", "indexer", "--rescan"], ref="ref-1")
    queue.submit(["sol", "indexer", "--rescan"], ref="ref-2")

    queue.set_ready()

    assert len(started) == 1
    assert started[0][0] == ["ref-1"]
    assert queue._queues["indexer"] == [
        {
            "refs": ["ref-2"],
            "cmd": ["sol", "indexer", "--rescan"],
            "day": None,
            "scheduler_name": None,
        }
    ]


def test_task_queue_ready_true_default_dispatches_immediately(monkeypatch):
    mod = importlib.import_module("solstone.think.supervisor")
    queue = mod.TaskQueue(on_queue_change=None)

    started = []

    def fake_thread_start(self):
        started.append(self._args)

    monkeypatch.setattr(mod.threading.Thread, "start", fake_thread_start)

    ref = queue.submit(["sol", "indexer", "--rescan"], ref="ready-ref")

    assert ref == "ready-ref"
    assert len(started) == 1
    assert started[0][0] == ["ready-ref"]
    assert queue._pending == []


def test_wait_for_convey_ready_success(caplog):
    mod = importlib.import_module("solstone.think.supervisor")
    caplog.set_level("INFO")
    convey_mp = SimpleNamespace(process=SimpleNamespace(poll=lambda: None))

    with mock.patch(
        "solstone.think.supervisor.is_solstone_up",
        side_effect=[False, False, True],
    ) as probe:
        assert mod.wait_for_convey_ready(convey_mp, timeout=1.0, interval=0.001) is True

    assert probe.call_count == 3
    assert "Convey ready after" in caplog.text


def test_wait_for_convey_ready_timeout(caplog):
    mod = importlib.import_module("solstone.think.supervisor")
    caplog.set_level("ERROR")
    convey_mp = SimpleNamespace(process=SimpleNamespace(poll=lambda: None))
    ticks = itertools.chain([0.0, 0.0, 0.1, 0.2, 0.3], itertools.repeat(0.35))

    with mock.patch("solstone.think.supervisor.is_solstone_up", return_value=False):
        with mock.patch(
            "solstone.think.supervisor.read_service_port", return_value=5015
        ):
            with mock.patch("solstone.think.supervisor.time.sleep", return_value=None):
                with mock.patch(
                    "solstone.think.supervisor.time.monotonic",
                    side_effect=lambda: next(ticks),
                ):
                    assert (
                        mod.wait_for_convey_ready(
                            convey_mp,
                            timeout=0.3,
                            interval=0.05,
                        )
                        is False
                    )

    assert "Convey not ready after" in caplog.text


def test_wait_for_convey_ready_convey_died(caplog):
    mod = importlib.import_module("solstone.think.supervisor")
    caplog.set_level("ERROR")
    convey_mp = SimpleNamespace(process=SimpleNamespace(poll=lambda: -11))

    with mock.patch("solstone.think.supervisor.is_solstone_up") as probe:
        assert (
            mod.wait_for_convey_ready(convey_mp, timeout=1.0, interval=0.001) is False
        )

    probe.assert_not_called()
    assert "Convey process exited during startup" in caplog.text


def test_require_solstone_tempfail_when_supervisor_spawned(monkeypatch, capsys):
    monkeypatch.delenv("SOL_SKIP_SUPERVISOR_CHECK", raising=False)
    monkeypatch.setenv("SOL_SUPERVISOR_SPAWNED", "1")

    with mock.patch("solstone.think.utils.is_solstone_up", return_value=False):
        with pytest.raises(SystemExit) as exc_info:
            utils.require_solstone()

    assert exc_info.value.code == utils.EXIT_TEMPFAIL
    assert capsys.readouterr().err == ""


def test_require_solstone_exit1_when_not_supervisor_spawned(monkeypatch, capsys):
    monkeypatch.delenv("SOL_SKIP_SUPERVISOR_CHECK", raising=False)
    monkeypatch.delenv("SOL_SUPERVISOR_SPAWNED", raising=False)

    with mock.patch("solstone.think.utils.is_solstone_up", return_value=False):
        with pytest.raises(SystemExit) as exc_info:
            utils.require_solstone()

    assert exc_info.value.code == 1
    assert (
        capsys.readouterr().err
        == "sol: solstone isn't running. Start it with 'journal up' and retry.\n"
    )


def test_require_solstone_skip_env_still_honored(monkeypatch):
    monkeypatch.setenv("SOL_SKIP_SUPERVISOR_CHECK", "1")
    monkeypatch.delenv("SOL_SUPERVISOR_SPAWNED", raising=False)

    with mock.patch(
        "solstone.think.utils.is_solstone_up",
        side_effect=AssertionError("should not run"),
    ):
        assert utils.require_solstone() is None


def test_startup_submits_digest_once():
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))
    submit = mock.Mock()

    mod._task_queue = SimpleNamespace(submit=submit)
    mod._is_remote_mode = False
    mod._digest_submitted_this_boot = False

    mod._maybe_submit_startup_digest(no_cortex=False)
    mod._maybe_submit_startup_digest(no_cortex=False)

    submit.assert_called_once_with(["sol", "call", "identity", "digest"])
    assert mod._digest_submitted_this_boot is True


def test_startup_skips_digest_when_no_cortex():
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))
    submit = mock.Mock()

    mod._task_queue = SimpleNamespace(submit=submit)
    mod._is_remote_mode = False
    mod._digest_submitted_this_boot = False

    mod._maybe_submit_startup_digest(no_cortex=True)

    submit.assert_not_called()
    assert mod._digest_submitted_this_boot is False
