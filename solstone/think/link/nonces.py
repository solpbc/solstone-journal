# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Pair-nonce store — shared between the CLI pair flow and convey's pair route.

`sol call link pair` mints a nonce and writes it to disk; convey's
`POST /link/pair` reads on every incoming pair request, garbage-collects
expired entries, and enforces single-use semantics. The file is the IPC
channel between the two processes — simple, durable across crashes, no
extra port.

Consumers treat the file as opaque and call only into the methods here.
Atomic replaces guard against partial writes.
"""

from __future__ import annotations

import fcntl
import json
import os
import time
from dataclasses import dataclass
from pathlib import Path

NONCE_TTL_SECONDS = 300  # 5 min per the spl pairing spec.


@dataclass(frozen=True)
class Nonce:
    value: str
    device_label: str
    issued_at: int
    expires_at: int
    used: bool
    manual_code: str | None
    role: str = "phone"


class NonceStore:
    def __init__(self, path: Path) -> None:
        self._path = path

    @property
    def path(self) -> Path:
        return self._path

    def add(
        self,
        nonce: str,
        device_label: str,
        *,
        role: str = "phone",
        manual_code: str | None = None,
        now: int | None = None,
        ttl: int = NONCE_TTL_SECONDS,
    ) -> Nonce:
        ts = now if now is not None else int(time.time())
        entry = Nonce(
            value=nonce,
            device_label=device_label,
            issued_at=ts,
            expires_at=ts + ttl,
            used=False,
            manual_code=manual_code,
            role=role,
        )
        with self._locked_read_write() as entries:
            self._gc_locked(entries, ts)
            entries[nonce] = entry
            self._write_locked(entries)
        return entry

    def consume(self, value: str, *, now: int | None = None) -> Nonce | None:
        """Mark a nonce used if valid. Single-use enforced atomically."""
        ts = now if now is not None else int(time.time())
        with self._locked_read_write() as entries:
            self._gc_locked(entries, ts)
            entry = entries.get(value)
            if entry is None:
                return None
            if entry.used or entry.expires_at <= ts:
                return None
            entry = Nonce(
                value=entry.value,
                device_label=entry.device_label,
                issued_at=entry.issued_at,
                expires_at=entry.expires_at,
                used=True,
                manual_code=entry.manual_code,
                role=entry.role,
            )
            entries[value] = entry
            self._write_locked(entries)
            return entry

    def consume_by_code(self, code: str, *, now: int | None = None) -> Nonce | None:
        """Mark the nonce matching a manual code used if valid."""
        ts = now if now is not None else int(time.time())
        with self._locked_read_write() as entries:
            self._gc_locked(entries, ts)
            for value, entry in entries.items():
                if entry.manual_code != code:
                    continue
                used_entry = Nonce(
                    value=entry.value,
                    device_label=entry.device_label,
                    issued_at=entry.issued_at,
                    expires_at=entry.expires_at,
                    used=True,
                    manual_code=entry.manual_code,
                    role=entry.role,
                )
                entries[value] = used_entry
                self._write_locked(entries)
                return used_entry
        return None

    def peek(self, value: str) -> Nonce | None:
        entries = self._read()
        return entries.get(value)

    def snapshot(self) -> list[Nonce]:
        return list(self._read().values())

    def gc(self, *, now: int | None = None) -> int:
        """Remove expired entries. Returns count removed."""
        ts = now if now is not None else int(time.time())
        with self._locked_read_write() as entries:
            before = len(entries)
            self._gc_locked(entries, ts)
            if len(entries) != before:
                self._write_locked(entries)
            return before - len(entries)

    def _read(self) -> dict[str, Nonce]:
        if not self._path.exists():
            return {}
        try:
            raw = json.loads(self._path.read_text("utf-8"))
        except (json.JSONDecodeError, OSError):
            return {}
        out: dict[str, Nonce] = {}
        if isinstance(raw, list):
            for item in raw:
                if not isinstance(item, dict):
                    continue
                val = item.get("value")
                if not isinstance(val, str):
                    continue
                out[val] = Nonce(
                    value=val,
                    device_label=str(item.get("device_label", "")),
                    issued_at=int(item.get("issued_at", 0)),
                    expires_at=int(item.get("expires_at", 0)),
                    used=bool(item.get("used", False)),
                    manual_code=(
                        item.get("manual_code")
                        if isinstance(item.get("manual_code"), str)
                        else None
                    ),
                    role=(
                        item.get("role")
                        if isinstance(item.get("role"), str)
                        else "phone"
                    ),
                )
        return out

    def _write_locked(self, entries: dict[str, Nonce]) -> None:
        self._path.parent.mkdir(parents=True, exist_ok=True)
        payload = [
            {
                "value": e.value,
                "device_label": e.device_label,
                "issued_at": e.issued_at,
                "expires_at": e.expires_at,
                "used": e.used,
                "manual_code": e.manual_code,
                "role": e.role,
            }
            for e in entries.values()
        ]
        tmp = self._path.with_suffix(self._path.suffix + ".tmp")
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(payload, f, indent=2)
            f.write("\n")
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, self._path)

    def _gc_locked(self, entries: dict[str, Nonce], now: int) -> None:
        to_drop = [k for k, e in entries.items() if e.used or e.expires_at <= now]
        for k in to_drop:
            del entries[k]

    class _Guard:
        def __init__(self, store: NonceStore) -> None:
            self.store = store
            self.lock_path = store._path.with_suffix(store._path.suffix + ".lock")
            self.fd = -1

        def __enter__(self) -> dict[str, Nonce]:
            self.lock_path.parent.mkdir(parents=True, exist_ok=True)
            self.fd = os.open(
                str(self.lock_path),
                os.O_RDWR | os.O_CREAT,
                0o600,
            )
            fcntl.flock(self.fd, fcntl.LOCK_EX)
            return self.store._read()

        def __exit__(self, *_: object) -> None:
            try:
                fcntl.flock(self.fd, fcntl.LOCK_UN)
            finally:
                os.close(self.fd)

    def _locked_read_write(self) -> NonceStore._Guard:
        return NonceStore._Guard(self)
