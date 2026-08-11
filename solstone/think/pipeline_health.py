# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Summarize think pipeline health from daily JSONL logs."""

from __future__ import annotations

import json
import logging
import os
from dataclasses import dataclass
from datetime import datetime, timedelta
from pathlib import Path

from solstone.think.catchup_state import (
    read_backoff_summary,
    read_segment_repair_attempted,
    read_segment_repair_summary,
)
from solstone.think.cluster import cluster_segments
from solstone.think.data_state import DataState
from solstone.think.deterministic_failure_caps import (
    DETERMINISTIC_FAILURE_REASON_CODES,
    failure_capped,
)
from solstone.think.utils import (
    DEFAULT_STREAM,
    day_dirs,
    day_is_complete,
    day_path,
    now_ms,
    resolve_segment_dir,
    updated_days,
)

logger = logging.getLogger(__name__)

# Test indirection: tests monkeypatch this for time-sensitive branches.
_now = datetime.now

_MODES = ("segment", "daily", "activity", "weekly", "flush", "cadence")
_FAILED_LIST_CAP = 20
_ACTIVITY_WORK_EVENTS = frozenset(
    {
        "run.start",
        "run.complete",
        "talent.dispatch",
        "talent.complete",
        "talent.fail",
        "talent.skip",
    }
)
SEGMENT_FLOOR_TALENTS: tuple[str, ...] = ("documents",)
SEGMENT_NONGATING_TALENTS: tuple[str, ...] = ("entities:detection",)
# A legacy segment talent is non-blocking only once its replacement has completed.
SEGMENT_SUPERSEDED_TALENTS: dict[str, str] = {"entities": "entities:detection"}
SEGMENT_NO_PROCESSING_MODALITIES = frozenset({"markdown", "browser"})
# Floor talents are capped after repeated failures spanning at least two hours.
CAP = 5
MIN_SPAN_MS = 7_200_000
SENSED_TERMINAL_STATES = frozenset(
    {
        DataState.ANALYZED.value,
        DataState.PURGED.value,
        DataState.EMPTY.value,
        DataState.FAILED_FINAL.value,
    }
)
STUCK_FAIL_THRESHOLD = 3
BACKLOG_DEFAULT_WINDOW = 30
NO_SENSE_COMPLETE_AGED_MS = 3 * 24 * 60 * 60 * 1000

TERMINAL_COMPLETE = "complete"
TERMINAL_FAIL = "fail"

WHY_FAILED = "failed"
WHY_CORRUPT_RAW = "corrupt_raw"
WHY_NEVER_ATTEMPTED = "never_attempted"
WHY_NO_SENSE_COMPLETE_AGED = "no_sense_complete_aged"
WHY_SENSED_NOT_THOUGHT = "sensed_not_thought"

REASON_CORRUPT_RAW = "corrupt_raw"
REASON_FAILING_STEP = "failing_step"
REASON_CATCHUP_BACKOFF = "catchup_backoff"
REASON_SEGMENT_REPAIR_DEGRADED = "segment_repair_degraded"
REASON_SEGMENT_REPAIR_PROGRESSING = "segment_repair_progressing"
REASON_SEGMENT_REPAIR_STUCK = "segment_repair_stuck"
REASON_SEGMENT_REPAIR_UNKNOWN = "segment_repair_unknown"

BACKLOG_STATE_COMPLETE = "complete"
BACKLOG_STATE_PENDING = "pending"
BACKLOG_STATE_STUCK = "stuck"
BACKLOG_STATE_UNKNOWN = "unknown"


@dataclass(frozen=True)
class SegmentProgress:
    """Per-segment fold of think-pipeline health for one day."""

    sensed: bool
    density: str | None
    change_class: str | None
    dispatched: frozenset[str]
    completed: frozenset[str]
    unconfigured: frozenset[str]
    capped: frozenset[str]


@dataclass(frozen=True)
class SegmentCompletion:
    """Per-segment completion verdict for clustered segments."""

    blockers: list[dict[str, str]]
    not_sensed: int
    not_thought: int
    total: int
    capped: int
    exhausted: tuple[str, ...]


@dataclass(frozen=True)
class SegmentBacklog:
    """Cross-day segment completion backlog over updated days."""

    days: tuple[str, ...]
    not_thought: int
    not_sensed: int
    total: int
    per_day: dict[str, SegmentCompletion]
    errors: tuple[str, ...]


@dataclass(frozen=True)
class TerminalUnit:
    """Identity for a terminal talent event within one day."""

    mode: str
    name: str
    facet: str | None
    stream: str | None
    segment: str | None
    activity: str | None


@dataclass(frozen=True)
class TerminalState:
    """Latest terminal state and trailing-failure diagnostic metadata."""

    latest_event: str
    latest_ts: int
    last_real_complete_ts: int | None
    trailing_fail_count: int
    deterministic_fail_count: int
    last_fail_ts: int | None
    use_id: str | None
    state: str | None
    reason_code: str | None
    provider: str | None
    model: str | None
    oldest_trailing_fail_ts: int | None


@dataclass(frozen=True)
class CompletionsSince:
    """Completed segment/activity units newer than a timestamp, for cadence."""

    segments: tuple[dict, ...]
    activities: tuple[dict, ...]


@dataclass(frozen=True)
class DeterministicFailure:
    """A daily unit whose latest terminal is a deterministic crash."""

    count: int
    reason_code: str


@dataclass(frozen=True)
class BacklogUnit:
    """Outstanding unit with why-axis classification.

    ``failed``, ``sensed_not_thought``, and ``stuck`` are derived for all modes
    that have observed health records. ``never_attempted`` is derived only for
    segment floor talents in ``SEGMENT_FLOOR_TALENTS``. Its absence on
    non-segment modes does not prove an attempt occurred; this why-axis is not
    exhaustive for non-segment never-attempted work.
    """

    mode: str
    name: str
    facet: str | None
    stream: str | None
    segment: str | None
    why: str
    reason_code: str | None
    provider: str | None
    model: str | None
    trailing_fail_count: int
    last_fail_ts: int | None
    stuck: bool


@dataclass(frozen=True)
class BacklogError:
    """Per-day backlog derivation error."""

    day: str
    stage: str
    message: str


