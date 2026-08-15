# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""authorized_clients.json — the PL revocation ledger.

The spl-protocol-fixed core shape is unchanged: `fingerprint`, `device_label`,
`paired_at`, `instance_id`, and `role`. `device_label` is the home-assigned,
renameable label for the paired client. Solstone also stores local-only
`last_seen_at`, `network`, `client_label`, `label_ordinal`, and `kind` for UX:

    {
      "fingerprint": "sha256:<hex>",
      "device_label": "Rae's iPhone",
      "paired_at": "2026-04-19T17:42:13Z",
      "instance_id": "<home_instance_id>",
      "role": "",
      "last_seen_at": "2026-04-19T18:03:12Z",  // optional; null/absent = never
      "network": "network",                    // optional; local display label source
      "client_label": "rae-laptop",            // optional; client self-name/hostname
      "label_ordinal": 2,                      // optional positive int;
                                               // invalid/absent = 1; omitted when 1
      "kind": "cert"
    }

This is a cert-only ledger. Legacy rows whose nonempty `kind` is not `cert`
are dropped on load with one warning per load call. Loading never rewrites the
file.

Role-less linked systems are stored with `role: ""`; peers are stored with
`role: "peer"`. The peer role is provenance, not a behavioral authorization
role: it denotes a linked system whose pairing provisioned a journal-content
source, minted a journal-source record, and records the sender `instance_id`.
That provenance is durable in data via per-segment `sender_instance_id` /
`sender_fingerprint` and identity-derived source directories. Readers reload
the file on mtime change so an unpair action takes effect within ~500 ms of the
file write. Convey's pair and unpair routes own the pairing writer surface; the
secure listener updates `last_seen_at` and uses this ledger for TLS verification
and per-request authorization.

