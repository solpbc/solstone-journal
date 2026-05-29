# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from pathlib import Path

import pytest
from cryptography.hazmat.primitives import serialization
from OpenSSL import SSL, crypto

from solstone.convey.secure_listener.tls import (
    TlsError,
    build_relaxed_server_context,
    build_server_context,
    drive_tls,
    issue_server_cert,
    new_server,
)
from solstone.think.link.auth import AuthorizedClients
from solstone.think.link.ca import LoadedCa, generate_ca, sign_csr
from solstone.think.link.client import (
    ClientIdentity,
    _build_csr,
    _build_tls_client_ctx,
    _drive_tls_client,
    _new_tls_client,
)
from solstone.think.link.tls import TlsError as ClientTlsError


def test_strict_context_rejects_no_cert(tmp_path: Path) -> None:
    ca, server_cert, server_key, authorized = _server_material(tmp_path)
    server_ctx = build_server_context(ca, server_cert, server_key, authorized)
    client_ctx = _build_no_cert_client_ctx(ca)

    with pytest.raises((TlsError, ClientTlsError)):
        _complete_handshake(server_ctx, client_ctx)


def test_relaxed_context_accepts_no_cert_with_none_fingerprint(tmp_path: Path) -> None:
    ca, server_cert, server_key, authorized = _server_material(tmp_path)
    server_ctx = build_relaxed_server_context(ca, server_cert, server_key, authorized)
    client_ctx = _build_no_cert_client_ctx(ca)

    server = _complete_handshake(server_ctx, client_ctx)

    assert server.peer_fingerprint is None


def test_relaxed_context_keeps_allowlisted_cert_fingerprint(tmp_path: Path) -> None:
    ca, server_cert, server_key, authorized = _server_material(tmp_path)
    private_key_pem, csr_pem = _build_csr("pytest phone")
    client_cert_pem, fingerprint = sign_csr(ca, csr_pem, "pytest phone")
    authorized.add(fingerprint, "pytest phone", "inst-1")
    server_ctx = build_relaxed_server_context(ca, server_cert, server_key, authorized)
    client_ctx = _build_tls_client_ctx(
        ClientIdentity(
            private_key_pem=private_key_pem,
            client_cert_pem=client_cert_pem,
            ca_chain_pem=ca.cert.public_bytes(serialization.Encoding.PEM).decode(
                "ascii"
            ),
            fingerprint=fingerprint,
            home_instance_id="inst-1",
            home_label="home",
            home_attestation="attest",
        )
    )

    server = _complete_handshake(server_ctx, client_ctx)

    assert server.peer_fingerprint == fingerprint


def _server_material(
    tmp_path: Path,
) -> tuple[LoadedCa, object, bytes, AuthorizedClients]:
    ca = generate_ca(tmp_path / "ca")
    server_cert, server_key = issue_server_cert(ca)
    authorized = AuthorizedClients(tmp_path / "authorized_clients.json")
    return ca, server_cert, server_key, authorized


def _build_no_cert_client_ctx(ca: LoadedCa) -> SSL.Context:
    ctx = SSL.Context(SSL.TLS_METHOD)
    ctx.set_min_proto_version(SSL.TLS1_3_VERSION)
    ctx.set_max_proto_version(SSL.TLS1_3_VERSION)
    store = ctx.get_cert_store()
    assert store is not None
    store.add_cert(crypto.X509.from_cryptography(ca.cert))
    ctx.set_verify(SSL.VERIFY_PEER, lambda *_args: True)
    return ctx


def _complete_handshake(server_ctx: SSL.Context, client_ctx: SSL.Context):
    server = new_server(server_ctx)
    client = _new_tls_client(client_ctx)
    client_to_server = b""
    server_to_client = b""

    for _ in range(100):
        client_out, _ = _drive_tls_client(client, inbound=server_to_client)
        server_to_client = b""
        client_to_server += client_out

        server_out, _ = drive_tls(server, inbound=client_to_server)
        client_to_server = b""
        server_to_client += server_out

        if server.handshake_done and client.handshake_done:
            return server

    raise AssertionError("TLS handshake did not complete")
