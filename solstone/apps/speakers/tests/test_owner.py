# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for owner voice identification."""

from __future__ import annotations

import json
import multiprocessing
import os
import re
import shutil
import subprocess
import traceback
from pathlib import Path
from queue import Empty
from typing import Any

import numpy as np
from flask import Flask

from solstone.apps.speakers.encoder_config import (
    ENCODER_ID,
    OVERLAP_DETECTOR_ID,
    OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD,
    OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG,
    OWNER_BOOTSTRAP_MIN_INTRA_COSINE_P25,
    OWNER_BOOTSTRAP_MIN_INTRA_COSINE_P25_STRONG,
    OWNER_MARGIN_MIN,
    SPEAKER_EVIDENCE_VERSION,
)
from solstone.think.awareness import get_current, update_state

SPEAKERS_WORKSPACE = Path(__file__).resolve().parents[1] / "workspace.html"


def _node_or_skip() -> str:
    node = shutil.which("node")
    if node is None:
        import pytest

        pytest.skip("node is not available")
    return node


def _workspace_function_source(source: str, name: str) -> str:
    pattern = rf"^  function {name}\([^)]*\) \{{[\s\S]*?^  \}}\n"
    match = re.search(pattern, source, flags=re.MULTILINE)
    assert match is not None
    return match.group(0)


def _drain_queue(queue: Any) -> list[Any]:
    found = []
    while True:
        try:
            found.append(queue.get_nowait())
        except Empty:
            return found


def _rebuild_owner_worker(
    journal_path: str,
    barrier: Any,
    results: Any,
    errors: Any,
) -> None:
    os.environ["SOLSTONE_JOURNAL"] = journal_path
    try:
        barrier.wait(timeout=5)

        from solstone.apps.speakers.owner import rebuild_owner_centroid

        result = rebuild_owner_centroid()
        results.put(result.get("status"))
    except BaseException:
        errors.put(traceback.format_exc())
        raise


def _normalized(vector: np.ndarray) -> np.ndarray:
    return vector / np.linalg.norm(vector)


def _write_segment(
    journal: Path,
    day: str,
    stream: str,
    segment_key: str,
    source: str,
    embeddings: np.ndarray,
    *,
    durations_s: np.ndarray | None = None,
) -> Path:
    chronicle_day = journal / "chronicle" / day
    chronicle_day.mkdir(parents=True, exist_ok=True)
    flat_day = journal / day
    if not flat_day.exists():
        flat_day.symlink_to(chronicle_day, target_is_directory=True)
    segment_dir = chronicle_day / stream / segment_key
    segment_dir.mkdir(parents=True, exist_ok=True)

    statement_ids = np.arange(1, len(embeddings) + 1, dtype=np.int32)
    npz_kwargs = {
        "embeddings": np.asarray(embeddings, dtype=np.float32),
        "statement_ids": statement_ids,
    }
    if durations_s is not None:
        npz_kwargs["durations_s"] = np.asarray(durations_s, dtype=np.float32)
    np.savez_compressed(segment_dir / f"{source}.npz", **npz_kwargs)

    time_part = segment_key.split("_")[0]
    base_h = int(time_part[0:2])
    base_m = int(time_part[2:4])
    base_s = int(time_part[4:6])
    base_seconds = base_h * 3600 + base_m * 60 + base_s

    lines = [json.dumps({"raw": f"{source}.flac", "model": "medium.en"})]
    for idx in range(len(embeddings)):
        abs_seconds = base_seconds + idx * 5
        h = (abs_seconds // 3600) % 24
        m = (abs_seconds % 3600) // 60
        s = abs_seconds % 60
        lines.append(
            json.dumps(
                {
                    "start": f"{h:02d}:{m:02d}:{s:02d}",
                    "text": f"Sentence {idx + 1}",
                }
            )
        )

    (segment_dir / f"{source}.jsonl").write_text("\n".join(lines) + "\n")
    (segment_dir / f"{source}.flac").write_bytes(b"")
    return segment_dir


def _rewrite_segment_header(segment_dir: Path, source: str, **updates: object) -> None:
    jsonl_path = segment_dir / f"{source}.jsonl"
    lines = jsonl_path.read_text(encoding="utf-8").splitlines()
    header = json.loads(lines[0]) if lines else {}
    header.update(updates)
    lines[0] = json.dumps(header)
    jsonl_path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def _owner_embeddings(count: int, rng: np.random.Generator) -> np.ndarray:
    base = np.zeros(256, dtype=np.float32)
    base[0] = 1.0
    return np.repeat(base.reshape(1, -1), count, axis=0)


def _noise_embeddings(count: int, rng: np.random.Generator) -> np.ndarray:
    embeddings = rng.normal(0, 1, (count, 256)).astype(np.float32)
    return embeddings / np.linalg.norm(embeddings, axis=1, keepdims=True)


def _other_cluster_embeddings(count: int) -> np.ndarray:
    base = np.zeros(256, dtype=np.float32)
    base[1] = 1.0
    return np.repeat(base.reshape(1, -1), count, axis=0)


def _two_lobe_embeddings(count: int, cosine: float) -> np.ndarray:
    first = count // 2
    second = count - first
    a = np.zeros(256, dtype=np.float32)
    a[0] = 1.0
    b = np.zeros(256, dtype=np.float32)
    b[0] = cosine
    b[1] = np.sqrt(1.0 - cosine**2)
    return np.vstack(
        [
            np.repeat(a.reshape(1, -1), first, axis=0),
            np.repeat(b.reshape(1, -1), second, axis=0),
        ]
    ).astype(np.float32)


def _centroid_for_embeddings(embeddings: np.ndarray) -> np.ndarray:
    return _normalized(np.mean(embeddings, axis=0).astype(np.float32))


def _candidate_path(journal: Path) -> Path:
    return journal / "awareness" / "owner_candidate.npz"


def _write_confirmed_owner_centroid(env, *, cluster_size: int = 60) -> Path:
    from solstone.apps.speakers.encoder_config import OWNER_THRESHOLD

    principal_dir = env.create_entity("Self Person", is_principal=True)
    centroid = _normalized(np.array([1.0] + [0.0] * 255, dtype=np.float32))
    np.savez_compressed(
        principal_dir / "owner_centroid.npz",
        centroid=centroid,
        cluster_size=np.array(cluster_size, dtype=np.int32),
        threshold=np.array(OWNER_THRESHOLD, dtype=np.float32),
        last_refreshed_at=np.array("2026-03-15T12:00:00Z"),
    )
    return principal_dir / "owner_centroid.npz"


def _normalize_rows(embeddings: np.ndarray) -> np.ndarray:
    norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
    return embeddings / np.where(norms == 0, 1.0, norms)


def _write_labeled_segment(
    env,
    day: str,
    segment_key: str,
    clusters: dict[int, np.ndarray],
    *,
    stream: str = "test",
    source: str = "mic_audio",
    duration_s: float = 5.0,
    overlap_fraction: float = 0.0,
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
    sentence_id = 1
    for cluster_label, cluster_embeddings in clusters.items():
        for embedding in cluster_embeddings:
            embeddings.append(embedding)
            statement_ids.append(sentence_id)
            durations.append(duration_s)
            labels.append(cluster_label)
            sentence_id += 1

    header = {
        "raw": f"{source}.flac",
        "model": "test",
        "overlap_fraction": overlap_fraction,
        "overlap_detector": OVERLAP_DETECTOR_ID,
    }
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


def _source_segment(
    day: str,
    segment_key: str,
    *,
    stream: str,
    source: str = "mic_audio",
    cluster_label: int = 1,
    sentence_ids: list[int] | None = None,
) -> dict[str, object]:
    source_segment: dict[str, object] = {
        "day": day,
        "stream": stream,
        "segment_key": segment_key,
        "source": source,
        "cluster_label": cluster_label,
    }
    if sentence_ids is not None:
        source_segment["sentence_ids"] = sentence_ids
    return source_segment


def _candidate_record(
    cand_id: int,
    source_segments: list[dict[str, object]],
    *,
    n_intervals: int,
    n_segments: int | None = None,
    total_duration_s: float = 300.0,
    status: str = "pending",
    confirmed_entity: str | None = None,
) -> dict[str, object]:
    centroid = np.zeros(256, dtype=np.float32)
    centroid[0] = 1.0
    migrated_source_segments = []
    for source_segment in source_segments:
        row = dict(source_segment)
        if "sentence_ids" not in row:
            row["sentence_ids"] = list(range(1, int(n_intervals) + 1))
        migrated_source_segments.append(row)
    return {
        "cand_id": cand_id,
        "centroid": centroid.astype(float).tolist(),
        "n_segments": n_segments if n_segments is not None else len(source_segments),
        "n_intervals": n_intervals,
        "total_duration_s": total_duration_s,
        "source_segments": migrated_source_segments,
        "confirmed_entity": confirmed_entity,
        "status": status,
    }


def _write_candidate_pool(
    journal: Path,
    candidates: list[dict[str, object]],
) -> Path:
    path = journal / "awareness" / "speaker_candidates.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    next_id = (
        max((int(candidate["cand_id"]) for candidate in candidates), default=0) + 1
    )
    path.write_text(
        json.dumps({"next_id": next_id, "candidates": candidates}, indent=2) + "\n",
        encoding="utf-8",
    )
    return path


def _save_manual_owner_tags(
    env,
    principal_id: str,
    day: str,
    segment_key: str,
    embeddings: np.ndarray,
    *,
    source: str = "audio",
    method: str = "user_assigned",
    durations_s: np.ndarray | None = None,
    overlap_fraction: float = 0.0,
    stream: str = "test",
) -> Path:
    from solstone.apps.speakers.routes import _save_voiceprint

    normalized_embeddings = _normalize_rows(np.asarray(embeddings, dtype=np.float32))
    segment_dir = _write_segment(
        env.journal,
        day,
        stream,
        segment_key,
        source,
        normalized_embeddings,
        durations_s=durations_s,
    )
    labels_dir = segment_dir / "talents"
    labels_dir.mkdir(parents=True, exist_ok=True)
    labels = [
        {
            "sentence_id": idx,
            "speaker": principal_id,
            "confidence": "high",
            "method": method,
        }
        for idx in range(1, len(normalized_embeddings) + 1)
    ]
    (labels_dir / "speaker_labels.json").write_text(
        json.dumps(
            {
                "labels": labels,
                "owner_centroid_last_refreshed_at": None,
                "voiceprint_versions": {},
            }
        )
        + "\n",
        encoding="utf-8",
    )
    _rewrite_segment_header(
        segment_dir,
        source,
        overlap_fraction=overlap_fraction,
        overlap_detector=OVERLAP_DETECTOR_ID,
    )
    for idx, embedding in enumerate(normalized_embeddings, start=1):
        _save_voiceprint(
            principal_id,
            embedding,
            day,
            segment_key,
            source,
            idx,
            stream=stream,
        )
    return segment_dir


def _write_rebuild_owner_centroid(
    env,
    *,
    centroid: np.ndarray | None = None,
    cluster_size: int = 30,
    threshold: float | None = None,
    last_refreshed_at: str = "2026-03-15T12:00:00Z",
    created_at: str | None = "2026-03-14T12:00:00Z",
    evidence_hash: str | None = "previous-hash",
    evidence_intra_cosine_p25: float | None = 1.0,
    evidence_tier: str | None = OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD,
) -> Path:
    from solstone.apps.speakers.encoder_config import OWNER_MARGIN_MIN, OWNER_THRESHOLD

    principal_dir = env.create_entity("Self Person", is_principal=True)
    centroid_array = (
        _normalized(np.array([1.0] + [0.0] * 255, dtype=np.float32))
        if centroid is None
        else _normalized(np.asarray(centroid, dtype=np.float32))
    )
    arrays: dict[str, np.ndarray] = {
        "centroid": centroid_array.astype(np.float32),
        "cluster_size": np.array(cluster_size, dtype=np.int32),
        "threshold": np.array(
            OWNER_THRESHOLD if threshold is None else threshold,
            dtype=np.float32,
        ),
        "margin": np.array(OWNER_MARGIN_MIN, dtype=np.float32),
        "last_refreshed_at": np.array(last_refreshed_at),
    }
    if created_at is not None:
        arrays["created_at"] = np.array(created_at)
    if evidence_hash is not None:
        arrays["evidence_hash"] = np.array(evidence_hash)
    if evidence_intra_cosine_p25 is not None:
        arrays["evidence_intra_cosine_p25"] = np.array(
            evidence_intra_cosine_p25, dtype=np.float32
        )
    if evidence_tier is not None:
        arrays["evidence_tier"] = np.array(evidence_tier)
    path = principal_dir / "owner_centroid.npz"
    np.savez_compressed(path, **arrays)
    return path


def _seed_rebuild_evidence(
    env,
    *,
    count: int = 30,
    principal_id: str = "self_person",
    method: str = "user_assigned",
    vector: np.ndarray | None = None,
    day: str = "20240101",
    segment_key: str = "090000_300",
    stream: str = "test",
    source: str = "audio",
    duration_s: float = 2.0,
) -> None:
    base = (
        _normalized(np.array([1.0] + [0.0] * 255, dtype=np.float32))
        if vector is None
        else _normalized(np.asarray(vector, dtype=np.float32))
    )
    embeddings = np.repeat(base.reshape(1, -1), count, axis=0)
    _save_manual_owner_tags(
        env,
        principal_id,
        day,
        segment_key,
        embeddings,
        source=source,
        method=method,
        durations_s=np.full(count, duration_s, dtype=np.float32),
        stream=stream,
    )


def test_load_owner_embedding_inventory_counts_without_materializing(
    speakers_env, monkeypatch
):
    import solstone.think.journal_io.npz as npz_io
    from solstone.apps.speakers import owner as owner_module
    from solstone.apps.speakers.owner import load_owner_embedding_inventory
    from solstone.apps.speakers.routes import (
        _load_embeddings_file,
        _scan_segment_embeddings,
    )
    from solstone.think.utils import segment_path

    env = speakers_env()
    _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: _owner_embeddings(4, np.random.default_rng(1))},
        stream="mic",
    )
    _write_labeled_segment(
        env,
        "20240101",
        "091000_300",
        {1: _owner_embeddings(6, np.random.default_rng(2))},
        stream="sys",
    )
    _write_labeled_segment(
        env,
        "20240101",
        "092000_300",
        {1: _owner_embeddings(5, np.random.default_rng(3))},
        stream="boundary",
        overlap_fraction=0.10,
    )

    reference_segments = 0
    reference_embeddings = 0
    for segment in _scan_segment_embeddings("20240101"):
        reference_segments += 1
        segment_dir = segment_path("20240101", segment["key"], segment["stream"])
        for source in segment["sources"]:
            emb_data = _load_embeddings_file(segment_dir / f"{source}.npz")
            assert emb_data is not None
            reference_embeddings += int(len(emb_data[0]))

    def fail_materialize(*args, **kwargs):
        raise AssertionError("inventory materialized embedding arrays")

    monkeypatch.setattr(owner_module, "_routes_helpers", fail_materialize)
    monkeypatch.setattr(owner_module, "load_npz", fail_materialize)
    monkeypatch.setattr(npz_io, "load_npz", fail_materialize)

    assert load_owner_embedding_inventory() == {
        "segments_available": reference_segments,
        "embeddings_available": reference_embeddings,
    }


def test_detect_owner_no_candidate_pool_marks_no_cluster(speakers_env):
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: _owner_embeddings(40, np.random.default_rng(1))},
        stream="mic",
    )

    result = detect_owner_candidate()

    assert result["status"] == "no_cluster"
    assert result["reason"] == "pool_missing"
    assert result["recommendation"] == "no_cluster"
    assert get_current()["voiceprint"]["status"] == "no_cluster"


def test_detect_owner_empty_candidate_pool_marks_no_cluster(speakers_env):
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    _write_candidate_pool(env.journal, [])

    result = detect_owner_candidate()

    assert result["status"] == "no_cluster"
    assert result["reason"] == "pool_empty"
    assert result["recommendation"] == "no_cluster"
    assert get_current()["voiceprint"]["status"] == "no_cluster"


