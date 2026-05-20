# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Regression tests for the link pair-start response contract."""

from __future__ import annotations

import re

from solstone.apps.link import routes as link_routes

PAIR_START_KEYS = [
    "nonce",
    "pair_link",
    "manual_code",
    "expires_in",
    "device_label",
    "lan_url",
    "ca_fingerprint",
]


def test_pair_start_shape_and_locked_order(link_env) -> None:
    env = link_env()

    response = env.client.post(
        "/app/link/pair-start",
        json={"device_label": "Test Phone"},
    )

    assert response.status_code == 200
    payload = response.get_json()
    assert list(payload.keys()) == PAIR_START_KEYS
    assert re.fullmatch(
        r"^https://link\.solpbc\.org/p#[0-9A-HJKMNP-TV-Z]{52}$",
        payload["pair_link"],
    )
    assert re.fullmatch(
        r"^[0-9A-HJKMNP-TV-Z]{4}-[0-9A-HJKMNP-TV-Z]{4}$",
        payload["manual_code"],
    )
    assert "://" not in payload["lan_url"]
    assert "pair_url" not in payload
    assert "qr_payload" not in payload


def test_pair_start_mints_distinct_nonce_and_manual_code(link_env) -> None:
    env = link_env()

    first = env.client.post(
        "/app/link/pair-start",
        json={"device_label": "First Phone"},
    ).get_json()
    second = env.client.post(
        "/app/link/pair-start",
        json={"device_label": "Second Phone"},
    ).get_json()

    assert first["nonce"] != second["nonce"]
    assert first["manual_code"] != second["manual_code"]


def test_pair_start_rejects_non_ipv4_pair_link_host(link_env, monkeypatch) -> None:
    env = link_env()
    monkeypatch.setattr(
        link_routes,
        "_resolve_host_port",
        lambda: "mylab.local:7070",
    )

    response = env.client.post(
        "/app/link/pair-start",
        json={"device_label": "Test Phone"},
    )

    assert response.status_code == 400
    payload = response.get_json()
    assert payload["reason_code"] == "pairing_request_invalid"
    assert "mylab.local" in payload["detail"]
    assert link_routes._nonces().snapshot() == []
