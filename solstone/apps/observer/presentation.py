# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Observer management presentation shared by non-wire consumers."""

from __future__ import annotations

from typing import Any

import solstone.convey.bridge as convey_bridge

from .utils import get_active_ingest_rejection, observer_filename_prefix

ACTIVE_THRESHOLD_MS = 30_000
STALE_THRESHOLD_MS = 120_000
FUTURE_CLOCK_DRIFT_TOLERANCE_MS = 5 * 60 * 1000

OBSERVER_STATE_LABELS = {
    "connected": "connected",
    "stale": "not reporting",
    "disconnected": "offline",
    "revoked": "removed",
}


def classify_observer_freshness(
    last_seen_ms: int | None,
    revoked: bool,
    now_ms: int,
) -> dict[str, object]:
    """Classify a registered observer's freshness.

    Returns keys: state, group, elapsed_ms, clock_skew.
    """
    if revoked:
        return {
            "state": "revoked",
            "group": "inactive",
            "elapsed_ms": None,
            "clock_skew": False,
        }
    if last_seen_ms is None:
        return {
            "state": "disconnected",
            "group": "inactive",
            "elapsed_ms": None,
            "clock_skew": False,
        }
    elapsed = now_ms - last_seen_ms
    if elapsed < -FUTURE_CLOCK_DRIFT_TOLERANCE_MS:
        return {
            "state": "disconnected",
            "group": "inactive",
            "elapsed_ms": elapsed,
            "clock_skew": True,
        }
    if elapsed < 0:
        return {
            "state": "connected",
            "group": "active",
            "elapsed_ms": 0,
            "clock_skew": False,
        }
    if elapsed < ACTIVE_THRESHOLD_MS:
        return {
            "state": "connected",
            "group": "active",
            "elapsed_ms": elapsed,
            "clock_skew": False,
        }
    if elapsed < STALE_THRESHOLD_MS:
        return {
            "state": "stale",
            "group": "stale",
            "elapsed_ms": elapsed,
            "clock_skew": False,
        }
    return {
        "state": "disconnected",
        "group": "inactive",
        "elapsed_ms": elapsed,
        "clock_skew": False,
    }


def serialize_observer(observer: dict[str, Any], current_now: int) -> dict[str, Any]:
    """Serialize a registered observer for management API consumers."""
    revoked = observer.get("revoked", False)
    enabled = observer.get("enabled", True)
    rejection = get_active_ingest_rejection(observer)
    failing = bool(rejection) and not revoked and enabled
    freshness = classify_observer_freshness(
        observer.get("last_seen"),
        revoked,
        current_now,
    )
    key_prefix = observer_filename_prefix(observer)
    data = {
        "prefix": key_prefix,
        "name": observer.get("name", ""),
        "created_at": observer.get("created_at", 0),
        "last_seen": observer.get("last_seen"),
        "last_segment": observer.get("last_segment"),
        "enabled": enabled,
        "revoked": revoked,
        "revoked_at": observer.get("revoked_at"),
        "stats": observer.get("stats", {}),
        "live": convey_bridge.subscription_count(key_prefix) > 0,
        "last_chat_request_at": convey_bridge.last_chat_request_at(key_prefix),
        **freshness,
        "label": OBSERVER_STATE_LABELS[str(freshness["state"])],
        "failing": failing,
    }
    if failing:
        data["ingest_rejection"] = {
            "reason_code": rejection.get("reason_code"),
            "active_count": rejection.get("active_count"),
            "first_ts": rejection.get("first_ts"),
            "latest_ts": rejection.get("latest_ts"),
            "summary": rejection.get("summary"),
            "stream": rejection.get("stream"),
            "version": rejection.get("version"),
        }
    return data
