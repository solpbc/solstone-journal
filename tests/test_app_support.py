# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for support app routes."""

import os
from datetime import datetime, timedelta

import pytest
from typer.testing import CliRunner

from solstone.apps.support.call import app
from solstone.apps.support.diagnostics import collect_recent_errors

runner = CliRunner()


def _health_dir(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    health_dir = tmp_path / "health"
    health_dir.mkdir()
    return health_dir


def _write_log(health_dir, name: str, lines: list[str]):
    log_path = health_dir / name
    log_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return log_path


@pytest.fixture
def support_client():
    """Create a Flask test client with support blueprint."""
    from flask import Flask

    from solstone.apps.support.routes import support_bp

    app = Flask(__name__)
    app.register_blueprint(support_bp)
    yield app.test_client()


class _TicketsClient:
    def __init__(self, tickets=None, error: Exception | None = None):
        self.tickets = tickets or []
        self.error = error

    def list_tickets(self, *, status=None):
        if self.error:
            raise self.error
        return self.tickets


def test_badge_count_enabled_empty(support_client, monkeypatch):
    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: True)
    monkeypatch.setattr(
        "solstone.apps.support.routes._get_client", lambda: _TicketsClient()
    )

    resp = support_client.get("/app/support/api/badge-count")

    assert resp.status_code == 200
    assert resp.get_json() == {"count": 0}


def test_badge_count_disabled_returns_403(support_client, monkeypatch):
    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: False)

    resp = support_client.get("/app/support/api/badge-count")

    assert resp.status_code == 403
    payload = resp.get_json()
    assert payload["error"] == "I couldn't use that feature because it isn't enabled."
    assert payload["reason_code"] == "feature_unavailable"
    assert payload["detail"] == "Support is not enabled"


def test_badge_count_error_returns_500(support_client, monkeypatch):
    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: True)
    monkeypatch.setattr(
        "solstone.apps.support.routes._get_client",
        lambda: _TicketsClient(error=RuntimeError("simulated")),
    )

    resp = support_client.get("/app/support/api/badge-count")

    assert resp.status_code == 500
    assert "error" in resp.get_json()


def test_create_ticket_accepts_error_report_contract(support_client, monkeypatch):
    captured: list[dict] = []

    def recorder(**kwargs):
        captured.append(kwargs)
        return {"id": 123, "subject": kwargs["subject"]}

    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: True)
    monkeypatch.setattr("solstone.apps.support.tools.support_create", recorder)

    resp = support_client.post(
        "/app/support/api/tickets",
        json={
            "subject": "I couldn't refresh vitals",
            "description": "owner-visible report body",
            "category": "error_report",
            "severity": "low",
            "anonymous": False,
            "auto_context": True,
            "user_context": {
                "url": "/app/home/",
                "correlation_id": "test-cid",
            },
        },
    )

    assert resp.status_code == 201
    payload = resp.get_json()
    assert isinstance(payload, dict)
    assert payload.get("id") or payload.get("ticket_id")
    assert captured == [
        {
            "subject": "I couldn't refresh vitals",
            "description": "owner-visible report body",
            "product": "solstone",
            "severity": "low",
            "category": "error_report",
            "user_context": {
                "url": "/app/home/",
                "correlation_id": "test-cid",
            },
            "auto_context": True,
            "anonymous": False,
        }
    ]


def test_feedback_anonymous_no_email_kwarg(support_client, monkeypatch):
    captured: list[dict] = []

    def recorder(**kwargs):
        captured.append(kwargs)
        return {"ok": True, "ticket_id": "t1"}

    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: True)
    monkeypatch.setattr("solstone.apps.support.tools.support_feedback", recorder)

    resp = support_client.post(
        "/app/support/api/feedback", json={"body": "hi", "anonymous": True}
    )

    assert resp.status_code == 201
    assert len(captured) == 1
    assert "user_email" not in captured[0]


def test_feedback_identified_forwards_email(support_client, monkeypatch):
    captured: list[dict] = []

    def recorder(**kwargs):
        captured.append(kwargs)
        return {"ok": True, "ticket_id": "t1"}

    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: True)
    monkeypatch.setattr("solstone.apps.support.tools.support_feedback", recorder)

    resp = support_client.post(
        "/app/support/api/feedback",
        json={"body": "hi", "anonymous": False, "user_email": "a@b.com"},
    )

    assert resp.status_code == 201
    assert len(captured) == 1
    assert captured[0]["user_email"] == "a@b.com"


def test_feedback_anonymous_drops_smuggled_email(support_client, monkeypatch):
    captured: list[dict] = []

    def recorder(**kwargs):
        captured.append(kwargs)
        return {"ok": True, "ticket_id": "t1"}

    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: True)
    monkeypatch.setattr("solstone.apps.support.tools.support_feedback", recorder)

    resp = support_client.post(
        "/app/support/api/feedback",
        json={"body": "hi", "anonymous": True, "user_email": "smug@x.com"},
    )

    assert resp.status_code == 201
    assert len(captured) == 1
    assert "user_email" not in captured[0]