def test_detect_owner_candidate_pool_ready(speakers_env):
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    rng = np.random.default_rng(42)
    _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: _owner_embeddings(20, rng)},
        stream="mic",
    )
    _write_labeled_segment(
        env,
        "20240101",
        "091000_300",
        {1: _owner_embeddings(20, rng)},
        stream="sys",
    )
    _write_labeled_segment(
        env,
        "20240101",
        "092000_300",
        {1: _owner_embeddings(20, rng)},
        stream="mic",
    )
    _write_candidate_pool(
        env.journal,
        [
            _candidate_record(
                1,
                [
                    _source_segment("20240101", "090000_300", stream="mic"),
                    _source_segment("20240101", "091000_300", stream="sys"),
                    _source_segment("20240101", "092000_300", stream="mic"),
                ],
                n_intervals=60,
                total_duration_s=300.0,
            )
        ],
    )

    result = detect_owner_candidate()

    assert result["status"] == "candidate"
    assert result["cluster_size"] == 60
    assert result["streams_represented"] == 2
    assert result["recommendation"] == "ready"
    assert len(result["samples"]) == 3
    sample_segments = set()
    for sample in result["samples"]:
        assert {
            "day",
            "stream",
            "segment_key",
            "source",
            "sentence_id",
            "duration_s",
            "audio_url",
        } <= set(sample)
        sample_segments.add((sample["day"], sample["stream"], sample["segment_key"]))
        assert sample["audio_url"] == (
            f"/app/speakers/api/serve_audio/{sample['day']}/"
            f"{sample['stream']}/{sample['segment_key']}/{sample['source']}.flac"
        )
    assert len(sample_segments) == len(result["samples"])
    assert _candidate_path(env.journal).exists()
    assert get_current()["voiceprint"]["status"] == "candidate"


def test_detect_owner_candidate_refuses_during_rejection_cooldown(speakers_env):
    from datetime import datetime

    from solstone.apps.speakers.copy import OWNER_REJECTION_COOLDOWN_GUIDANCE
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    _write_candidate_pool(
        env.journal,
        [
            _candidate_record(
                1,
                [_source_segment("20240101", "090000_300", stream="mic")],
                n_intervals=60,
            )
        ],
    )
    update_state(
        "voiceprint",
        {"status": "rejected", "rejected_at": datetime.now().isoformat()},
    )

    result = detect_owner_candidate()

    assert result["status"] == "no_cluster"
    assert result["reason"] == "cooldown"
    assert result["recommendation"] == "no_cluster"
    assert result["days_remaining"] == 14
    assert result["next_step"] == "wait_for_cooldown"
    assert result["guidance"] == OWNER_REJECTION_COOLDOWN_GUIDANCE
    assert "--force" in result["guidance"]
    assert not _candidate_path(env.journal).exists()


def test_detect_owner_candidate_force_bypasses_cooldown_and_refreshes_candidate(
    speakers_env,
):
    from datetime import datetime

    from solstone.apps.speakers.owner import (
        detect_owner_candidate,
        owner_detection_ready,
    )

    env = speakers_env()
    rng = np.random.default_rng(42)
    _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: _owner_embeddings(20, rng)},
        stream="mic",
    )
    _write_labeled_segment(
        env,
        "20240101",
        "091000_300",
        {1: _owner_embeddings(20, rng)},
        stream="sys",
    )
    _write_candidate_pool(
        env.journal,
        [
            _candidate_record(
                1,
                [
                    _source_segment("20240101", "090000_300", stream="mic"),
                    _source_segment("20240101", "091000_300", stream="sys"),
                ],
                n_intervals=40,
            )
        ],
    )
    update_state(
        "voiceprint",
        {"status": "rejected", "rejected_at": datetime.now().isoformat()},
    )

    result = detect_owner_candidate(force=True)

    assert result["status"] == "candidate"
    assert result["recommendation"] == "ready"
    assert _candidate_path(env.journal).exists()
    assert get_current()["voiceprint"]["rejected_at"] is None
    ready = owner_detection_ready()
    assert ready["ready"] is True
    assert ready["reason"] == "candidate_found"


def test_detect_owner_candidate_e2e_from_solo_candidate_tracker(speakers_env):
    from solstone.apps.speakers.candidate_tracker import CandidateTracker
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    base = _owner_embeddings(18, np.random.default_rng(42))
    for stream, segment_key in (("mic", "090000_300"), ("sys", "091000_300")):
        seg_dir = _write_labeled_segment(
            env,
            "20240101",
            segment_key,
            {1: base},
            stream=stream,
            speaker_evidence="single",
            write_speaker_labels=False,
        )
        CandidateTracker().process_segment(
            "20240101",
            segment_key,
            stream,
            "mic_audio",
            seg_dir,
        )

    tracker = CandidateTracker()
    candidates = tracker.load_all_candidates()
    assert len(candidates) == 1
    assert candidates[0].n_intervals == 36
    assert not _candidate_path(env.journal).exists()
    assert get_current().get("voiceprint", {}).get("status") != "candidate"

    result = detect_owner_candidate()

    assert result["status"] == "candidate"
    assert result["cluster_size"] == 36
    assert result["streams_represented"] == 2
    assert result["recommendation"] == "ready"
    assert _candidate_path(env.journal).exists()


def test_expand_owner_candidate_solo_uses_npz_statement_ids(speakers_env, tmp_path):
    from solstone.apps.speakers.candidate_tracker import CandidateTracker
    from solstone.apps.speakers.owner import _expand_owner_candidate

    env = speakers_env()
    embeddings = _owner_embeddings(3, np.random.default_rng(1))
    seg_dir = _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: embeddings},
        stream="mic",
        speaker_evidence="single",
        write_speaker_labels=False,
    )
    statement_ids = np.array([10, 20, 30], dtype=np.int32)
    np.savez_compressed(
        seg_dir / "mic_audio.npz",
        embeddings=embeddings.astype(np.float32),
        statement_ids=statement_ids,
        durations_s=np.full(3, 5.0, dtype=np.float32),
        encoder=np.array(ENCODER_ID),
    )
    tracker = CandidateTracker(tmp_path / "speaker_candidates.json")
    tracker.process_segment("20240101", "090000_300", "mic", "mic_audio", seg_dir)

    expansion = _expand_owner_candidate(tracker.load_all_candidates()[0])

    assert [record["sentence_id"] for record in expansion.provenance] == [10, 20, 30]
    assert "missing_integer_labels" not in expansion.skipped


def test_expand_owner_candidate_solo_reapplies_admission_trim_before_cap(
    speakers_env,
):
    from solstone.apps.speakers.candidate_tracker import CandidateTracker
    from solstone.apps.speakers.owner import (
        _expand_owner_candidate,
        confirm_owner_candidate,
        detect_owner_candidate,
    )

    env = speakers_env()
    env.create_entity("Self Person", is_principal=True)
    owner_rows = _owner_embeddings(100, np.random.default_rng(1))
    off_voice = _other_cluster_embeddings(20)
    embeddings = np.vstack([owner_rows, off_voice]).astype(np.float32)
    seg_dir = _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: embeddings},
        stream="mic",
        speaker_evidence="single",
        write_speaker_labels=False,
    )
    tracker = CandidateTracker()
    tracker.process_segment("20240101", "090000_300", "mic", "mic_audio", seg_dir)
    candidates = tracker.load_all_candidates()

    assert len(candidates) == 1
    assert candidates[0].n_intervals == 100
    expansion = _expand_owner_candidate(candidates[0], max_embeddings=105)
    assert len(expansion.provenance) == 100
    assert max(record["sentence_id"] for record in expansion.provenance) == 100

    detected = detect_owner_candidate()
    confirmed = confirm_owner_candidate()

    assert detected["status"] == "candidate"
    assert detected["cluster_size"] == 100
    assert confirmed["status"] == "confirmed"
    with np.load(
        env.journal / "entities" / "self_person" / "owner_centroid.npz",
        allow_pickle=False,
    ) as data:
        assert int(np.asarray(data["cluster_size"]).item()) == 100


def test_expand_owner_candidate_mixes_solo_and_diarizer_sources(speakers_env, tmp_path):
    from solstone.apps.speakers.candidate_tracker import CandidateTracker
    from solstone.apps.speakers.owner import _expand_owner_candidate

    env = speakers_env()
    base = _owner_embeddings(3, np.random.default_rng(1))
    solo_dir = _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: base},
        stream="mic",
        speaker_evidence="single",
        write_speaker_labels=False,
    )
    diarizer_dir = _write_labeled_segment(
        env,
        "20240101",
        "091000_300",
        {1: base},
        stream="mic",
    )
    tracker = CandidateTracker(tmp_path / "speaker_candidates.json")
    tracker.process_segment("20240101", "090000_300", "mic", "mic_audio", solo_dir)
    tracker.process_segment("20240101", "091000_300", "mic", "mic_audio", diarizer_dir)

    candidates = tracker.load_all_candidates()
    assert len(candidates) == 1
    expansion = _expand_owner_candidate(candidates[0])
    provenance_keys = {
        (
            record["day"],
            record["stream"],
            record["segment_key"],
            record["source"],
        )
        for record in expansion.provenance
    }

    assert provenance_keys == {
        ("20240101", "mic", "090000_300", "mic_audio"),
        ("20240101", "mic", "091000_300", "mic_audio"),
    }
    assert all("cluster_label" not in record for record in expansion.provenance)


def test_expand_owner_candidate_relocates_members_from_stored_sentence_ids(
    speakers_env,
    tmp_path,
):
    from solstone.apps.speakers.candidate_tracker import CandidateTracker
    from solstone.apps.speakers.owner import _expand_owner_candidate

    env = speakers_env()
    base = _owner_embeddings(3, np.random.default_rng(1))
    trap = _other_cluster_embeddings(3)
    seg_dir = _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: base, 2: trap},
        stream="mic",
    )
    tracker = CandidateTracker(tmp_path / "speaker_candidates.json")
    tracker.process_segment("20240101", "090000_300", "mic", "mic_audio", seg_dir)
    candidate = tracker.load_all_candidates()[0]
    _rewrite_integer_speaker_labels(
        seg_dir,
        "mic_audio",
        {1: 9, 2: 9, 3: 9, 4: 1, 5: 1, 6: 1},
    )

    expansion = _expand_owner_candidate(candidate)

    assert [record["sentence_id"] for record in expansion.provenance] == [1, 2, 3]


def test_expand_owner_candidate_skips_legacy_sources_without_sentence_ids(
    speakers_env,
):
    from solstone.apps.speakers.candidate_tracker import CandidateProfile
    from solstone.apps.speakers.owner import _expand_owner_candidate

    env = speakers_env()
    base = _owner_embeddings(3, np.random.default_rng(1))
    _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: base},
        stream="mic",
    )
    candidate = CandidateProfile(
        cand_id=1,
        centroid=base[0],
        n_segments=1,
        n_intervals=3,
        total_duration_s=15.0,
        source_segments=[
            {
                "day": "20240101",
                "stream": "mic",
                "segment_key": "090000_300",
                "source": "mic_audio",
                "cluster_label": 1,
            }
        ],
    )

    expansion = _expand_owner_candidate(candidate)

    assert expansion.embeddings.shape == (0, 0)
    assert expansion.provenance == []
    assert expansion.skipped["missing_source_sentence_ids"] == 1


def test_owner_candidate_samples_use_registered_audio_extension(speakers_env):
    from solstone.apps.speakers.owner import _owner_candidate_samples

    env = speakers_env()
    embeddings = _owner_embeddings(3, np.random.default_rng(1))
    provenance = []
    for idx, segment_key in enumerate(
        ("090000_300", "091000_300", "092000_300"),
        start=1,
    ):
        env.create_segment(
            "20240101",
            segment_key,
            ["mic_audio"],
            num_sentences=1,
            embeddings=embeddings[idx - 1 : idx],
            audio_extension=".m4a",
        )
        provenance.append(
            {
                "day": "20240101",
                "stream": "test",
                "segment_key": segment_key,
                "source": "mic_audio",
                "sentence_id": 1,
                "duration_s": 5.0,
            }
        )

    samples = _owner_candidate_samples(embeddings, embeddings[0], provenance)

    assert len(samples) == 3
    for sample in samples:
        assert sample["audio_url"] == (
            f"/app/speakers/api/serve_audio/{sample['day']}/"
            f"{sample['stream']}/{sample['segment_key']}/{sample['source']}.m4a"
        )


def test_owner_candidate_samples_allow_missing_audio(speakers_env):
    from solstone.apps.speakers.owner import _owner_candidate_samples

    env = speakers_env()
    embeddings = _owner_embeddings(1, np.random.default_rng(1))
    env.create_segment(
        "20240101",
        "090000_300",
        ["mic_audio"],
        num_sentences=1,
        embeddings=embeddings,
    )
    (
        env.journal
        / "chronicle"
        / "20240101"
        / "test"
        / "090000_300"
        / "mic_audio.flac"
    ).unlink()
    provenance = [
        {
            "day": "20240101",
            "stream": "test",
            "segment_key": "090000_300",
            "source": "mic_audio",
            "sentence_id": 1,
            "duration_s": 5.0,
        }
    ]

    samples = _owner_candidate_samples(embeddings, embeddings[0], provenance)

    assert samples[0]["audio_url"] is None


def test_detect_owner_candidate_selection_skips_rejected_and_non_principal(
    speakers_env,
):
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    env.create_entity("Self Person", is_principal=True)
    _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: _owner_embeddings(40, np.random.default_rng(3))},
        stream="mic",
    )
    _write_candidate_pool(
        env.journal,
        [
            _candidate_record(
                1,
                [_source_segment("20240101", "090000_300", stream="missing")],
                n_intervals=100,
                status="rejected",
            ),
            _candidate_record(
                2,
                [_source_segment("20240101", "090000_300", stream="missing")],
                n_intervals=90,
                confirmed_entity="someone_else",
            ),
            _candidate_record(
                3,
                [_source_segment("20240101", "090000_300", stream="mic")],
                n_intervals=40,
            ),
        ],
    )

    result = detect_owner_candidate()

    assert result["status"] == "candidate"
    assert result["cluster_size"] == 40


def test_detect_owner_candidate_prefilter_avoids_npz_load(speakers_env, monkeypatch):
    from solstone.apps.speakers import owner as owner_module
    from solstone.apps.speakers.encoder_config import OWNER_BOOTSTRAP_MIN_STMTS
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    _write_candidate_pool(
        env.journal,
        [
            _candidate_record(
                1,
                [_source_segment("20240101", "090000_300", stream="mic")],
                n_intervals=1,
            )
        ],
    )

    def fail_materialize(*args, **kwargs):
        raise AssertionError("prefilter opened segment embeddings")

    monkeypatch.setattr(
        owner_module,
        "_expand_owner_candidate",
        lambda *args, **kwargs: (_ for _ in ()).throw(AssertionError("expanded")),
    )
    monkeypatch.setattr(owner_module, "_routes_helpers", fail_materialize)
    monkeypatch.setattr(owner_module, "load_npz", fail_materialize)

    result = detect_owner_candidate()

    assert result["status"] == "low_quality"
    assert result["source"] == "candidate_pool"
    assert result["low_quality_reason"] == "too_few_stmts"
    assert result["observed_value"] == 1.0
    assert result["threshold_value"] == float(OWNER_BOOTSTRAP_MIN_STMTS)
    assert result["evidence_tier"] == OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD
    assert np.isclose(
        result["intra_cosine_p25_bound"],
        OWNER_BOOTSTRAP_MIN_INTRA_COSINE_P25,
    )
    assert result["segments_available"] == 1
    assert result["embeddings_available"] == 1


def test_detect_owner_candidate_round_robin_prevents_stream_starvation(
    speakers_env, monkeypatch
):
    from solstone.apps.speakers import owner as owner_module
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    rng = np.random.default_rng(7)
    source_segments: list[dict[str, object]] = []
    for idx in range(4):
        segment_key = f"09{idx:02d}00_300"
        _write_labeled_segment(
            env,
            "20240101",
            segment_key,
            {1: _owner_embeddings(20, rng)},
            stream="a_stream",
        )
        source_segments.append(
            _source_segment("20240101", segment_key, stream="a_stream")
        )
    _write_labeled_segment(
        env,
        "20240101",
        "100000_300",
        {1: _owner_embeddings(20, rng)},
        stream="b_stream",
    )
    source_segments.append(_source_segment("20240101", "100000_300", stream="b_stream"))
    _write_candidate_pool(
        env.journal,
        [
            _candidate_record(
                1, source_segments, n_intervals=100, total_duration_s=500.0
            )
        ],
    )
    monkeypatch.setattr(
        owner_module,
        "OWNER_CANDIDATE_EXPANSION_MAX_EMBEDDINGS",
        40,
    )

    result = detect_owner_candidate()

    assert result["status"] == "candidate"
    assert result["cluster_size"] == 40
    assert result["streams_represented"] == 2
    assert result["recommendation"] == "ready"


def test_low_quality_too_few_stmts_from_candidate_pool(speakers_env):
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: _owner_embeddings(10, np.random.default_rng(0))},
        stream="mic",
    )
    _write_candidate_pool(
        env.journal,
        [
            _candidate_record(
                1,
                [_source_segment("20240101", "090000_300", stream="mic")],
                n_intervals=40,
            )
        ],
    )

    result = detect_owner_candidate()

    assert result["status"] == "low_quality"
    assert result["source"] == "candidate_pool"
    assert result["recommendation"] == "low_quality"
    assert result["low_quality_reason"] == "too_few_stmts"
    assert get_current()["voiceprint"]["source"] == "candidate_pool"
    assert not _candidate_path(env.journal).exists()


