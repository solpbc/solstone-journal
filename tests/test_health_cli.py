# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import sys
from unittest.mock import patch

import pytest

from solstone.think import health_cli
from solstone.think.health_cli import (
    fetch_supervisor_status,
    health_check,
    main,
    print_status,
)


def test_health_check_no_socket(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    result = health_check()

    captured = capsys.readouterr()
    assert result == 1
    assert "callosum socket not found" in captured.err


def test_fetch_supervisor_status_returns_dict(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    sock = tmp_path / "health" / "callosum.sock"
    sock.parent.mkdir(parents=True)
    sock.touch()
    status = {
        "tract": "supervisor",
        "event": "status",
        "services": [{"name": "supervisor", "pid": 111, "uptime_seconds": 120}],
        "crashed": [],
        "tasks": [],
        "queues": {},
        "stale_heartbeats": [],
        "schedules": [],
        "callosum_clients": 1,
    }

    with patch("solstone.think.health_cli.CallosumConnection") as mock_conn_cls:
        mock_conn = mock_conn_cls.return_value

        def _start(*, callback):
            callback(status)

        mock_conn.start.side_effect = _start
        mock_conn.stop.return_value = None

        result = fetch_supervisor_status(timeout=health_cli.STATUS_TIMEOUT)

    assert result is status
    mock_conn.stop.assert_called_once()
    captured = capsys.readouterr()
    assert captured.out == ""
    assert captured.err == ""


def test_fetch_supervisor_status_no_socket(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    result = fetch_supervisor_status()

    assert result is None
    captured = capsys.readouterr()
    assert captured.out == ""
    assert captured.err == ""


def test_fetch_supervisor_status_timeout(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    sock = tmp_path / "health" / "callosum.sock"
    sock.parent.mkdir(parents=True)
    sock.touch()
    monkeypatch.setattr("solstone.think.health_cli.STATUS_TIMEOUT", 0.1)

    with patch("solstone.think.health_cli.CallosumConnection") as mock_conn_cls:
        mock_conn = mock_conn_cls.return_value
        mock_conn.start.return_value = None
        mock_conn.stop.return_value = None

        result = fetch_supervisor_status(timeout=health_cli.STATUS_TIMEOUT)

    assert result is None
    mock_conn.stop.assert_called_once()
    captured = capsys.readouterr()
    assert captured.out == ""
    assert captured.err == ""


def test_health_check_prints_status(capsys):
    status = {
        "services": [
            {"name": "supervisor", "pid": 1001, "uptime_seconds": 65},
            {"name": "observer", "pid": 2002, "uptime_seconds": 5},
        ],
        "crashed": [{"name": "sync", "restart_attempts": 2}],
        "tasks": [{"name": "daily", "duration_seconds": 12}],
        "queues": {"indexer": 3, "planner": 0},
        "stale_heartbeats": [],
        "callosum_clients": 5,
    }

    print_status(status)

    output = capsys.readouterr().out
    assert "Services:" in output
    assert "supervisor" in output
    assert "pid 1001" in output
    assert "observer" in output
    assert "Crashed:" in output
    assert "sync" in output
    assert "Tasks:" in output
    assert "daily" in output
    assert "queued indexer" in output
    assert "Heartbeat: ok" in output
    assert "Callosum: 5 clients" in output


def test_health_check_renders_stuck_marker(capsys):
    status = {
        "services": [],
        "tasks": [
            {
                "name": "providers",
                "duration_seconds": 313656,
                "max_runtime_seconds": 300,
                "stuck": True,
            }
        ],
        "queues": {},
    }

    print_status(status)

    output = capsys.readouterr().out
    assert "  providers        313656s  STUCK (cap 300s)" in output


def test_healthy_task_rendering_unchanged(capsys):
    status = {
        "services": [],
        "tasks": [
            {
                "name": "providers",
                "duration_seconds": 12,
                "max_runtime_seconds": 300,
                "stuck": False,
            }
        ],
        "queues": {},
    }

    print_status(status)

    output = capsys.readouterr().out
    assert "STUCK" not in output
    assert "  providers        12s" in output.splitlines()


def test_health_check_timeout(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    sock = tmp_path / "health" / "callosum.sock"
    sock.parent.mkdir(parents=True)
    sock.touch()
    monkeypatch.setattr("solstone.think.health_cli.STATUS_TIMEOUT", 0.1)

    with patch("solstone.think.health_cli.CallosumConnection") as mock_conn_cls:
        mock_conn = mock_conn_cls.return_value
        mock_conn.start.return_value = None
        mock_conn.stop.return_value = None

        result = health_check()

    captured = capsys.readouterr()
    assert result == 1
    assert "Timed out waiting for supervisor status" in captured.err
    mock_conn.stop.assert_called_once()


def test_health_check_receives_status(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    sock = tmp_path / "health" / "callosum.sock"
    sock.parent.mkdir(parents=True)
    sock.touch()

    with patch("solstone.think.health_cli.CallosumConnection") as mock_conn_cls:
        mock_conn = mock_conn_cls.return_value

        def _start(*, callback):
            callback(
                {
                    "tract": "supervisor",
                    "event": "status",
                    "services": [
                        {"name": "supervisor", "pid": 111, "uptime_seconds": 120}
                    ],
                    "tasks": [],
                    "queues": {},
                    "stale_heartbeats": [],
                }
            )

        mock_conn.start.side_effect = _start
        mock_conn.stop.return_value = None

        result = health_check()

    captured = capsys.readouterr()
    assert result == 0
    assert "Services:" in captured.out
    assert "supervisor" in captured.out


def test_main_routes_to_logs(monkeypatch):
    monkeypatch.setattr(sys, "argv", ["journal health", "logs", "--help"])

    with patch(
        "solstone.think.logs_cli.main", side_effect=SystemExit(0)
    ) as mock_logs_main:
        with pytest.raises(SystemExit):
            main()

    mock_logs_main.assert_called_once()
