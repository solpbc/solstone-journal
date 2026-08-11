# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the headless speaker candidate tracker."""

from __future__ import annotations

import json
import threading
from pathlib import Path

import numpy as np

from solstone.apps.speakers.candidate_tracker import (
    SOLO_CLUSTER_LABEL,
    CandidateProfile,
    CandidateTracker,
    candidate_source_anchors,
    canonical_candidate_anchor,
    encode_source_key,
    source_segment_anchor,
    source_segment_sentence_ids,
)
from solstone.apps.speakers.encoder_config import (
    CONFIRM_MIN_DURATION_S,
    CONFIRM_MIN_INTERVALS,
    CONFIRM_MIN_SEGMENTS,
    CONSOLIDATE_MERGE_THRESHOLD,
    CONSOLIDATE_MIN_INTERVALS,
    CONSOLIDATE_SUGGEST_MIN,
    ENCODER_ID,
    MERGE_THRESHOLD,
    SOLO_CLUSTER_MIN_COSINE,
    SPEAKER_EVIDENCE_VERSION,
    SPLIT_THRESHOLD,
    STABILITY_THRESHOLD,
    OWNER_THRESHOLD,
)
from solstone.apps.speakers.tests.conftest import journal_tree_hash

STREAM = "test"


def _unit(vector: list[float]) -> np.ndarray:
    emb = np.array(vector + [0.0] * (256 - len(vector)), dtype=np.float32)
    return emb / np.linalg.norm(emb)


def _setup_owner(env, name: str = "Self Person") -> tuple[Path, np.ndarray]:
    principal_dir = env.create_entity(name, is_principal=True)
    centroid = _unit([1.0, 0.0])
    np.savez_compressed(
        principal_dir / "owner_centroid.npz",
        centroid=centroid,
        cluster_size=np.array(70, dtype=np.int32),
        threshold=np.array(OWNER_THRESHOLD, dtype=np.float32),
        last_refreshed_at=np.array("2026-03-15T12:00:00Z"),
    )
    return principal_dir, centroid


def _write_labeled_segment(
    env,
    day: str,
    segment_key: str,
    clusters: dict[int, np.ndarray],
    *,
    stream: str = STREAM,
    source: str = "mic_audio",
    duration_s: float = 5.0,
    speaker_evidence: str | None = None,
    speaker_evidence_multi_fraction: float = 0.0,
    write_speaker_labels: bool = True,
    include_durations: bool = True,
) -> Path:
    flat_dir, chronicle_dir = env._segment_dirs(day, segment_key, stream=stream)
    embeddings: list[np.ndarray] = []
    statement_ids: list[int] = []
    durations: list[float] = []
    labels: list[int] = []
    sid = 1
    for cluster_label, cluster_embeddings in clusters.items():
        for embedding in cluster_embeddings:
            embeddings.append(embedding)
            statement_ids.append(sid)
            durations.append(duration_s)
            labels.append(cluster_label)
            sid += 1

    header = {"raw": f"{source}.flac", "model": "test"}
    if speaker_evidence is not None:
        header.update(
            {
                "speaker_evidence": speaker_evidence,
                "speaker_evidence_multi_fraction": speaker_evidence_multi_fraction,
                "speaker_evidence_version": SPEAKER_EVIDENCE_VERSION,
            }
        )
    lines = [json.dumps(header)]
    for sid, cluster_label in zip(statement_ids, labels):
        row = {
            "start": "09:00:00",
            "text": f"sentence {sid}",
        }
        if write_speaker_labels:
            row["speaker"] = int(cluster_label)
        lines.append(json.dumps(row))

    for seg_dir in (flat_dir, chronicle_dir):
        (seg_dir / f"{source}.jsonl").write_text(
            "\n".join(lines) + "\n",
            encoding="utf-8",
        )
        npz_payload = {
            "embeddings": np.stack(embeddings).astype(np.float32),
            "statement_ids": np.array(statement_ids, dtype=np.int32),
            "encoder": np.array(ENCODER_ID),
        }
        if include_durations:
            npz_payload["durations_s"] = np.array(durations, dtype=np.float32)
        np.savez_compressed(seg_dir / f"{source}.npz", **npz_payload)
        (seg_dir / f"{source}.flac").write_bytes(b"")
    return chronicle_dir


def _rewrite_integer_speaker_labels(
    seg_dir: Path,
    source: str,
    by_sentence_id: dict[int, int],
) -> None:
    jsonl_path = seg_dir / f"{source}.jsonl"
    lines = jsonl_path.read_text(encoding="utf-8").splitlines()
    rewritten = [lines[0]]
    for sentence_id, line in enumerate(lines[1:], start=1):
        row = json.loads(line)
        row["speaker"] = by_sentence_id[sentence_id]
        rewritten.append(json.dumps(row))
    jsonl_path.write_text("\n".join(rewritten) + "\n", encoding="utf-8")


def _voiceprint_count(entity_dir: Path) -> int:
    with np.load(entity_dir / "voiceprints.npz", allow_pickle=False) as data:
        return len(data["embeddings"])


def _voiceprint_entries(entity_dir: Path) -> list[dict[str, object]]:
    with np.load(entity_dir / "voiceprints.npz", allow_pickle=False) as data:
        return [json.loads(str(item)) for item in data["metadata"]]


