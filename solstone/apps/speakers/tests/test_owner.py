# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for owner voice identification."""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np
from flask import Flask

from solstone.apps.speakers.encoder_config import OVERLAP_DETECTOR_ID
from solstone.think.awareness import get_current, update_state


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


def _candidate_path(journal: Path) -> Path:
    return journal / "awareness" / "owner_candidate.npz"


def _normalize_rows(embeddings: np.ndarray) -> np.ndarray:
    norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
    return embeddings / np.where(norms == 0, 1.0, norms)


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
) -> Path:
    from solstone.apps.speakers.routes import _save_voiceprint

    normalized_embeddings = _normalize_rows(np.asarray(embeddings, dtype=np.float32))
    segment_dir = _write_segment(
        env.journal,
        day,
        "test",
        segment_key,
        source,
        normalized_embeddings,
        durations_s=durations_s,
    )
    env.create_speaker_labels(
        day,
        segment_key,
        [
            {
                "sentence_id": idx,
                "speaker": principal_id,
                "confidence": "high",
                "method": method,
            }
            for idx in range(1, len(normalized_embeddings) + 1)
        ],
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
            stream="test",
        )
    return segment_dir


def test_count_segments_with_embeddings(speakers_env):
    from solstone.apps.speakers.owner import count_segments_with_embeddings

    env = speakers_env()
    env.create_segment("20240101", "090000_300", ["mic_audio"])
    env.create_segment("20240101", "091000_300", ["sys_audio"])
    env.create_segment("20240102", "090000_300", ["audio"])

    assert count_segments_with_embeddings() == 3


def test_detect_owner_insufficient_segments(speakers_env):
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    rng = np.random.default_rng(1)
    for idx in range(10):
        _write_segment(
            env.journal,
            "20240101",
            "mic",
            f"{9 + idx:02d}0000_300",
            "audio",
            _owner_embeddings(1, rng),
        )

    result = detect_owner_candidate()
    assert result["status"] == "low_data"
    assert result["segments_available"] == 10
    assert result["embeddings_available"] == 10
    assert result["recommendation"] == "low_data"


def test_detect_owner_no_cluster(speakers_env):
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    for idx in range(50):
        embedding = np.zeros((1, 256), dtype=np.float32)
        embedding[0, idx] = 1.0
        _write_segment(
            env.journal,
            "20240101",
            "mic",
            f"{9 + idx // 12:02d}{(idx % 12) * 5:02d}00_300",
            "audio",
            embedding,
        )

    result = detect_owner_candidate()
    assert result["status"] == "no_clusters"
    assert result["segments_available"] == 50
    assert result["recommendation"] == "no_clusters"
    assert get_current()["voiceprint"]["status"] == "no_cluster"


