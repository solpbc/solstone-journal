# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for unknown speaker discovery."""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np
from typer.testing import CliRunner

from solstone.apps.speakers.call import app as speakers_app
from solstone.apps.speakers.discovery import (
    _discovery_cache_path,
    _discovery_resolved_path,
    discover_unknown_speakers,
    identify_cluster,
)
from solstone.apps.speakers.owner import OWNER_THRESHOLD

_runner = CliRunner()


def _make_speaker_embeddings(
    base_vector: list[float],
    count: int,
    noise_scale: float = 0.0,
) -> np.ndarray:
    """Create a cluster of similar embeddings around a base direction."""
    base = np.array(base_vector + [0.0] * (256 - len(base_vector)), dtype=np.float32)
    base = base / np.linalg.norm(base)
    rng = np.random.default_rng(42)
    noise = rng.normal(0, noise_scale, (count, 256)).astype(np.float32)
    embeddings = base + noise
    norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
    return embeddings / norms


def _setup_owner_centroid(
    journal: Path,
    vector: list[float],
    entity_id: str = "owner_test",
) -> np.ndarray:
    """Create owner entity with centroid for testing."""
    base = np.array(vector + [0.0] * (256 - len(vector)), dtype=np.float32)
    centroid = base / np.linalg.norm(base)
    entity_dir = journal / "entities" / entity_id
    entity_dir.mkdir(parents=True, exist_ok=True)
    (entity_dir / "entity.json").write_text(
        json.dumps(
            {
                "id": entity_id,
                "name": "Owner Test",
                "type": "Person",
                "is_principal": True,
            }
        ),
        encoding="utf-8",
    )
    np.savez_compressed(
        entity_dir / "owner_centroid.npz",
        centroid=centroid,
        cluster_size=np.array(100, dtype=np.int32),
        threshold=np.array(OWNER_THRESHOLD, dtype=np.float32),
        last_refreshed_at=np.array("2026-01-01T00:00:00Z"),
    )
    return centroid


def _create_cluster_segments(env, embeddings: np.ndarray) -> list[tuple[str, str, int]]:
    """Create four segments with one qualifying cluster and one filtered cluster."""
    segments = [
        ("20240101", "090000_300"),
        ("20240102", "090000_300"),
        ("20240103", "090000_300"),
        ("20240104", "090000_300"),
    ]
    alt_embeddings = _make_speaker_embeddings([0.0, 0.0, 1.0], embeddings.shape[0])
    results = []
    for idx, (day, segment_key) in enumerate(segments):
        segment_embeddings = embeddings
        if idx < 2:
            segment_embeddings = np.vstack([embeddings, alt_embeddings])
        env.create_segment(day, segment_key, ["audio"], embeddings=segment_embeddings)
        results.append((day, segment_key, segment_embeddings.shape[0]))
    return results


def _all_sentence_labels(entity_id: str, count: int) -> list[dict]:
    """Build fully attributed labels for a segment."""
    return [
        {
            "sentence_id": idx,
            "speaker": entity_id,
            "confidence": "high",
            "method": "user_identified",
        }
        for idx in range(1, count + 1)
    ]


def _load_voiceprint_count(journal: Path, entity_id: str) -> int:
    """Return number of saved voiceprints for an entity."""
    path = journal / "entities" / entity_id / "voiceprints.npz"
    if not path.exists():
        return 0
    data = np.load(path, allow_pickle=False)
    return int(len(data["embeddings"]))


def _load_corrections_count(journal: Path, day: str, segment_key: str) -> int:
    """Return number of correction entries for a segment."""
    path = journal / day / "test" / segment_key / "talents" / "speaker_corrections.json"
    if not path.exists():
        return 0
    return len(json.loads(path.read_text(encoding="utf-8")).get("corrections", []))


def test_discover_no_owner_centroid(speakers_env):
    speakers_env()

    result = discover_unknown_speakers()

    assert result == {"clusters": []}


def test_discover_no_unmatched(speakers_env):
    env = speakers_env()
    _setup_owner_centroid(env.journal, [0.0, 1.0])
    env.create_entity("Alice Test")
    embeddings = _make_speaker_embeddings([1.0, 0.0], 5)
    segments = _create_cluster_segments(env, embeddings)

    for day, segment_key, sentence_count in segments:
        env.create_speaker_labels(
            day,
            segment_key,
            _all_sentence_labels("alice_test", sentence_count),
        )

    result = discover_unknown_speakers()

    assert result == {"clusters": []}


def test_discover_clusters_found(speakers_env):
    env = speakers_env()
    _setup_owner_centroid(env.journal, [0.0, 1.0])
    embeddings = _make_speaker_embeddings([1.0, 0.0], 5)
    _create_cluster_segments(env, embeddings)

    result = discover_unknown_speakers()

    assert len(result["clusters"]) == 1
    cluster = result["clusters"][0]
    assert cluster["size"] == 20
    assert cluster["segment_count"] >= 3
    assert len(cluster["samples"]) == 3


