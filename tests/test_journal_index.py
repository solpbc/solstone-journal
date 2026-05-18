# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the unified journal index."""

import hashlib
import json
import os
from datetime import datetime
from pathlib import Path

import pytest

from solstone.convey.chat_stream import append_chat_event
from solstone.think.indexer import sanitize_fts_query
from solstone.think.indexer.journal import (
    extract_temporal_references,
    get_journal_index,
    search_journal,
)
from tests.conftest import copytree_tracked


class TestSanitizeFtsQuery:
    """Tests for FTS5 query sanitization."""

    def test_simple_words(self):
        """Simple words get NEAR proximity formulation."""
        assert sanitize_fts_query("foo bar baz") == (
            "NEAR(foo bar baz, 10) OR (foo AND bar AND baz)",
            None,
            None,
        )

    def test_preserves_or_operator(self):
        """OR operator is preserved."""
        assert sanitize_fts_query("foo OR bar") == ("foo OR bar", None, None)

    def test_preserves_and_operator(self):
        """AND operator is preserved."""
        assert sanitize_fts_query("foo AND bar") == ("foo AND bar", None, None)

    def test_preserves_not_operator(self):
        """NOT operator is preserved."""
        assert sanitize_fts_query("foo NOT bar") == ("foo NOT bar", None, None)

    def test_preserves_asterisk_prefix_match(self):
        """Asterisk for prefix matching is preserved."""
        assert sanitize_fts_query("test*") == ("test*", None, None)

    def test_preserves_quoted_phrases(self):
        """Quoted phrases are preserved."""
        assert sanitize_fts_query('"public benefit"') == (
            '"public benefit"',
            None,
            None,
        )

    def test_complex_query_with_or_and_quotes(self):
        """Complex query with OR and quoted phrases."""
        result = sanitize_fts_query('solstone OR pbc OR "public benefit"')
        assert result == ('solstone OR pbc OR "public benefit"', None, None)

    def test_dot_replaced_with_space(self):
        """Dots are replaced with spaces."""
        assert sanitize_fts_query("config.json") == (
            "NEAR(config json, 10) OR (config AND json)",
            None,
            None,
        )

    def test_colon_replaced_with_space(self):
        """Colons are replaced with spaces."""
        assert sanitize_fts_query("foo:bar") == (
            "NEAR(foo bar, 10) OR (foo AND bar)",
            None,
            None,
        )

    def test_special_chars_replaced_with_space(self):
        """Various special characters are replaced with spaces."""
        assert sanitize_fts_query("a@b#c$d") == (
            "NEAR(a b c d, 10) OR (a AND b AND c AND d)",
            None,
            None,
        )

    def test_preserves_apostrophe(self):
        """Apostrophes in contractions are preserved."""
        assert sanitize_fts_query("what's up") == (
            "NEAR(what's up, 10) OR (what's AND up)",
            None,
            None,
        )

    def test_unbalanced_quote_removed(self):
        """Unbalanced quotes are removed entirely."""
        assert sanitize_fts_query('"unbalanced') == ("unbalanced", None, None)

    def test_unbalanced_quote_removes_all(self):
        """When quotes are unbalanced, all quotes are removed."""
        assert sanitize_fts_query('foo "bar" baz "qux') == (
            "NEAR(foo bar baz qux, 10) OR (foo AND bar AND baz AND qux)",
            None,
            None,
        )

    def test_balanced_quotes_preserved(self):
        """Balanced quotes are kept."""
        assert sanitize_fts_query('"foo" "bar"') == ('"foo" "bar"', None, None)

    def test_near_two_words(self):
        """Two plain words get NEAR proximity formulation."""
        assert sanitize_fts_query("git commit") == (
            "NEAR(git commit, 10) OR (git AND commit)",
            None,
            None,
        )

    def test_near_three_words(self):
        """Three plain words get NEAR proximity formulation."""
        assert sanitize_fts_query("meeting with Alice") == (
            "NEAR(meeting with Alice, 10) OR (meeting AND with AND Alice)",
            None,
            None,
        )

    def test_near_with_prefix(self):
        """Prefix matching works within NEAR formulation."""
        assert sanitize_fts_query("test* foo") == (
            "NEAR(test* foo, 10) OR (test* AND foo)",
            None,
            None,
        )

    def test_single_word_no_near(self):
        """Single word does not get NEAR treatment."""
        assert sanitize_fts_query("hello") == ("hello", None, None)

    def test_empty_query(self):
        """Empty query returns empty string."""
        assert sanitize_fts_query("") == ("", None, None)

    def test_near_normalizes_whitespace(self):
        """Extra whitespace in input is normalized in NEAR output."""
        assert sanitize_fts_query("foo  bar") == (
            "NEAR(foo bar, 10) OR (foo AND bar)",
            None,
            None,
        )


class TestTemporalExtraction:
    """Tests for temporal date extraction from queries."""

    REF = datetime(2024, 1, 15)  # Monday

    def test_yesterday(self):
        result = sanitize_fts_query("meeting yesterday", self.REF)
        assert result == ("meeting", "20240114", "20240114")

    def test_today(self):
        result = sanitize_fts_query("meeting today", self.REF)
        assert result == ("meeting", "20240115", "20240115")

    def test_last_week(self):
        result = sanitize_fts_query("meeting last week", self.REF)
        assert result == ("meeting", "20240108", "20240114")

    def test_this_week(self):
        result = sanitize_fts_query("meeting this week", self.REF)
        assert result == ("meeting", "20240115", "20240121")

    def test_last_month(self):
        result = sanitize_fts_query("meeting last month", self.REF)
        assert result == ("meeting", "20231201", "20231231")

    def test_this_month(self):
        result = sanitize_fts_query("meeting this month", self.REF)
        assert result == ("meeting", "20240101", "20240131")

    def test_last_monday(self):
        # ref is Monday, so "last monday" = 7 days ago
        result = sanitize_fts_query("meeting last Monday", self.REF)
        assert result == ("meeting", "20240108", "20240108")

    def test_last_tuesday(self):
        result = sanitize_fts_query("meeting last Tuesday", self.REF)
        assert result == ("meeting", "20240109", "20240109")

    def test_last_wednesday(self):
        result = sanitize_fts_query("meeting last Wednesday", self.REF)
        assert result == ("meeting", "20240110", "20240110")

    def test_last_thursday(self):
        result = sanitize_fts_query("meeting last Thursday", self.REF)
        assert result == ("meeting", "20240111", "20240111")

    def test_last_friday(self):
        result = sanitize_fts_query("meeting last Friday", self.REF)
        assert result == ("meeting", "20240112", "20240112")

    def test_last_saturday(self):
        result = sanitize_fts_query("meeting last Saturday", self.REF)
        assert result == ("meeting", "20240113", "20240113")

    def test_last_sunday(self):
        result = sanitize_fts_query("meeting last Sunday", self.REF)
        assert result == ("meeting", "20240114", "20240114")

    def test_over_the_weekend(self):
        result = sanitize_fts_query("meeting over the weekend", self.REF)
        assert result == ("meeting", "20240113", "20240114")

    def test_on_the_weekend(self):
        result = sanitize_fts_query("meeting on the weekend", self.REF)
        assert result == ("meeting", "20240113", "20240114")

    def test_case_insensitive(self):
        result = sanitize_fts_query("meeting Last Monday", self.REF)
        assert result == ("meeting", "20240108", "20240108")

    def test_temporal_only_query(self):
        """Pure temporal query produces empty FTS string + date filter."""
        result = sanitize_fts_query("yesterday", self.REF)
        assert result == ("", "20240114", "20240114")

    def test_no_temporal_reference(self):
        """Query without temporal words returns None dates."""
        result = sanitize_fts_query("machine learning", self.REF)
        assert result == (
            "NEAR(machine learning, 10) OR (machine AND learning)",
            None,
            None,
        )

    def test_temporal_at_start(self):
        result = sanitize_fts_query("yesterday meeting with Alice", self.REF)
        assert result == (
            "NEAR(meeting with Alice, 10) OR (meeting AND with AND Alice)",
            "20240114",
            "20240114",
        )

    def test_temporal_in_middle(self):
        result = sanitize_fts_query("project last week update", self.REF)
        assert result == (
            "NEAR(project update, 10) OR (project AND update)",
            "20240108",
            "20240114",
        )

    def test_quoted_temporal_not_extracted(self):
        """Temporal words inside quotes are not extracted."""
        result = sanitize_fts_query('"last week" meeting', self.REF)
        assert result == ('"last week" meeting', None, None)

    def test_multiple_temporal_first_wins(self):
        """When multiple temporal references exist, first one wins."""
        result = sanitize_fts_query("yesterday last week meeting", self.REF)
        assert result == (
            "NEAR(last week meeting, 10) OR (last AND week AND meeting)",
            "20240114",
            "20240114",
        )

    def test_extract_temporal_references_directly(self):
        """Test extract_temporal_references for the cleaned query."""
        cleaned, day_from, day_to = extract_temporal_references(
            "meeting last week", self.REF
        )
        assert cleaned == "meeting"
        assert day_from == "20240108"
        assert day_to == "20240114"


