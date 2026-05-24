# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import pytest
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import NameOID

from solstone.apps.link import routes as link_routes


def _make_csr(label: str = "test") -> str:
    key = ec.generate_private_key(ec.SECP256R1())
    csr = (
        x509.CertificateSigningRequestBuilder()
        .subject_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, label)]))
        .sign(key, hashes.SHA256())
    )
    return csr.public_bytes(serialization.Encoding.PEM).decode("ascii")


def _start_pair(env, role: str) -> dict:
    response = env.client.post(
        "/app/link/pair-start",
        json={"device_label": "Role Device", "role": role},
    )
    assert response.status_code == 200
    return response.get_json()


@pytest.mark.parametrize("role", ["phone", "observer", "peer"])
def test_pair_route_persists_consumed_nonce_role(link_env, role: str) -> None:
    env = link_env()
    started = _start_pair(env, role)

    response = env.client.post(
        "/app/link/pair",
        json={"nonce": started["nonce"], "csr": _make_csr(role)},
    )

    assert response.status_code == 200
    entries = link_routes._authorized().snapshot()
    assert len(entries) == 1
    assert entries[0].role == role


@pytest.mark.parametrize("role", ["phone", "observer", "peer"])
def test_by_code_route_persists_consumed_nonce_role(link_env, role: str) -> None:
    env = link_env()
    started = _start_pair(env, role)

    response = env.client.post(
        "/app/link/by-code",
        json={"code": started["manual_code"], "csr": _make_csr(role)},
    )

    assert response.status_code == 200
    entries = link_routes._authorized().snapshot()
    assert len(entries) == 1
    assert entries[0].role == role
