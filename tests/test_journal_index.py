# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the unified journal index."""

import hashlib
import json
import subprocess
from datetime import datetime, timedelta
from pathlib import Path
from unittest.mock import Mock

import pytest

from solstone.convey.chat_stream import append_chat_event
from solstone.think.indexer.journal import (
    get_journal_index,
    search_counts,
    search_journal,
)


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


@pytest.fixture
def relax_fixture(tmp_path, monkeypatch):
    """Create a focused journal for search relaxation and rerank tests."""
    journal = tmp_path
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    import solstone.think.utils as think_utils

    think_utils._journal_path_cache = None

    from solstone.think.indexer.journal import scan_journal
    from solstone.think.streams import write_segment_stream

    day = journal / "chronicle" / "20240101"
    day.mkdir(parents=True)

    segment = day / "default" / "100000_300"
    talents = segment / "talents"
    talents.mkdir(parents=True)
    (talents / "activity.md").write_text(
        "# Activity\n\nMet with Scott Ward about Acme deal.\n",
        encoding="utf-8",
    )
    (talents / "screen.md").write_text(
        "# Screen\n\nViewed documentation.\n",
        encoding="utf-8",
    )
    write_segment_stream(str(segment), "default", None, None, 1)

    segment2 = day / "default" / "110000_300"
    talents2 = segment2 / "talents"
    talents2.mkdir(parents=True)
    (talents2 / "a_first.md").write_text(
        "# Zephyr\n\nMongoose census complete.\n",
        encoding="utf-8",
    )
    (talents2 / "b_second.md").write_text(
        "Quokka sightings logged.\n",
        encoding="utf-8",
    )
    write_segment_stream(str(segment2), "default", None, None, 1)

    day_talents = day / "talents"
    day_talents.mkdir()
    (day_talents / "apostrophe.md").write_text(
        "# Apostrophe\n\nOwner's decision log mentions budget.\n",
        encoding="utf-8",
    )
    (day_talents / "android.md").write_text(
        "# Android\n\nANDROID migration checklist.\n",
        encoding="utf-8",
    )
    (day_talents / "oracle.md").write_text(
        "# Oracle\n\nORACLE support matrix.\n",
        encoding="utf-8",
    )
    (day_talents / "rerank_a.md").write_text(
        "# Rerank A\n\nsharedrank alpha lowmark.\n",
        encoding="utf-8",
    )
    (day_talents / "rerank_b.md").write_text(
        "# Rerank B\n\nsharedrank beta midmark.\n",
        encoding="utf-8",
    )
    (day_talents / "rerank_c.md").write_text(
        "# Rerank C\n\nsharedrank gamma highmark.\n",
        encoding="utf-8",
    )
    (day_talents / "single.md").write_text(
        "# Single\n\nsinglematch only here.\n",
        encoding="utf-8",
    )

    events_dir = journal / "facets" / "work" / "events"
    events_dir.mkdir(parents=True)
    event = {
        "type": "meeting",
        "start": "09:00:00",
        "end": "09:30:00",
        "title": "Standup",
        "summary": "Daily sync meeting",
        "facet": "work",
        "occurred": True,
    }
    (events_dir / "20240101.jsonl").write_text(json.dumps(event), encoding="utf-8")

    yesterday = (datetime.now() - timedelta(days=1)).strftime("%Y%m%d")
    yesterday_talents = journal / "chronicle" / yesterday / "talents"
    yesterday_talents.mkdir(parents=True)
    (yesterday_talents / "temporal.md").write_text(
        "# Temporal\n\nCalypso ladder recall happened.\n",
        encoding="utf-8",
    )

    scan_journal(str(journal), full=True)
    return {"journal": journal, "yesterday": yesterday}


def test_scan_journal(journal_fixture):
    """Test scanning journal creates index."""
    from solstone.think.indexer.journal import scan_journal

    changed = scan_journal(str(journal_fixture), verbose=True).changed
    assert changed is True

    # Index file should exist
    index_path = journal_fixture / "indexer" / "journal.sqlite"
    assert index_path.exists()


def test_known_agents_returns_indexed_agents(journal_fixture):
    """known_agents() returns distinct non-empty lowercase agent names."""
    import solstone.think.utils as think_utils
    from solstone.think.indexer.journal import known_agents, scan_journal

    think_utils._journal_path_cache = None
    scan_journal(str(journal_fixture), full=True)
    agents = known_agents()
    assert agents  # non-empty
    assert "flow" in agents
    assert all(a and a == a.lower() for a in agents)


def test_known_agents_absent_index_raises(tmp_path, monkeypatch):
    """A fresh journal surfaces an absent native index."""
    from solstone.think.indexer.journal import NativeIndexerReadError, known_agents

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    import solstone.think.utils as think_utils

    think_utils._journal_path_cache = None
    try:
        with pytest.raises(NativeIndexerReadError, match="index"):
            known_agents()
    finally:
        think_utils._journal_path_cache = None


