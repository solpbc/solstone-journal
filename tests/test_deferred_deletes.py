# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import threading
import time

import pytest

import solstone.think.deferred_deletes as deferred_deletes


@pytest.fixture(autouse=True)
def cleanup_deferred_deletes():
    yield
    for pending_id in list(deferred_deletes._TIMERS):
        deferred_deletes.cancel(pending_id)


def test_schedule_runs_commit_once_after_ttl():
    committed = []
    done = threading.Event()

    def commit():
        committed.append("ran")
        done.set()

    deferred_deletes.schedule(commit, ttl_seconds=0.05)

    assert done.wait(0.3)
    assert committed == ["ran"]


def test_cancel_prevents_commit():
    committed = []

    deferred_id = deferred_deletes.schedule(
        lambda: committed.append("ran"),
        ttl_seconds=1.0,
    )

    assert deferred_deletes.cancel(deferred_id) is True
    time.sleep(1.15)
    assert committed == []


def test_double_cancel_returns_false_after_first():
    deferred_id = deferred_deletes.schedule(lambda: None, ttl_seconds=1.0)

    assert deferred_deletes.cancel(deferred_id) is True
    assert deferred_deletes.cancel(deferred_id) is False


def test_cancel_unknown_id_returns_false():
    assert deferred_deletes.cancel("0" * 32) is False


def test_cancel_commit_race_runs_at_most_once():
    iterations = 50

    for _ in range(iterations):
        commit_count = 0
        commit_lock = threading.Lock()
        commit_event = threading.Event()
        start = threading.Event()
        cancel_results = []

        def commit():
            nonlocal commit_count
            with commit_lock:
                commit_count += 1
            commit_event.set()

        deferred_id = deferred_deletes.schedule(commit, ttl_seconds=0.05)
        with deferred_deletes._LOCK:
            timer = deferred_deletes._TIMERS[deferred_id]

        def attempt_cancel():
            start.wait()
            cancel_results.append(deferred_deletes.cancel(deferred_id))

        threads = [threading.Thread(target=attempt_cancel) for _ in range(8)]
        for thread in threads:
            thread.start()

        start.set()
        for thread in threads:
            thread.join()

        true_cancels = sum(cancel_results)
        if true_cancels:
            timer.join(timeout=0.1)
            assert not timer.is_alive()
            assert not commit_event.is_set()
        else:
            assert commit_event.wait(0.1)
        assert commit_count in (0, 1)
        assert not (true_cancels and commit_count)
        assert (true_cancels == 1 and commit_count == 0) or (
            true_cancels == 0 and commit_count == 1
        )


def test_scheduled_timers_are_daemon_threads():
    deferred_id = deferred_deletes.schedule(lambda: None, ttl_seconds=1.0)

    with deferred_deletes._LOCK:
        timer = deferred_deletes._TIMERS[deferred_id]

    assert timer.daemon is True
