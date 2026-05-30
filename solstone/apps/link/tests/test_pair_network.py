# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Pairing-network persistence and event labeling."""

from __future__ import annotations

import pytest
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import NameOID

from solstone.apps.link import routes as link_routes
from solstone.convey.secure_listener.identity import ConveyIdentity


def _make_csr(label: str = "network-test") -> str:
    key = ec.generate_private_key(ec.SECP256R1())
    csr = (
        x509.CertificateSigningRequestBuilder()
        .subject_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, label)]))
        .sign(key, hashes.SHA256())
    )
    return csr.public_bytes(serialization.Encoding.PEM).decode("ascii")


def _start_pair(env) -> dict:
    response = env.client.post(
        "/app/link/pair-start",
        json={"device_label": "Network Device"},
    )
    assert response.status_code == 200
    return response.get_json()


def _spl_identity() -> ConveyIdentity:
    return ConveyIdentity(
        mode="pl-via-spl",
        fingerprint=None,
        device_label=None,
        paired_at=None,
        session_id="test-certless",
    )


def _assert_network_result(env, calls, expected_network: str, response) -> None:
    assert response.status_code == 200
    assert "network" not in response.get_json()

    entries = link_routes._authorized().snapshot()
    assert len(entries) == 1
    assert entries[0].network == expected_network

    devices_response = env.client.get("/app/link/api/devices")
    assert devices_response.status_code == 200
    devices = devices_response.get_json()["devices"]
    assert len(devices) == 1
    assert devices[0]["network"] == expected_network

    assert len(calls) == 1
    assert calls[0][0] == ("link", "pair_complete")
    assert calls[0][1]["network"] == expected_network


@pytest.mark.parametrize(
    ("environ_overrides", "expected_network"),
    [
        (None, "network"),
        ({"pl.identity": _spl_identity()}, "anywhere"),
    ],
)
def test_pair_route_network_persists_to_devices_and_pair_complete_event(
    link_env,
    monkeypatch: pytest.MonkeyPatch,
    environ_overrides: dict[str, ConveyIdentity] | None,
    expected_network: str,
) -> None:
    env = link_env()
    calls = []

    def mock_emit(*args, **kwargs):
        calls.append((args, kwargs))
        return True

    monkeypatch.setattr(link_routes, "emit", mock_emit)
    started = _start_pair(env)
    post_kwargs = {}
    if environ_overrides is not None:
        post_kwargs["environ_overrides"] = environ_overrides
    response = env.client.post(
        f"/app/link/pair?token={started['nonce']}",
        json={"csr": _make_csr("pair-network")},
        **post_kwargs,
    )

    _assert_network_result(env, calls, expected_network, response)


def test_by_code_route_network_defaults_to_on_network(
    link_env,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    env = link_env()
    calls = []

    def mock_emit(*args, **kwargs):
        calls.append((args, kwargs))
        return True

    monkeypatch.setattr(link_routes, "emit", mock_emit)
    started = _start_pair(env)

    response = env.client.post(
        "/app/link/by-code",
        json={"code": started["manual_code"], "csr": _make_csr("by-code-network")},
    )

    _assert_network_result(env, calls, "network", response)


def test_by_code_certless_pairing_is_confined(link_env) -> None:
    env = link_env()
    started = _start_pair(env)

    # certless_target_allowed confines cert-less pl-via-spl pairing to /pair.
    response = env.client.post(
        "/app/link/by-code",
        json={"code": started["manual_code"], "csr": _make_csr("by-code-confined")},
        environ_overrides={"pl.identity": _spl_identity()},
    )

    assert response.status_code == 403