def _only_candidate(tracker: CandidateTracker):
    assert len(tracker._candidates) == 1
    return next(iter(tracker._candidates.values()))


def _source_segment(day: str, cluster_label: int = 1) -> dict[str, object]:
    return {
        "day": day,
        "segment_key": "090000_300",
        "stream": STREAM,
        "source": "mic_audio",
        "cluster_label": cluster_label,
    }


def _profile(
    cand_id: int,
    centroid: np.ndarray,
    *,
    n_intervals: int = CONSOLIDATE_MIN_INTERVALS,
    status: str = "pending",
    confirmed_entity: str | None = None,
    source_segments: list[dict[str, object]] | None = None,
    merge_events: list[dict[str, object]] | None = None,
) -> CandidateProfile:
    return CandidateProfile(
        cand_id=cand_id,
        centroid=centroid,
        n_segments=len(
            {
                (
                    str(source_segment["day"]),
                    str(source_segment["segment_key"]),
                    str(source_segment["stream"]),
                    str(source_segment["source"]),
                )
                for source_segment in (
                    source_segments or [_source_segment(f"2026010{cand_id}")]
                )
            }
        ),
        n_intervals=n_intervals,
        total_duration_s=float(n_intervals),
        source_segments=list(source_segments or [_source_segment(f"2026010{cand_id}")]),
        confirmed_entity=confirmed_entity,
        status=status,
        merge_events=list(merge_events or []),
    )


def _seed_tracker(store: Path, candidates: list[CandidateProfile]) -> CandidateTracker:
    tracker = CandidateTracker(store)
    tracker._candidates = {candidate.cand_id: candidate for candidate in candidates}
    tracker._next_id = (
        max((candidate.cand_id for candidate in candidates), default=0) + 1
    )
    tracker.save()
    return CandidateTracker(store)


def _all_source_anchors(tracker: CandidateTracker) -> list[str]:
    return [
        encode_source_key(tuple_key)
        for tuple_key in sorted(
            tracker._existing_source_keys(),
            key=lambda item: encode_source_key(item),
        )
    ]


def test_tracker_constants_locked():
    assert MERGE_THRESHOLD == 0.72
    assert SPLIT_THRESHOLD == 0.55
    assert STABILITY_THRESHOLD == 0.25
    assert CONSOLIDATE_MIN_INTERVALS == 30
    assert CONSOLIDATE_MERGE_THRESHOLD == 0.65
    assert CONSOLIDATE_SUGGEST_MIN == 0.45
    assert SOLO_CLUSTER_MIN_COSINE == 0.43
    assert CONFIRM_MIN_SEGMENTS == 2
    assert CONFIRM_MIN_INTERVALS == 5
    assert CONFIRM_MIN_DURATION_S == 25.0


def test_solo_segment_admits_with_trimmed_count_zero(speakers_env, tmp_path):
    env = speakers_env()
    base = _unit([0.0, 1.0])
    seg_dir = _write_labeled_segment(
        env,
        "20260101",
        "090000_300",
        {1: np.stack([base] * 3)},
        speaker_evidence="single",
        write_speaker_labels=False,
    )

    tracker = CandidateTracker(tmp_path / "speaker_candidates.json")
    tracker.process_segment("20260101", "090000_300", STREAM, "mic_audio", seg_dir)

    candidate = _only_candidate(tracker)
    assert candidate.n_intervals == 3
    assert candidate.total_duration_s == 15.0
    assert candidate.source_segments == [
        {
            "day": "20260101",
            "segment_key": "090000_300",
            "stream": STREAM,
            "source": "mic_audio",
            "cluster_label": SOLO_CLUSTER_LABEL,
            "trimmed_count": 0,
            "sentence_ids": [1, 2, 3],
        }
    ]


def test_solo_trim_drops_skewed_contamination_and_counts_survivors(
    speakers_env, tmp_path
):
    env = speakers_env()
    voice_a = _unit([0.0, 1.0])
    voice_b = _unit([0.0, 0.0, 1.0])
    mixed = np.stack([voice_a] * 27 + [voice_b] * 3)
    centroid = mixed.mean(axis=0)
    centroid = centroid / np.linalg.norm(centroid)
    assert float(np.dot(voice_b, centroid)) < SOLO_CLUSTER_MIN_COSINE
    assert float(np.dot(voice_a, centroid)) > SOLO_CLUSTER_MIN_COSINE
    seg_dir = _write_labeled_segment(
        env,
        "20260101",
        "090000_300",
        {1: mixed},
        speaker_evidence="single",
        write_speaker_labels=False,
    )

    tracker = CandidateTracker(tmp_path / "speaker_candidates.json")
    tracker.process_segment("20260101", "090000_300", STREAM, "mic_audio", seg_dir)

    candidate = _only_candidate(tracker)
    assert candidate.n_intervals == 27
    assert candidate.total_duration_s == 135.0
    assert candidate.source_segments[0]["trimmed_count"] == 3


def test_solo_without_durations_fails_confirmation_closed(speakers_env, tmp_path):
    env = speakers_env()
    store = tmp_path / "speaker_candidates.json"
    base = _unit([0.0, 1.0])
    for day, stream in (("20260101", "mic"), ("20260102", "sys")):
        seg_dir = _write_labeled_segment(
            env,
            day,
            "090000_300",
            {1: np.stack([base] * 18)},
            stream=stream,
            speaker_evidence="single",
            write_speaker_labels=False,
            include_durations=False,
        )
        tracker = CandidateTracker(store)
        tracker.process_segment(day, "090000_300", stream, "mic_audio", seg_dir)

    candidate = _only_candidate(CandidateTracker(store))
    assert candidate.n_segments == 2
    assert candidate.n_intervals == 36
    assert candidate.total_duration_s == 0.0
    assert candidate.ready_for_confirmation() is False


