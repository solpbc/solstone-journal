# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Facet review-candidate storage helpers.

Sole write-owner of:
  journal/facets/review-candidates.jsonl
"""

from __future__ import annotations

import json
import logging
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable

from solstone.think.journal_io import atomic_replace, hold_lock
from solstone.think.utils import get_journal

logger = logging.getLogger(__name__)


def facet_review_candidates_dir() -> Path:
    """Return the facet review-candidates directory, creating it if needed."""
    path = Path(get_journal()) / "facets"
    path.mkdir(parents=True, exist_ok=True)
    return path


def facet_review_candidates_path() -> Path:
    """Return the facet review-candidates JSONL path."""
    return facet_review_candidates_dir() / "review-candidates.jsonl"


def _load_jsonl_rows(path: Path) -> list[dict[str, Any]]:
    """Load JSONL rows from *path*, skipping blanks and malformed lines."""
    if not path.exists():
        return []

    rows: list[dict[str, Any]] = []
    with open(path, encoding="utf-8") as handle:
        for lineno, line in enumerate(handle, start=1):
            raw = line.strip()
            if not raw:
                continue
            try:
                data = json.loads(raw)
            except json.JSONDecodeError:
                logger.warning(
                    "facet review candidates: malformed JSONL line %s in %s",
                    lineno,
                    path,
                )
                continue
            if not isinstance(data, dict):
                logger.warning(
                    "facet review candidates: non-object JSONL line %s in %s (got %s)",
                    lineno,
                    path,
                    type(data).__name__,
                )
                continue
            rows.append(data)
    return rows


def load_candidates() -> list[dict[str, Any]]:
    """Load facet review candidates from JSONL."""
    return _load_jsonl_rows(facet_review_candidates_path())


def _save_jsonl_rows(path: Path, rows: list[dict[str, Any]]) -> None:
    """Write *rows* to *path* as JSONL using an atomic replace."""
    content = (
        "\n".join(json.dumps(row, ensure_ascii=False) for row in rows) + "\n"
        if rows
        else ""
    )
    atomic_replace(path, content)


def save_candidates(rows: list[dict[str, Any]]) -> None:
    """Persist facet review candidates atomically."""
    _save_jsonl_rows(facet_review_candidates_path(), rows)


def candidate_key(name_key: str) -> str:
    """Return the deterministic key for one facet review candidate."""
    return name_key


def find_candidate(rows: list[dict[str, Any]], name_key: str) -> dict[str, Any] | None:
    """Return one facet review candidate by key, or None when not found."""
    target_key = candidate_key(name_key)
    for row in rows:
        row_key = candidate_key(str(row.get("name_key") or ""))
        if row_key == target_key:
            return row
    return None


def locked_modify_candidates(
    fn: Callable[[list[dict[str, Any]]], list[dict[str, Any]]],
) -> list[dict[str, Any]]:
    """Apply a locked read-modify-write cycle to review-candidates.jsonl."""
    with hold_lock(facet_review_candidates_path()):
        rows = load_candidates()
        new_rows = fn(rows)
        save_candidates(new_rows)
        return new_rows


def utc_now_iso() -> str:
    """Return the current UTC time as an ISO-8601 string ending in Z."""
    return (
        datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")
    )


def touch_updated(row: dict[str, Any]) -> None:
    """Update a candidate row's updated_at timestamp in place."""
    row["updated_at"] = utc_now_iso()


def accept_candidate(name_key: str) -> dict[str, Any] | None:
    """Mark one facet review candidate accepted."""
    row: dict[str, Any] | None = None

    def mutate(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
        nonlocal row
        existing = find_candidate(rows, name_key)
        if existing is None:
            return rows
        existing["status"] = "accepted"
        touch_updated(existing)
        row = existing
        return rows

    locked_modify_candidates(mutate)
    return row


def dismiss_candidate(name_key: str) -> dict[str, Any] | None:
    """Mark one facet review candidate dismissed and store its strength watermark."""
    row: dict[str, Any] | None = None

    def mutate(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
        nonlocal row
        existing = find_candidate(rows, name_key)
        if existing is None:
            return rows
        existing["status"] = "dismissed"
        # Preserved today; a future re-open may compare stronger evidence here.
        existing["dismissed_count"] = existing.get("count")
        touch_updated(existing)
        row = existing
        return rows

    locked_modify_candidates(mutate)
    return row


def record_facet_candidate(
    name: str,
    name_key: str,
    count: int,
    window_days: int,
    samples: list[dict[str, Any]],
    day: str,
) -> dict[str, Any]:
    """Record or refresh one proposed facet candidate."""
    row: dict[str, Any] | None = None

    def mutate(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
        nonlocal row
        existing = find_candidate(rows, name_key)
        now = utc_now_iso()
        if existing is None:
            row = {
                "name": name,
                "name_key": name_key,
                "status": "open",
                "count": count,
                "window_days": window_days,
                "evidence": {"samples": samples},
                "first_surfaced": day,
                "last_surfaced": day,
                "created_at": now,
                "updated_at": now,
            }
            return list(rows) + [row]

        ev = existing.setdefault("evidence", {})
        ev["samples"] = samples
        existing["count"] = count
        existing["window_days"] = window_days
        existing["last_surfaced"] = day
        existing["updated_at"] = now
        row = existing
        return rows

    locked_modify_candidates(mutate)

    if row is None:  # pragma: no cover - defensive assertion
        raise RuntimeError("record_facet_candidate produced no row")
    return row
