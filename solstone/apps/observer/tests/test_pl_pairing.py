# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for PL observer record minting during link pairing."""

from __future__ import annotations

import io
import json
import time

import pytest
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import NameOID

from solstone.apps.link import routes as link_routes
from solstone.apps.observer.utils import load_observer_by_fingerprint
from solstone.convey.secure_listener import ConveyIdentity
from solstone.think.link.nonces import Nonce


@pytest.fixture
def pair_env(tmp_path, monkeypatch):
    def _create():
        journal = tmp_path / "journal"
        journal.mkdir(exist_ok=True)

        config_dir = journal / "config"
        config_dir.mkdir(parents=True, exist_ok=True)
        (config_dir / "journal.json").write_text(
            json.dumps(
                {
                    "convey": {"trust_localhost": True},
                    "setup": {"completed_at": 1700000000000},
                },
                indent=2,
            )
        )
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

        from solstone.convey import create_app

        app = create_app(journal=str(journal))
        client = app.test_client()

        class Env:
            def __init__(self) -> None:
                self.journal = journal
                self.app = app
                self.client = client

        return Env()

    return _create


def _make_csr(label: str = "test") -> str:
    key = ec.generate_private_key(ec.SECP256R1())
    csr = (
        x509.CertificateSigningRequestBuilder()
        .subject_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, label)]))
        .sign(key, hashes.SHA256())
    )
    return csr.public_bytes(serialization.Encoding.PEM).decode("ascii")


def _start_pair(env, *, role: str, label: str = "Pair Device") -> dict:
    response = env.client.post(
        "/app/link/pair-start",
        json={"device_label": label, "role": role},
    )
    assert response.status_code == 200
    return response.get_json()


def _pair(env, *, role: str, label: str = "Pair Device") -> dict:
    started = _start_pair(env, role=role, label=label)
    response = env.client.post(
        "/app/link/pair",
        json={"nonce": started["nonce"], "csr": _make_csr(label)},
    )
    assert response.status_code == 200
    return response.get_json()


def _pl_identity(fingerprint: str, *, label: str = "Owner Phone") -> ConveyIdentity:
    return ConveyIdentity(
        mode="pl-via-spl",
        fingerprint=fingerprint,
        device_label=label,
        paired_at="2026-05-20T00:00:00Z",
        session_id="session-1",
    )


def _observer_record_paths(env) -> list:
    return sorted((env.journal / "apps" / "observer" / "observers").glob("*.json"))


def test_observer_role_pairing_mints_observer_record_and_authorized_client(
    pair_env,
) -> None:
    env = pair_env()

    response = _pair(env, role="observer", label="Observer Laptop")

    observer = load_observer_by_fingerprint(response["fingerprint"])
    assert observer is not None
    assert observer["name"] == "Observer Laptop"
    assert observer["mode"] == "pl"
    entries = link_routes._authorized().snapshot()
    assert len(entries) == 1
    assert entries[0].fingerprint == response["fingerprint"]
    assert entries[0].role == "observer"


def test_phone_role_pairing_does_not_mint_observer_record(pair_env) -> None:
    env = pair_env()

    response = _pair(env, role="phone", label="Owner Phone")

    assert load_observer_by_fingerprint(response["fingerprint"]) is None
    assert _observer_record_paths(env) == []
    entries = link_routes._authorized().snapshot()
    assert len(entries) == 1
    assert entries[0].fingerprint == response["fingerprint"]
    assert entries[0].role == "phone"


def test_phone_role_pl_ingest_returns_auth_required(pair_env) -> None:
    env = pair_env()
    response = _pair(env, role="phone", label="Owner Phone")

    ingest = env.client.post(
        "/app/observer/ingest",
        environ_overrides={
            "pl.identity": _pl_identity(response["fingerprint"], label="Owner Phone")
        },
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(b"phone content"), "phone.txt"),
        },
    )

    assert _observer_record_paths(env) == []
    assert ingest.status_code == 401
    assert ingest.get_json()["reason_code"] == "auth_required"


def test_attestation_failure_does_not_write_observer_or_authorized(
    pair_env,
    monkeypatch,
) -> None:
    env = pair_env()

    def fail_attestation(*args, **kwargs):
        raise RuntimeError("attestation failed")

    class Authorized:
        def add(self, *args, **kwargs) -> None:
            pytest.fail("authorized add should not run after attestation failure")

    monkeypatch.setattr(link_routes, "mint_attestation", fail_attestation)
    monkeypatch.setattr(link_routes, "_authorized", lambda: Authorized())
    now = int(time.time())
    consumed = Nonce(
        value="nonce",
        device_label="Observer Laptop",
        issued_at=now,
        expires_at=now + 300,
        used=True,
        manual_code=None,
        role="observer",
    )

    with pytest.raises(RuntimeError, match="attestation failed"):
        link_routes._complete_pairing(
            consumed, _make_csr("attestation"), "Observer Laptop"
        )

    assert _observer_record_paths(env) == []


def test_observer_record_mint_failure_does_not_add_authorized_client(
    pair_env,
    monkeypatch,
) -> None:
    env = pair_env()

    def fail_mint(*args, **kwargs):
        raise RuntimeError("observer mint failed")

    class Authorized:
        def add(self, *args, **kwargs) -> None:
            pytest.fail("authorized add should not run after observer mint failure")

    monkeypatch.setattr(link_routes, "mint_pl_observer_record", fail_mint)
    monkeypatch.setattr(link_routes, "_authorized", lambda: Authorized())
    now = int(time.time())
    consumed = Nonce(
        value="nonce",
        device_label="Observer Laptop",
        issued_at=now,
        expires_at=now + 300,
        used=True,
        manual_code=None,
        role="observer",
    )

    with pytest.raises(RuntimeError, match="observer mint failed"):
        link_routes._complete_pairing(consumed, _make_csr("mint"), "Observer Laptop")

    assert _observer_record_paths(env) == []


def test_observer_record_rolls_back_when_authorized_add_fails(
    pair_env,
    monkeypatch,
) -> None:
    env = pair_env()

    class BrokenAuthorized:
        def add(self, *args, **kwargs) -> None:
            raise RuntimeError("ledger write failed")

    monkeypatch.setattr(link_routes, "_authorized", lambda: BrokenAuthorized())
    now = int(time.time())
    consumed = Nonce(
        value="nonce",
        device_label="Observer Laptop",
        issued_at=now,
        expires_at=now + 300,
        used=True,
        manual_code=None,
        role="observer",
    )

    with pytest.raises(RuntimeError, match="ledger write failed"):
        link_routes._complete_pairing(
            consumed, _make_csr("rollback"), "Observer Laptop"
        )

    assert _observer_record_paths(env) == []


def test_repair_same_label_leaves_old_observer_record(pair_env) -> None:
    env = pair_env()

    first = _pair(env, role="observer", label="Observer Laptop")
    second = _pair(env, role="observer", label="Observer Laptop")

    assert first["fingerprint"] != second["fingerprint"]
    first_record = load_observer_by_fingerprint(first["fingerprint"])
    second_record = load_observer_by_fingerprint(second["fingerprint"])
    assert first_record is not None
    assert second_record is not None
    assert first_record["enabled"] is True
    assert second_record["enabled"] is True
    assert len(_observer_record_paths(env)) == 2
