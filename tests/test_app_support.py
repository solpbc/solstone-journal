# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for support app routes."""

import json
import io
import os
import re
from datetime import datetime, timedelta

import pytest

from solstone.apps.support.diagnostics import collect_recent_errors
from solstone.apps.support.operations import (
    IdempotencyConflictError,
    OperationInProgressError,
    OperationInvalidStateError,
)
from solstone.think.brain_health import HEADLINES

_LEAK_NEEDLES = ("private_key", "keypair", "access_token")


def _assert_no_credential_leak(serialized: str) -> None:
    assert not re.search(r"BEGIN.*PRIVATE KEY", serialized)
    for needle in _LEAK_NEEDLES:
        assert needle not in serialized, f"leaked {needle!r} in response body"


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
def journal(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    return tmp_path


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


def test_config_route_reports_enabled_and_portal_url(support_client, monkeypatch):
    monkeypatch.setattr("solstone.apps.support.portal.is_enabled", lambda: True)
    monkeypatch.setattr(
        "solstone.apps.support.portal._get_portal_url_from_settings",
        lambda: "https://support.example.test",
    )

    resp = support_client.get("/app/support/api/config")

    assert resp.status_code == 200
    assert resp.get_json() == {
        "enabled": True,
        "portal_url": "https://support.example.test",
    }
    _assert_no_credential_leak(resp.get_data(as_text=True))


def test_portal_url_defaults_to_support_host(journal, monkeypatch):
    from solstone.apps.support import portal

    monkeypatch.delenv(portal.SUPPORT_PORTAL_URL_ENV, raising=False)

    assert portal._get_portal_url_from_settings() == portal.DEFAULT_PORTAL_URL
    assert portal.get_client(anonymous=True).portal_url == portal.DEFAULT_PORTAL_URL


def test_portal_url_env_overrides_configured_host(journal, monkeypatch):
    from solstone.apps.support import portal

    config_dir = journal / "config"
    config_dir.mkdir()
    (config_dir / "config.json").write_text(
        json.dumps({"support": {"portal_url": "https://support.solstone.app"}})
    )
    monkeypatch.setenv(
        portal.SUPPORT_PORTAL_URL_ENV, "https://support-sink.example.test/"
    )

    assert portal._get_portal_url_from_settings() == "https://support-sink.example.test"
    assert portal.get_client(anonymous=True).portal_url == (
        "https://support-sink.example.test"
    )


def test_portal_anonymous_handles_are_random_stable_and_hostname_free(
    tmp_path, monkeypatch
):
    import socket

    from solstone.apps.support import portal

    draws = iter([b"\x01\x23\x45\x67", b"\x89\xab\xcd\xef"])

    def fake_urandom(size):
        assert size == 4
        return next(draws)

    def fail_gethostname():
        raise AssertionError("anonymous portal handles must not read hostname")

    monkeypatch.setattr(portal.os, "urandom", fake_urandom)
    monkeypatch.setattr(socket, "gethostname", fail_gethostname)

    first = portal.PortalClient(
        portal_url="https://support.example.test",
        storage_dir=tmp_path / "first",
        anonymous=True,
    )
    second = portal.PortalClient(
        portal_url="https://support.example.test",
        storage_dir=tmp_path / "second",
        anonymous=True,
    )

    first_handle = first.handle
    second_handle = second.handle

    assert first_handle == "anon-01234567"
    assert second_handle == "anon-89abcdef"
    assert first_handle != second_handle
    assert re.fullmatch(r"anon-[0-9a-f]{8}", first_handle)
    assert re.fullmatch(r"anon-[0-9a-f]{8}", second_handle)
    assert first.handle == first_handle
    assert second.handle == second_handle


def test_portal_anonymous_explicit_handle_is_preserved(tmp_path, monkeypatch):
    from solstone.apps.support import portal

    def fail_urandom(_size):
        raise AssertionError("explicit anonymous handle must not draw randomness")

    monkeypatch.setattr(portal.os, "urandom", fail_urandom)

    client = portal.PortalClient(
        portal_url="https://support.example.test",
        storage_dir=tmp_path / "explicit",
        handle="provided",
        anonymous=True,
    )

    assert client.handle == "provided"


def test_portal_non_anonymous_explicit_handle_is_preserved(tmp_path, monkeypatch):
    import socket

    from solstone.apps.support import portal

    def fail_urandom(_size):
        raise AssertionError("explicit handle must not draw randomness")

    def fail_gethostname():
        raise AssertionError("explicit handle must not read hostname")

    monkeypatch.setattr(portal.os, "urandom", fail_urandom)
    monkeypatch.setattr(socket, "gethostname", fail_gethostname)

    client = portal.PortalClient(
        portal_url="https://support.example.test",
        storage_dir=tmp_path / "identified",
        handle="chosen-handle",
    )

    assert client.handle == "chosen-handle"


def test_portal_non_anonymous_no_handle_uses_hostname(tmp_path, monkeypatch):
    import socket

    from solstone.apps.support import portal

    def fail_urandom(_size):
        raise AssertionError("non-anonymous hostname fallback must not draw randomness")

    monkeypatch.setattr(portal.os, "urandom", fail_urandom)
    monkeypatch.setattr(socket, "gethostname", lambda: "My_Host.local!")

    client = portal.PortalClient(
        portal_url="https://support.example.test",
        storage_dir=tmp_path / "identified",
    )

    assert client.handle == "solstone-my-host.local"


def test_config_route_ungated_when_disabled(support_client, monkeypatch):
    monkeypatch.setattr("solstone.apps.support.portal.is_enabled", lambda: False)
    monkeypatch.setattr(
        "solstone.apps.support.portal._get_portal_url_from_settings",
        lambda: "https://support.example.test",
    )

    resp = support_client.get("/app/support/api/config")

    assert resp.status_code == 200
    assert resp.get_json()["enabled"] is False


def test_article_route_returns_portal_article(support_client, monkeypatch):
    class ArticleClient:
        def get_article(self, slug):
            return {"slug": slug, "title": "Intro", "body": "hello"}

    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: True)
    monkeypatch.setattr(
        "solstone.apps.support.routes._get_client",
        lambda: ArticleClient(),
    )

    resp = support_client.get("/app/support/api/articles/intro")

    assert resp.status_code == 200
    assert resp.get_json() == {"slug": "intro", "title": "Intro", "body": "hello"}
    _assert_no_credential_leak(resp.get_data(as_text=True))