@pytest.fixture
def journal_fixture(tmp_path, monkeypatch):
    """Create a temporary journal with test data."""
    journal = tmp_path
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    # Create daily insight
    day = journal / "chronicle" / "20240101"
    day.mkdir(parents=True)
    agents_dir = day / "talents"
    agents_dir.mkdir()
    (agents_dir / "flow.md").write_text("# Flow Summary\n\nWorked on project alpha.\n")

    # Create segment with agent output
    stream_dir = day / "default"
    stream_dir.mkdir()
    segment = stream_dir / "100000_300"
    segment.mkdir()
    (segment / "talents").mkdir()
    (segment / "talents" / "screen.md").write_text(
        "# Screen Summary\n\nViewed documentation.\n"
    )
    # Add stream.json for segment stream metadata
    from solstone.think.streams import write_segment_stream

    write_segment_stream(str(segment), "default", None, None, 1)
    # Add second agent file for cross-file segment testing
    (segment / "talents" / "activity.md").write_text(
        "# Activity Summary\n\nMet with Scott Ward about Acme deal.\n"
    )

    # Create evening segment for time_bucket testing
    evening_segment = stream_dir / "200000_300"
    evening_segment.mkdir()
    (evening_segment / "talents").mkdir()
    (evening_segment / "talents" / "screen.md").write_text(
        "# Evening Screen\n\nReviewed evening reports.\n"
    )
    write_segment_stream(str(evening_segment), "default", None, None, 1)

    # Create facet events
    events_dir = journal / "facets" / "work" / "events"
    events_dir.mkdir(parents=True)
    event = {
        "type": "meeting",
        "start": "09:00:00",
        "end": "09:30:00",
        "title": "Standup",
        "summary": "Daily sync meeting",
        "facet": "work",
        "agent": "meetings",
        "occurred": True,
    }
    (events_dir / "20240101.jsonl").write_text(json.dumps(event))

    # Create facet entities
    entities_dir = journal / "facets" / "work" / "entities"
    entities_dir.mkdir(parents=True)
    entity = {
        "name": "Project Alpha",
        "type": "project",
        "description": "Main project",
    }
    (entities_dir / "20240101.jsonl").write_text(json.dumps(entity))

    # Create facet news
    news_dir = journal / "facets" / "work" / "news"
    news_dir.mkdir(parents=True)
    (news_dir / "20240101.md").write_text(
        "# News\n\nImportant update about the project.\n"
    )

    return journal


def test_scan_journal(journal_fixture):
    """Test scanning journal creates index."""
    from solstone.think.indexer.journal import scan_journal

    changed = scan_journal(str(journal_fixture), verbose=True)
    assert changed is True

    # Index file should exist
    index_path = journal_fixture / "indexer" / "journal.sqlite"
    assert index_path.exists()


def test_search_journal_outputs(journal_fixture):
    """Test searching returns agent output chunks."""
    from solstone.think.indexer.journal import scan_journal, search_journal

    scan_journal(str(journal_fixture))

    total, results = search_journal("project alpha")
    assert total >= 1
    # Should find the flow output mentioning "project alpha"
    found = any("alpha" in r["text"].lower() for r in results)
    assert found


def test_search_journal_events(journal_fixture):
    """Test searching returns event chunks."""
    from solstone.think.indexer.journal import scan_journal, search_journal

    scan_journal(str(journal_fixture))

    total, results = search_journal("Standup", agent="event")
    assert total >= 1
    assert any("Standup" in r["text"] for r in results)


def test_search_journal_filter_by_day(journal_fixture):
    """Test filtering search by day."""
    from solstone.think.indexer.journal import scan_journal, search_journal

    scan_journal(str(journal_fixture))

    # Search with day filter
    total, results = search_journal("", day="20240101")
    assert total >= 1
    for r in results:
        assert r["metadata"]["day"] == "20240101"


def test_search_journal_filter_by_facet(journal_fixture):
    """Test filtering search by facet."""
    from solstone.think.indexer.journal import scan_journal, search_journal

    scan_journal(str(journal_fixture))

    # Search with facet filter
    total, results = search_journal("", facet="work")
    assert total >= 1
    for r in results:
        assert r["metadata"]["facet"] == "work"


def test_search_journal_filter_by_agent(journal_fixture):
    """Test filtering search by agent."""
    from solstone.think.indexer.journal import scan_journal, search_journal

    scan_journal(str(journal_fixture))

    # Search events by agent
    total, results = search_journal("", agent="event")
    assert total >= 1
    for r in results:
        assert r["metadata"]["agent"] == "event"


def test_search_journal_facet_case_insensitive(journal_fixture):
    """Test facet filtering is case-insensitive."""
    from solstone.think.indexer.journal import scan_journal, search_journal

    scan_journal(str(journal_fixture))

    # Search with uppercase facet filter should find lowercase-indexed data
    total_upper, results_upper = search_journal("", facet="WORK")
    total_lower, _ = search_journal("", facet="work")
    total_mixed, _ = search_journal("", facet="Work")

    assert total_upper == total_lower == total_mixed
    assert total_upper >= 1
    # All results should have lowercase facet in metadata
    for r in results_upper:
        assert r["metadata"]["facet"] == "work"


