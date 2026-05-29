# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Summarize think pipeline health from daily JSONL logs."""

from __future__ import annotations

import json
import logging
from datetime import datetime

from solstone.think.utils import day_path, iter_segments, now_ms

logger = logging.getLogger(__name__)

# Test indirection: tests monkeypatch this for time-sensitive branches.
_now = datetime.now

_MODES = ("segment", "daily", "activity", "weekly", "flush")
_FAILED_LIST_CAP = 20


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

    if summary["runs"]["segment"]["count"] == 0:
        try:
            segs = list(iter_segments(day))
        except Exception:
            segs = []
        if len(segs) >= 1:
            summary["anomalies"].append({"kind": "segment_runs_missing"})

    has_stale = any(
        anomaly["kind"] in {"activity_agents_missing", "daily_agents_missing"}
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
    if any(anomaly.get("kind") == "talent_failure" for anomaly in anomalies):
        count = summary.get("talents", {}).get("failed", 0)
        plural = "s" if count != 1 else ""
        return {
            "status": "warning",
            "message": f"{count} talent error{plural} today",
        }
    return None