def test_discover_filters_attributed(speakers_env):
    env = speakers_env()
    _setup_owner_centroid(env.journal, [0.0, 1.0])
    env.create_entity("Alice Test")
    embeddings = _make_speaker_embeddings([1.0, 0.0], 5)
    segments = _create_cluster_segments(env, embeddings)

    for day, segment_key, sentence_count in segments[:3]:
        env.create_speaker_labels(
            day,
            segment_key,
            _all_sentence_labels("alice_test", sentence_count),
        )

    result = discover_unknown_speakers()

    assert result == {"clusters": []}


def test_identify_creates_entity(speakers_env):
    env = speakers_env()
    _setup_owner_centroid(env.journal, [0.0, 1.0])
    embeddings = _make_speaker_embeddings([1.0, 0.0], 5)
    segments = _create_cluster_segments(env, embeddings)

    scan_result = discover_unknown_speakers()
    cluster_id = scan_result["clusters"][0]["cluster_id"]

    result = identify_cluster(cluster_id, "Bob Smith")

    entity_dir = env.journal / "entities" / "bob_smith"
    assert result["status"] == "identified"
    assert result["entity_id"] == "bob_smith"
    assert entity_dir.joinpath("entity.json").exists()
    assert entity_dir.joinpath("voiceprints.npz").exists()
    assert result["voiceprints_saved"] == 20
    assert result["segments_updated"] == 4
    assert result["sentences_attributed"] == 20

    for day, segment_key, _sentence_count in segments:
        labels_path = (
            env.journal / day / "test" / segment_key / "talents" / "speaker_labels.json"
        )
        corrections_path = (
            env.journal
            / day
            / "test"
            / segment_key
            / "talents"
            / "speaker_corrections.json"
        )
        labels_data = json.loads(labels_path.read_text(encoding="utf-8"))
        assert all(label["speaker"] == "bob_smith" for label in labels_data["labels"])
        assert corrections_path.exists()


def test_identify_matches_existing(speakers_env):
    env = speakers_env()
    _setup_owner_centroid(env.journal, [0.0, 1.0])
    env.create_entity("Bob Smith")
    embeddings = _make_speaker_embeddings([1.0, 0.0], 5)
    _create_cluster_segments(env, embeddings)

    scan_result = discover_unknown_speakers()
    cluster_id = scan_result["clusters"][0]["cluster_id"]
    result = identify_cluster(cluster_id, "Bob Smith")

    assert result["entity_id"] == "bob_smith"
    assert result["voiceprints_saved"] == 20
    assert (env.journal / "entities" / "bob_smith" / "voiceprints.npz").exists()


def test_identify_idempotent(speakers_env):
    env = speakers_env()
    _setup_owner_centroid(env.journal, [0.0, 1.0])
    embeddings = _make_speaker_embeddings([1.0, 0.0], 5)
    segments = _create_cluster_segments(env, embeddings)

    scan_result = discover_unknown_speakers()
    cluster_id = scan_result["clusters"][0]["cluster_id"]

    first = identify_cluster(cluster_id, "Bob Smith")
    first_voiceprints = _load_voiceprint_count(env.journal, "bob_smith")
    first_corrections = {
        (day, segment_key): _load_corrections_count(env.journal, day, segment_key)
        for day, segment_key, _sentence_count in segments
    }

    second = identify_cluster(cluster_id, "Bob Smith")

    assert first["voiceprints_saved"] == 20
    assert second["voiceprints_saved"] == 0
    assert _discovery_cache_path().exists()
    assert _discovery_resolved_path().exists()
    assert _load_voiceprint_count(env.journal, "bob_smith") == first_voiceprints
    for day, segment_key, _sentence_count in segments:
        assert (
            _load_corrections_count(env.journal, day, segment_key)
            == first_corrections[(day, segment_key)]
        )


def test_identify_contamination_guard(speakers_env):
    env = speakers_env()
    _setup_owner_centroid(env.journal, [0.0, 1.0])
    embeddings = _make_speaker_embeddings([1.0, 0.0], 5)
    _create_cluster_segments(env, embeddings)

    scan_result = discover_unknown_speakers()
    cluster_id = scan_result["clusters"][0]["cluster_id"]
    _setup_owner_centroid(env.journal, [1.0, 0.0])

    result = identify_cluster(cluster_id, "Bob Smith")

    assert result["voiceprints_saved"] == 0
    assert not (env.journal / "entities" / "bob_smith" / "voiceprints.npz").exists()


def test_identify_cli_success(speakers_env):
    """CLI identify outputs JSON to stdout on success."""
    env = speakers_env()
    _setup_owner_centroid(env.journal, [0.0, 1.0])
    embeddings = _make_speaker_embeddings([1.0, 0.0], 5)
    _create_cluster_segments(env, embeddings)

    scan_result = discover_unknown_speakers()
    cluster_id = scan_result["clusters"][0]["cluster_id"]

    result = _runner.invoke(speakers_app, ["identify", str(cluster_id), "Bob Smith"])
    assert result.exit_code == 0
    data = json.loads(result.output)
    assert data["status"] == "identified"
    assert data["entity_id"] == "bob_smith"


def test_identify_cli_error_no_cache(speakers_env):
    """CLI identify outputs error JSON to stderr and exits 1 when no cache."""
    speakers_env()
    result = _runner.invoke(speakers_app, ["identify", "0", "Nobody"])
    assert result.exit_code == 1