def test_article_route_disabled_returns_403(support_client, monkeypatch):
    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: False)

    resp = support_client.get("/app/support/api/articles/intro")

    assert resp.status_code == 403
    assert resp.get_json()["reason_code"] == "feature_unavailable"


def test_article_route_portal_failure_returns_500(support_client, monkeypatch):
    class ArticleClient:
        def get_article(self, slug):
            raise RuntimeError("boom")

    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: True)
    monkeypatch.setattr(
        "solstone.apps.support.routes._get_client",
        lambda: ArticleClient(),
    )

    resp = support_client.get("/app/support/api/articles/intro")

    assert resp.status_code == 500
    payload = resp.get_json()
    assert payload["error"]
    assert payload["detail"]


def test_register_route_returns_handle_only(support_client, monkeypatch):
    class RegisterClient:
        def register(self):
            return {
                "handle": "solstone-foo",
                "access_token": "sk-SECRET",
                "keypair": "kp",
                "private_key": "-----BEGIN PRIVATE KEY-----abc",
            }

    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: True)
    monkeypatch.setattr(
        "solstone.apps.support.routes._get_client",
        lambda: RegisterClient(),
    )

    resp = support_client.post("/app/support/api/register")

    assert resp.status_code == 200
    assert resp.get_json() == {"handle": "solstone-foo"}
    _assert_no_credential_leak(resp.get_data(as_text=True))


def test_register_route_disabled_returns_403(support_client, monkeypatch):
    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: False)

    resp = support_client.post("/app/support/api/register")

    assert resp.status_code == 403


