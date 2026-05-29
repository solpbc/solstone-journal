# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Health consumer surface for journal-data trust signals.

This surface reports on capture, synthesis, and consumer-facing trust signals
derived from journal data. For infrastructure and service liveness, use
``sol health`` instead.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any, Iterator

from solstone.think.activities import load_activity_records
from solstone.think.entities.journal import load_all_journal_entities
from solstone.think.facets import get_facets
from solstone.think.pipeline_health import read_segment_backlog
from solstone.think.surfaces import ledger
from solstone.think.surfaces.types import (
    CaptureHealth,
    ConsumerSignalHealth,
    HealthNote,
    HealthReport,
    SegmentBacklogHealth,
    SynthesisHealth,
)
from solstone.think.utils import get_journal, segment_parse

FACET_SILENT_INFO_HOURS = 24
# 24h is the first trust-signal rung: a facet that has been quiet for a full day should surface as informational drift before it becomes an operational concern.
FACET_SILENT_WARN_HOURS = 72
# 72h is a stronger silence signal than 24h without yet implying an outright break; warn keeps the ladder graduated before the weekly threshold.
FACET_SILENT_CRITICAL_HOURS = 168
# 168h (7d) is the highest silent-facet rung: a facet quiet for a full week is likely broken, muted in practice, or missing expected capture.
INDEXER_STALE_WARN_DAYS = 7
# 7d matches the weekly freshness bar for search-backed consumers; shorter windows would over-warn on journals that intentionally rebuild less often.
LEDGER_STALE_DAYS = 14
# 14d mirrors the consumer-signal stale-item threshold so the health surface stays aligned with ledger backlog review.
USER_EDIT_ACTOR_PREFIXES = ("cli:", "owner", "user")
# These prefixes identify operator- or user-authored corrections without trying to enumerate every internal automation actor string.
_DAY_MS = 86_400_000
_HOUR_MS = 3_600_000
_SPEC_POINTER = "cpo/specs/in-flight/consumer-surface-health.md"


def _resolve_now() -> datetime:
    return datetime.now(UTC)


@dataclass(frozen=True)
class _ScanAggregate:
    capture_hour_slots: frozenset[tuple[str, int]]
    last_segment_at: int | None
    activities_count: int
    activities_with_participation: int
    activities_with_story: int
    activities_user_edited: int
    activities_anticipated_unfilled: int


def _resolve_day(day: str) -> str:
    try:
        datetime.strptime(day, "%Y%m%d")
    except ValueError as exc:
        raise ValueError("day must match YYYYMMDD") from exc
    return day


def _resolve_day_start(day: str) -> datetime:
    return datetime.strptime(_resolve_day(day), "%Y%m%d").replace(tzinfo=UTC)


def _parse_segment_bounds(
    raw_segment: str, day: str
) -> tuple[datetime, datetime] | None:
    start_time, _ = segment_parse(raw_segment)
    if start_time is None:
        return None

    try:
        duration_seconds = int(raw_segment.split("_", 1)[1])
    except (IndexError, ValueError):
        return None

    start_of_day = _resolve_day_start(day)
    start_dt = start_of_day.replace(
        hour=start_time.hour,
        minute=start_time.minute,
        second=start_time.second,
        microsecond=0,
    )
    return start_dt, start_dt + timedelta(seconds=duration_seconds)


def _iter_range_facet_days(day_from: str, day_to: str) -> Iterator[tuple[str, str]]:
    start_day = _resolve_day_start(day_from)
    end_day = _resolve_day_start(day_to)
    facets = tuple(sorted(get_facets().keys()))
    current = start_day
    while current <= end_day:
        day = current.strftime("%Y%m%d")
        for facet in facets:
            yield facet, day
        current += timedelta(days=1)


def _parse_segment_hour_slots(segments: object, day: str) -> set[tuple[str, int]]:
    if not isinstance(segments, list):
        return set()

    slots: set[tuple[str, int]] = set()
    start_of_day = _resolve_day_start(day)
    start_of_next_day = start_of_day + timedelta(days=1)
    for raw_segment in segments:
        if not isinstance(raw_segment, str):
            continue
        bounds = _parse_segment_bounds(raw_segment, day)
        if bounds is None:
            continue
        start_dt, end_dt = bounds
        clipped_end = min(end_dt, start_of_next_day)
        current = start_dt.replace(minute=0, second=0, microsecond=0)
        while current < clipped_end:
            slots.add((day, current.hour))
            current += timedelta(hours=1)
    return slots


