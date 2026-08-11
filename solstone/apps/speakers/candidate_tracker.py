# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Headless cross-segment speaker candidate pool."""

from __future__ import annotations

import json
import logging
from collections import defaultdict
from collections.abc import Callable
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, TypeVar

import numpy as np

from solstone.apps.speakers.evidence import (
    _read_segment_overlap_fraction,
    _read_segment_speaker_evidence,
)
from solstone.apps.speakers.attribution import (
    _load_integer_speaker_labels,
    segment_path,
)
from solstone.apps.speakers.encoder_config import (
    CONFIRM_MIN_DURATION_S,
    CONFIRM_MIN_INTERVALS,
    CONFIRM_MIN_SEGMENTS,
    CONSOLIDATE_MERGE_THRESHOLD,
    CONSOLIDATE_MIN_INTERVALS,
    CONSOLIDATE_SUGGEST_MIN,
    MERGE_THRESHOLD,
    SOLO_CLUSTER_MIN_COSINE,
    SPLIT_THRESHOLD,
    STABILITY_THRESHOLD,
)
from solstone.think.entities import normalize_embedding
from solstone.think.entities.voiceprints import load_embeddings_file, load_owner_centroid
from solstone.think.journal_io import (
    LockTimeout,
    MalformedPolicy,
    atomic_replace,
    hold_lock,
    read_json,
)
from solstone.think.utils import get_journal, now_ms, segment_start_ts_ms

# Synthetic cluster label for producer-proven solo speaker segments.
SOLO_CLUSTER_LABEL: int = -1
logger = logging.getLogger(__name__)
T = TypeVar("T")


def trim_solo_cluster_rows(
    cluster_rows: list[T],
    *,
    embedding_for_row: Callable[[T], np.ndarray],
    normalize_embedding: Callable[[np.ndarray], np.ndarray | None],
) -> tuple[list[T], np.ndarray | None, int]:
    """Trim solo-cluster rows against their first all-row centroid."""
    if not cluster_rows:
        return [], None, 0

    stacked = np.stack([embedding_for_row(row) for row in cluster_rows])
    centroid = normalize_embedding(np.mean(stacked, axis=0))
    if centroid is None:
        return [], None, 0

    similarities = stacked @ centroid
    survivors = [
        row
        for row, similarity in zip(cluster_rows, similarities)
        if float(similarity) >= SOLO_CLUSTER_MIN_COSINE
    ]
    trimmed_count = len(cluster_rows) - len(survivors)
    if not survivors:
        return [], None, trimmed_count

    stacked = np.stack([embedding_for_row(row) for row in survivors])
    centroid = normalize_embedding(np.mean(stacked, axis=0))
    if centroid is None:
        return [], None, trimmed_count
    return survivors, centroid, trimmed_count


@dataclass
class CandidateProfile:
    cand_id: int
    centroid: np.ndarray
    n_segments: int
    n_intervals: int
    total_duration_s: float
    source_segments: list[dict[str, Any]] = field(default_factory=list)
    confirmed_entity: str | None = None
    status: str = "pending"
    merge_events: list[dict[str, Any]] = field(default_factory=list)

    def to_json(self) -> dict[str, Any]:
        return {
            "cand_id": self.cand_id,
            "centroid": self.centroid.astype(float).tolist(),
            "n_segments": self.n_segments,
            "n_intervals": self.n_intervals,
            "total_duration_s": self.total_duration_s,
            "source_segments": self.source_segments,
            "confirmed_entity": self.confirmed_entity,
            "status": self.status,
            "merge_events": self.merge_events,
        }

    @classmethod
    def from_json(cls, data: dict[str, Any]) -> CandidateProfile:
        return cls(
            cand_id=int(data["cand_id"]),
            centroid=np.asarray(data["centroid"], dtype=np.float32),
            n_segments=int(data.get("n_segments", 0)),
            n_intervals=int(data.get("n_intervals", 0)),
            total_duration_s=float(data.get("total_duration_s", 0.0)),
            source_segments=list(data.get("source_segments", [])),
            confirmed_entity=data.get("confirmed_entity"),
            status=str(data.get("status", "pending")),
            merge_events=list(data.get("merge_events", [])),
        )

    def ready_for_confirmation(self) -> bool:
        return (
            self.status == "pending"
            and self.n_segments >= CONFIRM_MIN_SEGMENTS
            and self.n_intervals >= CONFIRM_MIN_INTERVALS
            and self.total_duration_s >= CONFIRM_MIN_DURATION_S
        )