@dataclass(frozen=True)
class BacklogDay:
    """Backlog state for one day in a bounded window."""

    day: str
    state: str
    segments: int
    units: int
    not_sensed: int
    why: tuple[BacklogUnit, ...]
    reason: str | None
    reason_code: str | None
    provider: str | None
    model: str | None
    error: BacklogError | None
    backoff_stuck: bool = False
    backoff_attempts: int = 0
    backoff_consecutive_non_completion: int = 0
    backoff_last_outcome: str | None = None
    backoff_next_retry_at: float | None = None
    segment_repair_status: str | None = None
    segment_repair_attempts: int = 0
    segment_repair_consecutive_non_completion: int = 0
    segment_repair_last_outcome: str | None = None
    segment_repair_next_retry_at: float | None = None
    segment_repair_reason_code: str | None = None
    segment_repair_timeout_seconds: int | None = None
    segment_repair_bounded: bool | None = None
    segment_repair_cleared: int | None = None
    segment_repair_remaining: int | None = None
    capped_daily_unit_count: int = 0
    capped_daily_unit: dict[str, object] | None = None


@dataclass(frozen=True)
class BacklogView:
    """Bounded cross-day backlog derivation."""

    window: int
    days: tuple[BacklogDay, ...]
    pending_days: int
    stuck_days: int
    oldest_pending_day: str | None
    errors: tuple[BacklogError, ...]
    degraded: bool = False


def summarize_pipeline_day(day: str) -> dict:
    """Return a day-level summary of think pipeline health."""
    summary = {
        "day": day,
        "generated_at": now_ms(),
        "status": "healthy",
        "anomalies": [],
        "runs": {mode: {"count": 0, "duration_ms_total": 0} for mode in _MODES},
        "talents": {
            "dispatched": 0,
            "completed": 0,
            "failed": 0,
            "outstanding_failed": 0,
            "skipped": 0,
            "capped": 0,
            "failed_list": [],
            "failed_list_truncated": False,
        },
        "activities": {
            "detected": 0,
            "persisted": 0,
            "talents_fired": False,
        },
        "exhausted_segments": {"count": 0, "segments": []},
    }

    def _apply_segment_census(segments: list[dict]) -> SegmentCompletion:
        completion = classify_segment_completion(
            segments,
            read_segment_progress(day),
        )
        # Exhausted segments are sensed-terminal and leave not_sensed blockers;
        # this census is the remaining signal that their raw media is held.
        summary["exhausted_segments"] = {
            "count": len(completion.exhausted),
            "segments": list(completion.exhausted),
        }
        return completion

    try:
        health_dir = day_path(day, create=False) / "health"
        if not health_dir.is_dir():
            today = _now().strftime("%Y%m%d")
            segments = cluster_segments(day)
            if segments:
                _apply_segment_census(segments)
            if day < today and segments:
                summary["status"] = "unknown"
                summary["anomalies"].append(
                    {"kind": "segments_not_thought", "error": "no_health_dir"}
                )
            return summary

        for path in sorted(health_dir.glob("*.jsonl")):
            mode = None
            for candidate in _MODES:
                if path.name.endswith(f"_{candidate}.jsonl"):
                    mode = candidate
                    break
            if mode is None:
                logger.debug("pipeline_health: skipping unrecognized file %s", path)
                continue

            summary["runs"][mode]["count"] += 1

            with path.open(encoding="utf-8") as handle:
                for raw_line in handle:
                    line = raw_line.strip()
                    if not line:
                        continue
                    try:
                        rec = json.loads(line)
                    except json.JSONDecodeError:
                        logger.debug("malformed jsonl line in %s", path)
                        continue

                    if not isinstance(rec, dict) or "event" not in rec:
                        logger.debug(
                            "pipeline_health: skipping invalid record in %s", path
                        )
                        continue

                    rec_day = rec.get("day")
                    if isinstance(rec_day, str) and rec_day != day:
                        continue

                    event = rec["event"]
                    if event in _ACTIVITY_WORK_EVENTS and rec.get("mode") == "activity":
                        summary["activities"]["talents_fired"] = True

                    if event == "talent.dispatch":
                        summary["talents"]["dispatched"] += 1
                    elif event == "talent.complete":
                        summary["talents"]["completed"] += 1
                    elif event == "talent.fail":
                        summary["talents"]["failed"] += 1
                    elif event == "talent.skip":
                        if rec.get("reason") == "capped":
                            summary["talents"]["capped"] += 1
                        else:
                            summary["talents"]["skipped"] += 1
                    elif event == "activity.detected":
                        summary["activities"]["detected"] += 1
                    elif event == "activity.persisted":
                        summary["activities"]["persisted"] += 1
                    elif event == "run.complete":
                        try:
                            duration_ms = int(rec.get("duration_ms", 0))
                        except (TypeError, ValueError):
                            duration_ms = 0
                        summary["runs"][mode]["duration_ms_total"] += duration_ms
    except ValueError:
        return summary
    except Exception:
        logger.warning(
            "pipeline_health: unexpected error summarizing %s",
            day,
            exc_info=True,
        )
        summary["status"] = "unknown"
        summary["anomalies"].append(
            {"kind": "segments_not_thought", "error": "scan_failed"}
        )
        return summary

    outstanding = [
        {
            "mode": unit.mode,
            "name": unit.name,
            "use_id": state.use_id,
            "state": state.state,
        }
        for unit, state in read_terminal_states(day, scope_to_day=True).items()
        if state.latest_event == TERMINAL_FAIL
    ]
    outstanding.sort(
        key=lambda failure: (
            failure.get("name") or "",
            failure.get("mode") or "",
            failure.get("use_id") or "",
        )
    )
    summary["talents"]["outstanding_failed"] = len(outstanding)
    summary["talents"]["failed_list"] = outstanding[:_FAILED_LIST_CAP]
    summary["talents"]["failed_list_truncated"] = len(outstanding) > _FAILED_LIST_CAP

    for failure in summary["talents"]["failed_list"]:
        summary["anomalies"].append({"kind": "talent_failure", **failure})

    if (
        summary["activities"]["detected"] > 0
        and not summary["activities"]["talents_fired"]
    ):
        summary["anomalies"].append({"kind": "activity_agents_missing"})

    current = _now()
    today = current.strftime("%Y%m%d")
    if day == today:
        if current.hour >= 23 and summary["runs"]["daily"]["count"] == 0:
            summary["anomalies"].append({"kind": "daily_agents_missing"})
    elif day < today and summary["runs"]["daily"]["count"] == 0:
        summary["anomalies"].append({"kind": "daily_agents_missing"})

    # Days with a health directory surface segment gaps here; degenerate
    # zero-health days are still counted by stats and withheld by the daily gate.
    try:
        completion = _apply_segment_census(cluster_segments(day))
        if completion.not_thought > 0:
            # The kind now means segments sensed-but-not-thought, not zero runs.
            summary["anomalies"].append(
                {
                    "kind": "segments_not_thought",
                    "not_thought": completion.not_thought,
                    "not_sensed": completion.not_sensed,
                    "total": completion.total,
                }
            )
    except Exception:
        logger.warning(
            "pipeline_health: segment completion fold failed for %s",
            day,
            exc_info=True,
        )
        summary["anomalies"].append(
            {"kind": "segments_not_thought", "error": "fold_failed"}
        )

    has_stale = any(
        anomaly["kind"]
        in {"activity_agents_missing", "daily_agents_missing", "segments_not_thought"}
        for anomaly in summary["anomalies"]
    )
    has_failure = any(
        anomaly["kind"] == "talent_failure" for anomaly in summary["anomalies"]
    )
    if has_stale:
        summary["status"] = "stale"
    elif has_failure:
        summary["status"] = "warning"

    return summary