def test_solo_process_segment_idempotent_for_same_source_segment(
    speakers_env, tmp_path
):
    env = speakers_env()
    base = _unit([0.0, 1.0])
    seg_dir = _write_labeled_segment(
        env,
        "20260101",
        "090000_300",
        {1: np.stack([base] * 3)},
        speaker_evidence="single",
        write_speaker_labels=False,
    )
    tracker = CandidateTracker(tmp_path / "speaker_candidates.json")
    tracker.process_segment("20260101", "090000_300", STREAM, "mic_audio", seg_dir)
    source_keys = tracker._existing_source_keys()
    assert source_keys == {
        ("20260101", "090000_300", STREAM, "mic_audio", SOLO_CLUSTER_LABEL)
    }

    tracker.process_segment("20260101", "090000_300", STREAM, "mic_audio", seg_dir)

    candidate = _only_candidate(tracker)
    assert tracker._existing_source_keys() == source_keys
    assert candidate.n_segments == 1
    assert candidate.n_intervals == 3
    assert candidate.total_duration_s == 15.0
    assert len(candidate.source_segments) == 1


def test_solo_then_diarizer_sequence_keeps_ingesting_without_warning(
    speakers_env, tmp_path, caplog
):
    import logging

    env = speakers_env()
    store = tmp_path / "speaker_candidates.json"
    solo = _unit([0.0, 1.0])
    diarized = _unit([0.0, 0.0, 1.0])
    solo_dir = _write_labeled_segment(
        env,
        "20260101",
        "090000_300",
        {1: np.stack([solo] * 3)},
        speaker_evidence="single",
        write_speaker_labels=False,
    )
    diarizer_dir = _write_labeled_segment(
        env,
        "20260101",
        "091000_300",
        {1: np.stack([diarized] * 3)},
    )
    tracker = CandidateTracker(store)

    with caplog.at_level(logging.WARNING):
        for segment_key, seg_dir in (
            ("090000_300", solo_dir),
            ("091000_300", diarizer_dir),
        ):
            try:
                tracker.process_segment(
                    "20260101", segment_key, STREAM, "mic_audio", seg_dir
                )
            except Exception:
                logging.warning("Speaker candidate tracking failed", exc_info=True)

    assert [
        record for record in caplog.records if record.levelno >= logging.WARNING
    ] == []
    assert len(tracker._candidates) == 2
    assert any(
        source_segment["cluster_label"] == SOLO_CLUSTER_LABEL
        for candidate in tracker._candidates.values()
        for source_segment in candidate.source_segments
    )


def test_solo_evidence_multi_not_admitted(speakers_env, tmp_path):
    env = speakers_env()
    base = _unit([0.0, 1.0])
    seg_dir = _write_labeled_segment(
        env,
        "20260101",
        "090000_300",
        {1: np.stack([base] * 3)},
        speaker_evidence="multi",
        speaker_evidence_multi_fraction=0.2,
        write_speaker_labels=False,
    )

    tracker = CandidateTracker(tmp_path / "speaker_candidates.json")
    tracker.process_segment("20260101", "090000_300", STREAM, "mic_audio", seg_dir)

    assert tracker._candidates == {}


def test_solo_evidence_none_not_admitted(speakers_env, tmp_path):
    env = speakers_env()
    base = _unit([0.0, 1.0])
    seg_dir = _write_labeled_segment(
        env,
        "20260101",
        "090000_300",
        {1: np.stack([base] * 3)},
        speaker_evidence="none",
        write_speaker_labels=False,
    )

    tracker = CandidateTracker(tmp_path / "speaker_candidates.json")
    tracker.process_segment("20260101", "090000_300", STREAM, "mic_audio", seg_dir)

    assert tracker._candidates == {}


def test_solo_evidence_absent_not_admitted(speakers_env, tmp_path):
    env = speakers_env()
    base = _unit([0.0, 1.0])
    seg_dir = _write_labeled_segment(
        env,
        "20260101",
        "090000_300",
        {1: np.stack([base] * 3)},
        write_speaker_labels=False,
    )

    tracker = CandidateTracker(tmp_path / "speaker_candidates.json")
    tracker.process_segment("20260101", "090000_300", STREAM, "mic_audio", seg_dir)

    assert tracker._candidates == {}


def test_solo_evidence_unreadable_not_admitted(speakers_env, tmp_path):
    env = speakers_env()
    base = _unit([0.0, 1.0])
    seg_dir = _write_labeled_segment(
        env,
        "20260101",
        "090000_300",
        {1: np.stack([base] * 3)},
        speaker_evidence="single",
        write_speaker_labels=False,
    )
    jsonl_path = seg_dir / "mic_audio.jsonl"
    jsonl_path.unlink()
    jsonl_path.mkdir()

    tracker = CandidateTracker(tmp_path / "speaker_candidates.json")
    tracker.process_segment("20260101", "090000_300", STREAM, "mic_audio", seg_dir)

    assert tracker._candidates == {}