def test_search_journal_outputs(journal_fixture):
    """Test searching returns agent output chunks."""
    from solstone.think.indexer.journal import scan_journal, search_journal

    scan_journal(str(journal_fixture))

    total, results = search_journal("project alpha")
    assert total >= 1
    # Should find the flow output mentioning "project alpha"
    found = any("alpha" in r["text"].lower() for r in results)
    assert found


def test_search_journal_apostrophe_terms(journal_fixture):
    """Apostrophe-bearing search terms are valid against a real scanned index."""
    from solstone.think.indexer.journal import scan_journal

    talents_dir = journal_fixture / "chronicle" / "20240101" / "talents"
    (talents_dir / "apostrophes.md").write_text(
        "# Apostrophe Search\n\n"
        "it's indexed exactly here. "
        "Bob O'Brien said don't panic. "
        "O'Brien brought dogs to the review.\n"
    )

    scan_journal(str(journal_fixture), full=True)

    for query in ("it's", "O'Brien", "don't panic", "O'Brien AND dogs"):
        total, results = search_journal(query)
        assert total >= 1
        assert results


def test_search_journal_yesterdays_meeting_apostrophe(journal_fixture):
    """Wall-clock temporal extraction still matches apostrophe residue."""
    from solstone.think.indexer.journal import scan_journal

    yesterday = (datetime.now() - timedelta(days=1)).strftime("%Y%m%d")
    talents_dir = journal_fixture / "chronicle" / yesterday / "talents"
    talents_dir.mkdir(parents=True, exist_ok=True)
    (talents_dir / "flow.md").write_text(
        "# Yesterday\n\nReviewed yesterday's meeting notes.\n"
    )

    scan_journal(str(journal_fixture), full=True)

    total, results = search_journal("yesterday's meeting")
    assert total >= 1
    assert any("yesterday's meeting" in result["text"].lower() for result in results)


def test_segment_sense_json_searchable_and_exact(journal_copy):
    """Segment sense JSON is indexed with its dedicated formatter."""
    from solstone.think.formatters import find_formattable_files
    from solstone.think.indexer.journal import scan_journal, search_journal

    day = "20990115"
    segment_dir = journal_copy / "chronicle" / day / "default" / "120000_300"
    talents_dir = segment_dir / "talents"
    talents_dir.mkdir(parents=True, exist_ok=True)
    (talents_dir / "sense.json").write_text(
        json.dumps(
            {
                "density": "active",
                "content_type": "coding",
                "activity_summary": (
                    "Discussed the Monazite Comet Handoff unique regression seed."
                ),
                "entities": [
                    {
                        "type": "Project",
                        "name": "Zephyr Quartz Index",
                        "role": "mentioned",
                        "source": "screen",
                        "context": "Anchored the Monazite Comet Handoff.",
                    }
                ],
                "facets": [
                    {"facet": "work", "activity": "search indexing", "level": "high"}
                ],
                "speculative_facet": None,
                "meeting_detected": True,
                "speakers": ["Alice Smith", "Bob Chen"],
                "recommend": {"screen_record": True, "speaker_attribution": True},
                "emotional_register": "focused",
            }
        ),
        encoding="utf-8",
    )
    rel_path = f"{day}/default/120000_300/talents/sense.json"

    formattable = find_formattable_files(str(journal_copy))
    assert rel_path in formattable

    scan_journal(str(journal_copy), full=True)

    total, name_results = search_journal("Zephyr Quartz Index")
    assert total >= 1
    assert any(result["metadata"]["path"] == rel_path for result in name_results)

    total, description_results = search_journal("Monazite Comet Handoff")
    assert total >= 1
    matching = [
        result
        for result in description_results
        if result["metadata"]["path"] == rel_path
    ]
    assert matching
    assert any("Monazite Comet Handoff" in result["text"] for result in matching)
    assert all(result["metadata"]["agent"] == "sense" for result in matching)


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

    # Segment child chunks should have correct time buckets
    morning_rows = conn.execute(
        "SELECT time_bucket FROM chunks WHERE path LIKE '%100000_300/talents/%'"
    ).fetchall()
    assert morning_rows
    assert all(r[0] == "morning" for r in morning_rows)

    evening_rows = conn.execute(
        "SELECT time_bucket FROM chunks WHERE path LIKE '%200000_300/talents/%'"
    ).fetchall()
    assert evening_rows
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

    # Morning filter should find 100000 child content
    total, results = search_journal("", time_bucket="morning")
    assert total >= 1
    for r in results:
        assert "100000_300" in r["metadata"]["path"]

    # Evening filter should find 200000 child content
    total, results = search_journal("", time_bucket="evening")
    assert total >= 1
    for r in results:
        assert "200000_300" in r["metadata"]["path"]

    # Non-matching bucket should return no content
    total, results = search_journal("", time_bucket="afternoon")
    assert total == 0
    assert results == []


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