def _count_user_edits(record: dict[str, Any]) -> int:
    edits = record.get("edits")
    if not isinstance(edits, list):
        return 0

    count = 0
    for edit in edits:
        if not isinstance(edit, dict):
            continue
        actor = edit.get("actor")
        if isinstance(actor, str) and actor.startswith(USER_EDIT_ACTOR_PREFIXES):
            count += 1
    return count


def _scan_records(day_from: str, day_to: str) -> _ScanAggregate:
    capture_hour_slots: set[tuple[str, int]] = set()
    last_segment_at: int | None = None
    activities_count = 0
    activities_with_participation = 0
    activities_with_story = 0
    activities_user_edited = 0
    activities_anticipated_unfilled = 0
    generated_at_ms = int(_resolve_now().timestamp() * 1000)

    for facet, day in _iter_range_facet_days(day_from, day_to):
        for record in load_activity_records(facet, day, include_hidden=True):
            capture_hour_slots.update(
                _parse_segment_hour_slots(record.get("segments"), day)
            )

            segments = record.get("segments")
            if isinstance(segments, list):
                for raw_segment in segments:
                    if not isinstance(raw_segment, str):
                        continue
                    bounds = _parse_segment_bounds(raw_segment, day)
                    if bounds is None:
                        continue
                    _, end_dt = bounds
                    end_ms = int(end_dt.timestamp() * 1000)
                    if last_segment_at is None or end_ms > last_segment_at:
                        last_segment_at = end_ms

            if bool(record.get("hidden", False)):
                continue

            activities_count += 1
            if record.get("participation"):
                activities_with_participation += 1
            if record.get("story"):
                activities_with_story += 1
            if _count_user_edits(record) > 0:
                activities_user_edited += 1

            if record.get("source") == "anticipated" and not bool(
                record.get("cancelled", False)
            ):
                start_value = record.get("start")
                if isinstance(start_value, str):
                    try:
                        start_dt = datetime.fromisoformat(
                            start_value.replace("Z", "+00:00")
                        )
                    except ValueError:
                        start_dt = None
                    if start_dt is not None:
                        if start_dt.tzinfo is None:
                            start_dt = start_dt.replace(tzinfo=UTC)
                        if int(start_dt.timestamp() * 1000) <= generated_at_ms:
                            activities_anticipated_unfilled += 1

    return _ScanAggregate(
        capture_hour_slots=frozenset(capture_hour_slots),
        last_segment_at=last_segment_at,
        activities_count=activities_count,
        activities_with_participation=activities_with_participation,
        activities_with_story=activities_with_story,
        activities_user_edited=activities_user_edited,
        activities_anticipated_unfilled=activities_anticipated_unfilled,
    )


def _last_segment_ts_per_facet() -> dict[str, int | None]:
    facets = tuple(sorted(get_facets().keys()))
    current = _resolve_now()
    days = [
        (current - timedelta(days=offset)).strftime("%Y%m%d")
        for offset in range(7, -1, -1)
    ]
    last_seen: dict[str, int | None] = {facet: None for facet in facets}

    for facet in facets:
        for day in days:
            for record in load_activity_records(facet, day, include_hidden=True):
                segments = record.get("segments")
                if not isinstance(segments, list):
                    continue
                for raw_segment in segments:
                    if not isinstance(raw_segment, str):
                        continue
                    bounds = _parse_segment_bounds(raw_segment, day)
                    if bounds is None:
                        continue
                    _, end_dt = bounds
                    end_ms = int(end_dt.timestamp() * 1000)
                    if last_seen[facet] is None or end_ms > last_seen[facet]:
                        last_seen[facet] = end_ms

    return last_seen


