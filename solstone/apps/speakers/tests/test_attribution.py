# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for speaker attribution engine."""

from __future__ import annotations

import json
import math
from pathlib import Path

import numpy as np
import pytest

from solstone.apps.speakers.encoder_config import (
    ACOUSTIC_HIGH,
    ENCODER_ID,
    OVERLAP_DETECTOR_ID,
    OWNER_MARGIN_MIN,
    OWNER_THRESHOLD,
)
from solstone.apps.speakers.tests.conftest import journal_tree_hash

# Test stream name (matches conftest.STREAM)
STREAM = "test"


def _normalized(vector: list[float]) -> np.ndarray:
    emb = np.array(vector + [0.0] * (256 - len(vector)), dtype=np.float32)
    return emb / np.linalg.norm(emb)


def _embedding_with_owner_cos(cosine: float) -> np.ndarray:
    return _normalized([cosine, math.sqrt(1.0 - cosine * cosine)])


def _embedding_with_cosine_to(target: np.ndarray, cosine: float) -> np.ndarray:
    orthogonal = np.zeros_like(target)
    orthogonal[0] = -target[1]
    orthogonal[1] = target[0]
    orthogonal = orthogonal / np.linalg.norm(orthogonal)
    return ((target * cosine) + (orthogonal * math.sqrt(1.0 - cosine * cosine))).astype(
        np.float32
    )


def _embedding_with_orthogonal_entity_cosines(
    first_cosine: float,
    second_cosine: float,
) -> np.ndarray:
    remaining = 1.0 - first_cosine * first_cosine - second_cosine * second_cosine
    assert remaining >= 0.0
    return _normalized([0.0, first_cosine, second_cosine, math.sqrt(remaining)])


def _setup_owner(env, name: str = "Self Person") -> tuple[Path, np.ndarray]:
    """Create a principal entity with confirmed owner centroid."""
    principal_dir = env.create_entity(name, is_principal=True)
    centroid = _normalized([1.0, 0.0])
    np.savez_compressed(
        principal_dir / "owner_centroid.npz",
        centroid=centroid,
        cluster_size=np.array(70, dtype=np.int32),
        threshold=np.array(OWNER_THRESHOLD, dtype=np.float32),
        last_refreshed_at=np.array("2026-03-15T12:00:00Z"),
    )
    return principal_dir, centroid


def _setup_margin_owner(env, name: str = "Self Person") -> tuple[Path, np.ndarray]:
    """Create a principal entity with a margin-bearing owner centroid."""
    principal_dir = env.create_entity(name, is_principal=True)
    centroid = _normalized([1.0, 0.0])
    np.savez_compressed(
        principal_dir / "owner_centroid.npz",
        centroid=centroid,
        cluster_size=np.array(70, dtype=np.int32),
        threshold=np.array(OWNER_THRESHOLD, dtype=np.float32),
        margin=np.array(OWNER_MARGIN_MIN, dtype=np.float32),
        last_refreshed_at=np.array("2026-03-15T12:00:00Z"),
    )
    return principal_dir, centroid


def _write_controlled_segment(
    env,
    day: str,
    segment_key: str,
    embeddings: np.ndarray,
    source: str = "mic_audio",
) -> Path:
    """Write a segment with specific embeddings."""
    return env.create_segment(
        day,
        segment_key,
        [source],
        stream=STREAM,
        embeddings=embeddings,
    )


def _write_labeled_controlled_segment(
    env,
    day: str,
    segment_key: str,
    embeddings: np.ndarray,
    speaker_labels: list[int | None],
    source: str = "mic_audio",
) -> Path:
    """Write a controlled segment with integer JSONL speaker labels."""
    seg_dir = _write_controlled_segment(env, day, segment_key, embeddings, source)
    jsonl_path = seg_dir / f"{source}.jsonl"
    lines = jsonl_path.read_text(encoding="utf-8").splitlines()
    updated = [lines[0]]
    for line, speaker_label in zip(lines[1:], speaker_labels):
        row = json.loads(line)
        if speaker_label is not None:
            row["speaker"] = speaker_label
        updated.append(json.dumps(row))
    jsonl_path.write_text("\n".join(updated) + "\n", encoding="utf-8")
    np.savez_compressed(
        seg_dir / f"{source}.npz",
        embeddings=embeddings.astype(np.float32),
        statement_ids=np.arange(1, len(embeddings) + 1, dtype=np.int32),
        durations_s=np.full(len(embeddings), 3.0, dtype=np.float32),
        encoder=np.array(ENCODER_ID),
    )
    return seg_dir


def _write_entity_voiceprints(
    entity_dir: Path,
    embeddings: list[np.ndarray],
    *,
    stream: str = STREAM,
) -> None:
    np.savez_compressed(
        entity_dir / "voiceprints.npz",
        embeddings=np.vstack(embeddings).astype(np.float32),
        metadata=np.array(
            [
                json.dumps(
                    {
                        "day": "20240101",
                        "segment_key": f"09{i:02d}00_300",
                        "source": "mic_audio",
                        "sentence_id": 1,
                        "stream": stream,
                        "added_at": 1700000000000,
                    }
                )
                for i in range(len(embeddings))
            ],
            dtype=str,
        ),
    )


def _rewrite_segment_header(seg_dir: Path, source: str, **updates: object) -> None:
    jsonl_path = seg_dir / f"{source}.jsonl"
    lines = jsonl_path.read_text(encoding="utf-8").splitlines()
    header = json.loads(lines[0]) if lines else {}
    header.update(updates)
    lines[0] = json.dumps(header)
    jsonl_path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def _setup_margin_trap_entities(
    env,
) -> tuple[np.ndarray, Path, Path]:
    trap = _embedding_with_owner_cos(0.45)
    competitor = _embedding_with_cosine_to(trap, 0.41)
    distractor = _normalized([0.0, 0.0, 1.0])
    # Sort order is load-bearing: first-only margin implementations must see
    # the low-cosine distractor before the real trap-band competitor.
    distractor_dir = env.create_entity("Aaron Reed")
    competitor_dir = env.create_entity("Zara Vale")
    _write_entity_voiceprints(competitor_dir, [competitor] * 5)
    _write_entity_voiceprints(distractor_dir, [distractor] * 5)

    assert float(np.dot(trap, _normalized([1.0, 0.0]))) >= OWNER_THRESHOLD
    assert (
        ACOUSTIC_HIGH
        < float(np.dot(trap, competitor))
        < float(np.dot(trap, _normalized([1.0, 0.0])))
    )
    assert (
        float(np.dot(trap, competitor))
        > float(np.dot(trap, _normalized([1.0, 0.0]))) - OWNER_MARGIN_MIN
    )
    assert np.isclose(float(np.dot(trap, distractor)), 0.0)
    return trap, competitor_dir, distractor_dir


# ---------------------------------------------------------------------------
# Layer 2 evidence readers
# ---------------------------------------------------------------------------