@dataclass(frozen=True)
class RetroactiveConfirmPlan:
    """Read-only plan for confirming a candidate and backfilling voiceprints."""

    matched: bool
    match_score: float | None
    candidate_id: int | None
    entity_id: str
    candidate_before: dict[str, Any] | None
    candidate_after: dict[str, Any] | None
    preexisting_voiceprint_keys: tuple[tuple[str, str, str, int], ...]
    voiceprints_to_add: tuple[dict[str, Any], ...]
    voiceprint_items_to_add: tuple[tuple[np.ndarray, dict[str, Any]], ...] = field(
        default_factory=tuple,
        repr=False,
        compare=False,
    )


def _empty_retroactive_plan(
    entity_id: str,
    *,
    match_score: float | None = None,
) -> RetroactiveConfirmPlan:
    return RetroactiveConfirmPlan(
        matched=False,
        match_score=match_score,
        candidate_id=None,
        entity_id=entity_id,
        candidate_before=None,
        candidate_after=None,
        preexisting_voiceprint_keys=(),
        voiceprints_to_add=(),
        voiceprint_items_to_add=(),
    )


def _entity_voiceprint_snapshot(
    entity_id: str,
    normalize_embedding: Callable[[np.ndarray], np.ndarray | None],
) -> tuple[set[tuple[str, str, str, int]], int, np.ndarray | None]:
    from solstone.think.entities import load_entity_voiceprints_file

    result = load_entity_voiceprints_file(entity_id)
    if result is None:
        return set(), 0, None

    embeddings, metadata = result
    keys: set[tuple[str, str, str, int]] = set()
    for meta in metadata:
        try:
            keys.add(
                (
                    str(meta["day"]),
                    str(meta["segment_key"]),
                    str(meta["source"]),
                    int(meta["sentence_id"]),
                )
            )
        except (KeyError, TypeError, ValueError):
            continue

    existing_norms = [
        normalized
        for embedding in embeddings
        if (normalized := normalize_embedding(embedding)) is not None
    ]
    centroid = (
        normalize_embedding(np.mean(existing_norms, axis=0)) if existing_norms else None
    )
    return keys, len(embeddings), centroid


def _retroactive_owner_context() -> tuple[np.ndarray, float] | None:
    centroid_data = load_owner_centroid()
    if centroid_data is None:
        return None
    return centroid_data.centroid, centroid_data.threshold


def _is_principal_entity(entity_id: str) -> bool:
    from solstone.think.entities import get_journal_principal

    principal = get_journal_principal()
    return bool(principal and principal.get("id") == entity_id)


def _retroactive_segment_noisy(seg_dir: Path, source: str) -> bool:
    from solstone.apps.speakers.attribution import NOISY_FLYWHEEL_OVERLAP_MAX

    jsonl_path = seg_dir / f"{source}.jsonl"
    return _read_segment_overlap_fraction(jsonl_path) > NOISY_FLYWHEEL_OVERLAP_MAX


def _retroactive_outlier_min_samples() -> int:
    from solstone.apps.speakers.attribution import VP_OUTLIER_MIN_SAMPLES

    return VP_OUTLIER_MIN_SAMPLES


def _retroactive_outlier_min_similarity() -> float:
    from solstone.apps.speakers.attribution import VP_OUTLIER_MIN_SIMILARITY

    return VP_OUTLIER_MIN_SIMILARITY


def _retroactive_voiceprint_metadata(
    day: str,
    stream: str,
    segment_key: str,
    source: str,
    sentence_id: int,
) -> dict[str, Any]:
    return {
        "day": day,
        "segment_key": segment_key,
        "source": source,
        "stream": stream,
        "sentence_id": sentence_id,
        "added_at": now_ms(),
        "last_seen_ts": segment_start_ts_ms(day, segment_key),
    }


