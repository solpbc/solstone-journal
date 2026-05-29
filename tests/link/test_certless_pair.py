# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
from pathlib import Path

import pytest

from solstone.think.link.auth import AuthorizedClients
from solstone.think.link.client import _build_csr
from solstone.think.link.nonces import NonceStore
from solstone.think.link.paths import authorized_clients_path, nonces_path
from tests.link.certless_helpers import (
    certless_identity,
    dispatch_request,
    make_convey_app,
)


@pytest.mark.asyncio
async def test_certless_pair_request_executes_handler_and_authorizes_client(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    app, _journal = make_convey_app(tmp_path, monkeypatch, link={"posture": "spl"})
    nonce = "0123456789abcdef"
    device_label = "pytest certless phone"
    NonceStore(nonces_path()).add(nonce, device_label)
    _private_key_pem, csr_pem = _build_csr(device_label)
    body = json.dumps(
        {
            "nonce": nonce,
            "csr": csr_pem,
            "device_label": device_label,
        }
    ).encode("utf-8")

    response = await dispatch_request(
        app,
        certless_identity(),
        "POST",
        "/app/link/pair",
        body=body,
        headers={"content-type": "application/json"},
    )

    assert response.status == 200
    payload = json.loads(response.body)
    fingerprint = payload["fingerprint"]
    assert fingerprint.startswith("sha256:")
    assert payload["client_cert"].startswith("-----BEGIN CERTIFICATE-----")
    assert AuthorizedClients(authorized_clients_path()).is_authorized(fingerprint)


@pytest.mark.asyncio
async def test_certless_identity_is_refused_at_non_pair_route(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    app, _journal = make_convey_app(tmp_path, monkeypatch, link={"posture": "spl"})
    NonceStore(nonces_path()).add("fedcba9876543210", "phone")

    response = await dispatch_request(
        app,
        certless_identity(),
        "GET",
        "/app/link/api/status",
    )

    assert response.status == 403
    assert b"pairing tunnel may only use /app/link/pair" in response.body
