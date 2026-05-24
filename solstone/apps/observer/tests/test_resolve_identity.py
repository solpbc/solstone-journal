# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import pytest
from flask import Flask, g

from solstone.apps.observer.utils import (
    load_observer_by_fingerprint,
    mint_pl_observer_record,
    resolve_observer_identity,
    save_observer,
)
from solstone.convey.secure_listener import ConveyIdentity

DL_KEY = "dlkey123456789"
FINGERPRINT = "sha256:" + ("c" * 64)
OTHER_FINGERPRINT = "sha256:" + ("d" * 64)


@pytest.fixture
def app_env(tmp_path, monkeypatch):
    from solstone.convey import state

    journal = tmp_path / "journal"
    journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setattr(state, "journal_root", str(journal))
    app = Flask(__name__)
    return app


def _error_payload(error):
    response, status = error
    return response.get_json(), status


def _pl_identity(fingerprint: str) -> ConveyIdentity:
    return ConveyIdentity(
        mode="pl-direct",
        fingerprint=fingerprint,
        device_label="observer",
        paired_at="2026-04-20T00:00:00Z",
        session_id=None,
    )


def test_resolve_dl_success_from_bearer(app_env):
    save_observer({"key": DL_KEY, "name": "dl", "enabled": True, "stats": {}})

    with app_env.test_request_context(headers={"Authorization": f"Bearer {DL_KEY}"}):
        observer, prefix, error = resolve_observer_identity()

    assert error is None
    assert observer["name"] == "dl"
    assert prefix == DL_KEY[:8]


def test_resolve_dl_missing_auth(app_env):
    with app_env.test_request_context():
        observer, prefix, error = resolve_observer_identity()

    payload, status = _error_payload(error)
    assert observer is None
    assert prefix is None
    assert status == 401
    assert payload["reason_code"] == "auth_required"


def test_resolve_dl_invalid_key(app_env):
    save_observer({"key": DL_KEY, "name": "dl", "enabled": True, "stats": {}})

    with app_env.test_request_context(headers={"Authorization": "Bearer wrong"}):
        observer, prefix, error = resolve_observer_identity()

    payload, status = _error_payload(error)
    assert observer is None
    assert prefix is None
    assert status == 401
    assert payload["reason_code"] == "auth_key_invalid"


def test_resolve_dl_revoked(app_env):
    save_observer({"key": DL_KEY, "name": "dl", "revoked": True, "stats": {}})

    with app_env.test_request_context(headers={"Authorization": f"Bearer {DL_KEY}"}):
        _observer, _prefix, error = resolve_observer_identity()

    payload, status = _error_payload(error)
    assert status == 403
    assert payload["reason_code"] == "pl_revoked"


def test_resolve_dl_disabled(app_env):
    save_observer({"key": DL_KEY, "name": "dl", "enabled": False, "stats": {}})

    with app_env.test_request_context(headers={"Authorization": f"Bearer {DL_KEY}"}):
        _observer, _prefix, error = resolve_observer_identity()

    payload, status = _error_payload(error)
    assert status == 403
    assert payload["reason_code"] == "feature_unavailable"


def test_resolve_pl_success(app_env):
    mint_pl_observer_record(FINGERPRINT, "observer", "2026-04-20T00:00:00Z")

    with app_env.test_request_context():
        g.identity = _pl_identity(FINGERPRINT)
        observer, prefix, error = resolve_observer_identity("ignored-route-key")

    assert error is None
    assert observer["name"] == "observer"
    assert prefix == "c" * 16


def test_resolve_pl_phone_without_observer_record_is_auth_required(app_env):
    with app_env.test_request_context():
        g.identity = _pl_identity(OTHER_FINGERPRINT)
        observer, prefix, error = resolve_observer_identity()

    payload, status = _error_payload(error)
    assert observer is None
    assert prefix is None
    assert status == 401
    assert payload["reason_code"] == "auth_required"


def test_resolve_pl_revoked(app_env):
    save_observer(
        {
            "fingerprint": FINGERPRINT,
            "name": "observer",
            "revoked": True,
            "stats": {},
        }
    )

    with app_env.test_request_context():
        g.identity = _pl_identity(FINGERPRINT)
        _observer, _prefix, error = resolve_observer_identity()

    payload, status = _error_payload(error)
    assert status == 403
    assert payload["reason_code"] == "pl_revoked"


def test_resolve_pl_disabled(app_env):
    save_observer(
        {
            "fingerprint": FINGERPRINT,
            "name": "observer",
            "enabled": False,
            "stats": {},
        }
    )

    with app_env.test_request_context():
        g.identity = _pl_identity(FINGERPRINT)
        _observer, _prefix, error = resolve_observer_identity()

    payload, status = _error_payload(error)
    assert status == 403
    assert payload["reason_code"] == "feature_unavailable"


def test_resolve_pl_does_not_require_url_key(app_env):
    mint_pl_observer_record(FINGERPRINT, "observer", "2026-04-20T00:00:00Z")

    with app_env.test_request_context("/app/observer/ingest/not-the-fingerprint/event"):
        g.identity = _pl_identity(FINGERPRINT)
        observer, prefix, error = resolve_observer_identity("not-the-fingerprint")

    assert error is None
    assert prefix == "c" * 16
    assert load_observer_by_fingerprint(observer["fingerprint"]) is not None
