# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Entity merge-candidate storage helpers.

Sole write-owner of:
  journal/entities/review-candidates.jsonl
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


def review_candidates_dir() -> Path:
    """Return the entity review-candidates directory, creating it if needed."""
    path = Path(get_journal()) / "entities"
    path.mkdir(parents=True, exist_ok=True)
    return path


def review_candidates_path() -> Path:
    """Return the entity merge-candidates JSONL path."""
    return review_candidates_dir() / "review-candidates.jsonl"


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
                    "review candidates: malformed JSONL line %s in %s", lineno, path
                )
                continue
            if not isinstance(data, dict):
                logger.warning(
                    "review candidates: non-object JSONL line %s in %s (got %s)",
                    lineno,
                    path,
                    type(data).__name__,
                )
                continue
            rows.append(data)
    return rows


def load_candidates() -> list[dict[str, Any]]:
    """Load entity merge candidates from JSONL."""
    return _load_jsonl_rows(review_candidates_path())


def _save_jsonl_rows(path: Path, rows: list[dict[str, Any]]) -> None:
    """Write *rows* to *path* as JSONL using an atomic replace."""
    content = ""
    if rows:
        content = "\n".join(json.dumps(row, ensure_ascii=False) for row in rows) + "\n"
    atomic_replace(path, content)


def save_candidates(rows: list[dict[str, Any]]) -> None:
    """Persist entity merge candidates atomically."""
    _save_jsonl_rows(review_candidates_path(), rows)


def candidate_key(facet: str, source_slug: str, target_slug: str) -> str:
    """Return the deterministic key for one merge candidate."""
    return f"{facet}|{source_slug}|{target_slug}"


def find_candidate(
    rows: list[dict[str, Any]], facet: str, source_slug: str, target_slug: str
) -> dict[str, Any] | None:
    """Return one merge candidate by key, or None when not found."""
    target_key = candidate_key(facet, source_slug, target_slug)
    for row in rows:
        row_key = candidate_key(
            str(row.get("facet") or ""),
            str(row.get("source_slug") or ""),
            str(row.get("target_slug") or ""),
        )
        if row_key == target_key:
            return row
    return None


def locked_modify_candidates(
    fn: Callable[[list[dict[str, Any]]], list[dict[str, Any]]],
) -> list[dict[str, Any]]:
    """Apply a locked read-modify-write cycle to review-candidates.jsonl."""
    with hold_lock(review_candidates_path()):
        rows = load_candidates()
        new_rows = fn(rows)
        save_candidates(new_rows)
        return new_rows


def utc_now_iso() -> str:
    """Return the current UTC time as an ISO-8601 string ending in Z."""
    return (
        datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")
    )


def record_merge_candidate(
    *,
    facet: str,
    day: str,
    source: str,
    source_slug: str,
    target: str,
    target_slug: str,
    evidence: str,
    basis: str = "name-variant",
    detections: int | None = None,
    needs: int | None = None,
) -> tuple[dict[str, Any], bool]:
    """Create or update one entity merge candidate."""
    row: dict[str, Any] | None = None
    created = False

    def mutate(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
        nonlocal row, created
        existing = find_candidate(rows, facet, source_slug, target_slug)
        now = utc_now_iso()
        if existing is None:
            row = {
                "facet": facet,
                "source": source,
                "source_slug": source_slug,
                "target": target,
                "target_slug": target_slug,
                "status": "open",
                "evidence": {
                    "basis": basis,
                    "summary": evidence,
                    "detection_count": detections,
                    "needs": needs,
                },
                "first_surfaced": day,
                "last_surfaced": day,
                "created_at": now,
                "updated_at": now,
            }
            created = True
            return list(rows) + [row]

        ev = existing.setdefault("evidence", {})
        ev["basis"] = basis
        ev["summary"] = evidence
        if detections is not None:
            ev["detection_count"] = detections
        if needs is not None:
            ev["needs"] = needs
        existing["last_surfaced"] = day
        existing["updated_at"] = now
        row = existing
        created = False
        return rows

    locked_modify_candidates(mutate)

    if row is None:  # pragma: no cover - defensive assertion
        raise RuntimeError("record-merge-candidate produced no row")

    return row, created


def touch_updated(row: dict[str, Any]) -> None:
    """Update a candidate row's updated_at timestamp in place."""
    row["updated_at"] = utc_now_iso()


def accept_candidate(
    facet: str,
    source_slug: str,
    target_slug: str,
    *,
    merge_id: str | None = None,
) -> dict[str, Any] | None:
    """Mark one entity merge candidate accepted."""
    row: dict[str, Any] | None = None

    def mutate(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
        nonlocal row
        existing = find_candidate(rows, facet, source_slug, target_slug)
        if existing is None:
            return rows
        existing["status"] = "accepted"
        if merge_id:
            existing["merge_id"] = merge_id
        touch_updated(existing)
        row = existing
        return rows

    locked_modify_candidates(mutate)
    return row


def dismiss_candidate(
    facet: str,
    source_slug: str,
    target_slug: str,
) -> dict[str, Any] | None:
    """Mark one entity merge candidate dismissed and store its strength watermark."""
    row: dict[str, Any] | None = None

    def mutate(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
        nonlocal row
        existing = find_candidate(rows, facet, source_slug, target_slug)
        if existing is None:
            return rows
        evidence = existing.get("evidence", {})
        existing["status"] = "dismissed"
        # Preserved today; a future re-open may compare stronger evidence here.
        existing["dismissed_detection_count"] = (
            evidence.get("detection_count") if isinstance(evidence, dict) else None
        )
        touch_updated(existing)
        row = existing
        return rows

    locked_modify_candidates(mutate)
    return row