def test_load_segment_speakers_with_gaps_missing_valid_and_blank(tmp_path):
    from solstone.apps.speakers.attribution import _load_segment_speakers_with_gaps

    seg_dir = tmp_path / "segment"

    assert _load_segment_speakers_with_gaps(seg_dir) == ([], [])

    speakers_path = seg_dir / "talents" / "speakers.json"
    speakers_path.parent.mkdir(parents=True)
    speakers_path.write_text(json.dumps(["Ana", "Bo"]), encoding="utf-8")
    assert _load_segment_speakers_with_gaps(seg_dir) == (["Ana", "Bo"], [])

    speakers_path.write_text(json.dumps(["Ok", ""]), encoding="utf-8")
    assert _load_segment_speakers_with_gaps(seg_dir) == (["Ok"], [])


@pytest.mark.parametrize(
    ("raw", "expected_names", "expected_gap"),
    [
        (b"\xff", [], {"source": "speakers", "reason": "malformed_json"}),
        (b"{not json", [], {"source": "speakers", "reason": "malformed_json"}),
        (
            json.dumps({"a": 1}).encode("utf-8"),
            [],
            {"source": "speakers", "reason": "wrong_shape"},
        ),
        (
            json.dumps([1, 2]).encode("utf-8"),
            [],
            {"source": "speakers", "reason": "wrong_shape"},
        ),
        (
            json.dumps(["Ok", 5]).encode("utf-8"),
            ["Ok"],
            {"source": "speakers", "reason": "wrong_shape"},
        ),
    ],
)
def test_load_segment_speakers_with_gaps_malformed_cases(
    tmp_path,
    raw: bytes,
    expected_names: list[str],
    expected_gap: dict[str, str],
):
    from solstone.apps.speakers.attribution import _load_segment_speakers_with_gaps

    speakers_path = tmp_path / "segment" / "talents" / "speakers.json"
    speakers_path.parent.mkdir(parents=True)
    speakers_path.write_bytes(raw)

    names, gaps = _load_segment_speakers_with_gaps(tmp_path / "segment")

    assert names == expected_names
    assert gaps == [expected_gap]
    assert len(gaps) == 1


def test_load_segment_speakers_with_gaps_unreadable(tmp_path, monkeypatch):
    from solstone.apps.speakers.attribution import _load_segment_speakers_with_gaps

    speakers_path = tmp_path / "segment" / "talents" / "speakers.json"
    speakers_path.parent.mkdir(parents=True)
    speakers_path.write_text(json.dumps(["Ana"]), encoding="utf-8")
    original_read_text = Path.read_text

    def fail_target_read(path: Path, *args, **kwargs):
        if path == speakers_path:
            raise OSError("boom")
        return original_read_text(path, *args, **kwargs)

    monkeypatch.setattr(Path, "read_text", fail_target_read)

    assert _load_segment_speakers_with_gaps(tmp_path / "segment") == (
        [],
        [{"source": "speakers", "reason": "unreadable"}],
    )


@pytest.mark.parametrize(
    ("raw", "expected"),
    [
        ("\n", (None, [])),
        (json.dumps({}) + "\n", (None, [])),
        (json.dumps({"setting": None}) + "\n", (None, [])),
        (
            json.dumps({"setting": "coffee with Priya"}) + "\n",
            ("coffee with Priya", []),
        ),
        (
            json.dumps([1, 2, 3]) + "\n",
            (None, [{"source": "setting", "reason": "wrong_shape"}]),
        ),
        (
            json.dumps({"setting": 5}) + "\n",
            (None, [{"source": "setting", "reason": "wrong_shape"}]),
        ),
        ("{not json\n", (None, [{"source": "setting", "reason": "malformed_json"}])),
    ],
)
def test_load_setting_field_with_gaps_branch_cases(
    tmp_path,
    raw: str,
    expected: tuple[str | None, list[dict[str, str]]],
):
    from solstone.apps.speakers.attribution import _load_setting_field_with_gaps

    seg_dir = tmp_path / "segment"
    seg_dir.mkdir()
    (seg_dir / "imported_audio.jsonl").write_text(raw, encoding="utf-8")

    assert _load_setting_field_with_gaps(seg_dir) == expected


def test_load_setting_field_with_gaps_missing_and_invalid_utf8(tmp_path):
    from solstone.apps.speakers.attribution import _load_setting_field_with_gaps

    missing_seg_dir = tmp_path / "missing"
    assert _load_setting_field_with_gaps(missing_seg_dir) == (None, [])

    seg_dir = tmp_path / "segment"
    seg_dir.mkdir()
    (seg_dir / "imported_audio.jsonl").write_bytes(b"\xff")

    assert _load_setting_field_with_gaps(seg_dir) == (
        None,
        [{"source": "setting", "reason": "malformed_json"}],
    )


# ---------------------------------------------------------------------------
# Setting name parser tests
# ---------------------------------------------------------------------------


def test_derive_owner_name_variants():
    from solstone.apps.speakers.attribution import _derive_owner_name_variants

    assert _derive_owner_name_variants(["Rae", "Raelyn Brooks", "Raylyn"]) == {
        "rae",
        "raelyn",
        "raelyn brooks",
        "brooks",
        "raylyn",
    }
    assert _derive_owner_name_variants(["Maya Chen"]) == {
        "maya",
        "maya chen",
        "chen",
    }


def test_parse_setting_names(speakers_env):
    from solstone.apps.speakers.attribution import _parse_setting_names

    env = speakers_env()
    env.set_identity(preferred="Rae", name="Raelyn Brooks", aliases=["Raylyn"])
    assert _parse_setting_names("Rae and Jack at coffee") == ["Jack"]
    assert _parse_setting_names("Raelyn and Jack at coffee") == ["Jack"]
    assert _parse_setting_names("RAELYN BROOKS and Jack at coffee") == ["Jack"]
    assert _parse_setting_names("Raylyn and Jack at coffee") == ["Jack"]
    assert _parse_setting_names("Brooks and Jack at coffee") == ["Jack"]
    assert _parse_setting_names("Meeting with Perry and Thomas") == ["Perry", "Thomas"]
    assert _parse_setting_names("Lunch with Jordan Patel") == ["Jordan Patel"]
    assert _parse_setting_names("") == []
    assert _parse_setting_names("Call with Ryan") == ["Ryan"]


def test_parse_setting_names_generalizes_identity_tokens(speakers_env):
    from solstone.apps.speakers.attribution import _parse_setting_names

    env = speakers_env()
    env.set_identity(name="Maya Chen", aliases=[])

    assert _parse_setting_names("Maya and Riley at coffee") == ["Riley"]
    assert _parse_setting_names("Chen and Riley at coffee") == ["Riley"]
    assert _parse_setting_names("Maya Chen and Riley at coffee") == ["Riley"]


def test_parse_setting_names_without_identity_keeps_names(speakers_env):
    from solstone.apps.speakers.attribution import _parse_setting_names

    speakers_env()

    assert _parse_setting_names("Alex and Blair at coffee") == ["Alex", "Blair"]