def test_low_quality_median_duration_too_short_from_candidate_pool(speakers_env):
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: _owner_embeddings(40, np.random.default_rng(0))},
        stream="mic",
        duration_s=0.1,
    )
    _write_candidate_pool(
        env.journal,
        [
            _candidate_record(
                1,
                [_source_segment("20240101", "090000_300", stream="mic")],
                n_intervals=40,
            )
        ],
    )

    result = detect_owner_candidate()

    assert result["status"] == "low_quality"
    assert result["source"] == "candidate_pool"
    assert result["low_quality_reason"] == "median_duration_too_short"
    assert get_current()["voiceprint"]["source"] == "candidate_pool"


def test_low_quality_cluster_too_diffuse_from_candidate_pool(speakers_env):
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: _noise_embeddings(40, np.random.default_rng(0))},
        stream="mic",
        duration_s=5.0,
    )
    _write_candidate_pool(
        env.journal,
        [
            _candidate_record(
                1,
                [_source_segment("20240101", "090000_300", stream="mic")],
                n_intervals=40,
            )
        ],
    )

    result = detect_owner_candidate()

    assert result["status"] == "low_quality"
    assert result["source"] == "candidate_pool"
    assert result["low_quality_reason"] == "cluster_too_diffuse"
    assert result["evidence_tier"] == OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD
    assert np.isclose(result["threshold_value"], OWNER_BOOTSTRAP_MIN_INTRA_COSINE_P25)
    assert np.isclose(
        result["intra_cosine_p25_bound"],
        OWNER_BOOTSTRAP_MIN_INTRA_COSINE_P25,
    )
    assert get_current()["voiceprint"]["evidence_tier"] == (
        OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD
    )
    assert np.isclose(
        get_current()["voiceprint"]["intra_cosine_p25_bound"],
        OWNER_BOOTSTRAP_MIN_INTRA_COSINE_P25,
    )
    assert get_current()["voiceprint"]["source"] == "candidate_pool"


def test_owner_quality_gates_report_tier_boundaries():
    from solstone.apps.speakers.owner import (
        LOW_QUALITY_REASON_CLUSTER_TOO_DIFFUSE,
        LOW_QUALITY_REASON_MEDIAN_DURATION_TOO_SHORT,
        LOW_QUALITY_REASON_TOO_FEW_STMTS,
        _evaluate_owner_quality_gates,
    )

    too_few = _evaluate_owner_quality_gates(
        _two_lobe_embeddings(29, 1.0),
        [2.0] * 29,
    )
    short_duration = _evaluate_owner_quality_gates(
        _two_lobe_embeddings(100, 1.0),
        [0.5] * 100,
    )
    strong_boundary = _evaluate_owner_quality_gates(
        _two_lobe_embeddings(100, OWNER_BOOTSTRAP_MIN_INTRA_COSINE_P25_STRONG),
        [2.0] * 100,
    )
    strong_under = _evaluate_owner_quality_gates(
        _two_lobe_embeddings(104, 0.149),
        [2.0] * 104,
    )
    standard_boundary = _evaluate_owner_quality_gates(
        _two_lobe_embeddings(98, OWNER_BOOTSTRAP_MIN_INTRA_COSINE_P25),
        [2.0] * 98,
    )
    standard_under = _evaluate_owner_quality_gates(
        _two_lobe_embeddings(99, 0.20),
        [2.0] * 99,
    )

    assert too_few.reason == LOW_QUALITY_REASON_TOO_FEW_STMTS
    assert too_few.evidence_tier == OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD
    assert np.isclose(too_few.intra_cosine_p25_bound, 0.30)
    assert short_duration.reason == LOW_QUALITY_REASON_MEDIAN_DURATION_TOO_SHORT
    assert short_duration.evidence_tier == OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG
    assert np.isclose(short_duration.intra_cosine_p25_bound, 0.15)
    assert strong_boundary.reason is None
    assert strong_boundary.evidence_tier == OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG
    assert np.isclose(strong_boundary.intra_cosine_p25_bound, 0.15)
    assert strong_under.reason == LOW_QUALITY_REASON_CLUSTER_TOO_DIFFUSE
    assert strong_under.evidence_tier == OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG
    assert np.isclose(strong_under.threshold_value, 0.15)
    assert np.isclose(strong_under.intra_cosine_p25_bound, 0.15)
    assert standard_boundary.reason is None
    assert standard_boundary.evidence_tier == OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD
    assert np.isclose(standard_boundary.intra_cosine_p25_bound, 0.30)
    assert standard_under.reason == LOW_QUALITY_REASON_CLUSTER_TOO_DIFFUSE
    assert standard_under.evidence_tier == OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD
    assert np.isclose(standard_under.threshold_value, 0.30)
    assert np.isclose(standard_under.intra_cosine_p25_bound, 0.30)


def test_detect_owner_candidate_strong_tier_writes_candidate_and_confirm_metadata(
    speakers_env,
):
    from solstone.apps.speakers.owner import (
        confirm_owner_candidate,
        detect_owner_candidate,
        load_owner_centroid,
    )

    env = speakers_env()
    env.create_entity("Self Person", is_principal=True)
    embeddings = _two_lobe_embeddings(120, 0.22)
    _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: embeddings[:60]},
        stream="mic",
        duration_s=2.0,
    )
    _write_labeled_segment(
        env,
        "20240101",
        "091000_300",
        {1: embeddings[60:]},
        stream="sys",
        duration_s=2.0,
    )
    _write_candidate_pool(
        env.journal,
        [
            _candidate_record(
                1,
                [
                    _source_segment("20240101", "090000_300", stream="mic"),
                    _source_segment("20240101", "091000_300", stream="sys"),
                ],
                n_intervals=120,
                total_duration_s=240.0,
            )
        ],
    )

    detected = detect_owner_candidate()

    assert detected["status"] == "candidate"
    assert detected["cluster_size"] == 120
    assert detected["evidence_tier"] == OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG
    with np.load(_candidate_path(env.journal), allow_pickle=False) as data:
        assert str(np.asarray(data["evidence_tier"]).item()) == (
            OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG
        )
    assert get_current()["voiceprint"]["evidence_tier"] == (
        OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG
    )

    confirmed = confirm_owner_candidate()
    loaded = load_owner_centroid()

    assert confirmed["status"] == "confirmed"
    assert confirmed["evidence_tier"] == OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG
    assert loaded is not None
    assert loaded.evidence_tier == OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG
    assert get_current()["voiceprint"]["evidence_tier"] == (
        OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG
    )


def test_detect_owner_candidate_skips_noisy_source_segments(speakers_env):
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    rng = np.random.default_rng(2)
    _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: _owner_embeddings(40, rng)},
        stream="mic",
        overlap_fraction=0.20,
    )
    _write_labeled_segment(
        env,
        "20240101",
        "091000_300",
        {1: _owner_embeddings(40, rng)},
        stream="mic",
        overlap_fraction=0.0,
    )
    _write_labeled_segment(
        env,
        "20240101",
        "092000_300",
        {1: _owner_embeddings(40, rng)},
        stream="mic",
        overlap_fraction=0.10,
    )
    _write_candidate_pool(
        env.journal,
        [
            _candidate_record(
                1,
                [
                    _source_segment("20240101", "090000_300", stream="mic"),
                    _source_segment("20240101", "091000_300", stream="mic"),
                    _source_segment("20240101", "092000_300", stream="mic"),
                ],
                n_intervals=120,
                total_duration_s=600.0,
            )
        ],
    )

    result = detect_owner_candidate()

    assert result["status"] == "candidate"
    assert result["cluster_size"] == 80
    assert result["recommendation"] == "single_stream"


def test_detect_owner_candidate_missing_npz_after_wipe_marks_no_cluster(speakers_env):
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    seg_dir = _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: _owner_embeddings(40, np.random.default_rng(1))},
        stream="mic",
    )
    (seg_dir / "mic_audio.npz").unlink()
    _write_candidate_pool(
        env.journal,
        [
            _candidate_record(
                1,
                [_source_segment("20240101", "090000_300", stream="mic")],
                n_intervals=40,
            )
        ],
    )

    result = detect_owner_candidate()

    assert result["status"] == "no_cluster"
    assert result["reason"] == "candidate_no_usable_embeddings"
    assert get_current()["voiceprint"]["status"] == "no_cluster"


def test_detect_owner_candidate_missing_segment_dir_marks_no_cluster(speakers_env):
    import shutil

    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    seg_dir = _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: _owner_embeddings(40, np.random.default_rng(1))},
        stream="mic",
    )
    shutil.rmtree(seg_dir)
    _write_candidate_pool(
        env.journal,
        [
            _candidate_record(
                1,
                [_source_segment("20240101", "090000_300", stream="mic")],
                n_intervals=40,
            )
        ],
    )

    result = detect_owner_candidate()

    assert result["status"] == "no_cluster"
    assert result["reason"] == "candidate_no_usable_embeddings"
    assert get_current()["voiceprint"]["status"] == "no_cluster"


def test_detect_owner_candidate_no_cluster_carries_manual_guidance(speakers_env):
    from solstone.apps.speakers.owner import detect_owner_candidate

    speakers_env()

    result = detect_owner_candidate()

    assert result["status"] == "no_cluster"
    assert result["reason"] == "pool_missing"
    assert result["next_step"] == "seed_manual_tags"
    assert result["manual_tags_count"] == 0
    assert result["can_build_from_tags"] is False
    assert result["guidance"]


def test_owner_guidance_next_step_reflects_manual_tag_readiness(speakers_env):
    from solstone.apps.speakers.encoder_config import OWNER_BOOTSTRAP_MIN_STMTS
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    _write_candidate_pool(
        env.journal,
        [
            _candidate_record(
                1,
                [_source_segment("20240101", "090000_300", stream="mic")],
                n_intervals=1,
            )
        ],
    )

    shortfall = detect_owner_candidate()

    env.create_entity("Self Person", is_principal=True)
    embeddings = np.zeros((OWNER_BOOTSTRAP_MIN_STMTS, 256), dtype=np.float32)
    embeddings[:, 0] = 1.0
    _save_manual_owner_tags(
        env,
        "self_person",
        "20240101",
        "100000_300",
        embeddings,
        durations_s=np.full(OWNER_BOOTSTRAP_MIN_STMTS, 2.0, dtype=np.float32),
    )
    ready = detect_owner_candidate()

    assert shortfall["status"] == "low_quality"
    assert shortfall["next_step"] == "seed_manual_tags"
    assert shortfall["can_build_from_tags"] is False
    assert ready["status"] == "low_quality"
    assert ready["next_step"] == "build_from_tags"
    assert ready["can_build_from_tags"] is True
    assert ready["manual_tags_count"] == OWNER_BOOTSTRAP_MIN_STMTS


def test_owner_guidance_is_read_time_only(speakers_env, monkeypatch):
    from solstone.apps.speakers import owner as owner_module
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    writes: list[dict] = []

    def capture_update(_key: str, payload: dict):
        writes.append(payload)

    monkeypatch.setattr(owner_module, "update_state", capture_update)

    no_cluster = detect_owner_candidate()
    _write_candidate_pool(
        env.journal,
        [
            _candidate_record(
                1,
                [_source_segment("20240101", "090000_300", stream="mic")],
                n_intervals=1,
            )
        ],
    )
    low_quality = detect_owner_candidate()

    assert no_cluster["next_step"] == "seed_manual_tags"
    assert low_quality["next_step"] == "seed_manual_tags"
    assert [set(payload) for payload in writes] == [
        {"status", "segments_checked", "attempted_at"},
        {
            "status",
            "source",
            "low_quality_reason",
            "observed_value",
            "threshold_value",
            "evidence_tier",
            "intra_cosine_p25_bound",
            "segments_checked",
            "attempted_at",
        },
    ]


def test_detect_owner_candidate_reuses_persisted_candidate(speakers_env, monkeypatch):
    from solstone.apps.speakers import owner as owner_module
    from solstone.apps.speakers.encoder_config import OWNER_THRESHOLD
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    candidate_path = _candidate_path(env.journal)
    candidate_path.parent.mkdir(parents=True, exist_ok=True)
    version = np.array("2026-03-19T12:00:00Z")
    np.savez_compressed(
        candidate_path,
        centroid=_normalized(np.array([1.0] + [0.0] * 255, dtype=np.float32)),
        cluster_size=np.array(40, dtype=np.int32),
        threshold=np.array(OWNER_THRESHOLD, dtype=np.float32),
        version=version,
    )
    update_state(
        "voiceprint",
        {
            "status": "candidate",
            "cluster_size": 40,
            "streams_represented": 2,
            "recommendation": "ready",
            "samples": [{"day": "20240101"}],
        },
    )
    monkeypatch.setattr(
        owner_module,
        "_expand_owner_candidate",
        lambda *args, **kwargs: (_ for _ in ()).throw(AssertionError("recomputed")),
    )

    result = detect_owner_candidate()

    assert result == {
        "status": "candidate",
        "cluster_size": 40,
        "streams_represented": 2,
        "recommendation": "ready",
        "samples": [{"day": "20240101"}],
        "evidence_tier": OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD,
    }
    with np.load(candidate_path, allow_pickle=False) as data:
        assert str(np.asarray(data["version"]).item()) == str(version.item())


def test_detect_owner_candidate_confirmed_short_circuit(speakers_env):
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    _write_confirmed_owner_centroid(env, cluster_size=60)

    result = detect_owner_candidate()

    assert result["status"] == "confirmed"
    assert result["recommendation"] == "confirmed"
    assert result["cluster_size"] == 60
    assert result["samples"] == []
    assert get_current()["voiceprint"]["status"] == "confirmed"
    assert get_current()["voiceprint"]["cluster_size"] == 60


def test_detect_owner_candidate_confirmed_short_circuit_idempotent(
    speakers_env, monkeypatch
):
    from solstone.apps.speakers import owner as owner_module
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    _write_confirmed_owner_centroid(env, cluster_size=60)
    update_state(
        "voiceprint",
        {
            "status": "confirmed",
            "cluster_size": 60,
            "confirmed_at": "2026-03-15T12:00:00Z",
        },
    )

    def fail_update_state(*args, **kwargs):
        raise AssertionError("confirmed short-circuit rewrote awareness")

    monkeypatch.setattr(owner_module, "update_state", fail_update_state)

    result = detect_owner_candidate()

    assert result["status"] == "confirmed"
    assert result["cluster_size"] == 60


def test_bootstrap_owner_from_manual_tags_confirms(speakers_env):
    from solstone.apps.speakers.encoder_config import (
        OWNER_BOOTSTRAP_MIN_STMTS,
        OWNER_THRESHOLD,
    )
    from solstone.apps.speakers.owner import bootstrap_owner_from_manual_tags

    env = speakers_env()
    principal_dir = env.create_entity("Self Person", is_principal=True)
    principal_id = "self_person"
    rng = np.random.default_rng(4)
    batch_count = 3
    batch_size = OWNER_BOOTSTRAP_MIN_STMTS // batch_count
    final_batch_size = OWNER_BOOTSTRAP_MIN_STMTS - (batch_size * (batch_count - 1))
    base = np.zeros((max(batch_size, final_batch_size), 256), dtype=np.float32)
    base[:, 0] = 1.0
    durations = np.full(base.shape[0], 2.4, dtype=np.float32)
    for idx, count in enumerate([batch_size, batch_size, final_batch_size]):
        embeddings = base[:count] + rng.normal(scale=0.01, size=(count, 256)).astype(
            np.float32
        )
        _save_manual_owner_tags(
            env,
            principal_id,
            "20240101",
            f"{9 + idx:02d}0000_300",
            embeddings,
            durations_s=durations[:count],
        )

    result = bootstrap_owner_from_manual_tags()

    owner_path = principal_dir / "owner_centroid.npz"
    assert set(result) == {"status", "principal_id", "cluster_size", "evidence_tier"}
    assert result["status"] == "confirmed"
    assert result["principal_id"] == principal_id
    assert result["cluster_size"] == OWNER_BOOTSTRAP_MIN_STMTS
    assert owner_path.exists()
    with np.load(owner_path, allow_pickle=False) as data:
        assert set(data.files) == {
            "centroid",
            "cluster_size",
            "threshold",
            "margin",
            "last_refreshed_at",
            "created_at",
            "evidence_tier",
        }
        centroid = data["centroid"]
        cluster_size = int(np.asarray(data["cluster_size"]).item())
        threshold = float(np.asarray(data["threshold"]).item())
        margin = float(np.asarray(data["margin"]).item())
        last_refreshed_at = str(np.asarray(data["last_refreshed_at"]).item())
        created_at = str(np.asarray(data["created_at"]).item())
    assert cluster_size == OWNER_BOOTSTRAP_MIN_STMTS
    assert np.isclose(np.linalg.norm(centroid), 1.0)
    assert np.isclose(threshold, OWNER_THRESHOLD)
    assert np.isclose(margin, OWNER_MARGIN_MIN)
    assert last_refreshed_at.endswith("Z")
    assert created_at == last_refreshed_at
    assert get_current()["voiceprint"]["status"] == "confirmed"
    assert get_current()["voiceprint"]["evidence_tier"] == (
        OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD
    )