def test_register_route_error_does_not_leak_credentials(support_client, monkeypatch):
    class RegisterClient:
        def register(self):
            raise RuntimeError(
                "POST https://support.solstone.app/api/signup — 500: "
                '{"access_token": "sk-LEAKED", '
                '"private_key": "-----BEGIN PRIVATE KEY-----xyz"}'
            )

    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: True)
    monkeypatch.setattr(
        "solstone.apps.support.routes._get_client",
        lambda: RegisterClient(),
    )

    resp = support_client.post("/app/support/api/register")
    serialized = resp.get_data(as_text=True)

    assert resp.status_code == 500
    _assert_no_credential_leak(serialized)
    assert "sk-LEAKED" not in serialized
    assert resp.get_json()["detail"] == "Registration with the support portal failed."


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
        headers={"Idempotency-Key": "test-create-error-report"},
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
            "action_id": "test-create-error-report",
        }
    ]


@pytest.mark.parametrize(
    ("path", "kwargs"),
    [
        ("/app/support/api/tickets", {"json": {"subject": "S", "description": "D"}}),
        ("/app/support/api/tickets/1/reply", {"json": {"content": "reply"}}),
        (
            "/app/support/api/tickets/1/attachments",
            {"data": {"file": (io.BytesIO(b"x"), "file.txt")}},
        ),
        ("/app/support/api/feedback", {"json": {"body": "feedback"}}),
    ],
)
def test_mutation_routes_require_parent_action_id(support_client, monkeypatch, path, kwargs):
    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: True)

    response = support_client.post(path, **kwargs)

    assert response.status_code == 400
    assert response.get_json()["reason_code"] == "missing_required_field"


@pytest.mark.parametrize(
    ("exception", "reason_code", "status"),
    [
        (IdempotencyConflictError(), "idempotency_conflict", 409),
        (OperationInProgressError(), "operation_in_progress", 409),
        (OperationInvalidStateError(), "invalid_state", 409),
    ],
)
def test_create_route_maps_local_operation_errors(
    support_client, monkeypatch, exception, reason_code, status
):
    def raise_local_error(**_kwargs):
        raise exception

    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: True)
    monkeypatch.setattr(
        "solstone.apps.support.tools.support_create", raise_local_error
    )

    response = support_client.post(
        "/app/support/api/tickets",
        headers={"Idempotency-Key": "route-error"},
        json={"subject": "S", "description": "D"},
    )

    assert response.status_code == status
    assert response.get_json()["reason_code"] == reason_code


def test_lifecycle_routes_forward_calls_and_ignore_drain_failures(
    support_client, monkeypatch
):
    calls: list[tuple] = []

    class BrokenDrainClient:
        def drain_pending_acknowledgements(self):
            raise RuntimeError("drain failed")

    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: True)
    monkeypatch.setattr(
        "solstone.apps.support.routes._get_client", lambda: BrokenDrainClient()
    )
    monkeypatch.setattr(
        "solstone.apps.support.tools.support_history",
        lambda *, cursor=None: calls.append(("history", cursor))
        or {"tickets": [], "next_cursor": cursor},
    )
    monkeypatch.setattr(
        "solstone.apps.support.tools.support_close",
        lambda ticket_id, *, action_id: calls.append(("close", ticket_id, action_id))
        or {"ticket_id": ticket_id, "status": "closed"},
    )
    monkeypatch.setattr(
        "solstone.apps.support.tools.support_resolved",
        lambda ticket_id, *, action_id: calls.append(("resolved", ticket_id, action_id))
        or {"ticket_id": ticket_id, "status": "closed"},
    )
    monkeypatch.setattr(
        "solstone.apps.support.tools.support_still_need_help",
        lambda ticket_id, *, action_id: calls.append(("still_need_help", ticket_id, action_id))
        or {"ticket_id": ticket_id, "status": "open", "subject": "keep"},
    )

    history = support_client.get("/app/support/api/tickets/closed?cursor=opaque+cursor")
    close = support_client.post(
        "/app/support/api/tickets/7/close", headers={"Idempotency-Key": "close-1"}
    )
    resolved = support_client.post(
        "/app/support/api/tickets/7/resolution/confirm",
        headers={"Idempotency-Key": "resolved-1"},
    )
    still_needed = support_client.post(
        "/app/support/api/tickets/7/resolution/still-need-help",
        headers={"Idempotency-Key": "still-1"},
    )

    assert history.status_code == 200
    assert close.status_code == resolved.status_code == still_needed.status_code == 201
    assert still_needed.get_json()["subject"] == "keep"
    assert calls == [
        ("history", "opaque cursor"),
        ("close", 7, "close-1"),
        ("resolved", 7, "resolved-1"),
        ("still_need_help", 7, "still-1"),
    ]


