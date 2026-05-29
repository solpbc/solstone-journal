# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""authorized_clients.json — the PL revocation ledger.

Entry shape is fixed by the spl protocol (see github.com/solpbc/spl
proto/pairing.md §6), plus a solstone-specific `last_seen_at` field for
UX:

            {
              "fingerprint": "sha256:<hex>",
              "device_label": "Jer's iPhone",
              "paired_at": "2026-04-19T17:42:13Z",
              "instance_id": "<home_instance_id>",
              "role": "phone",
              "last_seen_at": "2026-04-19T18:03:12Z"   // optional; null/absent = never
            }

Readers reload the file on mtime change so an unpair action takes effect
within ~500 ms of the file write. Convey's pair and unpair routes own the
pairing writer surface; the secure listener updates `last_seen_at` and uses
this ledger for TLS verification and per-request authorization.

`last_seen_at` is local-only — never transmitted externally.
"""

from __future__ import annotations

import datetime as dt
import fcntl
import json
import os
import threading
from dataclasses import dataclass, replace
from pathlib import Path

MAX_DEVICE_LABEL_LEN = 80


@dataclass(frozen=True)
class ClientEntry:
    fingerprint: str
    device_label: str
    paired_at: str
    instance_id: str
    role: str = "phone"
    last_seen_at: str | None = None


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
        role: str = "phone",
        paired_at: str | None = None,
    ) -> None:
        paired_at = paired_at or dt.datetime.now(dt.UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
        entry = ClientEntry(
            fingerprint=fingerprint,
            device_label=device_label,
            paired_at=paired_at,
            instance_id=instance_id,
            role=role,
            last_seen_at=None,
        )
        with self._lock:
            current = self._load_file_locked()
            current[fingerprint] = entry
            self._atomic_write_locked(current)
            self._entries = current

    def remove(self, fingerprint: str) -> bool:
        with self._lock:
            current = self._load_file_locked()
            if fingerprint not in current:
                return False
            del current[fingerprint]
            self._atomic_write_locked(current)
            self._entries = current
            return True

    def touch_last_seen(
        self, fingerprint: str, *, now: dt.datetime | None = None
    ) -> bool:
        """Update last_seen_at for a paired device. Returns False if not paired."""
        ts = (now or dt.datetime.now(dt.UTC)).strftime("%Y-%m-%dT%H:%M:%SZ")
        with self._lock:
            current = self._load_file_locked()
            existing = current.get(fingerprint)
            if existing is None:
                return False
            current[fingerprint] = replace(existing, last_seen_at=ts)
            self._atomic_write_locked(current)
            self._entries = current
            return True

    def update_label(self, fingerprint: str, label: str) -> bool:
        """Update device_label for a paired device. Returns False if not paired."""
        normalized = label.strip()
        if not normalized:
            raise ValueError("label must not be empty")
        if len(normalized) > MAX_DEVICE_LABEL_LEN:
            raise ValueError("label too long")
        with self._lock:
            current = self._load_file_locked()
            existing = current.get(fingerprint)
            if existing is None:
                return False
            current[fingerprint] = replace(existing, device_label=normalized)
            self._atomic_write_locked(current)
            self._entries = current
            return True

    def snapshot(self) -> list[ClientEntry]:
        self.reload_if_stale()
        with self._lock:
            return list(self._entries.values())

    def get(self, fingerprint: str) -> ClientEntry | None:
        self.reload_if_stale()
        with self._lock:
            return self._entries.get(fingerprint)

    def find_by_label(self, label: str) -> ClientEntry | None:
        self.reload_if_stale()
        with self._lock:
            for entry in self._entries.values():
                if entry.device_label == label:
                    return entry
        return None

    def _reload_locked(self) -> None:
        entries = self._load_file_locked()
        self._entries = entries
        try:
            self._mtime_ns = self._path.stat().st_mtime_ns
        except FileNotFoundError:
            self._mtime_ns = 0

    def _load_file_locked(self) -> dict[str, ClientEntry]:
        if not self._path.exists():
            return {}
        try:
            raw = json.loads(self._path.read_text("utf-8"))
        except (json.JSONDecodeError, OSError):
            # Unreadable authorized_clients.json means no clients are authorized. There is no last-good authorization cache.
            return {}
        out: dict[str, ClientEntry] = {}
        if isinstance(raw, list):
            for item in raw:
                if not isinstance(item, dict):
                    continue
                fp = item.get("fingerprint")
                if not isinstance(fp, str):
                    continue
                last_seen = item.get("last_seen_at")
                out[fp] = ClientEntry(
                    fingerprint=fp,
                    device_label=str(item.get("device_label", "")),
                    paired_at=str(item.get("paired_at", "")),
                    instance_id=str(item.get("instance_id", "")),
                    role=(
                        item.get("role")
                        if isinstance(item.get("role"), str)
                        else "phone"
                    ),
                    last_seen_at=last_seen if isinstance(last_seen, str) else None,
                )
        return out

    def _atomic_write_locked(self, entries: dict[str, ClientEntry]) -> None:
        self._path.parent.mkdir(parents=True, exist_ok=True)
        payload = [
            {
                "fingerprint": e.fingerprint,
                "device_label": e.device_label,
                "paired_at": e.paired_at,
                "instance_id": e.instance_id,
                "role": e.role,
                **({"last_seen_at": e.last_seen_at} if e.last_seen_at else {}),
            }
            for e in entries.values()
        ]
        tmp = self._path.with_suffix(self._path.suffix + ".tmp")
        with open(tmp, "w", encoding="utf-8") as f:
            fcntl.flock(f.fileno(), fcntl.LOCK_EX)
            json.dump(payload, f, indent=2)
            f.write("\n")
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, self._path)
