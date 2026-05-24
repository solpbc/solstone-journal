# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

from solstone.think.link.ca import cert_fingerprint
from solstone.think.utils import get_journal


class PeerLookupError(Exception):
    pass


@dataclass(frozen=True)
class PeerInfo:
    dir: Path
    instance_id: str
    label: str
    local_endpoints: list[dict[str, object]]
    cert_fingerprint: str


_cache: dict[tuple[int, str], PeerInfo] = {}


def resolve_peer(label: str) -> PeerInfo:
    peers_dir = Path(get_journal()) / "peers"
    if not peers_dir.is_dir():
        raise PeerLookupError('no peers paired (run "sol link join --as peer" first)')

    mtime_ns = peers_dir.stat().st_mtime_ns
    if any(key[0] != mtime_ns for key in _cache):
        _cache.clear()

    cache_key = (mtime_ns, label)
    cached = _cache.get(cache_key)
    if cached is not None:
        return cached

    matches: list[tuple[Path, dict[str, object]]] = []
    labels: set[str] = set()
    for peer_dir in sorted(peers_dir.iterdir()):
        if not peer_dir.is_dir():
            continue
        peer_json = peer_dir / "peer.json"
        if not peer_json.is_file():
            continue
        try:
            peer = json.loads(peer_json.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, OSError) as exc:
            raise PeerLookupError(f"invalid peer.json in {peer_dir}: {exc}") from exc
        peer_label = peer.get("label")
        if isinstance(peer_label, str) and peer_label:
            labels.add(peer_label)
        if peer_label == label:
            matches.append((peer_dir, peer))

    if not matches:
        labels_str = ", ".join(sorted(labels)) if labels else "none"
        raise PeerLookupError(f'no peer with label "{label}"; available: {labels_str}')
    if len(matches) > 1:
        ids = ", ".join(
            str(peer.get("instance_id") or peer_dir.name) for peer_dir, peer in matches
        )
        raise PeerLookupError(
            f'multiple peers with label "{label}": {ids}; '
            "use <journal_root>/peers/<instance_id> directly"
        )

    peer_dir, peer = matches[0]
    cert_pem = (peer_dir / "cert.pem").read_text(encoding="utf-8")
    endpoints = peer.get("local_endpoints") or []
    if not isinstance(endpoints, list):
        endpoints = []
    info = PeerInfo(
        dir=peer_dir,
        instance_id=str(peer.get("instance_id") or peer_dir.name),
        label=label,
        local_endpoints=endpoints,
        cert_fingerprint=cert_fingerprint(cert_pem),
    )
    _cache[cache_key] = info
    return info