def test_extract_screen_participants_returns_empty_when_screen_json_absent(tmp_path):
    from solstone.apps.speakers.attribution import _extract_screen_participants

    seg_dir = tmp_path / "20240101" / STREAM / "090000_300"
    seg_dir.mkdir(parents=True)

    assert _extract_screen_participants(seg_dir) == []


def test_extract_screen_participants_filters_structured_screen_entities(tmp_path):
    from solstone.apps.speakers.attribution import _extract_screen_participants

    seg_dir = tmp_path / "20240101" / STREAM / "090000_300"
    talents_dir = seg_dir / "talents"
    talents_dir.mkdir(parents=True)
    (talents_dir / "screen.json").write_text(
        json.dumps(
            {
                "narrative": "Alice Smith and Bob Chen appeared in Zoom.",
                "entities": [
                    {
                        "type": "Person",
                        "name": " Alice Smith ",
                        "role": "attendee",
                        "context": "Visible in participant tile.",
                    },
                    {
                        "type": "Person",
                        "name": "Carol Jones",
                        "role": "mentioned",
                        "context": "Named in chat.",
                    },
                    {
                        "type": "Tool",
                        "name": "Zoom",
                        "role": "mentioned",
                        "context": "Meeting tool.",
                    },
                    {
                        "type": "Person",
                        "name": "Bob Chen",
                        "role": "attendee",
                        "context": "Visible in participant tile.",
                    },
                ],
            }
        ),
        encoding="utf-8",
    )

    assert _extract_screen_participants(seg_dir) == ["Alice Smith", "Bob Chen"]


def test_extract_screen_participants_logs_and_skips_malformed_json(tmp_path, caplog):
    from solstone.apps.speakers.attribution import _extract_screen_participants

    seg_dir = tmp_path / "20240101" / STREAM / "090000_300"
    talents_dir = seg_dir / "talents"
    talents_dir.mkdir(parents=True)
    (talents_dir / "screen.json").write_text("{ invalid json", encoding="utf-8")

    with caplog.at_level("WARNING", logger="solstone.apps.speakers.attribution"):
        assert _extract_screen_participants(seg_dir) == []

    assert "failed to read screen participants" in caplog.text


# ---------------------------------------------------------------------------
# Layer 1: Owner separation
# ---------------------------------------------------------------------------


def test_attribute_no_owner_centroid(speakers_env):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    env.create_segment("20240101", "090000_300", ["mic_audio"])

    result = attribute_segment("20240101", STREAM, "090000_300")

    assert result.get("error") == "no_owner_centroid"


def test_attribute_segment_with_no_owner_centroid_flow_unchanged(speakers_env):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    env.create_segment("20240101", "090000_300", ["mic_audio"])

    result = attribute_segment("20240101", STREAM, "090000_300")

    assert result == {"error": "no_owner_centroid", "labels": [], "unmatched": []}


def test_attribute_no_embeddings(speakers_env):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    _setup_owner(env)
    # Create empty segment directory (no npz files)
    seg_dir = env.journal / "20240101" / STREAM / "090000_300"
    seg_dir.mkdir(parents=True, exist_ok=True)

    result = attribute_segment("20240101", STREAM, "090000_300")

    assert result["labels"] == []
    assert result["unmatched"] == []


def test_layer1_owner_classification(speakers_env):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    _setup_owner(env)

    # Sentence 1: close to owner centroid [1,0,...], sentence 2: far from it
    owner_emb = _normalized([0.6, 0.8])
    other_emb = _normalized([0.1, 0.99])
    embeddings = np.vstack([owner_emb, other_emb])

    _write_controlled_segment(env, "20240101", "090000_300", embeddings)

    result = attribute_segment("20240101", STREAM, "090000_300")
    labels = result["labels"]

    assert len(labels) == 2
    # First sentence should be owner
    assert labels[0]["speaker"] == "self_person"
    assert labels[0]["method"] == "owner_centroid"
    assert labels[0]["confidence"] == "high"
    # Second sentence: unmatched (no speakers.json, no voiceprints)
    assert labels[1]["speaker"] is None


def test_attribute_segment_metadata_uses_same_loaded_centroid_timestamp(
    speakers_env, monkeypatch
):
    from solstone.apps.speakers import attribution
    from solstone.apps.speakers.attribution import attribute_segment
    from solstone.think.entities.voiceprints import OwnerCentroid

    env = speakers_env()
    env.create_entity("Self Person", is_principal=True)
    owner_emb = _normalized([0.95, 0.05])
    _write_controlled_segment(
        env,
        "20240101",
        "090000_300",
        owner_emb.reshape(1, -1),
    )
    loaded: list[str] = []

    def fake_load_owner_centroid() -> OwnerCentroid:
        timestamp = "loaded-once" if not loaded else "torn-second-load"
        loaded.append(timestamp)
        return OwnerCentroid(
            centroid=_normalized([1.0, 0.0]).astype(np.float32),
            threshold=0.43,
            cluster_size=30,
            last_refreshed_at=timestamp,
            intra_cosine_p25=None,
            streams=[],
        )

    monkeypatch.setattr(attribution, "load_owner_centroid", fake_load_owner_centroid)

    result = attribute_segment("20240101", STREAM, "090000_300")

    assert loaded == ["loaded-once"]
    assert result["labels"][0]["method"] == "owner_centroid"
    assert result["metadata"]["owner_centroid_last_refreshed_at"] == "loaded-once"


def test_l2_single_speaker_downgrades_margin_declined_statement(
    speakers_env,
):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    _setup_margin_owner(env)
    trap, _competitor_dir, _distractor_dir = _setup_margin_trap_entities(env)
    control = _normalized([0.1, 0.99])
    _write_controlled_segment(
        env,
        "20240101",
        "091500_300",
        np.vstack([trap, control]),
    )
    env.create_speakers_json("20240101", "091500_300", ["Aaron Reed"])

    result = attribute_segment("20240101", STREAM, "091500_300")
    margin_label = result["labels"][0]
    control_label = result["labels"][1]

    assert margin_label["speaker"] == "aaron_reed"
    assert margin_label["confidence"] == "medium"
    assert margin_label["confidence"] != "high"
    assert margin_label["method"] == "structural_single_speaker"
    assert margin_label["owner_margin_declined"] is True
    assert control_label["speaker"] == "aaron_reed"
    assert control_label["confidence"] == "high"
    assert control_label["method"] == "structural_single_speaker"
    assert "owner_margin_declined" not in control_label
    assert result["unmatched"] == []
    assert result["metadata"]["candidate_evidence"] == [
        {"entity_id": "aaron_reed", "sources": ["speakers"]}
    ]
    assert result["metadata"]["voiceprint_versions"] == {}