def test_search_journal_agent_case_insensitive(journal_fixture):
    """Test agent filtering is case-insensitive."""
    from solstone.think.indexer.journal import scan_journal, search_journal

    scan_journal(str(journal_fixture))

    # Search with uppercase agent filter should find lowercase-indexed data
    total_upper, results_upper = search_journal("", agent="EVENT")
    total_lower, _ = search_journal("", agent="event")
    total_mixed, _ = search_journal("", agent="Event")

    assert total_upper == total_lower == total_mixed
    assert total_upper >= 1
    # All results should have lowercase agent in metadata
    for r in results_upper:
        assert r["metadata"]["agent"] == "event"


def test_time_bucket_population(journal_fixture):
    """Test that indexed chunks have correct time_bucket values."""
    from solstone.think.indexer.journal import get_journal_index, scan_journal

    scan_journal(str(journal_fixture), verbose=True, full=True)
    conn, _ = get_journal_index(str(journal_fixture))

    # Segment chunks should have correct time buckets
    morning_rows = conn.execute(
        "SELECT time_bucket FROM chunks WHERE agent='segment' AND path LIKE '%100000_300%'"
    ).fetchall()
    assert all(r[0] == "morning" for r in morning_rows)

    evening_rows = conn.execute(
        "SELECT time_bucket FROM chunks WHERE agent='segment' AND path LIKE '%200000_300%'"
    ).fetchall()
    assert all(r[0] == "evening" for r in evening_rows)

    # Non-segment content should have empty time_bucket
    entity_rows = conn.execute(
        "SELECT time_bucket FROM chunks WHERE path LIKE 'entity_search:%'"
    ).fetchall()
    assert all(r[0] == "" for r in entity_rows)

    conn.close()


def test_search_filter_by_time_bucket(journal_fixture):
    """Test filtering search by time_bucket."""
    from solstone.think.indexer.journal import scan_journal, search_journal

    scan_journal(str(journal_fixture), verbose=True, full=True)

    # Morning filter should find 100000 segment content
    total, results = search_journal("", time_bucket="morning")
    assert total >= 1
    # All segment results should be from morning segments
    for r in results:
        if r["metadata"]["agent"] == "segment":
            assert "100000_300" in r["metadata"]["path"]

    # Evening filter should find 200000 segment content
    total, results = search_journal("", time_bucket="evening")
    assert total >= 1
    for r in results:
        if r["metadata"]["agent"] == "segment":
            assert "200000_300" in r["metadata"]["path"]

    # Non-matching bucket should return no segment content
    total, results = search_journal("", time_bucket="afternoon")
    segment_results = [r for r in results if r["metadata"]["agent"] == "segment"]
    assert len(segment_results) == 0


def test_time_bucket_non_segment_empty(journal_fixture):
    """Test that non-segment content (day-level agents, facet files) has empty time_bucket."""
    from solstone.think.indexer.journal import get_journal_index, scan_journal

    scan_journal(str(journal_fixture), verbose=True, full=True)
    conn, _ = get_journal_index(str(journal_fixture))

    # Day-level flow agent should have empty time_bucket
    flow_rows = conn.execute(
        "SELECT time_bucket FROM chunks WHERE agent='flow'"
    ).fetchall()
    assert all(r[0] == "" for r in flow_rows)

    # Event chunks should have empty time_bucket
    event_rows = conn.execute(
        "SELECT time_bucket FROM chunks WHERE agent='event'"
    ).fetchall()
    assert all(r[0] == "" for r in event_rows)

    conn.close()


def test_reset_journal_index(journal_fixture):
    """Test resetting the journal index."""
    from solstone.think.indexer.journal import reset_journal_index, scan_journal

    scan_journal(str(journal_fixture))
    index_path = journal_fixture / "indexer" / "journal.sqlite"
    assert index_path.exists()

    reset_journal_index(str(journal_fixture))
    assert not index_path.exists()


def test_index_caching(journal_fixture):
    """Test that unchanged files are not re-indexed."""
    from solstone.think.indexer.journal import scan_journal

    # First scan indexes files
    changed = scan_journal(str(journal_fixture))
    assert changed is True

    # Second scan should be a no-op (all cached)
    changed = scan_journal(str(journal_fixture))
    assert changed is False


def test_is_historical_day():
    """Test _is_historical_day helper function."""
    from solstone.think.indexer.journal import _is_historical_day

    # Non-day paths are never historical
    assert _is_historical_day("facets/work/events/20240101.jsonl") is False
    assert _is_historical_day("imports/123/summary.md") is False
    assert _is_historical_day("apps/home/talents/foo.md") is False

    # Future dates are not historical
    assert _is_historical_day("29991231/talents/flow.md") is False

    # Path without slash is not historical
    assert _is_historical_day("20240101") is False
    assert _is_historical_day("") is False

    # Day paths before today are historical (tested with a very old date)
    assert _is_historical_day("20000101/talents/flow.md") is True


def test_scan_journal_full_mode(journal_fixture):
    """Test full mode includes all files including historical days."""
    from solstone.think.indexer.journal import scan_journal, search_journal

    # Full scan should include everything
    changed = scan_journal(str(journal_fixture), full=True)
    assert changed is True

    # Should find content from historical day
    total, results = search_journal("project alpha")
    assert total >= 1


def test_find_formattable_files(journal_fixture):
    """Test file discovery function finds only indexed content."""
    from solstone.think.formatters import find_formattable_files

    files = find_formattable_files(str(journal_fixture))

    # Should find various file types
    paths = set(files.keys())

    # Daily agent outputs
    assert "20240101/talents/flow.md" in paths

    # Segment agent outputs
    assert "20240101/default/100000_300/talents/screen.md" in paths

    # Facet content
    assert "facets/work/events/20240101.jsonl" in paths
    assert "facets/work/entities/20240101.jsonl" in paths
    assert "facets/work/news/20240101.md" in paths


def test_find_formattable_files_includes_weekly_reflection(journal_copy):
    """Test tracked fixture reflections are included in indexed file discovery."""
    from solstone.think.formatters import find_formattable_files

    fixture_path = Path("tests/fixtures/journal/reflections/weekly/20260308.md")
    target_path = journal_copy / "reflections" / "weekly" / "20260308.md"
    target_path.parent.mkdir(parents=True, exist_ok=True)
    target_path.write_text(fixture_path.read_text(encoding="utf-8"), encoding="utf-8")

    files = find_formattable_files(str(journal_copy))

    assert "reflections/weekly/20260308.md" in files


def test_search_journal_empty_query(journal_fixture):
    """Test search with empty query returns all results."""
    from solstone.think.indexer.journal import scan_journal, search_journal

    scan_journal(str(journal_fixture))

    # Empty query should return all chunks
    total, results = search_journal("")
    assert total > 0


def test_search_journal_pagination(journal_fixture):
    """Test search pagination."""
    from solstone.think.indexer.journal import scan_journal, search_journal

    scan_journal(str(journal_fixture))

    # Get first page
    total, results1 = search_journal("", limit=2, offset=0)

    # Get second page
    _, results2 = search_journal("", limit=2, offset=2)

    # Results should be different (if enough data)
    if total > 2:
        ids1 = {r["id"] for r in results1}
        ids2 = {r["id"] for r in results2}
        assert ids1 != ids2


