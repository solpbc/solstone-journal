# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""pyOpenSSL memory-BIO adapter for Convey's secure PL listener.

The secure listener terminates TLS 1.3 inside the Convey process before
feeding plaintext into the mux and inline WSGI dispatcher. pyOpenSSL's
`SSL.Connection` supports memory-BIO mode: the caller pushes ciphertext in
with `bio_write`, pulls ciphertext out with `bio_read`, and reads/writes
plaintext with `recv`/`send`.

This module also installs the pinned verify callback — the load-bearing
reason we use pyOpenSSL and not stdlib `ssl` (stdlib doesn't expose a
handshake-time callback that can reject a cert with a clean TLS alert).
"""

from __future__ import annotations

import hashlib
from collections.abc import Callable
from dataclasses import dataclass

from cryptography import x509
from cryptography.hazmat.primitives import serialization
from OpenSSL import SSL, crypto

from solstone.think.link.auth import AuthorizedClients
from solstone.think.link.ca import LoadedCa


class TlsError(RuntimeError):
    """Raised when the TLS handshake is aborted (e.g., fingerprint rejected)."""


@dataclass
class TlsServerState:
    conn: SSL.Connection
    handshake_done: bool = False
    peer_fingerprint: str | None = None


def build_server_context(
    ca: LoadedCa,
    server_cert: x509.Certificate,
    server_key: bytes,
    authorized: AuthorizedClients,
) -> SSL.Context:
    """Build a TLS 1.3 server context with the pinned verify callback."""
    return _build_server_context(
        ca,
        server_cert,
        server_key,
        authorized,
        SSL.VERIFY_PEER | SSL.VERIFY_FAIL_IF_NO_PEER_CERT,
    )


def build_relaxed_server_context(
    ca: LoadedCa,
    server_cert: x509.Certificate,
    server_key: bytes,
    authorized: AuthorizedClients,
) -> SSL.Context:
    """Build a TLS 1.3 server context that allows missing peer certs."""
    return _build_server_context(
        ca,
        server_cert,
        server_key,
        authorized,
        SSL.VERIFY_PEER,
    )


def _build_server_context(
    ca: LoadedCa,
    server_cert: x509.Certificate,
    server_key: bytes,
    authorized: AuthorizedClients,
    verify_flags: int,
) -> SSL.Context:
    ctx = SSL.Context(SSL.TLS_METHOD)
    ctx.set_min_proto_version(SSL.TLS1_3_VERSION)
    ctx.set_max_proto_version(SSL.TLS1_3_VERSION)
    ctx.use_certificate(
        crypto.X509.from_cryptography(server_cert),
    )
    ctx.use_privatekey(crypto.load_privatekey(crypto.FILETYPE_PEM, server_key))
    ctx.add_extra_chain_cert(crypto.X509.from_cryptography(ca.cert))
    store = ctx.get_cert_store()
    assert store is not None, "pyOpenSSL context must expose a cert store"
    store.add_cert(crypto.X509.from_cryptography(ca.cert))

    ctx.set_verify(verify_flags, _make_verify_cb(authorized))
    return ctx


def _make_verify_cb(
    authorized: AuthorizedClients,
) -> Callable[[SSL.Connection, crypto.X509, int, int, int], bool]:
    def verify_cb(
        _conn: SSL.Connection,
        cert: crypto.X509,
        _errno: int,
        depth: int,
        preverify_ok: int,
    ) -> bool:
        if not preverify_ok:
            return False
        if depth != 0:
            return True
        der = cert.to_cryptography().public_bytes(serialization.Encoding.DER)
        fp = f"sha256:{hashlib.sha256(der).hexdigest()}"
        return authorized.is_authorized(fp)

    return verify_cb


def new_server(ctx: SSL.Context) -> TlsServerState:
    """Fresh memory-BIO connection in accept state."""
    conn = SSL.Connection(ctx, None)
    conn.set_accept_state()
    return TlsServerState(conn=conn)


def drive_tls(
    state: TlsServerState,
    *,
    inbound: bytes,
    plaintext_out: bytes = b"",
) -> tuple[bytes, bytes]:
    """Push ciphertext in + plaintext out; return (ciphertext_to_send, plaintext_received)."""
    if inbound:
        state.conn.bio_write(inbound)
    if plaintext_out:
        try:
            state.conn.send(plaintext_out)
        except SSL.WantReadError:
            pass

    if not state.handshake_done:
        try:
            state.conn.do_handshake()
            state.handshake_done = True
            peer = state.conn.get_peer_certificate()
            if peer is not None:
                der = peer.to_cryptography().public_bytes(
                    serialization.Encoding.DER,
                )
                state.peer_fingerprint = f"sha256:{hashlib.sha256(der).hexdigest()}"
        except SSL.WantReadError:
            pass
        except SSL.Error as exc:
            raise TlsError(f"handshake failed: {exc}") from exc

    plaintext_in = bytearray()
    if state.handshake_done:
        while True:
            try:
                chunk = state.conn.recv(16 * 1024)
            except SSL.WantReadError:
                break
            except SSL.ZeroReturnError:
                break
            if not chunk:
                break
            plaintext_in.extend(chunk)

    outbound = bytearray()
    while True:
        try:
            chunk = state.conn.bio_read(16 * 1024)
        except SSL.WantReadError:
            break
        if not chunk:
            break
        outbound.extend(chunk)
    return bytes(outbound), bytes(plaintext_in)


def issue_server_cert(
    ca: LoadedCa,
    common_name: str = "solstone link",
) -> tuple[x509.Certificate, bytes]:
    """Mint a server cert (signed by the CA) + its PEM-encoded private key.

    Regenerated on each start — server-side TLS material doesn't need to
    survive restarts since the mobile pins the *CA* fingerprint, not the
    server cert.
    """
    import datetime as dt

    from cryptography.hazmat.primitives import hashes
    from cryptography.hazmat.primitives.asymmetric import ec
    from cryptography.x509.oid import NameOID

    key = ec.generate_private_key(ec.SECP256R1())
    now = dt.datetime.now(dt.UTC)
    cert = (
        x509.CertificateBuilder()
        .subject_name(
            x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, common_name)]),
        )
        .issuer_name(ca.cert.subject)
        .public_key(key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(now - dt.timedelta(minutes=5))
        .not_valid_after(now + dt.timedelta(days=30))
        .add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True)
        .add_extension(
            x509.ExtendedKeyUsage([x509.oid.ExtendedKeyUsageOID.SERVER_AUTH]),
            critical=False,
        )
        .sign(ca.private_key, hashes.SHA256())
    )
    key_pem = key.private_bytes(
        serialization.Encoding.PEM,
        serialization.PrivateFormat.PKCS8,
        serialization.NoEncryption(),
    )
    return cert, key_pem


__all__ = [
    "TlsError",
    "TlsServerState",
    "build_relaxed_server_context",
    "build_server_context",
    "drive_tls",
    "issue_server_cert",
    "new_server",
]
