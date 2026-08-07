# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import html
import re
import sqlite3
from datetime import datetime
from typing import Any

from flask import Blueprint, jsonify, request

from solstone.convey.day_grid import build_day_grid_payload
from solstone.convey.icons import lucide_svg
from solstone.convey.reasons import INVALID_DAY, SEARCH_FAILED
from solstone.convey.utils import error_response, format_date, parse_pagination_params
from solstone.think.facets import get_facets
from solstone.think.indexer.journal import (
    get_corpus_day_coverage,
    search_counts,
    search_journal,
)
from solstone.think.indexer.native import NativeIndexerReadError

search_bp = Blueprint(
    "app:search",
    __name__,
    url_prefix="/app/search",
)

# Agent icons for display
AGENT_ICONS = {
    "flow": "activity",
    "knowledge_graph": "chart-network",
    "meetings": "users",
    "event": "calendar-days",
    "span": "timeline",
    "audio": "mic-vocal",
    "screen": "monitor",
    "entity": "user",
    "entity:attached": "user-check",
    "entity:detected": "user-search",
    "news": "newspaper",
    "import": "import",
}
AGENT_ICON_FALLBACK = "file-text"

# Agent display names
AGENT_LABELS = {
    "flow": "Flow",
    "knowledge_graph": "Knowledge Graph",
    "meetings": "Meetings",
    "event": "Event",
    "span": "Span",
    "audio": "Transcript",
    "screen": "Screen",
    "entity": "Entity",
    "entity:attached": "Entity",
    "entity:detected": "Entity",
    "news": "News",
    "import": "Import",
}


def _agent_label(agent: str | None) -> str:
    if not agent:
        return ""
    mapped = AGENT_LABELS.get(agent)
    if mapped is not None:
        return mapped
    return " ".join(agent.replace("_", " ").split()).title()


def _agent_icon(agent: str | None) -> str:
    return AGENT_ICONS.get(agent or "", AGENT_ICON_FALLBACK)


def _agent_icon_svg(agent: str | None) -> str | None:
    return lucide_svg(_agent_icon(agent))


DAY_RE = re.compile(r"^\d{8}$")
DAY_BOUND_SENTINELS = {"00000000", "99999999"}


def _parse_facet_filter() -> str | None:
    """Parse facet filter from request args."""
    return request.args.get("facet", "").strip() or None


def _parse_agent_filter() -> str | None:
    """Parse single agent filter from request args."""
    return request.args.get("agent", "").strip() or None


def _parse_stream_filter() -> str | None:
    """Parse stream filter from request args."""
    return request.args.get("stream", "").strip() or None


def _parse_day_bound(name: str) -> str | None:
    """Parse a day range bound from request args."""
    value = request.args.get(name, "").strip()
    if not value or value in DAY_BOUND_SENTINELS:
        return None
    return value


def _validate_day_bound(name: str, value: str | None) -> str | None:
    if value is None:
        return None
    if not DAY_RE.fullmatch(value):
        return f"{name} must be YYYYMMDD"
    try:
        datetime.strptime(value, "%Y%m%d")
    except ValueError:
        return f"{name} must be a real day"
    return None


def _parse_day_range_filter() -> tuple[str | None, str | None, str | None]:
    day_from = _parse_day_bound("day_from")
    day_to = _parse_day_bound("day_to")
    for name, value in (("day_from", day_from), ("day_to", day_to)):
        error = _validate_day_bound(name, value)
        if error:
            return day_from, day_to, error
    if day_from and day_to and day_from > day_to:
        return day_from, day_to, "day_from must be <= day_to"
    return day_from, day_to, None


def _day_in_range(day: str, day_from: str | None, day_to: str | None) -> bool:
    if day_from and day < day_from:
        return False
    if day_to and day > day_to:
        return False
    return True


