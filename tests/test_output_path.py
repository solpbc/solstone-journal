# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for output path generation with facet support."""

import os
from pathlib import Path

from solstone.think.talent import get_output_name, get_output_path


class TestGetOutputName:
    """Tests for get_output_name."""

    def test_simple_key(self):
        assert get_output_name("activity") == "activity"

    def test_app_key(self):
        assert get_output_name("chat:sentiment") == "_chat_sentiment"

    def test_entities_app_key(self):
        assert get_output_name("entities:observer") == "_entities_observer"


class TestGetOutputPath:
    """Tests for get_output_path."""

    def test_daily_output_md(self):
        path = get_output_path("/journal/20250101", "activity", output_format="md")
        assert path == Path("/journal/20250101/talents/activity.md")

    def test_daily_output_json(self):
        path = get_output_path("/journal/20250101", "facets", output_format="json")
        assert path == Path("/journal/20250101/talents/facets.json")

    def test_segment_output(self):
        path = get_output_path(
            "/journal/20250101", "activity", segment="120000_300", output_format="md"
        )
        assert path == Path("/journal/20250101/120000_300/talents/activity.md")

    def test_app_key_output(self):
        path = get_output_path(
            "/journal/20250101", "entities:observer", output_format="md"
        )
        assert path == Path("/journal/20250101/talents/_entities_observer.md")

    def test_facet_daily_output(self):
        """Multi-facet agent output uses a facet subdirectory."""
        path = get_output_path(
            "/journal/20250101", "newsletter", output_format="md", facet="work"
        )
        assert path == Path("/journal/20250101/talents/work/newsletter.md")

    def test_facet_segment_output(self):
        """Multi-facet segment output uses a facet subdirectory."""
        path = get_output_path(
            "/journal/20250101",
            "summary",
            segment="120000_300",
            output_format="json",
            facet="personal",
        )
        assert path == Path(
            "/journal/20250101/120000_300/talents/personal/summary.json"
        )

    def test_facet_with_app_key(self):
        """App-qualified key with facet uses both prefixes."""
        path = get_output_path(
            "/journal/20250101", "entities:observer", output_format="md", facet="work"
        )
        assert path == Path("/journal/20250101/talents/work/_entities_observer.md")

    def test_facet_none_same_as_omitted(self):
        """Explicit facet=None produces same path as omitting facet."""
        path_none = get_output_path(
            "/journal/20250101", "activity", output_format="md", facet=None
        )
        path_omit = get_output_path("/journal/20250101", "activity", output_format="md")
        assert path_none == path_omit


class TestGetActivityOutputPath:
    """Tests for get_activity_output_path."""

    def test_markdown_output(self):
        from solstone.think.activities import get_activity_output_path

        path = get_activity_output_path(
            "work", "20260209", "coding_100000_300", "session_review"
        )
        journal = os.environ["SOLSTONE_JOURNAL"]
        expected = (
            Path(journal)
            / "facets/work/activities/20260209/coding_100000_300/session_review.md"
        )
        assert path == expected

    def test_json_output(self):
        from solstone.think.activities import get_activity_output_path

        path = get_activity_output_path(
            "work",
            "20260209",
            "meeting_090000_300",
            "analysis",
            output_format="json",
        )
        assert path.name == "analysis.json"
        assert "facets/work/activities/20260209/meeting_090000_300" in str(path)

    def test_app_key(self):
        from solstone.think.activities import get_activity_output_path

        path = get_activity_output_path(
            "personal", "20260210", "coding_100000_300", "chat:review"
        )
        assert path.name == "_chat_review.md"
        assert "facets/personal/activities/20260210/coding_100000_300" in str(path)
