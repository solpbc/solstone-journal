# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import time
from pathlib import Path

import pytest
import requests

from solstone.observe.observer_client import ObserverClient
from solstone.think.link.ca import cert_fingerprint
from solstone.think.link.client import _build_csr
from tests.link.live_helpers import (
    RELAY_URL,
    running_convey_server,
    running_link_service,
    skip_unless_live_relay,
)

pytestmark = pytest.mark.integration
skip_unless_live_relay()


def test_observer_over_pl_upload_roundtrip(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    tmp_journal = tmp_path / "journal"
    tmp_journal.mkdir()
    config_home = tmp_path / "config-home"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_journal))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))

    label = "pytest-observer-pl"
    with (
        running_convey_server(tmp_journal) as base_url,
        running_link_service(tmp_journal),
    ):
        identity = _pair_observer(base_url, label)
        _write_bundle(config_home, label, identity)
        _write_observer_config(tmp_journal, label)

        segment_file = tmp_path / "audio.flac"
        segment_file.write_bytes(b"observer over pl")
        client = ObserverClient("pytest-pl")
        try:
            result = client.upload_segment(
                "20250103",
                "120000_300",
                [segment_file],
                meta={"stream": "pytest-pl", "host": "pytest-pl"},
            )
            assert result.success is True
            prefix = cert_fingerprint(identity["client_cert"]).replace("sha256:", "")[
                :16
            ]
            _wait_until(
                lambda: (
                    tmp_journal
                    / "apps"
                    / "observer"
                    / "observers"
                    / prefix
                    / "hist"
                    / "20250103.jsonl"
                ).exists()
            )
        finally:
            client.stop()


def _pair_observer(base_url: str, label: str) -> dict:
    start = requests.post(
        f"{base_url}/app/link/pair-start",
        json={"device_label": label, "role": "observer"},
        timeout=10,
    )
    start.raise_for_status()
    private_key_pem, csr_pem = _build_csr(label)
    paired = requests.post(
        f"{base_url}/app/link/pair",
        json={"nonce": start.json()["nonce"], "csr": csr_pem, "device_label": label},
        timeout=10,
    )
    paired.raise_for_status()
    payload = paired.json()
    payload["private_key"] = private_key_pem
    return payload


def _write_bundle(config_home: Path, label: str, identity: dict) -> None:
    bundle = config_home / "solstone-observer" / "spl" / label
    bundle.mkdir(parents=True)
    chain_pem = "".join(identity["ca_chain"])
    peer = {
        "label": label,
        "paired_at": "2026-05-20T00:00:00Z",
        "instance_id": identity["instance_id"],
        "home_label": identity["home_label"],
        "fingerprint": cert_fingerprint(chain_pem),
        "local_endpoints": identity.get("local_endpoints", []),
        "role": "observer",
    }
    (bundle / "private.pem").write_text(identity["private_key"], encoding="utf-8")
    (bundle / "cert.pem").write_text(identity["client_cert"], encoding="utf-8")
    (bundle / "chain.pem").write_text(chain_pem, encoding="utf-8")
    (bundle / "home_attestation.jwt").write_text(
        identity["home_attestation"],
        encoding="utf-8",
    )
    (bundle / "peer.json").write_text(json.dumps(peer, indent=2) + "\n")


def _write_observer_config(journal: Path, label: str) -> None:
    path = journal / "config" / "journal.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(
            {
                "observe": {
                    "observer": {
                        "pair_mode": "pl",
                        "spl_label": label,
                        "spl_relay_url": RELAY_URL,
                        "name": "pytest-pl",
                    }
                }
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )


def _wait_until(predicate, timeout: float = 10.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.1)
    raise AssertionError("timed out waiting for condition")
