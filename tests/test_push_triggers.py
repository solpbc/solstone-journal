# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import re
import socket
from datetime import datetime
from pathlib import Path
from urllib.error import HTTPError

from solstone.convey.sol_initiated.copy import (
    KIND_OWNER_CHAT_DISMISSED,
    KIND_OWNER_CHAT_OPEN,
    KIND_SOL_CHAT_REQUEST,
)
from solstone.think.push import portal_dispatch, triggers


def _log_path(tmp_path: Path) -> Path:
    return tmp_path / "push" / "nudge_log.jsonl"


def _read_log(tmp_path: Path) -> list[dict[str, object]]:
    return [
        json.loads(line)
        for line in _log_path(tmp_path).read_text(encoding="utf-8").splitlines()
    ]


def test_handle_briefing_finish_polls_until_briefing_exists(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    responses = iter(
        [
            ({}, None, []),
            ({}, None, []),
            (
                {"needs_attention": "- item"},
                {"generated": "2026-04-19T06:45:00"},
                ["one"],
            ),
        ]
    )
    sent_calls: list[dict[str, object]] = []
    monkeypatch.setattr(triggers, "_load_briefing_md", lambda today: next(responses))
    monkeypatch.setattr(triggers.time, "sleep", lambda seconds: None)
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: [{"token": "a" * 64}])
    monkeypatch.setattr(
        triggers,
        "send_many",
        lambda devices, payload, *, collapse_id: (
            sent_calls.append(
                {"devices": devices, "payload": payload, "collapse_id": collapse_id}
            )
            or (1, 0)
        ),
    )

    triggers.handle_briefing_finish(
        {"tract": "cortex", "event": "finish", "name": "morning_briefing"}
    )

    assert len(sent_calls) == 1
    assert sent_calls[0]["collapse_id"].startswith("briefing.")
    log_lines = _log_path(tmp_path).read_text(encoding="utf-8").splitlines()
    assert len(log_lines) == 1


def test_handle_briefing_finish_is_idempotent(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    sent_calls: list[str] = []
    monkeypatch.setattr(
        triggers,
        "_load_briefing_md",
        lambda today: (
            {"needs_attention": "- item"},
            {"generated": "2026-04-19T06:45:00"},
            ["one"],
        ),
    )
    monkeypatch.setattr(triggers.time, "sleep", lambda seconds: None)
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: [{"token": "a" * 64}])
    monkeypatch.setattr(
        triggers,
        "send_many",
        lambda devices, payload, *, collapse_id: (
            sent_calls.append(collapse_id) or (1, 0)
        ),
    )

    message = {"tract": "cortex", "event": "finish", "name": "morning_briefing"}
    triggers.handle_briefing_finish(message)
    triggers.handle_briefing_finish(message)

    assert sent_calls == [sent_calls[0]]


def test_check_pre_meeting_prep_skips_muted_facets(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: [{"token": "a" * 64}])
    monkeypatch.setattr(triggers, "get_enabled_facets", lambda: {})
    sent_calls: list[str] = []
    monkeypatch.setattr(
        triggers,
        "send_many",
        lambda devices, payload, *, collapse_id: (
            sent_calls.append(collapse_id) or (1, 0)
        ),
    )

    triggers.check_pre_meeting_prep(datetime(2026, 4, 20, 8, 45, 0))

    assert sent_calls == []


def test_check_pre_meeting_prep_skips_non_anticipated(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: [{"token": "a" * 64}])
    monkeypatch.setattr(triggers, "get_enabled_facets", lambda: {"work": {}})
    monkeypatch.setattr(
        triggers,
        "load_activity_records",
        lambda facet, day: [{"id": "meeting", "source": "cogitate", "start": "09:00"}],
    )
    sent_calls: list[str] = []
    monkeypatch.setattr(
        triggers,
        "send_many",
        lambda devices, payload, *, collapse_id: (
            sent_calls.append(collapse_id) or (1, 0)
        ),
    )

    triggers.check_pre_meeting_prep(datetime(2026, 4, 20, 8, 45, 0))

    assert sent_calls == []


def test_check_pre_meeting_prep_fires_for_hhmm_and_hhmmss(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: [{"token": "a" * 64}])
    monkeypatch.setattr(triggers, "get_enabled_facets", lambda: {"work": {}})
    monkeypatch.setattr(
        triggers,
        "load_activity_records",
        lambda facet, day: [
            {
                "id": "anticipated_meeting_090000_0420",
                "source": "anticipated",
                "start": "09:00",
                "title": "Launch sync",
            },
            {
                "id": "anticipated_call_090030_0420",
                "source": "anticipated",
                "start": "09:00:30",
                "title": "Prep call",
            },
        ],
    )
    sent_calls: list[str] = []
    monkeypatch.setattr(
        triggers,
        "send_many",
        lambda devices, payload, *, collapse_id: (
            sent_calls.append(collapse_id) or (1, 0)
        ),
    )

    triggers.check_pre_meeting_prep(datetime(2026, 4, 20, 8, 45, 0))

    assert sent_calls == [
        "meeting.anticipated_meeting_090000_0420",
        "meeting.anticipated_call_090030_0420",
    ]