def test_feedback_identified_empty_email_omits_kwarg(support_client, monkeypatch):
    captured: list[dict] = []

    def recorder(**kwargs):
        captured.append(kwargs)
        return {"ok": True, "ticket_id": "t1"}

    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: True)
    monkeypatch.setattr("solstone.apps.support.tools.support_feedback", recorder)

    resp = support_client.post(
        "/app/support/api/feedback",
        json={"body": "hi", "anonymous": False, "user_email": "   "},
    )

    assert resp.status_code == 201
    assert len(captured) == 1
    assert "user_email" not in captured[0]


def test_recent_beats_stale_under_limit(tmp_path, monkeypatch):
    health_dir = _health_dir(tmp_path, monkeypatch)
    stale = (datetime.now() - timedelta(days=30)).isoformat(timespec="seconds")
    recent = (datetime.now() - timedelta(hours=1)).isoformat(timespec="seconds")

    _write_log(
        health_dir,
        "old.log",
        [f"{stale} [old:stderr] ERROR:root:stale-{i}" for i in range(12)],
    )
    _write_log(
        health_dir,
        "new.log",
        [f"{recent} [new:stderr] ERROR:root:recent-boom"],
    )

    result = collect_recent_errors()

    assert len(result) <= 10
    assert result[0]["message"] == "[new:stderr] ERROR:root:recent-boom"
    assert any("recent-boom" in entry["message"] for entry in result)
    assert all("stale-" not in entry["message"] for entry in result)
    assert [entry["time"] for entry in result] == sorted(
        entry["time"] for entry in result
    )[::-1]


def test_line_timestamp_beats_mtime_fallback(tmp_path, monkeypatch):
    health_dir = _health_dir(tmp_path, monkeypatch)
    line_dt = datetime.now() - timedelta(hours=2)
    mtime_dt = datetime.now() - timedelta(hours=1)
    line_ts = line_dt.isoformat(timespec="seconds")
    mtime_ts = mtime_dt.isoformat(timespec="seconds")

    log_path = _write_log(
        health_dir,
        "mixed.log",
        [
            f"{line_ts} [mixed:stderr] ERROR:root:line-timestamp",
            "ERROR something with no timestamp",
        ],
    )
    os.utime(log_path, (mtime_dt.timestamp(), mtime_dt.timestamp()))

    result = collect_recent_errors()
    line_entry = next(e for e in result if "line-timestamp" in e["message"])
    fallback_entry = next(e for e in result if "no timestamp" in e["message"])

    assert line_entry["time"] == line_ts
    assert line_entry["time_approximate"] is False
    assert fallback_entry["time"] == mtime_ts
    assert fallback_entry["time_approximate"] is True


def test_window_excludes_old_and_cli_empty_state(tmp_path, monkeypatch):
    health_dir = _health_dir(tmp_path, monkeypatch)
    stale = (datetime.now() - timedelta(days=30)).isoformat(timespec="seconds")
    _write_log(
        health_dir,
        "stale.log",
        [f"{stale} [stale:stderr] ERROR:root:too-old"],
    )

    assert collect_recent_errors() == []

    result = runner.invoke(app, ["diagnose"])

    assert result.exit_code == 0
    assert "No recent errors." in result.stdout


def test_unreadable_log_degrades_gracefully(tmp_path, monkeypatch):
    health_dir = _health_dir(tmp_path, monkeypatch)
    (health_dir / "bad.log").mkdir()
    recent = (datetime.now() - timedelta(hours=1)).isoformat(timespec="seconds")
    _write_log(
        health_dir,
        "good.log",
        [f"{recent} [good:stderr] ERROR:root:survived"],
    )

    result = collect_recent_errors()

    assert any("survived" in entry["message"] for entry in result)


def test_cli_count_matches_printed_rows(tmp_path, monkeypatch):
    health_dir = _health_dir(tmp_path, monkeypatch)
    now = datetime.now()
    count = 3
    lines = [
        (
            f"{(now - timedelta(minutes=i + 1)).isoformat(timespec='seconds')} "
            f"[count:stderr] ERROR:root:count-{i}"
        )
        for i in range(count)
    ]
    _write_log(health_dir, "count.log", lines)

    result = runner.invoke(app, ["diagnose"])

    assert result.exit_code == 0
    assert f"Recent errors ({count}):" in result.stdout
    rows = [
        line
        for line in result.stdout.splitlines()
        if line.startswith("  ") and "[count]" in line and "ERROR:root:count-" in line
    ]
    assert len(rows) == count