def test_l1_owner_margin_keeps_clean_owner_claim_when_best_non_owner_is_low(
    speakers_env,
):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    _setup_margin_owner(env)
    owner_like = _embedding_with_owner_cos(0.50)
    competitor = _embedding_with_cosine_to(owner_like, 0.20)
    competitor_dir = env.create_entity("Casey Rival")
    _write_entity_voiceprints(competitor_dir, [competitor] * 5)
    _write_controlled_segment(
        env,
        "20240101",
        "092500_300",
        owner_like.reshape(1, -1),
    )

    result = attribute_segment("20240101", STREAM, "092500_300")
    label = result["labels"][0]

    assert label["speaker"] == "self_person"
    assert label["confidence"] == "high"
    assert label["method"] == "owner_centroid"
    assert "owner_margin_declined" not in label


def test_l1_owner_margin_is_vacuous_without_usable_non_owner_voiceprints(
    speakers_env,
):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    _setup_margin_owner(env)
    env.create_entity("Silent Person")
    owner_like = _embedding_with_owner_cos(0.45)
    _write_controlled_segment(
        env,
        "20240101",
        "093500_300",
        owner_like.reshape(1, -1),
    )

    result = attribute_segment("20240101", STREAM, "093500_300")
    label = result["labels"][0]

    assert label["speaker"] == "self_person"
    assert label["method"] == "owner_centroid"
    assert "owner_margin_declined" not in label
    assert result["metadata"]["voiceprint_versions"] == {}


def test_l1_owner_margin_excludes_principal_voiceprints_from_competition(speakers_env):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    principal_dir, _centroid = _setup_margin_owner(env)
    owner_like = _embedding_with_owner_cos(0.45)
    _write_entity_voiceprints(principal_dir, [owner_like] * 5)
    _write_controlled_segment(
        env,
        "20240101",
        "094500_300",
        owner_like.reshape(1, -1),
    )

    result = attribute_segment("20240101", STREAM, "094500_300")
    label = result["labels"][0]

    assert label["speaker"] == "self_person"
    assert label["method"] == "owner_centroid"
    assert "owner_margin_declined" not in label


def test_legacy_owner_snapshot_does_not_emit_margin_flag_or_change_l2_flow(
    speakers_env,
):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    _setup_owner(env)
    env.create_entity("Ryan Bennett")
    owner_emb = _normalized([0.95, 0.05])
    other_emb = _normalized([0.1, 0.99])
    _write_controlled_segment(
        env,
        "20240101",
        "095500_300",
        np.vstack([owner_emb, other_emb]),
    )
    env.create_speakers_json("20240101", "095500_300", ["Ryan Bennett"])

    result = attribute_segment("20240101", STREAM, "095500_300")

    assert result["labels"][0]["method"] == "owner_centroid"
    assert result["labels"][1]["speaker"] == "ryan_bennett"
    assert result["labels"][1]["confidence"] == "high"
    assert result["labels"][1]["method"] == "structural_single_speaker"
    assert all("owner_margin_declined" not in label for label in result["labels"])


# ---------------------------------------------------------------------------
# Layer 2: Structural heuristics — single speaker
# ---------------------------------------------------------------------------


def test_layer2_single_speaker(speakers_env):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    _setup_owner(env)
    env.create_entity("Ryan Bennett")

    owner_emb = _normalized([0.95, 0.05])
    other_emb = _normalized([0.1, 0.99])
    embeddings = np.vstack([owner_emb, other_emb, other_emb])

    seg_dir = _write_controlled_segment(env, "20240101", "090000_300", embeddings)

    # speakers.json with exactly 1 speaker
    agents_dir = seg_dir / "talents"
    agents_dir.mkdir(parents=True, exist_ok=True)
    (agents_dir / "speakers.json").write_text(json.dumps(["Ryan Bennett"]))

    result = attribute_segment("20240101", STREAM, "090000_300")
    labels = result["labels"]

    assert labels[0]["method"] == "owner_centroid"  # sentence 1: owner
    assert labels[1]["speaker"] == "ryan_bennett"  # sentence 2: Ryan
    assert labels[1]["method"] == "structural_single_speaker"
    assert labels[1]["confidence"] == "high"
    assert labels[2]["speaker"] == "ryan_bennett"  # sentence 3: Ryan
    assert result["unmatched"] == []


def test_layer2_single_speaker_ambiguous_name_stays_unmatched(speakers_env):
    from solstone.apps.speakers.attribution import attribute_segment
    from solstone.think.entities import load_ambiguities

    env = speakers_env()
    _setup_owner(env)
    env.create_entity("Sarah Connor")
    env.create_entity("Sarah Lee")

    owner_emb = _normalized([0.95, 0.05])
    other_emb = _normalized([0.1, 0.99])
    embeddings = np.vstack([owner_emb, other_emb])

    seg_dir = _write_controlled_segment(env, "20240101", "090000_300", embeddings)
    agents_dir = seg_dir / "talents"
    agents_dir.mkdir(parents=True, exist_ok=True)
    (agents_dir / "speakers.json").write_text(json.dumps(["Sarah"]))

    result = attribute_segment("20240101", STREAM, "090000_300")

    assert result["labels"][1]["speaker"] is None
    assert result["labels"][1]["confidence"] is None
    assert result["labels"][1]["method"] is None
    assert result["unmatched"] == [2]
    rows = load_ambiguities()
    assert len(rows) == 1
    assert rows[0]["normalized_query"] == "sarah"


# ---------------------------------------------------------------------------
# Layer 2: Setting field
# ---------------------------------------------------------------------------


def test_layer2_setting_field(speakers_env):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    env.set_identity(preferred="Rae", name="Raelyn Brooks", aliases=["Raylyn"])
    _setup_owner(env)
    env.create_entity("Jack Andersohn")

    owner_emb = _normalized([0.95, 0.05])
    other_emb = _normalized([0.1, 0.99])
    embeddings = np.vstack([owner_emb, other_emb])

    seg_dir = _write_controlled_segment(
        env, "20240101", "090000_300", embeddings, source="imported_audio"
    )

    # Write imported_audio.jsonl with setting field
    jsonl_path = seg_dir / "imported_audio.jsonl"
    header = {
        "raw": "imported_audio.flac",
        "model": "medium.en",
        "setting": "Rae and Jack Andersohn at coffee",
    }
    lines = [json.dumps(header)]
    lines.append(json.dumps({"start": "09:00:00", "text": "Owner talking"}))
    lines.append(json.dumps({"start": "09:00:05", "text": "Jack talking"}))
    jsonl_path.write_text("\n".join(lines) + "\n")

    result = attribute_segment("20240101", STREAM, "090000_300")
    labels = result["labels"]

    assert labels[0]["method"] == "owner_centroid"
    assert labels[1]["speaker"] == "jack_andersohn"
    assert labels[1]["method"] == "structural_setting"