def _source_key(source_segment: dict[str, Any]) -> tuple[str, str, str, str, int]:
    # Source identity deliberately stays independent from persisted membership.
    # If re-analysis renumbers the same acoustic partition, dedupe can still add
    # a second source key; that is a separate follow-up from safe re-location.
    return (
        str(source_segment["day"]),
        str(source_segment["segment_key"]),
        str(source_segment["stream"]),
        str(source_segment["source"]),
        int(source_segment["cluster_label"]),
    )


def encode_source_key(source_key: tuple[str, str, str, str, int]) -> str:
    """Return the stable JSON-string anchor for a source key tuple."""
    return json.dumps(list(source_key), ensure_ascii=False, separators=(",", ":"))


def source_segment_anchor(source_segment: dict[str, Any]) -> str:
    """Return the stable anchor for one source segment."""
    return encode_source_key(_source_key(source_segment))


def source_segment_sentence_ids(source_segment: dict[str, Any]) -> list[int] | None:
    """Return persisted member sentence ids, or None for legacy/unresolved records."""
    raw = source_segment.get("sentence_ids")
    if not isinstance(raw, list):
        return None
    sentence_ids: list[int] = []
    for item in raw:
        if isinstance(item, bool):
            return None
        try:
            sentence_ids.append(int(item))
        except (TypeError, ValueError):
            return None
    return sorted(set(sentence_ids))


def recover_source_segment_sentence_ids(
    source_segment: dict[str, Any],
) -> list[int] | None:
    """Recover legacy source membership from current on-disk NPZ and labels."""
    try:
        day = str(source_segment["day"])
        segment_key = str(source_segment["segment_key"])
        stream = str(source_segment["stream"])
        source = str(source_segment["source"])
        cluster_label = int(source_segment["cluster_label"])
    except (KeyError, TypeError, ValueError):
        return None

    seg_dir = segment_path(day, segment_key, stream, create=False)
    if not seg_dir.exists():
        return None
    emb_data = load_embeddings_file(seg_dir / f"{source}.npz")
    if emb_data is None:
        return None
    embeddings, statement_ids, _durations_s = emb_data
    sid_to_idx = {int(sid): index for index, sid in enumerate(statement_ids)}
    available_ids = set(sid_to_idx)
    if cluster_label == SOLO_CLUSTER_LABEL:
        cluster_rows: list[tuple[int, np.ndarray]] = []
        for sid in sorted(available_ids):
            idx = sid_to_idx.get(sid)
            if idx is None or idx >= len(embeddings):
                continue
            normalized = normalize_embedding(embeddings[idx])
            if normalized is None:
                continue
            cluster_rows.append((sid, normalized))
        trimmed_rows, centroid, _trimmed_count = trim_solo_cluster_rows(
            cluster_rows,
            embedding_for_row=lambda row: row[1],
            normalize_embedding=normalize_embedding,
        )
        if centroid is None or not trimmed_rows:
            return None
        return sorted(sid for sid, _embedding in trimmed_rows)

    integer_labels = _load_integer_speaker_labels(seg_dir, source)
    if not integer_labels:
        return None
    recovered = [
        int(sid)
        for sid, label in sorted(integer_labels.items())
        if int(label) == cluster_label and int(sid) in available_ids
    ]
    return recovered or None


def candidate_source_anchors(candidate: CandidateProfile) -> set[str]:
    """Return all stable anchors carried by one candidate."""
    return {
        source_segment_anchor(source_segment)
        for source_segment in candidate.source_segments
    }


def canonical_candidate_anchor(candidate: CandidateProfile) -> str:
    """Return the deterministic record-time canonical anchor for one candidate."""
    return min(sorted(candidate_source_anchors(candidate)))


def _segment_identity(source_segment: dict[str, Any]) -> tuple[str, str, str, str]:
    return (
        str(source_segment["day"]),
        str(source_segment["segment_key"]),
        str(source_segment["stream"]),
        str(source_segment["source"]),
    )


def _utc_now_iso() -> str:
    return (
        datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")
    )