def test_search_journal_date_range(journal_fixture):
    """Test filtering search by date range."""
    from solstone.think.indexer.journal import scan_journal, search_journal

    scan_journal(str(journal_fixture))

    # Search with date range that includes our test day
    total, results = search_journal("", day_from="20240101", day_to="20240101")
    assert total >= 1
    for r in results:
        assert r["metadata"]["day"] == "20240101"

    # Search with date range that excludes our test day
    total, results = search_journal("", day_from="20240102", day_to="20240105")
    assert total == 0


def test_search_counts_date_range(journal_fixture):
    """Test search_counts with date range filtering."""
    from solstone.think.indexer.journal import scan_journal, search_counts

    scan_journal(str(journal_fixture))

    # Counts with date range including test data
    counts = search_counts("", day_from="20240101", day_to="20240101")
    assert counts["total"] >= 1
    assert "20240101" in counts["days"]

    # Counts with date range excluding test data
    counts = search_counts("", day_from="20240102", day_to="20240105")
    assert counts["total"] == 0


def test_search_journal_returns_counts(monkeypatch):
    """Test search tool returns counts aggregation."""
    from solstone.think.tools.search import search_journal

    # Use fixtures journal
    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")

    result = search_journal("test")

    # Should have counts structure
    assert "counts" in result
    counts = result["counts"]
    assert "facets" in counts
    assert "agents" in counts
    assert "recent_days" in counts
    assert "top_days" in counts
    assert "bucketed_days" in counts

    # recent_days should have 7 entries (including zeros)
    assert len(counts["recent_days"]) == 7


def test_search_journal_returns_query_echo(monkeypatch):
    """Test search tool returns query echo."""
    from solstone.think.tools.search import search_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")

    result = search_journal("test query", facet="work", agent="flow")

    assert "query" in result
    assert result["query"]["text"] == "test query"
    assert result["query"]["filters"]["facet"] == "work"
    assert result["query"]["filters"]["agent"] == "flow"


def test_search_journal_results_include_path(monkeypatch):
    """Test search tool results include path and idx."""
    from solstone.think.tools.search import search_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")

    result = search_journal("")

    if result.get("results"):
        item = result["results"][0]
        assert "path" in item
        assert "idx" in item


def test_search_journal_truncates_large_results(monkeypatch):
    """Test search tool truncates oversized result text."""
    from unittest.mock import patch

    from solstone.think.tools.search import _MAX_RESULT_TEXT, search_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")

    big_text = "x" * 10_000
    fake_results = [
        {
            "text": big_text,
            "metadata": {
                "day": "20240101",
                "facet": "",
                "agent": "test",
                "path": "a.md",
                "idx": 0,
            },
            "score": 1.0,
        }
    ]
    fake_counts = {"facets": [], "agents": [], "days": []}

    with (
        patch(
            "solstone.think.tools.search.search_journal_impl",
            return_value=(1, fake_results),
        ),
        patch(
            "solstone.think.tools.search.search_counts_impl", return_value=fake_counts
        ),
    ):
        result = search_journal("test")

    text = result["results"][0]["text"]
    assert len(text) < _MAX_RESULT_TEXT + 200  # truncated + note
    assert "truncated from 10,000 chars" in text


def test_bucket_day_counts():
    """Test day bucketing logic."""
    from datetime import datetime, timedelta

    from solstone.think.tools.search import _bucket_day_counts

    today = datetime.now()

    # Create test data with various dates
    day_counts = {}

    # Add recent days (within last 7 days)
    for i in range(3):
        d = (today - timedelta(days=i)).strftime("%Y%m%d")
        day_counts[d] = 5 + i

    # Add older days (more than 7 days ago)
    for i in range(10, 25):
        d = (today - timedelta(days=i)).strftime("%Y%m%d")
        day_counts[d] = 2

    result = _bucket_day_counts(day_counts)

    # recent_days should have 7 entries
    assert len(result["recent_days"]) == 7

    # top_days should have entries
    assert len(result["top_days"]) > 0

    # bucketed_days should have entries for older days
    assert len(result["bucketed_days"]) > 0

    # Bucketed day keys should be in YYYYMMDD-YYYYMMDD format
    for key in result["bucketed_days"]:
        assert "-" in key
        parts = key.split("-")
        assert len(parts) == 2
        assert len(parts[0]) == 8
        assert len(parts[1]) == 8


def test_light_scan_removes_deleted_facet_content(journal_fixture):
    """Test that light scan detects and removes deleted facet files."""
    from solstone.think.indexer.journal import scan_journal, search_journal

    # Initial scan
    scan_journal(str(journal_fixture), full=True)

    # Verify event is indexed
    total, _ = search_journal("Standup", agent="event")
    assert total >= 1

    # Delete the facet event file
    events_file = journal_fixture / "facets" / "work" / "events" / "20240101.jsonl"
    events_file.unlink()

    # Light rescan should detect the deletion (facet content is in scope)
    changed = scan_journal(str(journal_fixture), full=False)
    assert changed is True

    # Event should no longer be searchable
    total, _ = search_journal("Standup", agent="event")
    assert total == 0


def test_light_scan_removes_deleted_today_segment(tmp_path, monkeypatch):
    """Test that light scan detects and removes deleted content from today."""
    from datetime import datetime

    from solstone.think.indexer.journal import scan_journal, search_journal

    journal = tmp_path
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    # Create content for today (which is in light scan scope)
    today = datetime.now().strftime("%Y%m%d")
    day_dir = journal / today
    day_dir.mkdir(parents=True)
    agents_dir = day_dir / "talents"
    agents_dir.mkdir()
    output_file = agents_dir / "flow.md"
    output_file.write_text("# Today Flow\n\nWorked on unique_today_content.\n")

    # Initial scan
    scan_journal(str(journal), full=False)

    # Verify content is indexed
    total, _ = search_journal("unique_today_content")
    assert total >= 1

    # Delete the output file
    output_file.unlink()

    # Light rescan should detect the deletion
    changed = scan_journal(str(journal), full=False)
    assert changed is True

    # Content should no longer be searchable
    total, _ = search_journal("unique_today_content")
    assert total == 0


def test_light_scan_preserves_historical_content(tmp_path, monkeypatch):
    """Test that light scan does NOT remove historical day content from index."""
    from solstone.think.indexer.journal import scan_journal, search_journal

    journal = tmp_path
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    # Create historical day content
    day_dir = journal / "chronicle" / "20200101"
    day_dir.mkdir(parents=True)
    agents_dir = day_dir / "talents"
    agents_dir.mkdir()
    output_file = agents_dir / "flow.md"
    output_file.write_text("# Historical Flow\n\nWorked on historical_content.\n")

    # Full scan to index historical content
    scan_journal(str(journal), full=True)

    # Verify content is indexed
    total, _ = search_journal("historical_content")
    assert total >= 1

    # Delete the historical file
    output_file.unlink()

    # Light rescan should NOT remove the historical content (out of scope)
    changed = scan_journal(str(journal), full=False)
    # No changes because the historical path is out of scope
    assert changed is False

    # Content should still be searchable (not removed)
    total, _ = search_journal("historical_content")
    assert total >= 1