def test_pool_persist_reload_round_trip(speakers_env, tmp_path):
    env = speakers_env()
    seg_dir = _write_labeled_segment(
        env,
        "20260101",
        "090000_300",
        {1: np.stack([_unit([0.0, 1.0])] * 3)},
    )
    store = tmp_path / "speaker_candidates.json"

    tracker = CandidateTracker(store)
    tracker.process_segment("20260101", "090000_300", STREAM, "mic_audio", seg_dir)

    reloaded = CandidateTracker(store)
    candidate = _only_candidate(reloaded)
    assert candidate.cand_id == 1
    assert candidate.n_segments == 1
    assert candidate.n_intervals == 3
    assert candidate.total_duration_s == 15.0
    assert candidate.source_segments == [
        {
            "day": "20260101",
            "segment_key": "090000_300",
            "stream": STREAM,
            "source": "mic_audio",
            "cluster_label": 1,
            "sentence_ids": [1, 2, 3],
        }
    ]


def test_load_all_candidates_is_read_only(speakers_env, tmp_path):
    env = speakers_env()
    seg_dir = _write_labeled_segment(
        env,
        "20260101",
        "090000_300",
        {1: np.stack([_unit([0.0, 1.0])] * 3)},
    )
    store = tmp_path / "speaker_candidates.json"
    tracker = CandidateTracker(store)
    tracker.process_segment("20260101", "090000_300", STREAM, "mic_audio", seg_dir)
    before = store.read_text(encoding="utf-8")

    candidates = CandidateTracker(store).load_all_candidates()

    assert [candidate.cand_id for candidate in candidates] == [1]
    assert store.read_text(encoding="utf-8") == before


def test_merge_threshold_updates_existing_candidate(speakers_env, tmp_path):
    env = speakers_env()
    store = tmp_path / "speaker_candidates.json"
    base = _unit([0.0, 1.0])

    for day in ("20260101", "20260102"):
        seg_dir = _write_labeled_segment(
            env,
            day,
            "090000_300",
            {1: np.stack([base] * 3)},
        )
        tracker = CandidateTracker(store)
        tracker.process_segment(day, "090000_300", STREAM, "mic_audio", seg_dir)

    tracker = CandidateTracker(store)
    candidate = _only_candidate(tracker)
    assert candidate.n_segments == 2
    assert candidate.n_intervals == 6
    assert candidate.total_duration_s == 30.0


def test_split_threshold_creates_distinct_candidate(speakers_env, tmp_path):
    env = speakers_env()
    store = tmp_path / "speaker_candidates.json"
    bases = [_unit([0.0, 1.0]), _unit([0.0, 0.0, 1.0])]
    assert float(np.dot(bases[0], bases[1])) < SPLIT_THRESHOLD

    for i, base in enumerate(bases, start=1):
        day = f"2026010{i}"
        seg_dir = _write_labeled_segment(
            env,
            day,
            "090000_300",
            {1: np.stack([base] * 3)},
        )
        tracker = CandidateTracker(store)
        tracker.process_segment(day, "090000_300", STREAM, "mic_audio", seg_dir)

    tracker = CandidateTracker(store)
    assert len(tracker._candidates) == 2


def test_dead_zone_creates_distinct_candidate_without_growing_existing(
    speakers_env, tmp_path
):
    env = speakers_env()
    store = tmp_path / "speaker_candidates.json"
    base = _unit([0.0, 1.0])
    target = (MERGE_THRESHOLD + SPLIT_THRESHOLD) / 2
    ambiguous = _unit([0.0, target, np.sqrt(1.0 - target**2)])
    assert SPLIT_THRESHOLD < float(np.dot(base, ambiguous)) < MERGE_THRESHOLD

    seg_dir = _write_labeled_segment(
        env,
        "20260101",
        "090000_300",
        {1: np.stack([base] * 3)},
    )
    tracker = CandidateTracker(store)
    tracker.process_segment("20260101", "090000_300", STREAM, "mic_audio", seg_dir)
    original = _only_candidate(CandidateTracker(store))
    original_sources = list(original.source_segments)

    seg_dir = _write_labeled_segment(
        env,
        "20260102",
        "090000_300",
        {1: np.stack([ambiguous] * 3)},
    )
    tracker = CandidateTracker(store)
    tracker.process_segment("20260102", "090000_300", STREAM, "mic_audio", seg_dir)

    tracker = CandidateTracker(store)
    assert len(tracker._candidates) == 2
    assert tracker._candidates[original.cand_id].source_segments == original_sources
    assert sorted(
        candidate.n_intervals for candidate in tracker._candidates.values()
    ) == [
        3,
        3,
    ]