def test_index_caching(journal_fixture):
    """Test that unchanged files are not re-indexed."""
    from solstone.think.indexer.journal import scan_journal

    # First scan indexes files
    changed = scan_journal(str(journal_fixture)).changed
    assert changed is True

    # Second scan should be a no-op (all cached)
    changed = scan_journal(str(journal_fixture)).changed
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
    changed = scan_journal(str(journal_fixture), full=True).changed
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


def _joined_text(results):
    return "\n".join(result["text"] for result in results)


def _ids(results):
    return [result["id"] for result in results]


def test_search_total_parity(relax_fixture):
    q = "Acme"

    total, _ = search_journal(q)
    counts = search_counts(q)

    assert total == counts["total"] == 1

    conn, _ = get_journal_index()
    raw = conn.execute(
        "SELECT count(*) FROM chunks WHERE chunks MATCH ?", ["Acme"]
    ).fetchone()[0]
    conn.close()

    assert raw == total


def test_search_pagination_no_duplicates_or_missing_rows(relax_fixture):
    q = "Acme OR documentation"

    assert search_journal(q)[0] == 2

    page0_total, page0 = search_journal(q, limit=1, offset=0)
    page1_total, page1 = search_journal(q, limit=1, offset=1)
    page2_total, page2 = search_journal(q, limit=1, offset=2)

    assert page0_total == page1_total == page2_total == 2
    assert page2 == []

    pages = page0 + page1 + page2
    ids = _ids(pages)
    assert set(ids) == {
        "20240101/default/100000_300/talents/activity.md:0",
        "20240101/default/100000_300/talents/screen.md:0",
    }
    assert len(ids) == len(set(ids))


def test_search_counts_relaxed_flag(relax_fixture):
    relaxed = search_counts("what was the Acme deal", relax=True)

    assert relaxed["relaxed"] is True
    assert search_counts("what was the Acme deal", relax=False)["relaxed"] is False
    assert search_counts("Acme", relax=True)["relaxed"] is False
    assert relaxed["total"] == search_journal("what was the Acme deal", relax=True)[0]


def test_relax_all_stopwords_without_filter_never_browses(relax_fixture):
    assert search_journal("what did i do", relax=True) == (0, [])


def test_relax_keeps_tier_zero_matches_identical(relax_fixture):
    plain = search_journal("singlematch", relax=False)
    relaxed = search_journal("singlematch", relax=True)

    assert relaxed == plain


def test_relax_stopword_ladder_rescues_natural_question(relax_fixture):
    unrelaxed_total, unrelaxed_results = search_journal(
        "what was the Acme deal", relax=False
    )
    relaxed_total, relaxed_results = search_journal(
        "what was the Acme deal", relax=True
    )
    browse_total, _ = search_journal("", relax=False)

    assert (unrelaxed_total, unrelaxed_results) == (0, [])
    assert 0 < relaxed_total < browse_total
    assert "Acme deal" in _joined_text(relaxed_results)


def test_relax_or_tier_rescues_terms_in_different_chunks(relax_fixture):
    unrelaxed_total, unrelaxed_results = search_journal(
        "documentation Standup", relax=False
    )
    relaxed_total, relaxed_results = search_journal("documentation Standup", relax=True)
    text = _joined_text(relaxed_results)

    assert (unrelaxed_total, unrelaxed_results) == (0, [])
    assert relaxed_total >= 2
    assert "documentation" in text
    assert "Standup" in text


def test_relax_or_tier_quotes_apostrophe_terms(relax_fixture):
    total, results = search_journal("owner's Standup", relax=True)
    text = _joined_text(results)

    assert total >= 2
    assert "Owner's decision" in text
    assert "Standup" in text


def test_relax_temporal_content_threads_day_filter(relax_fixture):
    yesterday = relax_fixture["yesterday"]
    total, results = search_journal("what was Calypso yesterday", relax=True)

    assert total >= 1
    assert any("Calypso ladder recall" in result["text"] for result in results)
    assert all(result["metadata"]["day"] == yesterday for result in results)


def test_relax_power_user_queries_stay_empty(relax_fixture):
    assert search_journal("Acme AND Standup", relax=True) == (0, [])
    assert search_journal('"Acme deal" Standup', relax=True) == (0, [])


def test_relax_operator_detection_is_token_level(relax_fixture):
    total, results = search_journal("ANDROID ORACLE", relax=True)
    text = _joined_text(results)

    assert total >= 2
    assert "ANDROID migration" in text
    assert "ORACLE support" in text