def test_full_scan_removes_historical_content(tmp_path, monkeypatch):
    """Test that full scan removes deleted historical day content."""
    from solstone.think.indexer.journal import scan_journal, search_journal

    journal = tmp_path
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    # Create historical day content
    day_dir = journal / "chronicle" / "20200101"
    day_dir.mkdir(parents=True)
    agents_dir = day_dir / "talents"
    agents_dir.mkdir()
    output_file = agents_dir / "flow.md"
    output_file.write_text("# Historical Flow\n\nWorked on historical_full_test.\n")

    # Full scan to index historical content
    scan_journal(str(journal), full=True)

    # Verify content is indexed
    total, _ = search_journal("historical_full_test")
    assert total >= 1

    # Delete the historical file
    output_file.unlink()

    # Full rescan SHOULD remove the historical content
    changed = scan_journal(str(journal), full=True)
    assert changed is True

    # Content should no longer be searchable
    total, _ = search_journal("historical_full_test")
    assert total == 0


def test_index_file_valid(journal_fixture):
    """Test indexing a single valid file."""
    from solstone.think.indexer.journal import index_file, search_journal

    # Index a specific file
    result = index_file(str(journal_fixture), "20240101/talents/flow.md", verbose=True)
    assert result is True

    # Should be searchable
    total, results = search_journal("project alpha")
    assert total >= 1


def test_index_file_absolute_path(journal_fixture):
    """Test indexing with absolute path."""
    from solstone.think.indexer.journal import index_file, search_journal

    abs_path = str(journal_fixture / "chronicle" / "20240101" / "talents" / "flow.md")
    result = index_file(str(journal_fixture), abs_path, verbose=True)
    assert result is True

    # Should be searchable
    total, _ = search_journal("project alpha")
    assert total >= 1


def test_scan_journal_never_stores_chronicle_prefix(journal_fixture):
    """Chronicle is an on-disk prefix only, never a stored relative path."""
    from solstone.think.indexer.journal import scan_journal

    scan_journal(str(journal_fixture), full=True)

    conn, _ = get_journal_index(str(journal_fixture))
    try:
        assert (
            conn.execute(
                "SELECT count(*) FROM files WHERE path LIKE 'chronicle/%'"
            ).fetchone()[0]
            == 0
        )
        assert (
            conn.execute(
                "SELECT count(*) FROM chunks WHERE path LIKE 'chronicle/%'"
            ).fetchone()[0]
            == 0
        )
    finally:
        conn.close()


def test_index_file_updates_existing(journal_fixture):
    """Test that re-indexing a file replaces existing chunks."""
    from solstone.think.indexer.journal import index_file, search_journal

    # Index the file
    index_file(str(journal_fixture), "20240101/talents/flow.md")

    # Get initial count
    total1, _ = search_journal("project alpha")

    # Re-index the same file
    index_file(str(journal_fixture), "20240101/talents/flow.md")

    # Count should be the same (not doubled)
    total2, _ = search_journal("project alpha")
    assert total2 == total1


def test_index_file_not_found(journal_fixture):
    """Test indexing non-existent file raises error."""
    from solstone.think.indexer.journal import index_file

    with pytest.raises(FileNotFoundError, match="File not found"):
        index_file(str(journal_fixture), "nonexistent/file.md")


def test_index_file_outside_journal(journal_fixture, tmp_path_factory):
    """Test indexing file outside journal raises error."""
    from solstone.think.indexer.journal import index_file

    # Create a file in a separate temp directory (outside the journal)
    outside_dir = tmp_path_factory.mktemp("outside")
    outside_file = outside_dir / "outside.md"
    outside_file.write_text("# Outside\n\nThis is outside the journal.\n")

    with pytest.raises(ValueError, match="outside journal directory"):
        index_file(str(journal_fixture), str(outside_file))


def test_index_file_no_formatter(journal_fixture):
    """Test indexing file without formatter raises error."""
    from solstone.think.indexer.journal import index_file

    # Create a file with no formatter (e.g., .txt)
    txt_file = journal_fixture / "chronicle" / "20240101" / "notes.txt"
    txt_file.write_text("Just some text notes.\n")

    with pytest.raises(ValueError, match="No formatter found"):
        index_file(str(journal_fixture), str(txt_file))


# --- Stream indexing tests ---


def test_extract_stream_segment_path(tmp_path):
    """_extract_stream reads stream.json from segment directories."""
    from solstone.think.indexer.journal import _extract_stream
    from solstone.think.streams import write_segment_stream

    # Create a segment with stream marker
    seg_dir = tmp_path / "chronicle" / "20240101" / "default" / "123456_300"
    seg_dir.mkdir(parents=True)
    write_segment_stream(seg_dir, "archon", None, None, 1)

    result = _extract_stream(
        str(tmp_path), "20240101/default/123456_300/talents/work/flow.md"
    )
    assert result == "archon"


def test_extract_stream_non_segment_path(tmp_path):
    """_extract_stream returns None for non-segment paths."""
    from solstone.think.indexer.journal import _extract_stream

    result = _extract_stream(str(tmp_path), "20240101/talents/flow.md")
    assert result is None

    result = _extract_stream(str(tmp_path), "facets/work/events/20240101.jsonl")
    assert result is None


def test_extract_stream_missing_marker(tmp_path):
    """_extract_stream returns None when stream.json doesn't exist."""
    from solstone.think.indexer.journal import _extract_stream

    seg_dir = tmp_path / "chronicle" / "20240101" / "default" / "123456_300"
    seg_dir.mkdir(parents=True)

    result = _extract_stream(
        str(tmp_path), "20240101/default/123456_300/talents/work/flow.md"
    )
    assert result is None


def test_search_journal_stream_filter(monkeypatch):
    """search_journal filters by stream name."""
    from solstone.think.indexer.journal import scan_journal, search_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal(os.environ["SOLSTONE_JOURNAL"], full=True)

    # Search with matching stream
    total, results = search_journal("", stream="default")
    assert total > 0
    for r in results:
        assert r["metadata"]["stream"] == "default"

    # Search with non-existent stream
    total, results = search_journal("", stream="nonexistent")
    assert total == 0


def test_search_journal_results_include_stream(monkeypatch):
    """search_journal results include stream in metadata."""
    from solstone.think.indexer.journal import scan_journal, search_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal(os.environ["SOLSTONE_JOURNAL"], full=True)

    # Filter to segment content which has stream markers
    total, results = search_journal("", stream="default")
    assert total > 0

    for r in results:
        assert "stream" in r["metadata"]
        assert r["metadata"]["stream"] == "default"


def test_search_counts_stream_filter(monkeypatch):
    """search_counts filters by stream and includes streams aggregation."""
    from solstone.think.indexer.journal import scan_journal, search_counts

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal(os.environ["SOLSTONE_JOURNAL"], full=True)

    # Unfiltered counts should include streams
    counts = search_counts("")
    assert "streams" in counts

    # Filter by stream
    counts = search_counts("", stream="default")
    assert counts["total"] > 0

    # Non-existent stream returns zero
    counts = search_counts("", stream="nonexistent")
    assert counts["total"] == 0


