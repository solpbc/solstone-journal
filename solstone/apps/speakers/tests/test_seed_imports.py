# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for import-linking and import-based voiceprint seeding."""

from __future__ import annotations

import json

from solstone.apps.speakers.bootstrap import (
    bootstrap_voiceprints,
    link_import,
    seed_from_imports,
)

# --- link-import tests ---


def test_link_import_success(speakers_env):
    """link-import adds name as aka on entity."""
    env = speakers_env()
    env.create_entity("Sarah Chen")

    result = link_import("Sarah C", "sarah_chen")
    assert result["linked"] is True
    assert result["entity_id"] == "sarah_chen"
    assert result["name_added"] == "Sarah C"
    assert result["already_present"] is False

    # Verify entity was actually updated
    entity_path = env.journal / "entities" / "sarah_chen" / "entity.json"
    entity = json.loads(entity_path.read_text())
    assert "Sarah C" in entity["aka"]
    assert "updated_at" in entity


def test_link_import_already_present(speakers_env):
    """link-import reports already_present when name is already an aka."""
    env = speakers_env()
    entity_dir = env.create_entity("Sarah Chen")
    # Manually add aka
    entity_path = entity_dir / "entity.json"
    entity = json.loads(entity_path.read_text())
    entity["aka"] = ["Sarah C"]
    entity_path.write_text(json.dumps(entity))

    result = link_import("Sarah C", "sarah_chen")
    assert result["already_present"] is True


def test_link_import_entity_not_found(speakers_env):
    """link-import exits 1 with error JSON for missing entity."""
    speakers_env()
    result = link_import("Nobody", "nonexistent")
    assert "error" in result


def test_link_import_collision(speakers_env):
    """link-import exits 1 when aka collides with another entity."""
    env = speakers_env()
    env.create_entity("Alice Johnson")
    env.create_entity("Bob Smith")

    result = link_import("Bob Smith", "alice_johnson")
    assert "conflicts" in result["error"]


# --- seed-from-imports tests ---


def test_bootstrap_voiceprints_forwards_native_request() -> None:
    from unittest.mock import patch

    from solstone.apps.speakers.attribution import _speaker_encoder_identity
    from solstone.think.utils import get_journal

    expected = {"status": "completed", "embeddings_saved": 2}
    with patch(
        "solstone.apps.speakers.native.seed_from_imports", return_value=expected
    ) as seed:
        result = seed_from_imports(dry_run=True)

    assert result is expected
    args, kwargs = seed.call_args
    assert args == (get_journal(),)
    assert kwargs["dry_run"] is True
    assert kwargs["encoder"] == _speaker_encoder_identity()

    with patch(
        "solstone.apps.speakers.native.bootstrap_voiceprints", return_value=expected
    ) as bootstrap_call:
        result = bootstrap_voiceprints(dry_run=False)

    assert result is expected
    args, kwargs = bootstrap_call.call_args
    assert args == (get_journal(),)
    assert kwargs["dry_run"] is False
    assert kwargs["encoder"] == _speaker_encoder_identity()