def test_check_pre_meeting_prep_zero_devices_skips_log(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: [])
    monkeypatch.setattr(triggers, "get_enabled_facets", lambda: {"work": {}})
    monkeypatch.setattr(
        triggers,
        "load_activity_records",
        lambda facet, day: [
            {
                "id": "anticipated_meeting_090000_0420",
                "source": "anticipated",
                "start": "09:00",
                "title": "Launch sync",
            }
        ],
    )

    triggers.check_pre_meeting_prep(datetime(2026, 4, 20, 8, 45, 0))

    assert _log_path(tmp_path).exists() is False


def test_send_agent_alert_same_context_id_fires_once(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: [{"token": "a" * 64}])
    sent_calls: list[dict[str, object]] = []
    monkeypatch.setattr(
        triggers,
        "send_many",
        lambda devices, payload, *, collapse_id: (
            sent_calls.append({"collapse_id": collapse_id, "payload": payload})
            or (1, 0)
        ),
    )

    first = triggers.send_agent_alert(
        title="Agent Alert", body="Needs review", context_id="ctx-1"
    )
    second = triggers.send_agent_alert(
        title="Agent Alert", body="Needs review", context_id="ctx-1"
    )

    assert first == (1, 0)
    assert second == (0, 0)
    assert sent_calls == [
        {
            "collapse_id": "alert.ctx-1",
            "payload": {
                "aps": {
                    "alert": {"title": "Agent Alert", "body": "Needs review"},
                    "category": "SOLSTONE_AGENT_ALERT",
                    "sound": "default",
                    "mutable-content": 1,
                    "content-available": 1,
                },
                "data": {"action": "open_alert", "context_id": "ctx-1"},
            },
        }
    ]
    lines = [
        json.loads(line)
        for line in _log_path(tmp_path).read_text(encoding="utf-8").splitlines()
    ]
    assert len(lines) == 1


def test_send_agent_alert_forwards_route(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: [{"token": "a" * 64}])
    payloads: list[dict[str, object]] = []
    monkeypatch.setattr(
        triggers,
        "send_many",
        lambda devices, payload, *, collapse_id: payloads.append(payload) or (1, 0),
    )

    sent, failed = triggers.send_agent_alert(
        title="Agent Alert",
        body="Needs review",
        context_id="ctx-2",
        route="/app/reflections/20260308",
    )

    assert (sent, failed) == (1, 0)
    assert payloads[0]["data"]["route"] == "/app/reflections/20260308"


def test_handle_weekly_reflection_finish_sends_once_and_appends_chat_event(
    monkeypatch, tmp_path
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    reflection_path = tmp_path / "reflections" / "weekly" / "20260308.md"
    reflection_path.parent.mkdir(parents=True, exist_ok=True)
    reflection_path.write_text("# reflection\n", encoding="utf-8")
    monkeypatch.setattr(triggers.time, "sleep", lambda seconds: None)
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: [{"token": "a" * 64}])
    sent_calls: list[dict[str, object]] = []
    chat_events: list[dict[str, str]] = []
    monkeypatch.setattr(
        triggers,
        "send_many",
        lambda devices, payload, *, collapse_id: (
            sent_calls.append({"payload": payload, "collapse_id": collapse_id})
            or (1, 0)
        ),
    )
    monkeypatch.setattr(
        triggers,
        "append_chat_event",
        lambda kind, **fields: chat_events.append({"kind": kind, **fields}),
    )

    message = {
        "tract": "cortex",
        "event": "finish",
        "name": "weekly_reflection",
        "day": "20260308",
    }
    triggers.handle_weekly_reflection_finish(message)
    triggers.handle_weekly_reflection_finish(message)

    assert sent_calls == [
        {
            "payload": {
                "aps": {
                    "alert": {"title": "your week is ready", "body": ""},
                    "category": "SOLSTONE_AGENT_ALERT",
                    "sound": "default",
                    "mutable-content": 1,
                    "content-available": 1,
                },
                "data": {
                    "action": "open_alert",
                    "context_id": "weekly_reflection:20260308",
                    "route": "/app/reflections/20260308",
                },
            },
            "collapse_id": "alert.weekly_reflection:20260308",
        }
    ]
    assert chat_events == [
        {
            "kind": "reflection_ready",
            "day": "20260308",
            "url": "/app/reflections/20260308",
        }
    ]
    lines = [
        json.loads(line)
        for line in _log_path(tmp_path).read_text(encoding="utf-8").splitlines()
    ]
    assert len(lines) == 1
    assert lines[0]["category"] == "SOLSTONE_AGENT_ALERT"
    assert lines[0]["context_id"] == "weekly_reflection:20260308"