@pytest.mark.parametrize(
    "path",
    [
        "/app/support/api/tickets/7/close",
        "/app/support/api/tickets/7/resolution/confirm",
        "/app/support/api/tickets/7/resolution/still-need-help",
    ],
)
def test_lifecycle_mutation_routes_require_parent_action_id(
    support_client, monkeypatch, path
):
    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: True)

    response = support_client.post(path)

    assert response.status_code == 400
    assert response.get_json()["reason_code"] == "missing_required_field"


def test_support_feedback_uses_feedback_subject_without_auto_context(monkeypatch):
    from solstone.apps.support import tools

    captured: list[dict] = []

    class FakePortalClient:
        def submit_feedback(self, **kwargs):
            captured.append(kwargs)
            return {"id": 123}

    monkeypatch.setattr(
        "solstone.apps.support.portal.get_client", lambda **_kwargs: FakePortalClient()
    )

    result = tools.support_feedback(
        body="owner feedback", anonymous=True, action_id="feedback-1"
    )

    assert result == {"id": 123}
    assert len(captured) == 1
    assert captured[0] == {
        "body": "owner feedback",
        "product": "solstone",
        "user_email": None,
        "user_context": None,
        "action_id": "feedback-1",
    }


def test_feedback_route_submits_without_auto_context(support_client, monkeypatch):
    captured: list[dict] = []

    class FakePortalClient:
        def submit_feedback(self, **kwargs):
            captured.append(kwargs)
            return {"id": 124}

    def fail_collect_all():
        raise AssertionError("feedback must not collect automatic diagnostics")

    def fail_collect_recent_errors():
        raise AssertionError("feedback must not collect recent errors")

    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: True)
    monkeypatch.setattr(
        "solstone.apps.support.portal.get_client",
        lambda **_kwargs: FakePortalClient(),
    )
    monkeypatch.setattr(
        "solstone.apps.support.diagnostics.collect_all", fail_collect_all
    )
    monkeypatch.setattr(
        "solstone.apps.support.diagnostics.collect_recent_errors",
        fail_collect_recent_errors,
    )

    resp = support_client.post(
        "/app/support/api/feedback",
        headers={"Idempotency-Key": "feedback-without-context"},
        json={"body": "hi", "anonymous": True},
    )

    assert resp.status_code == 201
    assert len(captured) == 1
    assert captured[0]["body"] == "hi"
    assert captured[0]["action_id"] == "feedback-without-context"


def test_create_ticket_route_still_collects_auto_context(support_client, monkeypatch):
    captured: list[dict] = []
    diagnostics_calls: list[str] = []

    class FakePortalClient:
        def create_ticket(self, **kwargs):
            captured.append(kwargs)
            return {"id": 125, "subject": kwargs["subject"]}

    def collect_all():
        diagnostics_calls.append("collect_all")
        return {"version": "9.9.9", "revision": "abc1234"}

    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: True)
    monkeypatch.setattr(
        "solstone.apps.support.portal.get_client",
        lambda **_kwargs: FakePortalClient(),
    )
    monkeypatch.setattr("solstone.apps.support.diagnostics.collect_all", collect_all)

    resp = support_client.post(
        "/app/support/api/tickets",
        headers={"Idempotency-Key": "create-with-context"},
        json={"subject": "S", "description": "D"},
    )

    assert resp.status_code == 201
    assert diagnostics_calls == ["collect_all"]
    assert len(captured) == 1
    assert captured[0]["subject"] == "S"
    assert captured[0]["user_context"] == {
        "version": "9.9.9",
        "revision": "abc1234",
    }