def _str_or_none(value: object) -> str | None:
    return value if isinstance(value, str) else None


def read_terminal_states(
    day: str, *, scope_to_day: bool = False
) -> dict[TerminalUnit, TerminalState]:
    """Return latest terminal talent state per unit for one day."""
    records: dict[
        TerminalUnit,
        list[
            tuple[
                int,
                int,
                str,
                str | None,
                str | None,
                str | None,
                str | None,
                str | None,
                bool,
            ]
        ],
    ] = {}
    sequence = 0

    try:
        health_dir = day_path(day, create=False) / "health"
        if not health_dir.is_dir():
            return {}

        for path in sorted(health_dir.glob("*.jsonl")):
            with path.open(encoding="utf-8") as handle:
                for raw_line in handle:
                    line = raw_line.strip()
                    if not line:
                        continue
                    try:
                        rec = json.loads(line)
                    except json.JSONDecodeError:
                        logger.debug("malformed jsonl line in %s", path)
                        continue

                    if not isinstance(rec, dict):
                        logger.debug(
                            "pipeline_health: skipping invalid record in %s", path
                        )
                        continue

                    if scope_to_day:
                        rec_day = rec.get("day")
                        if isinstance(rec_day, str) and rec_day != day:
                            continue

                    event = rec.get("event")
                    if event not in {"talent.complete", "talent.fail"}:
                        continue

                    mode = rec.get("mode")
                    name = rec.get("name")
                    if not isinstance(mode, str) or not isinstance(name, str):
                        logger.debug(
                            "pipeline_health: skipping terminal record missing "
                            "mode/name in %s",
                            path,
                        )
                        continue

                    try:
                        ts = int(rec["ts"])
                    except (KeyError, TypeError, ValueError):
                        logger.debug(
                            "pipeline_health: skipping terminal record with invalid "
                            "ts in %s",
                            path,
                        )
                        continue

                    sequence += 1
                    unit = TerminalUnit(
                        mode=mode,
                        name=name,
                        facet=_str_or_none(rec.get("facet")),
                        stream=_str_or_none(rec.get("stream")),
                        segment=_str_or_none(rec.get("segment")),
                        activity=_str_or_none(rec.get("activity")),
                    )
                    latest_event = (
                        TERMINAL_COMPLETE
                        if event == "talent.complete"
                        else TERMINAL_FAIL
                    )
                    records.setdefault(unit, []).append(
                        (
                            ts,
                            sequence,
                            latest_event,
                            _str_or_none(rec.get("use_id")),
                            _str_or_none(rec.get("state")),
                            _str_or_none(rec.get("reason_code")),
                            _str_or_none(rec.get("provider")),
                            _str_or_none(rec.get("model")),
                            rec.get("cache_hit") is True,
                        )
                    )
    except Exception:
        logger.warning(
            "pipeline_health: unexpected error reading terminal states for %s",
            day,
            exc_info=True,
        )
        return {}

    states: dict[TerminalUnit, TerminalState] = {}
    for unit, unit_records in records.items():
        ordered = sorted(unit_records, key=lambda item: (item[0], item[1]))
        (
            latest_ts,
            _seq,
            latest_event,
            _use_id,
            _state,
            _reason_code,
            _provider,
            _model,
            _cache_hit,
        ) = ordered[-1]
        real_complete_ts = [
            ts
            for (
                ts,
                _seq,
                event,
                _use_id,
                _state,
                _reason_code,
                _provider,
                _model,
                cache_hit,
            ) in ordered
            if event == TERMINAL_COMPLETE and not cache_hit
        ]
        last_real_complete_ts = max(real_complete_ts) if real_complete_ts else None
        trailing_fail_count = 0
        oldest_trailing_fail_ts = None
        for (
            ts,
            _seq,
            event,
            _use_id,
            _state,
            _reason_code,
            _provider,
            _model,
            _cache_hit,
        ) in reversed(ordered):
            if event != TERMINAL_FAIL:
                break
            trailing_fail_count += 1
            oldest_trailing_fail_ts = ts
        deterministic_fail_count = 0
        for (
            _ts,
            _seq,
            event,
            _use_id,
            _state,
            reason_code,
            _provider,
            _model,
            _cache_hit,
        ) in reversed(ordered):
            if event == TERMINAL_COMPLETE:
                break
            if (
                event == TERMINAL_FAIL
                and reason_code in DETERMINISTIC_FAILURE_REASON_CODES
            ):
                deterministic_fail_count += 1
        last_fail = next(
            (record for record in reversed(ordered) if record[2] == TERMINAL_FAIL),
            None,
        )
        states[unit] = TerminalState(
            latest_event=latest_event,
            latest_ts=latest_ts,
            last_real_complete_ts=last_real_complete_ts,
            trailing_fail_count=trailing_fail_count,
            deterministic_fail_count=deterministic_fail_count,
            last_fail_ts=last_fail[0] if last_fail else None,
            use_id=last_fail[3] if last_fail else None,
            state=last_fail[4] if last_fail else None,
            reason_code=last_fail[5] if last_fail else None,
            provider=last_fail[6] if last_fail else None,
            model=last_fail[7] if last_fail else None,
            oldest_trailing_fail_ts=oldest_trailing_fail_ts,
        )
    return states


def is_floor_talent_capped(
    day: str, stream: str | None, segment: str, name: str
) -> bool:
    """Return True when a segment floor talent has hit the failure cap."""
    state = read_terminal_states(day).get(
        TerminalUnit(
            mode="segment",
            name=name,
            facet=None,
            stream=stream,
            segment=segment,
            activity=None,
        )
    )
    if state is None:
        return False
    if state.trailing_fail_count < CAP:
        return False
    if state.oldest_trailing_fail_ts is None or state.last_fail_ts is None:
        return False
    return state.last_fail_ts - state.oldest_trailing_fail_ts >= MIN_SPAN_MS