class CandidateTracker:
    def __init__(self, store_path: Path | None = None) -> None:
        self.store_path = store_path or (
            Path(get_journal()) / "awareness" / "speaker_candidates.json"
        )
        self._candidates: dict[int, CandidateProfile] = {}
        self._next_id = 1
        self.consolidation_summary: dict[str, Any] = {
            "merge_count_total": 0,
            "last_merge": None,
        }
        self.load()

    def load(self) -> None:
        data = read_json(
            self.store_path,
            on_error=MalformedPolicy.WARN_AND_SKIP,
            default={"next_id": 1, "candidates": []},
        )
        self._next_id = int(data.get("next_id", 1))
        summary = data.get("consolidation_summary")
        if isinstance(summary, dict):
            last_merge = summary.get("last_merge")
            self.consolidation_summary = {
                "merge_count_total": int(summary.get("merge_count_total", 0)),
                "last_merge": last_merge if isinstance(last_merge, dict) else None,
            }
        else:
            self.consolidation_summary = {
                "merge_count_total": 0,
                "last_merge": None,
            }
        self._candidates = {
            candidate.cand_id: candidate
            for candidate in (
                CandidateProfile.from_json(raw)
                for raw in data.get("candidates", [])
                if isinstance(raw, dict)
            )
        }

    def save(self) -> None:
        with hold_lock(self.store_path):
            self._write()

    def _write(self) -> None:
        data = {
            "next_id": self._next_id,
            "candidates": [
                candidate.to_json()
                for candidate in sorted(
                    self._candidates.values(), key=lambda item: item.cand_id
                )
            ],
            "consolidation_summary": self.consolidation_summary,
        }
        atomic_replace(
            self.store_path,
            json.dumps(data, indent=2, sort_keys=True) + "\n",
        )

    def load_all_candidates(self) -> list[CandidateProfile]:
        """Return all tracked speaker candidates without mutating state."""
        return sorted(self._candidates.values(), key=lambda item: item.cand_id)

    def snapshot_candidates_locked(self) -> list[CandidateProfile]:
        """Return a fresh candidate snapshot under the pool lock."""
        with hold_lock(self.store_path):
            self.load()
            return self.load_all_candidates()

    def backfill_source_sentence_ids(self) -> dict[str, int]:
        """Backfill persisted member ids on legacy source segments."""
        updated = 0
        unresolved = 0
        already_present = 0
        with hold_lock(self.store_path):
            self.load()
            for candidate in self._candidates.values():
                for source_segment in candidate.source_segments:
                    if source_segment_sentence_ids(source_segment) is not None:
                        already_present += 1
                        continue
                    recovered = recover_source_segment_sentence_ids(source_segment)
                    if recovered is None:
                        unresolved += 1
                        continue
                    source_segment["sentence_ids"] = recovered
                    updated += 1
            if updated:
                self._write()
        return {
            "updated": updated,
            "unresolved": unresolved,
            "already_present": already_present,
        }

    def _new_id(self) -> int:
        cand_id = self._next_id
        self._next_id += 1
        return cand_id

    def _existing_source_keys(self) -> set[tuple[str, str, str, str, int]]:
        return {
            _source_key(source_segment)
            for candidate in self._candidates.values()
            for source_segment in candidate.source_segments
        }

    def _best_match(self, centroid: np.ndarray) -> tuple[int | None, float]:
        best_id: int | None = None
        best_score = -1.0
        for cand_id, candidate in self._candidates.items():
            if candidate.status == "rejected":
                continue
            score = float(np.dot(centroid, candidate.centroid))
            if score > best_score:
                best_id = cand_id
                best_score = score
        return best_id, best_score

    def _merge_candidate(
        self,
        candidate: CandidateProfile,
        centroid: np.ndarray,
        n_intervals: int,
        duration_s: float,
        source_segment: dict[str, Any],
        normalize_embedding,
    ) -> None:
        combined = candidate.centroid * float(candidate.n_intervals)
        combined += centroid * float(n_intervals)
        merged = normalize_embedding(combined)
        if merged is not None:
            candidate.centroid = merged

        segment_seen = any(
            existing["day"] == source_segment["day"]
            and existing["segment_key"] == source_segment["segment_key"]
            and existing["stream"] == source_segment["stream"]
            and existing["source"] == source_segment["source"]
            for existing in candidate.source_segments
        )
        if not segment_seen:
            candidate.n_segments += 1
        candidate.n_intervals += n_intervals
        candidate.total_duration_s += duration_s
        candidate.source_segments.append(source_segment)

    def _create_candidate(
        self,
        centroid: np.ndarray,
        n_intervals: int,
        duration_s: float,
        source_segment: dict[str, Any],
    ) -> CandidateProfile:
        cand_id = self._new_id()
        candidate = CandidateProfile(
            cand_id=cand_id,
            centroid=centroid,
            n_segments=1,
            n_intervals=n_intervals,
            total_duration_s=duration_s,
            source_segments=[source_segment],
        )
        self._candidates[cand_id] = candidate
        return candidate

    def _eligible_for_auto_consolidation(self, candidate: CandidateProfile) -> bool:
        return (
            candidate.n_intervals >= CONSOLIDATE_MIN_INTERVALS
            and candidate.status == "pending"
            and candidate.confirmed_entity is None
        )

    def _best_consolidation_pair(self) -> tuple[int, int, float] | None:
        eligible = [
            candidate
            for candidate in self.load_all_candidates()
            if self._eligible_for_auto_consolidation(candidate)
        ]
        best: tuple[int, int, float] | None = None
        for left_idx, left in enumerate(eligible):
            for right in eligible[left_idx + 1 :]:
                score = float(np.dot(left.centroid, right.centroid))
                if score < CONSOLIDATE_MERGE_THRESHOLD:
                    continue
                left_id, right_id = sorted([left.cand_id, right.cand_id])
                if best is None:
                    best = (left_id, right_id, score)
                    continue
                best_left, best_right, best_score = best
                if score > best_score or (
                    score == best_score
                    and (left_id, right_id) < (best_left, best_right)
                ):
                    best = (left_id, right_id, score)
        return best

    def _merge_candidate_profile(
        self,
        survivor: CandidateProfile,
        absorbed: CandidateProfile,
        score: float,
    ) -> dict[str, Any]:
        combined = survivor.centroid * float(survivor.n_intervals)
        combined += absorbed.centroid * float(absorbed.n_intervals)
        merged = normalize_embedding(combined)
        if merged is not None:
            survivor.centroid = merged

        existing_source_keys = {
            _source_key(source_segment) for source_segment in survivor.source_segments
        }
        for source_segment in absorbed.source_segments:
            source_key = _source_key(source_segment)
            if source_key in existing_source_keys:
                continue
            survivor.source_segments.append(source_segment)
            existing_source_keys.add(source_key)

        survivor.n_intervals += absorbed.n_intervals
        survivor.total_duration_s += absorbed.total_duration_s
        survivor.n_segments = len(
            {
                _segment_identity(source_segment)
                for source_segment in survivor.source_segments
            }
        )
        survivor.merge_events.extend(absorbed.merge_events)

        event = {
            "survivor_id": survivor.cand_id,
            "absorbed_id": absorbed.cand_id,
            "score": score,
            "merged_at": _utc_now_iso(),
            "absorbed_n_intervals": absorbed.n_intervals,
            "survivor_n_intervals_after": survivor.n_intervals,
        }
        survivor.merge_events.append(event)
        del self._candidates[absorbed.cand_id]
        return event

    def _record_consolidation_merge(self, event: dict[str, Any]) -> None:
        self.consolidation_summary = {
            "merge_count_total": int(
                self.consolidation_summary.get("merge_count_total", 0)
            )
            + 1,
            "last_merge": dict(event),
        }

    def consolidate_dense_candidates(self) -> dict[str, Any]:
        """Merge dense pending candidates to fixpoint under one pool lock."""
        merges: list[dict[str, Any]] = []
        with hold_lock(self.store_path):
            self.load()
            while True:
                pair = self._best_consolidation_pair()
                if pair is None:
                    break
                survivor_id, absorbed_id, score = pair
                survivor = self._candidates[survivor_id]
                absorbed = self._candidates[absorbed_id]
                event = self._merge_candidate_profile(survivor, absorbed, score)
                survivor.status = "pending"
                survivor.confirmed_entity = None
                self._record_consolidation_merge(event)
                merges.append(event)
                logger.info(
                    "speaker candidate auto-merged: survivor_id=%s absorbed_id=%s "
                    "score=%.4f absorbed_n_intervals=%s survivor_n_intervals_after=%s",
                    event["survivor_id"],
                    event["absorbed_id"],
                    event["score"],
                    event["absorbed_n_intervals"],
                    event["survivor_n_intervals_after"],
                )
            if merges:
                self._write()
        return {"status": "ok", "merged": len(merges), "merges": merges}

    def _candidate_for_anchor(self, anchor: str) -> CandidateProfile | None:
        for candidate in self._candidates.values():
            if anchor in candidate_source_anchors(candidate):
                return candidate
        return None

    def merge_candidate_pair(self, anchor_a: str, anchor_b: str) -> dict[str, Any]:
        """Manually merge one review-approved candidate pair by source anchors."""
        with hold_lock(self.store_path):
            self.load()
            left = self._candidate_for_anchor(anchor_a)
            right = self._candidate_for_anchor(anchor_b)
            if left is None or right is None:
                return {"status": "error", "error": "candidate anchor not found"}
            if left.cand_id == right.cand_id:
                return {"status": "already_merged", "survivor_id": left.cand_id}
            if left.status == "rejected" or right.status == "rejected":
                return {"status": "error", "error": "cannot merge rejected candidate"}
            confirmed = [
                candidate
                for candidate in (left, right)
                if candidate.status == "confirmed"
                or candidate.confirmed_entity is not None
            ]
            if len(confirmed) > 1:
                return {
                    "status": "error",
                    "error": "cannot merge two confirmed candidates",
                }
            score = float(np.dot(left.centroid, right.centroid))
            if score < CONSOLIDATE_SUGGEST_MIN:
                return {
                    "status": "error",
                    "error": "candidate pair is below review threshold",
                    "score": score,
                }

            survivor_id, absorbed_id = sorted([left.cand_id, right.cand_id])
            survivor = self._candidates[survivor_id]
            absorbed = self._candidates[absorbed_id]
            confirmed_entity = (
                confirmed[0].confirmed_entity if confirmed and confirmed[0] else None
            )
            event = self._merge_candidate_profile(survivor, absorbed, score)
            if confirmed_entity is not None:
                survivor.status = "confirmed"
                survivor.confirmed_entity = confirmed_entity
            else:
                survivor.status = "pending"
                survivor.confirmed_entity = None
            self._write()
        return {
            "status": "merged",
            "survivor_id": survivor_id,
            "absorbed_id": absorbed_id,
            "score": score,
            "merge_event": event,
            "confirmed_entity": confirmed_entity,
        }

    def process_segment(
        self,
        day: str,
        segment_key: str,
        stream: str,
        source: str,
        seg_dir: Path,
    ) -> None:
        integer_labels = _load_integer_speaker_labels(seg_dir, source)
        if not integer_labels:
            evidence = _read_segment_speaker_evidence(seg_dir / f"{source}.jsonl")
            if evidence.speaker_evidence != "single":
                return

        emb_data = load_embeddings_file(seg_dir / f"{source}.npz")
        if emb_data is None:
            return
        embeddings, statement_ids, durations_s = emb_data
        sid_to_idx = {int(sid): idx for idx, sid in enumerate(statement_ids)}
        existing_source_keys = self._existing_source_keys()
        changed = False
        density_edges: set[int] = set()

        cluster_sids: dict[int, list[int]] = defaultdict(list)
        if integer_labels:
            for sid, label in integer_labels.items():
                cluster_sids[int(label)].append(int(sid))
        else:
            cluster_sids[SOLO_CLUSTER_LABEL] = sorted(int(sid) for sid in statement_ids)

        for cluster_label, sentence_ids in sorted(cluster_sids.items()):
            source_segment = {
                "day": day,
                "segment_key": segment_key,
                "stream": stream,
                "source": source,
                "cluster_label": int(cluster_label),
            }
            source_key = _source_key(source_segment)
            if source_key in existing_source_keys:
                continue

            cluster_rows: list[tuple[int, np.ndarray, float]] = []
            for sid in sentence_ids:
                idx = sid_to_idx.get(sid)
                if idx is None:
                    continue
                normalized = normalize_embedding(embeddings[idx])
                if normalized is None:
                    continue
                duration_s = (
                    float(durations_s[idx])
                    if durations_s is not None and idx < len(durations_s)
                    else 0.0
                )
                cluster_rows.append((int(sid), normalized, duration_s))

            if not cluster_rows:
                continue

            if cluster_label == SOLO_CLUSTER_LABEL:
                cluster_rows, centroid, trimmed_count = trim_solo_cluster_rows(
                    cluster_rows,
                    embedding_for_row=lambda row: row[1],
                    normalize_embedding=normalize_embedding,
                )
                if centroid is None or not cluster_rows:
                    continue
                stacked = np.stack([embedding for _sid, embedding, _ in cluster_rows])
                source_segment["trimmed_count"] = trimmed_count
            else:
                stacked = np.stack([embedding for _sid, embedding, _ in cluster_rows])
                centroid = normalize_embedding(np.mean(stacked, axis=0))
                if centroid is None:
                    continue

            spread = float(np.mean(1.0 - stacked @ centroid))
            if spread >= STABILITY_THRESHOLD:
                continue

            source_segment["sentence_ids"] = sorted(
                sid for sid, _embedding, _ in cluster_rows
            )
            n_intervals = len(cluster_rows)
            duration_s = sum(duration for _sid, _embedding, duration in cluster_rows)
            best_id, best_score = self._best_match(centroid)
            if best_id is not None and best_score >= MERGE_THRESHOLD:
                pre_intervals = self._candidates[best_id].n_intervals
                self._merge_candidate(
                    self._candidates[best_id],
                    centroid,
                    n_intervals,
                    duration_s,
                    source_segment,
                    normalize_embedding,
                )
                candidate = self._candidates[best_id]
                if (
                    pre_intervals < CONSOLIDATE_MIN_INTERVALS
                    and candidate.n_intervals >= CONSOLIDATE_MIN_INTERVALS
                ):
                    density_edges.add(best_id)
                existing_source_keys.add(source_key)
                changed = True
            elif best_id is None or best_score < SPLIT_THRESHOLD:
                cand_id = self._next_id
                self._create_candidate(
                    centroid,
                    n_intervals,
                    duration_s,
                    source_segment,
                )
                candidate = self._candidates[cand_id]
                if candidate.n_intervals >= CONSOLIDATE_MIN_INTERVALS:
                    density_edges.add(candidate.cand_id)
                existing_source_keys.add(source_key)
                changed = True
            else:
                candidate = self._create_candidate(
                    centroid,
                    n_intervals,
                    duration_s,
                    source_segment,
                )
                if candidate.n_intervals >= CONSOLIDATE_MIN_INTERVALS:
                    density_edges.add(candidate.cand_id)
                existing_source_keys.add(source_key)
                changed = True

        if changed:
            self.save()
            if density_edges:
                try:
                    self.consolidate_dense_candidates()
                except LockTimeout as exc:
                    logger.warning(
                        "speaker candidate consolidation skipped after density edge: %s",
                        exc,
                    )

    def confirmation_queue(self) -> list[CandidateProfile]:
        return [
            candidate
            for candidate in self._candidates.values()
            if candidate.ready_for_confirmation()
        ]

    def confirm(self, cand_id: int, entity_id: str) -> None:
        candidate = self._candidates[int(cand_id)]
        candidate.status = "confirmed"
        candidate.confirmed_entity = entity_id
        self.save()

    def reject(self, cand_id: int) -> None:
        candidate = self._candidates[int(cand_id)]
        candidate.status = "rejected"
        candidate.confirmed_entity = None
        self.save()

    def restore_confirmed_candidate(
        self,
        cand_id: int,
        *,
        expected_after: dict[str, Any],
        candidate_before: dict[str, Any],
    ) -> dict[str, Any]:
        """Compare-restore a candidate confirmed by identify undo."""
        report = {
            "restored_count": 0,
            "skipped_count": 0,
            "skipped_reasons": {
                "missing": 0,
                "already_restored": 0,
                "concurrent_change": 0,
            },
        }
        with hold_lock(self.store_path):
            self.load()
            candidate = self._candidates.get(int(cand_id))
            if candidate is None:
                report["skipped_count"] += 1
                report["skipped_reasons"]["missing"] += 1
                return report
            current = candidate.to_json()
            if current == candidate_before:
                report["skipped_count"] += 1
                report["skipped_reasons"]["already_restored"] += 1
                return report
            if current != expected_after:
                report["skipped_count"] += 1
                report["skipped_reasons"]["concurrent_change"] += 1
                return report
            candidate.status = str(candidate_before.get("status", "pending"))
            confirmed_entity = candidate_before.get("confirmed_entity")
            candidate.confirmed_entity = (
                str(confirmed_entity) if confirmed_entity is not None else None
            )
            self._write()
            report["restored_count"] = 1
            return report

    def plan_retroactive_confirm(
        self,
        centroid: np.ndarray,
        entity_id: str,
    ) -> RetroactiveConfirmPlan:
        """Plan retroactive confirmation without mutating tracker or voiceprints."""
        normalized_centroid = normalize_embedding(centroid)
        if normalized_centroid is None:
            return _empty_retroactive_plan(entity_id)

        cand_id, score = self._best_match(normalized_centroid)
        if cand_id is None or score < MERGE_THRESHOLD:
            return _empty_retroactive_plan(entity_id, match_score=score)

        candidate = self._candidates[cand_id]
        candidate_before = candidate.to_json()
        candidate_after = dict(candidate_before)
        candidate_after["status"] = "confirmed"
        candidate_after["confirmed_entity"] = entity_id

        preexisting_keys, existing_count, existing_centroid = (
            _entity_voiceprint_snapshot(entity_id, normalize_embedding)
        )
        working_keys = set(preexisting_keys)
        voiceprint_items: list[tuple[np.ndarray, dict[str, Any]]] = []
        voiceprints_to_add: list[dict[str, Any]] = []

        owner_context = _retroactive_owner_context()
        if owner_context is None or _is_principal_entity(entity_id):
            return RetroactiveConfirmPlan(
                matched=True,
                match_score=score,
                candidate_id=cand_id,
                entity_id=entity_id,
                candidate_before=candidate_before,
                candidate_after=candidate_after,
                preexisting_voiceprint_keys=tuple(sorted(preexisting_keys)),
                voiceprints_to_add=(),
                voiceprint_items_to_add=(),
            )
        owner_centroid, owner_threshold = owner_context

        for source_segment in candidate.source_segments:
            day = str(source_segment["day"])
            segment_key = str(source_segment["segment_key"])
            stream = str(source_segment["stream"])
            source = str(source_segment["source"])

            seg_dir = segment_path(day, segment_key, stream, create=False)
            if not seg_dir.exists():
                continue
            if _retroactive_segment_noisy(seg_dir, source):
                continue
            emb_data = load_embeddings_file(seg_dir / f"{source}.npz")
            if emb_data is None:
                continue
            embeddings, statement_ids, _durations_s = emb_data
            sid_to_idx = {int(sid): index for index, sid in enumerate(statement_ids)}
            sentence_ids = source_segment_sentence_ids(source_segment)
            if sentence_ids is None:
                continue

            for sid in sentence_ids:
                idx = sid_to_idx.get(int(sid))
                if idx is None:
                    continue
                normalized = normalize_embedding(embeddings[idx])
                if normalized is None:
                    continue
                if float(np.dot(normalized, owner_centroid)) >= owner_threshold:
                    continue
                key = (day, segment_key, source, int(sid))
                if key in working_keys:
                    continue
                if (
                    existing_count >= _retroactive_outlier_min_samples()
                    and existing_centroid is not None
                    and float(np.dot(normalized, existing_centroid))
                    < _retroactive_outlier_min_similarity()
                ):
                    continue
                metadata = _retroactive_voiceprint_metadata(
                    day,
                    stream,
                    segment_key,
                    source,
                    int(sid),
                )
                voiceprint_items.append((normalized, metadata))
                voiceprints_to_add.append(
                    {
                        "key": {
                            "day": day,
                            "segment_key": segment_key,
                            "source": source,
                            "sentence_id": int(sid),
                        },
                        "metadata": metadata,
                    }
                )
                working_keys.add(key)

        return RetroactiveConfirmPlan(
            matched=True,
            match_score=score,
            candidate_id=cand_id,
            entity_id=entity_id,
            candidate_before=candidate_before,
            candidate_after=candidate_after,
            preexisting_voiceprint_keys=tuple(sorted(preexisting_keys)),
            voiceprints_to_add=tuple(voiceprints_to_add),
            voiceprint_items_to_add=tuple(voiceprint_items),
        )