def test_feedback_anonymous_no_email_kwarg(support_client, monkeypatch):
    captured: list[dict] = []

    def recorder(**kwargs):
        captured.append(kwargs)
        return {"ok": True, "ticket_id": "t1"}

    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: True)
    monkeypatch.setattr("solstone.apps.support.tools.support_feedback", recorder)

    resp = support_client.post(
        "/app/support/api/feedback",
        headers={"Idempotency-Key": "feedback-anonymous"},
        json={"body": "hi", "anonymous": True},
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
        headers={"Idempotency-Key": "feedback-email"},
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
        headers={"Idempotency-Key": "feedback-smuggled"},
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
        headers={"Idempotency-Key": "feedback-empty"},
        json={"body": "hi", "anonymous": False, "user_email": "   "},
    )

    assert resp.status_code == 201
    assert len(captured) == 1
    assert "user_email" not in captured[0]


def test_revision_hash_when_git_available(monkeypatch):
    import subprocess

    from solstone.apps.support import diagnostics

    class _CP:
        returncode = 0
        stdout = "abc1234\n"

    monkeypatch.setattr(subprocess, "run", lambda *a, **k: _CP())
    assert diagnostics.collect_revision() == "abc1234"


def test_revision_none_when_not_a_repo(monkeypatch):
    import subprocess

    from solstone.apps.support import diagnostics

    class _CP:
        returncode = 128
        stdout = ""

    monkeypatch.setattr(subprocess, "run", lambda *a, **k: _CP())
    assert diagnostics.collect_revision() is None


def test_revision_none_when_git_raises(monkeypatch):
    import subprocess

    from solstone.apps.support import diagnostics

    def _boom(*a, **k):
        raise FileNotFoundError("git")

    monkeypatch.setattr(subprocess, "run", _boom)
    assert diagnostics.collect_revision() is None


def test_collect_all_includes_revision(monkeypatch):
    from solstone.apps.support import diagnostics

    monkeypatch.setattr(diagnostics, "collect_revision", lambda: "deadbee")
    bundle = diagnostics.collect_all()
    assert bundle["revision"] == "deadbee"
    assert "version" in bundle


def test_proactive_support_suppresses_readiness_blockers(monkeypatch):
    from solstone.apps.events import EventContext
    from solstone.apps.support import events

    captured: list[tuple[tuple, dict]] = []
    events._error_counts.clear()
    monkeypatch.setattr(events, "_is_proactive_enabled", lambda: True)
    monkeypatch.setattr(
        "solstone.think.callosum.callosum_send",
        lambda *args, **kwargs: captured.append((args, kwargs)),
    )

    for reason_code in (
        "provider_key_missing",
        "provider_key_missing",
        "local_server_unhealthy",
    ):
        events.detect_repeated_errors(
            EventContext(
                msg={"service": "cortex", "reason_code": reason_code},
                app="support",
                tract="cortex",
                event="error",
            )
        )

    assert captured == []
    assert "cortex" not in events._error_counts


def test_proactive_support_still_emits_for_generic_errors(monkeypatch):
    from solstone.apps.events import EventContext
    from solstone.apps.support import events

    captured: list[tuple[tuple, dict]] = []
    events._error_counts.clear()
    monkeypatch.setattr(events, "_is_proactive_enabled", lambda: True)
    monkeypatch.setattr(
        "solstone.think.callosum.callosum_send",
        lambda *args, **kwargs: captured.append((args, kwargs)),
    )

    for _ in range(3):
        events.detect_repeated_errors(
            EventContext(
                msg={"service": "cortex", "reason_code": "chat_timeout"},
                app="support",
                tract="cortex",
                event="error",
            )
        )

    assert len(captured) == 1
    args, kwargs = captured[0]
    assert args == ("support", "proactive_suggestion")
    assert kwargs["service"] == "cortex"
    assert kwargs["count"] == 3


