# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Durable media-offload ledger.

This ledger is JSONL, not sqlite, because backup excludes ``*.sqlite*`` while
``health/`` is included. Losing the journal must not lose the map to the
owner's only backed-up copy of their media. It is fsync'd even though the
nearby pruning audit writer is not: pruning audits are post-hoc records, while
this ledger is a pre-mark witness for a later path that mints a pending-approval
removal mark immediately after append returns.

Per-file records intentionally use ``name`` and ``bytes`` to match the existing
raw-media audit shape, but use ``sha256`` instead of ``hash`` because this
durable schema rides encrypted backups and may be read years later by a restore
flow. A future delete path should compute each digest once, feed ``sha256`` to
this ledger, and remap only that key to the audit writer's ``hash`` at the call
site; it must not hash twice.

Restore events carry only the identity spine and time. Repeating old file or
snapshot facts on restore would invite ambiguous folded reads; restore simply
invalidates the current offload state for a segment.

Fold order is append order. Timestamps are informational, so a clock step
backward must not change the winning event. Read degradation is also deliberate:
unlike the repo's usual fail-loudly rule, a crashing read could let a future
teardown gate lose the owner's only media copy. A degraded zero is not a clean
zero; teardown gates must check ``degraded is False`` before trusting zero
offloaded bytes or segments.
"""

from __future__ import annotations

import json
import logging
import re
import time as time_module
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Literal

from solstone.think.journal_io import append_jsonl
from solstone.think.utils import (
    DATE_RE,
    DEFAULT_STREAM,
    STREAM_RE,
    get_journal,
    segment_key,
)

logger = logging.getLogger(__name__)

EVENT_OFFLOAD = "offload"
EVENT_RESTORE = "restore"
SHA256_RE = re.compile(r"[0-9a-f]{64}")


@dataclass(frozen=True)
class OffloadFile:
    name: str
    bytes: int
    sha256: str


@dataclass(frozen=True)
class SegmentOffloadSummary:
    day: str
    stream: str
    segment: str
    currently_offloaded: bool
    snapshot_id: str | None
    files: tuple[OffloadFile, ...]
    offloaded_bytes: int
    offloaded_file_count: int
    skipped_records: int
    unreadable_ledgers: tuple[str, ...]

    @property
    def degraded(self) -> bool:
        return bool(self.unreadable_ledgers) or self.skipped_records > 0


@dataclass(frozen=True)
class DayOffloadSummary:
    day: str
    segments: tuple[SegmentOffloadSummary, ...]
    offloaded_bytes: int
    offloaded_file_count: int
    offloaded_segments: int
    skipped_records: int
    unreadable_ledgers: tuple[str, ...]

    @property
    def degraded(self) -> bool:
        return bool(self.unreadable_ledgers) or self.skipped_records > 0


@dataclass(frozen=True)
class JournalOffloadSummary:
    days: tuple[DayOffloadSummary, ...]
    offloaded_bytes: int
    offloaded_file_count: int
    offloaded_segments: int
    offloaded_days: int
    skipped_records: int
    unreadable_ledgers: tuple[str, ...]

    @property
    def degraded(self) -> bool:
        return bool(self.unreadable_ledgers) or self.skipped_records > 0


@dataclass(frozen=True)
class _LedgerEvent:
    event_kind: Literal["offload", "restore"]
    time: int
    day: str
    stream: str
    segment: str
    snapshot_id: str | None
    files: tuple[OffloadFile, ...]


@dataclass(frozen=True)
class _LedgerRead:
    events: tuple[_LedgerEvent, ...]
    skipped_records: int
    unreadable_ledgers: tuple[str, ...]


def append_offload_event(
    *,
    day: str,
    stream: str,
    segment: str,
    snapshot_id: str,
    files: tuple[OffloadFile, ...] | list[OffloadFile],
    time: int | None = None,
) -> None:
    event_time = _write_event_time(time)
    _validate_identity(day, stream, segment)
    if not isinstance(snapshot_id, str) or not snapshot_id:
        raise ValueError("snapshot_id must be a non-empty string")
    if not files:
        raise ValueError("files must not be empty")
    validated_files = tuple(_validate_file_record(file) for file in files)

    append_jsonl(
        _ledger_path(day),
        {
            "event_kind": EVENT_OFFLOAD,
            "time": event_time,
            "day": day,
            "stream": stream,
            "segment": segment,
            "snapshot_id": snapshot_id,
            "files": [_file_to_record(file) for file in validated_files],
        },
    )


def append_restore_event(
    *,
    day: str,
    stream: str,
    segment: str,
    time: int | None = None,
) -> None:
    event_time = _write_event_time(time)
    _validate_identity(day, stream, segment)

    append_jsonl(
        _ledger_path(day),
        {
            "event_kind": EVENT_RESTORE,
            "time": event_time,
            "day": day,
            "stream": stream,
            "segment": segment,
        },
    )


def summarize_segment(day: str, stream: str, segment: str) -> SegmentOffloadSummary:
    _validate_identity(day, stream, segment)
    read = _read_ledger_day(day)
    states = _fold_events(read.events)
    current = states.get((day, stream, segment))
    return _segment_summary(
        day,
        stream,
        segment,
        current,
        skipped_records=read.skipped_records,
        unreadable_ledgers=read.unreadable_ledgers,
    )


def summarize_day(day: str) -> DayOffloadSummary:
    if not DATE_RE.fullmatch(day):
        raise ValueError("day must be in YYYYMMDD format")
    read = _read_ledger_day(day)
    states = _fold_events(read.events)
    segments = tuple(
        _segment_summary(
            key_day,
            stream,
            segment,
            current,
            skipped_records=read.skipped_records,
            unreadable_ledgers=read.unreadable_ledgers,
        )
        for (key_day, stream, segment), current in sorted(states.items())
        if key_day == day
    )
    return _day_summary(
        day,
        segments,
        skipped_records=read.skipped_records,
        unreadable_ledgers=read.unreadable_ledgers,
    )


def summarize_journal() -> JournalOffloadSummary:
    ledger_dir = _ledger_dir()
    if not ledger_dir.is_dir():
        return JournalOffloadSummary(
            days=(),
            offloaded_bytes=0,
            offloaded_file_count=0,
            offloaded_segments=0,
            offloaded_days=0,
            skipped_records=0,
            unreadable_ledgers=(),
        )

    days = tuple(
        summarize_day(path.stem)
        for path in sorted(ledger_dir.glob("*.jsonl"))
        if DATE_RE.fullmatch(path.stem)
    )
    unreadable_ledgers = tuple(
        ledger for day in days for ledger in day.unreadable_ledgers
    )
    return JournalOffloadSummary(
        days=days,
        offloaded_bytes=sum(day.offloaded_bytes for day in days),
        offloaded_file_count=sum(day.offloaded_file_count for day in days),
        offloaded_segments=sum(day.offloaded_segments for day in days),
        offloaded_days=sum(1 for day in days if day.offloaded_segments > 0),
        skipped_records=sum(day.skipped_records for day in days),
        unreadable_ledgers=unreadable_ledgers,
    )


def ledger_path_for_day(day: str) -> Path:
    if not DATE_RE.fullmatch(day):
        raise ValueError("day must be in YYYYMMDD format")
    return _ledger_path(day)


def _ledger_dir() -> Path:
    return Path(get_journal()) / "health" / "offload"


def _ledger_path(day: str) -> Path:
    return _ledger_dir() / f"{day}.jsonl"


def _write_event_time(value: int | None) -> int:
    if value is None:
        return int(time_module.time())
    return _read_event_time(value)


def _read_event_time(value: Any) -> int:
    if type(value) is not int or value < 0:
        raise ValueError("time must be a non-negative integer epoch second")
    return value


def _validate_identity(day: str, stream: str, segment: str) -> None:
    if not isinstance(day, str) or not DATE_RE.fullmatch(day):
        raise ValueError("day must be in YYYYMMDD format")
    if not isinstance(stream, str) or not (
        stream == DEFAULT_STREAM or STREAM_RE.fullmatch(stream)
    ):
        raise ValueError("stream must be a valid stream name")
    if not isinstance(segment, str) or segment_key(segment) != segment:
        raise ValueError("segment must be an exact segment key")


def _validate_file_record(file: OffloadFile) -> OffloadFile:
    if not isinstance(file, OffloadFile):
        raise ValueError("files must contain OffloadFile records")
    _validate_file_fields(file.name, file.bytes, file.sha256)
    return file


def _validate_file_fields(name: Any, size: Any, sha256: Any) -> None:
    if not isinstance(name, str) or not name or "/" in name or name in {".", ".."}:
        raise ValueError("file name must be a segment-local basename")
    if type(size) is not int or size < 0:
        raise ValueError("file bytes must be a non-negative integer")
    if not isinstance(sha256, str) or SHA256_RE.fullmatch(sha256) is None:
        raise ValueError("file sha256 must be a lowercase 64-character hex digest")


def _file_to_record(file: OffloadFile) -> dict[str, Any]:
    return {"name": file.name, "bytes": file.bytes, "sha256": file.sha256}


def _read_ledger_day(day: str) -> _LedgerRead:
    return _read_ledger_file(_ledger_path(day), expected_day=day)


def _read_ledger_file(path: Path, *, expected_day: str | None) -> _LedgerRead:
    if not path.exists():
        return _LedgerRead(events=(), skipped_records=0, unreadable_ledgers=())

    try:
        raw = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        logger.warning("offload ledger read degraded for %s: %s", path, exc)
        return _LedgerRead(
            events=(), skipped_records=0, unreadable_ledgers=(str(path),)
        )

    events: list[_LedgerEvent] = []
    skipped_records = 0
    for lineno, line in enumerate(raw.splitlines(), start=1):
        if not line.strip():
            continue
        try:
            record = json.loads(line)
            event = _parse_event(record)
            if expected_day is not None and event.day != expected_day:
                raise ValueError("record day does not match ledger file day")
        except json.JSONDecodeError:
            skipped_records += 1
            logger.warning(
                "skipping malformed offload ledger line %d in %s", lineno, path
            )
            continue
        except (TypeError, ValueError) as exc:
            skipped_records += 1
            logger.warning(
                "skipping invalid offload ledger record line %d in %s: %s",
                lineno,
                path,
                exc,
            )
            continue
        events.append(event)
    return _LedgerRead(
        events=tuple(events), skipped_records=skipped_records, unreadable_ledgers=()
    )


def _parse_event(record: Any) -> _LedgerEvent:
    if not isinstance(record, dict):
        raise ValueError("record must be a JSON object")
    event_kind = record.get("event_kind")
    if event_kind == EVENT_OFFLOAD:
        expected = {
            "event_kind",
            "time",
            "day",
            "stream",
            "segment",
            "snapshot_id",
            "files",
        }
        if set(record) != expected:
            raise ValueError("offload record has unexpected fields")
        _validate_identity(record["day"], record["stream"], record["segment"])
        event_time = _read_event_time(record["time"])
        snapshot_id = record["snapshot_id"]
        if not isinstance(snapshot_id, str) or not snapshot_id:
            raise ValueError("snapshot_id must be a non-empty string")
        raw_files = record["files"]
        if not isinstance(raw_files, list) or not raw_files:
            raise ValueError("files must be a non-empty list")
        files = tuple(_parse_file_record(file) for file in raw_files)
        return _LedgerEvent(
            event_kind=EVENT_OFFLOAD,
            time=event_time,
            day=record["day"],
            stream=record["stream"],
            segment=record["segment"],
            snapshot_id=snapshot_id,
            files=files,
        )

    if event_kind == EVENT_RESTORE:
        expected = {"event_kind", "time", "day", "stream", "segment"}
        if set(record) != expected:
            raise ValueError("restore record has unexpected fields")
        _validate_identity(record["day"], record["stream"], record["segment"])
        return _LedgerEvent(
            event_kind=EVENT_RESTORE,
            time=_read_event_time(record["time"]),
            day=record["day"],
            stream=record["stream"],
            segment=record["segment"],
            snapshot_id=None,
            files=(),
        )

    raise ValueError("event_kind must be offload or restore")


def _parse_file_record(record: Any) -> OffloadFile:
    if not isinstance(record, dict) or set(record) != {"name", "bytes", "sha256"}:
        raise ValueError("file record must contain name, bytes, sha256")
    _validate_file_fields(record["name"], record["bytes"], record["sha256"])
    return OffloadFile(
        name=record["name"],
        bytes=record["bytes"],
        sha256=record["sha256"],
    )


def _fold_events(
    events: tuple[_LedgerEvent, ...],
) -> dict[tuple[str, str, str], _LedgerEvent]:
    states: dict[tuple[str, str, str], _LedgerEvent] = {}
    for event in events:
        key = (event.day, event.stream, event.segment)
        if event.event_kind == EVENT_OFFLOAD:
            states[key] = event
        elif event.event_kind == EVENT_RESTORE:
            states.pop(key, None)
        else:  # pragma: no cover - parser owns the closed vocabulary.
            raise RuntimeError(f"unexpected offload event kind: {event.event_kind}")
    return states


def _segment_summary(
    day: str,
    stream: str,
    segment: str,
    current: _LedgerEvent | None,
    *,
    skipped_records: int,
    unreadable_ledgers: tuple[str, ...],
) -> SegmentOffloadSummary:
    files = current.files if current is not None else ()
    return SegmentOffloadSummary(
        day=day,
        stream=stream,
        segment=segment,
        currently_offloaded=current is not None,
        snapshot_id=current.snapshot_id if current is not None else None,
        files=files,
        offloaded_bytes=sum(file.bytes for file in files),
        offloaded_file_count=len(files),
        skipped_records=skipped_records,
        unreadable_ledgers=unreadable_ledgers,
    )


def _day_summary(
    day: str,
    segments: tuple[SegmentOffloadSummary, ...],
    *,
    skipped_records: int,
    unreadable_ledgers: tuple[str, ...],
) -> DayOffloadSummary:
    return DayOffloadSummary(
        day=day,
        segments=segments,
        offloaded_bytes=sum(segment.offloaded_bytes for segment in segments),
        offloaded_file_count=sum(segment.offloaded_file_count for segment in segments),
        offloaded_segments=sum(
            1 for segment in segments if segment.currently_offloaded
        ),
        skipped_records=skipped_records,
        unreadable_ledgers=unreadable_ledgers,
    )


__all__ = [
    "DayOffloadSummary",
    "EVENT_OFFLOAD",
    "EVENT_RESTORE",
    "JournalOffloadSummary",
    "OffloadFile",
    "SegmentOffloadSummary",
    "append_offload_event",
    "append_restore_event",
    "ledger_path_for_day",
    "summarize_day",
    "summarize_journal",
    "summarize_segment",
]
