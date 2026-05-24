# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared utilities for the observer app.

Provides common helpers for observer metadata management and sync history
that are used by both routes.py and events.py.
"""

from __future__ import annotations

import json
import logging
import os
import re
import threading
from pathlib import Path
from typing import Any

from solstone.apps.utils import get_app_storage_path
from solstone.convey.reasons import (
    AUTH_KEY_INVALID,
    AUTH_REQUIRED,
    FEATURE_UNAVAILABLE,
    PL_REVOKED,
)
from solstone.convey.utils import error_response
from solstone.think.entities.core import atomic_write
from solstone.think.utils import now_ms

logger = logging.getLogger(__name__)
FINGERPRINT_RE = re.compile(r"^sha256:([a-f0-9]{64})$")


def get_observers_dir() -> Path:
    """Get the observers storage directory."""
    return get_app_storage_path("observer", "observers", ensure_exists=True)


def get_hist_dir(key_prefix: str, ensure_exists: bool = True) -> Path:
    """Get the history directory for an observer.

    Args:
        key_prefix: Observer filename prefix
        ensure_exists: Create directory if it doesn't exist (default: True)

    Returns:
        Path to apps/observer/observers/<key_prefix>/hist/
    """
    return get_app_storage_path(
        "observer", "observers", key_prefix, "hist", ensure_exists=ensure_exists
    )


def _fingerprint_hex(fingerprint: str) -> str:
    match = FINGERPRINT_RE.fullmatch(fingerprint)
    if match is None:
        raise ValueError("observer fingerprint must be sha256:<64 hex chars>")
    return match.group(1)


def observer_filename_prefix(record: dict[str, Any]) -> str:
    fingerprint = record.get("fingerprint")
    key = record.get("key")
    if isinstance(fingerprint, str) and fingerprint:
        return _fingerprint_hex(fingerprint)[:16]
    if isinstance(key, str) and key:
        return key[:8]
    raise ValueError("observer record must include key or fingerprint")


def observer_mode(record: dict[str, Any]) -> str:
    return "pl" if record.get("fingerprint") else "dl"


def _observer_filename(record: dict[str, Any]) -> str:
    return f"{observer_filename_prefix(record)}.json"


def _persistable_record(record: dict[str, Any]) -> dict[str, Any]:
    clean = dict(record)
    clean.pop("filename_prefix", None)
    return clean


def _augment_record(record: dict[str, Any], filename_prefix: str | None = None) -> dict:
    augmented = dict(record)
    augmented["mode"] = observer_mode(record)
    augmented["filename_prefix"] = filename_prefix or observer_filename_prefix(record)
    return augmented


def _validate_observer_record(record: dict[str, Any], path: Path) -> dict | None:
    key = record.get("key")
    fingerprint = record.get("fingerprint")
    has_key = isinstance(key, str) and bool(key)
    has_fingerprint = isinstance(fingerprint, str) and bool(fingerprint)
    if has_key == has_fingerprint:
        logger.warning("Skipping invalid observer record %s", path)
        return None
    try:
        prefix = observer_filename_prefix(record)
    except ValueError as exc:
        logger.warning("Skipping invalid observer record %s: %s", path, exc)
        return None
    return _augment_record(record, prefix)


class ObserverRegistry:
    _instance: ObserverRegistry | None = None
    _instance_lock = threading.Lock()

    def __init__(self, observers_dir: Path) -> None:
        self._observers_dir = observers_dir
        self._lock = threading.Lock()
        self._mtime_ns = -1
        self._by_key: dict[str, dict] = {}
        self._by_fingerprint: dict[str, dict] = {}
        self._by_prefix: dict[str, dict] = {}
        self._records: list[dict] = []

    @classmethod
    def singleton(cls) -> ObserverRegistry:
        observers_dir = get_observers_dir()
        with cls._instance_lock:
            if cls._instance is None or cls._instance._observers_dir != observers_dir:
                cls._instance = cls(observers_dir)
            return cls._instance

    def invalidate(self) -> None:
        with self._lock:
            self._mtime_ns = -1

    def _current_mtime_ns(self) -> int:
        try:
            current = self._observers_dir.stat().st_mtime_ns
        except FileNotFoundError:
            return 0
        for observer_path in self._observers_dir.glob("*.json"):
            try:
                current = max(current, observer_path.stat().st_mtime_ns)
            except FileNotFoundError:
                continue
        return current

    def reload_if_stale(self) -> None:
        current_mtime = self._current_mtime_ns()
        with self._lock:
            if current_mtime == self._mtime_ns:
                return
            self._reload_locked(current_mtime)

    def by_key(self, key: str) -> dict | None:
        self.reload_if_stale()
        with self._lock:
            record = self._by_key.get(key)
            return dict(record) if record is not None else None

    def by_fingerprint(self, fingerprint: str) -> dict | None:
        self.reload_if_stale()
        with self._lock:
            record = self._by_fingerprint.get(fingerprint)
            return dict(record) if record is not None else None

    def by_prefix(self, prefix: str) -> dict | None:
        self.reload_if_stale()
        with self._lock:
            record = self._by_prefix.get(prefix)
            return dict(record) if record is not None else None

    def by_name(self, name: str) -> dict | None:
        self.reload_if_stale()
        with self._lock:
            for record in self._records:
                if record.get("name") == name:
                    return dict(record)
        return None

    def all(self) -> list[dict]:
        self.reload_if_stale()
        with self._lock:
            return [dict(record) for record in self._records]

    def _reload_locked(self, current_mtime: int) -> None:
        by_key: dict[str, dict] = {}
        by_fingerprint: dict[str, dict] = {}
        by_prefix: dict[str, dict] = {}
        records: list[dict] = []
        for observer_path in self._observers_dir.glob("*.json"):
            try:
                with open(observer_path, encoding="utf-8") as f:
                    raw = json.load(f)
            except (json.JSONDecodeError, OSError) as exc:
                logger.warning(
                    "Skipping unreadable observer record %s: %s", observer_path, exc
                )
                continue
            if not isinstance(raw, dict):
                logger.warning("Skipping invalid observer record %s", observer_path)
                continue
            record = _validate_observer_record(raw, observer_path)
            if record is None:
                continue
            prefix = record["filename_prefix"]
            if observer_path.name != f"{prefix}.json":
                logger.warning(
                    "Skipping observer record with mismatched filename %s",
                    observer_path,
                )
                continue
            key = record.get("key")
            if isinstance(key, str) and key:
                by_key[key] = record
            fingerprint = record.get("fingerprint")
            if isinstance(fingerprint, str) and fingerprint:
                by_fingerprint[fingerprint] = record
            by_prefix[prefix] = record
            records.append(record)
        records.sort(key=lambda item: item.get("created_at", 0), reverse=True)
        self._by_key = by_key
        self._by_fingerprint = by_fingerprint
        self._by_prefix = by_prefix
        self._records = records
        self._mtime_ns = current_mtime


def load_observer(key: str) -> dict | None:
    """Load observer metadata by DL bearer key."""
    return ObserverRegistry.singleton().by_key(key)


def load_observer_by_fingerprint(fingerprint: str) -> dict | None:
    """Load observer metadata by PL client certificate fingerprint."""
    return ObserverRegistry.singleton().by_fingerprint(fingerprint)


def save_observer(data: dict) -> bool:
    """Save observer metadata.

    Args:
        data: Observer metadata dict (must contain 'key' field)

    Returns:
        True if saved successfully, False otherwise
    """
    observers_dir = get_observers_dir()
    try:
        clean = _persistable_record(data)
        observer_path = observers_dir / _observer_filename(clean)
        atomic_write(observer_path, json.dumps(clean, indent=2))
        os.chmod(observer_path, 0o600)
        ObserverRegistry.singleton().invalidate()
        return True
    except (OSError, ValueError):
        return False


def mint_pl_observer_record(
    fingerprint: str, device_label: str, paired_at: str
) -> Path:
    prefix = _fingerprint_hex(fingerprint)[:16]
    observers_dir = get_observers_dir()
    observer_path = observers_dir / f"{prefix}.json"
    if observer_path.exists():
        raise FileExistsError(observer_path)
    record = {
        "fingerprint": fingerprint,
        "mode": "pl",
        "name": device_label,
        "paired_at": paired_at,
        "created_at": now_ms(),
        "last_seen": None,
        "last_segment": None,
        "enabled": True,
        "stats": {
            "segments_received": 0,
            "bytes_received": 0,
        },
    }
    atomic_write(observer_path, json.dumps(record, indent=2))
    os.chmod(observer_path, 0o600)
    ObserverRegistry.singleton().invalidate()
    return observer_path


def list_observers() -> list[dict]:
    """List all registered observers.

    Returns:
        List of observer metadata dicts, sorted by created_at descending
    """
    return ObserverRegistry.singleton().all()


def find_observer_by_name(name: str) -> dict | None:
    """Find observer metadata by name.

    Args:
        name: Observer name to search for

    Returns:
        Observer metadata dict if found, None otherwise
    """
    return ObserverRegistry.singleton().by_name(name)


def _get_auth_key(url_key: str | None = None) -> str | None:
    from flask import request

    auth = request.headers.get("Authorization", "")
    if auth.startswith("Bearer "):
        bearer = auth[7:].strip()
        if bearer:
            return bearer
    return url_key or None


def _identity_fingerprint() -> str | None:
    from flask import g

    identity = getattr(g, "identity", None)
    if identity is None or identity.mode not in {"pl-direct", "pl-via-spl"}:
        return None
    fingerprint = getattr(identity, "fingerprint", None)
    return fingerprint if isinstance(fingerprint, str) and fingerprint else None


def _auth_failure():
    return error_response(AUTH_REQUIRED, detail="Authorization required")


def _check_observer_enabled(observer: dict):
    if observer.get("revoked", False):
        return error_response(PL_REVOKED, detail="Observer revoked")
    if not observer.get("enabled", True):
        return error_response(FEATURE_UNAVAILABLE, detail="Observer disabled")
    return None


def resolve_observer_identity(url_key: str | None = None):
    fingerprint = _identity_fingerprint()
    if fingerprint is not None:
        observer = load_observer_by_fingerprint(fingerprint)
        if observer is None:
            return None, None, _auth_failure()
        error = _check_observer_enabled(observer)
        if error is not None:
            return None, None, error
        return observer, observer["filename_prefix"], None

    auth_key = _get_auth_key(url_key)
    if not auth_key:
        return None, None, _auth_failure()
    observer = load_observer(auth_key)
    if observer is None:
        return None, None, error_response(AUTH_KEY_INVALID, detail="Invalid key")
    error = _check_observer_enabled(observer)
    if error is not None:
        return None, None, error
    return observer, observer["filename_prefix"], None


def append_history_record(key_prefix: str, day: str, record: dict) -> None:
    """Append a record to the sync history file.

    Args:
        key_prefix: Observer filename prefix
        day: Day string (YYYYMMDD)
        record: Record to append (will be JSON-serialized)
    """
    hist_dir = get_hist_dir(key_prefix)
    hist_path = hist_dir / f"{day}.jsonl"
    with open(hist_path, "a", encoding="utf-8") as f:
        f.write(json.dumps(record, ensure_ascii=False) + "\n")


def load_history(key_prefix: str, day: str) -> list[dict]:
    """Load sync history for an observer on a given day.

    Args:
        key_prefix: Observer filename prefix
        day: Day string (YYYYMMDD)

    Returns:
        List of history records, empty if file doesn't exist
    """
    hist_dir = get_hist_dir(key_prefix, ensure_exists=False)
    hist_path = hist_dir / f"{day}.jsonl"
    if not hist_path.exists():
        return []

    records = []
    try:
        with open(hist_path, encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if line:
                    records.append(json.loads(line))
    except (json.JSONDecodeError, OSError) as e:
        logger.warning(f"Failed to load sync history {hist_path}: {e}")
    return records


def increment_stat(key_prefix: str, stat_name: str) -> None:
    """Increment a stat counter for an observer.

    Args:
        key_prefix: Observer filename prefix
        stat_name: Name of the stat to increment (e.g., 'segments_observed')
    """
    observers_dir = get_observers_dir()
    observer_path = observers_dir / f"{key_prefix}.json"
    if not observer_path.exists():
        return

    try:
        with open(observer_path) as f:
            data = json.load(f)

        data["stats"][stat_name] = data["stats"].get(stat_name, 0) + 1

        atomic_write(observer_path, json.dumps(data, indent=2))
        os.chmod(observer_path, 0o600)
        ObserverRegistry.singleton().invalidate()
    except (json.JSONDecodeError, OSError, KeyError) as e:
        logger.warning(f"Failed to update {stat_name} for {key_prefix}: {e}")


def find_segment_by_sha256(
    key_prefix: str, day: str, file_sha256s: set[str]
) -> tuple[str | None, set[str]]:
    """Find existing segment with matching file SHA256 signatures.

    Searches history records for the given day to find a segment where
    all provided SHA256 hashes match existing files.

    Args:
        key_prefix: Observer filename prefix
        day: Day string (YYYYMMDD)
        file_sha256s: Set of SHA256 hashes to match

    Returns:
        Tuple of (segment_key, matched_sha256s):
        - If full match: (segment_key, all sha256s)
        - If partial match: (None, set of matching sha256s)
        - If no match: (None, empty set)
    """
    records = load_history(key_prefix, day)
    if not records:
        return None, set()

    # Build map of sha256 -> segment for all upload records
    sha256_to_segment: dict[str, str] = {}
    segment_sha256s: dict[str, set[str]] = {}

    for record in records:
        # Skip non-upload records (e.g., "observed" type)
        if record.get("type"):
            continue

        segment = record.get("segment", "")
        if not segment:
            continue

        if segment not in segment_sha256s:
            segment_sha256s[segment] = set()

        for file_rec in record.get("files", []):
            sha256 = file_rec.get("sha256", "")
            if sha256:
                sha256_to_segment[sha256] = segment
                segment_sha256s[segment].add(sha256)

    # Check for full match - all incoming sha256s exist in a single segment
    for segment, existing_sha256s in segment_sha256s.items():
        if file_sha256s and file_sha256s.issubset(existing_sha256s):
            return segment, file_sha256s

    # Check for partial match - some sha256s already exist
    matched = file_sha256s & set(sha256_to_segment.keys())
    return None, matched