def test_collect_brain_health_is_redacted(monkeypatch):
    from solstone.apps.support import diagnostics

    revai_secret = "REVAI_ACCESS_TOKEN_REDACTION_VALUE"
    snapshot = {
        "state": "blocked",
        "headline": HEADLINES["blocked"],
        "reason_code": "provider_key_invalid",
        "reason_text": "provider key invalid",
        "failing_component": "generate",
        "action": {"label": "open thinking", "href": "/app/thinking/#main"},
        "identity": {
            "lane": "cloud",
            "provider": "anthropic",
            "model": "claude-test",
        },
        "evidence": {
            "observed_at": None,
            "age_seconds": None,
            "age_text": None,
        },
        "components": {
            "generate": {
                "status": "blocked",
                "reason_code": "provider_key_invalid",
                "reason_text": ("reason_code=provider_key_invalid; provider=anthropic"),
                "observed_at": None,
            },
            "cogitate": {
                "status": "ready",
                "reason_code": None,
                "reason_text": None,
                "observed_at": None,
            },
        },
        "api_key": revai_secret,
        "progressing": False,
    }
    monkeypatch.setattr(
        "solstone.think.brain_health.build_brain_snapshot",
        lambda *_args, **_kwargs: snapshot,
    )
    monkeypatch.setattr(
        "solstone.think.brain_health.render_brain_health_lines",
        lambda _snapshot: ["Brain Health", f"  {HEADLINES['blocked']}"],
    )

    payload = diagnostics.collect_brain_health()
    serialized = json.dumps(payload)

    assert payload["snapshot"]["identity"]["provider"] == "anthropic"
    assert payload["snapshot"]["components"]["generate"]["status"] == "blocked"
    assert payload["lines"] == ["Brain Health", f"  {HEADLINES['blocked']}"]
    assert "ANTHROPIC_API_KEY" not in serialized
    assert "REVAI_ACCESS_TOKEN" not in serialized
    assert "OPENAI_API_KEY" not in serialized
    assert revai_secret not in serialized


def test_bounded_redacted_text_redacts_secret_assignments_and_preserves_reset():
    from solstone.apps.support import diagnostics

    assert diagnostics._bounded_redacted_text("/home/alice/secret") == "<path>"
    assert diagnostics._bounded_redacted_text(r"C:\Users\bob\key.txt") == "<path>"

    token = diagnostics._bounded_redacted_text("CUSTOM_TOKEN=opaquevalue")
    assert token == "<secret>"
    assert "CUSTOM_TOKEN" not in token
    assert "opaquevalue" not in token

    password = diagnostics._bounded_redacted_text("password=hunter2")
    assert password == "<secret>"
    assert "hunter2" not in password

    colon = diagnostics._bounded_redacted_text("api_key: sk-live-xyz")
    assert colon == "<secret>"
    assert "api_key" not in colon
    assert "sk-live-xyz" not in colon

    assert diagnostics._bounded_redacted_text("sk-ant-live-secret") == "<secret>"
    assert diagnostics._bounded_redacted_text("AIzaLiveSecret") == "<secret>"
    assert (
        diagnostics._bounded_redacted_text("Traceback (most recent call last): boom")
        == "traceback redacted boom"
    )
    assert diagnostics._bounded_redacted_text("reset_at_ms=123") == "reset_at_ms=123"


def test_collect_all_includes_brain_health(monkeypatch):
    from solstone.apps.support import diagnostics

    monkeypatch.setattr(
        diagnostics,
        "collect_brain_health",
        lambda: {"snapshot": {"state": "ready"}, "lines": ["Brain Health"]},
    )

    assert diagnostics.collect_all()["brain_health"] == {
        "snapshot": {"state": "ready"},
        "lines": ["Brain Health"],
    }


def test_collect_recent_errors_redacts_before_bounding(tmp_path, monkeypatch):
    health_dir = _health_dir(tmp_path, monkeypatch)
    older = (datetime.now() - timedelta(hours=2)).isoformat(timespec="seconds")
    newer = (datetime.now() - timedelta(hours=1)).isoformat(timespec="seconds")
    boundary_secret = "boundarysecret"
    boundary_prefix = "x" * 490

    _write_log(
        health_dir,
        "privacy.log",
        [
            (
                f"{older} [privacy:stderr] ERROR:root:{boundary_prefix} "
                f"CUSTOM_TOKEN={boundary_secret}; reset_at_ms=123"
            ),
            f"{newer} [privacy:stderr] ERROR:root:password=hunter2 /home/alice/secret",
            "ERROR raw api_key: colonsecret C:\\Users\\bob\\key.txt",
        ],
    )

    result = collect_recent_errors()
    messages = [entry["message"] for entry in result]
    serialized = "\n".join(messages)

    assert len(result) <= 10
    assert [entry["time"] for entry in result] == sorted(
        entry["time"] for entry in result
    )[::-1]
    assert all(len(message) <= 500 for message in messages)
    for leaked in (
        "CUSTOM_TOKEN",
        boundary_secret,
        "password",
        "hunter2",
        "api_key",
        "colonsecret",
        "/home/alice",
        "C:\\Users\\bob",
    ):
        assert leaked not in serialized
    assert "<secret>" in serialized
    assert "<path>" in serialized


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