def test_consolidate_dense_candidates_chained_trio_recomputes_centroid_to_fixpoint(
    tmp_path,
):
    store = tmp_path / "speaker_candidates.json"
    a = _unit([1.0, 0.0, 0.0])
    b = _unit([0.70, np.sqrt(1.0 - 0.70**2), 0.0])
    c_y = (0.66 - 0.70 * 0.30) / np.sqrt(1.0 - 0.70**2)
    c = _unit([0.30, c_y, np.sqrt(1.0 - 0.30**2 - c_y**2)])
    assert np.isclose(float(np.dot(a, b)), 0.70)
    assert np.isclose(float(np.dot(b, c)), 0.66)
    assert np.isclose(float(np.dot(a, c)), 0.30)

    _seed_tracker(
        store,
        [
            _profile(1, a),
            _profile(2, b),
            _profile(3, c),
        ],
    )

    result = CandidateTracker(store).consolidate_dense_candidates()

    assert result["merged"] == 1
    tracker = CandidateTracker(store)
    assert sorted(tracker._candidates) == [1, 3]
    survivor = tracker._candidates[1]
    assert np.isclose(float(np.dot(survivor.centroid, c)), 0.520633, atol=1e-5)
    assert float(np.dot(survivor.centroid, c)) < CONSOLIDATE_MERGE_THRESHOLD
    assert survivor.merge_events[-1]["survivor_id"] == 1
    assert survivor.merge_events[-1]["absorbed_id"] == 2


def test_consolidate_dense_candidates_fixpoint_merges_newly_eligible_pair(tmp_path):
    store = tmp_path / "speaker_candidates.json"
    ab = 0.66
    ac = 0.64
    bc = 0.64
    a = _unit([1.0, 0.0, 0.0])
    b = _unit([ab, np.sqrt(1.0 - ab**2), 0.0])
    c_y = (bc - ab * ac) / np.sqrt(1.0 - ab**2)
    c = _unit([ac, c_y, np.sqrt(1.0 - ac**2 - c_y**2)])
    assert float(np.dot(a, b)) >= CONSOLIDATE_MERGE_THRESHOLD
    assert float(np.dot(a, c)) < CONSOLIDATE_MERGE_THRESHOLD
    assert float(np.dot(b, c)) < CONSOLIDATE_MERGE_THRESHOLD

    _seed_tracker(store, [_profile(1, a), _profile(2, b), _profile(3, c)])

    result = CandidateTracker(store).consolidate_dense_candidates()

    assert result["merged"] == 2
    tracker = CandidateTracker(store)
    survivor = _only_candidate(tracker)
    assert survivor.cand_id == 1
    assert survivor.n_intervals == CONSOLIDATE_MIN_INTERVALS * 3
    assert len(_all_source_anchors(tracker)) == 3


def test_consolidate_dense_candidates_positive_then_idempotent_no_churn(tmp_path):
    store = tmp_path / "speaker_candidates.json"
    a = _unit([1.0, 0.0])
    b = _unit([0.70, np.sqrt(1.0 - 0.70**2)])
    _seed_tracker(store, [_profile(1, a), _profile(2, b)])

    first = CandidateTracker(store).consolidate_dense_candidates()
    after_first = store.read_text(encoding="utf-8")
    second = CandidateTracker(store).consolidate_dense_candidates()

    assert first["merged"] == 1
    assert second["merged"] == 0
    assert store.read_text(encoding="utf-8") == after_first


def test_consolidate_dense_candidates_excludes_rejected_and_confirmed(tmp_path):
    store = tmp_path / "speaker_candidates.json"
    a = _unit([1.0, 0.0])
    b = _unit([0.0, 1.0])
    _seed_tracker(
        store,
        [
            _profile(1, a, status="rejected"),
            _profile(2, a),
            _profile(3, b),
            _profile(4, b, status="confirmed", confirmed_entity="alice_test"),
        ],
    )

    result = CandidateTracker(store).consolidate_dense_candidates()

    assert result["merged"] == 0
    tracker = CandidateTracker(store)
    assert sorted(tracker._candidates) == [1, 2, 3, 4]


def test_manual_merge_pending_pending_matches_auto_semantics(tmp_path):
    a = _unit([1.0, 0.0])
    b = _unit([0.70, np.sqrt(1.0 - 0.70**2)])
    auto_store = tmp_path / "auto.json"
    manual_store = tmp_path / "manual.json"
    _seed_tracker(auto_store, [_profile(1, a), _profile(2, b)])
    manual = _seed_tracker(manual_store, [_profile(1, a), _profile(2, b)])
    anchors = [
        canonical_candidate_anchor(candidate)
        for candidate in manual.load_all_candidates()
    ]

    auto_result = CandidateTracker(auto_store).consolidate_dense_candidates()
    manual_result = CandidateTracker(manual_store).merge_candidate_pair(
        anchors[0], anchors[1]
    )

    assert auto_result["merged"] == 1
    assert manual_result["status"] == "merged"
    auto_candidate = _only_candidate(CandidateTracker(auto_store))
    manual_candidate = _only_candidate(CandidateTracker(manual_store))
    assert manual_candidate.cand_id == auto_candidate.cand_id == 1
    assert manual_candidate.status == auto_candidate.status == "pending"
    assert manual_candidate.confirmed_entity is None
    assert manual_candidate.n_intervals == auto_candidate.n_intervals
    assert candidate_source_anchors(manual_candidate) == candidate_source_anchors(
        auto_candidate
    )


def test_manual_merge_pending_confirmed_confirms_lowest_id_survivor(tmp_path):
    store = tmp_path / "speaker_candidates.json"
    a = _unit([1.0, 0.0])
    b = _unit([0.70, np.sqrt(1.0 - 0.70**2)])
    tracker = _seed_tracker(
        store,
        [
            _profile(1, a),
            _profile(2, b, status="confirmed", confirmed_entity="alice_test"),
        ],
    )
    anchors = [
        canonical_candidate_anchor(candidate)
        for candidate in tracker.load_all_candidates()
    ]

    result = CandidateTracker(store).merge_candidate_pair(anchors[0], anchors[1])

    assert result["status"] == "merged"
    survivor = _only_candidate(CandidateTracker(store))
    assert survivor.cand_id == 1
    assert survivor.status == "confirmed"
    assert survivor.confirmed_entity == "alice_test"
    assert survivor.merge_events[-1]["absorbed_id"] == 2


