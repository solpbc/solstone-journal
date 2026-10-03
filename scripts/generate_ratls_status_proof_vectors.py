#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Generate the RA-TLS status-proof v1 vectors and synthetic exchange fixtures.

The journal's Rust codec is the verifier of record; these files are produced
independently, here, with Python and `cryptography`, so the Rust tests and a
later gateway implementation check one contract from two implementations.

    uv run --no-project --with cryptography==46.0.7 python3 \
        scripts/generate_ratls_status_proof_vectors.py

Writes:

- `core/fixtures/ratls-status-proofs-v1-vectors.json`: valid and invalid
  extension values with their expected decoding.
- `tests/fixtures/spp_attest/status-proofs/exchange/`: a gateway TLS key, an
  AK key for exporter quotes, and certificates carrying the composite evidence
  extension and the status-proof extension in valid and invalid shapes.

Keys are generated fresh on every run, so a rerun rewrites the exchange
fixtures. The vectors are deterministic apart from the real NVIDIA bundle,
which is read from `tests/fixtures/spp_attest/status-proofs/nvidia/`.
"""

from __future__ import annotations

import datetime as dt
import hashlib
import json
import pathlib

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec, rsa
from cryptography.x509.oid import NameOID

ROOT = pathlib.Path(__file__).resolve().parents[1]
CONTRACT = ROOT / "core/fixtures/ratls-contract.json"
VECTORS = ROOT / "core/fixtures/ratls-status-proofs-v1-vectors.json"
SPP = ROOT / "tests/fixtures/spp_attest"
NVIDIA_BUNDLE = SPP / "status-proofs/nvidia/bundle.der"
EXCHANGE = SPP / "status-proofs/exchange"


# An independent DER writer; the Rust side has its own reader.
def der_len(length: int) -> bytes:
    if length < 0x80:
        return bytes([length])
    raw = length.to_bytes((length.bit_length() + 7) // 8, "big")
    return bytes([0x80 | len(raw)]) + raw


def tlv(tag: int, value: bytes) -> bytes:
    return bytes([tag]) + der_len(len(value)) + value


def integer(value: int) -> bytes:
    raw = value.to_bytes(max(1, (value.bit_length() + 8) // 8), "big", signed=True)
    return tlv(0x02, raw)


def bundle(responses: list[bytes], version: int = 1) -> bytes:
    return tlv(0x30, integer(version) + tlv(0x30, b"".join(tlv(0x04, r) for r in responses)))


def vectors(contract: dict, real: bytes) -> list[dict]:
    limits = contract["status_proofs"]["limits"]
    max_response = limits["max_response_bytes"]
    max_responses = limits["max_responses"]
    max_total = limits["max_extension_value_bytes"]
    small = bytes.fromhex("3003020101")  # opaque to the codec
    long_response = b"\x30" + b"\x00" * (max_response - 1)
    cases = [
        ("one_response", bundle([small]), 1),
        ("max_responses", bundle([small] * max_responses), max_responses),
        ("max_response_bytes", bundle([long_response]), 1),
        ("nvidia_h100_inventory", real, 8),
        ("version_2", bundle([small], version=2), None),
        ("version_0", bundle([small], version=0), None),
        ("empty_responses", bundle([]), None),
        ("too_many_responses", bundle([small] * (max_responses + 1)), None),
        ("response_too_long", bundle([long_response + b"\x00"]), None),
        ("empty_response", bundle([b""]), None),
        ("trailing_byte", bundle([small]) + b"\x00", None),
        ("version_non_minimal_integer", tlv(0x30, tlv(0x02, b"\x00\x01") + tlv(0x30, tlv(0x04, small))), None),
        ("version_non_minimal_length", tlv(0x30, b"\x02\x81\x01\x01" + tlv(0x30, tlv(0x04, small))), None),
        ("indefinite_length", b"\x30\x80" + integer(1) + tlv(0x30, tlv(0x04, small)) + b"\x00\x00", None),
        ("item_not_octet_string", tlv(0x30, integer(1) + tlv(0x30, tlv(0x30, small))), None),
        ("extra_field", tlv(0x30, integer(1) + tlv(0x30, tlv(0x04, small)) + tlv(0x05, b"")), None),
        ("truncated", bundle([small])[:-1], None),
        ("not_a_sequence", tlv(0x31, integer(1) + tlv(0x30, tlv(0x04, small))), None),
    ]
    oversized = bundle([long_response] * (max_total // max_response + 1))
    assert len(oversized) > max_total
    cases.append(("total_too_long", oversized, None))
    return [
        {
            "name": name,
            "valid": count is not None,
            "response_count": count,
            "bytes": len(value),
            "sha256": hashlib.sha256(value).hexdigest(),
            "hex": value.hex(),
        }
        for name, value, count in cases
    ]


def oid_arcs(oid: str) -> x509.ObjectIdentifier:
    return x509.ObjectIdentifier(oid)


def composite_evidence(spki: bytes, ak_public_pem: bytes) -> bytes:
    nonce = bytes.fromhex("".join((SPP / "nonce.hex").read_text().split()))
    fields = [
        nonce,
        spki,
        (SPP / "report.bin").read_bytes(),
        (SPP / "hcl_report.bin").read_bytes(),
        ak_public_pem,
        (SPP / "quote.msg").read_bytes(),
        (SPP / "quote.sig").read_bytes(),
        (SPP / "quote.pcrs").read_bytes(),
        (SPP / "certs/ark.pem").read_bytes(),
        (SPP / "certs/ask.pem").read_bytes(),
        (SPP / "certs/vcek.pem").read_bytes(),
        (SPP / "gpu-envelope.tlv").read_bytes(),
    ]
    return tlv(0x30, integer(1) + b"".join(tlv(0x04, field) for field in fields))


def certificate(key, composite_oid: str, composite: bytes, proofs_oid: str, proofs: bytes | None, *, proofs_critical=False) -> bytes:
    now = dt.datetime(2026, 10, 1, tzinfo=dt.timezone.utc)
    builder = (
        x509.CertificateBuilder()
        .subject_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "spp-engine")]))
        .issuer_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "spp-engine")]))
        .public_key(key.public_key())
        .serial_number(1)
        .not_valid_before(now)
        .not_valid_after(now + dt.timedelta(days=3650))
        .add_extension(x509.UnrecognizedExtension(oid_arcs(composite_oid), composite), critical=True)
    )
    if proofs is not None:
        builder = builder.add_extension(
            x509.UnrecognizedExtension(oid_arcs(proofs_oid), proofs), critical=proofs_critical
        )
    return builder.sign(key, hashes.SHA256()).public_bytes(serialization.Encoding.DER)


def main() -> None:
    contract = json.loads(CONTRACT.read_text())
    real = NVIDIA_BUNDLE.read_bytes()
    VECTORS.write_text(
        json.dumps(
            {
                "contract": "core/fixtures/ratls-contract.json#status_proofs",
                "generator": "scripts/generate_ratls_status_proof_vectors.py",
                "cryptography": __import__("cryptography").__version__,
                "vectors": vectors(contract, real),
            },
            indent=2,
        )
        + "\n"
    )

    EXCHANGE.mkdir(parents=True, exist_ok=True)
    tls_key = ec.generate_private_key(ec.SECP256R1())
    ak_key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    pkcs8 = serialization.PrivateFormat.PKCS8
    (EXCHANGE / "tls-key.pem").write_bytes(
        tls_key.private_bytes(serialization.Encoding.PEM, pkcs8, serialization.NoEncryption())
    )
    (EXCHANGE / "ak-key.pem").write_bytes(
        ak_key.private_bytes(serialization.Encoding.PEM, pkcs8, serialization.NoEncryption())
    )
    ak_public = ak_key.public_key().public_bytes(
        serialization.Encoding.PEM, serialization.PublicFormat.SubjectPublicKeyInfo
    )
    spki = tls_key.public_key().public_bytes(
        serialization.Encoding.DER, serialization.PublicFormat.SubjectPublicKeyInfo
    )
    composite_oid = contract["x509_extension"]["oid"]
    proofs_oid = contract["status_proofs"]["oid"]
    evidence = composite_evidence(spki, ak_public)
    shapes = {
        "certificate.der": dict(proofs=real),
        "certificate-without-proofs.der": dict(proofs=None),
        "certificate-proofs-critical.der": dict(proofs=real, proofs_critical=True),
        "certificate-proofs-version-2.der": dict(proofs=bundle([b"\x30\x00"], version=2)),
        "certificate-proofs-trailing.der": dict(proofs=real + b"\x00"),
    }
    receipt = {}
    for name, shape in shapes.items():
        der = certificate(tls_key, composite_oid, evidence, proofs_oid, **shape)
        (EXCHANGE / name).write_bytes(der)
        receipt[name] = {"bytes": len(der), "sha256": hashlib.sha256(der).hexdigest()}
    receipt["composite_evidence_bytes"] = len(evidence)
    receipt["nvidia_bundle_bytes"] = len(real)
    (EXCHANGE / "receipt.json").write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