def read_completed_units(day: str) -> set[tuple[str, str, str | None]]:
    """Return unit keys whose latest terminal health event is complete.

    Delegates to ``read_terminal_states`` so there is one latest-terminal
    completion definition. The public return shape is retained for daily
    idempotency callers.

    This function does not create, modify, or delete journal state.
    """
    return {
        (unit.mode, unit.name, unit.facet)
        for unit, state in read_terminal_states(day).items()
        if unit.segment is None
        and unit.activity is None
        and state.latest_event == TERMINAL_COMPLETE
    }


def read_completed_since(day: str, since_ms: int) -> CompletionsSince:
    """Return unique completed segment/activity units newer than since_ms.

    Scans ``day`` and the prior day because post-midnight completions can
    reference the previous day's health dir. Projects ``read_terminal_states``
    from per-talent identities to unique segment/activity units, each tagged
    with the newest completion ts.

    This function does not create, modify, or delete journal state.
    """
    prev = (datetime.strptime(day, "%Y%m%d") - timedelta(days=1)).strftime("%Y%m%d")
    seg_max: dict[tuple[str | None, str], int] = {}
    act_max: dict[tuple[str | None, str], int] = {}

    for scan_day in (day, prev):
        for unit, state in read_terminal_states(scan_day).items():
            real_complete_ts = state.last_real_complete_ts
            if (
                state.latest_event != TERMINAL_COMPLETE
                or real_complete_ts is None
                or real_complete_ts <= since_ms
            ):
                continue

            if unit.segment:
                seg_key = (unit.stream, unit.segment)
                seg_max[seg_key] = max(seg_max.get(seg_key, 0), real_complete_ts)
            elif unit.activity:
                act_key = (unit.facet, unit.activity)
                act_max[act_key] = max(act_max.get(act_key, 0), real_complete_ts)

    segments = tuple(
        sorted(
            (
                {"stream": stream, "segment": segment, "ts": ts}
                for (stream, segment), ts in seg_max.items()
            ),
            key=lambda item: (
                item["ts"],
                item["stream"] or "",
                item["segment"],
            ),
        )
    )
    activities = tuple(
        sorted(
            (
                {"facet": facet, "activity": activity, "ts": ts}
                for (facet, activity), ts in act_max.items()
            ),
            key=lambda item: (
                item["ts"],
                item["facet"] or "",
                item["activity"],
            ),
        )
    )
    return CompletionsSince(segments=segments, activities=activities)


def read_daily_deterministic_failures(
    day: str,
) -> dict[tuple[str, str | None], DeterministicFailure]:
    """Return daily units whose latest terminal is a deterministic failure.

    Keyed by ``(name, facet)``. A unit qualifies only when its latest
    terminal health event is a ``talent.fail`` whose ``reason_code`` is in
    ``DETERMINISTIC_FAILURE_REASON_CODES``; the count is the number of such
    deterministic failures since the unit's last completion that day. A
    completion or a transient latest failure excludes the unit.

    This function does not create, modify, or delete journal state.
    """
    result: dict[tuple[str, str | None], DeterministicFailure] = {}
    for unit, state in read_terminal_states(day).items():
        if (
            unit.mode != "daily"
            or unit.segment is not None
            or unit.activity is not None
        ):
            continue
        if state.latest_event != TERMINAL_FAIL:
            continue
        if state.reason_code not in DETERMINISTIC_FAILURE_REASON_CODES:
            continue
        result[(unit.name, unit.facet)] = DeterministicFailure(
            count=state.deterministic_fail_count,
            reason_code=state.reason_code,
        )
    return result


def read_segment_progress(day: str) -> dict[tuple[str | None, str], SegmentProgress]:
    """Return per-segment progress from the day's segment health events.

    Folds the day's health JSONL files read-only. Progress is keyed by
    ``(stream, segment)``. Untagged historical records use a legacy ``None``
    stream bucket. Segment-scoped records are records with ``mode == "segment"``
    and a truthy string ``segment`` field. Terminal fold states are
    ``talent.complete``, ``talent.fail``, and ``talent.skip`` with
    ``reason="capped"``; the latest terminal per ``((stream, segment), name)``
    wins by ``ts``. Other ``talent.skip`` records are non-terminal, except
    ``reason="no_config"`` is tracked for floor verdicts.

    This function does not create, modify, or delete journal state.
    """
    latest_sense: dict[tuple[str | None, str], tuple[int, str | None]] = {}
    latest_change: dict[tuple[str | None, str], tuple[int, str | None]] = {}
    dispatched: dict[tuple[str | None, str], set[str]] = {}
    terminals: dict[tuple[str | None, str], dict[str, tuple[int, str]]] = {}
    unconfigured: dict[tuple[str | None, str], set[str]] = {}

    try:
        health_dir = day_path(day, create=False) / "health"
        if not health_dir.is_dir():
            return {}

        for path in sorted(health_dir.glob("*.jsonl")):
            with path.open(encoding="utf-8") as handle:
                for raw_line in handle:
                    line = raw_line.strip()
                    if not line:
                        continue
                    try:
                        rec = json.loads(line)
                    except json.JSONDecodeError:
                        logger.debug("malformed jsonl line in %s", path)
                        continue

                    if not isinstance(rec, dict):
                        logger.debug(
                            "pipeline_health: skipping invalid record in %s", path
                        )
                        continue

                    segment = rec.get("segment")
                    if rec.get("mode") != "segment" or not isinstance(segment, str):
                        continue
                    if not segment:
                        continue
                    stream = rec.get("stream")
                    key = (stream if isinstance(stream, str) else None, segment)

                    event = rec.get("event")
                    if event == "sense.complete":
                        try:
                            ts = int(rec["ts"])
                        except (KeyError, TypeError, ValueError):
                            logger.debug(
                                "pipeline_health: skipping sense.complete with "
                                "invalid ts in %s",
                                path,
                            )
                            continue
                        density = rec.get("density")
                        if not isinstance(density, str):
                            density = None
                        if key not in latest_sense or ts >= latest_sense[key][0]:
                            latest_sense[key] = (ts, density)
                    elif event == "sense.change_detect":
                        try:
                            ts = int(rec["ts"])
                        except (KeyError, TypeError, ValueError):
                            logger.debug(
                                "pipeline_health: skipping sense.change_detect with invalid ts in %s",
                                path,
                            )
                            continue
                        change_class = rec.get("change_class")
                        if not isinstance(change_class, str):
                            change_class = None
                        if key not in latest_change or ts >= latest_change[key][0]:
                            latest_change[key] = (ts, change_class)
                    elif event == "talent.dispatch":
                        name = rec.get("name")
                        if isinstance(name, str):
                            dispatched.setdefault(key, set()).add(name)
                    elif event in {"talent.complete", "talent.fail"}:
                        name = rec.get("name")
                        if not isinstance(name, str):
                            logger.debug(
                                "pipeline_health: skipping segment terminal missing "
                                "name in %s",
                                path,
                            )
                            continue
                        try:
                            ts = int(rec["ts"])
                        except (KeyError, TypeError, ValueError):
                            logger.debug(
                                "pipeline_health: skipping segment terminal with "
                                "invalid ts in %s",
                                path,
                            )
                            continue

                        segment_terminals = terminals.setdefault(key, {})
                        if (
                            name not in segment_terminals
                            or ts >= segment_terminals[name][0]
                        ):
                            segment_terminals[name] = (
                                ts,
                                "complete" if event == "talent.complete" else "fail",
                            )
                    elif event == "talent.skip" and rec.get("reason") == "capped":
                        name = rec.get("name")
                        if not isinstance(name, str):
                            logger.debug(
                                "pipeline_health: skipping capped segment terminal "
                                "missing name in %s",
                                path,
                            )
                            continue
                        try:
                            ts = int(rec["ts"])
                        except (KeyError, TypeError, ValueError):
                            logger.debug(
                                "pipeline_health: skipping capped segment terminal "
                                "with invalid ts in %s",
                                path,
                            )
                            continue
                        segment_terminals = terminals.setdefault(key, {})
                        if (
                            name not in segment_terminals
                            or ts >= segment_terminals[name][0]
                        ):
                            segment_terminals[name] = (ts, "capped")
                    elif event == "talent.skip" and rec.get("reason") == "no_config":
                        name = rec.get("name")
                        if isinstance(name, str):
                            unconfigured.setdefault(key, set()).add(name)
    except Exception:
        logger.warning(
            "pipeline_health: unexpected error reading segment progress for %s",
            day,
            exc_info=True,
        )
        return {}

    segments = (
        set(latest_sense)
        | set(latest_change)
        | set(dispatched)
        | set(terminals)
        | set(unconfigured)
    )
    progress: dict[tuple[str | None, str], SegmentProgress] = {}
    for key in sorted(segments, key=lambda k: (k[1], k[0] is not None, k[0] or "")):
        segment_terminals = terminals.get(key, {})
        progress[key] = SegmentProgress(
            sensed=key in latest_sense,
            density=latest_sense.get(key, (0, None))[1],
            change_class=latest_change.get(key, (0, None))[1],
            dispatched=frozenset(dispatched.get(key, set())),
            completed=frozenset(
                name
                for name, (_ts, state) in segment_terminals.items()
                if state == "complete"
            ),
            unconfigured=frozenset(unconfigured.get(key, set())),
            capped=frozenset(
                name
                for name, (_ts, state) in segment_terminals.items()
                if state == "capped"
            ),
        )
    return progress


