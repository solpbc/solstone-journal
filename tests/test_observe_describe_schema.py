# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from pathlib import Path
from unittest.mock import AsyncMock, patch

import pytest
from jsonschema import Draft202012Validator

from solstone.observe import describe as describe_mod
from solstone.think.batch import Batch

_SCHEMA = describe_mod._SCHEMA


def test_describe_schema_file_is_valid_draft_2020_12():
    Draft202012Validator.check_schema(_SCHEMA)


def test_describe_schema_accepts_and_rejects_expected_values():
    validator = Draft202012Validator(_SCHEMA)

    assert validator.is_valid(
        {
            "visual_description": "A browser window with multiple open tabs.",
            "primary": "browsing",
            "secondary": "reading",
            "overlap": True,
        }
    )
    assert validator.is_valid(
        {
            "visual_description": "A code editor with a terminal pane.",
            "primary": "code",
            "secondary": "none",
            "overlap": False,
        }
    )
    assert not validator.is_valid(
        {
            "visual_description": "A dashboard view.",
            "primary": "unknown",
            "secondary": "none",
            "overlap": False,
        }
    )
    assert not validator.is_valid(
        {
            "visual_description": "A dashboard view.",
            "primary": "productivity",
            "secondary": "unknown",
            "overlap": False,
        }
    )
    assert not validator.is_valid(
        {
            "visual_description": "A dashboard view.",
            "secondary": "none",
            "overlap": False,
        }
    )
    assert not validator.is_valid(
        {
            "visual_description": "A dashboard view.",
            "primary": "productivity",
            "secondary": "none",
        }
    )
    assert not validator.is_valid(
        {
            "visual_description": "A dashboard view.",
            "primary": "productivity",
            "secondary": "none",
            "overlap": False,
            "confidence": 0.9,
        }
    )
    assert not validator.is_valid(
        {
            "visual_description": "A dashboard view.",
            "primary": "productivity",
            "secondary": "none",
            "overlap": "yes",
        }
    )
    assert not validator.is_valid(
        {
            "visual_description": 7,
            "primary": "productivity",
            "secondary": "none",
            "overlap": False,
        }
    )


@pytest.mark.asyncio
@patch("solstone.think.batch.agenerate", new_callable=AsyncMock)
async def test_describe_batch_call_passes_schema(mock_agenerate):
    mock_agenerate.return_value = (
        '{"visual_description":"A code editor is visible.","primary":"code",'
        '"secondary":"none","overlap":false}'
    )

    batch = Batch(max_concurrent=1)
    req = batch.create(
        contents="Analyze this screenshot frame from a screencast recording.",
        context="observe.describe.frame",
        json_output=True,
        json_schema=_SCHEMA,
    )
    batch.add(req)

    results = []
    async for completed_req in batch.drain_batch():
        results.append(completed_req)

    assert len(results) == 1
    assert mock_agenerate.call_args.kwargs["json_schema"] is describe_mod._SCHEMA


def test_category_enum_matches_registry():
    """The enums in `primary` and `secondary` MUST match the filenames under observe/categories/*.md."""
    categories_dir = Path(describe_mod.__file__).resolve().parent / "categories"
    on_disk = {p.stem for p in categories_dir.glob("*.md")}

    assert set(_SCHEMA["properties"]["primary"]["enum"]) == on_disk
    assert set(_SCHEMA["properties"]["secondary"]["enum"]) - {"none"} == on_disk
    assert "none" in _SCHEMA["properties"]["secondary"]["enum"]