def test_relax_preserves_facet_and_date_range_filters(relax_fixture):
    facet_total, facet_results = search_journal(
        "what was Standup", facet="work", relax=True
    )
    range_total, range_results = search_journal(
        "what was the Acme deal",
        day_from="20240101",
        day_to="20240101",
        relax=True,
    )

    assert facet_total >= 1
    assert all(result["metadata"]["facet"] == "work" for result in facet_results)
    assert range_total >= 1
    assert all(result["metadata"]["day"] == "20240101" for result in range_results)


def test_relax_counts_and_results_choose_same_tier_for_all_tiers(relax_fixture):
    for query in (
        "what was the Acme deal",
        "documentation Standup",
        "what did i do yesterday",
    ):
        total, _ = search_journal(query, relax=True)
        counts = search_counts(query, relax=True)

        assert total > 0
        assert counts["total"] == total


def test_rerank_reorders_top_fts_pool(monkeypatch, relax_fixture):
    total, bm25_results = search_journal("sharedrank", limit=3, rerank=False)
    assert total == 3
    score_by_text = {
        result["text"]: float(index)
        for index, result in enumerate(bm25_results, start=1)
    }

    def stub(query, texts):
        assert query == "sharedrank"
        return [score_by_text[text] for text in texts]

    monkeypatch.setattr("solstone.think.indexer.journal.score", stub)

    _, reranked = search_journal("sharedrank", limit=3, rerank=True)

    assert _ids(reranked) == list(reversed(_ids(bm25_results)))
    assert all("rerank_score" in result for result in reranked)


def test_rerank_score_none_falls_back_to_bm25(monkeypatch, relax_fixture):
    _, bm25_results = search_journal("sharedrank", limit=3, rerank=False)
    monkeypatch.setattr("solstone.think.indexer.journal.score", lambda *_args: None)

    _, results = search_journal("sharedrank", limit=3, rerank=True)

    assert results == bm25_results
    assert all("rerank_score" not in result for result in results)


def test_rerank_skips_without_calling_score(monkeypatch, relax_fixture):
    def fail_score(*_args):
        raise AssertionError("score should not be called")

    monkeypatch.setattr("solstone.think.indexer.journal.score", fail_score)

    search_journal("sharedrank", limit=2, offset=49, rerank=True)
    search_journal("", limit=2, rerank=True)
    search_journal("singlematch", rerank=True)
    search_journal("yesterday", rerank=True)


def test_rerank_equal_scores_keep_bm25_order(monkeypatch, relax_fixture):
    _, bm25_results = search_journal("sharedrank", limit=3, rerank=False)

    def stub(_query, texts):
        return [1.0 for _text in texts]

    monkeypatch.setattr("solstone.think.indexer.journal.score", stub)

    _, results = search_journal("sharedrank", limit=3, rerank=True)

    assert _ids(results) == _ids(bm25_results)
    assert all(result["rerank_score"] == 1.0 for result in results)


def test_search_tool_keeps_rerank_without_relax(monkeypatch):
    from collections import Counter

    from solstone.think.tools import search as search_tool

    calls = {}

    def fake_search(query, limit, offset, **kwargs):
        calls["search"] = (query, limit, offset, kwargs)
        return 0, []

    def fake_counts(query, **kwargs):
        calls["counts"] = (query, kwargs)
        return {
            "total": 0,
            "facets": Counter(),
            "agents": Counter(),
            "days": Counter(),
            "streams": Counter(),
        }

    monkeypatch.setattr(search_tool, "search_journal_impl", fake_search)
    monkeypatch.setattr(search_tool, "search_counts_impl", fake_counts)

    search_tool.search_journal("needle", facet="work")

    assert "relax" not in calls["search"][3]
    assert calls["search"][3]["rerank"] is True
    assert calls["search"][3]["facet"] == "work"
    assert "relax" not in calls["counts"][1]
    assert "rerank" not in calls["counts"][1]


def test_call_tool_keeps_rerank_without_relax(monkeypatch):
    from collections import Counter

    from solstone.think.indexer import journal as journal_index
    from solstone.think.tools import call as call_tool

    calls = {}

    def fake_search(query, limit, offset, **kwargs):
        calls["search"] = (query, limit, offset, kwargs)
        return 0, []

    def fake_counts(query, **kwargs):
        calls["counts"] = (query, kwargs)
        return {
            "total": 0,
            "facets": Counter(),
            "agents": Counter(),
            "days": Counter(),
            "streams": Counter(),
        }

    monkeypatch.setattr(journal_index, "search_journal", fake_search)
    monkeypatch.setattr(journal_index, "search_counts", fake_counts)

    call_tool.search(
        query="needle",
        query_opt=None,
        limit=5,
        offset=2,
        day=None,
        day_from=None,
        day_to=None,
        facet=None,
        agent=None,
        stream=None,
        time_bucket=None,
        json_output=False,
    )

    assert "relax" not in calls["search"][3]
    assert calls["search"][3]["rerank"] is True
    assert "relax" not in calls["counts"][1]
    assert "rerank" not in calls["counts"][1]