def segment_fully_sensed(data_state: dict[str, str]) -> bool:
    """True when every non-absent modality has finished sensing.

    ``data_state`` is the per-segment dict from ``cluster_segments``; it already
    omits absent modalities, so an absent modality cannot peg a segment. Empty
    outputs and exhausted failures are terminal; retryable failed outputs still
    block sensing completion.
    """
    return all(state in SENSED_TERMINAL_STATES for state in data_state.values())


def segment_requires_processing(segment: dict) -> bool:
    """True when a clustered segment has media/data that should enter think.

    An empty data_state returns True so an anomalous segment surfaces as a
    visible blocker instead of silently counting as complete.
    """
    data_state = segment.get("data_state") or {}
    if not data_state:
        return True
    return any(
        modality not in SEGMENT_NO_PROCESSING_MODALITIES for modality in data_state
    )


def segment_fully_thought(progress: SegmentProgress | None) -> tuple[bool, str | None]:
    """Per-segment fully-thought verdict. Returns (ok, blocking_reason)."""
    if progress is None or not progress.sensed:
        return False, "no_sense_complete"
    if progress.density == "idle":
        return True, None
    if progress.change_class == "redundant":
        return True, None
    for name in SEGMENT_FLOOR_TALENTS:
        if (
            name not in progress.completed
            and name not in progress.unconfigured
            and name not in progress.capped
        ):
            return False, f"floor:{name}"
    for name in sorted(progress.dispatched):
        if name in SEGMENT_NONGATING_TALENTS:
            continue
        replacement = SEGMENT_SUPERSEDED_TALENTS.get(name)
        if replacement is not None and replacement in progress.completed:
            continue
        if name not in progress.completed and name not in progress.capped:
            return False, f"dispatched:{name}"
    return True, None


def lookup_segment_progress(
    progress: dict[tuple[str | None, str], SegmentProgress],
    stream: str,
    segment: str,
) -> SegmentProgress | None:
    """Resolve a clustered segment's progress.

    Exact ``(stream, segment)`` first; only on an exact miss fall back to the
    legacy untagged bucket ``(None, segment)``. Never crosses to a different
    stream's progress, and never falls back when an exact entry exists.
    """
    hit = progress.get((stream, segment))
    if hit is not None:
        return hit
    return progress.get((None, segment))


def classify_segment_completion(
    segments: list[dict],
    progress: dict[tuple[str | None, str], SegmentProgress],
) -> SegmentCompletion:
    """Purely classify clustered segment completion without journal reads/writes."""
    blockers: list[dict[str, str]] = []
    not_sensed = 0
    not_thought = 0
    capped = 0
    exhausted: set[str] = set()

    for seg in segments:
        if not segment_requires_processing(seg):
            continue
        key = seg["key"]
        data_state = seg["data_state"]
        segment_progress = lookup_segment_progress(progress, seg["stream"], key)
        if segment_progress is not None and segment_progress.capped:
            capped += 1
        if any(state == DataState.FAILED_FINAL.value for state in data_state.values()):
            exhausted.add(key)
        if not segment_fully_sensed(data_state):
            detail = ",".join(
                f"{modality}={state}"
                for modality, state in sorted(data_state.items())
                if state not in SENSED_TERMINAL_STATES
            )
            blockers.append(
                {
                    "segment": key,
                    "dimension": "not_sensed",
                    "detail": detail,
                }
            )
            not_sensed += 1
            continue

        ok, reason = segment_fully_thought(segment_progress)
        if not ok:
            blockers.append(
                {
                    "segment": key,
                    "dimension": "not_thought",
                    "detail": reason or "",
                }
            )
            not_thought += 1

    return SegmentCompletion(
        blockers=blockers,
        not_sensed=not_sensed,
        not_thought=not_thought,
        total=len(segments),
        capped=capped,
        exhausted=tuple(sorted(exhausted)),
    )