def test_detect_owner_basic(speakers_env):
    from solstone.apps.speakers.owner import detect_owner_candidate

    env = speakers_env()
    rng = np.random.default_rng(42)

    for idx in range(55):
        hour = 9 + (idx // 12)
        minute = (idx % 12) * 5
        stream = "mic" if idx % 2 == 0 else "sys"
        _write_segment(
            env.journal,
            "20240101",
            stream,
            f"{hour:02d}{minute:02d}00_300",
            "audio",
            _owner_embeddings(2, rng),
        )

    for idx in range(50):
        hour = 9 + (idx // 12)
        minute = (idx % 12) * 5
        stream = "other" if idx % 2 == 0 else "other_sys"
        _write_segment(
            env.journal,
            "20240102",
            stream,
            f"{hour:02d}{minute:02d}00_300",
            "audio",
            _other_cluster_embeddings(2),
        )

    result = detect_owner_candidate()

    assert result is not None
    assert result["status"] == "candidate"
    assert result["cluster_size"] >= 50
    assert result["streams_represented"] == 2
    assert result["recommendation"] == "ready"
    assert len(result["samples"]) == 3
    assert _candidate_path(env.journal).exists()
    assert get_current()["voiceprint"]["status"] == "candidate"


def test_low_quality_too_few_stmts(speakers_env, monkeypatch):
    import solstone.apps.speakers.owner as owner_module
    from solstone.apps.speakers.owner import detect_owner_candidate

    def stub_hdbscan(labels: np.ndarray):
        class StubHDBSCAN:
            def __init__(self, **kwargs):
                self.labels_ = np.asarray(labels, dtype=np.int32)

            def fit(self, embeddings: np.ndarray):
                assert embeddings.shape[0] == len(self.labels_)
                return self

        return StubHDBSCAN

    env = speakers_env()
    rng = np.random.default_rng(0)
    embeddings = _owner_embeddings(60, rng)
    _write_segment(
        env.journal,
        "20240101",
        "mic",
        "090000_300",
        "audio",
        embeddings,
        durations_s=np.full(60, 2.0, dtype=np.float32),
    )

    labels = np.concatenate(
        [
            np.zeros(29, dtype=np.int32),
            np.full(31, -1, dtype=np.int32),
        ]
    )
    monkeypatch.setattr(owner_module, "HDBSCAN", stub_hdbscan(labels))

    result = detect_owner_candidate()

    assert result["status"] == "low_quality"
    assert result["recommendation"] == "low_quality"
    assert result["low_quality_reason"] == "too_few_stmts"
    assert get_current()["voiceprint"]["status"] == "low_quality"
    assert not _candidate_path(env.journal).exists()


def test_low_quality_median_duration_too_short(speakers_env, monkeypatch):
    import solstone.apps.speakers.owner as owner_module
    from solstone.apps.speakers.owner import detect_owner_candidate

    def stub_hdbscan(labels: np.ndarray):
        class StubHDBSCAN:
            def __init__(self, **kwargs):
                self.labels_ = np.asarray(labels, dtype=np.int32)

            def fit(self, embeddings: np.ndarray):
                assert embeddings.shape[0] == len(self.labels_)
                return self

        return StubHDBSCAN

    env = speakers_env()
    rng = np.random.default_rng(1)
    embeddings = _owner_embeddings(60, rng)
    _write_segment(
        env.journal,
        "20240101",
        "mic",
        "090000_300",
        "audio",
        embeddings,
        durations_s=np.full(60, 1.0, dtype=np.float32),
    )

    monkeypatch.setattr(
        owner_module, "HDBSCAN", stub_hdbscan(np.zeros(60, dtype=np.int32))
    )

    result = detect_owner_candidate()

    assert result["status"] == "low_quality"
    assert result["recommendation"] == "low_quality"
    assert result["low_quality_reason"] == "median_duration_too_short"
    assert result["observed_value"] < 1.5
    assert get_current()["voiceprint"]["status"] == "low_quality"
    assert not _candidate_path(env.journal).exists()


def test_low_quality_cluster_too_diffuse(speakers_env, monkeypatch):
    import solstone.apps.speakers.owner as owner_module
    from solstone.apps.speakers.owner import detect_owner_candidate

    def stub_hdbscan(labels: np.ndarray):
        class StubHDBSCAN:
            def __init__(self, **kwargs):
                self.labels_ = np.asarray(labels, dtype=np.int32)

            def fit(self, embeddings: np.ndarray):
                assert embeddings.shape[0] == len(self.labels_)
                return self

        return StubHDBSCAN

    env = speakers_env()
    rng = np.random.default_rng(0)
    template = np.ones((60, 256), dtype=np.float32)
    embeddings = template + rng.normal(scale=2.0, size=(60, 256)).astype(np.float32)
    _write_segment(
        env.journal,
        "20240101",
        "mic",
        "090000_300",
        "audio",
        embeddings,
        durations_s=np.full(60, 1.6, dtype=np.float32),
    )

    monkeypatch.setattr(
        owner_module, "HDBSCAN", stub_hdbscan(np.zeros(60, dtype=np.int32))
    )

    result = detect_owner_candidate()

    assert result["status"] == "low_quality"
    assert result["recommendation"] == "low_quality"
    assert result["low_quality_reason"] == "cluster_too_diffuse"
    assert result["observed_value"] < 0.30
    assert get_current()["voiceprint"]["status"] == "low_quality"
    assert not _candidate_path(env.journal).exists()


def test_detect_owner_candidate_excludes_chaotic_segments(speakers_env, monkeypatch):
    import solstone.apps.speakers.owner as owner_module
    from solstone.apps.speakers.owner import detect_owner_candidate

    class StubHDBSCAN:
        def __init__(self, **kwargs):
            self.labels_ = np.zeros(60, dtype=np.int32)

        def fit(self, embeddings: np.ndarray):
            assert embeddings.shape[0] == 60
            return self

    env = speakers_env()
    rng = np.random.default_rng(2)
    clean_dir = _write_segment(
        env.journal,
        "20240101",
        "mic",
        "090000_300",
        "audio",
        _owner_embeddings(60, rng),
        durations_s=np.full(60, 2.0, dtype=np.float32),
    )
    _rewrite_segment_header(
        clean_dir,
        "audio",
        overlap_fraction=0.05,
        overlap_detector=OVERLAP_DETECTOR_ID,
    )

    chaotic_dir = _write_segment(
        env.journal,
        "20240102",
        "mic",
        "090000_300",
        "audio",
        _owner_embeddings(60, rng),
        durations_s=np.full(60, 2.0, dtype=np.float32),
    )
    _rewrite_segment_header(
        chaotic_dir,
        "audio",
        overlap_fraction=0.20,
        overlap_detector=OVERLAP_DETECTOR_ID,
    )

    monkeypatch.setattr(owner_module, "HDBSCAN", StubHDBSCAN)

    result = detect_owner_candidate()

    assert result["status"] == "candidate"
    assert result["cluster_size"] == 60


def test_bootstrap_owner_from_manual_tags_confirms(speakers_env):
    from solstone.apps.speakers.encoder_config import OWNER_THRESHOLD
    from solstone.apps.speakers.owner import bootstrap_owner_from_manual_tags

    env = speakers_env()
    principal_dir = env.create_entity("Self Person", is_principal=True)
    principal_id = "self_person"
    rng = np.random.default_rng(4)
    base = np.zeros((10, 256), dtype=np.float32)
    base[:, 0] = 1.0
    durations = np.full(10, 2.4, dtype=np.float32)
    for idx in range(3):
        embeddings = base + rng.normal(scale=0.01, size=(10, 256)).astype(np.float32)
        _save_manual_owner_tags(
            env,
            principal_id,
            "20240101",
            f"{9 + idx:02d}0000_300",
            embeddings,
            durations_s=durations,
        )

    result = bootstrap_owner_from_manual_tags()

    owner_path = principal_dir / "owner_centroid.npz"
    assert result["status"] == "confirmed"
    assert result["principal_id"] == principal_id
    assert result["cluster_size"] == 30
    assert owner_path.exists()
    with np.load(owner_path, allow_pickle=False) as data:
        assert set(data.files) == {
            "centroid",
            "cluster_size",
            "threshold",
            "last_refreshed_at",
        }
        centroid = data["centroid"]
        cluster_size = int(np.asarray(data["cluster_size"]).item())
        threshold = float(np.asarray(data["threshold"]).item())
        last_refreshed_at = str(np.asarray(data["last_refreshed_at"]).item())
    assert cluster_size == 30
    assert np.isclose(np.linalg.norm(centroid), 1.0)
    assert np.isclose(threshold, OWNER_THRESHOLD)
    assert last_refreshed_at.endswith("Z")
    assert get_current()["voiceprint"]["status"] == "confirmed"


def test_bootstrap_owner_from_manual_tags_too_few_stmts(speakers_env):
    from solstone.apps.speakers.owner import (
        LOW_QUALITY_REASON_TOO_FEW_STMTS,
        bootstrap_owner_from_manual_tags,
    )

    env = speakers_env()
    env.create_entity("Self Person", is_principal=True)
    embeddings = np.zeros((10, 256), dtype=np.float32)
    embeddings[:, 0] = 1.0
    _save_manual_owner_tags(
        env,
        "self_person",
        "20240101",
        "090000_300",
        embeddings,
        durations_s=np.full(10, 2.0, dtype=np.float32),
    )

    result = bootstrap_owner_from_manual_tags()

    assert result["status"] == "low_quality"
    assert result["source"] == "manual_tags"
    assert result["low_quality_reason"] == LOW_QUALITY_REASON_TOO_FEW_STMTS
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


def test_manual_tag_overlap_guard_excludes_rows(speakers_env):
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

    assert count_manual_tag_embeddings("self_person") == 5
    result = bootstrap_owner_from_manual_tags()
    assert result["low_quality_reason"] == LOW_QUALITY_REASON_TOO_FEW_STMTS


def test_owner_centroid_schema_parity_between_confirm_and_manual_build(speakers_env):
    from solstone.apps.speakers.encoder_config import OWNER_THRESHOLD
    from solstone.apps.speakers.owner import (
        bootstrap_owner_from_manual_tags,
        clear_owner_provisional_cache,
        confirm_owner_candidate,
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

    confirm_owner_candidate()
    owner_path = principal_dir / "owner_centroid.npz"
    with np.load(owner_path, allow_pickle=False) as data:
        confirmed_keys = set(data.files)

    owner_path.unlink()
    clear_owner_provisional_cache("self_person")
    update_state("voiceprint", {"status": "none"})

    base = np.zeros((10, 256), dtype=np.float32)
    base[:, 0] = 1.0
    for idx in range(3):
        _save_manual_owner_tags(
            env,
            "self_person",
            "20240101",
            f"{9 + idx:02d}0000_300",
            base,
            durations_s=np.full(10, 2.0, dtype=np.float32),
        )

    bootstrap_owner_from_manual_tags()
    with np.load(owner_path, allow_pickle=False) as data:
        manual_keys = set(data.files)

    assert (
        confirmed_keys
        == manual_keys
        == {
            "centroid",
            "cluster_size",
            "threshold",
            "last_refreshed_at",
        }
    )


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

    assert response.status_code == 200
    assert response.get_json() == {
        "status": "none",
        "manual_tags_count": 0,
        "segments_available": 0,
        "segments_with_embeddings": 0,
        "embeddings_available": 0,
        "streams_represented": 0,
        "can_build_from_tags": False,
    }


def test_api_owner_status_needs_detection(speakers_env):
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


def test_api_owner_status_manual_tags_count(speakers_env):
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
    from solstone.apps.speakers.routes import speakers_bp

    speakers_env()
    update_state(
        "voiceprint",
        {
            "status": "low_quality",
            "low_quality_reason": "too_few_stmts",
            "observed_value": 5,
            "threshold_value": 30,
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
        "source": "hdbscan",
        "low_quality_reason": "too_few_stmts",
        "observed_value": 5,
        "threshold_value": 30,
        "manual_tags_count": 0,
        "segments_available": 0,
        "segments_with_embeddings": 0,
        "embeddings_available": 0,
        "streams_represented": 0,
        "can_build_from_tags": False,
    }


def test_api_owner_status_no_cluster(speakers_env):
    from solstone.apps.speakers.routes import speakers_bp

    speakers_env()
    update_state("voiceprint", {"status": "no_cluster"})
    app = Flask(__name__)
    app.register_blueprint(speakers_bp)

    with app.test_client() as client:
        response = client.get("/app/speakers/api/owner/status")

    assert response.status_code == 200
    assert response.get_json()["status"] == "no_cluster"


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
        "centroid_metadata": {
            "cluster_size": 0,
            "streams": [],
            "last_refreshed_at": "",
            "intra_cosine_p25": None,
        },
    }


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
    for idx in range(55):
        hour = 9 + (idx // 12)
        minute = (idx % 12) * 5
        stream = "mic" if idx % 2 == 0 else "sys"
        _write_segment(
            env.journal,
            "20240101",
            stream,
            f"{hour:02d}{minute:02d}00_300",
            "audio",
            _owner_embeddings(2, rng),
        )
    for idx in range(50):
        hour = 9 + (idx // 12)
        minute = (idx % 12) * 5
        stream = "other" if idx % 2 == 0 else "other_sys"
        _write_segment(
            env.journal,
            "20240102",
            stream,
            f"{hour:02d}{minute:02d}00_300",
            "audio",
            _other_cluster_embeddings(2),
        )

    app = Flask(__name__)
    app.register_blueprint(speakers_bp)

    with app.test_client() as client:
        response = client.post("/app/speakers/api/owner/detect")

    data = response.get_json()
    assert response.status_code == 200
    assert data["status"] == "candidate"
    assert data["cluster_size"] >= 50
    assert "streams_represented" in data
    assert "recommendation" in data


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