def test_voice_tool_flips_relax_and_rerank(monkeypatch):
    from solstone.think.voice import tools as voice_tools

    calls = {}

    def fake_search(query, **kwargs):
        calls["search"] = (query, kwargs)
        return 0, []

    monkeypatch.setattr(voice_tools, "search_journal", fake_search)

    result = voice_tools.handle_journal_search(
        {"query": "needle", "facet": "work", "days": 7, "limit": 3},
        app=None,
    )

    assert result["count"] == 0
    assert calls["search"][1]["relax"] is True
    assert calls["search"][1]["rerank"] is True
    assert calls["search"][1]["facet"] == "work"


def test_search_journal_returns_counts(monkeypatch):
    """Test search tool returns counts aggregation."""
    from solstone.think.tools import search as search_tools

    monkeypatch.setattr(search_tools, "search_journal_impl", Mock(return_value=(0, [])))
    monkeypatch.setattr(
        search_tools,
        "search_counts_impl",
        Mock(
            return_value={
                "facets": [("work", 2)],
                "agents": [("flow", 2)],
                "days": [("20240101", 2)],
            }
        ),
    )

    result = search_tools.search_journal("test")

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
    from solstone.think.tools import search as search_tools

    monkeypatch.setattr(search_tools, "search_journal_impl", Mock(return_value=(0, [])))
    monkeypatch.setattr(
        search_tools,
        "search_counts_impl",
        Mock(return_value={"facets": [], "agents": [], "days": []}),
    )

    result = search_tools.search_journal("test query", facet="work", agent="flow")

    assert "query" in result
    assert result["query"]["text"] == "test query"
    assert result["query"]["filters"]["facet"] == "work"
    assert result["query"]["filters"]["agent"] == "flow"


def test_search_journal_results_include_path(monkeypatch):
    """Test search tool results include path and idx."""
    from solstone.think.tools import search as search_tools

    monkeypatch.setattr(
        search_tools,
        "search_journal_impl",
        Mock(
            return_value=(
                1,
                [
                    {
                        "text": "result",
                        "metadata": {"path": "facets/work/result.md", "idx": 3},
                    }
                ],
            )
        ),
    )
    monkeypatch.setattr(
        search_tools,
        "search_counts_impl",
        Mock(return_value={"facets": [], "agents": [], "days": []}),
    )

    result = search_tools.search_journal("")

    item = result["results"][0]
    assert item["path"] == "facets/work/result.md"
    assert item["idx"] == 3
    assert item["id"] == "facets/work/result.md:3"


def test_search_journal_truncates_large_results(monkeypatch):
    """Test search tool truncates oversized result text."""
    from unittest.mock import patch

    from solstone.think.tools.search import _MAX_RESULT_TEXT, search_journal

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
    changed = scan_journal(str(journal_fixture), full=False).changed
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
    changed = scan_journal(str(journal), full=False).changed
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
    changed = scan_journal(str(journal), full=False).changed
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
    changed = scan_journal(str(journal), full=True).changed
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


def test_search_tool_stream_filter(monkeypatch):
    """Agent search tool accepts and passes stream filter."""
    from solstone.think.tools import search as search_tools

    search = Mock(return_value=(1, [{"text": "result", "metadata": {}}]))
    counts = Mock(return_value={"facets": [], "agents": [], "days": [("20240101", 1)]})
    monkeypatch.setattr(search_tools, "search_journal_impl", search)
    monkeypatch.setattr(search_tools, "search_counts_impl", counts)

    result = search_tools.search_journal("", stream="default")

    assert "results" in result
    assert result["total"] == 1
    assert result["query"]["filters"]["stream"] == "default"
    search.assert_called_once_with("", 10, 0, rerank=True, stream="default")
    counts.assert_called_once_with("", stream="default")