def blocked_segment_keys(
    segments: list[dict],
    progress: dict[tuple[str | None, str], SegmentProgress],
) -> set[tuple[str | None, str]]:
    """Return the clustered segment identities still blocked by completion gates."""
    blocked: set[tuple[str | None, str]] = set()
    for seg in segments:
        if not segment_requires_processing(seg):
            continue
        key = seg["key"]
        if not segment_fully_sensed(seg["data_state"]):
            blocked.add((seg["stream"], key))
            continue

        segment_progress = lookup_segment_progress(progress, seg["stream"], key)
        ok, _reason = segment_fully_thought(segment_progress)
        if not ok:
            blocked.add((seg["stream"], key))
    return blocked


def _stream_updated_ms(day: str) -> int | None:
    path = day_path(day, create=False) / "health" / "stream.updated"
    if not path.is_file():
        return None
    return int(os.path.getmtime(path) * 1000)


def _segment_dir_for_backlog(
    day: str,
    stream: str | None,
    segment: str,
) -> Path:
    resolved_stream = DEFAULT_STREAM if stream is None else stream
    return resolve_segment_dir(day, stream=resolved_stream, segment=segment)


def _read_failed_marker(
    day: str,
    stream: str | None,
    segment: str,
    modality: str,
) -> tuple[str, int] | None:
    marker = (
        _segment_dir_for_backlog(day, stream, segment) / f".analyze_failed_{modality}"
    )
    try:
        data = json.loads(marker.read_text(encoding="utf-8"))
        if not isinstance(data, dict):
            logger.debug("pipeline_health: failed marker is not an object: %s", marker)
            return None

        reason = data.get("reason")
        failed_at = data.get("failed_at")
        if not isinstance(reason, str) or not isinstance(failed_at, str):
            logger.debug(
                "pipeline_health: failed marker lacks reason/failed_at strings: %s",
                marker,
            )
            return None

        failed_at_ms = int(
            datetime.fromisoformat(failed_at.replace("Z", "+00:00")).timestamp() * 1000
        )
    except (OSError, json.JSONDecodeError, ValueError, TypeError):
        logger.debug(
            "pipeline_health: failed marker unreadable or unparseable: %s",
            marker,
            exc_info=True,
        )
        return None

    return reason, failed_at_ms


def _terminal_unit_for_segment(
    name: str,
    stream: str | None,
    segment: str,
) -> TerminalUnit:
    return TerminalUnit(
        mode="segment",
        name=name,
        facet=None,
        stream=stream,
        segment=segment,
        activity=None,
    )


def _is_stuck(state: TerminalState | None, stream_updated_ms: int | None) -> bool:
    if state is None or state.latest_event != TERMINAL_FAIL:
        return False
    if state.trailing_fail_count < STUCK_FAIL_THRESHOLD:
        return False
    if state.last_fail_ts is None or stream_updated_ms is None:
        return False
    return stream_updated_ms <= state.last_fail_ts


def read_day_stuck(day: str) -> bool:
    """Return True when any terminal unit for a day is stuck."""
    stream_ms = _stream_updated_ms(day)
    states = read_terminal_states(day)
    return any(_is_stuck(state, stream_ms) for state in states.values())


def _failed_backlog_unit(
    unit: TerminalUnit,
    state: TerminalState,
    stream_updated_ms: int | None,
) -> BacklogUnit:
    return BacklogUnit(
        mode=unit.mode,
        name=unit.name,
        facet=unit.facet,
        stream=unit.stream,
        segment=unit.segment,
        why=WHY_FAILED,
        reason_code=state.reason_code,
        provider=state.provider,
        model=state.model,
        trailing_fail_count=state.trailing_fail_count,
        last_fail_ts=state.last_fail_ts,
        stuck=_is_stuck(state, stream_updated_ms),
    )


def _segment_backlog_units(
    day: str,
    segments: list[dict],
    progress: dict[tuple[str | None, str], SegmentProgress],
    terminal_states: dict[TerminalUnit, TerminalState],
    stream_updated_ms: int | None,
    repair_attempted: bool,
) -> tuple[BacklogUnit, ...]:
    why: list[BacklogUnit] = []
    for seg in segments:
        key = seg["key"]
        if not segment_fully_sensed(seg["data_state"]):
            for modality, state in seg["data_state"].items():
                if state != "failed":
                    continue
                marker = _read_failed_marker(day, seg["stream"], key, modality)
                if marker is None:
                    continue
                reason, failed_at_ms = marker
                if reason != "marker_corrupt":
                    continue
                if stream_updated_ms is not None and stream_updated_ms > failed_at_ms:
                    continue
                why.append(
                    BacklogUnit(
                        mode="segment",
                        name=modality,
                        facet=None,
                        stream=seg["stream"],
                        segment=key,
                        why=WHY_CORRUPT_RAW,
                        reason_code=None,
                        provider=None,
                        model=None,
                        trailing_fail_count=0,
                        last_fail_ts=failed_at_ms,
                        stuck=True,
                    )
                )
            continue

        segment_progress = lookup_segment_progress(progress, seg["stream"], key)
        ok, reason = segment_fully_thought(segment_progress)
        if ok or reason is None:
            continue
        if reason == "no_sense_complete":
            if (
                not repair_attempted
                and stream_updated_ms is not None
                and now_ms() - stream_updated_ms >= NO_SENSE_COMPLETE_AGED_MS
            ):
                why.append(
                    BacklogUnit(
                        mode="segment",
                        name="sense",
                        facet=None,
                        stream=seg["stream"],
                        segment=key,
                        why=WHY_NO_SENSE_COMPLETE_AGED,
                        reason_code=None,
                        provider=None,
                        model=None,
                        trailing_fail_count=0,
                        last_fail_ts=stream_updated_ms,
                        stuck=False,
                    )
                )
            continue

        if reason.startswith("floor:"):
            name = reason.split(":", 1)[1]
            unit = _terminal_unit_for_segment(name, seg["stream"], key)
            state = terminal_states.get(unit)
            if state and state.latest_event == TERMINAL_FAIL:
                why.append(_failed_backlog_unit(unit, state, stream_updated_ms))
            elif segment_progress and name in segment_progress.dispatched:
                why.append(
                    BacklogUnit(
                        mode=unit.mode,
                        name=unit.name,
                        facet=unit.facet,
                        stream=unit.stream,
                        segment=unit.segment,
                        why=WHY_SENSED_NOT_THOUGHT,
                        reason_code=None,
                        provider=None,
                        model=None,
                        trailing_fail_count=0,
                        last_fail_ts=None,
                        stuck=False,
                    )
                )
            else:
                # never_attempted is intentionally enumerated only for segment
                # floor talents. Non-segment modes do not have a persisted
                # expected-unit set in this pure-read derivation.
                why.append(
                    BacklogUnit(
                        mode=unit.mode,
                        name=unit.name,
                        facet=unit.facet,
                        stream=unit.stream,
                        segment=unit.segment,
                        why=WHY_NEVER_ATTEMPTED,
                        reason_code=None,
                        provider=None,
                        model=None,
                        trailing_fail_count=0,
                        last_fail_ts=None,
                        stuck=False,
                    )
                )
        elif reason.startswith("dispatched:"):
            name = reason.split(":", 1)[1]
            unit = _terminal_unit_for_segment(name, seg["stream"], key)
            state = terminal_states.get(unit)
            if state and state.latest_event == TERMINAL_FAIL:
                why.append(_failed_backlog_unit(unit, state, stream_updated_ms))
            else:
                why.append(
                    BacklogUnit(
                        mode=unit.mode,
                        name=unit.name,
                        facet=unit.facet,
                        stream=unit.stream,
                        segment=unit.segment,
                        why=WHY_SENSED_NOT_THOUGHT,
                        reason_code=None,
                        provider=None,
                        model=None,
                        trailing_fail_count=0,
                        last_fail_ts=None,
                        stuck=False,
                    )
                )
    return tuple(why)