def test_bootstrap_owner_from_manual_tags_strong_boundary_writes_tier(
    speakers_env,
):
    from solstone.apps.speakers.owner import (
        bootstrap_owner_from_manual_tags,
        load_owner_centroid,
    )

    env = speakers_env()
    principal_dir = env.create_entity("Self Person", is_principal=True)
    embeddings = _two_lobe_embeddings(100, OWNER_BOOTSTRAP_MIN_INTRA_COSINE_P25_STRONG)
    _save_manual_owner_tags(
        env,
        "self_person",
        "20240101",
        "090000_300",
        embeddings,
        durations_s=np.full(100, 2.0, dtype=np.float32),
    )

    result = bootstrap_owner_from_manual_tags()
    loaded = load_owner_centroid()

    assert result["status"] == "confirmed"
    assert result["cluster_size"] == 100
    assert result["evidence_tier"] == OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG
    with np.load(principal_dir / "owner_centroid.npz", allow_pickle=False) as data:
        assert str(np.asarray(data["evidence_tier"]).item()) == (
            OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG
        )
    assert loaded is not None
    assert loaded.evidence_tier == OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG
    assert get_current()["voiceprint"]["evidence_tier"] == (
        OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG
    )


def test_bootstrap_owner_from_manual_tags_too_few_stmts(speakers_env):
    from solstone.apps.speakers.encoder_config import OWNER_BOOTSTRAP_MIN_STMTS
    from solstone.apps.speakers.owner import (
        LOW_QUALITY_REASON_TOO_FEW_STMTS,
        bootstrap_owner_from_manual_tags,
    )

    env = speakers_env()
    env.create_entity("Self Person", is_principal=True)
    embeddings = np.zeros((10, 256), dtype=np.float32)
    embeddings[:, 0] = 1.0
    validated_count = int(embeddings.shape[0])
    _save_manual_owner_tags(
        env,
        "self_person",
        "20240101",
        "090000_300",
        embeddings,
        durations_s=np.full(validated_count, 2.0, dtype=np.float32),
    )

    result = bootstrap_owner_from_manual_tags()

    assert result["status"] == "low_quality"
    assert result["source"] == "manual_tags"
    assert result["low_quality_reason"] == LOW_QUALITY_REASON_TOO_FEW_STMTS
    assert result["observed_value"] == validated_count
    assert result["threshold_value"] == OWNER_BOOTSTRAP_MIN_STMTS
    assert result["manual_tags_count"] == validated_count
    assert result["next_step"] == "seed_manual_tags"
    assert get_current()["voiceprint"]["source"] == "manual_tags"


def test_bootstrap_owner_from_manual_tags_short_durations(speakers_env):
    from solstone.apps.speakers.owner import (
        LOW_QUALITY_REASON_MEDIAN_DURATION_TOO_SHORT,
        bootstrap_owner_from_manual_tags,
    )

    env = speakers_env()
    env.create_entity("Self Person", is_principal=True)
    base = np.zeros((10, 256), dtype=np.float32)
    base[:, 0] = 1.0
    for idx in range(3):
        _save_manual_owner_tags(
            env,
            "self_person",
            "20240101",
            f"{9 + idx:02d}0000_300",
            base,
            durations_s=np.full(10, 0.3, dtype=np.float32),
        )

    result = bootstrap_owner_from_manual_tags()

    assert result["status"] == "low_quality"
    assert result["source"] == "manual_tags"
    assert result["low_quality_reason"] == LOW_QUALITY_REASON_MEDIAN_DURATION_TOO_SHORT


def test_bootstrap_owner_from_manual_tags_diffuse_cluster(speakers_env):
    from solstone.apps.speakers.owner import (
        LOW_QUALITY_REASON_CLUSTER_TOO_DIFFUSE,
        bootstrap_owner_from_manual_tags,
    )

    env = speakers_env()
    env.create_entity("Self Person", is_principal=True)
    rng = np.random.default_rng(9)
    for idx in range(3):
        _save_manual_owner_tags(
            env,
            "self_person",
            "20240101",
            f"{9 + idx:02d}0000_300",
            _noise_embeddings(10, rng),
            durations_s=np.full(10, 2.0, dtype=np.float32),
        )

    result = bootstrap_owner_from_manual_tags()

    assert result["status"] == "low_quality"
    assert result["source"] == "manual_tags"
    assert result["low_quality_reason"] == LOW_QUALITY_REASON_CLUSTER_TOO_DIFFUSE


def test_manual_tag_overlap_boundary_excludes_above_accepts_equal_absent(
    speakers_env,
):
    from solstone.apps.speakers.owner import (
        LOW_QUALITY_REASON_TOO_FEW_STMTS,
        bootstrap_owner_from_manual_tags,
        count_manual_tag_embeddings,
    )

    env = speakers_env()
    env.create_entity("Self Person", is_principal=True)
    embeddings = np.zeros((5, 256), dtype=np.float32)
    embeddings[:, 0] = 1.0
    _save_manual_owner_tags(
        env,
        "self_person",
        "20240101",
        "090000_300",
        embeddings,
        durations_s=np.full(5, 2.0, dtype=np.float32),
        overlap_fraction=0.0,
    )
    _save_manual_owner_tags(
        env,
        "self_person",
        "20240101",
        "100000_300",
        embeddings,
        durations_s=np.full(5, 2.0, dtype=np.float32),
        overlap_fraction=0.20,
    )
    _save_manual_owner_tags(
        env,
        "self_person",
        "20240101",
        "110000_300",
        embeddings,
        durations_s=np.full(5, 2.0, dtype=np.float32),
        overlap_fraction=0.10,
    )
    absent_overlap_dir = _save_manual_owner_tags(
        env,
        "self_person",
        "20240101",
        "120000_300",
        embeddings,
        durations_s=np.full(5, 2.0, dtype=np.float32),
        overlap_fraction=0.0,
    )
    jsonl_path = absent_overlap_dir / "audio.jsonl"
    lines = jsonl_path.read_text(encoding="utf-8").splitlines()
    header = json.loads(lines[0])
    header.pop("overlap_fraction")
    header.pop("overlap_detector")
    lines[0] = json.dumps(header)
    jsonl_path.write_text("\n".join(lines) + "\n", encoding="utf-8")

    assert count_manual_tag_embeddings("self_person") == 15
    result = bootstrap_owner_from_manual_tags()
    assert result["low_quality_reason"] == LOW_QUALITY_REASON_TOO_FEW_STMTS
    assert result["observed_value"] == 15


def test_bootstrap_owner_from_manual_tags_is_idempotent(speakers_env):
    from solstone.apps.speakers.owner import bootstrap_owner_from_manual_tags

    env = speakers_env()
    env.create_entity("Self Person", is_principal=True)
    base = np.zeros((10, 256), dtype=np.float32)
    base[:, 0] = 1.0
    for idx in range(3):
        _save_manual_owner_tags(
            env,
            "self_person",
            "20240101",
            f"{9 + idx:02d}0000_300",
            base,
            durations_s=np.full(10, 2.1, dtype=np.float32),
        )

    first = bootstrap_owner_from_manual_tags()
    state_before = dict(get_current()["voiceprint"])
    second = bootstrap_owner_from_manual_tags()

    assert first["status"] == "confirmed"
    assert second["status"] == "confirmed"
    assert second["cluster_size"] == first["cluster_size"]
    assert dict(get_current()["voiceprint"]) == state_before


def test_confirm_owner_candidate_writes_margin_schema(speakers_env):
    from solstone.apps.speakers.encoder_config import OWNER_MARGIN_MIN, OWNER_THRESHOLD
    from solstone.apps.speakers.owner import (
        confirm_owner_candidate,
        load_owner_centroid,
    )

    env = speakers_env()
    principal_dir = env.create_entity("Self Person", is_principal=True)
    candidate_path = _candidate_path(env.journal)
    candidate_path.parent.mkdir(parents=True, exist_ok=True)
    centroid = _normalized(np.array([1.0] + [0.0] * 255, dtype=np.float32))
    np.savez_compressed(
        candidate_path,
        centroid=centroid,
        cluster_size=np.array(40, dtype=np.int32),
        threshold=np.array(OWNER_THRESHOLD, dtype=np.float32),
        version=np.array("2026-03-19T12:00:00"),
    )

    result = confirm_owner_candidate()

    assert result["status"] == "confirmed"
    with np.load(principal_dir / "owner_centroid.npz", allow_pickle=False) as data:
        assert set(data.files) == {
            "centroid",
            "cluster_size",
            "threshold",
            "margin",
            "last_refreshed_at",
            "created_at",
            "evidence_tier",
        }
        assert str(np.asarray(data["created_at"]).item()) == str(
            np.asarray(data["last_refreshed_at"]).item()
        )
        assert np.isclose(float(np.asarray(data["margin"]).item()), OWNER_MARGIN_MIN)
        assert str(np.asarray(data["evidence_tier"]).item()) == (
            OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD
        )
    loaded = load_owner_centroid()
    assert loaded is not None
    assert np.isclose(loaded.margin, OWNER_MARGIN_MIN)
    assert loaded.evidence_tier == OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD


def test_confirm_owner_candidate_pre_first_centroid_flow_unchanged(speakers_env):
    from solstone.apps.speakers.encoder_config import OWNER_THRESHOLD
    from solstone.apps.speakers.owner import confirm_owner_candidate

    env = speakers_env()
    principal_dir = env.create_entity("Self Person", is_principal=True)
    candidate_path = _candidate_path(env.journal)
    candidate_path.parent.mkdir(parents=True, exist_ok=True)
    np.savez_compressed(
        candidate_path,
        centroid=_normalized(np.array([1.0] + [0.0] * 255, dtype=np.float32)),
        cluster_size=np.array(88, dtype=np.int32),
        threshold=np.array(OWNER_THRESHOLD, dtype=np.float32),
        version=np.array("2026-03-19T12:00:00"),
    )

    result = confirm_owner_candidate()

    assert result == {
        "status": "confirmed",
        "principal_id": "self_person",
        "cluster_size": 88,
        "evidence_tier": OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD,
    }
    assert not candidate_path.exists()
    assert (principal_dir / "owner_centroid.npz").exists()
    assert get_current()["voiceprint"]["status"] == "confirmed"


def test_bootstrap_owner_from_manual_tags_writes_margin_schema(speakers_env):
    from solstone.apps.speakers.encoder_config import OWNER_BOOTSTRAP_MIN_STMTS
    from solstone.apps.speakers.owner import (
        bootstrap_owner_from_manual_tags,
        load_owner_centroid,
    )

    env = speakers_env()
    principal_dir = env.create_entity("Self Person", is_principal=True)
    _seed_rebuild_evidence(env, count=OWNER_BOOTSTRAP_MIN_STMTS)

    result = bootstrap_owner_from_manual_tags()

    assert result["status"] == "confirmed"
    with np.load(principal_dir / "owner_centroid.npz", allow_pickle=False) as data:
        assert set(data.files) == {
            "centroid",
            "cluster_size",
            "threshold",
            "margin",
            "last_refreshed_at",
            "created_at",
            "evidence_tier",
        }
        assert str(np.asarray(data["created_at"]).item()) == str(
            np.asarray(data["last_refreshed_at"]).item()
        )
        assert np.isclose(float(np.asarray(data["margin"]).item()), OWNER_MARGIN_MIN)
        assert str(np.asarray(data["evidence_tier"]).item()) == (
            OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD
        )
    loaded = load_owner_centroid()
    assert loaded is not None
    assert np.isclose(loaded.margin, OWNER_MARGIN_MIN)
    assert loaded.evidence_tier == OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD


def test_rebuild_npz_key_set_includes_only_rebuild_metadata(speakers_env):
    from solstone.apps.speakers.encoder_config import OWNER_THRESHOLD
    from solstone.apps.speakers.owner import (
        OWNER_REBUILD_EXPECTED_KEYS,
        load_owner_centroid,
        rebuild_owner_centroid,
    )

    env = speakers_env()
    owner_path = _write_rebuild_owner_centroid(env, evidence_hash=None, created_at=None)
    _seed_rebuild_evidence(env)

    result = rebuild_owner_centroid()

    assert result["status"] == "rebuilt"
    assert result["created_at"] == "2026-03-15T12:00:00Z"
    assert np.isclose(result["margin"], OWNER_MARGIN_MIN)
    with np.load(owner_path, allow_pickle=False) as data:
        assert set(data.files) == set(OWNER_REBUILD_EXPECTED_KEYS)
        assert np.isclose(float(np.asarray(data["threshold"]).item()), OWNER_THRESHOLD)
        assert np.isclose(float(np.asarray(data["margin"]).item()), OWNER_MARGIN_MIN)
        assert str(np.asarray(data["evidence_tier"]).item()) == (
            OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD
        )

    loaded = load_owner_centroid()
    assert loaded is not None
    assert np.isclose(loaded.margin, OWNER_MARGIN_MIN)
    assert loaded.evidence_tier == OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD


def test_load_owner_centroid_old_file_missing_rebuild_keys(speakers_env):
    from solstone.apps.speakers.owner import load_owner_centroid

    env = speakers_env()
    _write_confirmed_owner_centroid(env)

    centroid = load_owner_centroid()

    assert centroid is not None
    assert centroid.created_at is None
    assert centroid.evidence_hash is None
    assert centroid.evidence_intra_cosine_p25 is None
    assert centroid.evidence_tier == OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD


def test_rebuild_uses_load_manual_tag_rows_label_authority(speakers_env):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    _write_rebuild_owner_centroid(env, evidence_hash=None)
    _seed_rebuild_evidence(env)
    env.create_speaker_corrections(
        "20240101",
        "090000_300",
        [
            {
                "sentence_id": 1,
                "original_speaker": "self_person",
                "corrected_speaker": "someone_else",
                "timestamp": 0,
            }
        ],
    )

    result = rebuild_owner_centroid()

    assert result["status"] == "rebuilt"
    assert result["evidence_counts"]["used"] == 30


def test_rebuild_excludes_overlap_gated_rows_exactly_like_manual_loader(speakers_env):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    _write_rebuild_owner_centroid(env, evidence_hash=None)
    _save_manual_owner_tags(
        env,
        "self_person",
        "20240101",
        "090000_300",
        np.repeat(
            _normalized(np.array([1.0] + [0.0] * 255, dtype=np.float32)).reshape(1, -1),
            30,
            axis=0,
        ),
        durations_s=np.full(30, 2.0, dtype=np.float32),
        overlap_fraction=0.2,
    )

    result = rebuild_owner_centroid()

    assert result["status"] == "low_quality"
    assert result["low_quality_reason"] == "too_few_stmts"
    assert result["evidence_counts"]["used"] == 0


def test_rebuild_counts_user_confirmed_but_excludes_from_evidence(speakers_env):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    _write_rebuild_owner_centroid(env, evidence_hash=None)
    _seed_rebuild_evidence(env, method="user_confirmed")

    result = rebuild_owner_centroid()

    assert result["status"] == "low_quality"
    assert result["low_quality_reason"] == "too_few_stmts"
    assert result["evidence_counts"] == {
        "eligible": 30,
        "used": 0,
        "user_assigned": 0,
        "user_corrected": 0,
        "user_confirmed_excluded": 30,
    }


def test_existing_manual_tag_consumers_keep_default_method_set(speakers_env):
    from solstone.apps.speakers.owner import count_manual_tag_embeddings

    env = speakers_env()
    env.create_entity("Self Person", is_principal=True)
    _seed_rebuild_evidence(env, method="user_confirmed")

    assert count_manual_tag_embeddings("self_person") == 30


def test_rebuild_rejects_centroid_agreement_too_low(speakers_env):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    incumbent = np.array([0.0, 1.0] + [0.0] * 254, dtype=np.float32)
    _write_rebuild_owner_centroid(env, centroid=incumbent, evidence_hash=None)
    _seed_rebuild_evidence(env)

    result = rebuild_owner_centroid()

    assert result["status"] == "rejected_regression"
    assert result["reason"] == "centroid_agreement_too_low"


