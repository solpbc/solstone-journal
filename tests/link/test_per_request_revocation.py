# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import time
from pathlib import Path

import pytest

from solstone.apps.link import routes as link_routes
from solstone.convey import root as root_module
from solstone.think.link.auth import AuthorizedClients
from solstone.think.link.paths import authorized_clients_path
from tests.link.certless_helpers import make_convey_app, pl_identity

FINGERPRINT = "sha256:" + ("a" * 64)


def test_fingerprinted_pl_identity_rechecked_each_request(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    app, _journal = make_convey_app(tmp_path, monkeypatch)
    store = AuthorizedClients(authorized_clients_path())
    store.add(FINGERPRINT, "phone", "inst-1")
    monkeypatch.setattr(root_module, "get_authorized_clients", lambda: store)
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")

    client = app.test_client()
    identity = pl_identity(FINGERPRINT)
    response = client.get(
        "/app/link/api/status",
        base_url="https://solstone.local",
        environ_overrides={"pl.identity": identity},
    )
    assert response.status_code == 200

    time.sleep(0.02)
    authorized_clients_path().write_text(json.dumps([], indent=2) + "\n")

    response = client.get(
        "/app/link/api/status",
        base_url="https://solstone.local",
        environ_overrides={"pl.identity": identity},
    )

    assert response.status_code == 403
    assert response.get_json()["reason_code"] == "pl_revoked"


def test_corrupt_authorized_clients_fails_closed_without_last_good_cache(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    app, _journal = make_convey_app(tmp_path, monkeypatch)
    store = AuthorizedClients(authorized_clients_path())
    store.add(FINGERPRINT, "phone", "inst-1")
    monkeypatch.setattr(root_module, "get_authorized_clients", lambda: store)

    client = app.test_client()
    identity = pl_identity(FINGERPRINT)
    response = client.get(
        "/app/link/api/status",
        base_url="https://solstone.local",
        environ_overrides={"pl.identity": identity},
    )
    assert response.status_code == 200

    time.sleep(0.02)
    authorized_clients_path().write_text("{not json", encoding="utf-8")

    response = client.get(
        "/app/link/api/status",
        base_url="https://solstone.local",
        environ_overrides={"pl.identity": identity},
    )

    # Unreadable authorized_clients.json means no clients are authorized. There is no last-good authorization cache.
    assert response.status_code == 403
    assert response.get_json()["reason_code"] == "pl_revoked"
