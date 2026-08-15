# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Speaker name-variant review-candidate storage helpers.

Sole write-owner of:
  journal/speakers/review-candidates.jsonl
"""

from __future__ import annotations

import json
import logging
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable

from solstone.think.journal_io import atomic_replace, hold_lock
from solstone.think.speaker_keep_separate import name_variant_pair_suppressed
from solstone.think.utils import get_journal

logger = logging.getLogger(__name__)


def review_candidates_dir() -> Path:
    """Return the speaker review-candidates directory, creating it if needed."""
    path = Path(get_journal()) / "speakers"
    path.mkdir(parents=True, exist_ok=True)
    return path


def review_candidates_path() -> Path:
    """Return the speaker name-variant candidates JSONL path."""
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
                    "speaker review candidates: malformed JSONL line %s in %s",
                    lineno,
                    path,
                )
                continue
            if not isinstance(data, dict):
                logger.warning(
                    "speaker review candidates: non-object JSONL line %s in %s (got %s)",
                    lineno,
                    path,
                    type(data).__name__,
                )
                continue
            rows.append(data)
    return rows


def load_candidates() -> list[dict[str, Any]]:
    """Load speaker name-variant candidates from JSONL."""
    return _load_jsonl_rows(review_candidates_path())


def _save_jsonl_rows(path: Path, rows: list[dict[str, Any]]) -> None:
    """Write *rows* to *path* as JSONL using an atomic replace."""
    content = ""
    if rows:
        content = "\n".join(json.dumps(row, ensure_ascii=False) for row in rows) + "\n"
    atomic_replace(path, content)


def save_candidates(rows: list[dict[str, Any]]) -> None:
    """Persist speaker name-variant candidates atomically."""
    _save_jsonl_rows(review_candidates_path(), rows)


def candidate_key(id_a: str, id_b: str) -> str:
    """Return the deterministic order-independent key for one speaker candidate."""
    return "|".join(sorted([id_a, id_b]))


def find_candidate(
    rows: list[dict[str, Any]], id_a: str, id_b: str
) -> dict[str, Any] | None:
    """Return one speaker review candidate by key, or None when not found."""
    target_key = candidate_key(id_a, id_b)
    for row in rows:
        row_key = candidate_key(
            str(row.get("source_id") or ""),
            str(row.get("target_id") or ""),
        )
        if row_key == target_key:
            return row
    return None


def _detection_count_from_row(row: dict[str, Any] | None) -> int:
    if not isinstance(row, dict):
        return 1
    evidence = row.get("evidence")
    if not isinstance(evidence, dict):
        return 1
    try:
        return max(1, int(evidence.get("detection_count", 1)))
    except (TypeError, ValueError):
        return 1


def detection_count_for_pair(id_a: str, id_b: str) -> int:
    """Return the current detection count for a candidate pair, defaulting to 1."""
    return _detection_count_from_row(find_candidate(load_candidates(), id_a, id_b))


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


def _summary(source_label: str, target_label: str, similarity: float) -> str:
    return (
        f"{source_label} and {target_label} have matching speaker voiceprints "
        f"(similarity {similarity:.4f})."
    )


def _evidence(
    source_label: str,
    target_label: str,
    similarity: float,
    readiness: str,
    detection_count: int,
) -> dict[str, Any]:
    return {
        "basis": "speaker-name-variant",
        "summary": _summary(source_label, target_label, similarity),
        "similarity": similarity,
        "detection_count": detection_count,
        "readiness": readiness,
    }


def record_name_variant_candidate(
    *,
    source_id: str,
    source_label: str,
    target_id: str,
    target_label: str,
    similarity: float,
    readiness: str = "ready",
) -> tuple[dict[str, Any], bool, bool]:
    """Create or update one speaker name-variant candidate."""
    row: dict[str, Any] | None = None
    created = False
    suppressed = False

    def mutate(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
        nonlocal row, created, suppressed
        existing = find_candidate(rows, source_id, target_id)
        now = utc_now_iso()
        if existing is None:
            detection_count = 1
            suppressed = name_variant_pair_suppressed(
                source_id,
                target_id,
                detection_count,
            )
            # Deterministic pairs reaching the recorder already met threshold and gates; "waiting" is reserved.
            row = {
                "source_id": source_id,
                "source_label": source_label,
                "target_id": target_id,
                "target_label": target_label,
                "status": "suppressed" if suppressed else "open",
                "similarity": similarity,
                "readiness": readiness,
                "evidence": _evidence(
                    source_label,
                    target_label,
                    similarity,
                    readiness,
                    detection_count,
                ),
                "first_surfaced": now,
                "last_surfaced": now,
                "created_at": now,
                "updated_at": now,
            }
            if suppressed:
                row["suppressed_by_keep_separate"] = True
                row["suppressed_detection_count"] = detection_count
            created = True
            return list(rows) + [row]

        evidence = existing.get("evidence", {})
        if not isinstance(evidence, dict):
            evidence = {}
        detection_count = evidence.get("detection_count", 0)
        try:
            detection_count = int(detection_count)
        except (TypeError, ValueError):
            detection_count = 0

        existing["source_id"] = source_id
        existing["source_label"] = source_label
        existing["target_id"] = target_id
        existing["target_label"] = target_label
        existing["similarity"] = similarity
        existing["readiness"] = readiness
        next_evidence = dict(evidence)
        next_detection_count = detection_count + 1
        next_evidence.update(
            _evidence(
                source_label,
                target_label,
                similarity,
                readiness,
                next_detection_count,
            )
        )
        existing["evidence"] = next_evidence
        existing["last_surfaced"] = now
        existing["updated_at"] = now
        status = str(existing.get("status") or "")
        suppressed_now = name_variant_pair_suppressed(
            source_id,
            target_id,
            next_detection_count,
        )
        if status not in {"accepted", "dismissed"}:
            if suppressed_now:
                existing["status"] = "suppressed"
                existing["suppressed_by_keep_separate"] = True
                existing["suppressed_detection_count"] = next_detection_count
                suppressed = True
            elif existing.get("suppressed_by_keep_separate") is True:
                existing["status"] = "open"
                existing.pop("suppressed_by_keep_separate", None)
                existing.pop("suppressed_detection_count", None)
                suppressed = False
        row = existing
        created = False
        return rows

    locked_modify_candidates(mutate)

    if row is None:  # pragma: no cover - defensive assertion
        raise RuntimeError("record-name-variant-candidate produced no row")

    return row, created, suppressed


def touch_updated(row: dict[str, Any]) -> None:
    """Update a candidate row's updated_at timestamp in place."""
    row["updated_at"] = utc_now_iso()


def accept_candidate(
    id_a: str,
    id_b: str,
    *,
    merge_id: str | None = None,
) -> dict[str, Any] | None:
    """Mark one speaker review candidate accepted."""
    row: dict[str, Any] | None = None

    def mutate(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
        nonlocal row
        existing = find_candidate(rows, id_a, id_b)
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


def dismiss_candidate(id_a: str, id_b: str) -> dict[str, Any] | None:
    """Mark one speaker review candidate dismissed and store its strength watermark."""
    row: dict[str, Any] | None = None

    def mutate(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
        nonlocal row
        existing = find_candidate(rows, id_a, id_b)
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