def test_rebuild_override_forces_write_past_regression_guard(speakers_env):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    incumbent = np.array([0.0, 1.0] + [0.0] * 254, dtype=np.float32)
    owner_path = _write_rebuild_owner_centroid(env, centroid=incumbent)
    _seed_rebuild_evidence(env)

    result = rebuild_owner_centroid(override=True)

    assert result["status"] == "rebuilt"
    assert result["override_applied"] is True
    with np.load(owner_path, allow_pickle=False) as data:
        centroid = data["centroid"]
    assert centroid[0] > 0.99


def test_rebuild_override_does_not_bypass_absolute_quality_floor(speakers_env):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    _write_rebuild_owner_centroid(env, evidence_hash=None)
    _seed_rebuild_evidence(env, count=1)

    result = rebuild_owner_centroid(override=True)

    assert result["status"] == "low_quality"
    assert result["low_quality_reason"] == "too_few_stmts"


def test_first_rebuild_skips_population_specific_guards_for_confirm_incumbent(
    speakers_env,
):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    _write_rebuild_owner_centroid(env, cluster_size=3000, evidence_hash=None)
    _seed_rebuild_evidence(env)

    result = rebuild_owner_centroid()

    assert result["status"] == "rebuilt"
    assert result["incumbent_guard"]["cluster_size_compared"] is False
    assert result["incumbent_guard"]["cohesion_compared"] is False
    assert result["incumbent_guard"]["reason"] == "incumbent_not_rebuild_sourced"


def test_first_rebuild_response_marks_incumbent_cohesion_unknown(speakers_env):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    _write_rebuild_owner_centroid(env, evidence_hash=None)
    _seed_rebuild_evidence(env)

    first = rebuild_owner_centroid()
    second = rebuild_owner_centroid()

    assert first["status"] == "rebuilt"
    assert first["incumbent_cohesion"] == "unknown"
    assert second["status"] == "unchanged"
    assert isinstance(second["incumbent_cohesion"], float)


def test_rebuild_to_rebuild_applies_cluster_size_and_cohesion_guards(speakers_env):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    _write_rebuild_owner_centroid(env, cluster_size=60, evidence_intra_cosine_p25=1.0)
    _seed_rebuild_evidence(env, count=30)

    result = rebuild_owner_centroid()

    assert result["status"] == "rejected_regression"
    assert result["reason"] == "cluster_size_regression"
    assert result["incumbent_guard"]["cluster_size_compared"] is True
    assert result["incumbent_guard"]["cohesion_compared"] is True


def test_rebuild_owner_centroid_strong_far_field_accepts_lower_floor(
    speakers_env,
):
    from solstone.apps.speakers.owner import load_owner_centroid, rebuild_owner_centroid

    env = speakers_env()
    embeddings = _two_lobe_embeddings(102, 0.16)
    _write_rebuild_owner_centroid(
        env,
        centroid=_centroid_for_embeddings(embeddings),
        evidence_hash=None,
    )
    _save_manual_owner_tags(
        env,
        "self_person",
        "20240101",
        "090000_300",
        embeddings,
        durations_s=np.full(102, 2.0, dtype=np.float32),
    )

    result = rebuild_owner_centroid()
    loaded = load_owner_centroid()

    assert result["status"] == "rebuilt"
    assert result["cluster_size"] == 102
    assert result["evidence_tier"] == OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG
    assert np.isclose(result["intra_cosine_p25_bound"], 0.15)
    assert np.isclose(result["evidence_quality"]["intra_cosine_p25"], 0.16)
    assert loaded is not None
    assert loaded.evidence_tier == OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG
    assert get_current()["voiceprint"]["evidence_tier"] == (
        OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG
    )


def test_rebuild_owner_centroid_strong_floor_refuses_just_under_bound(
    speakers_env,
):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    _write_rebuild_owner_centroid(env, evidence_hash=None)
    embeddings = _two_lobe_embeddings(100, 0.149)
    _save_manual_owner_tags(
        env,
        "self_person",
        "20240101",
        "090000_300",
        embeddings,
        durations_s=np.full(100, 2.0, dtype=np.float32),
    )

    result = rebuild_owner_centroid()

    assert result["status"] == "low_quality"
    assert result["low_quality_reason"] == "cluster_too_diffuse"
    assert result["evidence_tier"] == OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG
    assert np.isclose(result["threshold_value"], 0.15)
    assert np.isclose(result["intra_cosine_p25_bound"], 0.15)
    assert result["observed_value"] < 0.15


def test_rebuild_cross_tier_skips_cohesion_drop_guard_when_agreement_and_size_pass(
    speakers_env,
):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    embeddings = _two_lobe_embeddings(100, 0.16)
    _write_rebuild_owner_centroid(
        env,
        centroid=_centroid_for_embeddings(embeddings),
        cluster_size=100,
        evidence_intra_cosine_p25=0.35,
        evidence_tier=OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD,
    )
    _save_manual_owner_tags(
        env,
        "self_person",
        "20240101",
        "090000_300",
        embeddings,
        durations_s=np.full(100, 2.0, dtype=np.float32),
    )

    result = rebuild_owner_centroid()

    assert result["status"] == "rebuilt"
    assert result["evidence_tier"] == OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG
    assert result["incumbent_guard"]["cluster_size_compared"] is True
    assert result["incumbent_guard"]["same_evidence_tier"] is False
    assert result["incumbent_guard"]["cohesion_compared"] is False


def test_rebuild_cross_tier_still_refuses_low_centroid_agreement(speakers_env):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    embeddings = _two_lobe_embeddings(100, 0.16)
    _write_rebuild_owner_centroid(
        env,
        centroid=np.array([0.0, 0.0, 1.0] + [0.0] * 253, dtype=np.float32),
        cluster_size=100,
        evidence_intra_cosine_p25=0.35,
        evidence_tier=OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD,
    )
    _save_manual_owner_tags(
        env,
        "self_person",
        "20240101",
        "090000_300",
        embeddings,
        durations_s=np.full(100, 2.0, dtype=np.float32),
    )

    result = rebuild_owner_centroid()

    assert result["status"] == "rejected_regression"
    assert result["reason"] == "centroid_agreement_too_low"
    assert result["evidence_tier"] == OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG
    assert result["incumbent_guard"]["cohesion_compared"] is False


def test_rebuild_same_tier_cohesion_regression_still_refuses(speakers_env):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    embeddings = _two_lobe_embeddings(100, 0.16)
    _write_rebuild_owner_centroid(
        env,
        centroid=_centroid_for_embeddings(embeddings),
        cluster_size=100,
        evidence_intra_cosine_p25=0.35,
        evidence_tier=OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG,
    )
    _save_manual_owner_tags(
        env,
        "self_person",
        "20240101",
        "090000_300",
        embeddings,
        durations_s=np.full(100, 2.0, dtype=np.float32),
    )

    result = rebuild_owner_centroid()

    assert result["status"] == "rejected_regression"
    assert result["reason"] == "cohesion_regression"
    assert result["incumbent_guard"]["same_evidence_tier"] is True
    assert result["incumbent_guard"]["cohesion_compared"] is True


def test_rebuild_regression_ignores_load_time_voiceprint_summary_cohesion(speakers_env):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    _write_rebuild_owner_centroid(env, evidence_intra_cosine_p25=0.30)
    _seed_rebuild_evidence(env)

    result = rebuild_owner_centroid()

    assert result["status"] == "rebuilt"


def test_rebuild_missing_incumbent_evidence_cohesion_is_none_not_zero(speakers_env):
    from solstone.apps.speakers.owner import load_owner_centroid

    env = speakers_env()
    _write_rebuild_owner_centroid(
        env,
        evidence_hash=None,
        evidence_intra_cosine_p25=None,
    )

    centroid = load_owner_centroid()

    assert centroid is not None
    assert centroid.evidence_intra_cosine_p25 is None


def test_rebuild_regression_refusal_preserves_owner_centroid_bytes_mtime_and_confirmed_awareness(
    speakers_env,
):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    owner_path = _write_rebuild_owner_centroid(
        env,
        centroid=np.array([0.0, 1.0] + [0.0] * 254, dtype=np.float32),
    )
    update_state("voiceprint", {"status": "confirmed", "cluster_size": 30})
    _seed_rebuild_evidence(env)
    os.utime(owner_path, (1_700_000_000, 1_700_000_000))
    before = owner_path.read_bytes()
    before_mtime = owner_path.stat().st_mtime_ns

    result = rebuild_owner_centroid()

    assert result["status"] == "rejected_regression"
    assert owner_path.read_bytes() == before
    assert owner_path.stat().st_mtime_ns == before_mtime
    assert get_current()["voiceprint"]["status"] == "confirmed"


def test_rebuild_low_quality_refusal_preserves_owner_centroid_bytes_mtime_and_confirmed_awareness(
    speakers_env,
):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    owner_path = _write_rebuild_owner_centroid(env, evidence_hash=None)
    update_state("voiceprint", {"status": "confirmed", "cluster_size": 30})
    _seed_rebuild_evidence(env, count=1)
    os.utime(owner_path, (1_700_000_000, 1_700_000_000))
    before = owner_path.read_bytes()
    before_mtime = owner_path.stat().st_mtime_ns

    result = rebuild_owner_centroid()

    assert result["status"] == "low_quality"
    assert owner_path.read_bytes() == before
    assert owner_path.stat().st_mtime_ns == before_mtime
    assert get_current()["voiceprint"]["status"] == "confirmed"


def test_rebuild_low_quality_does_not_update_awareness(speakers_env):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    _write_rebuild_owner_centroid(env, evidence_hash=None)
    update_state("voiceprint", {"status": "confirmed", "cluster_size": 30})
    _seed_rebuild_evidence(env, count=1)
    state_before = dict(get_current()["voiceprint"])

    result = rebuild_owner_centroid()

    assert result["status"] == "low_quality"
    assert dict(get_current()["voiceprint"]) == state_before


def test_detect_owner_candidate_low_quality_still_updates_awareness(speakers_env):
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    _write_candidate_pool(
        env.journal,
        [
            _candidate_record(
                1,
                [_source_segment("20240101", "090000_300", stream="mic")],
                n_intervals=1,
            )
        ],
    )

    result = detect_owner_candidate()

    assert result["status"] == "low_quality"
    assert get_current()["voiceprint"]["status"] == "low_quality"


def test_bootstrap_owner_from_manual_tags_low_quality_still_updates_awareness(
    speakers_env,
):
    from solstone.apps.speakers.owner import bootstrap_owner_from_manual_tags

    env = speakers_env()
    env.create_entity("Self Person", is_principal=True)
    _seed_rebuild_evidence(env, count=1)

    result = bootstrap_owner_from_manual_tags()

    assert result["status"] == "low_quality"
    assert get_current()["voiceprint"]["status"] == "low_quality"


def test_rebuild_no_principal_returns_clean_refusal(speakers_env):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    speakers_env()

    result = rebuild_owner_centroid()

    assert result["status"] == "refused"
    assert result["reason"] == "no_principal"
    assert result["next_step"] == "set_identity"
    assert (
        result["guidance"] == "set your journal identity before rebuilding your voice."
    )


def test_rebuild_no_principal_with_identity_guides_owner_statement_tag(
    speakers_env,
):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    env.set_identity(preferred="Self Person")

    result = rebuild_owner_centroid()

    assert result["status"] == "refused"
    assert result["reason"] == "no_principal"
    assert result["next_step"] == "tag_owner_statement"
    assert result["guidance"] == (
        "tag one owner statement first so sol can create your owner profile, "
        "then build your voice from tags."
    )


def test_rebuild_confirm_sourced_without_manual_tags_refuses_too_few_stmts_preserves_incumbent(
    speakers_env,
):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    owner_path = _write_rebuild_owner_centroid(env, evidence_hash=None)
    before = owner_path.read_bytes()

    result = rebuild_owner_centroid()

    assert result["status"] == "low_quality"
    assert result["low_quality_reason"] == "too_few_stmts"
    assert owner_path.read_bytes() == before


def test_rebuild_same_five_tuple_evidence_hash_noops_without_deleting_npz(
    speakers_env,
):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    owner_path = _write_rebuild_owner_centroid(env, evidence_hash=None)
    _seed_rebuild_evidence(env)
    first = rebuild_owner_centroid()
    before = owner_path.read_bytes()

    second = rebuild_owner_centroid()

    assert first["status"] == "rebuilt"
    assert second["status"] == "unchanged"
    assert owner_path.exists()
    assert owner_path.read_bytes() == before


def test_retag_same_sentence_does_not_change_rebuild_evidence_hash(speakers_env):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    _write_rebuild_owner_centroid(env, evidence_hash=None)
    _seed_rebuild_evidence(env)
    first = rebuild_owner_centroid()
    labels_path = (
        env.journal
        / "chronicle"
        / "20240101"
        / "test"
        / "090000_300"
        / "talents"
        / "speaker_labels.json"
    )
    labels_data = json.loads(labels_path.read_text(encoding="utf-8"))
    for label in labels_data["labels"]:
        label["method"] = "user_corrected"
    labels_path.write_text(json.dumps(labels_data) + "\n", encoding="utf-8")

    second = rebuild_owner_centroid()

    assert first["status"] == "rebuilt"
    assert second["status"] == "unchanged"
    assert second["reason"] == "evidence_hash_match"


def test_rebuild_five_tuple_hash_changes_when_evidence_row_set_changes(speakers_env):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    _write_rebuild_owner_centroid(env, evidence_hash=None)
    _seed_rebuild_evidence(env)
    first = rebuild_owner_centroid()
    _seed_rebuild_evidence(
        env,
        count=1,
        day="20240102",
        segment_key="100000_300",
    )

    second = rebuild_owner_centroid()

    assert first["status"] == "rebuilt"
    assert second["status"] == "rebuilt"
    assert second["evidence_hash"] != first["evidence_hash"]


def test_concurrent_rebuild_same_evidence_second_reports_unchanged(speakers_env):
    env = speakers_env()
    owner_path = _write_rebuild_owner_centroid(env, evidence_hash=None)
    _seed_rebuild_evidence(env)
    ctx = multiprocessing.get_context("spawn")
    barrier = ctx.Barrier(2)
    results = ctx.Queue()
    errors = ctx.Queue()
    processes = [
        ctx.Process(
            target=_rebuild_owner_worker,
            args=(str(env.journal), barrier, results, errors),
        )
        for _ in range(2)
    ]

    for process in processes:
        process.start()
    for process in processes:
        process.join(timeout=10)
    for process in processes:
        if process.is_alive():
            process.terminate()
            process.join(timeout=2)

    error_text = "\n".join(_drain_queue(errors))
    statuses = _drain_queue(results)
    assert all(not process.is_alive() for process in processes), error_text
    assert all(process.exitcode == 0 for process in processes), error_text
    assert error_text == ""
    assert sorted(statuses) == ["rebuilt", "unchanged"]
    with np.load(owner_path, allow_pickle=False) as data:
        assert data["centroid"].shape == (256,)
        assert int(np.asarray(data["cluster_size"]).item()) == 30
        assert str(data["evidence_hash"].item())


def test_rebuild_unchanged_evidence_skips_native_write(speakers_env, monkeypatch):
    from solstone.apps.speakers import owner as owner_module
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    owner_path = _write_rebuild_owner_centroid(env, evidence_hash=None)
    _seed_rebuild_evidence(env)
    rebuild_owner_centroid()
    native_calls = []

    def capture_native_write(*args, **kwargs):
        native_calls.append((args, kwargs))
        raise AssertionError("unchanged evidence must not request a native write")

    monkeypatch.setattr(
        owner_module.native_speakers,
        "rebuild_owner_centroid",
        capture_native_write,
    )

    result = rebuild_owner_centroid()

    assert result["status"] == "unchanged"
    assert native_calls == []
    assert owner_path.exists()


def test_first_rebuild_seeds_created_at_from_existing_last_refreshed_at_under_lock(
    speakers_env,
):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    _write_rebuild_owner_centroid(
        env,
        evidence_hash=None,
        created_at=None,
        last_refreshed_at="2026-01-01T00:00:00Z",
    )
    _seed_rebuild_evidence(env)

    result = rebuild_owner_centroid()

    assert result["status"] == "rebuilt"
    assert result["created_at"] == "2026-01-01T00:00:00Z"


def test_rebuild_preserves_created_at_on_later_rebuilds(speakers_env):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    _write_rebuild_owner_centroid(env, created_at="2026-01-01T00:00:00Z")
    _seed_rebuild_evidence(env)

    result = rebuild_owner_centroid()

    assert result["status"] == "rebuilt"
    assert result["created_at"] == "2026-01-01T00:00:00Z"


def test_rebuild_response_reports_streams_represented_from_evidence_rows(speakers_env):
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    _write_rebuild_owner_centroid(env, evidence_hash=None)
    _seed_rebuild_evidence(env, count=15, stream="mic", segment_key="090000_300")
    _seed_rebuild_evidence(env, count=15, stream="sys", segment_key="091000_300")

    result = rebuild_owner_centroid()

    assert result["status"] == "rebuilt"
    assert result["streams_represented"] == 2


