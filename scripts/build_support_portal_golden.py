#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Mint the read-old golden for the native support portal client.

The native client must load a `keypair.pem` and `token.json` an existing install
already has, derive the same JWK and the same RFC 7638 thumbprint from them, and
produce DPoP proofs the portal still verifies. **Write new, read old.**

🔴 The reference client cannot be the live oracle for that, for two reasons that
are often confused. It is not that a gate forbids running Python — this tree has
a purpose-built cross-language differential rail for exactly that. It is that
**the reference is being deleted**, and a differential dies with it. A committed
golden is executable forever, and it is executable inside the poisoned gate where
no interpreter resolves at all.

⚠ Everything here is a freshly generated throwaway with an explicit
non-production label, following the naming convention already used by
`core/fixtures/convey_network_corpus_ca_nonproduction/`.

⛔ This script is BUILD-TIME tooling. It is run once, by hand, on a machine with
the reference tree installed; its output is committed and the script is kept only
so the provenance of those bytes is checkable rather than asserted.

Usage:
    bwrap --unshare-net --dev-bind / / -- python3 scripts/build_support_portal_golden.py
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
OUT = REPO_ROOT / "core" / "fixtures" / "support_portal_golden_nonproduction"

# Authored, so nothing here is a fact about the generating machine.
HANDLE = "solstone-golden-nonproduction"
ACCESS_TOKEN_TOS = "Golden terms of service. Authored for this fixture; not a real agreement.\n"
DPOP_METHOD = "POST"
DPOP_URL = "https://support.example.invalid/api/tickets?status=open#frag"
# A fixed instant, so `iat` is reproducible and the vectors are stable.
PINNED_IAT = 1767225600
# A fixed message the native side signs with `ring` and compares byte for byte.
SIGNATURE_PROBE = b"golden interop probe"


