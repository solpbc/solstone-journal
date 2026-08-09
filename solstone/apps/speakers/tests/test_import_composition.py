# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for import-linking and import-based voiceprint seeding."""

from __future__ import annotations

import json

from solstone.apps.speakers.bootstrap import link_import
from solstone.think.entities.journal import load_journal_entity


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