def test_rebuild_superseded_scan_uses_30_day_window(speakers_env, monkeypatch):
    from solstone.apps.speakers import owner as owner_module
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    _write_rebuild_owner_centroid(env, evidence_hash=None)
    _seed_rebuild_evidence(env, day="20240131", segment_key="090000_300")
    for index in range(31):
        day = f"202401{index + 1:02d}"
        seg_dir = env.create_segment(day, "120000_300", ["audio"])
        talents = seg_dir / "talents"
        talents.mkdir(parents=True, exist_ok=True)
        (talents / "speaker_labels.json").write_text(
            json.dumps({"labels": [], "owner_centroid_last_refreshed_at": "old"})
            + "\n",
            encoding="utf-8",
        )
    monkeypatch.setattr(owner_module, "_iso_now", lambda: "new-refresh")

    result = rebuild_owner_centroid()

    assert result["status"] == "rebuilt"
    assert result["superseded_labels_window_days"] == 30
    assert result["superseded_labels_window_count"] == 30


def test_rebuild_counts_superseded_unstamped_and_errors_separately(
    speakers_env, monkeypatch
):
    from solstone.apps.speakers import owner as owner_module
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    _write_rebuild_owner_centroid(env, evidence_hash=None)
    _seed_rebuild_evidence(env, day="20240104", segment_key="090000_300")
    stamps = {
        "20240101": {"owner_centroid_last_refreshed_at": "old"},
        "20240102": {"owner_centroid_last_refreshed_at": None},
        "20240103": {"owner_centroid_last_refreshed_at": "new-refresh"},
    }
    for day, metadata in stamps.items():
        seg_dir = env.create_segment(day, "120000_300", ["audio"])
        talents = seg_dir / "talents"
        talents.mkdir(parents=True, exist_ok=True)
        payload = {"labels": [], **metadata}
        (talents / "speaker_labels.json").write_text(
            json.dumps(payload) + "\n",
            encoding="utf-8",
        )
    bad_dir = env.create_segment("20240105", "120000_300", ["audio"])
    (bad_dir / "talents").mkdir(parents=True, exist_ok=True)
    (bad_dir / "talents" / "speaker_labels.json").write_text("{", encoding="utf-8")
    monkeypatch.setattr(owner_module, "_iso_now", lambda: "new-refresh")

    result = rebuild_owner_centroid()

    assert result["status"] == "rebuilt"
    assert result["superseded_labels_window_count"] == 1
    assert result["unstamped_labels_window_count"] == 2
    assert result["superseded_labels_window_error_count"] == 1


def test_check_owner_contamination_uses_rebuilt_owner_centroid(speakers_env):
    from solstone.apps.speakers.owner import rebuild_owner_centroid
    from solstone.apps.speakers.routes import _check_owner_contamination

    env = speakers_env()
    _write_rebuild_owner_centroid(
        env,
        centroid=np.array([0.0, 1.0] + [0.0] * 254, dtype=np.float32),
        evidence_hash=None,
    )
    _seed_rebuild_evidence(env)

    result = rebuild_owner_centroid(override=True)

    assert result["status"] == "rebuilt"
    assert _check_owner_contamination(
        _normalized(np.array([1.0] + [0.0] * 255, dtype=np.float32))
    )


def test_accumulate_voiceprints_uses_rebuilt_owner_centroid_for_subtraction(
    speakers_env,
):
    from solstone.apps.speakers.attribution import accumulate_voiceprints
    from solstone.apps.speakers.owner import rebuild_owner_centroid

    env = speakers_env()
    _write_rebuild_owner_centroid(env, evidence_hash=None)
    _seed_rebuild_evidence(env)
    rebuild_owner_centroid()
    env.create_entity("Alice Test")
    owner_embedding = _normalized(np.array([1.0] + [0.0] * 255, dtype=np.float32))
    _write_segment(
        env.journal,
        "20240102",
        "test",
        "100000_300",
        "audio",
        owner_embedding.reshape(1, -1),
        durations_s=np.array([2.0], dtype=np.float32),
    )
    env.create_speaker_labels(
        "20240102",
        "100000_300",
        [{"sentence_id": 1, "speaker": "alice_test", "confidence": "high"}],
    )

    saved = accumulate_voiceprints(
        "20240102",
        "test",
        "100000_300",
        [{"sentence_id": 1, "speaker": "alice_test", "confidence": "high"}],
        "audio",
    )

    assert saved == {}


def test_provisional_owner_guard_stays_disabled_after_rebuild(speakers_env):
    from solstone.apps.speakers.owner import (
        load_owner_provisional_centroid,
        rebuild_owner_centroid,
    )

    env = speakers_env()
    _write_rebuild_owner_centroid(env, evidence_hash=None)
    _seed_rebuild_evidence(env)

    rebuild_owner_centroid()

    assert load_owner_provisional_centroid("self_person") is None


def test_reject_owner_candidate_cooldown_flow_unchanged(speakers_env):
    from solstone.apps.speakers.owner import (
        owner_detection_ready,
        reject_owner_candidate,
    )

    env = speakers_env()
    candidate_path = _candidate_path(env.journal)
    candidate_path.parent.mkdir(parents=True, exist_ok=True)
    candidate_path.write_bytes(b"test")

    result = reject_owner_candidate()
    ready = owner_detection_ready()

    assert result["status"] == "rejected"
    assert not candidate_path.exists()
    assert ready["ready"] is False
    assert ready["reason"] == "cooldown"


def test_load_owner_centroid_no_principal(speakers_env):
    from solstone.apps.speakers.owner import load_owner_centroid

    speakers_env()
    assert load_owner_centroid() is None


def test_load_owner_centroid_no_file(speakers_env):
    from solstone.apps.speakers.owner import load_owner_centroid

    env = speakers_env()
    env.create_entity("Self Person", is_principal=True)

    assert load_owner_centroid() is None


def test_load_owner_centroid_success(speakers_env):
    from solstone.apps.speakers.owner import OWNER_THRESHOLD, load_owner_centroid

    env = speakers_env()
    principal_dir = env.create_entity("Self Person", is_principal=True)
    centroid = _normalized(np.array([1.0] + [0.0] * 255, dtype=np.float32))
    np.savez_compressed(
        principal_dir / "owner_centroid.npz",
        centroid=centroid,
        cluster_size=np.array(60, dtype=np.int32),
        threshold=np.array(OWNER_THRESHOLD, dtype=np.float32),
        last_refreshed_at=np.array("2026-03-15T12:00:00Z"),
    )

    loaded = load_owner_centroid()

    assert loaded is not None
    assert np.allclose(loaded.centroid, centroid)
    assert np.isclose(loaded.threshold, OWNER_THRESHOLD)
    assert loaded.cluster_size == 60
    assert loaded.last_refreshed_at == "2026-03-15T12:00:00Z"
    assert loaded.intra_cosine_p25 is None
    assert loaded.streams == []


def test_owner_detection_ready_not_ready_when_centroid_exists(speakers_env):
    from solstone.apps.speakers.encoder_config import OWNER_THRESHOLD
    from solstone.apps.speakers.owner import owner_detection_ready

    env = speakers_env()
    principal_dir = env.create_entity("Self Person", is_principal=True)
    centroid = _normalized(np.array([1.0] + [0.0] * 255, dtype=np.float32))
    np.savez_compressed(
        principal_dir / "owner_centroid.npz",
        centroid=centroid,
        cluster_size=np.array(60, dtype=np.int32),
        threshold=np.array(OWNER_THRESHOLD, dtype=np.float32),
        last_refreshed_at=np.array("2026-03-15T12:00:00Z"),
    )

    result = owner_detection_ready()

    assert result["ready"] is False
    assert result["reason"] == "centroid_exists"


def test_owner_detection_ready_not_ready_during_cooldown(speakers_env, monkeypatch):
    from datetime import datetime

    from solstone.apps.speakers import owner as owner_module
    from solstone.apps.speakers.owner import owner_detection_ready

    speakers_env()
    update_state("voiceprint", {"rejected_at": datetime.now().isoformat()})
    monkeypatch.setattr(
        owner_module,
        "detect_owner_candidate",
        lambda: (_ for _ in ()).throw(AssertionError("called detection")),
    )

    result = owner_detection_ready()

    assert result["ready"] is False
    assert result["reason"] == "cooldown"
    assert result["days_remaining"] == 14


def test_owner_detection_ready_reads_persisted_candidate(speakers_env, monkeypatch):
    from solstone.apps.speakers import owner as owner_module
    from solstone.apps.speakers.copy import OWNER_CANDIDATE_CONFIRM_GUIDANCE
    from solstone.apps.speakers.encoder_config import OWNER_THRESHOLD
    from solstone.apps.speakers.owner import owner_detection_ready

    env = speakers_env()
    candidate_path = _candidate_path(env.journal)
    candidate_path.parent.mkdir(parents=True, exist_ok=True)
    np.savez_compressed(
        candidate_path,
        centroid=_normalized(np.array([1.0] + [0.0] * 255, dtype=np.float32)),
        cluster_size=np.array(40, dtype=np.int32),
        threshold=np.array(OWNER_THRESHOLD, dtype=np.float32),
        version=np.array("2026-03-19T12:00:00Z"),
    )
    update_state(
        "voiceprint",
        {
            "status": "candidate",
            "cluster_size": 40,
            "streams_represented": 2,
            "recommendation": "ready",
            "samples": [{"day": "20240101"}],
        },
    )
    monkeypatch.setattr(
        owner_module,
        "detect_owner_candidate",
        lambda: (_ for _ in ()).throw(AssertionError("called detection")),
    )

    result = owner_detection_ready()

    assert result["ready"] is True
    assert result["reason"] == "candidate_found"
    assert result["cluster_size"] == 40
    assert result["streams_represented"] == 2
    assert result["samples"] == [{"day": "20240101"}]
    assert result["candidate_available"] is True
    assert result["recommendation"] == "ready"
    assert result["next_step"] == "confirm_candidate"
    assert result["guidance"] == OWNER_CANDIDATE_CONFIRM_GUIDANCE


def test_owner_detection_ready_preserves_single_stream_not_ready(
    speakers_env, monkeypatch
):
    from solstone.apps.speakers import owner as owner_module
    from solstone.apps.speakers.copy import OWNER_CANDIDATE_CONFIRM_GUIDANCE
    from solstone.apps.speakers.encoder_config import OWNER_THRESHOLD
    from solstone.apps.speakers.owner import owner_detection_ready

    env = speakers_env()
    candidate_path = _candidate_path(env.journal)
    candidate_path.parent.mkdir(parents=True, exist_ok=True)
    np.savez_compressed(
        candidate_path,
        centroid=_normalized(np.array([1.0] + [0.0] * 255, dtype=np.float32)),
        cluster_size=np.array(40, dtype=np.int32),
        threshold=np.array(OWNER_THRESHOLD, dtype=np.float32),
        version=np.array("2026-03-19T12:00:00Z"),
    )
    update_state(
        "voiceprint",
        {
            "status": "candidate",
            "cluster_size": 40,
            "streams_represented": 1,
            "recommendation": "single_stream",
            "samples": [],
        },
    )
    monkeypatch.setattr(
        owner_module,
        "detect_owner_candidate",
        lambda: (_ for _ in ()).throw(AssertionError("called detection")),
    )

    result = owner_detection_ready()

    assert result["ready"] is False
    assert result["reason"] == "single_stream"
    assert result["candidate_available"] is True
    assert result["cluster_size"] == 40
    assert result["streams_represented"] == 1
    assert result["samples"] == []
    assert result["recommendation"] == "single_stream"
    assert result["next_step"] == "confirm_candidate"
    assert result["guidance"] == OWNER_CANDIDATE_CONFIRM_GUIDANCE


def test_owner_candidate_payload_matches_status_section(speakers_env, monkeypatch):
    from solstone.apps.speakers import owner as owner_module
    from solstone.apps.speakers.encoder_config import OWNER_THRESHOLD
    from solstone.apps.speakers.owner import owner_detection_ready
    from solstone.apps.speakers.status import _owner_section

    env = speakers_env()
    candidate_path = _candidate_path(env.journal)
    candidate_path.parent.mkdir(parents=True, exist_ok=True)
    np.savez_compressed(
        candidate_path,
        centroid=_normalized(np.array([1.0] + [0.0] * 255, dtype=np.float32)),
        cluster_size=np.array(40, dtype=np.int32),
        threshold=np.array(OWNER_THRESHOLD, dtype=np.float32),
        version=np.array("2026-03-19T12:00:00Z"),
    )
    update_state(
        "voiceprint",
        {
            "status": "candidate",
            "cluster_size": 40,
            "streams_represented": 1,
            "recommendation": "single_stream",
            "samples": [],
        },
    )
    monkeypatch.setattr(
        owner_module,
        "detect_owner_candidate",
        lambda: (_ for _ in ()).throw(AssertionError("called detection")),
    )

    ready = owner_detection_ready()
    status = _owner_section()

    assert ready["candidate_available"] == status["candidate_available"] is True
    assert ready["next_step"] == status["next_step"] == "confirm_candidate"
    assert ready["guidance"] == status["guidance"]


def test_owner_detection_ready_no_candidate_data(speakers_env, monkeypatch):
    from solstone.apps.speakers import owner as owner_module
    from solstone.apps.speakers.owner import owner_detection_ready

    speakers_env()
    monkeypatch.setattr(
        owner_module,
        "detect_owner_candidate",
        lambda: (_ for _ in ()).throw(AssertionError("called detection")),
    )

    result = owner_detection_ready()

    assert result == {"ready": False, "reason": "no_candidate"}


def test_owner_detection_ready_cooldown_expired_allows_candidate(
    speakers_env, monkeypatch
):
    from datetime import datetime, timedelta

    from solstone.apps.speakers import owner as owner_module
    from solstone.apps.speakers.encoder_config import OWNER_THRESHOLD
    from solstone.apps.speakers.owner import owner_detection_ready

    env = speakers_env()
    candidate_path = _candidate_path(env.journal)
    candidate_path.parent.mkdir(parents=True, exist_ok=True)
    np.savez_compressed(
        candidate_path,
        centroid=_normalized(np.array([1.0] + [0.0] * 255, dtype=np.float32)),
        cluster_size=np.array(40, dtype=np.int32),
        threshold=np.array(OWNER_THRESHOLD, dtype=np.float32),
        version=np.array("2026-03-19T12:00:00Z"),
    )
    update_state(
        "voiceprint",
        {
            "status": "candidate",
            "rejected_at": (datetime.now() - timedelta(days=15)).isoformat(),
            "cluster_size": 40,
            "streams_represented": 2,
            "recommendation": "ready",
            "samples": [],
        },
    )
    monkeypatch.setattr(
        owner_module,
        "detect_owner_candidate",
        lambda: (_ for _ in ()).throw(AssertionError("called detection")),
    )

    result = owner_detection_ready()

    assert result["ready"] is True


def test_classify_sentences_no_centroid(speakers_env):
    from solstone.apps.speakers.owner import classify_sentences

    env = speakers_env()
    env.create_segment("20240101", "090000_300", ["audio"], num_sentences=2)

    assert classify_sentences("20240101", "test", "090000_300", "audio") == []


def test_classify_sentences_with_centroid(speakers_env):
    from solstone.apps.speakers.owner import OWNER_THRESHOLD, classify_sentences

    env = speakers_env()
    principal_dir = env.create_entity("Self Person", is_principal=True)
    centroid = _normalized(np.array([1.0] + [0.0] * 255, dtype=np.float32))
    np.savez_compressed(
        principal_dir / "owner_centroid.npz",
        centroid=centroid,
        cluster_size=np.array(70, dtype=np.int32),
        threshold=np.array(OWNER_THRESHOLD, dtype=np.float32),
        last_refreshed_at=np.array("2026-03-15T12:00:00Z"),
    )

    close = _normalized(np.array([0.95, 0.05] + [0.0] * 254, dtype=np.float32))
    far = _normalized(np.array([0.1, 0.99] + [0.0] * 254, dtype=np.float32))
    _write_segment(
        env.journal,
        "20240101",
        "mic",
        "090000_300",
        "audio",
        np.vstack([close, far]),
    )

    results = classify_sentences("20240101", "mic", "090000_300", "audio")

    assert len(results) == 2
    assert results[0]["sentence_id"] == 1
    assert results[0]["is_owner"] is True
    assert results[1]["sentence_id"] == 2
    assert results[1]["is_owner"] is False


