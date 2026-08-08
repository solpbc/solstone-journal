# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Local, durable idempotency state for support portal operations.

This module is the sole writer for ``apps/support/portal/operations`` and its
local fingerprint key.  The server idempotency key is deliberately derived in
memory only: the ledger retains keyed fingerprints, never request content or
the server key itself.
"""

from __future__ import annotations

import base64
import hashlib
import hmac
import json
import math
import os
import re
import unicodedata
import uuid
from collections.abc import Mapping
from dataclasses import dataclass
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any

from solstone.apps.utils import get_app_storage_path
from solstone.convey.reasons import (
    IDEMPOTENCY_CONFLICT,
    OPERATION_IN_PROGRESS,
    OPERATION_RETIRED,
    SUPPORT_INVALID_STATE,
    Reason,
)
from solstone.think.journal_io.atomic import atomic_replace
from solstone.think.journal_io.errors import LockTimeout
from solstone.think.journal_io.locking import hold_lock


CANONICAL_NAMESPACE = "solstone.support.operation-key.v1"
SCHEMA_VERSION = 1
KEY_BYTES = 32
LEASE_DURATION = timedelta(seconds=60)
RETENTION = timedelta(days=45)
_ACTION_ID_RE = re.compile(r"^sact1_[A-Za-z0-9_-]+$")
_TERMINAL_REASON_RE = re.compile(r"^[a-z0-9_]{1,64}$")
_VERB_FIELDS: dict[str, tuple[str, ...]] = {
    "create": (
        "product",
        "subject",
        "description",
        "severity",
        "category",
        "user_email",
        "user_context",
        "anonymous",
    ),
    "reply": ("ticket_id", "content"),
    "attach": (
        "ticket_id",
        "filename",
        "content_type",
        "byte_size",
        "content_sha256",
    ),
    "feedback": ("product", "body", "user_email", "user_context", "anonymous"),
    "close": ("ticket_id",),
    "resolved": ("ticket_id",),
    "still_need_help": ("ticket_id",),
}
_TERMINAL_STATES = frozenset({"completed", "failed", "conflict"})


class OperationError(RuntimeError):
    """Typed local support-operation failure with an optional public reason."""

    def __init__(self, message: str, *, reason: Reason | None = None) -> None:
        super().__init__(message)
        self.reason = reason


class OperationStateUnavailableError(OperationError):
    """The local ledger cannot safely derive or recover an operation key."""


class IdempotencyConflictError(OperationError):
    """One action id was reused with different request content."""

    def __init__(self) -> None:
        super().__init__(IDEMPOTENCY_CONFLICT.message, reason=IDEMPOTENCY_CONFLICT)


class OperationInProgressError(OperationError):
    """A current generation owns an unexpired operation lease."""

    def __init__(self) -> None:
        super().__init__(OPERATION_IN_PROGRESS.message, reason=OPERATION_IN_PROGRESS)


class OperationRetiredError(OperationError):
    """A compacted terminal operation cannot be resumed."""

    def __init__(self) -> None:
        super().__init__(OPERATION_RETIRED.message, reason=OPERATION_RETIRED)


class OperationInvalidStateError(OperationError):
    """A stale generation or illegal lifecycle transition was attempted."""

    def __init__(self) -> None:
        super().__init__(SUPPORT_INVALID_STATE.message, reason=SUPPORT_INVALID_STATE)


@dataclass(frozen=True)
class OperationRecord:
    """In-memory representation of a ledger record.

    ``operation_key`` is intentionally never serialized.  It is present only
    for records returned by :func:`begin_operation`, where canonical input is
    available to derive it.
    """

    parent_action_id: str
    child_action_id: str
    verb: str
    principal_tag: str
    canonical_fingerprint: str
    state: str
    generation: int
    lease_id: str | None
    lease_expires_at: str | None
    remote_operation_id: str | None
    ack_state: str
    completed_at: str | None
    created_at: str
    terminal_reason: str | None = None
    operation_key: str | None = None


def derive_child_action_id(parent_action_id: str, verb: str, index: int = 0) -> str:
    """Return a deterministic child id from a length-prefixed UTF-8 tuple.

    Every tuple member is encoded as a four-byte big-endian byte length followed
    by its UTF-8 bytes.  Length-prefixing avoids delimiter ambiguities even when
    an action id or verb contains a would-be delimiter.
    """
    if index < 0:
        raise ValueError("operation index must be non-negative")
    parts = (parent_action_id, verb, str(index))
    encoded = b"".join(
        len(_utf8(part)).to_bytes(4, "big") + _utf8(part) for part in parts
    )
    return "sact1_" + _b64url(hashlib.sha256(encoded).digest())


def canonicalize_operation(
    verb: str,
    fields: Mapping[str, Any],
    *,
    principal: str,
    child_action_id: str,
) -> bytes:
    """Canonicalize one support action into versioned, strict UTF-8 JSON."""
    if verb not in _VERB_FIELDS:
        raise ValueError(f"unsupported support operation verb: {verb}")
    if not isinstance(fields, Mapping):
        raise TypeError("operation fields must be a mapping")
    if not isinstance(principal, str) or not (
        principal == "anonymous" or principal.startswith("jkt:") and principal[4:]
    ):
        raise ValueError("principal must be anonymous or a jkt thumbprint")

    allowed = set(_VERB_FIELDS[verb])
    unknown = set(fields) - allowed
    if unknown:
        raise ValueError(f"unsupported operation fields: {sorted(unknown)!r}")

    ordered_fields = [
        {"name": name, "value": _field_envelope(fields, name)}
        for name in _VERB_FIELDS[verb]
    ]
    # Dict insertion order is part of Python's language guarantee.  Keeping a
    # normal JSON object also makes the canonical namespace self-describing.
    root = {
        "namespace": CANONICAL_NAMESPACE,
        "principal": _normalize_string(principal),
        "verb": _normalize_string(verb),
        "child_action_id": _normalize_string(child_action_id),
        "fields": ordered_fields,
    }
    return json.dumps(
        root, ensure_ascii=False, separators=(",", ":"), allow_nan=False
    ).encode("utf-8")


def begin_operation(
    parent_action_id: str,
    verb: str,
    fields: Mapping[str, Any],
    *,
    principal: str,
    index: int = 0,
    now: datetime | None = None,
    storage_dir: Path | None = None,
) -> OperationRecord:
    """Create or recover one operation lease and its deterministic server key."""
    current = _coerce_now(now)
    storage = _storage_dir(storage_dir, ensure_exists=True)
    compact_expired_terminal_records(current, storage_dir=storage)
    child_action_id = derive_child_action_id(parent_action_id, verb, index)
    canonical = canonicalize_operation(
        verb, fields, principal=principal, child_action_id=child_action_id
    )
    key = _load_or_create_fingerprint_key(storage)
    fingerprint = _digest(key, b"portal-fingerprint\x00", canonical)
    principal_tag = _digest(key, b"portal-principal\x00", _utf8(principal))
    operation_key = "spk1_" + _b64url(
        hmac.new(key, b"portal-key\x00" + canonical, hashlib.sha256).digest()
    )
    path = _record_path(storage, child_action_id)

    try:
        with hold_lock(path, mode=0o600):
            stored = _read_record(path)
            if stored is None:
                record = OperationRecord(
                    parent_action_id=_normalize_string(parent_action_id),
                    child_action_id=child_action_id,
                    verb=verb,
                    principal_tag=principal_tag,
                    canonical_fingerprint=fingerprint,
                    state="pending",
                    generation=1,
                    lease_id=uuid.uuid4().hex,
                    lease_expires_at=_iso(current + LEASE_DURATION),
                    remote_operation_id=None,
                    ack_state="not_applicable",
                    completed_at=None,
                    created_at=_iso(current),
                    operation_key=operation_key,
                )
                _write_record(path, record)
                return record
            if isinstance(stored, _RetiredRecord):
                raise OperationRetiredError()
            if not hmac.compare_digest(stored.canonical_fingerprint, fingerprint):
                raise IdempotencyConflictError()
            if stored.state == "in_progress" and _lease_is_live(stored, current):
                raise OperationInProgressError()
            if stored.state in {"pending", "in_progress"} and not _lease_is_live(
                stored, current
            ):
                stored = _replace(
                    stored,
                    generation=stored.generation + 1,
                    lease_id=uuid.uuid4().hex,
                    lease_expires_at=_iso(current + LEASE_DURATION),
                )
                _write_record(path, stored)
            return _replace(stored, operation_key=operation_key)
    except LockTimeout as exc:
        raise OperationStateUnavailableError("operation ledger is busy") from exc


def mark_in_progress(
    record: OperationRecord,
    *,
    now: datetime | None = None,
    storage_dir: Path | None = None,
) -> OperationRecord:
    """Transition the current generation from pending to in-progress."""
    current = _coerce_now(now)
    return _update_current(
        record,
        storage_dir=storage_dir,
        update=lambda stored: _mark_in_progress(stored, current),
    )


def mark_completed(
    record: OperationRecord,
    *,
    remote_operation_id: str | None,
    now: datetime | None = None,
    storage_dir: Path | None = None,
) -> OperationRecord:
    """Record a terminal success for the current generation."""
    current = _coerce_now(now)
    return _update_current(
        record,
        storage_dir=storage_dir,
        update=lambda stored: _mark_completed(stored, remote_operation_id, current),
    )


def mark_failed(
    record: OperationRecord,
    *,
    reason: str,
    now: datetime | None = None,
    storage_dir: Path | None = None,
) -> OperationRecord:
    """Record a terminal failure for the current generation."""
    current = _coerce_now(now)
    return _update_current(
        record,
        storage_dir=storage_dir,
        update=lambda stored: _mark_failed(stored, reason, current),
    )


def mark_acknowledged(
    record: OperationRecord,
    *,
    storage_dir: Path | None = None,
) -> OperationRecord:
    """Mark a completed remote operation as acknowledged locally only."""
    return _update_current(
        record, storage_dir=storage_dir, update=_mark_acknowledged
    )


def list_pending_acknowledgements(
    *, storage_dir: Path | None = None
) -> list[OperationRecord]:
    """Return completed, unacknowledged records without mutating the ledger."""
    storage = _storage_dir(storage_dir, ensure_exists=False)
    operations = storage / "operations"
    if not operations.is_dir():
        return []
    records: list[OperationRecord] = []
    for path in sorted(operations.glob("*.json")):
        stored = _read_record(path)
        if isinstance(stored, OperationRecord) and (
            stored.state == "completed" and stored.ack_state == "unacknowledged"
        ):
            records.append(stored)
    return records


def compact_expired_terminal_records(
    now: datetime | None = None,
    *,
    storage_dir: Path | None = None,
) -> None:
    """Replace terminal records older than retention with refusal-only markers."""
    current = _coerce_now(now)
    storage = _storage_dir(storage_dir, ensure_exists=True)
    operations = storage / "operations"
    if not operations.is_dir():
        return
    for path in sorted(operations.glob("*.json")):
        try:
            with hold_lock(path, mode=0o600):
                stored = _read_record(path)
                if not isinstance(stored, OperationRecord):
                    continue
                if stored.state not in _TERMINAL_STATES or not stored.completed_at:
                    continue
                if current - _parse_iso(stored.completed_at) <= RETENTION:
                    continue
                marker = {
                    "schema_version": SCHEMA_VERSION,
                    "child_action_id": stored.child_action_id,
                    "terminal_reason": stored.terminal_reason or stored.state,
                }
                atomic_replace(
                    path,
                    json.dumps(marker, separators=(",", ":")) + "\n",
                    mode=0o600,
                )
        except LockTimeout as exc:
            raise OperationStateUnavailableError("operation ledger is busy") from exc


@dataclass(frozen=True)
class _RetiredRecord:
    child_action_id: str
    terminal_reason: str


def _mark_in_progress(record: OperationRecord, current: datetime) -> OperationRecord:
    if record.state != "pending" or not _lease_is_live(record, current):
        raise OperationInvalidStateError()
    return _replace(record, state="in_progress")


def _mark_completed(
    record: OperationRecord, remote_operation_id: str | None, current: datetime
) -> OperationRecord:
    if record.state != "in_progress" or not _lease_is_live(record, current):
        raise OperationInvalidStateError()
    return _replace(
        record,
        state="completed",
        remote_operation_id=remote_operation_id,
        ack_state="unacknowledged" if remote_operation_id else "not_applicable",
        completed_at=_iso(current),
        lease_id=None,
        lease_expires_at=None,
        terminal_reason=None,
    )


def _mark_failed(record: OperationRecord, reason: str, current: datetime) -> OperationRecord:
    if record.state not in {"pending", "in_progress"} or not _lease_is_live(record, current):
        raise OperationInvalidStateError()
    if not isinstance(reason, str) or not _TERMINAL_REASON_RE.fullmatch(reason):
        raise ValueError("operation failure reason must be an opaque reason code")
    return _replace(
        record,
        state="failed",
        ack_state="not_applicable",
        completed_at=_iso(current),
        lease_id=None,
        lease_expires_at=None,
        terminal_reason=reason,
    )


def _mark_acknowledged(record: OperationRecord) -> OperationRecord:
    if record.state != "completed":
        raise OperationInvalidStateError()
    if record.ack_state == "acknowledged":
        return record
    if record.ack_state != "unacknowledged":
        raise OperationInvalidStateError()
    return _replace(record, ack_state="acknowledged")


def _update_current(
    record: OperationRecord,
    *,
    storage_dir: Path | None,
    update: Any,
) -> OperationRecord:
    storage = _storage_dir(storage_dir, ensure_exists=True)
    path = _record_path(storage, record.child_action_id)
    try:
        with hold_lock(path, mode=0o600):
            stored = _read_record(path)
            if not isinstance(stored, OperationRecord) or stored.generation != record.generation:
                raise OperationInvalidStateError()
            updated = update(stored)
            _write_record(path, updated)
            return _replace(updated, operation_key=record.operation_key)
    except LockTimeout as exc:
        raise OperationStateUnavailableError("operation ledger is busy") from exc


def _storage_dir(storage_dir: Path | None, *, ensure_exists: bool) -> Path:
    return (
        Path(storage_dir)
        if storage_dir is not None
        else get_app_storage_path("support", "portal", ensure_exists=ensure_exists)
    )


def _load_or_create_fingerprint_key(storage: Path) -> bytes:
    key_path = storage / "operation-fingerprint.key"
    try:
        with hold_lock(key_path, mode=0o600):
            if key_path.exists():
                try:
                    if key_path.stat().st_mode & 0o077:
                        raise OperationStateUnavailableError(
                            "operation fingerprint key permissions are unsafe"
                        )
                    key = key_path.read_bytes()
                except OSError as exc:
                    raise OperationStateUnavailableError(
                        "operation fingerprint key is unreadable"
                    ) from exc
                if len(key) != KEY_BYTES:
                    raise OperationStateUnavailableError("operation fingerprint key is invalid")
                return key
            if _has_operation_artifacts(storage / "operations"):
                raise OperationStateUnavailableError("operation fingerprint key is unavailable")
            key = os.urandom(KEY_BYTES)
            atomic_replace(key_path, key, mode=0o600)
            return key
    except LockTimeout as exc:
        raise OperationStateUnavailableError("operation fingerprint key is busy") from exc


def _has_operation_artifacts(operations: Path) -> bool:
    return operations.exists() and any(path.is_file() for path in operations.rglob("*"))


def _record_path(storage: Path, child_action_id: str) -> Path:
    if not _ACTION_ID_RE.fullmatch(child_action_id):
        raise OperationStateUnavailableError("invalid operation action id")
    return storage / "operations" / f"{child_action_id}.json"


def _read_record(path: Path) -> OperationRecord | _RetiredRecord | None:
    if not path.exists():
        return None
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise OperationStateUnavailableError("operation ledger record is unreadable") from exc
    if not isinstance(value, dict) or value.get("schema_version") != SCHEMA_VERSION:
        raise OperationStateUnavailableError("operation ledger record is invalid")
    if set(value) == {"schema_version", "child_action_id", "terminal_reason"}:
        return _RetiredRecord(value["child_action_id"], value["terminal_reason"])
    required = {
        "schema_version",
        "parent_action_id",
        "child_action_id",
        "verb",
        "principal_tag",
        "canonical_fingerprint",
        "state",
        "generation",
        "lease_id",
        "lease_expires_at",
        "remote_operation_id",
        "ack_state",
        "completed_at",
        "created_at",
        "terminal_reason",
    }
    if set(value) != required:
        raise OperationStateUnavailableError("operation ledger record is invalid")
    try:
        return OperationRecord(
            **{name: value[name] for name in required if name != "schema_version"}
        )
    except TypeError as exc:
        raise OperationStateUnavailableError("operation ledger record is invalid") from exc


def _write_record(path: Path, record: OperationRecord) -> None:
    payload = {
        "schema_version": SCHEMA_VERSION,
        "parent_action_id": record.parent_action_id,
        "child_action_id": record.child_action_id,
        "verb": record.verb,
        "principal_tag": record.principal_tag,
        "canonical_fingerprint": record.canonical_fingerprint,
        "state": record.state,
        "generation": record.generation,
        "lease_id": record.lease_id,
        "lease_expires_at": record.lease_expires_at,
        "remote_operation_id": record.remote_operation_id,
        "ack_state": record.ack_state,
        "completed_at": record.completed_at,
        "created_at": record.created_at,
        "terminal_reason": record.terminal_reason,
    }
    atomic_replace(path, json.dumps(payload, separators=(",", ":")) + "\n", mode=0o600)


def _field_envelope(fields: Mapping[str, Any], name: str) -> dict[str, Any]:
    if name not in fields:
        return {"present": False}
    return {"present": True, "value": _canonical_value(fields[name])}


def _canonical_value(value: Any) -> Any:
    if isinstance(value, str):
        return _normalize_string(value)
    if value is None or isinstance(value, bool) or isinstance(value, int):
        return value
    if isinstance(value, float):
        if not math.isfinite(value):
            raise ValueError("operation values cannot contain non-finite floats")
        return value
    if isinstance(value, Mapping):
        normalized: list[tuple[str, Any]] = []
        seen: set[bytes] = set()
        for key, item in value.items():
            if not isinstance(key, str):
                raise TypeError("operation map keys must be strings")
            normalized_key = _normalize_string(key)
            key_bytes = _utf8(normalized_key)
            if key_bytes in seen:
                raise ValueError("operation map keys collide after normalization")
            seen.add(key_bytes)
            normalized.append((normalized_key, _canonical_value(item)))
        return {key: item for key, item in sorted(normalized, key=lambda pair: _utf8(pair[0]))}
    if isinstance(value, list):
        return [_canonical_value(item) for item in value]
    raise TypeError(f"unsupported operation value type: {type(value).__name__}")


def _normalize_string(value: str) -> str:
    if any(0xD800 <= ord(char) <= 0xDFFF for char in value):
        raise ValueError("operation strings cannot contain surrogate code points")
    return unicodedata.normalize("NFC", value)


def _utf8(value: str) -> bytes:
    return _normalize_string(value).encode("utf-8")


def _digest(key: bytes, prefix: bytes, payload: bytes) -> str:
    return hmac.new(key, prefix + payload, hashlib.sha256).hexdigest()


def _b64url(value: bytes) -> str:
    return base64.urlsafe_b64encode(value).rstrip(b"=").decode("ascii")


def _coerce_now(now: datetime | None) -> datetime:
    if now is None:
        return datetime.now(UTC)
    if now.tzinfo is None:
        raise ValueError("operation time must be timezone-aware")
    return now.astimezone(UTC)


def _iso(value: datetime) -> str:
    return value.astimezone(UTC).isoformat()


def _parse_iso(value: str) -> datetime:
    parsed = datetime.fromisoformat(value)
    if parsed.tzinfo is None:
        raise OperationStateUnavailableError("operation timestamp is invalid")
    return parsed.astimezone(UTC)


def _lease_is_live(record: OperationRecord, current: datetime) -> bool:
    return bool(
        record.lease_id
        and record.lease_expires_at
        and _parse_iso(record.lease_expires_at) > current
    )


def _replace(record: OperationRecord, **changes: Any) -> OperationRecord:
    values = record.__dict__ | changes
    return OperationRecord(**values)
