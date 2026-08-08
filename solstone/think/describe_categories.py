# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Load native describe category metadata generated with core fixtures."""

from __future__ import annotations

import json
from functools import lru_cache
from pathlib import Path
from typing import Any


_FIXTURE_PATH = Path(__file__).with_name("describe_categories.json")
_SCHEMA = "solstone-describe-categories-v1"


class DescribeCategoriesFixtureError(RuntimeError):
    """The packaged native describe category fixture is unavailable or invalid."""


@lru_cache(maxsize=1)
def _load_fixture() -> dict[str, Any]:
    try:
        raw = _FIXTURE_PATH.read_text(encoding="utf-8")
    except FileNotFoundError as error:
        raise DescribeCategoriesFixtureError(
            f"describe categories fixture is missing: {_FIXTURE_PATH}"
        ) from error
    try:
        fixture = json.loads(raw)
    except json.JSONDecodeError as error:
        raise DescribeCategoriesFixtureError(
            f"describe categories fixture is malformed: {_FIXTURE_PATH}"
        ) from error
    if (
        not isinstance(fixture, dict)
        or fixture.get("schema") != _SCHEMA
        or set(fixture) != {"schema", "default_max_extractions", "categories"}
        or not isinstance(fixture.get("default_max_extractions"), int)
        or isinstance(fixture.get("default_max_extractions"), bool)
        or not isinstance(fixture.get("categories"), dict)
        or not all(
            isinstance(name, str) and isinstance(metadata, dict)
            for name, metadata in fixture["categories"].items()
        )
    ):
        raise DescribeCategoriesFixtureError(
            f"describe categories fixture has an invalid envelope: {_FIXTURE_PATH}"
        )
    return fixture


def load_categories() -> dict[str, dict[str, Any]]:
    """Return the native category metadata mapping cached from the fixture."""
    return _load_fixture()["categories"]


CATEGORIES: dict[str, dict[str, Any]] = load_categories()
DEFAULT_MAX_EXTRACTIONS: int = _load_fixture()["default_max_extractions"]
