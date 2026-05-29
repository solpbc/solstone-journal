# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Regression tests for the link pair-start response contract."""

from __future__ import annotations

import re
import time
import uuid

from solstone.apps.link import routes as link_routes
from solstone.apps.link.crockford32 import decode as crockford_decode
from solstone.apps.link.relay_link import TOTP_STEP_SECONDS, compute_current_totp
from solstone.think.link.ca import load_or_generate_ca
from solstone.think.link.nonces import NONCE_TTL_SECONDS
from solstone.think.link.paths import LinkState, ca_dir

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
        r"^https://link\.solpbc\.org/p#[0-9A-HJKMNP-TV-Z]{64}$",
        payload["pair_link"],
    )
    assert re.fullmatch(
        r"^[0-9A-HJKMNP-TV-Z]{4}-[0-9A-HJKMNP-TV-Z]{4}$",
        payload["manual_code"],
    )
    snap = link_routes._nonces().snapshot()
    assert payload["expires_in"] == NONCE_TTL_SECONDS
    assert len(snap) == 1
    assert snap[0].expires_at - snap[0].issued_at == NONCE_TTL_SECONDS
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


def _fragment(pair_link: str) -> str:
    return pair_link.rsplit("#", 1)[1]


def _decode_pair_link(pair_link: str) -> bytes:
    return crockford_decode(_fragment(pair_link))


def test_pair_start_spl_mints_relay_form_pair_link(link_env) -> None:
    secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"
    env = link_env(posture="spl", totp_secret=secret)

    response = env.client.post(
        "/app/link/pair-start",
        json={"device_label": "Test Phone"},
    )

    assert response.status_code == 200
    payload = response.get_json()
    decoded = _decode_pair_link(payload["pair_link"])
    instance_id = LinkState.load_or_create().instance_id
    ca = load_or_generate_ca(ca_dir())
    now = int(time.time())

    assert decoded[0] == 0x03
    assert decoded[1:17] == uuid.UUID(instance_id).bytes
    assert int.from_bytes(decoded[17:20], "big") in {
        compute_current_totp(secret, now + delta) for delta in (-1, 0, 1)
    }
    assert len(decoded[20:36]) == 16
    assert decoded[36] == 0x01
    assert decoded[37:53] == bytes.fromhex(ca.spki_fingerprint_sha256())[:16]
    assert decoded[53] == 0x00
    assert len(decoded) == 54


def test_pair_start_spl_allows_non_ipv4_host(link_env, monkeypatch) -> None:
    env = link_env(posture="spl", totp_secret="GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ")
    monkeypatch.setattr(
        link_routes,
        "_resolve_host_port",
        lambda: "mylab.local:7070",
    )

    response = env.client.post(
        "/app/link/pair-start",
        json={"device_label": "Test Phone"},
    )

    assert response.status_code == 200
    payload = response.get_json()
    assert re.fullmatch(
        r"^https://link\.solpbc\.org/p#[0-9A-HJKMNP-TV-Z]+$",
        payload["pair_link"],
    )
    assert _decode_pair_link(payload["pair_link"])[0] == 0x03


def test_pair_start_spl_uses_thirty_second_expiry_and_nonce_ttl(link_env) -> None:
    env = link_env(posture="spl", totp_secret="GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ")

    response = env.client.post(
        "/app/link/pair-start",
        json={"device_label": "Test Phone"},
    )

    assert response.status_code == 200
    payload = response.get_json()
    snap = link_routes._nonces().snapshot()
    assert payload["expires_in"] == TOTP_STEP_SECONDS
    assert len(snap) == 1
    assert snap[0].expires_at - snap[0].issued_at == TOTP_STEP_SECONDS


def test_pair_start_spl_keeps_role_home_private(link_env, monkeypatch) -> None:
    env = link_env(posture="spl", totp_secret="GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ")
    monkeypatch.setattr(link_routes, "generate_relay_nonce", lambda: "00" * 16)

    response = env.client.post(
        "/app/link/pair-start",
        json={"device_label": "Observer", "role": "observer"},
    )

    assert response.status_code == 200
    payload = response.get_json()
    assert link_routes._nonces().snapshot()[0].role == "observer"
    assert b"observer" not in _decode_pair_link(payload["pair_link"])


def test_pair_start_spl_missing_totp_secret_errors_without_nonce(link_env) -> None:
    env = link_env(posture="spl")

    response = env.client.post(
        "/app/link/pair-start",
        json={"device_label": "Test Phone"},
    )

    assert response.status_code == 400
    payload = response.get_json()
    assert payload["reason_code"] == "invalid_operation_for_state"
    assert link_routes._nonces().snapshot() == []


def test_pair_start_spl_response_order_and_display_fingerprint(link_env) -> None:
    env = link_env(posture="spl", totp_secret="GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ")

    response = env.client.post(
        "/app/link/pair-start",
        json={"device_label": "Test Phone"},
    )

    assert response.status_code == 200
    payload = response.get_json()
    ca = load_or_generate_ca(ca_dir())
    assert list(payload.keys()) == PAIR_START_KEYS
    assert payload["ca_fingerprint"] == ca.fingerprint_sha256()
    assert payload["ca_fingerprint"] != ca.spki_fingerprint_sha256()