def test_search_tool_stream_filter(monkeypatch):
    """Agent search tool accepts and passes stream filter."""
    from solstone.think.indexer.journal import scan_journal
    from solstone.think.tools.search import search_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal(os.environ["SOLSTONE_JOURNAL"], full=True)

    result = search_journal("", stream="default")
    assert "results" in result
    assert result["total"] > 0
    assert result["query"]["filters"]["stream"] == "default"


def test_entity_schema_creation(monkeypatch):
    """Verify entities table exists after schema init."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    conn, _ = get_journal_index()

    tables = conn.execute(
        "SELECT name FROM sqlite_master WHERE type='table' AND name='entities'"
    ).fetchall()
    conn.close()
    assert len(tables) == 1


def test_scan_entities_identity(monkeypatch):
    """Verify journal entity identity rows are indexed."""
    from solstone.think.indexer.journal import scan_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal("tests/fixtures/journal", full=True)

    conn, _ = get_journal_index("tests/fixtures/journal")
    rows = conn.execute("SELECT * FROM entities WHERE source='identity'").fetchall()
    conn.close()
    assert len(rows) == 33


def test_scan_entities_relationship(monkeypatch):
    """Verify facet relationship rows are indexed."""
    from solstone.think.indexer.journal import scan_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal("tests/fixtures/journal", full=True)

    conn, _ = get_journal_index("tests/fixtures/journal")
    rows = conn.execute("SELECT * FROM entities WHERE source='relationship'").fetchall()
    conn.close()
    assert len(rows) == 40


def test_scan_entities_detected(monkeypatch):
    """Verify detected entity rows are indexed."""
    from solstone.think.indexer.journal import scan_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal("tests/fixtures/journal", full=True)

    conn, _ = get_journal_index("tests/fixtures/journal")
    rows = conn.execute("SELECT * FROM entities WHERE source='detected'").fetchall()
    conn.close()
    assert len(rows) >= 4


def test_scan_entities_observations(monkeypatch):
    """Verify observation summary rows are indexed."""
    from solstone.think.indexer.journal import scan_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal("tests/fixtures/journal", full=True)

    conn, _ = get_journal_index("tests/fixtures/journal")
    rows = conn.execute(
        "SELECT entity_id, facet, observation_count, last_observed FROM entities WHERE source='observation'"
    ).fetchall()
    conn.close()
    assert len(rows) == 23

    by_entity = {(r[0], r[1]): (r[2], r[3]) for r in rows}
    assert by_entity[("alice_johnson", "personal")][0] == 3
    assert by_entity[("john_smith", "test-facet")][0] == 2


def test_scan_entities_incremental_noop(monkeypatch):
    """Verify second scan is a no-op."""
    from solstone.think.indexer.journal import scan_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal("tests/fixtures/journal", full=True)

    conn, _ = get_journal_index("tests/fixtures/journal")
    count1 = conn.execute("SELECT count(*) FROM entities").fetchone()[0]
    conn.close()

    scan_journal("tests/fixtures/journal", full=True)
    conn, _ = get_journal_index("tests/fixtures/journal")
    count2 = conn.execute("SELECT count(*) FROM entities").fetchone()[0]
    conn.close()
    assert count1 == count2


def test_scan_entities_deletion(tmp_path, monkeypatch):
    """Verify entity rows are removed when source file is deleted."""
    src = Path("tests/fixtures/journal")
    dst = tmp_path / "journal"
    copytree_tracked(src, dst)
    j = str(dst)
    monkeypatch.setenv("SOLSTONE_JOURNAL", j)

    from solstone.think.indexer.journal import scan_journal

    scan_journal(j, full=True)

    conn, _ = get_journal_index(j)
    initial = conn.execute(
        "SELECT count(*) FROM entities WHERE source='identity'"
    ).fetchone()[0]
    conn.close()
    assert initial == 33

    entity_file = dst / "entities" / "alice_johnson" / "entity.json"
    entity_file.unlink()

    scan_journal(j, full=True)
    conn, _ = get_journal_index(j)
    after = conn.execute(
        "SELECT count(*) FROM entities WHERE source='identity'"
    ).fetchone()[0]
    conn.close()
    assert after == 32


def test_scan_entities_preserves_fts(monkeypatch):
    """Verify FTS5 chunks still work after entity scan."""
    from solstone.think.indexer.journal import scan_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal("tests/fixtures/journal", full=True)
    total, results = search_journal("Alice", limit=5)
    assert isinstance(total, int)


def test_signal_schema_creation(monkeypatch):
    """Verify entity_signals table exists after schema init."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    conn, _ = get_journal_index()

    tables = conn.execute(
        "SELECT name FROM sqlite_master WHERE type='table' AND name='entity_signals'"
    ).fetchall()
    conn.close()
    assert len(tables) == 1


def test_scan_signals_kg_appearances(monkeypatch):
    """Verify KG appearance signals are extracted."""
    from solstone.think.indexer.journal import scan_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal("tests/fixtures/journal", full=True)

    conn, _ = get_journal_index("tests/fixtures/journal")
    rows = conn.execute(
        """
        SELECT entity_name, entity_type, day FROM entity_signals
        WHERE signal_type='kg_appearance'
        """
    ).fetchall()
    conn.close()

    assert len(rows) == 45
    names = {r[0] for r in rows}
    assert "Alice Johnson" in names
    assert "Romeo Montague" in names
    # Non-bold entity names (plain text and backtick-wrapped) are also extracted
    assert "Rosaline Capulet" in names
    assert "Prince Escalus" in names
    day_20240101 = [r for r in rows if r[2] == "20240101"]
    assert len(day_20240101) == 4
    by_name_d1 = {r[0]: r[1] for r in day_20240101}
    assert by_name_d1["Alice Johnson"] == "Person"
    assert by_name_d1["Acme Corp"] == "Organization"


def test_scan_signals_kg_edges(monkeypatch):
    """Verify KG edge signals are extracted."""
    from solstone.think.indexer.journal import scan_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal("tests/fixtures/journal", full=True)

    conn, _ = get_journal_index("tests/fixtures/journal")
    rows = conn.execute(
        """
        SELECT entity_name, target_name, relationship_type, day FROM entity_signals
        WHERE signal_type='kg_edge'
        """
    ).fetchall()
    conn.close()

    assert len(rows) == 25
    edges = {(r[0], r[1]): r[2] for r in rows}
    assert edges[("Alice Johnson", "Bob Smith")] == "collaborates-with"
    assert edges[("Alice Johnson", "Acme Corp")] == "client-liaison"
    assert edges[("Bob Smith", "Project Alpha")] == "contributor"


def test_scan_signals_event_participants(monkeypatch):
    """Verify event participant signals are extracted."""
    from solstone.think.indexer.journal import scan_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal("tests/fixtures/journal", full=True)

    conn, _ = get_journal_index("tests/fixtures/journal")
    rows = conn.execute(
        """
        SELECT entity_name, event_title, event_type, day, facet
        FROM entity_signals
        WHERE signal_type='event_participant'
        """
    ).fetchall()
    conn.close()

    assert len(rows) == 54
    names = [r[0] for r in rows]
    assert names.count("Alice") == 2
    assert names.count("Bob") == 2
    assert names.count("Charlie") == 1