def test_l2_setting_field_downgrades_margin_declined_statement(speakers_env):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    env.set_identity(preferred="Self", name="Self Person", aliases=[])
    _setup_margin_owner(env)
    trap, _competitor_dir, _distractor_dir = _setup_margin_trap_entities(env)
    control = _normalized([0.1, 0.99])
    seg_dir = _write_controlled_segment(
        env,
        "20240101",
        "101500_300",
        np.vstack([trap, control]),
        source="imported_audio",
    )
    _rewrite_segment_header(
        seg_dir,
        "imported_audio",
        setting="Self and Aaron Reed at coffee",
    )

    result = attribute_segment("20240101", STREAM, "101500_300")
    margin_label = result["labels"][0]
    control_label = result["labels"][1]

    assert margin_label["speaker"] == "aaron_reed"
    assert margin_label["confidence"] == "medium"
    assert margin_label["confidence"] != "high"
    assert margin_label["method"] == "structural_setting"
    assert margin_label["owner_margin_declined"] is True
    assert control_label["speaker"] == "aaron_reed"
    assert control_label["confidence"] == "high"
    assert control_label["method"] == "structural_setting"
    assert "owner_margin_declined" not in control_label
    assert result["unmatched"] == []


# ---------------------------------------------------------------------------
# Layer 3: Acoustic matching
# ---------------------------------------------------------------------------


def test_layer3_acoustic_matching(speakers_env):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    _setup_owner(env)

    # Create entity with voiceprints similar to [0, 1, 0, ...]
    vp_emb = _normalized([0.0, 1.0])
    entity_dir = env.create_entity("Alice Test")
    np.savez_compressed(
        entity_dir / "voiceprints.npz",
        embeddings=np.vstack([vp_emb] * 10).astype(np.float32),
        metadata=np.array(
            [
                json.dumps(
                    {
                        "day": "20240101",
                        "segment_key": f"09{i:02d}00_300",
                        "source": "mic_audio",
                        "sentence_id": 1,
                        "stream": STREAM,
                        "added_at": 1700000000000,
                    }
                )
                for i in range(10)
            ],
            dtype=str,
        ),
    )

    owner_emb = _normalized([0.95, 0.05])
    alice_emb = _normalized([0.05, 0.95])  # similar to voiceprint
    embeddings = np.vstack([owner_emb, alice_emb])

    _write_controlled_segment(env, "20240101", "090000_300", embeddings)
    # No speakers.json, so Layer 2 can't resolve — falls through to Layer 3

    result = attribute_segment("20240101", STREAM, "090000_300")
    labels = result["labels"]

    assert labels[0]["method"] == "owner_centroid"
    assert labels[1]["speaker"] == "alice_test"
    assert labels[1]["method"] == "acoustic"
    assert labels[1]["confidence"] == "high"
    assert (
        result["metadata"]["owner_centroid_last_refreshed_at"] == "2026-03-15T12:00:00Z"
    )


def test_l3_per_statement_demotes_inside_acoustic_margin_band(speakers_env):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    _setup_owner(env)
    distractor_dir = env.create_entity("Aaron Distractor")
    match_dir = env.create_entity("Mona Match")
    runner_dir = env.create_entity("Zara Runner")
    _write_entity_voiceprints(
        distractor_dir,
        [_normalized([0.0, 0.0, 0.0, 0.0, 1.0])] * 5,
    )
    _write_entity_voiceprints(match_dir, [_normalized([0.0, 1.0])] * 5)
    _write_entity_voiceprints(runner_dir, [_normalized([0.0, 0.0, 1.0])] * 5)
    embedding = _embedding_with_orthogonal_entity_cosines(0.40, 0.37)
    _write_controlled_segment(
        env,
        "20240101",
        "105500_300",
        embedding.reshape(1, -1),
    )

    result = attribute_segment("20240101", STREAM, "105500_300")
    label = result["labels"][0]

    assert label["speaker"] == "mona_match"
    assert label["method"] == "acoustic"
    assert label["confidence"] == "medium"
    assert label["acoustic_margin_declined"] is True
    assert "owner_margin_declined" not in label


def test_l3_per_statement_preserves_high_outside_acoustic_margin_band(speakers_env):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    _setup_owner(env)
    match_dir = env.create_entity("Mona Match")
    runner_dir = env.create_entity("Zara Runner")
    _write_entity_voiceprints(match_dir, [_normalized([0.0, 1.0])] * 5)
    _write_entity_voiceprints(runner_dir, [_normalized([0.0, 0.0, 1.0])] * 5)
    embedding = _embedding_with_orthogonal_entity_cosines(0.42, 0.34)
    _write_controlled_segment(
        env,
        "20240101",
        "110500_300",
        embedding.reshape(1, -1),
    )

    result = attribute_segment("20240101", STREAM, "110500_300")
    label = result["labels"][0]

    assert label["speaker"] == "mona_match"
    assert label["method"] == "acoustic"
    assert label["confidence"] == "high"
    assert "acoustic_margin_declined" not in label


def test_l3_per_statement_single_usable_centroid_preserves_high(speakers_env):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    _setup_owner(env)
    match_dir = env.create_entity("Mona Match")
    _write_entity_voiceprints(match_dir, [_normalized([0.0, 1.0])] * 5)
    embedding = _embedding_with_orthogonal_entity_cosines(0.40, 0.37)
    _write_controlled_segment(
        env,
        "20240101",
        "111500_300",
        embedding.reshape(1, -1),
    )

    result = attribute_segment("20240101", STREAM, "111500_300")
    label = result["labels"][0]

    assert label["speaker"] == "mona_match"
    assert label["method"] == "acoustic"
    assert label["confidence"] == "high"
    assert "acoustic_margin_declined" not in label


def test_l3_acoustic_cluster_demotes_contested_consumed_entity(speakers_env):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    _setup_owner(env)
    x_dir = env.create_entity("Xavier Rawmax")
    y_dir = env.create_entity("Yara Assigned")
    _write_entity_voiceprints(x_dir, [_normalized([0.0, 1.0])] * 5)
    _write_entity_voiceprints(y_dir, [_normalized([0.0, 0.0, 1.0])] * 5)
    cluster_a = _embedding_with_orthogonal_entity_cosines(0.90, 0.10)
    cluster_b = _embedding_with_orthogonal_entity_cosines(0.40, 0.37)
    _write_labeled_controlled_segment(
        env,
        "20240101",
        "112500_300",
        np.vstack([cluster_a, cluster_b]),
        [1, 2],
    )

    result = attribute_segment("20240101", STREAM, "112500_300")
    labels = result["labels"]

    assert labels[0]["speaker"] == "xavier_rawmax"
    assert labels[0]["method"] == "acoustic_cluster"
    assert labels[0]["confidence"] == "high"
    assert "acoustic_margin_declined" not in labels[0]
    assert labels[1]["speaker"] == "yara_assigned"
    assert labels[1]["method"] == "acoustic_cluster"
    assert labels[1]["confidence"] == "medium"
    assert labels[1]["acoustic_margin_declined"] is True