`last_seen_at`, `network`, `client_label`, `label_ordinal`, and `kind` are
local-only — never transmitted externally.
"""

from __future__ import annotations

import datetime as dt
import json
import logging
import threading
from dataclasses import dataclass, replace
from pathlib import Path

from solstone.think.journal_io import hold_lock, write_json

MAX_DEVICE_LABEL_LEN = 80
logger = logging.getLogger(__name__)


def is_peer(role: str) -> bool:
    return role == "peer"


@dataclass(frozen=True)
class ClientEntry:
    fingerprint: str
    device_label: str
    paired_at: str
    instance_id: str
    role: str = ""
    last_seen_at: str | None = None
    network: str | None = None
    client_label: str = ""
    label_ordinal: int = 1
    kind: str = "cert"
    observer_handle: str | None = None

    @property
    def base_label(self) -> str:
        return self.device_label or self.client_label

    @property
    def display_label(self) -> str:
        base = self.base_label
        if self.label_ordinal == 1 or not base:
            return base
        return f"{base} ({self.label_ordinal})"


class AuthorizedClients:
    """In-memory view of authorized_clients.json with mtime-based reload."""

    def __init__(self, path: Path) -> None:
        self._path = path
        self._lock = threading.Lock()
        self._entries: dict[str, ClientEntry] = {}
        self._mtime_ns = 0
        if path.exists():
            self._reload_locked()

    @property
    def path(self) -> Path:
        return self._path

    def reload_if_stale(self) -> bool:
        """Re-read the file if its mtime changed. Returns True if reloaded."""
        with self._lock:
            try:
                current = self._path.stat().st_mtime_ns
            except FileNotFoundError:
                if self._entries:
                    self._entries = {}
                    self._mtime_ns = 0
                    return True
                return False
            if current == self._mtime_ns:
                return False
            self._reload_locked()
            return True

    def is_authorized(self, fingerprint: str) -> bool:
        self.reload_if_stale()
        with self._lock:
            return fingerprint in self._entries

    def add(
        self,
        fingerprint: str,
        device_label: str,
        instance_id: str,
        *,
        role: str = "",
        paired_at: str | None = None,
        network: str | None = None,
        client_label: str = "",
    ) -> ClientEntry:
        with self._lock:
            with hold_lock(self._path):
                current = self._load_file_locked()
                paired_at = paired_at or dt.datetime.now(dt.UTC).strftime(
                    "%Y-%m-%dT%H:%M:%SZ"
                )
                entry = ClientEntry(
                    fingerprint=fingerprint,
                    device_label=device_label,
                    paired_at=paired_at,
                    instance_id=instance_id,
                    role=role,
                    last_seen_at=None,
                    network=network,
                    client_label=client_label,
                )
                entry = replace(
                    entry,
                    label_ordinal=self._allocate_label_ordinal_locked(
                        current,
                        entry.base_label,
                        fingerprint,
                    ),
                )
                current[fingerprint] = entry
                self._write(current)
                self._entries = current
                return entry

    def remove(self, fingerprint: str) -> bool:
        with self._lock:
            with hold_lock(self._path):
                current = self._load_file_locked()
                if fingerprint not in current:
                    return False
                del current[fingerprint]
                self._write(current)
                self._entries = current
                return True

    def touch_last_seen(
        self, fingerprint: str, *, now: dt.datetime | None = None
    ) -> bool:
        """Update last_seen_at for a paired device. Returns False if not paired."""
        ts = (now or dt.datetime.now(dt.UTC)).strftime("%Y-%m-%dT%H:%M:%SZ")
        with self._lock:
            with hold_lock(self._path):
                current = self._load_file_locked()
                existing = current.get(fingerprint)
                if existing is None:
                    return False
                current[fingerprint] = replace(existing, last_seen_at=ts)
                self._write(current)
                self._entries = current
                return True

    def update_label(self, fingerprint: str, label: str) -> ClientEntry | None:
        """Update device_label for a paired device. Returns None if not paired."""
        normalized = label.strip()
        if not normalized:
            raise ValueError("label must not be empty")
        with self._lock:
            with hold_lock(self._path):
                current = self._load_file_locked()
                existing = current.get(fingerprint)
                if existing is None:
                    return None
                if normalized == existing.display_label:
                    normalized = existing.base_label
                if len(normalized) > MAX_DEVICE_LABEL_LEN:
                    raise ValueError("label too long")
                updated = replace(existing, device_label=normalized)
                updated = replace(
                    updated,
                    label_ordinal=self._allocate_label_ordinal_locked(
                        current,
                        updated.base_label,
                        fingerprint,
                    ),
                )
                current[fingerprint] = updated
                self._write(current)
                self._entries = current
                return updated

    def snapshot(self) -> list[ClientEntry]:
        self.reload_if_stale()
        with self._lock:
            return list(self._entries.values())

    def get(self, fingerprint: str) -> ClientEntry | None:
        self.reload_if_stale()
        with self._lock:
            return self._entries.get(fingerprint)

    def find_all_by_display_label(self, label: str) -> list[ClientEntry]:
        self.reload_if_stale()
        with self._lock:
            return [
                entry
                for entry in self._entries.values()
                if label and entry.display_label == label
            ]

    def _reload_locked(self) -> None:
        entries = self._load_file_locked()
        self._entries = entries
        try:
            self._mtime_ns = self._path.stat().st_mtime_ns
        except FileNotFoundError:
            self._mtime_ns = 0

    def backfill_label_ordinals(self) -> bool:
        """Repair duplicate label ordinals in authorized_clients.json."""
        with self._lock:
            with hold_lock(self._path):
                current = self._load_file_locked()
                groups: dict[str, list[ClientEntry]] = {}
                for entry in current.values():
                    base = entry.base_label
                    if not base:
                        continue
                    groups.setdefault(base, []).append(entry)

                changed = False
                for entries in groups.values():
                    seen: set[int] = set()
                    needs_repair = False
                    for entry in entries:
                        if entry.label_ordinal in seen:
                            needs_repair = True
                            break
                        seen.add(entry.label_ordinal)
                    if not needs_repair:
                        continue

                    for ordinal, entry in enumerate(
                        sorted(entries, key=lambda e: (e.paired_at, e.fingerprint)),
                        start=1,
                    ):
                        if entry.label_ordinal == ordinal:
                            continue
                        current[entry.fingerprint] = replace(
                            entry,
                            label_ordinal=ordinal,
                        )
                        changed = True

                if not changed:
                    return False
                self._write(current)
                self._entries = current
                return True

    def _load_file_locked(self) -> dict[str, ClientEntry]:
        if not self._path.exists():
            return {}
        try:
            raw = json.loads(self._path.read_text("utf-8"))
        except (json.JSONDecodeError, OSError):
            # Unreadable authorized_clients.json means no clients are authorized.
            # There is no last-good authorization cache.
            return {}
        out: dict[str, ClientEntry] = {}
        dropped_non_cert = False
        if isinstance(raw, list):
            for item in raw:
                if not isinstance(item, dict):
                    continue
                fp = item.get("fingerprint")
                if not isinstance(fp, str):
                    continue
                if "kind" in item and item["kind"] != "cert":
                    dropped_non_cert = True
                    continue
                last_seen = item.get("last_seen_at")
                network = item.get("network")
                client_label = item.get("client_label")
                raw_label_ordinal = item.get("label_ordinal", 1)
                label_ordinal = (
                    raw_label_ordinal
                    if isinstance(raw_label_ordinal, int)
                    and not isinstance(raw_label_ordinal, bool)
                    and raw_label_ordinal > 0
                    else 1
                )
                out[fp] = ClientEntry(
                    fingerprint=fp,
                    device_label=str(item.get("device_label", "")),
                    paired_at=str(item.get("paired_at", "")),
                    instance_id=str(item.get("instance_id", "")),
                    role=item.get("role") if isinstance(item.get("role"), str) else "",
                    last_seen_at=last_seen if isinstance(last_seen, str) else None,
                    network=network if isinstance(network, str) else None,
                    client_label=client_label if isinstance(client_label, str) else "",
                    label_ordinal=label_ordinal,
                )
        if dropped_non_cert:
            logger.warning(
                "authorized_clients.json: ignored one or more entries with an "
                "unsupported kind (expected cert)"
            )
        return out

    def _allocate_label_ordinal_locked(
        self,
        entries: dict[str, ClientEntry],
        base_label: str,
        fingerprint: str,
    ) -> int:
        if not base_label:
            return 1
        held = {
            entry.label_ordinal
            for entry in entries.values()
            if entry.fingerprint != fingerprint and entry.base_label == base_label
        }
        ordinal = 1
        while ordinal in held:
            ordinal += 1
        return ordinal

    def _write(self, entries: dict[str, ClientEntry]) -> None:
        payload = [
            {
                "fingerprint": e.fingerprint,
                "device_label": e.device_label,
                "paired_at": e.paired_at,
                "instance_id": e.instance_id,
                "role": e.role,
                "kind": e.kind,
                **({"last_seen_at": e.last_seen_at} if e.last_seen_at else {}),
                **({"network": e.network} if e.network else {}),
                **({"client_label": e.client_label} if e.client_label else {}),
                **({"label_ordinal": e.label_ordinal} if e.label_ordinal != 1 else {}),
            }
            for e in entries.values()
        ]
        write_json(self._path, payload)