def test_api_owner_status_none(speakers_env):
    from solstone.apps.speakers.routes import speakers_bp

    speakers_env()
    app = Flask(__name__)
    app.register_blueprint(speakers_bp)

    with app.test_client() as client:
        response = client.get("/app/speakers/api/owner/status")

    data = response.get_json()
    assert response.status_code == 200
    assert set(data) == {
        "status",
        "manual_tags_count",
        "segments_available",
        "segments_with_embeddings",
        "embeddings_available",
        "streams_represented",
        "can_build_from_tags",
        "next_step",
        "guidance",
    }
    assert data["status"] == "none"
    assert data["manual_tags_count"] == 0
    assert data["segments_available"] == 0
    assert data["segments_with_embeddings"] == 0
    assert data["embeddings_available"] == 0
    assert data["streams_represented"] == 0
    assert data["can_build_from_tags"] is False
    assert data["next_step"] == "seed_manual_tags"
    assert data["guidance"]


def test_api_owner_status_needs_detection(speakers_env):
    from solstone.apps.speakers.copy import OWNER_DETECT_CANDIDATE_GUIDANCE
    from solstone.apps.speakers.routes import speakers_bp

    env = speakers_env()
    for idx in range(50):
        env.create_segment(
            "20240101", f"{idx // 12 + 9:02d}{(idx % 12) * 5:02d}00_300", ["audio"]
        )

    app = Flask(__name__)
    app.register_blueprint(speakers_bp)

    with app.test_client() as client:
        response = client.get("/app/speakers/api/owner/status")

    data = response.get_json()
    assert response.status_code == 200
    assert data["status"] == "needs_detection"
    assert data["segments_with_embeddings"] == 50
    assert data["segments_available"] == 50
    assert data["embeddings_available"] == 250
    assert data["manual_tags_count"] == 0
    assert data["streams_represented"] == 0
    assert data["can_build_from_tags"] is False
    assert data["next_step"] == "detect_candidate"
    assert data["guidance"] == OWNER_DETECT_CANDIDATE_GUIDANCE


def test_api_owner_status_manual_tags_count(speakers_env):
    from solstone.apps.speakers.copy import OWNER_DETECT_CANDIDATE_GUIDANCE
    from solstone.apps.speakers.routes import speakers_bp

    env = speakers_env()
    env.create_entity("Self Person", is_principal=True)
    embeddings = np.zeros((7, 256), dtype=np.float32)
    embeddings[:, 0] = 1.0
    _save_manual_owner_tags(
        env,
        "self_person",
        "20240101",
        "090000_300",
        embeddings,
        durations_s=np.full(7, 2.0, dtype=np.float32),
    )

    app = Flask(__name__)
    app.register_blueprint(speakers_bp)

    with app.test_client() as client:
        response = client.get("/app/speakers/api/owner/status")

    data = response.get_json()
    assert response.status_code == 200
    assert data["status"] == "needs_detection"
    assert data["manual_tags_count"] == 7
    assert data["segments_available"] == 1
    assert data["segments_with_embeddings"] == 1
    assert data["embeddings_available"] == 7
    assert data["streams_represented"] == 1
    assert data["can_build_from_tags"] is False
    assert data["next_step"] == "detect_candidate"
    assert data["guidance"] == OWNER_DETECT_CANDIDATE_GUIDANCE


def test_api_owner_status_candidate(speakers_env):
    from solstone.apps.speakers.routes import speakers_bp

    speakers_env()
    update_state(
        "voiceprint",
        {
            "status": "candidate",
            "cluster_size": 55,
            "samples": [{"day": "20240101"}],
        },
    )
    app = Flask(__name__)
    app.register_blueprint(speakers_bp)

    with app.test_client() as client:
        response = client.get("/app/speakers/api/owner/status")

    assert response.status_code == 200
    assert response.get_json()["status"] == "candidate"


def test_api_owner_status_low_quality(speakers_env):
    from solstone.apps.speakers.encoder_config import OWNER_BOOTSTRAP_MIN_STMTS
    from solstone.apps.speakers.routes import speakers_bp

    speakers_env()
    update_state(
        "voiceprint",
        {
            "status": "low_quality",
            "low_quality_reason": "too_few_stmts",
            "observed_value": 5,
            "threshold_value": OWNER_BOOTSTRAP_MIN_STMTS,
            "segments_checked": 1,
            "attempted_at": "2026-03-15T12:00:00",
        },
    )
    app = Flask(__name__)
    app.register_blueprint(speakers_bp)

    with app.test_client() as client:
        response = client.get("/app/speakers/api/owner/status")

    assert response.status_code == 200
    assert response.get_json() == {
        "status": "low_quality",
        "source": "candidate_pool",
        "low_quality_reason": "too_few_stmts",
        "observed_value": 5,
        "threshold_value": OWNER_BOOTSTRAP_MIN_STMTS,
        "evidence_tier": None,
        "intra_cosine_p25_bound": None,
        "manual_tags_count": 0,
        "segments_available": 0,
        "segments_with_embeddings": 0,
        "embeddings_available": 0,
        "streams_represented": 0,
        "can_build_from_tags": False,
        "next_step": "seed_manual_tags",
        "guidance": (
            "Use sol call speakers tag-owner <day> <stream> <segment> "
            "<source> <sentence-id> on owner sentences in raw media until "
            f"you have {OWNER_BOOTSTRAP_MIN_STMTS} validated owner tags; "
            f"{OWNER_BOOTSTRAP_MIN_STMTS} more needed. Then run "
            "sol call speakers build-from-tags."
        ),
    }


def test_owner_gate_diagnostics_falls_back_when_renderer_declines_under_node():
    node = _node_or_skip()
    source = SPEAKERS_WORKSPACE.read_text(encoding="utf-8")
    functions = "\n".join(
        _workspace_function_source(source, name)
        for name in (
            "ownerTeachMomentumCount",
            "renderOwnerColdStartDiagnostics",
            "renderOwnerGateDiagnostics",
        )
    )
    script = "\n".join(
        [
            "const window = {};",
            r"""
function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function escapeHtml(value) {
  return String(value ?? '').replace(/[&<>"']/g, (char) => ({
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#39;'
  })[char]);
}

function formatOwnerMetric(value) {
  const number = Number(value);
  if (!Number.isFinite(number)) return 'n/a';
  return Number.isInteger(number) ? String(number) : number.toFixed(2);
}
""",
            functions,
            r"""
const lowQualityDefault = {
  status: 'low_quality',
  source: 'candidate_pool',
  low_quality_reason: '',
  observed_value: 0,
  threshold_value: 0,
  manual_tags_count: 7,
  segments_available: 8,
  embeddings_available: 9,
  can_build_from_tags: true,
};

window.GateDrawer = { render() { return ''; } };
const fallback = renderOwnerGateDiagnostics(lowQualityDefault);
assert(fallback.includes('<details class="spk-owner-diagnostics">'), 'fallback diagnostics render');
assert(!fallback.includes('id="spkOwnerBuildFromTags"'), 'fallback omits build-from-tags button');
assert(fallback.includes(String(lowQualityDefault.manual_tags_count)), 'fallback keeps manual tag count');
assert(fallback.includes(String(lowQualityDefault.segments_available)), 'fallback keeps segment count');
assert(fallback.includes(String(lowQualityDefault.embeddings_available)), 'fallback keeps embedding count');

window.GateDrawer = {
  render(data, options) {
    assert(options === undefined, 'drawer receives no action html');
    return '<details class="drawer">diagnostics</details>';
  }
};
const claimed = renderOwnerGateDiagnostics({
  ...lowQualityDefault,
  low_quality_reason: 'too_few_stmts',
});
assert(claimed.includes('class="drawer"'), 'claimed drawer renders');
assert(!claimed.includes('spk-owner-diagnostics'), 'claimed drawer does not duplicate cold-start details');
assert(!claimed.includes('id="spkOwnerBuildFromTags"'), 'claimed drawer omits build-from-tags button');
""",
        ]
    )

    subprocess.run([node, "-e", script], check=True, text=True)


def test_owner_teach_pool_order_resume_and_count_consistency_under_node(speakers_env):
    from solstone.apps.speakers.routes import speakers_bp

    node = _node_or_skip()
    env = speakers_env()
    embeddings = np.eye(3, 256, dtype=np.float32)
    _write_segment(
        env.journal,
        "20240101",
        "mic",
        "090000_300",
        "audio",
        embeddings,
        durations_s=np.array([1.0, 9.0, 4.0], dtype=np.float32),
    )
    app = Flask(__name__)
    app.register_blueprint(speakers_bp)
    with app.test_client() as client:
        response = client.get("/app/speakers/api/review/20240101/mic/090000_300/audio")
    assert response.status_code == 200
    review_payload = response.get_json()
    review_payload["day"] = "20240101"
    review_payload["stream"] = "mic"

    source = SPEAKERS_WORKSPACE.read_text(encoding="utf-8")
    functions = "\n".join(
        _workspace_function_source(source, name)
        for name in (
            "ownerTeachMomentumCount",
            "ownerTeachTemplateText",
            "ownerTeachProgressText",
            "ownerTeachExhaustedBody",
            "ownerTeachStatementKey",
            "ownerTeachEligibleStatements",
            "ownerTeachOrderStatements",
            "ownerTeachPoolCounts",
            "ownerTeachSegmentsApiUrl",
            "ownerTeachSegmentPageState",
        )
    )
    script = "\n".join(
        [
            r"""
function assert(condition, message) {
  if (!condition) throw new Error(message);
}
""",
            functions,
            f"const fixturePayload = {json.dumps(review_payload)};",
            r"""
const copy = {
  SPK_OWNER_TEACH_PROGRESS_TEMPLATE: '{count} of {minimum} longer statements',
  SPK_OWNER_TEACH_EXHAUSTED_BODY_TEMPLATE: '{count} of {minimum} longer statements taught',
};
const state = { manual_tags_count: 7 };
const progress = ownerTeachProgressText(state, copy, 30);
const exhausted = ownerTeachExhaustedBody(state, copy, 30);
assert(ownerTeachMomentumCount(state) === 7, 'momentum count reads manual_tags_count');
assert(progress.startsWith('7 of 30'), 'progress uses momentum count');
assert(exhausted.startsWith('7 of 30'), 'exhausted uses same momentum count');

const syntheticPayload = {
  day: '20240101',
  stream: 'mic',
  segment: { key: '090000_300', start: '09:00', end: '09:05' },
  source: 'audio',
  audio_file: '/audio.flac',
  audio_mimetype: 'audio/flac',
  sentences: [
    { id: 1, text: 'first', has_embedding: true, speaker_entity_id: null, duration_s: 1.0 },
    { id: 2, text: 'second', has_embedding: true, speaker_entity_id: null, duration_s: 9.0 },
    { id: 3, text: 'third', has_embedding: true, speaker_entity_id: null, duration_s: 4.0 },
    { id: 4, text: 'tagged', has_embedding: true, speaker_entity_id: 'alice', duration_s: 8.0 },
    { id: 5, text: 'missing', has_embedding: false, speaker_entity_id: null, duration_s: 20.0 },
  ],
};
const declined = new Set(['20240101/mic/090000_300/audio/3']);
const eligible = ownerTeachEligibleStatements(syntheticPayload, declined);
assert(eligible.map(item => item.id).join(',') === '1,2', 'eligibility filters missing, tagged, and declined statements');

const allItems = ownerTeachEligibleStatements(fixturePayload, new Set());
const ordered = ownerTeachOrderStatements(allItems, 0).map(item => item.id);
assert(ordered.join(',') === '2,3,1', 'duration order wins');
assert(ordered.join(',') !== '1,2,3', 'transcript order would fail');
const resumed = ownerTeachOrderStatements(allItems, 1).map(item => item.id);
assert(resumed.join(',') === '3,1,2', 'manual tag count rotates starting statement');
assert(resumed[0] !== ordered[0], 'fresh page resumes from persisted tag count');
const unknownLast = ownerTeachOrderStatements([
  { id: 10, day: '20240101', stream: 'mic', segment_key: '090000_300', source: 'audio', duration_s: 5 },
  { id: 11, day: '20240101', stream: 'mic', segment_key: '090000_300', source: 'audio' },
], 0).map(item => item.id);
assert(unknownLast.join(',') === '10,11', 'unknown duration sorts last');

const counts = ownerTeachPoolCounts([
  { items: [allItems[0], allItems[1]], failed_count: 1 },
  { items: [allItems[2]], failed_count: 0 },
]);
assert(counts.eligible_count === 3, 'pool counts eligible rows');
assert(counts.failed_count === 1, 'pool counts failed reviews');
assert(!('unavailable_count' in counts), 'pool counts do not report a dead unavailable branch');

globalThis.speakerFilter = 'alice_test';
const poolUrl = ownerTeachSegmentsApiUrl('20240101', 20, 0);
assert(poolUrl === '/app/speakers/api/segments/20240101?limit=20&offset=0', 'owner teach segment URL is unfiltered');
assert(!poolUrl.includes('speaker='), 'active speaker filter does not shrink owner teach pool');

let pageState = ownerTeachSegmentPageState([], {
  total: 25,
  segments: Array.from({ length: 20 }, (_, index) => ({ key: `seg-${index}` })),
});
assert(pageState.segments.length === 20, 'first page is retained');
assert(pageState.nextOffset === 20, 'next offset advances by collected rows');
assert(pageState.done === false, 'pool continues past the view page size');
pageState = ownerTeachSegmentPageState(pageState.segments, {
  total: 25,
  segments: Array.from({ length: 5 }, (_, index) => ({ key: `tail-${index}` })),
});
assert(pageState.segments.length === 25, 'pool covers all pages');
assert(pageState.done === true, 'pool stops when total is reached');
""",
        ]
    )

    subprocess.run([node, "-e", script], check=True, text=True)


def test_owner_teach_reveal_latch_and_refusal_classification_under_node():
    node = _node_or_skip()
    source = SPEAKERS_WORKSPACE.read_text(encoding="utf-8")
    functions = "\n".join(
        _workspace_function_source(source, name)
        for name in (
            "ownerTeachMomentumCount",
            "ownerTeachTemplateText",
            "ownerRevealStatementText",
            "ownerRevealFactTexts",
            "ownerRevealDecision",
            "classifyOwnerTeachResult",
            "ownerTeachApplyResult",
        )
    )
    script = "\n".join(
        [
            r"""
function assert(condition, message) {
  if (!condition) throw new Error(message);
}
""",
            functions,
            r"""
const copy = {
  SPK_OWNER_REVEAL_STATEMENTS_TEMPLATE: '{count} statements taught',
  SPK_OWNER_REVEAL_STREAMS_TEMPLATE: '{count} places heard',
  SPK_OWNER_REVEAL_EVIDENCE_TEMPLATE: '{tier} evidence',
  SPK_OWNER_TEACH_BUSY: 'busy',
  SPK_OWNER_TEACH_FAILED: 'failed',
};
const confirmed = {
  status: 'confirmed',
  manual_tags_count: 30,
  centroid_metadata: { streams: ['mic', 'sys'], evidence_tier: 'standard' },
};
const first = ownerRevealDecision(false, 'manual_tags', 'confirmed');
const second = ownerRevealDecision(first.nextLatchShown, 'manual_tags', 'confirmed');
assert(first.show === true, 'first confirmed transition shows reveal');
assert(second.show === false, 'second in-flight status response does not show reveal');
assert(ownerRevealDecision(false, null, 'confirmed').show === false, 'confirmed first load does not show reveal');

const manualFacts = ownerRevealFactTexts(confirmed, 'manual_tags', copy);
const candidateFacts = ownerRevealFactTexts(confirmed, 'candidate', copy);
assert(manualFacts.join('|') === '30 statements taught', 'manual reveal uses statements taught');
assert(candidateFacts.join('|') === '2 places heard|standard evidence', 'candidate reveal uses stream and evidence facts');
assert(!candidateFacts.join('|').includes('statements taught'), 'candidate reveal omits statements taught');

const refused = classifyOwnerTeachResult(null, 'refused');
const busy = classifyOwnerTeachResult(null, 'busy');
const failed = classifyOwnerTeachResult(null, 'failed');
const assigned = classifyOwnerTeachResult(null, null);
assert(refused.kind === 'refused', 'low-quality refusal is distinct');
assert(busy.kind === 'busy' && busy.copyKey === 'SPK_OWNER_TEACH_BUSY', 'busy refusal is distinct');
assert(failed.kind === 'failed' && failed.copyKey === 'SPK_OWNER_TEACH_FAILED', 'failed outcome is distinct');

function baseState() {
  return {
    statusData: { manual_tags_count: 7 },
    items: [{ owner_teach_key: 'first' }, { owner_teach_key: 'second' }],
    index: 0,
    notice: '',
    noticeKind: '',
    refusalGuidance: '',
  };
}

[
  [refused, { owner_bootstrap_guidance: 'Use clearer statements.' }],
  [busy, {}],
  [failed, {}],
].forEach(([classification, result]) => {
  const next = ownerTeachApplyResult(baseState(), classification, result, copy);
  assert(ownerTeachMomentumCount(next.statusData) === 7, `${classification.kind} keeps count unchanged`);
  assert(next.index === 0, `${classification.kind} keeps session position unchanged`);
  assert(next.items.map(item => item.owner_teach_key).join(',') === 'first,second', `${classification.kind} keeps current item`);
});

const advanced = ownerTeachApplyResult(baseState(), assigned, {}, copy);
assert(ownerTeachMomentumCount(advanced.statusData) === 7, 'assigned still reads count from status data');
assert(advanced.items.map(item => item.owner_teach_key).join(',') === 'second', 'assigned consumes current item');
""",
        ]
    )

    subprocess.run([node, "-e", script], check=True, text=True)


