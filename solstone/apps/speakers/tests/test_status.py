# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for speaker subsystem status."""

from __future__ import annotations

import json

import numpy as np


def _save_principal_manual_tags(env, principal_id: str, count: int) -> None:
    from solstone.apps.speakers.routes import _save_voiceprint

    embeddings = np.zeros((count, 256), dtype=np.float32)
    embeddings[:, 0] = 1.0
    env.create_segment("20240101", "090000_300", ["audio"], embeddings=embeddings)
    env.create_speaker_labels(
        "20240101",
        "090000_300",
        [
            {
                "sentence_id": idx,
                "speaker": principal_id,
                "confidence": "high",
                "method": "user_assigned",
            }
            for idx in range(1, count + 1)
        ],
    )
    for idx, embedding in enumerate(embeddings, start=1):
        _save_voiceprint(
            principal_id,
            embedding,
            "20240101",
            "090000_300",
            "audio",
            idx,
            stream="test",
        )


def test_status_all_sections(speakers_env):
    from solstone.apps.speakers.status import get_speakers_status

    speakers_env()
    result = get_speakers_status()
    assert "embeddings" in result
    assert "owner" in result
    assert "speakers" in result
    assert "clusters" in result
    assert "imports" in result
    assert "attribution" in result


def test_status_single_section(speakers_env):
    from solstone.apps.speakers.status import get_speakers_status

    speakers_env()
    result = get_speakers_status(section="owner")
    assert "status" in result
    assert "centroid_saved" in result


def test_status_owner_includes_bootstrap_diagnostics(speakers_env):
    from solstone.apps.speakers.status import get_speakers_status

    env = speakers_env()
    env.create_entity("Self Person", is_principal=True)
    _save_principal_manual_tags(env, "self_person", 7)

    result = get_speakers_status(section="owner")

    assert result["status"] == "none"
    assert result["manual_tags_count"] == 7
    assert result["segments_available"] == 1
    assert result["embeddings_available"] == 7
    assert result["streams_represented"] == 1
    assert result["can_build_from_tags"] is False


def test_owner_section_confirmed_includes_centroid_metadata_locked_shape(speakers_env):
    from solstone.apps.speakers.encoder_config import OWNER_THRESHOLD
    from solstone.apps.speakers.status import get_speakers_status

    env = speakers_env()
    principal_dir = env.create_entity("Self Person", is_principal=True)
    embeddings = np.zeros((2, 256), dtype=np.float32)
    embeddings[:, 0] = 1.0
    metadata = [
        json.dumps(
            {
                "day": "20240101",
                "segment_key": "090000_300",
                "source": "audio",
                "stream": "test",
                "sentence_id": idx,
                "last_seen_ts": 1704103200000 + idx,
            }
        )
        for idx in range(1, 3)
    ]
    np.savez_compressed(
        principal_dir / "voiceprints.npz",
        embeddings=embeddings,
        metadata=np.asarray(metadata, dtype=str),
    )
    np.savez_compressed(
        principal_dir / "owner_centroid.npz",
        centroid=embeddings[0],
        cluster_size=np.array(2, dtype=np.int32),
        threshold=np.array(OWNER_THRESHOLD, dtype=np.float32),
        last_refreshed_at=np.array("2026-03-15T12:00:00Z"),
    )
    from solstone.think.awareness import update_state

    update_state("voiceprint", {"status": "confirmed"})

    result = get_speakers_status(section="owner")

    assert set(result["centroid_metadata"]) == {
        "cluster_size",
        "streams",
        "last_refreshed_at",
        "intra_cosine_p25",
    }
    assert result["centroid_metadata"]["cluster_size"] == 2
    assert result["centroid_metadata"]["streams"] == ["test"]
    assert result["centroid_metadata"]["last_refreshed_at"] == "2026-03-15T12:00:00Z"
    assert result["centroid_metadata"]["intra_cosine_p25"] == 1.0


def test_speakers_section_includes_last_seen_ts_and_intra_cosine_p25_per_entity(
    speakers_env,
):
    from solstone.apps.speakers.status import get_speakers_status

    env = speakers_env()
    entity_dir = env.create_entity("Alice Test")
    embeddings = np.zeros((2, 256), dtype=np.float32)
    embeddings[:, 0] = 1.0
    metadata = [
        json.dumps(
            {
                "day": "20240101",
                "segment_key": "090000_300",
                "source": "audio",
                "stream": "mic",
                "sentence_id": 1,
                "last_seen_ts": 10,
            }
        ),
        json.dumps(
            {
                "day": "20240102",
                "segment_key": "100000_300",
                "source": "audio",
                "stream": "sys",
                "sentence_id": 2,
                "last_seen_ts": 20,
            }
        ),
    ]
    np.savez_compressed(
        entity_dir / "voiceprints.npz",
        embeddings=embeddings,
        metadata=np.asarray(metadata, dtype=str),
    )

    result = get_speakers_status(section="speakers")

    assert result == [
        {
            "entity_id": "alice_test",
            "name": "Alice Test",
            "embedding_count": 2,
            "segment_count": 2,
            "streams": ["mic", "sys"],
            "last_seen_ts": 20,
            "intra_cosine_p25": 1.0,
        }
    ]


def test_status_unknown_section(speakers_env):
    from solstone.apps.speakers.status import get_speakers_status

    speakers_env()
    result = get_speakers_status(section="nonexistent")
    assert "error" in result


def test_status_embeddings_with_data(speakers_env):
    from solstone.apps.speakers.status import get_speakers_status

    env = speakers_env()
    env.create_segment("20240101", "090000_300", ["mic_audio"])
    env.create_segment("20240101", "091000_300", ["sys_audio"])
    env.create_segment("20240102", "090000_300", ["audio"])

    result = get_speakers_status(section="embeddings")
    assert result["segments"] == 3
    assert result["days"] == 2
    assert result["date_range"] == ["20240101", "20240102"]


def test_status_attribution_with_labels(speakers_env):
    from solstone.apps.speakers.status import get_speakers_status

    env = speakers_env()
    env.create_speaker_labels(
        "20240101",
        "090000_300",
        [
            {
                "sentence_id": 1,
                "speaker": "alice",
                "confidence": "high",
                "method": "voiceprint",
            },
            {
                "sentence_id": 2,
                "speaker": None,
                "confidence": "low",
                "method": "unmatched",
            },
        ],
    )

    result = get_speakers_status(section="attribution")
    assert result["files"] == 1
    assert result["labels"] == 2
    assert result["by_confidence"]["high"] == 1
    assert result["by_confidence"]["low"] == 1
    assert result["by_method"]["voiceprint"] == 1
    assert result["by_method"]["unmatched"] == 1
