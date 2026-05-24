# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest
from cryptography.hazmat.primitives import serialization

import solstone.observe.peer_lookup as peer_lookup
import solstone.think.utils as think_utils
from solstone.think.link.ca import cert_fingerprint, generate_ca


def _set_journal(monkeypatch: pytest.MonkeyPatch, journal: Path) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    think_utils._journal_path_cache = None
    peer_lookup._cache.clear()


def _write_peer(
    journal: Path,
    instance_id: str,
    label: str,
    *,
    endpoints: list[dict[str, object]] | None = None,
) -> Path:
    peer_dir = journal / "peers" / instance_id
    peer_dir.mkdir(parents=True)
    ca = generate_ca(peer_dir / "ca")
    cert_pem = ca.cert.public_bytes(serialization.Encoding.PEM).decode("ascii")
    (peer_dir / "cert.pem").write_text(cert_pem, encoding="utf-8")
    (peer_dir / "private.pem").write_text("private", encoding="utf-8")
    (peer_dir / "chain.pem").write_text(cert_pem, encoding="utf-8")
    (peer_dir / "home_attestation.jwt").write_text("jwt", encoding="utf-8")
    (peer_dir / "peer.json").write_text(
        json.dumps(
            {
                "label": label,
                "instance_id": instance_id,
                "home_label": "solstone",
                "local_endpoints": endpoints or [{"ip": "127.0.0.1", "port": 7657}],
            }
        ),
        encoding="utf-8",
    )
    return peer_dir


def test_resolve_peer_happy_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    journal = tmp_path / "journal"
    _set_journal(monkeypatch, journal)
    peer_dir = _write_peer(
        journal,
        "12345678-1234-1234-1234-123456789abc",
        "host-a",
    )

    info = peer_lookup.resolve_peer("host-a")

    assert info.dir == peer_dir
    assert info.instance_id == "12345678-1234-1234-1234-123456789abc"
    assert info.label == "host-a"
    assert info.local_endpoints == [{"ip": "127.0.0.1", "port": 7657}]
    expected = cert_fingerprint((peer_dir / "cert.pem").read_text(encoding="utf-8"))
    assert info.cert_fingerprint == expected
    assert info.cert_fingerprint.startswith("sha256:")


def test_resolve_peer_no_match_lists_available(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    journal = tmp_path / "journal"
    _set_journal(monkeypatch, journal)
    (journal / "peers").mkdir(parents=True)

    with pytest.raises(peer_lookup.PeerLookupError, match="available: none"):
        peer_lookup.resolve_peer("missing")

    _write_peer(journal, "iid-a", "host-a")
    _write_peer(journal, "iid-b", "host-b")

    with pytest.raises(peer_lookup.PeerLookupError) as exc_info:
        peer_lookup.resolve_peer("missing")

    message = str(exc_info.value)
    assert 'no peer with label "missing"' in message
    assert "available: host-a, host-b" in message


def test_resolve_peer_multiple_matches(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    journal = tmp_path / "journal"
    _set_journal(monkeypatch, journal)
    _write_peer(journal, "iid-a", "host-a")
    _write_peer(journal, "iid-b", "host-a")

    with pytest.raises(peer_lookup.PeerLookupError) as exc_info:
        peer_lookup.resolve_peer("host-a")

    message = str(exc_info.value)
    assert 'multiple peers with label "host-a"' in message
    assert "iid-a" in message
    assert "iid-b" in message
    assert "use <journal_root>/peers/<instance_id>" in message


def test_resolve_peer_cache_invalidates_on_peers_dir_mtime(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    journal = tmp_path / "journal"
    _set_journal(monkeypatch, journal)
    _write_peer(journal, "iid-a", "host-a")

    assert peer_lookup.resolve_peer("host-a").instance_id == "iid-a"

    peers_dir = journal / "peers"
    _write_peer(journal, "iid-b", "host-b")
    stat = peers_dir.stat()
    os.utime(peers_dir, ns=(stat.st_atime_ns, stat.st_mtime_ns + 1_000_000_000))

    assert peer_lookup.resolve_peer("host-b").instance_id == "iid-b"