def test_handle_weekly_reflection_finish_ignores_unrelated_events(
    monkeypatch, tmp_path
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    send_calls: list[tuple] = []
    monkeypatch.setattr(
        triggers,
        "send_agent_alert",
        lambda **kwargs: send_calls.append(tuple(sorted(kwargs.items()))) or (1, 0),
    )
    monkeypatch.setattr(
        triggers,
        "append_chat_event",
        lambda kind, **fields: send_calls.append(("chat", kind, fields)),
    )

    triggers.handle_weekly_reflection_finish(
        {
            "tract": "chat",
            "event": "finish",
            "name": "weekly_reflection",
            "day": "20260308",
        }
    )
    triggers.handle_weekly_reflection_finish(
        {
            "tract": "cortex",
            "event": "start",
            "name": "weekly_reflection",
            "day": "20260308",
        }
    )
    triggers.handle_weekly_reflection_finish(
        {
            "tract": "cortex",
            "event": "finish",
            "name": "morning_briefing",
            "day": "20260308",
        }
    )

    assert send_calls == []


def test_handle_weekly_reflection_finish_skips_when_file_never_appears(
    monkeypatch, tmp_path
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    sleeps: list[int] = []
    send_calls: list[dict[str, object]] = []
    chat_events: list[dict[str, object]] = []
    monkeypatch.setattr(triggers.time, "sleep", lambda seconds: sleeps.append(seconds))
    monkeypatch.setattr(
        triggers,
        "send_agent_alert",
        lambda **kwargs: send_calls.append(kwargs) or (1, 0),
    )
    monkeypatch.setattr(
        triggers,
        "append_chat_event",
        lambda kind, **fields: chat_events.append({"kind": kind, **fields}),
    )

    triggers.handle_weekly_reflection_finish(
        {
            "tract": "cortex",
            "event": "finish",
            "name": "weekly_reflection",
            "day": "20260308",
        }
    )

    assert sleeps == [1] * 10
    assert send_calls == []
    assert chat_events == []


def test_handle_weekly_reflection_finish_dedupes_chat_event_without_devices(
    monkeypatch, tmp_path
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    reflection_path = tmp_path / "reflections" / "weekly" / "20260308.md"
    reflection_path.parent.mkdir(parents=True, exist_ok=True)
    reflection_path.write_text("# reflection\n", encoding="utf-8")
    monkeypatch.setattr(triggers.time, "sleep", lambda seconds: None)
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: [])
    chat_events: list[dict[str, object]] = []
    monkeypatch.setattr(triggers, "read_chat_events", lambda day: list(chat_events))
    monkeypatch.setattr(
        triggers,
        "append_chat_event",
        lambda kind, **fields: chat_events.append({"kind": kind, **fields}),
    )

    message = {
        "tract": "cortex",
        "event": "finish",
        "name": "weekly_reflection",
        "day": "20260308",
    }
    triggers.handle_weekly_reflection_finish(message)
    triggers.handle_weekly_reflection_finish(message)

    assert chat_events == [
        {
            "kind": "reflection_ready",
            "day": "20260308",
            "url": "/app/reflections/20260308",
        }
    ]
    assert not _log_path(tmp_path).exists()


def test_handle_sol_chat_request_filters_wrong_tract(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    send_calls: list[dict[str, object]] = []
    monkeypatch.setattr(
        triggers,
        "send_many",
        lambda *args, **kwargs: send_calls.append(kwargs) or (1, 0),
    )

    triggers.handle_sol_chat_request(
        {"tract": "cortex", "event": KIND_SOL_CHAT_REQUEST, "request_id": "req-1"}
    )

    assert send_calls == []
    assert not _log_path(tmp_path).exists()


def test_handle_sol_chat_request_filters_wrong_event(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    send_calls: list[dict[str, object]] = []
    monkeypatch.setattr(
        triggers,
        "send_many",
        lambda *args, **kwargs: send_calls.append(kwargs) or (1, 0),
    )

    triggers.handle_sol_chat_request(
        {"tract": "chat", "event": KIND_OWNER_CHAT_OPEN, "request_id": "req-1"}
    )

    assert send_calls == []
    assert not _log_path(tmp_path).exists()


def test_handle_sol_chat_request_skips_when_unconfigured(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(triggers, "is_configured", lambda: False)
    monkeypatch.setattr(
        triggers,
        "_eligible_devices",
        lambda: (_ for _ in ()).throw(AssertionError("devices should not load")),
    )

    triggers.handle_sol_chat_request(
        {"tract": "chat", "event": KIND_SOL_CHAT_REQUEST, "request_id": "req-1"}
    )

    assert not _log_path(tmp_path).exists()


def test_handle_sol_chat_request_skips_when_no_devices(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(triggers, "is_configured", lambda: True)
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: [])

    triggers.handle_sol_chat_request(
        {"tract": "chat", "event": KIND_SOL_CHAT_REQUEST, "request_id": "req-1"}
    )

    assert not _log_path(tmp_path).exists()


def test_handle_sol_chat_request_dispatches_and_logs(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(triggers, "is_configured", lambda: True)
    devices = [{"token": "a" * 64}]
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: devices)
    monkeypatch.setattr(triggers.time, "time", lambda: 123.0)
    sent_calls: list[dict[str, object]] = []

    def fake_send_many(push_devices, payload, *, collapse_id, priority, **kwargs):
        sent_calls.append(
            {
                "devices": push_devices,
                "payload": payload,
                "collapse_id": collapse_id,
                "priority": priority,
                "kwargs": kwargs,
            }
        )
        return 1, 0

    monkeypatch.setattr(triggers, "send_many", fake_send_many)

    triggers.handle_sol_chat_request(
        {
            "tract": "chat",
            "event": KIND_SOL_CHAT_REQUEST,
            "request_id": "req-1",
            "summary": "Needs a reply",
            "category": "notice",
        }
    )

    assert sent_calls[0]["devices"] == devices
    assert sent_calls[0]["payload"]["data"] == {
        "action": "open_chat_request",
        "request_id": "req-1",
        "category": "notice",
    }
    assert sent_calls[0]["collapse_id"] == f"{KIND_SOL_CHAT_REQUEST}:req-1"
    assert sent_calls[0]["priority"] == 10
    assert sent_calls[0]["kwargs"] == {}
    assert _read_log(tmp_path) == [
        {
            "ts": 123,
            "kind": f"{KIND_SOL_CHAT_REQUEST}_push",
            "dedupe_key": "req-1",
            "category": "notice",
            "outcome": "dispatched",
            "via": "local",
        }
    ]


def test_handle_sol_chat_request_logs_error_on_apns_failure(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(triggers, "is_configured", lambda: True)
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: [{"token": "a" * 64}])
    monkeypatch.setattr(triggers.time, "time", lambda: 123.0)

    def fail_send_many(*args, **kwargs):
        raise RuntimeError("apns failed")

    monkeypatch.setattr(triggers, "send_many", fail_send_many)

    triggers.handle_sol_chat_request(
        {
            "tract": "chat",
            "event": KIND_SOL_CHAT_REQUEST,
            "request_id": "req-1",
            "summary": "Needs a reply",
            "category": "notice",
        }
    )

    assert _read_log(tmp_path)[0]["outcome"] == "error"


def test_handle_chat_lifecycle_dispatches_silent_for_open(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(triggers, "is_configured", lambda: True)
    devices = [{"token": "a" * 64}]
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: devices)
    monkeypatch.setattr(triggers.time, "time", lambda: 123.0)
    sent_calls: list[dict[str, object]] = []

    def fake_send_many(push_devices, payload, *, collapse_id, priority, push_type):
        sent_calls.append(
            {
                "devices": push_devices,
                "payload": payload,
                "collapse_id": collapse_id,
                "priority": priority,
                "push_type": push_type,
            }
        )
        return 1, 0

    monkeypatch.setattr(triggers, "send_many", fake_send_many)

    triggers.handle_chat_lifecycle(
        {"tract": "chat", "event": KIND_OWNER_CHAT_OPEN, "request_id": "req-1"}
    )

    assert sent_calls == [
        {
            "devices": devices,
            "payload": {
                "aps": {"mutable-content": 1, "content-available": 1},
                "data": {"action": KIND_OWNER_CHAT_OPEN, "request_id": "req-1"},
            },
            "collapse_id": f"sol_chat_lifecycle:req-1:{KIND_OWNER_CHAT_OPEN}",
            "priority": 5,
            "push_type": "background",
        }
    ]
    assert _read_log(tmp_path) == [
        {
            "ts": 123,
            "kind": "sol_chat_lifecycle_push",
            "dedupe_key": "req-1",
            "category": KIND_OWNER_CHAT_OPEN,
            "outcome": "dispatched",
            "via": "local",
        }
    ]


def test_handle_chat_lifecycle_dispatches_silent_for_dismissed(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(triggers, "is_configured", lambda: True)
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: [{"token": "a" * 64}])
    sent_calls: list[dict[str, object]] = []
    monkeypatch.setattr(
        triggers,
        "send_many",
        lambda devices, payload, *, collapse_id, priority, push_type: (
            sent_calls.append(
                {
                    "payload": payload,
                    "collapse_id": collapse_id,
                    "priority": priority,
                    "push_type": push_type,
                }
            )
            or (1, 0)
        ),
    )

    triggers.handle_chat_lifecycle(
        {"tract": "chat", "event": KIND_OWNER_CHAT_DISMISSED, "request_id": "req-1"}
    )

    assert sent_calls[0]["payload"]["data"]["action"] == KIND_OWNER_CHAT_DISMISSED
    assert sent_calls[0]["push_type"] == "background"
    assert _read_log(tmp_path)[0]["category"] == KIND_OWNER_CHAT_DISMISSED


def test_handle_chat_lifecycle_filters_other_events(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    send_calls: list[dict[str, object]] = []
    monkeypatch.setattr(
        triggers,
        "send_many",
        lambda *args, **kwargs: send_calls.append(kwargs) or (1, 0),
    )

    triggers.handle_chat_lifecycle(
        {"tract": "chat", "event": KIND_SOL_CHAT_REQUEST, "request_id": "req-1"}
    )

    assert send_calls == []
    assert not _log_path(tmp_path).exists()


class _PortalResponse:
    def __init__(self, body: bytes, status: int = 200):
        self._body = body
        self.status = status

    def __enter__(self):
        return self

    def __exit__(self, *args):
        return False

    def read(self) -> bytes:
        return self._body

    def getcode(self) -> int:
        return self.status


def test_dispatch_via_portal_happy_path(monkeypatch):
    monkeypatch.setattr(
        portal_dispatch,
        "scout_provenance",
        lambda: {"dispatch_token": "dispatch-token", "account_id": "acct-1"},
    )
    monkeypatch.setattr(
        portal_dispatch, "portal_base_url", lambda: "https://portal.test"
    )

    def fake_urlopen(request, timeout):
        assert timeout == 10
        assert request.full_url == "https://portal.test/push/dispatch"
        assert request.get_method() == "POST"
        assert request.get_header("Authorization") == "Bearer dispatch-token"
        assert json.loads(request.data.decode("utf-8")) == {
            "summary": "Needs a reply",
            "category": "notice",
            "request_id": "req-1",
        }
        return _PortalResponse(b'{"ok": true, "sent": 2}')

    monkeypatch.setattr(portal_dispatch.urllib_request, "urlopen", fake_urlopen)

    assert portal_dispatch.dispatch_via_portal(
        request_id="req-1",
        summary="Needs a reply",
        category="notice",
    ) == {"ok": True, "sent": 2}


def test_dispatch_via_portal_4xx_returns_none(monkeypatch, caplog):
    token = "dispatch-token"
    monkeypatch.setattr(
        portal_dispatch,
        "scout_provenance",
        lambda: {"dispatch_token": token, "account_id": "acct-1"},
    )
    caplog.set_level("WARNING", logger=portal_dispatch.logger.name)

    def fake_urlopen(request, timeout):
        raise HTTPError(request.full_url, 400, "bad request", hdrs=None, fp=None)

    monkeypatch.setattr(portal_dispatch.urllib_request, "urlopen", fake_urlopen)

    assert (
        portal_dispatch.dispatch_via_portal(
            request_id="req-1",
            summary="Needs a reply",
            category="notice",
        )
        is None
    )
    assert "portal dispatch rejected" in caplog.text
    assert token not in caplog.text


def test_dispatch_via_portal_5xx_returns_none(monkeypatch):
    monkeypatch.setattr(
        portal_dispatch,
        "scout_provenance",
        lambda: {"dispatch_token": "dispatch-token", "account_id": "acct-1"},
    )

    def fake_urlopen(request, timeout):
        raise HTTPError(request.full_url, 500, "server error", hdrs=None, fp=None)

    monkeypatch.setattr(portal_dispatch.urllib_request, "urlopen", fake_urlopen)

    assert (
        portal_dispatch.dispatch_via_portal(
            request_id="req-1",
            summary="Needs a reply",
            category="notice",
        )
        is None
    )


def test_dispatch_via_portal_timeout_returns_none(monkeypatch):
    monkeypatch.setattr(
        portal_dispatch,
        "scout_provenance",
        lambda: {"dispatch_token": "dispatch-token", "account_id": "acct-1"},
    )
    monkeypatch.setattr(
        portal_dispatch.urllib_request,
        "urlopen",
        lambda request, timeout: (_ for _ in ()).throw(socket.timeout("timed out")),
    )

    assert (
        portal_dispatch.dispatch_via_portal(
            request_id="req-1",
            summary="Needs a reply",
            category="notice",
        )
        is None
    )


def test_dispatch_via_portal_no_scout_returns_none(monkeypatch):
    urlopen_called = False
    monkeypatch.setattr(portal_dispatch, "scout_provenance", lambda: None)

    def fake_urlopen(request, timeout):
        nonlocal urlopen_called
        urlopen_called = True
        return _PortalResponse(b"{}")

    monkeypatch.setattr(portal_dispatch.urllib_request, "urlopen", fake_urlopen)

    assert (
        portal_dispatch.dispatch_via_portal(
            request_id="req-1",
            summary="Needs a reply",
            category="notice",
        )
        is None
    )
    assert not urlopen_called


def test_dispatch_dedup_via_portal_posts_and_returns_payload(monkeypatch):
    monkeypatch.setattr(
        portal_dispatch,
        "scout_provenance",
        lambda: {"dispatch_token": "dispatch-token", "account_id": "acct-1"},
    )
    monkeypatch.setattr(
        portal_dispatch, "portal_base_url", lambda: "https://portal.test"
    )

    def fake_urlopen(request, timeout):
        assert timeout == 10
        assert request.full_url == "https://portal.test/push/dedup"
        assert request.get_method() == "POST"
        assert request.get_header("Authorization") == "Bearer dispatch-token"
        assert json.loads(request.data.decode("utf-8")) == {
            "request_id": "req-1",
            "action": KIND_OWNER_CHAT_OPEN,
        }
        return _PortalResponse(b'{"ok": true, "fanout": 3}')

    monkeypatch.setattr(portal_dispatch.urllib_request, "urlopen", fake_urlopen)

    assert portal_dispatch.dispatch_dedup_via_portal(
        request_id="req-1",
        action=KIND_OWNER_CHAT_OPEN,
    ) == {"ok": True, "fanout": 3}


def test_dispatch_dedup_via_portal_returns_none_on_4xx(monkeypatch):
    monkeypatch.setattr(
        portal_dispatch,
        "scout_provenance",
        lambda: {"dispatch_token": "dispatch-token", "account_id": "acct-1"},
    )

    def fake_urlopen(request, timeout):
        raise HTTPError(request.full_url, 400, "bad request", hdrs=None, fp=None)

    monkeypatch.setattr(portal_dispatch.urllib_request, "urlopen", fake_urlopen)

    assert (
        portal_dispatch.dispatch_dedup_via_portal(
            request_id="req-1",
            action=KIND_OWNER_CHAT_OPEN,
        )
        is None
    )


def test_dispatch_dedup_via_portal_returns_none_on_5xx(monkeypatch):
    monkeypatch.setattr(
        portal_dispatch,
        "scout_provenance",
        lambda: {"dispatch_token": "dispatch-token", "account_id": "acct-1"},
    )

    def fake_urlopen(request, timeout):
        raise HTTPError(request.full_url, 500, "server error", hdrs=None, fp=None)

    monkeypatch.setattr(portal_dispatch.urllib_request, "urlopen", fake_urlopen)

    assert (
        portal_dispatch.dispatch_dedup_via_portal(
            request_id="req-1",
            action=KIND_OWNER_CHAT_OPEN,
        )
        is None
    )


def test_dispatch_dedup_via_portal_returns_none_on_timeout(monkeypatch):
    monkeypatch.setattr(
        portal_dispatch,
        "scout_provenance",
        lambda: {"dispatch_token": "dispatch-token", "account_id": "acct-1"},
    )
    monkeypatch.setattr(
        portal_dispatch.urllib_request,
        "urlopen",
        lambda request, timeout: (_ for _ in ()).throw(socket.timeout("timed out")),
    )

    assert (
        portal_dispatch.dispatch_dedup_via_portal(
            request_id="req-1",
            action=KIND_OWNER_CHAT_OPEN,
        )
        is None
    )


def test_dispatch_dedup_via_portal_returns_none_when_no_scout(monkeypatch):
    urlopen_called = False
    monkeypatch.setattr(portal_dispatch, "scout_provenance", lambda: None)

    def fake_urlopen(request, timeout):
        nonlocal urlopen_called
        urlopen_called = True
        return _PortalResponse(b"{}")

    monkeypatch.setattr(portal_dispatch.urllib_request, "urlopen", fake_urlopen)

    assert (
        portal_dispatch.dispatch_dedup_via_portal(
            request_id="req-1",
            action=KIND_OWNER_CHAT_OPEN,
        )
        is None
    )
    assert not urlopen_called


def test_dispatch_dedup_via_portal_returns_none_when_dispatch_token_missing(
    monkeypatch,
):
    urlopen_called = False

    def fake_urlopen(request, timeout):
        nonlocal urlopen_called
        urlopen_called = True
        return _PortalResponse(b"{}")

    monkeypatch.setattr(portal_dispatch.urllib_request, "urlopen", fake_urlopen)
    for scout in ({"account_id": "acct-1"}, {"dispatch_token": ""}):
        monkeypatch.setattr(portal_dispatch, "scout_provenance", lambda: scout)
        assert (
            portal_dispatch.dispatch_dedup_via_portal(
                request_id="req-1",
                action=KIND_OWNER_CHAT_OPEN,
            )
            is None
        )

    assert not urlopen_called


def test_handle_sol_chat_request_routes_via_portal_when_scout_enabled(
    monkeypatch, tmp_path
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        triggers,
        "scout_provenance",
        lambda: {"dispatch_token": "dispatch-token", "account_id": "acct-1"},
    )
    monkeypatch.setattr(triggers.time, "time", lambda: 123.0)
    send_calls: list[dict[str, object]] = []
    portal_calls: list[dict[str, str]] = []
    monkeypatch.setattr(
        triggers,
        "send_many",
        lambda *args, **kwargs: send_calls.append(kwargs) or (1, 0),
    )

    def fake_dispatch_via_portal(*, request_id, summary, category):
        portal_calls.append(
            {"request_id": request_id, "summary": summary, "category": category}
        )
        return {"ok": True, "sent": 1}

    monkeypatch.setattr(triggers, "dispatch_via_portal", fake_dispatch_via_portal)

    triggers.handle_sol_chat_request(
        {
            "tract": "chat",
            "event": KIND_SOL_CHAT_REQUEST,
            "request_id": "req-1",
            "summary": "Needs a reply",
            "category": "notice",
        }
    )

    assert portal_calls == [
        {"request_id": "req-1", "summary": "Needs a reply", "category": "notice"}
    ]
    assert send_calls == []
    assert _read_log(tmp_path) == [
        {
            "ts": 123,
            "kind": f"{KIND_SOL_CHAT_REQUEST}_push",
            "dedupe_key": "req-1",
            "category": "notice",
            "outcome": "dispatched",
            "via": "portal",
        }
    ]


def test_handle_sol_chat_request_falls_back_to_local_when_portal_fails(
    monkeypatch, tmp_path
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        triggers,
        "scout_provenance",
        lambda: {"dispatch_token": "dispatch-token", "account_id": "acct-1"},
    )
    monkeypatch.setattr(triggers, "dispatch_via_portal", lambda **kwargs: None)
    monkeypatch.setattr(triggers, "is_configured", lambda: True)
    devices = [{"token": "a" * 64}]
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: devices)
    monkeypatch.setattr(triggers.time, "time", lambda: 123.0)
    sent_calls: list[list[dict[str, object]]] = []
    monkeypatch.setattr(
        triggers,
        "send_many",
        lambda push_devices, *args, **kwargs: sent_calls.append(push_devices) or (1, 0),
    )

    triggers.handle_sol_chat_request(
        {
            "tract": "chat",
            "event": KIND_SOL_CHAT_REQUEST,
            "request_id": "req-1",
            "summary": "Needs a reply",
            "category": "notice",
        }
    )

    assert sent_calls == [devices]
    assert _read_log(tmp_path)[0]["via"] == "local"


def test_handle_sol_chat_request_falls_back_to_local_when_scout_missing_token(
    monkeypatch, tmp_path
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(triggers, "scout_provenance", lambda: {"account_id": "acct-1"})
    monkeypatch.setattr(
        triggers,
        "dispatch_via_portal",
        lambda **kwargs: (_ for _ in ()).throw(AssertionError("portal should not run")),
    )
    monkeypatch.setattr(triggers, "is_configured", lambda: True)
    devices = [{"token": "a" * 64}]
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: devices)
    sent_calls: list[list[dict[str, object]]] = []
    monkeypatch.setattr(
        triggers,
        "send_many",
        lambda push_devices, *args, **kwargs: sent_calls.append(push_devices) or (1, 0),
    )

    triggers.handle_sol_chat_request(
        {
            "tract": "chat",
            "event": KIND_SOL_CHAT_REQUEST,
            "request_id": "req-1",
            "summary": "Needs a reply",
            "category": "notice",
        }
    )

    assert sent_calls == [devices]


def test_handle_sol_chat_request_no_scout_unchanged(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(triggers, "scout_provenance", lambda: None)
    monkeypatch.setattr(
        triggers,
        "dispatch_via_portal",
        lambda **kwargs: (_ for _ in ()).throw(AssertionError("portal should not run")),
    )
    monkeypatch.setattr(triggers, "is_configured", lambda: True)
    devices = [{"token": "a" * 64}]
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: devices)
    sent_calls: list[list[dict[str, object]]] = []
    monkeypatch.setattr(
        triggers,
        "send_many",
        lambda push_devices, *args, **kwargs: sent_calls.append(push_devices) or (1, 0),
    )

    triggers.handle_sol_chat_request(
        {
            "tract": "chat",
            "event": KIND_SOL_CHAT_REQUEST,
            "request_id": "req-1",
            "summary": "Needs a reply",
            "category": "notice",
        }
    )

    assert sent_calls == [devices]


def test_handle_chat_lifecycle_routes_via_portal_when_scout_enabled(
    monkeypatch, tmp_path
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        triggers,
        "scout_provenance",
        lambda: {"dispatch_token": "dispatch-token", "account_id": "acct-1"},
    )
    monkeypatch.setattr(triggers.time, "time", lambda: 123.0)
    send_calls: list[dict[str, object]] = []
    portal_calls: list[dict[str, str]] = []
    monkeypatch.setattr(
        triggers,
        "_eligible_devices",
        lambda: (_ for _ in ()).throw(AssertionError("devices should not load")),
    )
    monkeypatch.setattr(
        triggers,
        "send_many",
        lambda *args, **kwargs: send_calls.append(kwargs) or (1, 0),
    )

    def fake_dispatch_dedup_via_portal(*, request_id, action):
        portal_calls.append({"request_id": request_id, "action": action})
        return {"ok": True}

    monkeypatch.setattr(
        triggers, "dispatch_dedup_via_portal", fake_dispatch_dedup_via_portal
    )

    triggers.handle_chat_lifecycle(
        {"tract": "chat", "event": KIND_OWNER_CHAT_OPEN, "request_id": "req-1"}
    )

    assert portal_calls == [{"request_id": "req-1", "action": KIND_OWNER_CHAT_OPEN}]
    assert send_calls == []
    assert _read_log(tmp_path) == [
        {
            "ts": 123,
            "kind": "sol_chat_lifecycle_push",
            "dedupe_key": "req-1",
            "category": KIND_OWNER_CHAT_OPEN,
            "outcome": "dispatched",
            "via": "portal",
        }
    ]


def test_handle_chat_lifecycle_falls_back_to_local_when_portal_fails(
    monkeypatch, tmp_path
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        triggers,
        "scout_provenance",
        lambda: {"dispatch_token": "dispatch-token", "account_id": "acct-1"},
    )
    monkeypatch.setattr(triggers, "dispatch_dedup_via_portal", lambda **kwargs: None)
    monkeypatch.setattr(triggers, "is_configured", lambda: True)
    devices = [{"token": "a" * 64}]
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: devices)
    monkeypatch.setattr(triggers.time, "time", lambda: 123.0)
    sent_calls: list[dict[str, object]] = []

    def fake_send_many(push_devices, payload, *, collapse_id, priority, push_type):
        sent_calls.append(
            {
                "devices": push_devices,
                "payload": payload,
                "collapse_id": collapse_id,
                "priority": priority,
                "push_type": push_type,
            }
        )
        return 1, 0

    monkeypatch.setattr(triggers, "send_many", fake_send_many)

    triggers.handle_chat_lifecycle(
        {"tract": "chat", "event": KIND_OWNER_CHAT_OPEN, "request_id": "req-1"}
    )

    assert sent_calls == [
        {
            "devices": devices,
            "payload": {
                "aps": {"mutable-content": 1, "content-available": 1},
                "data": {"action": KIND_OWNER_CHAT_OPEN, "request_id": "req-1"},
            },
            "collapse_id": f"sol_chat_lifecycle:req-1:{KIND_OWNER_CHAT_OPEN}",
            "priority": 5,
            "push_type": "background",
        }
    ]
    assert _read_log(tmp_path) == [
        {
            "ts": 123,
            "kind": "sol_chat_lifecycle_push",
            "dedupe_key": "req-1",
            "category": KIND_OWNER_CHAT_OPEN,
            "outcome": "dispatched",
            "via": "local",
        }
    ]


def test_handle_chat_lifecycle_falls_back_to_local_when_scout_missing_token(
    monkeypatch, tmp_path
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(triggers, "scout_provenance", lambda: {"account_id": "acct-1"})
    monkeypatch.setattr(
        triggers,
        "dispatch_dedup_via_portal",
        lambda **kwargs: (_ for _ in ()).throw(AssertionError("portal should not run")),
    )
    monkeypatch.setattr(triggers, "is_configured", lambda: True)
    devices = [{"token": "a" * 64}]
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: devices)
    sent_calls: list[list[dict[str, object]]] = []
    monkeypatch.setattr(
        triggers,
        "send_many",
        lambda push_devices, *args, **kwargs: sent_calls.append(push_devices) or (1, 0),
    )

    triggers.handle_chat_lifecycle(
        {"tract": "chat", "event": KIND_OWNER_CHAT_OPEN, "request_id": "req-1"}
    )

    assert sent_calls == [devices]
    assert _read_log(tmp_path)[0]["via"] == "local"


def test_handle_chat_lifecycle_no_scout_unchanged(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(triggers, "scout_provenance", lambda: None)
    monkeypatch.setattr(
        triggers,
        "dispatch_dedup_via_portal",
        lambda **kwargs: (_ for _ in ()).throw(AssertionError("portal should not run")),
    )
    monkeypatch.setattr(triggers, "is_configured", lambda: True)
    devices = [{"token": "a" * 64}]
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: devices)
    sent_calls: list[list[dict[str, object]]] = []
    monkeypatch.setattr(
        triggers,
        "send_many",
        lambda push_devices, *args, **kwargs: sent_calls.append(push_devices) or (1, 0),
    )

    triggers.handle_chat_lifecycle(
        {"tract": "chat", "event": KIND_OWNER_CHAT_OPEN, "request_id": "req-1"}
    )

    assert sent_calls == [devices]
    assert _read_log(tmp_path)[0]["via"] == "local"


def test_handle_chat_lifecycle_local_send_many_error_records_outcome_error(
    monkeypatch, tmp_path
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(triggers, "scout_provenance", lambda: None)
    monkeypatch.setattr(triggers, "is_configured", lambda: True)
    monkeypatch.setattr(triggers, "_eligible_devices", lambda: [{"token": "a" * 64}])
    monkeypatch.setattr(triggers.time, "time", lambda: 123.0)

    def fail_send_many(*args, **kwargs):
        raise RuntimeError("apns failed")

    monkeypatch.setattr(triggers, "send_many", fail_send_many)

    triggers.handle_chat_lifecycle(
        {"tract": "chat", "event": KIND_OWNER_CHAT_OPEN, "request_id": "req-1"}
    )

    assert _read_log(tmp_path) == [
        {
            "ts": 123,
            "kind": "sol_chat_lifecycle_push",
            "dedupe_key": "req-1",
            "category": KIND_OWNER_CHAT_OPEN,
            "outcome": "error",
            "via": "local",
        }
    ]


def test_dispatch_via_portal_does_not_log_token_plaintext(monkeypatch, caplog):
    token = "TEST_TOKEN_SHOULD_NEVER_APPEAR"
    monkeypatch.setattr(
        portal_dispatch,
        "scout_provenance",
        lambda: {"dispatch_token": token, "account_id": "acct-1"},
    )
    caplog.set_level("WARNING", logger=portal_dispatch.logger.name)

    def fake_urlopen(request, timeout):
        raise HTTPError(request.full_url, 400, "bad request", hdrs=None, fp=None)

    monkeypatch.setattr(portal_dispatch.urllib_request, "urlopen", fake_urlopen)

    assert (
        portal_dispatch.dispatch_via_portal(
            request_id="req-1",
            summary="Needs a reply",
            category="notice",
        )
        is None
    )
    assert token not in caplog.text


def test_dispatch_dedup_via_portal_does_not_log_token_plaintext(monkeypatch, caplog):
    token = "TEST_DEDUP_TOKEN_SHOULD_NEVER_APPEAR"
    monkeypatch.setattr(
        portal_dispatch,
        "scout_provenance",
        lambda: {"dispatch_token": token, "account_id": "acct-1"},
    )
    caplog.set_level("WARNING", logger=portal_dispatch.logger.name)

    def raise_http(status):
        def fake_urlopen(request, timeout):
            raise HTTPError(request.full_url, status, "error", hdrs=None, fp=None)

        return fake_urlopen

    scenarios = (
        lambda request, timeout: _PortalResponse(b'{"ok": true}'),
        raise_http(400),
        raise_http(500),
        lambda request, timeout: (_ for _ in ()).throw(socket.timeout("timed out")),
    )
    for fake_urlopen in scenarios:
        caplog.clear()
        monkeypatch.setattr(portal_dispatch.urllib_request, "urlopen", fake_urlopen)
        portal_dispatch.dispatch_dedup_via_portal(
            request_id="req-1",
            action=KIND_OWNER_CHAT_OPEN,
        )
        assert token not in caplog.text


def test_dispatch_via_portal_module_has_no_brand_canon_violations():
    text = Path(portal_dispatch.__file__).read_text(encoding="utf-8")

    for pattern in (
        r"\bsign in\b",
        r"\byour account\b",
        r"\blinked\b",
        r"\bauthenticate\b",
        r"\blog[ -]?in\b",
    ):
        assert re.search(pattern, text, re.IGNORECASE) is None