def test_manual_merge_refuses_rejected_and_two_confirmed(tmp_path):
    a = _unit([1.0, 0.0])
    b = _unit([0.70, np.sqrt(1.0 - 0.70**2)])
    rejected_store = tmp_path / "rejected.json"
    confirmed_store = tmp_path / "confirmed.json"
    rejected = _seed_tracker(
        rejected_store,
        [_profile(1, a, status="rejected"), _profile(2, b)],
    )
    confirmed = _seed_tracker(
        confirmed_store,
        [
            _profile(1, a, status="confirmed", confirmed_entity="alice_test"),
            _profile(2, b, status="confirmed", confirmed_entity="bob_test"),
        ],
    )

    rejected_anchors = [
        canonical_candidate_anchor(candidate)
        for candidate in rejected.load_all_candidates()
    ]
    confirmed_anchors = [
        canonical_candidate_anchor(candidate)
        for candidate in confirmed.load_all_candidates()
    ]

    assert (
        CandidateTracker(rejected_store).merge_candidate_pair(
            rejected_anchors[0], rejected_anchors[1]
        )["error"]
        == "cannot merge rejected candidate"
    )
    assert (
        CandidateTracker(confirmed_store).merge_candidate_pair(
            confirmed_anchors[0], confirmed_anchors[1]
        )["error"]
        == "cannot merge two confirmed candidates"
    )
    assert len(CandidateTracker(rejected_store)._candidates) == 2
    assert len(CandidateTracker(confirmed_store)._candidates) == 2