def test_recent_errors_ignore_demoted_openhands_max_iterations_line(
    tmp_path,
    monkeypatch,
):
    health_dir = _health_dir(tmp_path, monkeypatch)
    recent = (datetime.now() - timedelta(hours=1)).isoformat(timespec="seconds")
    demoted = (
        "INFO:openhands.sdk.conversation.impl.local_conversation:"
        "Agent reached maximum iterations limit (4)."
    )
    untouched = (
        "ERROR:openhands.sdk.conversation.impl.local_conversation:"
        "Agent reached maximum iterations limit (4)."
    )
    _write_log(
        health_dir,
        "cortex.log",
        [
            f"{recent} [cortex:stderr] {demoted}",
            f"{recent} [cortex:stderr] {untouched}",
        ],
    )

    result = collect_recent_errors()
    messages = [entry["message"] for entry in result]

    assert any(untouched in message for message in messages)
    assert not any(demoted in message for message in messages)


def test_unparseable_line_inherits_preceding_timestamp(tmp_path, monkeypatch):
    health_dir = _health_dir(tmp_path, monkeypatch)
    line_dt = datetime.now() - timedelta(hours=2)
    mtime_dt = datetime.now() - timedelta(hours=1)
    line_ts = line_dt.isoformat(timespec="seconds")

    log_path = _write_log(
        health_dir,
        "mixed.log",
        [
            f"{line_ts} [mixed:stderr] ERROR:root:line-timestamp",
            "ERROR something with no timestamp",
        ],
    )
    # mtime is more recent than the parsed line; carry-forward must win over it.
    os.utime(log_path, (mtime_dt.timestamp(), mtime_dt.timestamp()))

    result = collect_recent_errors()
    line_entry = next(e for e in result if "line-timestamp" in e["message"])
    carried_entry = next(e for e in result if "no timestamp" in e["message"])

    assert line_entry["time"] == line_ts
    assert line_entry["time_approximate"] is False
    # Inherits the preceding parsed timestamp, NOT the file mtime.
    assert carried_entry["time"] == line_ts
    assert carried_entry["time_approximate"] is True


def test_old_anchor_excludes_carried_unparseable(tmp_path, monkeypatch):
    health_dir = _health_dir(tmp_path, monkeypatch)
    stale_dt = datetime.now() - timedelta(days=30)
    stale_ts = stale_dt.isoformat(timespec="seconds")

    log_path = _write_log(
        health_dir,
        "stale_carry.log",
        [
            f"{stale_ts} [carry:stderr] ERROR:root:old-anchor",
            "ERROR continuation with no timestamp",
        ],
    )
    # A recent mtime would, under the old bug, pull the unparseable line in.
    now_ts = datetime.now().timestamp()
    os.utime(log_path, (now_ts, now_ts))

    # Anchor is outside the window; the unparseable line inherits it -> both excluded.
    assert collect_recent_errors() == []


def test_unparseable_first_line_uses_mtime(tmp_path, monkeypatch):
    health_dir = _health_dir(tmp_path, monkeypatch)
    mtime_dt = datetime.now() - timedelta(hours=1)
    mtime_ts = mtime_dt.isoformat(timespec="seconds")

    log_path = _write_log(
        health_dir,
        "headless.log",
        ["ERROR boom with no leading timestamp"],
    )
    os.utime(log_path, (mtime_dt.timestamp(), mtime_dt.timestamp()))

    result = collect_recent_errors()
    entry = next(e for e in result if "boom" in e["message"])
    assert entry["time"] == mtime_ts
    assert entry["time_approximate"] is True


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