def _highlight_query_terms(text: str, query: str) -> str:
    """Add bold highlighting around query terms in text."""
    if not query:
        return text
    text = html.escape(text)
    # Extract individual words from query (ignore FTS operators)
    terms = [t for t in query.split() if t.upper() not in ("AND", "OR", "NOT")]
    for term in terms:
        # Remove quotes and wildcards for matching
        clean_term = term.strip('"*')
        if len(clean_term) >= 2:
            # Case-insensitive replacement with bold
            pattern = re.compile(re.escape(clean_term), re.IGNORECASE)
            text = pattern.sub(lambda m: f"<strong>{m.group()}</strong>", text)
    return text


def _format_result(result: dict, query: str, facets_map: dict) -> dict:
    """Format a search result for API response."""
    meta = result.get("metadata", {})
    agent = meta.get("agent", "")
    text = result.get("text", "")
    facet_name = meta.get("facet", "")

    # Get facet metadata
    facet_info = facets_map.get(facet_name, {})

    # Clean and truncate text
    words = text.split()
    if len(words) > 50:
        display_text = " ".join(words[:50]) + "..."
    else:
        display_text = text

    # Apply highlighting
    display_text = _highlight_query_terms(display_text, query)

    return {
        "id": result.get("id", ""),
        "day": meta.get("day", ""),
        "agent": agent,
        "agent_icon": _agent_icon(agent),
        "agent_icon_svg": _agent_icon_svg(agent),
        "agent_label": _agent_label(agent),
        "facet": facet_name,
        "facet_title": facet_info.get("title", facet_name),
        "facet_color": facet_info.get("color", ""),
        "facet_emoji": facet_info.get("emoji", ""),
        "text": display_text,
        "stream": meta.get("stream", ""),
        "path": meta.get("path", ""),
        "idx": meta.get("idx", 0),
        "score": result.get("score", 0.0),
    }


