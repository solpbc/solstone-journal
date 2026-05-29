# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from solstone.think.link.auth import AuthorizedClients
from solstone.think.link.paths import authorized_clients_path

FINGERPRINT = "sha256:" + ("a" * 64)
UNKNOWN_FINGERPRINT = "sha256:" + ("b" * 64)
PAIRED_AT = "2026-05-20T00:00:00Z"


def _authorized() -> AuthorizedClients:
    return AuthorizedClients(authorized_clients_path())


def _add_device() -> None:
    _authorized().add(
        FINGERPRINT,
        "old name",
        "inst-1",
        paired_at=PAIRED_AT,
    )


def test_rename_updates_paired_device_label(link_env) -> None:
    env = link_env()
    _add_device()

    response = env.client.post(
        "/app/link/rename",
        json={"fingerprint": FINGERPRINT, "label": "  new name  "},
    )

    assert response.status_code == 200
    assert response.get_json() == {"fingerprint": FINGERPRINT, "label": "new name"}
    entry = _authorized().get(FINGERPRINT)
    assert entry is not None
    assert entry.device_label == "new name"


def test_rename_empty_label_returns_invalid_request(link_env) -> None:
    env = link_env()
    _add_device()

    response = env.client.post(
        "/app/link/rename",
        json={"fingerprint": FINGERPRINT, "label": "   "},
    )

    assert response.status_code == 400
    payload = response.get_json()
    assert payload["reason_code"] == "invalid_request_value"
    assert payload["detail"] == "label must not be empty"


def test_rename_unknown_fingerprint_returns_not_found(link_env) -> None:
    env = link_env()

    response = env.client.post(
        "/app/link/rename",
        json={"fingerprint": UNKNOWN_FINGERPRINT, "label": "new name"},
    )

    assert response.status_code == 404
    payload = response.get_json()
    assert payload["reason_code"] == "paired_device_not_found"
    assert payload["detail"] == "fingerprint not paired"
