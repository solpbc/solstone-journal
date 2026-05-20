# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Local CA management + home attestation minting.

Generates an ECDSA-P256 CA on first run, keeps the private key unencrypted
on disk with mode 0600 (no passphrase layer — filesystem perms + disk
encryption are the protection surface; see the spl-solstone-integration
spec "CA lifecycle" for rationale), signs mobile CSRs during the LAN pair
ceremony, and mints short-lived home-attestation JWTs that spl-relay
consumes at /enroll/device.

See github.com/solpbc/spl proto/pairing.md §"the local CA" and
proto/tokens.md §"POST /enroll/device" for the protocol contracts.
"""

from __future__ import annotations

import base64
import datetime as dt
import hashlib
import json
import secrets
import time
import uuid
from dataclasses import dataclass
from pathlib import Path

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import decode_dss_signature
from cryptography.x509.oid import NameOID

CA_VALIDITY_DAYS = 365 * 10
CLIENT_CERT_VALIDITY_DAYS = 365 * 10  # Revocation is via authorized_clients.json.
ATTESTATION_LIFETIME_SECONDS = 240  # 4 min — under the 5 min relay cap.


@dataclass(frozen=True)
class LoadedCa:
    """Materialized CA state: certificate, private key, and cached public-key PEM."""

    cert: x509.Certificate
    private_key: ec.EllipticCurvePrivateKey
    pubkey_spki_pem: str

    def fingerprint_sha256(self) -> str:
        """SHA-256 of the CA cert DER — used as the CA identifier at /enroll/home."""
        return _hex_sha256(self.cert.public_bytes(serialization.Encoding.DER))


def generate_ca(
    ca_dir: Path,
    common_name: str = "solstone link CA",
) -> LoadedCa:
    """Generate a fresh ECDSA-P256 CA and write it to disk.

    Writes `<ca_dir>/cert.pem` (world-readable) + `<ca_dir>/private.pem`
    (mode 0600). No passphrase — filesystem perms + disk encryption are
    the protection surface per the spec.
    """
    private_key = ec.generate_private_key(ec.SECP256R1())
    now = dt.datetime.now(dt.UTC)
    subject = issuer = x509.Name(
        [x509.NameAttribute(NameOID.COMMON_NAME, common_name)],
    )
    cert = (
        x509.CertificateBuilder()
        .subject_name(subject)
        .issuer_name(issuer)
        .public_key(private_key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(now - dt.timedelta(minutes=5))
        .not_valid_after(now + dt.timedelta(days=CA_VALIDITY_DAYS))
        .add_extension(x509.BasicConstraints(ca=True, path_length=0), critical=True)
        .add_extension(
            x509.KeyUsage(
                digital_signature=True,
                content_commitment=False,
                key_encipherment=False,
                data_encipherment=False,
                key_agreement=False,
                key_cert_sign=True,
                crl_sign=True,
                encipher_only=False,
                decipher_only=False,
            ),
            critical=True,
        )
        .sign(private_key, hashes.SHA256())
    )

    ca_dir.mkdir(parents=True, exist_ok=True)
    cert_path = _cert_path(ca_dir)
    key_path = _key_path(ca_dir)
    cert_path.write_bytes(cert.public_bytes(serialization.Encoding.PEM))
    _write_key(key_path, private_key)
    return _materialize(cert, private_key)


def load_ca(ca_dir: Path) -> LoadedCa:
    """Load a previously generated CA. Raises if either file is missing."""
    cert_pem = _cert_path(ca_dir).read_bytes()
    key_pem = _key_path(ca_dir).read_bytes()
    cert = x509.load_pem_x509_certificate(cert_pem)
    key = serialization.load_pem_private_key(key_pem, password=None)
    if not isinstance(key, ec.EllipticCurvePrivateKey):
        raise ValueError("CA key is not an ECDSA private key")
    return _materialize(cert, key)


def load_or_generate_ca(ca_dir: Path) -> LoadedCa:
    """Return an existing CA if present; otherwise generate a fresh one."""
    if _cert_path(ca_dir).exists() and _key_path(ca_dir).exists():
        return load_ca(ca_dir)
    return generate_ca(ca_dir)


def sign_csr(
    ca: LoadedCa,
    csr_pem: str | bytes,
    device_label: str,
) -> tuple[str, str]:
    """Sign a mobile CSR with the CA. Returns (client_cert_pem, fingerprint)."""
    csr_bytes = csr_pem.encode("utf-8") if isinstance(csr_pem, str) else csr_pem
    csr = x509.load_pem_x509_csr(csr_bytes)
    if not csr.is_signature_valid:
        raise ValueError("CSR signature invalid")
    pub = csr.public_key()
    if not isinstance(pub, ec.EllipticCurvePublicKey):
        raise ValueError("CSR public key must be ECDSA")
    if not isinstance(pub.curve, ec.SECP256R1):
        raise ValueError("CSR public key must be ECDSA-P256")

    now = dt.datetime.now(dt.UTC)
    cert = (
        x509.CertificateBuilder()
        .subject_name(
            x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, device_label)])
        )
        .issuer_name(ca.cert.subject)
        .public_key(pub)
        .serial_number(x509.random_serial_number())
        .not_valid_before(now - dt.timedelta(minutes=5))
        .not_valid_after(now + dt.timedelta(days=CLIENT_CERT_VALIDITY_DAYS))
        .add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True)
        .add_extension(
            x509.KeyUsage(
                digital_signature=True,
                content_commitment=False,
                key_encipherment=False,
                data_encipherment=False,
                key_agreement=False,
                key_cert_sign=False,
                crl_sign=False,
                encipher_only=False,
                decipher_only=False,
            ),
            critical=True,
        )
        .add_extension(
            x509.ExtendedKeyUsage([x509.oid.ExtendedKeyUsageOID.CLIENT_AUTH]),
            critical=False,
        )
        .sign(ca.private_key, hashes.SHA256())
    )
    pem = cert.public_bytes(serialization.Encoding.PEM).decode("ascii")
    fp = _hex_sha256(cert.public_bytes(serialization.Encoding.DER))
    return pem, f"sha256:{fp}"


def mint_attestation(
    ca: LoadedCa,
    instance_id: str,
    device_fp: str,
    *,
    now: int | None = None,
) -> str:
    """Mint an ES256 home-attestation JWT for /enroll/device.

    Shape is locked in github.com/solpbc/spl proto/tokens.md §"POST /enroll/device".
    """
    iat = now if now is not None else int(time.time())
    exp = iat + ATTESTATION_LIFETIME_SECONDS
    header = {"alg": "ES256", "typ": "home-attest"}
    claims = {
        "iss": f"home:{instance_id}",
        "aud": "spl-relay",
        "scope": "device.enroll",
        "instance_id": instance_id,
        "device_fp": device_fp,
        "iat": iat,
        "exp": exp,
        "jti": str(uuid.uuid4()),
    }
    header_b64 = _b64url(json.dumps(header, separators=(",", ":")).encode("utf-8"))
    payload_b64 = _b64url(json.dumps(claims, separators=(",", ":")).encode("utf-8"))
    signing_input = f"{header_b64}.{payload_b64}".encode("ascii")
    der_sig = ca.private_key.sign(signing_input, ec.ECDSA(hashes.SHA256()))
    r, s = decode_dss_signature(der_sig)
    raw_sig = r.to_bytes(32, "big") + s.to_bytes(32, "big")
    sig_b64 = _b64url(raw_sig)
    return f"{header_b64}.{payload_b64}.{sig_b64}"


def cert_fingerprint(cert_pem: str | bytes) -> str:
    """Compute `sha256:<hex>` over the DER form of a PEM-encoded cert."""
    pem_bytes = cert_pem.encode("utf-8") if isinstance(cert_pem, str) else cert_pem
    cert = x509.load_pem_x509_certificate(pem_bytes)
    return f"sha256:{_hex_sha256(cert.public_bytes(serialization.Encoding.DER))}"


def generate_nonce() -> str:
    """16-character hex nonce / 8 bytes for the pair ceremony."""
    return secrets.token_hex(8)


def _materialize(cert: x509.Certificate, key: ec.EllipticCurvePrivateKey) -> LoadedCa:
    pub_pem = (
        cert.public_key()
        .public_bytes(
            serialization.Encoding.PEM,
            serialization.PublicFormat.SubjectPublicKeyInfo,
        )
        .decode("ascii")
    )
    return LoadedCa(cert=cert, private_key=key, pubkey_spki_pem=pub_pem)


def _cert_path(ca_dir: Path) -> Path:
    return ca_dir / "cert.pem"


def _key_path(ca_dir: Path) -> Path:
    return ca_dir / "private.pem"


def _write_key(path: Path, key: ec.EllipticCurvePrivateKey) -> None:
    encoded = key.private_bytes(
        encoding=serialization.Encoding.PEM,
        format=serialization.PrivateFormat.PKCS8,
        encryption_algorithm=serialization.NoEncryption(),
    )
    path.write_bytes(encoded)
    path.chmod(0o600)


def _hex_sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode("ascii")