@search_bp.route("/api/search")
def search_journal_api() -> Any:
    """Unified journal search endpoint with day grouping.

    Query parameters:
        q: Search query (required)
        limit: Max results per day (default 5)
        offset: Day offset for pagination (default 0)
        facet: Filter by facet name (optional, empty string for no-facet items)
        agent: Filter by single agent (optional)
        stream: Filter by stream name (optional)
        day_from: Inclusive day range start (optional, YYYYMMDD)
        day_to: Inclusive day range end (optional, YYYYMMDD)

    Returns:
        JSON with:
        - total: Total match count (range-scoped when day_from/day_to is active)
        - total_days: Count of day groups after range filtering
        - showing_days: Count of day groups in this page
        - relaxed: Whether search used relaxed matching
        - days: List of day groups, each with date info and results
        - facets: List of facets with counts for filter sidebar
        - talents: List of talents with counts for filter sidebar
        - day_grid: {coverage, days, pending} payload for the day grid
    """
    query = request.args.get("q", "").strip()

    # Parse parameters
    results_per_day, day_offset = parse_pagination_params(
        default_limit=5, max_limit=100, min_limit=1
    )
    facet_filter = _parse_facet_filter()
    agent_filter = _parse_agent_filter()
    stream_filter = _parse_stream_filter()
    day_from_filter, day_to_filter, day_range_error = _parse_day_range_filter()
    if day_range_error:
        return error_response(INVALID_DAY, detail=day_range_error)
    range_active = day_from_filter is not None or day_to_filter is not None

    # Load facet metadata for enriching results
    facets_map = get_facets()

    try:
        # Get aggregation counts efficiently (lightweight query, no content)
        # First get unfiltered counts for sidebar display
        base_counts = search_counts(query, stream=stream_filter, relax=True)
        facet_counts = dict(base_counts["facets"])
        agent_counts = dict(base_counts["agents"])

        # Get filtered counts for results
        filtered_counts = search_counts(
            query,
            facet=facet_filter,
            agent=agent_filter,
            stream=stream_filter,
            relax=True,
        )
        day_counts = dict(filtered_counts["days"])
        day_grid = build_day_grid_payload(
            day_counts,
            max(day_counts, default=None),
            coverage=get_corpus_day_coverage(),
        )

        # Determine which days to show (sorted descending)
        sorted_days = [
            day
            for day in sorted(day_counts.keys(), reverse=True)
            if _day_in_range(day, day_from_filter, day_to_filter)
        ]
        total = (
            sum(day_counts.get(day, 0) for day in sorted_days)
            if range_active
            else filtered_counts["total"]
        )

        # Apply day pagination
        paginated_days = sorted_days[day_offset : day_offset + 20]

        # Fetch results for each paginated day
        days_response = []
        for day in paginated_days:
            _, day_results = search_journal(
                query,
                limit=results_per_day,
                offset=0,
                day=day,
                facet=facet_filter,
                agent=agent_filter,
                stream=stream_filter,
                relax=True,
                include_total=False,
            )
            total_in_day = day_counts.get(day, 0)

            formatted_results = [
                _format_result(r, query, facets_map) for r in day_results
            ]

            days_response.append(
                {
                    "day": day,
                    "date": format_date(day),
                    "total": total_in_day,
                    "showing": len(formatted_results),
                    "has_more": total_in_day > results_per_day,
                    "results": formatted_results,
                }
            )
    except (NativeIndexerReadError, sqlite3.OperationalError) as exc:
        return error_response(SEARCH_FAILED, detail=str(exc))

    # Build facet list for sidebar with counts (unfiltered counts for discovery)
    facets_list = []
    for name, data in facets_map.items():
        if data.get("muted"):
            continue
        facets_list.append(
            {
                "name": name,
                "title": data.get("title", name),
                "color": data.get("color", ""),
                "emoji": data.get("emoji", ""),
                "count": facet_counts.get(name, 0),
            }
        )
    # Sort by count descending
    facets_list.sort(key=lambda x: x["count"], reverse=True)

    # Build agent list for sidebar (unfiltered counts for discovery)
    agents_list = []
    for agent, count in sorted(agent_counts.items(), key=lambda x: -x[1]):
        agents_list.append(
            {
                "name": agent,
                "label": _agent_label(agent),
                "icon": _agent_icon(agent),
                "icon_svg": _agent_icon_svg(agent),
                "count": count,
            }
        )

    return jsonify(
        {
            "total": total,
            "total_days": len(sorted_days),
            "showing_days": len(days_response),
            "relaxed": filtered_counts["relaxed"],
            "days": days_response,
            "facets": facets_list,
            "talents": agents_list,
            "day_grid": day_grid,
        }
    )


@search_bp.route("/api/day_results")
def day_results_api() -> Any:
    """Get more results for a specific day.

    Query parameters:
        q: Search query
        day: Day to get results for (YYYYMMDD)
        offset: Result offset within the day (default 0)
        limit: Max results (default 20)
        facet: Facet filter (optional)
        agent: Single agent filter (optional)
    """
    query = request.args.get("q", "").strip()
    day = request.args.get("day", "").strip()
    if not day:
        return jsonify({"results": [], "total": 0})

    limit, offset = parse_pagination_params(
        default_limit=20, max_limit=100, min_limit=1
    )
    facet_filter = _parse_facet_filter()
    agent_filter = _parse_agent_filter()
    stream_filter = _parse_stream_filter()

    facets_map = get_facets()

    try:
        # Get total count for this day with filters
        counts = search_counts(
            query,
            day=day,
            facet=facet_filter,
            agent=agent_filter,
            stream=stream_filter,
            relax=True,
        )
        total_in_day = counts["total"]

        # Fetch paginated results
        _, rows = search_journal(
            query,
            limit=limit,
            offset=offset,
            day=day,
            facet=facet_filter,
            agent=agent_filter,
            stream=stream_filter,
            relax=True,
            include_total=False,
        )

        formatted = [_format_result(r, query, facets_map) for r in rows]
    except (NativeIndexerReadError, sqlite3.OperationalError) as exc:
        return error_response(SEARCH_FAILED, detail=str(exc))

    return jsonify(
        {
            "day": day,
            "total": total_in_day,
            "offset": offset,
            "results": formatted,
        }
    )
