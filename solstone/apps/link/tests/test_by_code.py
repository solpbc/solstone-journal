# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Regression tests for manual-code link pairing."""

from __future__ import annotations

import re

import pytest
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import NameOID

from solstone.apps.link import routes as link_routes

PAIR_RESPONSE_KEYS = {
    "client_cert",
    "ca_chain",
    "instance_id",
    "home_label",
    "home_attestation",
    "fingerprint",
}


def _make_csr(label: str = "test") -> str:
    key = ec.generate_private_key(ec.SECP256R1())
    csr = (
        x509.CertificateSigningRequestBuilder()
        .subject_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, label)]))
        .sign(key, hashes.SHA256())
    )
    return csr.public_bytes(serialization.Encoding.PEM).decode("ascii")


def _start_pair(env, device_label: str = "Test Phone") -> dict:
    response = env.client.post(
        "/app/link/pair-start",
        json={"device_label": device_label},
    )
    assert response.status_code == 200
    return response.get_json()


def _assert_pair_response(payload: dict) -> None:
    assert PAIR_RESPONSE_KEYS <= set(payload.keys())
    assert set(payload.keys()) <= PAIR_RESPONSE_KEYS | {"local_endpoints"}
    assert payload["client_cert"].startswith("-----BEGIN CERTIFICATE-----")
    assert isinstance(payload["ca_chain"], list)
    assert payload["ca_chain"]
    assert payload["fingerprint"].startswith("sha256:")


def test_by_code_happy_path(link_env) -> None:
    env = link_env()
    started = _start_pair(env)

    response = env.client.post(
        "/app/link/by-code",
        json={"code": started["manual_code"], "csr": _make_csr()},
    )

    assert response.status_code == 200
    _assert_pair_response(response.get_json())


@pytest.mark.parametrize("variant", ["lowercase", "no_hyphen", "whitespace"])
def test_by_code_tolerates_normalized_code_variants(link_env, variant: str) -> None:
    env = link_env()
    started = _start_pair(env)
    code = started["manual_code"]
    if variant == "lowercase":
        code = code.lower()
    elif variant == "no_hyphen":
        code = code.replace("-", "")
    elif variant == "whitespace":
        code = f"  {code}  "

    response = env.client.post(
        "/app/link/by-code",
        json={"code": code, "csr": _make_csr()},
    )

    assert response.status_code == 200


def test_by_code_consumption_blocks_pair_token(link_env) -> None:
    env = link_env()
    started = _start_pair(env)

    by_code = env.client.post(
        "/app/link/by-code",
        json={"code": started["manual_code"], "csr": _make_csr("by-code")},
    )
    assert by_code.status_code == 200

    by_token = env.client.post(
        f"/app/link/pair?token={started['nonce']}",
        json={"csr": _make_csr("by-token")},
    )

    assert by_token.status_code == 410
    payload = by_token.get_json()
    assert (
        payload["error"]
        == "I couldn't finish because that action is no longer available."
    )
    assert payload["reason_code"] == "operation_no_longer_available"
    assert payload["detail"] == "nonce expired or used"


def test_pair_token_consumption_blocks_by_code(link_env) -> None:
    env = link_env()
    started = _start_pair(env)

    by_token = env.client.post(
        f"/app/link/pair?token={started['nonce']}",
        json={"csr": _make_csr("by-token")},
    )
    assert by_token.status_code == 200

    by_code = env.client.post(
        "/app/link/by-code",
        json={"code": started["manual_code"], "csr": _make_csr("by-code")},
    )

    assert by_code.status_code == 410
    payload = by_code.get_json()
    assert (
        payload["error"]
        == "I couldn't finish because that action is no longer available."
    )
    assert payload["reason_code"] == "operation_no_longer_available"
    assert payload["detail"] == "nonce expired or used"


def test_by_code_bad_manual_code_format_returns_400(link_env) -> None:
    env = link_env()

    response = env.client.post(
        "/app/link/by-code",
        json={"code": "??", "csr": _make_csr()},
    )

    assert response.status_code == 400


@pytest.mark.parametrize("manual_code", ["0000-0000", "1111-1111"])
def test_by_code_accepts_crockford_zero_and_one(
    link_env,
    monkeypatch: pytest.MonkeyPatch,
    manual_code: str,
) -> None:
    env = link_env()
    monkeypatch.setattr(link_routes, "generate_manual_code", lambda: manual_code)
    started = _start_pair(env)

    response = env.client.post(
        "/app/link/by-code",
        json={"code": started["manual_code"], "csr": _make_csr("by-code")},
    )

    assert response.status_code == 200


@pytest.mark.parametrize("manual_code", ["IIII-IIII", "LLLL-LLLL"])
def test_by_code_rejects_non_crockford_ambiguous_letters(
    link_env,
    manual_code: str,
) -> None:
    env = link_env()

    response = env.client.post(
        "/app/link/by-code",
        json={"code": manual_code, "csr": _make_csr("by-code")},
    )

    assert response.status_code == 400
    payload = response.get_json()
    assert payload["reason_code"] == "pairing_request_invalid"
    assert payload["detail"] == "bad code"


@pytest.mark.parametrize("route", ["pair", "by-code"])
def test_successful_pairing_emits_pair_complete_once(
    link_env,
    monkeypatch: pytest.MonkeyPatch,
    route: str,
) -> None:
    env = link_env()
    calls = []

    def mock_emit(*args, **kwargs):
        calls.append((args, kwargs))
        return True

    monkeypatch.setattr("solstone.apps.link.routes.emit", mock_emit)
    started = _start_pair(env, "Emit Phone")
    if route == "pair":
        response = env.client.post(
            f"/app/link/pair?token={started['nonce']}",
            json={"csr": _make_csr("emit-pair")},
        )
    else:
        response = env.client.post(
            "/app/link/by-code",
            json={"code": started["manual_code"], "csr": _make_csr("emit-code")},
        )

    assert response.status_code == 200
    assert len(calls) == 1
    args, kwargs = calls[0]
    assert args == ("link", "pair_complete")
    assert kwargs["device_label"] == "Emit Phone"
    assert kwargs["fingerprint"].startswith("sha256:")
    assert re.fullmatch(r"[a-f0-9]{16}", kwargs["fingerprint_short"])
    assert re.fullmatch(
        r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z",
        kwargs["paired_at"],
    )