def test_l3_acoustic_cluster_margin_demotion_happens_after_mean_gate(speakers_env):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    _setup_owner(env)
    x_dir = env.create_entity("Xavier Rawmax")
    y_dir = env.create_entity("Yara Assigned")
    _write_entity_voiceprints(x_dir, [_normalized([0.0, 1.0])] * 5)
    _write_entity_voiceprints(y_dir, [_normalized([0.0, 0.0, 1.0])] * 5)
    cluster_a = _embedding_with_orthogonal_entity_cosines(0.38, 0.35)
    cluster_b = _embedding_with_orthogonal_entity_cosines(0.20, 0.19)
    _write_labeled_controlled_segment(
        env,
        "20240101",
        "113500_300",
        np.vstack([cluster_a, cluster_b]),
        [1, 2],
    )

    result = attribute_segment("20240101", STREAM, "113500_300")
    labels = result["labels"]

    # The assigned mean is (0.38 + 0.19) / 2 = 0.285; pruning A first
    # would leave 0.19 and fail the gate.
    assert labels[0]["speaker"] == "xavier_rawmax"
    assert labels[0]["method"] == "acoustic_cluster"
    assert labels[0]["confidence"] == "medium"
    assert labels[0]["acoustic_margin_declined"] is True
    assert labels[1]["speaker"] is None


def test_l3_per_statement_downgrades_margin_declined_high_match_to_medium(
    speakers_env,
):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    _setup_margin_owner(env)
    trap, _competitor_dir, _distractor_dir = _setup_margin_trap_entities(env)
    _write_controlled_segment(
        env,
        "20240101",
        "102500_300",
        trap.reshape(1, -1),
    )

    result = attribute_segment("20240101", STREAM, "102500_300")
    label = result["labels"][0]

    assert label["speaker"] == "zara_vale"
    assert label["method"] == "acoustic"
    assert label["confidence"] == "medium"
    assert label["owner_margin_declined"] is True
    assert "acoustic_margin_declined" not in label
    assert result["unmatched"] == []


def test_l3_acoustic_cluster_downgrades_margin_declined_high_match_to_medium(
    speakers_env,
):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    _setup_margin_owner(env)
    trap, _competitor_dir, _distractor_dir = _setup_margin_trap_entities(env)
    _write_labeled_controlled_segment(
        env,
        "20240101",
        "103500_300",
        trap.reshape(1, -1),
        [1],
    )

    result = attribute_segment("20240101", STREAM, "103500_300")
    label = result["labels"][0]

    assert label["speaker"] == "zara_vale"
    assert label["method"] == "acoustic_cluster"
    assert label["confidence"] == "medium"
    assert label["owner_margin_declined"] is True
    assert result["unmatched"] == []


def test_layer3_hybrid_engages_and_assigns_clusters_one_to_one(speakers_env):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    _setup_owner(env)

    alice_emb = _normalized([0.0, 1.0])
    bob_emb = _normalized([0.0, 0.0, 1.0])
    alice_dir = env.create_entity("Alice Test")
    bob_dir = env.create_entity("Bob Smith")
    _write_entity_voiceprints(alice_dir, [alice_emb] * 6)
    _write_entity_voiceprints(bob_dir, [bob_emb] * 6)

    embeddings = np.vstack([alice_emb, alice_emb, bob_emb, bob_emb])
    _write_labeled_controlled_segment(
        env,
        "20240101",
        "091000_300",
        embeddings,
        [1, 1, 2, 2],
    )

    result = attribute_segment("20240101", STREAM, "091000_300")
    labels = result["labels"]

    assert [label["method"] for label in labels] == ["acoustic_cluster"] * 4
    assert [label["confidence"] for label in labels] == ["high"] * 4
    assert [label["speaker"] for label in labels[:2]] == ["alice_test"] * 2
    assert [label["speaker"] for label in labels[2:]] == ["bob_smith"] * 2
    assert labels[0]["speaker"] != labels[2]["speaker"]
    assert result["unmatched"] == []


def test_layer3_hybrid_abandons_on_low_confidence(speakers_env):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    _setup_owner(env)

    alice_emb = _normalized([0.0, 1.0])
    bob_emb = _normalized([0.0, 0.0, 1.0])
    unknown_emb = _normalized([0.0, 0.0, 0.0, 1.0])
    alice_dir = env.create_entity("Alice Test")
    bob_dir = env.create_entity("Bob Smith")
    _write_entity_voiceprints(alice_dir, [alice_emb] * 6)
    _write_entity_voiceprints(bob_dir, [bob_emb] * 6)

    embeddings = np.vstack([unknown_emb, unknown_emb])
    _write_labeled_controlled_segment(
        env,
        "20240101",
        "092000_300",
        embeddings,
        [1, 1],
    )
    _write_controlled_segment(env, "20240101", "092500_300", embeddings)

    labeled_result = attribute_segment("20240101", STREAM, "092000_300")
    fallback_result = attribute_segment("20240101", STREAM, "092500_300")

    assert all(
        label["method"] != "acoustic_cluster" for label in labeled_result["labels"]
    )
    assert labeled_result["labels"] == fallback_result["labels"]
    assert labeled_result["unmatched"] == fallback_result["unmatched"] == [1, 2]


def test_layer3_hybrid_skips_label_less_segment(speakers_env):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    _setup_owner(env)

    alice_emb = _normalized([0.0, 1.0])
    alice_dir = env.create_entity("Alice Test")
    _write_entity_voiceprints(alice_dir, [alice_emb] * 6)
    _write_controlled_segment(
        env,
        "20240101",
        "093000_300",
        np.vstack([alice_emb]),
    )

    result = attribute_segment("20240101", STREAM, "093000_300")
    label = result["labels"][0]

    assert label["speaker"] == "alice_test"
    assert label["method"] == "acoustic"
    assert label["confidence"] == "high"
    assert result["unmatched"] == []


# ---------------------------------------------------------------------------
# Graceful degradation: unmatched → null
# ---------------------------------------------------------------------------


def test_unmatched_sentences_get_null(speakers_env):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    _setup_owner(env)

    owner_emb = _normalized([0.95, 0.05])
    unknown_emb = _normalized([0.1, 0.5, 0.5])  # no matching voiceprint
    embeddings = np.vstack([owner_emb, unknown_emb])

    _write_controlled_segment(env, "20240101", "090000_300", embeddings)

    result = attribute_segment("20240101", STREAM, "090000_300")

    assert result["labels"][1]["speaker"] is None
    assert result["labels"][1]["confidence"] is None
    assert result["labels"][1]["method"] is None
    assert 2 in result["unmatched"]
    assert 2 in result["unmatched_texts"]


# ---------------------------------------------------------------------------
# Candidate evidence persistence
# ---------------------------------------------------------------------------