def test_owner_not_ready_cold_start_diagnostics_retained_workspace_source():
    source = SPEAKERS_WORKSPACE.read_text(encoding="utf-8")

    assert "function renderOwnerColdStartDiagnostics(data)" in source
    assert '<details class="spk-owner-diagnostics">' in source
    assert (
        '<summary class="spk-owner-diagnostics-summary">why not yet?</summary>'
        in source
    )
    assert (
        '<div class="spk-owner-diagnostics-line">source: ${escapeHtml(data.source || '
        "'auto')}</div>"
    ) in source
    assert (
        '<div class="spk-owner-diagnostics-line">Manual tags: '
        "${escapeHtml(String(ownerTeachMomentumCount(data)))}</div>"
    ) in source
    assert (
        '<div class="spk-owner-diagnostics-line">Segments with audio: '
        "${escapeHtml(String(data.segments_available || 0))}</div>"
    ) in source
    assert (
        '<div class="spk-owner-diagnostics-line">Embeddings: '
        "${escapeHtml(String(data.embeddings_available || 0))}</div>"
    ) in source
    cold_start = _workspace_function_source(source, "renderOwnerColdStartDiagnostics")
    gate = _workspace_function_source(source, "renderOwnerGateDiagnostics")
    not_ready = _workspace_function_source(source, "renderOwnerNotReady")
    assert "spkOwnerBuildFromTags" not in cold_start
    assert "actionHtml" not in gate
    assert "spkOwnerBuildFromTags" in not_ready
    assert "data.can_build_from_tags === true" in not_ready


def test_api_owner_status_no_cluster(speakers_env):
    from solstone.apps.speakers.routes import speakers_bp

    speakers_env()
    update_state("voiceprint", {"status": "no_cluster"})
    app = Flask(__name__)
    app.register_blueprint(speakers_bp)

    with app.test_client() as client:
        response = client.get("/app/speakers/api/owner/status")

    data = response.get_json()
    assert response.status_code == 200
    assert data["status"] == "no_cluster"
    assert data["next_step"] == "seed_manual_tags"
    assert data["guidance"]


def test_api_owner_status_fallthrough_has_guidance(speakers_env):
    from solstone.apps.speakers.routes import speakers_bp

    speakers_env()
    update_state("voiceprint", {"status": "unexpected"})
    app = Flask(__name__)
    app.register_blueprint(speakers_bp)

    with app.test_client() as client:
        response = client.get("/app/speakers/api/owner/status")

    data = response.get_json()
    assert response.status_code == 200
    assert data["status"] == "none"
    assert data["next_step"] == "seed_manual_tags"
    assert data["guidance"]


def test_api_owner_status_rejected_cooldown_wins_over_seed_guidance(speakers_env):
    from datetime import datetime

    from solstone.apps.speakers.copy import OWNER_REJECTION_COOLDOWN_GUIDANCE
    from solstone.apps.speakers.routes import speakers_bp

    speakers_env()
    update_state(
        "voiceprint",
        {
            "status": "rejected",
            "rejected_at": datetime.now().isoformat(),
        },
    )
    app = Flask(__name__)
    app.register_blueprint(speakers_bp)

    with app.test_client() as client:
        response = client.get("/app/speakers/api/owner/status")

    data = response.get_json()
    assert response.status_code == 200
    assert data["status"] == "none"
    assert data["reason"] == "cooldown"
    assert data["days_remaining"] == 14
    assert data["next_step"] == "wait_for_cooldown"
    assert data["guidance"] == OWNER_REJECTION_COOLDOWN_GUIDANCE
    assert data["next_step"] != "seed_manual_tags"


def test_api_owner_status_confirmed(speakers_env):
    from solstone.apps.speakers.routes import speakers_bp

    speakers_env()
    update_state("voiceprint", {"status": "confirmed"})
    app = Flask(__name__)
    app.register_blueprint(speakers_bp)

    with app.test_client() as client:
        response = client.get("/app/speakers/api/owner/status")

    assert response.status_code == 200
    assert response.get_json() == {
        "status": "confirmed",
        "manual_tags_count": 0,
        "centroid_metadata": {
            "cluster_size": 0,
            "streams": [],
            "created_at": None,
            "last_refreshed_at": "",
            "threshold": None,
            "margin": None,
            "intra_cosine_p25": None,
            "evidence_hash": None,
            "evidence_intra_cosine_p25": None,
            "evidence_tier": None,
        },
    }


def test_api_owner_status_confirmed_includes_manual_tags_count(speakers_env):
    from solstone.apps.speakers.routes import speakers_bp

    env = speakers_env()
    env.create_entity("Self Person", is_principal=True)
    embeddings = np.zeros((7, 256), dtype=np.float32)
    embeddings[:, 0] = 1.0
    _save_manual_owner_tags(
        env,
        "self_person",
        "20240101",
        "090000_300",
        embeddings,
        durations_s=np.full(7, 2.0, dtype=np.float32),
    )
    update_state("voiceprint", {"status": "confirmed"})
    app = Flask(__name__)
    app.register_blueprint(speakers_bp)

    with app.test_client() as client:
        response = client.get("/app/speakers/api/owner/status")

    assert response.status_code == 200
    assert response.get_json()["manual_tags_count"] == 7


def test_api_owner_classify_no_centroid(speakers_env):
    from solstone.apps.speakers.routes import speakers_bp

    env = speakers_env()
    env.create_segment("20240101", "090000_300", ["audio"], num_sentences=2)
    app = Flask(__name__)
    app.register_blueprint(speakers_bp)

    with app.test_client() as client:
        response = client.post(
            "/app/speakers/api/owner/classify",
            json={
                "day": "20240101",
                "stream": "test",
                "segment_key": "090000_300",
                "source": "audio",
            },
        )

    assert response.status_code == 200
    assert response.get_json() == {"sentences": []}


def test_api_owner_confirm(speakers_env):
    from solstone.apps.speakers.owner import OWNER_THRESHOLD
    from solstone.apps.speakers.routes import speakers_bp

    env = speakers_env()
    principal_dir = env.create_entity("Self Person", is_principal=True)
    candidate_path = _candidate_path(env.journal)
    candidate_path.parent.mkdir(parents=True, exist_ok=True)
    centroid = _normalized(np.array([1.0] + [0.0] * 255, dtype=np.float32))
    np.savez_compressed(
        candidate_path,
        centroid=centroid,
        cluster_size=np.array(88, dtype=np.int32),
        threshold=np.array(OWNER_THRESHOLD, dtype=np.float32),
        version=np.array("2026-03-15T12:00:00"),
    )

    app = Flask(__name__)
    app.register_blueprint(speakers_bp)

    with app.test_client() as client:
        response = client.post("/app/speakers/api/owner/confirm")

    assert response.status_code == 200
    assert response.get_json()["status"] == "confirmed"
    assert not candidate_path.exists()
    assert (principal_dir / "owner_centroid.npz").exists()
    assert get_current()["voiceprint"]["status"] == "confirmed"


def test_api_owner_reject(speakers_env):
    from solstone.apps.speakers.routes import speakers_bp

    env = speakers_env()
    candidate_path = _candidate_path(env.journal)
    candidate_path.parent.mkdir(parents=True, exist_ok=True)
    candidate_path.write_bytes(b"test")

    app = Flask(__name__)
    app.register_blueprint(speakers_bp)

    with app.test_client() as client:
        response = client.post("/app/speakers/api/owner/reject")

    assert response.status_code == 200
    assert response.get_json() == {"status": "needs_detection"}
    assert not candidate_path.exists()
    assert get_current()["voiceprint"]["status"] == "rejected"


def test_api_owner_detect(speakers_env):
    from solstone.apps.speakers.routes import speakers_bp

    env = speakers_env()
    rng = np.random.default_rng(42)
    _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: _owner_embeddings(20, rng)},
        stream="mic",
    )
    _write_labeled_segment(
        env,
        "20240101",
        "091000_300",
        {1: _owner_embeddings(20, rng)},
        stream="sys",
    )
    _write_candidate_pool(
        env.journal,
        [
            _candidate_record(
                1,
                [
                    _source_segment("20240101", "090000_300", stream="mic"),
                    _source_segment("20240101", "091000_300", stream="sys"),
                ],
                n_intervals=40,
            )
        ],
    )

    app = Flask(__name__)
    app.register_blueprint(speakers_bp)

    with app.test_client() as client:
        response = client.post("/app/speakers/api/owner/detect")
        status_response = client.get("/app/speakers/api/owner/status")

    data = response.get_json()
    assert response.status_code == 200
    assert data["status"] == "candidate"
    assert data["cluster_size"] == 40
    assert data["streams_represented"] == 2
    assert data["recommendation"] == "ready"
    assert status_response.status_code == 200
    assert status_response.get_json()["status"] == "candidate"


def test_api_owner_detect_no_pool_does_not_loop_needs_detection(speakers_env):
    from solstone.apps.speakers.routes import speakers_bp

    env = speakers_env()
    _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: _owner_embeddings(40, np.random.default_rng(1))},
        stream="mic",
    )

    app = Flask(__name__)
    app.register_blueprint(speakers_bp)

    with app.test_client() as client:
        first_status = client.get("/app/speakers/api/owner/status")
        detect_response = client.post("/app/speakers/api/owner/detect")
        second_status = client.get("/app/speakers/api/owner/status")

    assert first_status.status_code == 200
    assert first_status.get_json()["status"] == "needs_detection"
    assert detect_response.status_code == 200
    assert detect_response.get_json()["status"] == "no_cluster"
    assert second_status.status_code == 200
    assert second_status.get_json()["status"] != "needs_detection"
    assert second_status.get_json()["status"] == "no_cluster"


def test_api_owner_detect_small_pool_does_not_loop_needs_detection(speakers_env):
    from solstone.apps.speakers.routes import speakers_bp

    env = speakers_env()
    _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: _owner_embeddings(12, np.random.default_rng(1))},
        stream="mic",
    )
    _write_candidate_pool(
        env.journal,
        [
            _candidate_record(
                1,
                [_source_segment("20240101", "090000_300", stream="mic")],
                n_intervals=12,
                n_segments=1,
            )
        ],
    )

    app = Flask(__name__)
    app.register_blueprint(speakers_bp)

    with app.test_client() as client:
        first_status = client.get("/app/speakers/api/owner/status")
        detect_response = client.post("/app/speakers/api/owner/detect")
        second_status = client.get("/app/speakers/api/owner/status")

    assert first_status.status_code == 200
    assert first_status.get_json()["status"] == "needs_detection"
    assert detect_response.status_code == 200
    assert detect_response.get_json()["status"] == "low_quality"
    assert detect_response.get_json()["low_quality_reason"] == "too_few_stmts"
    assert second_status.status_code == 200
    assert second_status.get_json()["status"] != "needs_detection"
    assert second_status.get_json()["status"] == "low_quality"


def test_api_owner_detect_confirmed_centroid_repairs_awareness_no_loop(speakers_env):
    from solstone.apps.speakers.routes import speakers_bp

    env = speakers_env()
    _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: _owner_embeddings(40, np.random.default_rng(1))},
        stream="mic",
    )
    _write_confirmed_owner_centroid(env, cluster_size=60)

    app = Flask(__name__)
    app.register_blueprint(speakers_bp)

    with app.test_client() as client:
        first_status = client.get("/app/speakers/api/owner/status")
        detect_response = client.post("/app/speakers/api/owner/detect")
        second_status = client.get("/app/speakers/api/owner/status")

    assert first_status.status_code == 200
    assert first_status.get_json()["status"] == "needs_detection"
    assert detect_response.status_code == 200
    assert detect_response.get_json()["status"] == "confirmed"
    assert get_current()["voiceprint"]["status"] == "confirmed"
    assert second_status.status_code == 200
    assert second_status.get_json()["status"] != "needs_detection"
    assert second_status.get_json()["status"] == "confirmed"


def test_api_owner_status_does_not_detect_or_materialize_embeddings(
    speakers_env, monkeypatch
):
    from solstone.apps.speakers import owner as owner_module
    from solstone.apps.speakers import routes as speakers_routes
    from solstone.apps.speakers.routes import speakers_bp

    env = speakers_env()
    _write_labeled_segment(
        env,
        "20240101",
        "090000_300",
        {1: _owner_embeddings(40, np.random.default_rng(1))},
        stream="mic",
    )

    def fail_detect():
        raise AssertionError("status called detect_owner_candidate")

    def fail_materialize(*args, **kwargs):
        raise AssertionError("status materialized embedding arrays")

    monkeypatch.setattr(speakers_routes, "detect_owner_candidate", fail_detect)
    monkeypatch.setattr(owner_module, "load_npz", fail_materialize)

    app = Flask(__name__)
    app.register_blueprint(speakers_bp)

    with app.test_client() as client:
        response = client.get("/app/speakers/api/owner/status")

    assert response.status_code == 200
    assert response.get_json()["status"] == "needs_detection"


def test_confirm_owner_candidate_no_candidate(speakers_env):
    from solstone.apps.speakers.owner import confirm_owner_candidate

    speakers_env()
    result = confirm_owner_candidate()
    assert "error" in result
    assert "No candidate" in result["error"]


def test_confirm_owner_candidate_success(speakers_env):
    from solstone.apps.speakers.owner import OWNER_THRESHOLD, confirm_owner_candidate

    env = speakers_env()
    principal_dir = env.create_entity("Self Person", is_principal=True)
    candidate_path = _candidate_path(env.journal)
    candidate_path.parent.mkdir(parents=True, exist_ok=True)
    centroid = _normalized(np.array([1.0] + [0.0] * 255, dtype=np.float32))
    np.savez_compressed(
        candidate_path,
        centroid=centroid,
        cluster_size=np.array(88, dtype=np.int32),
        threshold=np.array(OWNER_THRESHOLD, dtype=np.float32),
        version=np.array("2026-03-19T12:00:00"),
    )

    result = confirm_owner_candidate()

    assert result["status"] == "confirmed"
    assert result["principal_id"] is not None
    assert result["cluster_size"] == 88
    assert not candidate_path.exists()
    assert (principal_dir / "owner_centroid.npz").exists()
    assert get_current()["voiceprint"]["status"] == "confirmed"


def test_reject_owner_candidate(speakers_env):
    from solstone.apps.speakers.owner import reject_owner_candidate

    env = speakers_env()
    candidate_path = _candidate_path(env.journal)
    candidate_path.parent.mkdir(parents=True, exist_ok=True)
    candidate_path.write_bytes(b"test")

    result = reject_owner_candidate()

    assert result["status"] == "rejected"
    assert not candidate_path.exists()
    state = get_current()
    assert state["voiceprint"]["status"] == "rejected"
    assert "rejected_at" in state["voiceprint"]


def test_reject_owner_candidate_enforces_detection_cooldown(speakers_env, monkeypatch):
    from solstone.apps.speakers.owner import (
        owner_detection_ready,
        reject_owner_candidate,
    )

    env = speakers_env()
    candidate_path = _candidate_path(env.journal)
    candidate_path.parent.mkdir(parents=True, exist_ok=True)
    candidate_path.write_bytes(b"test")

    reject_owner_candidate()
    rejected_at = get_current()["voiceprint"]["rejected_at"]
    assert rejected_at.endswith("Z")

    detection_calls = []

    def fake_detect_owner_candidate():
        detection_calls.append(True)
        return {
            "status": "candidate",
            "recommendation": "ready",
            "cluster_size": 88,
            "streams_represented": 2,
            "samples": [],
        }

    monkeypatch.setattr(
        "solstone.apps.speakers.owner.load_owner_centroid",
        lambda: None,
    )
    monkeypatch.setattr(
        "solstone.apps.speakers.owner.detect_owner_candidate",
        fake_detect_owner_candidate,
    )

    result = owner_detection_ready()

    assert result["ready"] is False
    assert result["reason"] == "cooldown"
    assert result["days_remaining"] == 14
    assert detection_calls == []