def test_scan_signals_incremental_noop(monkeypatch):
    """Verify second scan is a no-op."""
    from solstone.think.indexer.journal import scan_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal("tests/fixtures/journal", full=True)

    conn, _ = get_journal_index("tests/fixtures/journal")
    count1 = conn.execute("SELECT count(*) FROM entity_signals").fetchone()[0]
    conn.close()

    scan_journal("tests/fixtures/journal", full=True)
    conn, _ = get_journal_index("tests/fixtures/journal")
    count2 = conn.execute("SELECT count(*) FROM entity_signals").fetchone()[0]
    conn.close()
    assert count1 == count2


def test_scan_signals_deletion(tmp_path):
    """Verify signal rows are removed when source file is deleted."""
    src = Path("tests/fixtures/journal")
    dst = tmp_path / "journal"
    copytree_tracked(src, dst)
    j = str(dst)

    from solstone.think.indexer.journal import scan_journal

    scan_journal(j, full=True)

    conn, _ = get_journal_index(j)
    initial = conn.execute(
        "SELECT count(*) FROM entity_signals WHERE signal_type='kg_appearance'"
    ).fetchone()[0]
    conn.close()
    assert initial == 45

    kg_file = dst / "chronicle" / "20240101" / "talents" / "knowledge_graph.md"
    kg_file.unlink()

    scan_journal(j, full=True)
    conn, _ = get_journal_index(j)
    after = conn.execute(
        "SELECT count(*) FROM entity_signals WHERE signal_type='kg_appearance'"
    ).fetchone()[0]
    conn.close()
    assert after == 41


def test_scan_signals_kg_facet_assignment(monkeypatch):
    """Verify KG signals get facet assigned from detection data."""
    from solstone.think.indexer.journal import scan_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal("tests/fixtures/journal", full=True)

    conn, _ = get_journal_index("tests/fixtures/journal")

    # Appearances: entities in detection data get facet assigned
    rows = conn.execute(
        """
        SELECT entity_name, facet FROM entity_signals
        WHERE signal_type='kg_appearance' AND day='20260310'
        ORDER BY entity_name, facet
        """
    ).fetchall()
    by_name: dict[str, list[str | None]] = {}
    for name, facet in rows:
        by_name.setdefault(name, []).append(facet)

    # Romeo is in montague + verona detection data → two rows
    assert by_name["Romeo Montague"] == ["montague", "verona"]
    # Juliet is in capulet + verona → two rows
    assert by_name["Juliet Capulet"] == ["capulet", "verona"]
    # Mercutio is only in montague → one row
    assert by_name["Mercutio Escalus"] == ["montague"]
    # Verona Platform is only in verona → one row
    assert by_name["Verona Platform"] == ["verona"]

    # Edges: facet assigned only when BOTH entities share a facet
    edges = conn.execute(
        """
        SELECT entity_name, target_name, facet FROM entity_signals
        WHERE signal_type='kg_edge' AND day='20260310'
        ORDER BY entity_name, target_name
        """
    ).fetchall()
    edge_facets = {(r[0], r[1]): r[2] for r in edges}

    # Romeo + Verona Platform share verona → facet=verona
    assert edge_facets[("Romeo Montague", "Verona Platform")] == "verona"
    # Juliet + Verona Platform share verona → facet=verona
    assert edge_facets[("Juliet Capulet", "Verona Platform")] == "verona"
    # Tybalt (capulet) + Romeo (montague, verona) → no shared facet → NULL
    assert edge_facets[("Tybalt Capulet", "Romeo Montague")] is None
    # Mercutio (montague) + Verona Platform (verona) → no shared facet → NULL
    assert edge_facets[("Mercutio Escalus", "Verona Platform")] is None

    # Entities with no detection data on their day stay NULL
    null_rows = conn.execute(
        """
        SELECT entity_name FROM entity_signals
        WHERE signal_type='kg_appearance' AND day='20240101' AND facet IS NULL
        """
    ).fetchall()
    assert len(null_rows) == 4  # Alice Johnson, Bob Smith, Acme Corp, Project Alpha

    conn.close()


def test_entity_search_chunks_indexed(monkeypatch):
    """Entity search chunks are generated from identity + relationship data."""
    from solstone.think.indexer.journal import scan_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal("tests/fixtures/journal", full=True)
    conn, _ = get_journal_index("tests/fixtures/journal")
    count = conn.execute("SELECT count(*) FROM chunks WHERE agent='entity'").fetchone()[
        0
    ]
    conn.close()
    # One chunk per entity-facet relationship in the current fixture journal.
    assert count == 40


def test_entity_search_chunks_use_entity_search_path(monkeypatch):
    """Entity search chunks use entity_search: path prefix."""
    from solstone.think.indexer.journal import scan_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal("tests/fixtures/journal", full=True)
    conn, _ = get_journal_index("tests/fixtures/journal")
    rows = conn.execute(
        "SELECT DISTINCT path FROM chunks WHERE agent='entity'"
    ).fetchall()
    conn.close()
    assert all(r[0].startswith("entity_search:") for r in rows)


def test_entity_search_by_name(monkeypatch):
    """Entity name is searchable via FTS."""
    from solstone.think.indexer.journal import scan_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal("tests/fixtures/journal", full=True)
    total, results = search_journal("Alice Johnson", agent="entity")
    assert total >= 1
    assert any(r["metadata"]["agent"] == "entity" for r in results)


def test_entity_search_by_type(monkeypatch):
    """Entity type is searchable via FTS."""
    from solstone.think.indexer.journal import scan_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal("tests/fixtures/journal", full=True)
    total, results = search_journal("Person", agent="entity")
    assert total >= 1


def test_entity_search_includes_description(monkeypatch):
    """Entity search chunks include relationship descriptions."""
    from solstone.think.indexer.journal import scan_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal("tests/fixtures/journal", full=True)
    # Alice has description "Close friend from college" in personal facet
    total, results = search_journal("college", agent="entity")
    assert total >= 1
    matched = [r for r in results if "college" in r["text"].lower()]
    assert len(matched) >= 1


def test_entity_search_includes_facet(monkeypatch):
    """Entity search chunks have facet metadata from relationships."""
    from solstone.think.indexer.journal import scan_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal("tests/fixtures/journal", full=True)
    total, results = search_journal("Alice Johnson", agent="entity", facet="personal")
    assert total >= 1
    assert all(r["metadata"]["facet"] == "personal" for r in results)


def test_entity_search_idempotent(monkeypatch):
    """Two full scans produce identical entity chunk count (no duplicates)."""
    from solstone.think.indexer.journal import scan_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    scan_journal("tests/fixtures/journal", full=True)
    conn, _ = get_journal_index("tests/fixtures/journal")
    count1 = conn.execute(
        "SELECT count(*) FROM chunks WHERE agent='entity'"
    ).fetchone()[0]
    conn.close()
    scan_journal("tests/fixtures/journal", full=True)
    conn, _ = get_journal_index("tests/fixtures/journal")
    count2 = conn.execute(
        "SELECT count(*) FROM chunks WHERE agent='entity'"
    ).fetchone()[0]
    conn.close()
    assert count1 == count2 == 40


