# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tool functions for search operations.

These functions can be imported and called directly from agent workflows,
tests, or other internal modules.
"""

from datetime import datetime, timedelta
from typing import Any

from solstone.think.indexer.journal import search_counts as search_counts_impl
from solstone.think.indexer.journal import search_journal as search_journal_impl

_MAX_RESULT_TEXT = 4096


def _bucket_day_counts(day_counts: dict[str, int]) -> dict[str, Any]:
    """Bucket day counts into recent days, top days, and bucketed days.

    Returns:
        Dict with:
        - recent_days: Last 7 days individually (includes 0 counts)
        - top_days: Top 20 days by count
        - bucketed_days: Older days grouped by week (day_from-day_to format)
    """
    today = datetime.now()

    # Generate last 7 days (including today)
    recent_dates = []
    for i in range(7):
        d = today - timedelta(days=i)
        recent_dates.append(d.strftime("%Y%m%d"))

    recent_days = {d: day_counts.get(d, 0) for d in recent_dates}

    # Top 20 days by count
    sorted_days = sorted(day_counts.items(), key=lambda x: (-x[1], x[0]))
    top_days = dict(sorted_days[:20])

    # Weekly buckets for days older than 7 days
    cutoff = (today - timedelta(days=7)).strftime("%Y%m%d")
    older_days = {d: c for d, c in day_counts.items() if d < cutoff}

    weekly_buckets: dict[str, int] = {}
    for day_str, count in older_days.items():
        try:
            day_date = datetime.strptime(day_str, "%Y%m%d")
            # Find the Monday of that week
            week_start = day_date - timedelta(days=day_date.weekday())
            week_end = week_start + timedelta(days=6)
            bucket_key = (
                f"{week_start.strftime('%Y%m%d')}-{week_end.strftime('%Y%m%d')}"
            )
            weekly_buckets[bucket_key] = weekly_buckets.get(bucket_key, 0) + count
        except ValueError:
            continue

    # Sort bucketed days by start date descending, omit empty weeks
    bucketed_days = dict(
        sorted(
            ((k, v) for k, v in weekly_buckets.items() if v > 0),
            key=lambda x: x[0],
            reverse=True,
        )
    )

    return {
        "recent_days": recent_days,
        "top_days": top_days,
        "bucketed_days": bucketed_days,
    }


def search_journal(
    query: str = "",
    limit: int = 10,
    offset: int = 0,
    *,
    day: str | None = None,
    day_from: str | None = None,
    day_to: str | None = None,
    facet: str | None = None,
    agent: str | None = None,
    stream: str | None = None,
    time_bucket: str | None = None,
) -> dict[str, Any]:
    """Search across all journal content using semantic full-text search.

    This tool searches through all indexed journal content including insights,
    transcripts, events, entities, and todos. Use filters to narrow results
    to specific content types or contexts.

    Args:
        query: Search query. Words are AND'd by default; use OR to match any
            (e.g., "apple OR orange"), quotes for exact phrases, * for prefix match.
            Empty string returns all content matching the filters.
        limit: Maximum number of results to return (default: 10)
        offset: Number of results to skip for pagination (default: 0)
        day: Filter by exact day in ``YYYYMMDD`` format (mutually exclusive with day_from/day_to)
        day_from: Filter by date range start (``YYYYMMDD``, inclusive)
        day_to: Filter by date range end (``YYYYMMDD``, inclusive)
        facet: Filter by facet name (e.g., "work", "personal")
        agent: Filter by agent (e.g., "meetings", "event", "entity:detected", "news")
        stream: Filter by stream name (e.g., "archon", "import.apple")
        time_bucket: Filter by time of day — "morning" (06:00–11:59),
            "afternoon" (12:00–16:59), "evening" (17:00–20:59), or "night" (21:00–05:59)

    Returns:
        Dictionary containing:
        - total: Total number of matching results
        - limit: Current limit value
        - offset: Current offset value
        - query: Echo of query text and applied filters
        - counts: Aggregation metadata with facets, agents, and bucketed days
        - results: List of matches with day, facet, agent, stream, text, path, and idx

    Examples:
        - search_journal("machine learning")
        - search_journal("meeting notes", day="20240101")
        - search_journal("project planning", facet="work")
        - search_journal("standup", agent="event")
        - search_journal("weekly sync", day_from="20241201", day_to="20241207")
        - search_journal(agent="meetings", day="20240101")  # Browse all meetings for a day
        - search_journal("meeting", stream="archon")  # Filter by stream
        - search_journal("standup", time_bucket="morning")  # Morning meetings
    """
    try:
        kwargs: dict[str, Any] = {}
        filters: dict[str, Any] = {}
        if day is not None:
            kwargs["day"] = day
            filters["day"] = day
        if day_from is not None:
            kwargs["day_from"] = day_from
            filters["day_from"] = day_from
        if day_to is not None:
            kwargs["day_to"] = day_to
            filters["day_to"] = day_to
        if facet is not None:
            kwargs["facet"] = facet
            filters["facet"] = facet
        if agent is not None:
            kwargs["agent"] = agent
            filters["agent"] = agent
        if stream is not None:
            kwargs["stream"] = stream
            filters["stream"] = stream
        if time_bucket is not None:
            kwargs["time_bucket"] = time_bucket
            filters["time_bucket"] = time_bucket

        # Get search results
        total, results = search_journal_impl(query, limit, offset, **kwargs)

        # Get aggregation counts
        counts_data = search_counts_impl(query, **kwargs)

        # Build result items with full metadata
        items = []
        for r in results:
            meta = r.get("metadata", {})
            text = r.get("text", "")
            if len(text) > _MAX_RESULT_TEXT:
                text = text[:_MAX_RESULT_TEXT] + (
                    f"\n\n[... truncated from {len(text):,} chars]"
                )
            item = {
                "day": meta.get("day", ""),
                "facet": meta.get("facet", ""),
                "agent": meta.get("agent", ""),
                "text": text,
                "path": meta.get("path", ""),
                "idx": meta.get("idx", 0),
            }
            if meta.get("stream"):
                item["stream"] = meta["stream"]
            items.append(item)

        # Build counts structure
        day_buckets = _bucket_day_counts(dict(counts_data["days"]))
        counts = {
            "facets": dict(counts_data["facets"]),
            "agents": dict(counts_data["agents"]),
            **day_buckets,
        }

        return {
            "total": total,
            "limit": limit,
            "offset": offset,
            "query": {"text": query, "filters": filters},
            "counts": counts,
            "results": items,
        }
    except Exception as exc:
        return {
            "error": f"Failed to search journal: {exc}",
            "suggestion": "try adjusting the query or ensure the index exists (run journal indexer --rescan)",
        }
