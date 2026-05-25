# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for import-linking and import-based voiceprint seeding."""

from __future__ import annotations

import json

import numpy as np
from typer.testing import CliRunner

from solstone.apps.speakers.bootstrap import link_import, seed_from_imports
from solstone.apps.speakers.call import app as speakers_app
from solstone.think.entities.journal import load_journal_entity

_runner = CliRunner()


def _normalized_vector(seed: int) -> np.ndarray:
    rng = np.random.default_rng(seed)
    vec = rng.standard_normal(256).astype(np.float32)
    return vec / np.linalg.norm(vec)


def _mock_owner(monkeypatch, vector: np.ndarray, threshold: float = 0.85) -> None:
    from solstone.apps.speakers.owner import OwnerCentroid

    monkeypatch.setattr(
        "solstone.apps.speakers.bootstrap.load_owner_centroid",
        lambda: OwnerCentroid(
            centroid=vector.astype(np.float32),
            threshold=threshold,
            cluster_size=1,
            last_refreshed_at="2026-03-15T12:00:00Z",
            intra_cosine_p25=None,
            streams=[],
        ),
    )


def test_link_import_success(speakers_env):
    env = speakers_env()
    env.create_entity("Alice Test")

    result = link_import("Alice Imported", "alice_test")

    assert result["linked"] is True
    assert result["already_present"] is False
    entity = load_journal_entity("alice_test")
    assert entity is not None
    assert "Alice Imported" in entity.get("aka", [])


def test_link_import_entity_not_found(speakers_env):
    speakers_env()

    result = link_import("Alice", "nonexistent")

    assert "error" in result


def test_link_import_already_present(speakers_env):
    env = speakers_env()
    entity_dir = env.create_entity("Alice Test")
    entity_path = entity_dir / "entity.json"
    entity = json.loads(entity_path.read_text(encoding="utf-8"))
    entity["aka"] = ["Alice Imported"]
    entity_path.write_text(json.dumps(entity), encoding="utf-8")

    result = link_import("Alice Imported", "alice_test")

    assert result["linked"] is True
    assert result["already_present"] is True


def test_seed_from_imports_basic(speakers_env, monkeypatch):
    env = speakers_env()
    env.create_entity("Alice Test")
    embeddings = np.vstack([_normalized_vector(i) for i in range(3)]).astype(np.float32)
    env.create_import_segment(
        "20240101",
        "120000_300",
        [("Alice Test", "Hello")] * 3,
        embeddings=embeddings,
    )
    _mock_owner(monkeypatch, _normalized_vector(99), threshold=0.99)

    result = seed_from_imports()

    assert result["segments_scanned"] == 1
    assert result["segments_with_speakers"] == 1
    assert result["embeddings_saved"] == 3
    assert result["speakers_found"] == {"Alice Test": 3}
    vp_path = env.journal / "entities" / "alice_test" / "voiceprints.npz"
    assert vp_path.exists()


def test_seed_from_imports_dry_run(speakers_env, monkeypatch):
    env = speakers_env()
    env.create_entity("Alice Test")
    embeddings = np.vstack([_normalized_vector(i) for i in range(3)]).astype(np.float32)
    env.create_import_segment(
        "20240101",
        "120000_300",
        [("Alice Test", "Hello")] * 3,
        embeddings=embeddings,
    )
    _mock_owner(monkeypatch, _normalized_vector(77), threshold=0.99)

    result = seed_from_imports(dry_run=True)

    assert result["embeddings_saved"] == 3
    vp_path = env.journal / "entities" / "alice_test" / "voiceprints.npz"
    assert not vp_path.exists()


def test_seed_from_imports_owner_skip(speakers_env, monkeypatch):
    env = speakers_env()
    env.create_entity("Alice Test")
    owner_vec = _normalized_vector(123)
    embeddings = np.vstack([owner_vec, owner_vec, owner_vec]).astype(np.float32)
    env.create_import_segment(
        "20240101",
        "120000_300",
        [("Alice Test", "Hello")] * 3,
        embeddings=embeddings,
    )
    _mock_owner(monkeypatch, owner_vec, threshold=0.8)

    result = seed_from_imports()

    assert result["embeddings_saved"] == 0
    assert result["embeddings_skipped_owner"] == 3