class TestSegmentChunks:
    """Tests for segment-level concatenated FTS5 chunks."""

    def test_segment_chunks_created(self, journal_fixture):
        """scan_journal creates segment chunks with agent='segment'."""
        from solstone.think.indexer.journal import get_journal_index, scan_journal

        scan_journal(str(journal_fixture), verbose=True, full=True)
        conn, _ = get_journal_index(str(journal_fixture))
        rows = conn.execute(
            "SELECT content, path, day, facet, agent, stream FROM chunks WHERE agent='segment'"
        ).fetchall()
        conn.close()
        assert len(rows) >= 1
        content, path, day, facet, agent, stream = rows[0]
        assert content
        assert path == "20240101/default/100000_300"
        assert day == "20240101"
        assert facet == ""
        assert agent == "segment"
        assert stream == "default"

    def test_segment_chunk_contains_all_agent_content(self, journal_fixture):
        """Segment chunk content includes text from all agent files."""
        from solstone.think.indexer.journal import get_journal_index, scan_journal

        scan_journal(str(journal_fixture), verbose=True, full=True)
        conn, _ = get_journal_index(str(journal_fixture))
        rows = conn.execute(
            "SELECT content FROM chunks WHERE agent='segment'"
        ).fetchall()
        conn.close()
        all_content = " ".join(r[0] for r in rows)
        assert "Viewed documentation" in all_content
        assert "Scott Ward" in all_content

    def test_segment_chunk_searchable(self, journal_fixture):
        """Segment chunks are searchable via search_journal."""
        from solstone.think.indexer.journal import scan_journal, search_journal

        scan_journal(str(journal_fixture), verbose=True, full=True)
        total, results = search_journal("Scott Ward")
        assert total >= 1
        segment_results = [r for r in results if r["metadata"]["agent"] == "segment"]
        assert len(segment_results) >= 1

    def test_segment_chunk_cross_file_search(self, journal_fixture):
        """Search spanning multiple agent files matches segment chunk."""
        from solstone.think.indexer.journal import scan_journal, search_journal

        scan_journal(str(journal_fixture), verbose=True, full=True)
        total1, results1 = search_journal("documentation")
        total2, results2 = search_journal("Acme deal")
        assert total1 >= 1
        assert total2 >= 1
        seg1 = [r for r in results1 if r["metadata"]["agent"] == "segment"]
        seg2 = [r for r in results2 if r["metadata"]["agent"] == "segment"]
        assert len(seg1) >= 1
        assert len(seg2) >= 1

    def test_agent_filter_returns_only_segments(self, journal_fixture):
        """agent='segment' filter returns only segment chunks."""
        from solstone.think.indexer.journal import scan_journal, search_journal

        scan_journal(str(journal_fixture), verbose=True, full=True)
        total, results = search_journal("", agent="segment")
        assert total >= 1
        for r in results:
            assert r["metadata"]["agent"] == "segment"

    def test_existing_agent_chunks_unchanged(self, journal_fixture):
        """Segment chunks are additive — agent-level chunks still exist."""
        from solstone.think.indexer.journal import get_journal_index, scan_journal

        scan_journal(str(journal_fixture), verbose=True, full=True)
        conn, _ = get_journal_index(str(journal_fixture))
        screen_chunks = conn.execute(
            "SELECT count(*) FROM chunks WHERE path='20240101/default/100000_300/talents/screen.md'"
        ).fetchone()[0]
        activity_chunks = conn.execute(
            "SELECT count(*) FROM chunks WHERE path='20240101/default/100000_300/talents/activity.md'"
        ).fetchone()[0]
        segment_chunks = conn.execute(
            "SELECT count(*) FROM chunks WHERE agent='segment'"
        ).fetchone()[0]
        conn.close()
        assert screen_chunks >= 1
        assert activity_chunks >= 1
        assert segment_chunks >= 1

    def test_idempotent_scan(self, journal_fixture):
        """Running scan_journal twice produces same segment chunk count."""
        from solstone.think.indexer.journal import get_journal_index, scan_journal

        scan_journal(str(journal_fixture), verbose=True, full=True)
        conn, _ = get_journal_index(str(journal_fixture))
        count1 = conn.execute(
            "SELECT count(*) FROM chunks WHERE agent='segment'"
        ).fetchone()[0]
        conn.close()
        scan_journal(str(journal_fixture), verbose=True, full=True)
        conn, _ = get_journal_index(str(journal_fixture))
        count2 = conn.execute(
            "SELECT count(*) FROM chunks WHERE agent='segment'"
        ).fetchone()[0]
        conn.close()
        assert count1 == count2


def test_chat_turn_is_searchable_after_rescan(journal_fixture):
    from solstone.think.indexer.journal import scan_journal, search_journal

    append_chat_event(
        "owner_message",
        text="Tell me about the nebula phrase",
        app="sol",
        path="/app/sol",
        facet="work",
    )
    append_chat_event(
        "sol_message",
        use_id="1713628000000",
        text="The unique nebula phrase is now in chat history.",
        notes="done",
        requested_target=None,
        requested_task=None,
    )

    scan_journal(str(journal_fixture), full=True)
    total, results = search_journal("unique nebula phrase")

    assert total >= 1
    assert any("unique nebula phrase" in result["text"].lower() for result in results)


def test_weekly_reflection_is_searchable_after_rescan(journal_copy):
    from solstone.think.indexer.journal import scan_journal, search_journal

    fixture_path = Path("tests/fixtures/journal/reflections/weekly/20260308.md")
    target_path = journal_copy / "reflections" / "weekly" / "20260308.md"
    target_path.parent.mkdir(parents=True, exist_ok=True)
    target_path.write_text(fixture_path.read_text(encoding="utf-8"), encoding="utf-8")

    scan_journal(str(journal_copy), full=True)
    total, results = search_journal("boardroom balcony inflection")

    assert total >= 1
    assert any(
        "boardroom balcony inflection" in result["text"].lower() for result in results
    )


def test_scan_journal_is_pure_wrt_entity_state(journal_copy):
    """scan_journal must not mutate journal/entities/ state."""
    from solstone.think.indexer.journal import scan_journal

    journal_path = Path(journal_copy)
    today = datetime.now().strftime("%Y%m%d")
    segment_dir = (
        journal_path / "chronicle" / today / "default" / "120000_300" / "talents"
    )
    segment_dir.mkdir(parents=True)
    (segment_dir / "entities.jsonl").write_text(
        '{"name":"Zephyr Quartz Index","type":"Project","description":"Unique regression seed"}\n',
        encoding="utf-8",
    )

    def snapshot_entities(root: Path) -> list[tuple[str, str]]:
        entries = []
        for path in sorted((root / "entities").rglob("*")):
            if not path.is_file():
                continue
            rel = path.relative_to(root).as_posix()
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            entries.append((rel, digest))
        return entries

    snap_before = snapshot_entities(journal_path)
    scan_journal(str(journal_path), full=True)
    snap_between = snapshot_entities(journal_path)
    scan_journal(str(journal_path), full=True)
    snap_after = snapshot_entities(journal_path)

    assert snap_before == snap_between == snap_after, (
        "scan_journal() mutated journal/entities/ — see docs/coding-standards.md § L6"
    )