def test_prune_chunks_by_stream(monkeypatch, tmp_path):
    """prune_chunks_by_stream removes only the target stream's chunks."""
    from solstone.think.indexer.journal import (
        get_journal_index,
        prune_chunks_by_stream,
        scan_journal,
    )
    from solstone.think.streams import write_segment_stream

    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    share_seg = journal / "chronicle" / "20240101" / "import.share" / "100000_300"
    share_talents = share_seg / "talents"
    share_talents.mkdir(parents=True)
    (share_talents / "screen.md").write_text(
        "# Share\n\nImported share source content.\n",
        encoding="utf-8",
    )
    write_segment_stream(share_seg, "import.share", None, None, 1)

    other_seg = journal / "chronicle" / "20240101" / "import.apple" / "101000_300"
    other_talents = other_seg / "talents"
    other_talents.mkdir(parents=True)
    (other_talents / "screen.md").write_text(
        "# Apple\n\nImported apple source content.\n",
        encoding="utf-8",
    )
    write_segment_stream(other_seg, "import.apple", None, None, 1)

    scan_journal(str(journal), full=True)
    conn, _ = get_journal_index(str(journal))
    share_count = conn.execute(
        "SELECT count(*) FROM chunks WHERE stream=?",
        ("import.share",),
    ).fetchone()[0]
    other_count = conn.execute(
        "SELECT count(*) FROM chunks WHERE stream=?",
        ("import.apple",),
    ).fetchone()[0]
    share_paths = [
        row[0]
        for row in conn.execute(
            "SELECT DISTINCT path FROM chunks WHERE stream=?",
            ("import.share",),
        ).fetchall()
    ]
    conn.close()

    assert share_count > 0
    assert other_count > 0
    assert share_paths

    result = prune_chunks_by_stream("import.share", str(journal))

    assert result["chunks"] == share_count
    assert result["files"] == len(share_paths)

    conn, _ = get_journal_index(str(journal))
    assert (
        conn.execute(
            "SELECT count(*) FROM chunks WHERE stream=?",
            ("import.share",),
        ).fetchone()[0]
        == 0
    )
    assert (
        conn.execute(
            "SELECT count(*) FROM chunks WHERE stream=?",
            ("import.apple",),
        ).fetchone()[0]
        == other_count
    )
    for path in share_paths:
        assert (
            conn.execute(
                "SELECT count(*) FROM files WHERE path=?",
                (path,),
            ).fetchone()[0]
            == 0
        )
    conn.close()


def test_delete_segment_index_rows_removes_only_target_segment(monkeypatch, tmp_path):
    """delete_segment_index_rows remains an in-process index maintenance helper."""
    from solstone.think.indexer.journal import (
        delete_segment_index_rows,
        get_journal_index,
        scan_journal,
    )
    from solstone.think.streams import write_segment_stream

    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    target_rel = "20240101/default/100000_300"
    target_seg = journal / "chronicle" / target_rel
    target_talents = target_seg / "talents"
    target_talents.mkdir(parents=True)
    (target_talents / "screen.md").write_text(
        "# Target\n\nTarget segment content.\n",
        encoding="utf-8",
    )
    write_segment_stream(target_seg, "default", None, None, 1)

    other_rel = "20240101/default/101000_300"
    other_seg = journal / "chronicle" / other_rel
    other_talents = other_seg / "talents"
    other_talents.mkdir(parents=True)
    (other_talents / "screen.md").write_text(
        "# Other\n\nOther segment content.\n",
        encoding="utf-8",
    )
    write_segment_stream(other_seg, "default", None, None, 1)

    scan_journal(str(journal), full=True)
    conn, _ = get_journal_index(str(journal))
    target_chunks = conn.execute(
        "SELECT count(*) FROM chunks WHERE path = ? OR path LIKE ?",
        (target_rel, f"{target_rel}/%"),
    ).fetchone()[0]
    target_files = conn.execute(
        "SELECT count(*) FROM files WHERE path LIKE ?",
        (f"{target_rel}/%",),
    ).fetchone()[0]
    other_chunks = conn.execute(
        "SELECT count(*) FROM chunks WHERE path = ? OR path LIKE ?",
        (other_rel, f"{other_rel}/%"),
    ).fetchone()[0]
    conn.close()

    assert target_chunks > 0
    assert target_files > 0
    assert other_chunks > 0

    result = delete_segment_index_rows(str(journal), target_rel)

    assert result == {"chunks": target_chunks, "files": target_files, "error": None}

    conn, _ = get_journal_index(str(journal))
    assert (
        conn.execute(
            "SELECT count(*) FROM chunks WHERE path = ? OR path LIKE ?",
            (target_rel, f"{target_rel}/%"),
        ).fetchone()[0]
        == 0
    )
    assert (
        conn.execute(
            "SELECT count(*) FROM files WHERE path LIKE ?",
            (f"{target_rel}/%",),
        ).fetchone()[0]
        == 0
    )
    assert (
        conn.execute(
            "SELECT count(*) FROM chunks WHERE path = ? OR path LIKE ?",
            (other_rel, f"{other_rel}/%"),
        ).fetchone()[0]
        == other_chunks
    )
    conn.close()