def _representative_reason_unit(why: tuple[BacklogUnit, ...]) -> BacklogUnit | None:
    candidates = [unit for unit in why if unit.why == WHY_FAILED and unit.reason_code]
    if not candidates:
        return None
    return sorted(
        candidates,
        key=lambda unit: (
            unit.mode,
            unit.name,
            unit.facet or "",
            unit.stream or "",
            unit.segment or "",
        ),
    )[0]


def _non_segment_failed_units(
    terminal_states: dict[TerminalUnit, TerminalState],
    stream_updated_ms: int | None,
) -> tuple[BacklogUnit, ...]:
    why: list[BacklogUnit] = []
    for unit, state in sorted(
        terminal_states.items(),
        key=lambda item: (
            item[0].mode,
            item[0].name,
            item[0].facet or "",
            item[0].activity or "",
        ),
    ):
        if unit.segment is not None:
            continue
        if unit.mode not in {"daily", "activity", "flush"}:
            continue
        if state.latest_event != TERMINAL_FAIL:
            continue
        # These modes do not have a persisted expected-unit set, so only
        # observed latest-fail units are surfaced; never-attempted is not inferred.
        why.append(_failed_backlog_unit(unit, state, stream_updated_ms))
    return tuple(why)


def _capped_daily_complete_fields(day: str) -> dict[str, object]:
    capped = [
        {
            "name": name,
            "facet": facet,
            "reason_code": failure.reason_code,
            "count": failure.count,
        }
        for (name, facet), failure in sorted(
            read_daily_deterministic_failures(day).items(),
            key=lambda item: (item[0][0], item[0][1] or ""),
        )
        if failure_capped(failure.reason_code, failure.count)
    ]
    if not capped:
        return {}
    return {
        "capped_daily_unit_count": len(capped),
        "capped_daily_unit": capped[0],
    }


def _complete_backlog_day(day: str) -> BacklogDay:
    return BacklogDay(
        day=day,
        state=BACKLOG_STATE_COMPLETE,
        segments=0,
        units=0,
        not_sensed=0,
        why=(),
        reason=None,
        reason_code=None,
        provider=None,
        model=None,
        error=None,
        **_capped_daily_complete_fields(day),
    )


_SEGMENT_REPAIR_STATE = {
    "degraded": (BACKLOG_STATE_PENDING, REASON_SEGMENT_REPAIR_DEGRADED),
    "progressing": (BACKLOG_STATE_PENDING, REASON_SEGMENT_REPAIR_PROGRESSING),
    "stuck": (BACKLOG_STATE_STUCK, REASON_SEGMENT_REPAIR_STUCK),
    "unknown": (BACKLOG_STATE_UNKNOWN, REASON_SEGMENT_REPAIR_UNKNOWN),
}
_STATE_SEVERITY = {
    BACKLOG_STATE_COMPLETE: 0,
    BACKLOG_STATE_PENDING: 1,
    BACKLOG_STATE_STUCK: 2,
    BACKLOG_STATE_UNKNOWN: 3,
}


def _segment_repair_fields(repair: dict | None) -> dict:
    if not repair:
        return {}
    return {
        "segment_repair_status": repair["status"],
        "segment_repair_attempts": int(repair.get("attempts") or 0),
        "segment_repair_consecutive_non_completion": int(
            repair.get("consecutive_non_completion") or 0
        ),
        "segment_repair_last_outcome": repair.get("last_outcome") or None,
        "segment_repair_next_retry_at": repair.get("next_retry_at"),
        "segment_repair_reason_code": repair.get("repair_reason_code"),
        "segment_repair_timeout_seconds": repair.get("timeout_seconds"),
        "segment_repair_bounded": repair.get("bounded"),
        "segment_repair_cleared": repair.get("cleared"),
        "segment_repair_remaining": repair.get("remaining"),
    }


def _escalate_for_repair(state, reason, reason_code, error, day, repair):
    if not repair:
        return state, reason, reason_code, error
    sr_state, sr_reason = _SEGMENT_REPAIR_STATE[repair["status"]]
    if _STATE_SEVERITY[sr_state] > _STATE_SEVERITY[state]:
        state = sr_state
    if reason_code is None:
        reason = sr_reason
        reason_code = sr_reason
    if repair["status"] == "unknown" and error is None:
        error = BacklogError(
            day=day,
            stage="segment_repair",
            message="segment-repair state unreadable",
        )
    return state, reason, reason_code, error


def _backlog_day_for_complete(day: str, repair: dict | None) -> BacklogDay:
    if not repair:
        return _complete_backlog_day(day)
    state, reason, reason_code, error = _escalate_for_repair(
        BACKLOG_STATE_COMPLETE, None, None, None, day, repair
    )
    return BacklogDay(
        day=day,
        state=state,
        segments=0,
        units=0,
        not_sensed=0,
        why=(),
        reason=reason,
        reason_code=reason_code,
        provider=None,
        model=None,
        error=error,
        **_capped_daily_complete_fields(day),
        **_segment_repair_fields(repair),
    )