def _build_capture_health(
    aggregate: _ScanAggregate,
    report_range: tuple[str, str],
    facets: tuple[str, ...],
    generated_at: int,
) -> tuple[CaptureHealth, list[HealthNote]]:
    last_seen_by_facet = _last_segment_ts_per_facet()
    recent_cutoff = generated_at - (FACET_SILENT_INFO_HOURS * _HOUR_MS)
    recent_facets = tuple(
        sorted(
            facet
            for facet in facets
            if last_seen_by_facet.get(facet) is not None
            and int(last_seen_by_facet[facet] or 0) >= recent_cutoff
        )
    )
    recent_facet_set = set(recent_facets)
    silent_facets = tuple(
        sorted(facet for facet in facets if facet not in recent_facet_set)
    )

    day_from, day_to = report_range
    hours_total = (
        (_resolve_day_start(day_to) - _resolve_day_start(day_from)).days + 1
    ) * 24
    notes = [
        HealthNote(
            severity="info",
            category="capture",
            message="coverage_ratio unavailable in v1 — expected-hours denominator arrives Sprint 5+",
            detected_at=generated_at,
            detail_pointer=_SPEC_POINTER,
        )
    ]

    for facet in facets:
        last_seen = last_seen_by_facet.get(facet)
        if last_seen is None:
            notes.append(
                HealthNote(
                    severity="info",
                    category="capture",
                    message=f"{facet}: no captures recorded in the last 7 days.",
                    detected_at=generated_at,
                    detail_pointer=None,
                )
            )
            continue

        gap_hours = max(0, (generated_at - last_seen) // _HOUR_MS)
        if gap_hours >= FACET_SILENT_CRITICAL_HOURS:
            severity = "critical"
        elif gap_hours >= FACET_SILENT_WARN_HOURS:
            severity = "warn"
        elif gap_hours >= FACET_SILENT_INFO_HOURS:
            severity = "info"
        else:
            continue

        last_seen_text = (
            datetime.fromtimestamp(last_seen / 1000, tz=UTC)
            .isoformat()
            .replace("+00:00", "Z")
        )
        notes.append(
            HealthNote(
                severity=severity,
                category="capture",
                message=f"{facet}: last capture {gap_hours}h ago ({last_seen_text}).",
                detected_at=generated_at,
                detail_pointer=None,
            )
        )

    return (
        CaptureHealth(
            hours_with_capture=len(aggregate.capture_hour_slots),
            hours_total=hours_total,
            coverage_ratio=None,
            facets_with_recent_capture=recent_facets,
            facets_silent_24h=silent_facets,
            last_segment_at=aggregate.last_segment_at,
        ),
        notes,
    )


def _build_synthesis_health(
    aggregate: _ScanAggregate,
    generated_at: int,
) -> tuple[SynthesisHealth, list[HealthNote]]:
    notes = [
        HealthNote(
            severity="info",
            category="synthesis",
            message="corrections roll-up not available — corrections ledger exists only from Sprint 5+",
            detected_at=generated_at,
            detail_pointer=_SPEC_POINTER,
        )
    ]

    generated_at_dt = datetime.fromtimestamp(generated_at / 1000, tz=UTC)
    talent_days = (
        generated_at_dt.strftime("%Y%m%d"),
        (generated_at_dt - timedelta(days=1)).strftime("%Y%m%d"),
    )
    talents_dir = Path(get_journal()) / "talents"
    talent_rows: list[dict[str, Any]] = []
    missing_talent_days: list[str] = []
    for day in talent_days:
        path = talents_dir / f"{day}.jsonl"
        if not path.exists():
            missing_talent_days.append(day)
            continue
        with path.open(encoding="utf-8") as handle:
            for raw_line in handle:
                line = raw_line.strip()
                if not line:
                    continue
                try:
                    payload = json.loads(line)
                except ValueError:
                    continue
                if isinstance(payload, dict):
                    talent_rows.append(payload)

    talent_run_failures_24h: int | None
    if missing_talent_days:
        talent_run_failures_24h = None
        notes.append(
            HealthNote(
                severity="info",
                category="synthesis",
                message=(
                    "talent day-index logs missing for "
                    + ", ".join(missing_talent_days)
                    + "; last-24h failure count unavailable."
                ),
                detected_at=generated_at,
                detail_pointer=None,
            )
        )
    else:
        talent_run_failures_24h = 0
        cutoff = generated_at - _DAY_MS
        for row in talent_rows:
            try:
                timestamp = int(row.get("ts"))  # type: ignore[arg-type]
            except (TypeError, ValueError):
                continue
            if timestamp < cutoff or timestamp > generated_at:
                continue
            status = row.get("status")
            if row.get("error") or status not in ("ok", "completed", None):
                talent_run_failures_24h += 1

    indexer_path = Path(get_journal()) / "indexer" / "journal.sqlite"
    if not indexer_path.exists():
        indexer_last_rebuild_at = None
        notes.append(
            HealthNote(
                severity="warn",
                category="synthesis",
                message="indexer database missing at journal/indexer/journal.sqlite; search-backed consumers may be stale.",
                detected_at=generated_at,
                detail_pointer=None,
            )
        )
    else:
        indexer_last_rebuild_at = indexer_path.stat().st_mtime_ns // 1_000_000
        if generated_at - indexer_last_rebuild_at > INDEXER_STALE_WARN_DAYS * _DAY_MS:
            stale_days = (generated_at - indexer_last_rebuild_at) // _DAY_MS
            notes.append(
                HealthNote(
                    severity="warn",
                    category="synthesis",
                    message=(
                        f"indexer database last rebuilt {stale_days}d ago; "
                        "search-backed consumers may be stale."
                    ),
                    detected_at=generated_at,
                    detail_pointer=None,
                )
            )

    return (
        SynthesisHealth(
            activities_count=aggregate.activities_count,
            activities_with_participation=aggregate.activities_with_participation,
            activities_with_story=aggregate.activities_with_story,
            activities_user_edited=aggregate.activities_user_edited,
            activities_anticipated_unfilled=aggregate.activities_anticipated_unfilled,
            talent_run_failures_24h=talent_run_failures_24h,
            indexer_last_rebuild_at=indexer_last_rebuild_at,
        ),
        notes,
    )


def _build_consumer_signal_health() -> ConsumerSignalHealth:
    return ConsumerSignalHealth(
        ledger_open_items_total=len(ledger.list(state="open")),
        ledger_stale_items_count=len(
            ledger.list(state="open", age_days_gte=LEDGER_STALE_DAYS)
        ),
        profile_entities_total=len(load_all_journal_entities()),
    )


def _build_segment_backlog_health() -> SegmentBacklogHealth:
    backlog = read_segment_backlog()
    days_with_backlog = sum(
        1 for completion in backlog.per_day.values() if completion.not_thought > 0
    )
    return SegmentBacklogHealth(
        not_thought=backlog.not_thought,
        days_with_backlog=days_with_backlog,
        errors=backlog.errors,
    )


def _build_report(day_from: str, day_to: str) -> HealthReport:
    generated_at = int(_resolve_now().timestamp() * 1000)
    facets = tuple(sorted(get_facets().keys()))
    aggregate = _scan_records(day_from, day_to)
    capture_health, capture_notes = _build_capture_health(
        aggregate,
        (day_from, day_to),
        facets,
        generated_at,
    )
    synthesis_health, synthesis_notes = _build_synthesis_health(
        aggregate,
        generated_at,
    )
    notes = capture_notes + synthesis_notes
    severity_rank = {"critical": 0, "warn": 1, "info": 2}
    notes.sort(
        key=lambda note: (
            severity_rank.get(note.severity, 99),
            note.category,
            note.message,
        )
    )
    return HealthReport(
        generated_at=generated_at,
        range=(day_from, day_to),
        facets=facets,
        capture_health=capture_health,
        synthesis_health=synthesis_health,
        consumer_signal=_build_consumer_signal_health(),
        segment_backlog=_build_segment_backlog_health(),
        notes=tuple(notes),
    )


def summary(day: str | None = None) -> HealthReport:
    resolved_day = (
        _resolve_now().strftime("%Y%m%d") if day is None else _resolve_day(day)
    )
    return _build_report(resolved_day, resolved_day)


def full(day: str | None = None) -> HealthReport:
    resolved_day = (
        _resolve_now().strftime("%Y%m%d") if day is None else _resolve_day(day)
    )
    return _build_report(resolved_day, resolved_day)


def for_range(
    day_from: str | None = None,
    day_to: str | None = None,
) -> HealthReport:
    if day_from is None and day_to is None:
        today = _resolve_now()
        day_to = today.strftime("%Y%m%d")
        day_from = (today - timedelta(days=6)).strftime("%Y%m%d")
    elif day_from is None or day_to is None:
        raise ValueError("both endpoints or neither")
    else:
        _resolve_day(day_from)
        _resolve_day(day_to)

    if day_from > day_to:
        raise ValueError("day_from must be <= day_to")

    return _build_report(day_from, day_to)