def test_existing_agent_chunks_unchanged(journal_fixture):
    """Per-file child chunks remain independently searchable."""
    from solstone.think.indexer.journal import get_journal_index, scan_journal

    scan_journal(str(journal_fixture), verbose=True, full=True)
    conn, _ = get_journal_index(str(journal_fixture))
    screen_chunks = conn.execute(
        "SELECT count(*) FROM chunks WHERE path='20240101/default/100000_300/talents/screen.md'"
    ).fetchone()[0]
    activity_chunks = conn.execute(
        "SELECT count(*) FROM chunks WHERE path='20240101/default/100000_300/talents/activity.md'"
    ).fetchone()[0]
    conn.close()
    assert screen_chunks >= 1
    assert activity_chunks >= 1


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
    (segment_dir / "sense.json").write_text(
        json.dumps(
            {
                "density": "active",
                "content_type": "coding",
                "activity_summary": "Unique regression seed for pure indexing.",
                "entities": [
                    {
                        "name": "Zephyr Quartz Index",
                        "type": "Project",
                        "role": "mentioned",
                        "source": "screen",
                        "context": "Used only for indexer purity coverage.",
                    }
                ],
                "facets": [
                    {"facet": "work", "activity": "search indexing", "level": "high"}
                ],
                "speculative_facet": None,
                "meeting_detected": False,
                "speakers": [],
                "recommend": {"screen_record": False, "speaker_attribution": False},
                "emotional_register": "focused",
            }
        ),
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


def test_scan_journal_rescan_does_not_record_entity_ambiguities(journal_copy):
    """Index rescans must not create domain ambiguity rows for low-confidence names."""
    from solstone.think.entities import load_ambiguities
    from solstone.think.indexer.journal import scan_journal

    journal_path = Path(journal_copy)
    for entity_id, name in [
        ("sarah_connor", "Sarah Connor"),
        ("sarah_lee", "Sarah Lee"),
    ]:
        entity_dir = journal_path / "entities" / entity_id
        entity_dir.mkdir(parents=True, exist_ok=True)
        (entity_dir / "entity.json").write_text(
            json.dumps(
                {
                    "id": entity_id,
                    "name": name,
                    "type": "Person",
                    "created_at": "2026-01-01T00:00:00Z",
                    "updated_at": "2026-01-01T00:00:00Z",
                }
            ),
            encoding="utf-8",
        )

    today = datetime.now().strftime("%Y%m%d")
    segment_dir = (
        journal_path / "chronicle" / today / "default" / "121000_300" / "talents"
    )
    segment_dir.mkdir(parents=True)
    (segment_dir / "sense.json").write_text(
        json.dumps(
            {
                "density": "active",
                "content_type": "meeting",
                "activity_summary": "Sarah discussed a low-confidence indexer case.",
                "entities": [
                    {
                        "name": "Sarah",
                        "type": "Person",
                        "role": "mentioned",
                        "source": "screen",
                        "context": "Ambiguous against Sarah Connor and Sarah Lee.",
                    }
                ],
                "facets": [
                    {"facet": "work", "activity": "search indexing", "level": "high"}
                ],
                "speculative_facet": None,
                "meeting_detected": False,
                "speakers": [],
                "recommend": {"screen_record": False, "speaker_attribution": False},
                "emotional_register": "focused",
            }
        ),
        encoding="utf-8",
    )

    ambiguity_path = journal_path / "entities" / "ambiguities.jsonl"
    assert not ambiguity_path.exists()

    scan_journal(str(journal_path), full=True)
    scan_journal(str(journal_path), full=True)

    assert not ambiguity_path.exists()
    assert load_ambiguities() == []


def _native_read_kwargs() -> dict:
    from solstone.think import core_handshake

    helper = Path(__file__).resolve().parents[1] / "core" / "target" / "debug" / "solstone-core"
    return {
        "handshake_checker": lambda: core_handshake.CoreHandshakeResult("ok"),
        "helper_locator": lambda: helper,
        "platform_reader": lambda: ("linux", "x86_64"),
        "platform_tag_reader": lambda: {"manylinux2014_x86_64"},
    }


def _seed_native_chunk(
    journal: Path,
    *,
    content: str,
    day: str,
    idx: int = 0,
    agent: str = "flow",
) -> None:
    conn, _ = get_journal_index(str(journal))
    conn.execute(
        "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        (content, f"notes/{day}-{idx}.md", day, "work", agent, "default", idx, ""),
    )
    conn.commit()
    conn.close()


def test_search_journal_exposes_building_state_without_changing_tuple_shape(
    tmp_path, monkeypatch
):
    """The native reset state reaches the opt-in degraded output dictionary."""
    import solstone.think.utils as think_utils
    from solstone.think.indexer import journal, native

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    think_utils._journal_path_cache = None
    helper = Path(__file__).resolve().parents[1] / "core" / "target" / "debug" / "solstone-core"
    subprocess.run(
        [str(helper), "indexer", "--journal", str(tmp_path), "--reset"],
        check=True,
        capture_output=True,
        text=True,
    )
    _seed_native_chunk(
        tmp_path,
        content="building state needle",
        day="20260107",
    )
    monkeypatch.setattr(
        journal,
        "run_native_indexer_search",
        lambda query, journal_path, **options: native.run_native_indexer_search(
            query, journal_path, **_native_read_kwargs(), **options
        ),
    )

    degraded: dict[str, object] = {}
    result = search_journal("needle", degraded_out=degraded)

    assert isinstance(result, tuple)
    assert result[0] == 1
    assert len(result[1]) == 1
    assert degraded["kind"] == "building"


def test_native_read_bridge_uses_real_binary_for_recency_and_unicode(tmp_path, monkeypatch):
    """The public shim preserves Rust browse ordering and Unicode retrieval."""
    import solstone.think.utils as think_utils

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    think_utils._journal_path_cache = None
    for day in range(1, 8):
        for idx in range(3):
            _seed_native_chunk(
                tmp_path,
                content="José planning meeting",
                day=f"2026010{day}",
                idx=idx,
            )

    total, results = search_journal(
        "", day_from="20260101", day_to="20260107", agent="flow", limit=12
    )
    assert total == 21
    assert [result["metadata"]["day"] for result in results] == [
        "20260107",
        "20260107",
        "20260107",
        "20260106",
        "20260106",
        "20260106",
        "20260105",
        "20260105",
        "20260105",
        "20260104",
        "20260104",
        "20260104",
    ]
    _, browse_results = search_journal("", rerank=True, limit=5, offset=0)
    assert len(browse_results) == 5
    assert search_journal("José")[0] == 21

    _seed_native_chunk(
        tmp_path,
        content="ordinary unicode result",
        day="20260107",
        idx=99,
        agent="flow",
    )
    _, ordinary_results = search_journal("ordinary unicode result")
    assert set(ordinary_results[0]) == {"id", "text", "metadata", "score"}


def test_native_read_shim_distinguishes_empty_and_absent_indexes(tmp_path, monkeypatch):
    """Only an existing empty index maps to the legacy empty read shapes."""
    import solstone.think.utils as think_utils
    from solstone.think.indexer.journal import (
        NativeIndexerReadError,
        get_corpus_day_coverage,
        known_agents,
    )

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    think_utils._journal_path_cache = None
    conn, _ = get_journal_index(str(tmp_path))
    conn.close()
    assert search_journal("needle") == (0, [])
    assert search_counts("needle")["total"] == 0
    assert known_agents() == set()
    assert get_corpus_day_coverage() is None

    absent = tmp_path / "absent"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(absent))
    think_utils._journal_path_cache = None
    for read in (
        lambda: search_journal("needle"),
        lambda: search_counts("needle"),
        known_agents,
        get_corpus_day_coverage,
    ):
        with pytest.raises(NativeIndexerReadError, match="index"):
            read()