def main() -> int:
    from cryptography.hazmat.primitives.asymmetric import rsa

    from solstone.apps.support import portal

    OUT.mkdir(parents=True, exist_ok=True)

    client = portal.PortalClient(
        portal_url="https://support.example.invalid",
        storage_dir=OUT,
        handle=HANDLE,
    )
    # Generate through the reference's own path so the PEM encoding, the JWK
    # derivation and the thumbprint are all the reference's, not this script's.
    client._ensure_keypair()
    assert isinstance(client._private_key, rsa.RSAPrivateKey)

    client._access_token = client._create_access_token(ACCESS_TOKEN_TOS)
    client._save_token()
    client._save_tos(ACCESS_TOKEN_TOS)

    # Freeze `iat` and `jti` so the proof and the token are byte-reproducible.
    real_time = portal.time.time
    real_uuid4 = portal.uuid.uuid4

    class _FixedUuid:
        def __init__(self, value: str) -> None:
            self._value = value

        def __str__(self) -> str:
            return self._value

    portal.time.time = lambda: float(PINNED_IAT)  # type: ignore[assignment]
    portal.uuid.uuid4 = lambda: _FixedUuid("00000000-0000-4000-8000-000000000000")  # type: ignore[assignment]
    try:
        access_token = client._create_access_token(ACCESS_TOKEN_TOS)
        proof_unauthed = client._create_dpop_proof(DPOP_METHOD, DPOP_URL)
        proof_authed = client._create_dpop_proof(DPOP_METHOD, DPOP_URL, access_token)
        # 🔴 `htu` strips the QUERY and does NOT strip a bare fragment, even though
        # the reference's own inline comment says query/fragment. Both vectors are
        # recorded so a port cannot pass by getting one of them right.
        proof_fragment_only = client._create_dpop_proof(
            DPOP_METHOD, "https://support.example.invalid/api/tickets#frag"
        )
        tos_signature = client._sign_tos(ACCESS_TOKEN_TOS)
    finally:
        portal.time.time = real_time  # type: ignore[assignment]
        portal.uuid.uuid4 = real_uuid4  # type: ignore[assignment]

    vectors = {
        "schema": "solstone-support-portal-golden-v1",
        "generator": "scripts/build_support_portal_golden.py",
        "label": "NON-PRODUCTION. A freshly generated throwaway keypair, committed so the "
        "native client can prove it reads what an existing install already wrote. It "
        "authenticates to nothing.",
        "pinned": {
            "handle": HANDLE,
            "iat": PINNED_IAT,
            "jti": "00000000-0000-4000-8000-000000000000",
            "portal_url": "https://support.example.invalid",
            "tos_text": ACCESS_TOKEN_TOS,
        },
        "jwk": client._jwk,
        "jwk_thumbprint": client._thumbprint,
        "tos_hash_b64url": portal._sha256_b64url(ACCESS_TOKEN_TOS),
        "tos_signature_b64url": tos_signature,
        "access_token": access_token,
        # 🔴 The vector that retires this port's largest stated risk. The whole
        # split-keygen-from-signing decision rests on `ring` accepting the PKCS#8
        # that the reference's `cryptography` produced. Verified in the calling
        # session: `ring::RsaKeyPair::from_pkcs8` accepts this key and its
        # `RSA_PKCS1_SHA256` signature over the probe below is BYTE-IDENTICAL to
        # the reference's — PKCS#1 v1.5 is deterministic, so equality is the
        # right assertion rather than a verify.
        "signature_interop": {
            "probe": SIGNATURE_PROBE.decode("ascii"),
            "signature_b64url": portal._b64url_encode(
                client._private_key.sign(
                    SIGNATURE_PROBE,
                    portal.padding.PKCS1v15(),
                    portal.hashes.SHA256(),
                )
            ),
        },
        "dpop": {
            "method": DPOP_METHOD,
            "url_with_query_and_fragment": DPOP_URL,
            "url_fragment_only": "https://support.example.invalid/api/tickets#frag",
            "expected_htu_from_query_and_fragment": DPOP_URL.split("?")[0],
            "expected_htu_from_fragment_only": "https://support.example.invalid/api/tickets#frag",
            "proof_without_access_token": proof_unauthed,
            "proof_with_access_token": proof_authed,
            "proof_fragment_only": proof_fragment_only,
            "ath_b64url": portal._sha256_b64url(access_token),
        },
    }

    # The reference derives `htu` as `url.split("?")[0]`. Assert the recorded
    # expectation matches what the proof actually carries, rather than restating it.
    for name, proof, expected in (
        ("proof_without_access_token", proof_unauthed, DPOP_URL.split("?")[0]),
        (
            "proof_fragment_only",
            proof_fragment_only,
            "https://support.example.invalid/api/tickets#frag",
        ),
    ):
        payload = json.loads(portal._b64url_decode(proof.split(".")[1]))
        if payload["htu"] != expected:
            raise SystemExit(f"{name}: htu is {payload['htu']!r}, expected {expected!r}")
    if "ath" in json.loads(portal._b64url_decode(proof_unauthed.split(".")[1])):
        raise SystemExit("proof_without_access_token carries an ath; it must not")
    if "ath" not in json.loads(portal._b64url_decode(proof_authed.split(".")[1])):
        raise SystemExit("proof_with_access_token is missing its ath")

    (OUT / "vectors.json").write_text(json.dumps(vectors, indent=2, sort_keys=True) + "\n")
    (OUT / "NON_PRODUCTION_ONLY.txt").write_text(
        "The keypair, token and terms text in this directory are a freshly generated\n"
        "throwaway, committed so the native support portal client can prove it reads\n"
        "what an existing install already wrote. They authenticate to nothing and are\n"
        "not, and never were, associated with any account.\n"
    )
    print(f"wrote {OUT}")
    for path in sorted(OUT.iterdir()):
        print(f"  {path.name} ({path.stat().st_size} bytes, mode {oct(path.stat().st_mode & 0o777)})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
