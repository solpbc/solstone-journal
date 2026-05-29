# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Summarize think pipeline health from daily JSONL logs."""

from __future__ import annotations

import json
import logging
from dataclasses import dataclass
from datetime import datetime

from solstone.think.cluster import cluster_segments
from solstone.think.utils import day_path, now_ms, updated_days

logger = logging.getLogger(__name__)

# Test indirection: tests monkeypatch this for time-sensitive branches.
_now = datetime.now

_MODES = ("segment", "daily", "activity", "weekly", "flush")
_FAILED_LIST_CAP = 20
SEGMENT_FLOOR_TALENTS: tuple[str, ...] = ("entities", "documents")


@dataclass(frozen=True)
class SegmentProgress:
    """Per-segment fold of think-pipeline health for one day."""

    sensed: bool
    density: str | None
    dispatched: frozenset[str]
    completed: frozenset[str]
    unconfigured: frozenset[str]


@dataclass(frozen=True)
class SegmentCompletion:
    """Per-segment completion verdict for clustered segments."""

    blockers: list[dict[str, str]]
    not_sensed: int
    not_thought: int
    total: int


@dataclass(frozen=True)
class SegmentBacklog:
    """Cross-day segment completion backlog over updated days."""

    days: tuple[str, ...]
    not_thought: int
    not_sensed: int
    total: int
    per_day: dict[str, SegmentCompletion]
    errors: tuple[str, ...]


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
            "skipped": 0,
            "failed_list": [],
            "failed_list_truncated": False,
        },
        "activities": {
            "detected": 0,
            "persisted": 0,
            "talents_fired": False,
        },
    }

    try:
        health_dir = day_path(day, create=False) / "health"
        if not health_dir.is_dir():
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

                    event = rec["event"]
                    if event == "talent.dispatch":
                        summary["talents"]["dispatched"] += 1
                    elif event == "talent.complete":
                        summary["talents"]["completed"] += 1
                    elif event == "talent.fail":
                        summary["talents"]["failed"] += 1
                        if len(summary["talents"]["failed_list"]) < _FAILED_LIST_CAP:
                            summary["talents"]["failed_list"].append(
                                {
                                    "mode": rec.get("mode") or mode,
                                    "name": rec.get("name"),
                                    "use_id": rec.get("use_id"),
                                    "state": rec.get("state"),
                                }
                            )
                        else:
                            summary["talents"]["failed_list_truncated"] = True
                    elif event == "talent.skip":
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
                    elif (
                        event == "run.start" and (rec.get("mode") or mode) == "activity"
                    ):
                        summary["activities"]["talents_fired"] = True
    except Exception:
        logger.warning(
            "pipeline_health: unexpected error summarizing %s",
            day,
            exc_info=True,
        )
        return summary

    for failure in summary["talents"]["failed_list"]:
        summary["anomalies"].append({"kind": "talent_failure", **failure})

    if (
        summary["activities"]["detected"] > 0
        and summary["runs"]["activity"]["count"] == 0
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
        completion = classify_segment_completion(
            cluster_segments(day),
            read_segment_progress(day),
        )
        if completion.not_thought > 0:
            # The kind now means segments sensed-but-not-thought, not zero runs.
            summary["anomalies"].append(
                {
                    "kind": "segment_runs_missing",
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
            {"kind": "segment_runs_missing", "error": "fold_failed"}
        )

    has_stale = any(
        anomaly["kind"]
        in {"activity_agents_missing", "daily_agents_missing", "segment_runs_missing"}
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


def read_completed_units(day: str) -> set[tuple[str, str, str | None]]:
    """Return unit keys whose latest terminal health event is complete.

    Folds the day's health JSONL files read-only. Terminal events are only
    ``talent.complete`` and ``talent.fail``; ``talent.skip``,
    ``talent.dispatch``, and ``run.*`` events are non-terminal. For each
    ``(mode, name, facet)`` unit, the latest terminal event wins by ``ts``;
    when timestamps tie, later records in sorted-file and line order win.
    Units with no terminal event are incomplete and omitted. Units whose latest
    terminal event is ``talent.fail`` are incomplete and omitted.

    This function does not create, modify, or delete journal state.
    """
    latest: dict[tuple[str, str, str | None], tuple[int, bool]] = {}

    try:
        health_dir = day_path(day, create=False) / "health"
        if not health_dir.is_dir():
            return set()

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

                    key = (mode, name, rec.get("facet"))
                    if key not in latest or ts >= latest[key][0]:
                        latest[key] = (ts, event == "talent.complete")
    except Exception:
        logger.warning(
            "pipeline_health: unexpected error reading completed units for %s",
            day,
            exc_info=True,
        )

    return {key for key, (_ts, is_complete) in latest.items() if is_complete}


def read_segment_progress(day: str) -> dict[tuple[str | None, str], SegmentProgress]:
    """Return per-segment progress from the day's segment health events.

    Folds the day's health JSONL files read-only. Progress is keyed by
    ``(stream, segment)``. Untagged historical records use a legacy ``None``
    stream bucket. Segment-scoped records are records with ``mode == "segment"``
    and a truthy string ``segment`` field. Terminal events are only
    ``talent.complete`` and ``talent.fail``; the latest terminal per
    ``((stream, segment), name)`` wins by ``ts``. ``talent.skip`` is
    non-terminal, except ``reason="no_config"`` is tracked for floor verdicts.

    This function does not create, modify, or delete journal state.
    """
    latest_sense: dict[tuple[str | None, str], tuple[int, str | None]] = {}
    dispatched: dict[tuple[str | None, str], set[str]] = {}
    terminals: dict[tuple[str | None, str], dict[str, tuple[int, bool]]] = {}
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
                                event == "talent.complete",
                            )
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

    segments = set(latest_sense) | set(dispatched) | set(terminals) | set(unconfigured)
    progress: dict[tuple[str | None, str], SegmentProgress] = {}
    for key in sorted(segments, key=lambda k: (k[1], k[0] is not None, k[0] or "")):
        segment_terminals = terminals.get(key, {})
        progress[key] = SegmentProgress(
            sensed=key in latest_sense,
            density=latest_sense.get(key, (0, None))[1],
            dispatched=frozenset(dispatched.get(key, set())),
            completed=frozenset(
                name
                for name, (_ts, is_complete) in segment_terminals.items()
                if is_complete
            ),
            unconfigured=frozenset(unconfigured.get(key, set())),
        )
    return progress


def segment_fully_sensed(data_state: dict[str, str]) -> bool:
    """True when every non-absent modality has finished sensing.

    ``data_state`` is the per-segment dict from ``cluster_segments``; it already
    omits absent modalities, so an absent modality cannot peg a segment. Note
    cluster's ``_detect_data_state`` cannot emit ``purged`` today; ``purged`` is
    kept here for ``DataState`` vocabulary alignment and forward-safety.
    """
    return all(state in {"analyzed", "purged"} for state in data_state.values())


def segment_fully_thought(progress: SegmentProgress | None) -> tuple[bool, str | None]:
    """Per-segment fully-thought verdict. Returns (ok, blocking_reason)."""
    if progress is None or not progress.sensed:
        return False, "no_sense_complete"
    if progress.density == "idle":
        return True, None
    for name in SEGMENT_FLOOR_TALENTS:
        if name not in progress.completed and name not in progress.unconfigured:
            return False, f"floor:{name}"
    for name in sorted(progress.dispatched):
        if name not in progress.completed:
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

    for seg in segments:
        key = seg["key"]
        if not segment_fully_sensed(seg["data_state"]):
            detail = ",".join(
                f"{modality}={state}"
                for modality, state in sorted(seg["data_state"].items())
                if state not in {"analyzed", "purged"}
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

        ok, reason = segment_fully_thought(
            lookup_segment_progress(progress, seg["stream"], key)
        )
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
            if anomaly.get("kind") == "segment_runs_missing"
        ),
        None,
    )
    if seg is not None:
        if seg.get("error"):
            return {
                "status": "stale",
                "message": "Segment thinking status unavailable",
            }
        count = seg.get("not_thought", 0)
        plural = "s" if count != 1 else ""
        return {
            "status": "stale",
            "message": f"{count} segment{plural} awaiting thinking",
        }
    if any(anomaly.get("kind") == "talent_failure" for anomaly in anomalies):
        count = summary.get("talents", {}).get("failed", 0)
        plural = "s" if count != 1 else ""
        return {
            "status": "warning",
            "message": f"{count} talent error{plural} today",
        }
    return None
