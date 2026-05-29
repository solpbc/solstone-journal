# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import logging
import os

from solstone.think import supervisor


def _touch_marker(journal, mtime):
    marker = journal / "health" / "local-server.last-use"
    marker.parent.mkdir(parents=True, exist_ok=True)
    marker.touch()
    os.utime(marker, (mtime, mtime))
    return marker


def test_stale_marker_evicts_local_server(monkeypatch, tmp_path, caplog):
    terminated = []
    monkeypatch.setattr(
        supervisor, "_find_local_server_pids", lambda journal: [123, 456]
    )
    monkeypatch.setattr(
        supervisor,
        "_terminate_pids",
        lambda pids, grace: terminated.append((pids, grace)) or len(pids),
    )
    monkeypatch.setattr(
        supervisor,
        "get_config",
        lambda: {"providers": {"local": {"idle_timeout_s": 300}}},
    )
    monkeypatch.setattr(supervisor.time, "time", lambda: 1_000.0)
    _touch_marker(tmp_path, 600.0)
    caplog.set_level(logging.INFO)

    supervisor._check_local_server_idle(tmp_path, grace=1.5)

    assert terminated == [([123, 456], 1.5)]
    assert "local server eviction: pid=123 reason=idle_timeout" in caplog.text
    assert "local server eviction: pid=456 reason=idle_timeout" in caplog.text


def test_fresh_marker_does_not_evict_local_server(monkeypatch, tmp_path):
    monkeypatch.setattr(supervisor, "_find_local_server_pids", lambda journal: [123])
    monkeypatch.setattr(
        supervisor,
        "_terminate_pids",
        lambda pids, grace: (_ for _ in ()).throw(AssertionError("unexpected evict")),
    )
    monkeypatch.setattr(
        supervisor,
        "get_config",
        lambda: {"providers": {"local": {"idle_timeout_s": 300}}},
    )
    monkeypatch.setattr(supervisor.time, "time", lambda: 1_000.0)
    _touch_marker(tmp_path, 800.0)

    supervisor._check_local_server_idle(tmp_path)


def test_missing_marker_does_not_evict_local_server(monkeypatch, tmp_path):
    monkeypatch.setattr(supervisor, "_find_local_server_pids", lambda journal: [123])
    monkeypatch.setattr(
        supervisor,
        "_terminate_pids",
        lambda pids, grace: (_ for _ in ()).throw(AssertionError("unexpected evict")),
    )
    monkeypatch.setattr(
        supervisor,
        "get_config",
        lambda: {"providers": {"local": {"idle_timeout_s": 300}}},
    )

    supervisor._check_local_server_idle(tmp_path)