def test_seed_from_imports_dedup(speakers_env, monkeypatch):
    env = speakers_env()
    env.create_entity("Alice Test")
    embeddings = np.vstack([_normalized_vector(i) for i in range(3)]).astype(np.float32)
    env.create_import_segment(
        "20240101",
        "120000_300",
        [("Alice Test", "Hello")] * 3,
        embeddings=embeddings,
    )
    _mock_owner(monkeypatch, _normalized_vector(88), threshold=0.99)

    first = seed_from_imports()
    second = seed_from_imports()

    assert first["embeddings_saved"] == 3
    assert second["embeddings_saved"] == 0
    assert second["embeddings_skipped_duplicate"] == 3


def test_seed_from_imports_unmatched_speaker(speakers_env, monkeypatch):
    env = speakers_env()
    embeddings = np.vstack([_normalized_vector(i) for i in range(2)]).astype(np.float32)
    env.create_import_segment(
        "20240101",
        "120000_300",
        [("Unknown Person", "Hello")] * 2,
        embeddings=embeddings,
    )
    _mock_owner(monkeypatch, _normalized_vector(55), threshold=0.99)

    result = seed_from_imports()

    assert result["embeddings_saved"] == 0
    assert result["speakers_unmatched"] == ["Unknown Person"]


def test_seed_from_imports_ai_chat_skip(speakers_env, monkeypatch):
    env = speakers_env()
    env.create_entity("Alice Test")
    embeddings = np.vstack([_normalized_vector(i) for i in range(2)]).astype(np.float32)
    env.create_import_segment(
        "20240101",
        "120000_300",
        [("Alice Test", "Hello")] * 2,
        stream="import.chatgpt",
        embeddings=embeddings,
    )
    _mock_owner(monkeypatch, _normalized_vector(44), threshold=0.99)

    result = seed_from_imports()

    assert result["segments_scanned"] == 0
    assert result["segments_with_speakers"] == 0
    assert result["embeddings_saved"] == 0


def test_seed_from_imports_empty_speaker(speakers_env, monkeypatch):
    env = speakers_env()
    env.create_entity("Alice Test")
    embeddings = np.vstack([_normalized_vector(i) for i in range(3)]).astype(np.float32)
    env.create_import_segment(
        "20240101",
        "120000_300",
        [("", "skip one"), ("", "skip two"), ("Alice Test", "keep this")],
        embeddings=embeddings,
    )
    _mock_owner(monkeypatch, _normalized_vector(33), threshold=0.99)

    result = seed_from_imports()

    assert result["embeddings_saved"] == 1
    assert result["speakers_found"] == {"Alice Test": 1}


def test_link_import_cli_json_success(speakers_env):
    env = speakers_env()
    env.create_entity("Alice Test")

    result = _runner.invoke(
        speakers_app,
        ["link-import", "Alice Imported", "--entity-id", "alice_test"],
    )

    assert result.exit_code == 0
    data = json.loads(result.output)
    assert data["linked"] is True
    assert data["entity_id"] == "alice_test"


def test_seed_from_imports_cli_json_success(speakers_env, monkeypatch):
    env = speakers_env()
    env.create_entity("Alice Test")
    embeddings = np.vstack([_normalized_vector(i) for i in range(2)]).astype(np.float32)
    env.create_import_segment(
        "20240101",
        "120000_300",
        [("Alice Test", "Hello")] * 2,
        embeddings=embeddings,
    )
    _mock_owner(monkeypatch, _normalized_vector(22), threshold=0.99)

    result = _runner.invoke(speakers_app, ["seed-from-imports", "--commit", "--json"])

    assert result.exit_code == 0
    data = json.loads(result.output)
    assert data["embeddings_saved"] == 2