def test_consolidate_dense_candidates_serializes_threads_no_duplicate_sources(
    tmp_path,
):
    store = tmp_path / "speaker_candidates.json"
    base = _unit([1.0, 0.0])
    _seed_tracker(store, [_profile(i, base) for i in range(1, 7)])
    barrier = threading.Barrier(4)
    errors: list[BaseException] = []

    def worker() -> None:
        try:
            barrier.wait()
            CandidateTracker(store).consolidate_dense_candidates()
        except BaseException as exc:  # pragma: no cover - assertion surface
            errors.append(exc)

    threads = [threading.Thread(target=worker) for _ in range(4)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()

    assert errors == []
    tracker = CandidateTracker(store)
    survivor = _only_candidate(tracker)
    source_anchors = list(candidate_source_anchors(survivor))
    assert len(source_anchors) == 6
    assert len(set(source_anchors)) == 6
    final_ids = set(tracker._candidates)
    for event in survivor.merge_events:
        assert event["survivor_id"] in final_ids
        assert event["absorbed_id"] not in final_ids


def test_merge_events_include_content_and_carry_forward(tmp_path):
    store = tmp_path / "speaker_candidates.json"
    previous_event = {
        "survivor_id": 2,
        "absorbed_id": 7,
        "score": 0.91,
        "merged_at": "2026-01-01T00:00:00Z",
        "absorbed_n_intervals": 9,
        "survivor_n_intervals_after": 39,
    }
    a = _unit([1.0, 0.0])
    b = _unit([0.70, np.sqrt(1.0 - 0.70**2)])
    _seed_tracker(
        store,
        [
            _profile(1, a),
            _profile(2, b, merge_events=[previous_event]),
        ],
    )

    result = CandidateTracker(store).consolidate_dense_candidates()

    assert result["merged"] == 1
    event = result["merges"][0]
    assert event["survivor_id"] == 1
    assert event["absorbed_id"] == 2
    assert np.isclose(event["score"], 0.70)
    assert event["absorbed_n_intervals"] == CONSOLIDATE_MIN_INTERVALS
    assert event["survivor_n_intervals_after"] == CONSOLIDATE_MIN_INTERVALS * 2
    survivor = _only_candidate(CandidateTracker(store))
    assert survivor.merge_events[0] == previous_event
    assert survivor.merge_events[-1]["absorbed_id"] == 2


def test_edge_trigger_fires_after_save_for_merge_and_create(
    speakers_env, tmp_path, monkeypatch
):
    env = speakers_env()
    calls: list[list[int]] = []

    def fake_consolidate(self):
        data = json.loads(self.store_path.read_text(encoding="utf-8"))
        calls.append([candidate["n_intervals"] for candidate in data["candidates"]])
        return {"status": "ok", "merged": 0, "merges": []}

    monkeypatch.setattr(
        CandidateTracker, "consolidate_dense_candidates", fake_consolidate
    )

    store = tmp_path / "merge_edge.json"
    base = _unit([0.0, 1.0])
    first_dir = _write_labeled_segment(
        env,
        "20260101",
        "090000_300",
        {1: np.stack([base] * (CONSOLIDATE_MIN_INTERVALS - 1))},
    )
    CandidateTracker(store).process_segment(
        "20260101", "090000_300", STREAM, "mic_audio", first_dir
    )
    assert calls == []

    second_dir = _write_labeled_segment(
        env,
        "20260102",
        "090000_300",
        {1: np.stack([base])},
    )
    CandidateTracker(store).process_segment(
        "20260102", "090000_300", STREAM, "mic_audio", second_dir
    )
    assert calls == [[CONSOLIDATE_MIN_INTERVALS]]

    create_store = tmp_path / "create_edge.json"
    create_dir = _write_labeled_segment(
        env,
        "20260103",
        "090000_300",
        {1: np.stack([base] * CONSOLIDATE_MIN_INTERVALS)},
    )
    CandidateTracker(create_store).process_segment(
        "20260103", "090000_300", STREAM, "mic_audio", create_dir
    )
    assert calls == [[CONSOLIDATE_MIN_INTERVALS], [CONSOLIDATE_MIN_INTERVALS]]


def test_pool_summary_defaults_roundtrip_and_noop_no_churn(tmp_path):
    store = tmp_path / "speaker_candidates.json"
    pool_without_summary = json.dumps({"next_id": 1, "candidates": []}, indent=2) + "\n"
    store.write_text(pool_without_summary, encoding="utf-8")

    tracker = CandidateTracker(store)
    assert tracker.consolidation_summary == {
        "merge_count_total": 0,
        "last_merge": None,
    }
    result = tracker.consolidate_dense_candidates()
    assert result["merged"] == 0
    assert store.read_text(encoding="utf-8") == pool_without_summary

    tracker.save()
    reloaded = CandidateTracker(store)
    assert reloaded.consolidation_summary == {
        "merge_count_total": 0,
        "last_merge": None,
    }
    assert "consolidation_summary" in json.loads(store.read_text(encoding="utf-8"))


def test_stability_rejects_incoherent_cluster(speakers_env, tmp_path):
    env = speakers_env()
    a = _unit([0.0, 1.0])
    b = _unit([0.0, 0.0, 1.0])
    unstable = np.stack([a, a, b, b])
    centroid = unstable.mean(axis=0)
    centroid = centroid / np.linalg.norm(centroid)
    spread = float(np.mean(1.0 - unstable @ centroid))
    assert spread >= STABILITY_THRESHOLD

    seg_dir = _write_labeled_segment(
        env,
        "20260101",
        "090000_300",
        {1: unstable},
    )
    tracker = CandidateTracker(tmp_path / "speaker_candidates.json")
    tracker.process_segment("20260101", "090000_300", STREAM, "mic_audio", seg_dir)

    assert tracker._candidates == {}


def test_confirmation_queue_maturity_gates(speakers_env, tmp_path):
    env = speakers_env()
    store = tmp_path / "speaker_candidates.json"
    base = _unit([0.0, 1.0])

    seg_dir = _write_labeled_segment(
        env,
        "20260101",
        "090000_300",
        {1: np.stack([base] * 3)},
    )
    tracker = CandidateTracker(store)
    tracker.process_segment("20260101", "090000_300", STREAM, "mic_audio", seg_dir)
    assert tracker.confirmation_queue() == []

    seg_dir = _write_labeled_segment(
        env,
        "20260102",
        "090000_300",
        {1: np.stack([base] * 3)},
    )
    tracker = CandidateTracker(store)
    tracker.process_segment("20260102", "090000_300", STREAM, "mic_audio", seg_dir)
    queue = tracker.confirmation_queue()

    assert len(queue) == 1
    candidate = queue[0]
    assert candidate.n_segments >= CONFIRM_MIN_SEGMENTS
    assert candidate.n_intervals >= CONFIRM_MIN_INTERVALS
    assert candidate.total_duration_s >= CONFIRM_MIN_DURATION_S


def test_confirm_and_reject_status_transitions(speakers_env, tmp_path):
    env = speakers_env()
    store = tmp_path / "speaker_candidates.json"
    seg_dir = _write_labeled_segment(
        env,
        "20260101",
        "090000_300",
        {
            1: np.stack([_unit([0.0, 1.0])] * 3),
            2: np.stack([_unit([0.0, 0.0, 1.0])] * 3),
        },
    )
    tracker = CandidateTracker(store)
    tracker.process_segment("20260101", "090000_300", STREAM, "mic_audio", seg_dir)

    cand_ids = sorted(tracker._candidates)
    tracker.confirm(cand_ids[0], "alice_test")
    tracker.reject(cand_ids[1])

    reloaded = CandidateTracker(store)
    assert reloaded._candidates[cand_ids[0]].status == "confirmed"
    assert reloaded._candidates[cand_ids[0]].confirmed_entity == "alice_test"
    assert reloaded._candidates[cand_ids[1]].status == "rejected"


def test_plan_retroactive_confirm_writes_nothing(speakers_env, tmp_path):
    env = speakers_env()
    _setup_owner(env)
    env.create_entity("Alice Test")
    base = _unit([0.0, 1.0])
    seg_dir = _write_labeled_segment(
        env,
        "20260101",
        "090000_300",
        {7: np.stack([base] * 3)},
    )
    tracker = CandidateTracker(tmp_path / "speaker_candidates.json")
    tracker.process_segment("20260101", "090000_300", STREAM, "mic_audio", seg_dir)
    before = journal_tree_hash(env.journal)

    plan = tracker.plan_retroactive_confirm(base, "alice_test")

    assert plan.matched is True
    assert plan.candidate_id == _only_candidate(tracker).cand_id
    assert len(plan.voiceprints_to_add) == 3
    assert journal_tree_hash(env.journal) == before
    candidate = _only_candidate(tracker)
    assert candidate.status == "pending"
    assert candidate.confirmed_entity is None


def test_process_segment_idempotent_for_same_source_segment(speakers_env, tmp_path):
    env = speakers_env()
    base = _unit([0.0, 1.0])
    seg_dir = _write_labeled_segment(
        env,
        "20260101",
        "090000_300",
        {1: np.stack([base] * 3)},
    )
    tracker = CandidateTracker(tmp_path / "speaker_candidates.json")
    tracker.process_segment("20260101", "090000_300", STREAM, "mic_audio", seg_dir)
    tracker.process_segment("20260101", "090000_300", STREAM, "mic_audio", seg_dir)

    candidate = _only_candidate(tracker)
    assert candidate.n_segments == 1
    assert candidate.n_intervals == 3
    assert candidate.total_duration_s == 15.0


def test_process_segment_persists_membership_without_changing_source_key(
    speakers_env,
    tmp_path,
):
    env = speakers_env()
    base = _unit([0.0, 1.0])
    seg_dir = _write_labeled_segment(
        env,
        "20260101",
        "090000_300",
        {7: np.stack([base] * 3)},
    )
    tracker = CandidateTracker(tmp_path / "speaker_candidates.json")
    tracker.process_segment("20260101", "090000_300", STREAM, "mic_audio", seg_dir)

    candidate = _only_candidate(tracker)
    source_segment = candidate.source_segments[0]
    anchor_with_membership = source_segment_anchor(source_segment)
    source_without_membership = dict(source_segment)
    source_without_membership.pop("sentence_ids")

    assert source_segment["sentence_ids"] == [1, 2, 3]
    assert source_segment_anchor(source_without_membership) == anchor_with_membership


def test_backfill_source_sentence_ids_recovers_legacy_membership(
    speakers_env,
    tmp_path,
):
    env = speakers_env()
    base = _unit([0.0, 1.0])
    seg_dir = _write_labeled_segment(
        env,
        "20260101",
        "090000_300",
        {7: np.stack([base] * 3)},
    )
    tracker = _seed_tracker(
        tmp_path / "speaker_candidates.json",
        [
            _profile(
                1,
                base,
                source_segments=[
                    {
                        "day": "20260101",
                        "segment_key": "090000_300",
                        "stream": STREAM,
                        "source": "mic_audio",
                        "cluster_label": 7,
                    },
                    {
                        "day": "20260102",
                        "segment_key": "missing_300",
                        "stream": STREAM,
                        "source": "mic_audio",
                        "cluster_label": 7,
                    },
                ],
            )
        ],
    )
    assert seg_dir.exists()

    result = tracker.backfill_source_sentence_ids()

    tracker.load()
    candidate = _only_candidate(tracker)
    assert result == {"updated": 1, "unresolved": 1, "already_present": 0}
    assert source_segment_sentence_ids(candidate.source_segments[0]) == [1, 2, 3]
    assert source_segment_sentence_ids(candidate.source_segments[1]) is None


def test_backfill_source_sentence_ids_trims_legacy_solo_membership(
    speakers_env,
    tmp_path,
):
    env = speakers_env()
    base = _unit([1.0, 0.0])
    outlier = _unit([0.0, 1.0])
    seg_dir = _write_labeled_segment(
        env,
        "20260101",
        "090000_300",
        {SOLO_CLUSTER_LABEL: np.stack([base, base, base, outlier])},
        speaker_evidence="single",
        write_speaker_labels=False,
    )
    tracker = _seed_tracker(
        tmp_path / "speaker_candidates.json",
        [
            _profile(
                1,
                base,
                source_segments=[
                    {
                        "day": "20260101",
                        "segment_key": "090000_300",
                        "stream": STREAM,
                        "source": "mic_audio",
                        "cluster_label": SOLO_CLUSTER_LABEL,
                    }
                ],
            )
        ],
    )
    assert seg_dir.exists()

    result = tracker.backfill_source_sentence_ids()

    tracker.load()
    candidate = _only_candidate(tracker)
    assert result == {"updated": 1, "unresolved": 0, "already_present": 0}
    assert source_segment_sentence_ids(candidate.source_segments[0]) == [1, 2, 3]


def test_retroactive_confirm_relocates_members_from_stored_sentence_ids(
    speakers_env,
    tmp_path,
):
    env = speakers_env()
    _setup_owner(env)
    env.create_entity("Alice Test")
    base = _unit([0.0, 1.0, 0.0])
    trap = _unit([0.0, 0.0, 1.0])
    seg_dir = _write_labeled_segment(
        env,
        "20260101",
        "090000_300",
        {
            1: np.stack([base] * 3),
            2: np.stack([trap] * 3),
        },
    )
    tracker = CandidateTracker(tmp_path / "speaker_candidates.json")
    tracker.process_segment("20260101", "090000_300", STREAM, "mic_audio", seg_dir)
    _rewrite_integer_speaker_labels(
        seg_dir,
        "mic_audio",
        {1: 9, 2: 9, 3: 9, 4: 1, 5: 1, 6: 1},
    )

    plan = tracker.plan_retroactive_confirm(base, "alice_test")

    assert [entry["key"]["sentence_id"] for entry in plan.voiceprints_to_add] == [
        1,
        2,
        3,
    ]