def test_candidate_evidence_persisted_for_resolved_entities_only(speakers_env):
    from unittest.mock import patch

    from solstone.apps.speakers.attribution import process_attributed_segment

    env = speakers_env()
    _setup_owner(env)
    env.create_entity("Alice Test")
    _write_controlled_segment(
        env,
        "20240101",
        "094000_300",
        np.vstack([_normalized([1.0, 0.0])]),
    )
    env.create_screen_json(
        "20240101",
        "094000_300",
        ["Alice Test", "No Match Zyxq"],
        stream=STREAM,
    )

    with (
        patch(
            "solstone.apps.speakers.attribution.native_speakers.write_full_labels"
        ) as write_full_labels,
        patch("solstone.apps.speakers.attribution.accumulate_voiceprints"),
    ):
        process_attributed_segment(
            "20240101",
            STREAM,
            "094000_300",
            commit=True,
            read_only=False,
        )

    assert write_full_labels.call_args.args[3]["candidate_evidence"] == [
        {"entity_id": "alice_test", "sources": ["screen"]}
    ]
    assert "candidate_evidence_gaps" not in write_full_labels.call_args.args[3]


def test_legacy_speaker_labels_without_candidate_evidence_still_load(speakers_env):
    from solstone.apps.speakers.attribution import _read_speaker_labels

    env = speakers_env()
    labels = [
        {
            "sentence_id": 1,
            "speaker": "alice_test",
            "confidence": "high",
            "method": "acoustic",
        }
    ]
    env.create_speaker_labels(
        "20240101",
        "100000_300",
        labels,
        metadata={"owner_centroid_last_refreshed_at": None, "voiceprint_versions": {}},
    )
    seg_dir = env.journal / "chronicle" / "20240101" / STREAM / "100000_300"

    assert _read_speaker_labels(seg_dir) == {
        "labels": labels,
        "owner_centroid_last_refreshed_at": None,
        "voiceprint_versions": {},
    }


def test_meeting_participant_reader_missing_day_is_read_only(speakers_env):
    from solstone.apps.speakers.attribution import (
        _extract_meeting_participants_with_gaps,
    )

    env = speakers_env()
    missing_day = "19990101"
    missing_day_dir = env.journal / "chronicle" / missing_day
    before = journal_tree_hash(env.journal)

    names, gaps = _extract_meeting_participants_with_gaps(
        missing_day,
        "000000_300",
    )

    assert names == []
    assert gaps == []
    assert not missing_day_dir.exists()
    assert journal_tree_hash(env.journal) == before


def test_candidate_evidence_records_malformed_gaps_and_keeps_siblings(
    speakers_env,
):
    from solstone.apps.speakers.attribution import attribute_segment

    env = speakers_env()
    _setup_owner(env)
    env.create_entity("Alice Test")
    env.create_import_segment(
        "20240101",
        "100500_300",
        [("", "Hello.")],
        stream=STREAM,
        embeddings=np.vstack([_normalized([1.0, 0.0])]),
        setting="Meeting with Alice Test",
    )
    env.create_screen_json(
        "20240101",
        "100500_300",
        [],
        stream=STREAM,
        malformed=True,
    )

    result = attribute_segment("20240101", STREAM, "100500_300")

    assert result["metadata"]["candidate_evidence_gaps"] == [
        {"source": "screen", "reason": "malformed_json"}
    ]
    assert result["metadata"]["candidate_evidence"] == [
        {"entity_id": "alice_test", "sources": ["setting"]}
    ]

    env.create_import_segment(
        "20240101",
        "101000_300",
        [("", "Hello.")],
        stream=STREAM,
        embeddings=np.vstack([_normalized([1.0, 0.0])]),
        setting="Meeting with Alice Test",
    )

    no_gap = attribute_segment("20240101", STREAM, "101000_300")

    assert no_gap["metadata"]["candidate_evidence_gaps"] == []
    assert no_gap["metadata"]["candidate_evidence"] == [
        {"entity_id": "alice_test", "sources": ["setting"]}
    ]


def test_candidate_evidence_keeps_setting_when_speakers_are_malformed(
    speakers_env,
):
    from solstone.apps.speakers.attribution import (
        compute_segment_candidate_evidence_readonly,
    )

    env = speakers_env()
    env.create_entity("Alice Test")
    env.create_entity("Bob Test")
    env.create_import_segment(
        "20240101",
        "101500_300",
        [("", "Hello.")],
        stream=STREAM,
        setting="Meeting with Alice Test",
    )
    env.create_speakers_json(
        "20240101",
        "101500_300",
        [],
        raw=json.dumps(["Bob Test", 5]),
    )

    evidence, gaps = compute_segment_candidate_evidence_readonly(
        "20240101",
        STREAM,
        "101500_300",
    )

    assert gaps == [{"source": "speakers", "reason": "wrong_shape"}]
    assert evidence == [
        {"entity_id": "alice_test", "sources": ["setting"]},
        {"entity_id": "bob_test", "sources": ["speakers"]},
    ]


def test_candidate_evidence_keeps_speakers_when_setting_is_wrong_shaped(
    speakers_env,
):
    from solstone.apps.speakers.attribution import (
        compute_segment_candidate_evidence_readonly,
    )

    env = speakers_env()
    env.create_entity("Bob Test")
    seg_dir = env.create_segment("20240101", "102000_300", ["imported_audio"])
    (seg_dir / "imported_audio.jsonl").write_text(
        json.dumps({"setting": 5}) + "\n",
        encoding="utf-8",
    )
    env.create_speakers_json("20240101", "102000_300", ["Bob Test"])

    evidence, gaps = compute_segment_candidate_evidence_readonly(
        "20240101",
        STREAM,
        "102000_300",
    )

    assert gaps == [{"source": "setting", "reason": "wrong_shape"}]
    assert evidence == [{"entity_id": "bob_test", "sources": ["speakers"]}]


def test_save_speaker_labels_forwards_native_request(tmp_path: Path) -> None:
    from unittest.mock import patch

    from solstone.apps.speakers.attribution import save_speaker_labels
    from solstone.think.utils import get_journal

    labels = [
        {
            "sentence_id": 1,
            "speaker": "alice",
            "confidence": "high",
            "method": "acoustic",
        }
    ]
    metadata = {"voiceprint_versions": {"alice": 1}}

    with patch(
        "solstone.apps.speakers.attribution.native_speakers.write_full_labels"
    ) as write_full_labels:
        path = save_speaker_labels(tmp_path, labels, metadata)

    assert path == tmp_path / "talents" / "speaker_labels.json"
    write_full_labels.assert_called_once_with(
        get_journal(),
        tmp_path,
        labels,
        {
            "owner_centroid_last_refreshed_at": None,
            "voiceprint_versions": {"alice": 1},
            "candidate_evidence": [],
        },
    )


def test_restore_label_rows_forwards_native_request(tmp_path: Path) -> None:
    from unittest.mock import patch

    from solstone.apps.speakers.attribution import restore_label_rows
    from solstone.think.utils import get_journal

    restorations = [
        {
            "sentence_id": 1,
            "expected_current_label": {"sentence_id": 1, "speaker": "target"},
            "prior_state": "absent",
            "prior_label": None,
        }
    ]
    report = {"restored_count": 1, "skipped_count": 0}

    with patch(
        "solstone.apps.speakers.attribution.native_speakers.restore_label_rows",
        return_value=report,
    ) as restore_rows:
        assert restore_label_rows(tmp_path, restorations) == report

    restore_rows.assert_called_once_with(get_journal(), tmp_path, restorations)


