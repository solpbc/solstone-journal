# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json
from pathlib import Path

from jsonschema import Draft202012Validator

from solstone.think.talent import get_talent

TALENT_DIR = Path(__file__).resolve().parents[1] / "solstone" / "talent"
PARTICIPATION_ENTRY_SCHEMA_PATH = TALENT_DIR / "participation_entry.schema.json"
PARTICIPATION_SCHEMA_PATH = TALENT_DIR / "participation.schema.json"


def _load_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def test_participation_entry_schema_is_valid_draft_2020_12():
    schema = _load_json(PARTICIPATION_ENTRY_SCHEMA_PATH)

    Draft202012Validator.check_schema(schema)


def test_participation_schema_is_valid_and_matches_loaded():
    schema = _load_json(PARTICIPATION_SCHEMA_PATH)

    Draft202012Validator.check_schema(schema)

    assert get_talent("participation")["json_schema"] == schema


def test_participation_schema_items_match_fragment():
    schema = _load_json(PARTICIPATION_SCHEMA_PATH)
    fragment = _load_json(PARTICIPATION_ENTRY_SCHEMA_PATH)

    items = dict(schema["properties"]["participation"]["items"])
    fragment_without_schema = dict(fragment)

    assert items == fragment_without_schema
