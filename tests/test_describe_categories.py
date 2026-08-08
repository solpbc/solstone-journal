# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import pytest

from solstone.think import describe_categories


def test_load_categories_returns_native_fixture_metadata() -> None:
    categories = describe_categories.load_categories()

    assert categories is describe_categories.CATEGORIES
    assert describe_categories.DEFAULT_MAX_EXTRACTIONS == 20
    assert categories["calendar"]["importance"] == "high"
    assert "importance" not in categories["browsing"]
    assert categories["calendar"]["label"] == "Calendar"
    assert categories["calendar"]["group"] == "Screen Analysis"
    assert "json_schema" in categories["calendar"]


def test_load_categories_reports_a_missing_fixture(tmp_path, monkeypatch) -> None:
    monkeypatch.setattr(describe_categories, "_FIXTURE_PATH", tmp_path / "missing.json")
    describe_categories._load_fixture.cache_clear()

    with pytest.raises(describe_categories.DescribeCategoriesFixtureError, match="missing"):
        describe_categories.load_categories()

    describe_categories._load_fixture.cache_clear()


def test_load_categories_reports_a_malformed_fixture(tmp_path, monkeypatch) -> None:
    fixture = tmp_path / "describe_categories.json"
    fixture.write_text("not json", encoding="utf-8")
    monkeypatch.setattr(describe_categories, "_FIXTURE_PATH", fixture)
    describe_categories._load_fixture.cache_clear()

    with pytest.raises(
        describe_categories.DescribeCategoriesFixtureError, match="malformed"
    ):
        describe_categories.load_categories()

    describe_categories._load_fixture.cache_clear()