# ---------------------------------------------------------------------------
# Voiceprint accumulation
# ---------------------------------------------------------------------------


def test_voiceprint_accumulation_methods_are_explicit() -> None:
    from solstone.apps.speakers.attribution import VOICEPRINT_ACCUMULATION_METHODS

    assert VOICEPRINT_ACCUMULATION_METHODS == {
        "structural_single_speaker",
        "structural_setting",
        "acoustic",
        "acoustic_cluster",
    }


def test_accumulate_contamination_guard(speakers_env):
    from solstone.apps.speakers.attribution import accumulate_voiceprints

    env = speakers_env()
    _setup_owner(env)
    env.create_entity("Bob Smith")

    # Embedding very similar to owner centroid [1, 0, ...]
    owner_like = _normalized([0.99, 0.01])
    _write_controlled_segment(env, "20240101", "090000_300", np.vstack([owner_like]))

    labels = [
        {
            "sentence_id": 1,
            "speaker": "bob_smith",
            "confidence": "high",
            "method": "structural_single_speaker",
        }
    ]

    saved = accumulate_voiceprints(
        "20240101", STREAM, "090000_300", labels, "mic_audio"
    )

    # Should not save — embedding is too similar to owner
    assert saved == {}


def test_accumulate_skips_medium_confidence(speakers_env):
    from solstone.apps.speakers.attribution import accumulate_voiceprints

    env = speakers_env()
    _setup_owner(env)
    env.create_entity("Bob Smith")

    other_emb = _normalized([0.1, 0.99])
    _write_controlled_segment(env, "20240101", "090000_300", np.vstack([other_emb]))

    labels = [
        {
            "sentence_id": 1,
            "speaker": "bob_smith",
            "confidence": "medium",  # Not high — should not accumulate
            "method": "acoustic",
        }
    ]

    saved = accumulate_voiceprints(
        "20240101", STREAM, "090000_300", labels, "mic_audio"
    )

    assert saved == {}


@pytest.mark.parametrize("method", ["context", "contextual"])
def test_accumulate_skips_context_methods(speakers_env, method: str):
    from solstone.apps.speakers.attribution import accumulate_voiceprints

    env = speakers_env()
    _setup_owner(env)
    env.create_entity("Bob Smith")

    other_emb = _normalized([0.1, 0.99])
    _write_controlled_segment(env, "20240101", "090000_300", np.vstack([other_emb]))

    labels = [
        {
            "sentence_id": 1,
            "speaker": "bob_smith",
            "confidence": "high",
            "method": method,
        }
    ]

    saved = accumulate_voiceprints(
        "20240101", STREAM, "090000_300", labels, "mic_audio"
    )

    assert saved == {}


def test_accumulate_voiceprints_skips_chaotic_segment(speakers_env):
    from solstone.apps.speakers.attribution import accumulate_voiceprints

    env = speakers_env()
    _setup_owner(env)
    env.create_entity("Bob Smith")

    other_emb = _normalized([0.1, 0.99])
    seg_dir = _write_controlled_segment(
        env, "20240101", "090000_300", np.vstack([other_emb])
    )
    _rewrite_segment_header(
        seg_dir,
        "mic_audio",
        overlap_fraction=0.20,
        overlap_detector=OVERLAP_DETECTOR_ID,
    )

    labels = [
        {
            "sentence_id": 1,
            "speaker": "bob_smith",
            "confidence": "high",
            "method": "structural_single_speaker",
        }
    ]

    saved = accumulate_voiceprints(
        "20240101", STREAM, "090000_300", labels, "mic_audio"
    )

    assert saved == {}
    vp_path = env.journal / "entities" / "bob_smith" / "voiceprints.npz"
    assert not vp_path.exists()


# ---------------------------------------------------------------------------
# Backfill
# ---------------------------------------------------------------------------


def test_backfill_dry_run_enumerates(speakers_env):
    """Dry run counts segments without writing anything."""
    from solstone.apps.speakers.attribution import backfill_segments

    env = speakers_env()
    _setup_owner(env)

    # Two segments with embeddings, one without
    _write_controlled_segment(
        env, "20260201", "090000_300", np.vstack([_normalized([1.0, 0.0])])
    )
    _write_controlled_segment(
        env, "20260202", "100000_300", np.vstack([_normalized([0.1, 0.99])])
    )
    # Segment without embeddings (no npz)
    no_emb = env.journal / "20260201" / STREAM / "110000_300"
    no_emb.mkdir(parents=True, exist_ok=True)
    (no_emb / "mic_audio.flac").write_bytes(b"")

    stats = backfill_segments(dry_run=True)

    assert stats["total_eligible"] == 2
    assert stats["skipped_no_embed"] == 1
    assert stats["already_labeled"] == 0
    assert stats["processed"] == 0


def test_backfill_skips_already_labeled(speakers_env):
    """Segments with existing speaker_labels.json are skipped."""
    from solstone.apps.speakers.attribution import backfill_segments

    env = speakers_env()
    _setup_owner(env)

    seg_dir = _write_controlled_segment(
        env, "20260201", "090000_300", np.vstack([_normalized([1.0, 0.0])])
    )
    # Pre-create speaker_labels.json
    agents_dir = seg_dir / "talents"
    agents_dir.mkdir(parents=True, exist_ok=True)
    (agents_dir / "speaker_labels.json").write_text('{"labels": []}')

    stats = backfill_segments(dry_run=True)

    assert stats["total_eligible"] == 1
    assert stats["already_labeled"] == 1


def test_backfill_last_seen_forwards_pending_metadata_to_native(speakers_env):
    from unittest.mock import patch

    from solstone.apps.speakers.attribution import backfill_last_seen
    from solstone.think.utils import segment_start_ts_ms

    env = speakers_env()
    env.create_entity(
        "Alice Test",
        voiceprints=[("20240101", "090000_300", "audio", 1)],
    )
    env.create_speaker_labels(
        "20240101",
        "090000_300",
        [{"sentence_id": 1, "speaker": "alice_test", "confidence": "high"}],
    )
    env.create_speaker_labels(
        "20240102",
        "100000_300",
        [{"sentence_id": 1, "speaker": "alice_test", "confidence": "high"}],
    )

    preview = backfill_last_seen(dry_run=True)
    with patch(
        "solstone.apps.speakers.attribution.native_speakers.backfill_voiceprint_last_seen",
        return_value={"rows_written": 1},
    ) as backfill:
        committed = backfill_last_seen(dry_run=False)

    expected = segment_start_ts_ms("20240102", "100000_300")
    assert preview["rows_pending"] == 1
    assert committed["rows_written"] == 1
    assert backfill.call_args.kwargs["entity_id"] == "alice_test"
    assert backfill.call_args.kwargs["last_seen_ts"] == expected