def read_backlog_view(window: int = BACKLOG_DEFAULT_WINDOW) -> BacklogView:
    """Return a bounded cross-day backlog view."""
    backlog_days: list[BacklogDay] = []
    errors: list[BacklogError] = []

    for day in sorted(day_dirs().keys(), reverse=True)[:window]:
        repair = read_segment_repair_summary(day)
        repair_attempted = read_segment_repair_attempted(day)
        if day_is_complete(day):
            backlog_days.append(_backlog_day_for_complete(day, repair))
            continue

        try:
            terminal_states = read_terminal_states(day)
        except Exception as exc:
            logger.warning(
                "pipeline_health: terminal-state backlog fold failed for %s",
                day,
                exc_info=True,
            )
            error = BacklogError(day=day, stage="terminal_states", message=str(exc))
            errors.append(error)
            backlog_days.append(
                BacklogDay(
                    day=day,
                    state=BACKLOG_STATE_UNKNOWN,
                    segments=0,
                    units=0,
                    not_sensed=0,
                    why=(),
                    reason=None,
                    reason_code=None,
                    provider=None,
                    model=None,
                    error=error,
                )
            )
            continue

        try:
            progress = read_segment_progress(day)
            segments = cluster_segments(day)
            completion = classify_segment_completion(segments, progress)
        except Exception as exc:
            logger.warning(
                "pipeline_health: segment backlog fold failed for %s",
                day,
                exc_info=True,
            )
            error = BacklogError(day=day, stage="segment_completion", message=str(exc))
            errors.append(error)
            backlog_days.append(
                BacklogDay(
                    day=day,
                    state=BACKLOG_STATE_UNKNOWN,
                    segments=0,
                    units=0,
                    not_sensed=0,
                    why=(),
                    reason=None,
                    reason_code=None,
                    provider=None,
                    model=None,
                    error=error,
                )
            )
            continue

        stream_ms = _stream_updated_ms(day)
        why = _segment_backlog_units(
            day, segments, progress, terminal_states, stream_ms, repair_attempted
        ) + _non_segment_failed_units(terminal_states, stream_ms)
        backoff = read_backoff_summary(day)
        segment_depth = completion.not_sensed + completion.not_thought
        if any(unit.why == WHY_CORRUPT_RAW and unit.stuck for unit in why):
            reason = REASON_CORRUPT_RAW
        elif any(unit.stuck for unit in why):
            reason = REASON_FAILING_STEP
        else:
            reason = None

        representative = _representative_reason_unit(why)
        reason_code = representative.reason_code if representative else None
        if reason is None and backoff:
            reason = REASON_CATCHUP_BACKOFF
            reason_code = "catchup_backoff"

        if any(unit.stuck for unit in why) or backoff:
            state = BACKLOG_STATE_STUCK
        elif segment_depth > 0 or why:
            state = BACKLOG_STATE_PENDING
        else:
            state = BACKLOG_STATE_COMPLETE
        state, reason, reason_code, sr_error = _escalate_for_repair(
            state, reason, reason_code, None, day, repair
        )

        backlog_days.append(
            BacklogDay(
                day=day,
                state=state,
                segments=segment_depth,
                units=len(why),
                not_sensed=completion.not_sensed,
                why=why,
                reason=reason,
                reason_code=reason_code,
                provider=representative.provider if representative else None,
                model=representative.model if representative else None,
                error=sr_error,
                backoff_stuck=bool(backoff),
                backoff_attempts=backoff["attempts"] if backoff else 0,
                backoff_consecutive_non_completion=(
                    backoff["consecutive_non_completion"] if backoff else 0
                ),
                backoff_last_outcome=backoff["last_outcome"] if backoff else None,
                backoff_next_retry_at=backoff["next_retry_at"] if backoff else None,
                **_segment_repair_fields(repair),
            )
        )

    pending_days = sum(1 for day in backlog_days if day.state == BACKLOG_STATE_PENDING)
    stuck_days = sum(1 for day in backlog_days if day.state == BACKLOG_STATE_STUCK)
    outstanding = [
        day.day
        for day in backlog_days
        if day.state in {BACKLOG_STATE_PENDING, BACKLOG_STATE_STUCK}
    ]
    return BacklogView(
        window=window,
        days=tuple(backlog_days),
        pending_days=pending_days,
        stuck_days=stuck_days,
        oldest_pending_day=min(outstanding) if outstanding else None,
        errors=tuple(errors),
        degraded=bool(errors)
        or any(day.state == BACKLOG_STATE_UNKNOWN for day in backlog_days),
    )


def read_segment_backlog() -> SegmentBacklog:
    """Sum segment-completion verdicts across updated_days() read-only."""
    days = tuple(updated_days())
    per_day: dict[str, SegmentCompletion] = {}
    errors: list[str] = []

    for day in days:
        try:
            per_day[day] = classify_segment_completion(
                cluster_segments(day),
                read_segment_progress(day),
            )
        except Exception:
            logger.warning(
                "pipeline_health: segment completion fold failed for %s",
                day,
                exc_info=True,
            )
            errors.append(day)

    return SegmentBacklog(
        days=days,
        not_thought=sum(completion.not_thought for completion in per_day.values()),
        not_sensed=sum(completion.not_sensed for completion in per_day.values()),
        total=sum(completion.total for completion in per_day.values()),
        per_day=per_day,
        errors=tuple(errors),
    )


def pipeline_status_message(summary: dict) -> dict | None:
    """Return a short user-facing message for non-healthy summaries."""
    if summary.get("status") == "healthy":
        return None

    anomalies = summary.get("anomalies", [])
    if any(anomaly.get("kind") == "activity_agents_missing" for anomaly in anomalies):
        return {
            "status": "stale",
            "message": "Activity processing gap — meeting notes may be delayed",
        }
    if any(anomaly.get("kind") == "daily_agents_missing" for anomaly in anomalies):
        return {
            "status": "stale",
            "message": "Daily processing hasn't run yet",
        }
    seg = next(
        (
            anomaly
            for anomaly in anomalies
            if anomaly.get("kind") == "segments_not_thought"
        ),
        None,
    )
    if seg is not None:
        if seg.get("error"):
            return {
                "status": summary.get("status", "stale"),
                "message": "Segment analysis status unavailable",
            }
        count = seg.get("not_thought", 0)
        plural = "s" if count != 1 else ""
        return {
            "status": "stale",
            "message": f"{count} segment{plural} awaiting thinking",
        }
    if any(anomaly.get("kind") == "talent_failure" for anomaly in anomalies):
        count = summary.get("talents", {}).get("outstanding_failed", 0)
        plural = "s" if count != 1 else ""
        return {
            "status": "warning",
            "message": f"{count} talent error{plural} today",
        }
    return None