def test_non_tokenizable_native_reason_is_available_at_bridge_boundary(tmp_path):
    from collections import Counter

    from solstone.think.indexer import native

    response = native.run_native_indexer_search(
        "📅", str(tmp_path), limit=0, offset=0, **_native_read_kwargs()
    )
    assert response["reason"] == "not_tokenizable"
    assert search_journal("📅") == (0, [])
    assert search_counts("📅") == {
        "total": 0,
        "facets": Counter(),
        "agents": Counter(),
        "days": Counter(),
        "streams": Counter(),
        "relaxed": False,
        "degraded": None,
    }


def test_rerank_requests_native_pool_with_limit_fifty(monkeypatch):
    """Reranking fetches a fixed native pool before local scoring."""
    from solstone.think.indexer import journal, native

    argvs: list[list[str]] = []

    def runner(argv, *, capture_output, text, check):
        assert capture_output is True
        assert text is True
        assert check is False
        argvs.append(argv)
        return subprocess.CompletedProcess(
            argv,
            0,
            stdout=json.dumps(
                {
                    "results": [],
                    "order": "relevance",
                    "relaxed": False,
                    "total": 0,
                    "counts": {"total": 0, "facets": {}, "agents": {}, "days": {}, "streams": {}, "relaxed": False},
                    "cleaned_query": "needle",
                }
            ),
        )

    monkeypatch.setattr(
        journal,
        "run_native_indexer_search",
        lambda query, journal_path, **options: native.run_native_indexer_search(
            query, journal_path, native_runner=runner, **_native_read_kwargs(), **options
        ),
    )
    search_journal("needle", limit=5, offset=0, rerank=True)
    assert argvs[0][argvs[0].index("--limit") + 1] == "50"
    assert argvs[0][argvs[0].index("--offset") + 1] == "0"
