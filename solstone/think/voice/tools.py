# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Voice tool manifest and dispatch."""

from __future__ import annotations

import json
import logging
import re
from dataclasses import asdict
from datetime import date, datetime, timedelta
from pathlib import Path
from typing import Any, Callable
from urllib.parse import quote_plus

from solstone.apps.entities.routes import _build_facet_relationships
from solstone.apps.home.routes import _load_briefing_md
from solstone.think.activities import load_activity_records
from solstone.think.cluster import cluster_segments, scan_day
from solstone.think.entities.journal import load_journal_entity
from solstone.think.facets import get_facets
from solstone.think.indexer.journal import search_journal
from solstone.think.surfaces import ledger as ledger_surface
from solstone.think.surfaces.profile import full as load_profile
from solstone.think.utils import day_path
from solstone.think.voice.nav_queue import get_nav_queue
from solstone.think.voice.observer_queue import get_observer_queue

logger = logging.getLogger(__name__)

SEARCH_ENTITY_RE = re.compile(r"(?:^entity:|entities/)([a-z0-9_]+)")
SUMMARY_MARKERS_RE = re.compile(r"[*_`>#]")
TOOL_MANIFEST: list[dict[str, Any]] = [
    {
        "type": "function",
        "name": "journal.get_day",
        "description": "Read one journal day and summarize the available segments.",
        "parameters": {
            "type": "object",
            "properties": {"day": {"type": "string"}},
            "required": ["day"],
        },
    },
    {
        "type": "function",
        "name": "journal.search",
        "description": "Search the journal for recent matching entries.",
        "parameters": {
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "facet": {"type": ["string", "null"]},
                "days": {"type": ["integer", "null"]},
                "limit": {"type": ["integer", "null"]},
            },
            "required": ["query"],
        },
    },
    {
        "type": "function",
        "name": "entities.get",
        "description": "Read one entity profile by slug.",
        "parameters": {
            "type": "object",
            "properties": {"entity_slug": {"type": "string"}},
            "required": ["entity_slug"],
        },
    },
    {
        "type": "function",
        "name": "entities.recent_with",
        "description": "Read recent interactions with an entity.",
        "parameters": {
            "type": "object",
            "properties": {
                "entity_slug": {"type": "string"},
                "days": {"type": ["integer", "null"]},
                "facet": {"type": ["string", "null"]},
            },
            "required": ["entity_slug"],
        },
    },
    {
        "type": "function",
        "name": "commitments.list",
        "description": "List commitments from the ledger surface.",
        "parameters": {
            "type": "object",
            "properties": {
                "state": {"type": ["string", "null"]},
                "facet": {"type": ["string", "null"]},
                "limit": {"type": ["integer", "null"]},
            },
        },
    },
    {
        "type": "function",
        "name": "commitments.complete",
        "description": "Close a commitment through the ledger surface.",
        "parameters": {
            "type": "object",
            "properties": {
                "commitment_id": {"type": "string"},
                "resolution": {"type": "string"},
            },
            "required": ["commitment_id", "resolution"],
        },
    },
    {
        "type": "function",
        "name": "calendar.today",
        "description": "Read today's anticipated activities.",
        "parameters": {"type": "object", "properties": {}},
    },
    {
        "type": "function",
        "name": "briefing.get",
        "description": "Read today's briefing if one exists.",
        "parameters": {"type": "object", "properties": {}},
    },
    {
        "type": "function",
        "name": "observer.start_listening",
        "description": (
            "Request sol to start listening in the given mode. "
            "Returns immediately after queueing the start request."
        ),
        "parameters": {
            "type": "object",
            "properties": {"mode": {"type": "string"}},
            "required": ["mode"],
        },
    },
]

VALID_COMMITMENT_STATES = {"open", "closed", "dropped"}
VALID_RESOLUTIONS = {"done", "sent", "signed", "dropped", "deferred"}
VALID_LISTEN_MODES = {"meeting", "voice_memo"}


def _today() -> date:
    return date.today()


def _format_day_external(day_value: str) -> str:
    parsed = datetime.strptime(day_value, "%Y%m%d").date()
    return parsed.isoformat()


def _today_internal() -> str:
    return _today().strftime("%Y%m%d")


def _normalize_day(value: Any) -> tuple[str, str]:
    if not isinstance(value, str) or not value.strip():
        raise ValueError("invalid day")
    raw = value.strip()
    try:
        parsed = datetime.strptime(raw, "%Y-%m-%d").date()
    except ValueError as exc:
        raise ValueError("invalid day") from exc
    internal = parsed.strftime("%Y%m%d")
    return internal, parsed.isoformat()


def _as_int(value: Any, *, default: int, field_name: str, minimum: int = 1) -> int:
    if value is None:
        return default
    if isinstance(value, bool) or not isinstance(value, int):
        raise ValueError(f"invalid {field_name}")
    if value < minimum:
        raise ValueError(f"invalid {field_name}")
    return value


def _clean_optional_str(value: Any) -> str | None:
    if value is None:
        return None
    if not isinstance(value, str):
        raise ValueError("invalid string field")
    cleaned = value.strip()
    return cleaned or None


def get_tool_manifest() -> list[dict[str, Any]]:
    return json.loads(json.dumps(TOOL_MANIFEST))


def _segment_duration(segment_key: str) -> int:
    try:
        return int(segment_key.rsplit("_", 1)[1])
    except (IndexError, ValueError) as exc:
        raise ValueError("invalid segment key") from exc


def _segment_summary(day_dir: Path, stream: str, segment_key: str) -> str:
    segment_dir = day_dir / stream / segment_key
    if not segment_dir.is_dir():
        return ""
    summaries = sorted(segment_dir.glob("*summary.md"))
    if not summaries:
        return ""
    chunks: list[str] = []
    for path in summaries:
        text = path.read_text(encoding="utf-8").strip()
        if text:
            chunks.append(text)
    return "\n\n".join(chunks)


def _build_day_summary(segment_summaries: list[str]) -> str:
    cleaned = [summary.strip() for summary in segment_summaries if summary.strip()]
    return "\n\n".join(cleaned)


def _extract_entity_slug(path_value: Any) -> str | None:
    if not isinstance(path_value, str):
        return None
    match = SEARCH_ENTITY_RE.search(path_value)
    return match.group(1) if match else None


def _truncate_snippet(text: str, *, words: int = 50) -> str:
    tokens = text.split()
    if len(tokens) <= words:
        return text
    return " ".join(tokens[:words]) + "..."


def _plain_text(value: str) -> str:
    cleaned_lines = []
    for line in value.splitlines():
        stripped = line.strip()
        if stripped.startswith("- "):
            stripped = stripped[2:]
        stripped = SUMMARY_MARKERS_RE.sub("", stripped)
        stripped = re.sub(r"\s+", " ", stripped).strip()
        if stripped:
            cleaned_lines.append(stripped)
    return "\n".join(cleaned_lines)


def _item_day(epoch_ms: int | None) -> str | None:
    if not epoch_ms:
        return None
    return datetime.fromtimestamp(epoch_ms / 1000).date().isoformat()


def _shape_commitment(item: Any, *, resolution: str | None = None) -> dict[str, Any]:
    payload = asdict(item)
    payload.pop("sources", None)
    result = {
        "id": payload["id"],
        "owner": payload["owner"],
        "action": payload["action"],
        "counterparty": payload["counterparty"],
        "state": payload["state"],
        "context": payload["context"],
        "day_opened": _item_day(payload.get("opened_at")),
    }
    closed_day = _item_day(payload.get("closed_at"))
    if closed_day:
        result["day_closed"] = closed_day
    if resolution:
        result["resolution"] = resolution
    elif payload["state"] == "dropped":
        result["resolution"] = "dropped"
    return result


def _build_profile_markdown(profile: Any) -> str:
    lines = [f"# {profile.name}", ""]
    if profile.description:
        lines.append(profile.description)
        lines.append("")
    lines.append(f"- Type: {profile.type}")
    if profile.facets:
        lines.append(f"- Facets: {', '.join(profile.facets)}")
    lines.append(
        f"- Recent interactions (30d): {profile.cadence.recent_interactions_count_30d}"
    )
    lines.append(f"- Open commitments: {len(profile.open_with_them)}")
    lines.append(f"- Decisions (30d): {len(profile.decisions_involving_them)}")
    return "\n".join(lines).strip()


def _recent_context_from_profile(profile: Any) -> list[dict[str, str]]:
    items = list(profile.open_with_them) + list(profile.closed_with_them_30d)
    items.sort(key=lambda item: item.closed_at or item.opened_at or 0, reverse=True)
    context: list[dict[str, str]] = []
    for item in items[:5]:
        when = (
            item.when
            if isinstance(item.when, str) and item.when
            else _item_day(item.opened_at)
        )
        if not when:
            continue
        context.append(
            {
                "date": when,
                "summary": item.summary or item.context or item.action,
            }
        )
    return context


def _tag_values(
    profile: Any | None, journal_entity: dict[str, Any] | None
) -> list[str]:
    tags: list[str] = []
    for value in getattr(profile, "facets", ()) or ():
        if value not in tags:
            tags.append(value)
    for value in getattr(profile, "aka", ()) or ():
        if value not in tags:
            tags.append(value)
    if isinstance(journal_entity, dict):
        for value in journal_entity.get("aka", []) or []:
            if isinstance(value, str) and value not in tags:
                tags.append(value)
    return tags


def _resolve_entity(entity_slug: Any) -> tuple[Any | None, dict[str, Any] | None]:
    slug = _clean_optional_str(entity_slug)
    if slug is None:
        raise ValueError("entity_slug is required")
    profile = load_profile(slug)
    journal_entity = load_journal_entity(slug)
    if profile is None and journal_entity is None:
        return None, None
    return profile, journal_entity


def _matching_names(
    profile: Any | None, journal_entity: dict[str, Any] | None
) -> set[str]:
    values: set[str] = set()
    if profile is not None:
        values.add(profile.name.casefold())
        values.update(alias.casefold() for alias in profile.aka)
    if isinstance(journal_entity, dict):
        name = journal_entity.get("name")
        if isinstance(name, str) and name.strip():
            values.add(name.strip().casefold())
        for alias in journal_entity.get("aka", []) or []:
            if isinstance(alias, str) and alias.strip():
                values.add(alias.strip().casefold())
    return values


def _iter_days(window_days: int) -> list[str]:
    today = _today()
    return [
        (today - timedelta(days=offset)).strftime("%Y%m%d")
        for offset in range(window_days)
    ]


def _list_facets(facet: str | None) -> list[str]:
    if facet:
        return [facet]
    return list(get_facets().keys())


def _matching_participation_note(
    record: dict[str, Any], names: set[str], slug: str
) -> str:
    for entry in record.get("participation", []) or []:
        if not isinstance(entry, dict):
            continue
        entity_id = entry.get("entity_id")
        if entity_id == slug:
            context = entry.get("context")
            return context.strip() if isinstance(context, str) else ""
        name = entry.get("name")
        if isinstance(name, str) and name.strip().casefold() in names:
            context = entry.get("context")
            return context.strip() if isinstance(context, str) else ""
    return ""


def handle_journal_get_day(payload: dict[str, Any], app: Any) -> dict[str, Any]:
    del app
    try:
        internal_day, external_day = _normalize_day(payload.get("day"))
    except ValueError as exc:
        return {"error": str(exc)}
    day_dir = day_path(internal_day, create=False)
    if not day_dir.is_dir():
        return {"error": "day not found"}

    _, _, scan_rows = scan_day(internal_day)
    segment_rows = cluster_segments(internal_day)
    scan_keys = {row["key"] for row in scan_rows}
    summaries: list[str] = []
    segments: list[dict[str, Any]] = []
    for row in segment_rows:
        summary = _segment_summary(day_dir, row["stream"], row["key"])
        summaries.append(summary)
        segments.append(
            {
                "id": row["key"],
                "time_of_day": row["start"],
                "duration_s": _segment_duration(row["key"]),
                "summary": summary,
                "agent_type": row["stream"],
            }
        )
    if scan_keys and not segments:
        logger.warning(
            "voice day lookup saw scan rows but no segments for %s", internal_day
        )
    return {
        "day": external_day,
        "segments": segments,
        "summary": _build_day_summary(summaries),
        "_nav_target": f"today/journal/{external_day}",
    }


def handle_journal_search(payload: dict[str, Any], app: Any) -> dict[str, Any]:
    del app
    query = _clean_optional_str(payload.get("query"))
    if query is None:
        return {"error": "query is required"}
    facet = _clean_optional_str(payload.get("facet"))
    try:
        days = payload.get("days")
        days_value = (
            _as_int(days, default=30, field_name="days") if days is not None else None
        )
        limit = _as_int(payload.get("limit"), default=10, field_name="limit")
    except ValueError as exc:
        return {"error": str(exc)}
    day_from = None
    if days_value is not None:
        day_from = (_today() - timedelta(days=days_value)).strftime("%Y%m%d")
    count, rows = search_journal(query, limit=limit, facet=facet, day_from=day_from)
    results: list[dict[str, Any]] = []
    for row in rows:
        metadata = row.get("metadata", {})
        day_value = metadata.get("day", "")
        result = {
            "id": row.get("id", ""),
            "day": _format_day_external(day_value) if day_value else "",
            "source": metadata.get("agent") or metadata.get("path") or "journal",
            "snippet": _truncate_snippet(str(row.get("text", "")).strip()),
        }
        entity_slug = _extract_entity_slug(metadata.get("path"))
        if entity_slug:
            result["entity_slug"] = entity_slug
        results.append(result)
    output = {"results": results, "count": count}
    if query:
        output["_nav_target"] = f"today/search?q={quote_plus(query)}"
    return output


def handle_entities_get(payload: dict[str, Any], app: Any) -> dict[str, Any]:
    del app
    try:
        slug = _clean_optional_str(payload.get("entity_slug"))
        if slug is None:
            raise ValueError("entity_slug is required")
    except ValueError as exc:
        return {"error": str(exc)}
    profile, journal_entity = _resolve_entity(slug)
    if profile is None and journal_entity is None:
        return {"error": "not found"}

    if profile is None:
        facets_config = get_facets()
        name = str(journal_entity.get("name") or slug)
        facet_relationships, _, _ = _build_facet_relationships(
            slug, name, facets_config
        )
        profile_text = "\n".join(
            [
                f"# {name}",
                "",
                f"- Type: {journal_entity.get('type', '')}",
                (
                    "- Facets: " + ", ".join(rel["name"] for rel in facet_relationships)
                    if facet_relationships
                    else "- Facets: "
                ),
            ]
        ).strip()
        recent_context: list[dict[str, str]] = []
        entity_name = name
        entity_type = str(journal_entity.get("type") or "")
    else:
        profile_text = _build_profile_markdown(profile)
        recent_context = _recent_context_from_profile(profile)
        entity_name = profile.name
        entity_type = profile.type
    return {
        "slug": slug,
        "name": entity_name,
        "type": entity_type,
        "profile": profile_text,
        "tags": _tag_values(profile, journal_entity),
        "recent_context": recent_context,
        "_nav_target": f"entity/{slug}",
    }


def handle_entities_recent_with(payload: dict[str, Any], app: Any) -> dict[str, Any]:
    del app
    try:
        slug = _clean_optional_str(payload.get("entity_slug"))
        if slug is None:
            raise ValueError("entity_slug is required")
        days = _as_int(payload.get("days"), default=7, field_name="days")
        facet = _clean_optional_str(payload.get("facet"))
    except ValueError as exc:
        return {"error": str(exc)}
    profile, journal_entity = _resolve_entity(slug)
    if profile is None and journal_entity is None:
        return {"error": "not found"}

    names = _matching_names(profile, journal_entity)
    interactions: list[tuple[int, dict[str, str]]] = []
    for facet_name in _list_facets(facet):
        for day_value in _iter_days(days):
            for row in load_activity_records(facet_name, day_value):
                matches = False
                for entry in row.get("participation", []) or []:
                    if not isinstance(entry, dict):
                        continue
                    if entry.get("entity_id") == slug:
                        matches = True
                        break
                    name = entry.get("name")
                    if isinstance(name, str) and name.strip().casefold() in names:
                        matches = True
                        break
                if not matches:
                    continue
                story = row.get("story")
                story_body = ""
                if isinstance(story, dict):
                    body = story.get("body")
                    if isinstance(body, str):
                        story_body = body.strip()
                created_at = int(row.get("created_at", 0) or 0)
                interactions.append(
                    (
                        created_at,
                        {
                            "date": _format_day_external(day_value),
                            "activity": str(
                                row.get("title") or row.get("activity") or ""
                            ),
                            "context": story_body or str(row.get("description") or ""),
                            "note": str(row.get("details") or "")
                            or _matching_participation_note(row, names, slug),
                        },
                    )
                )
    interactions.sort(key=lambda item: item[0], reverse=True)
    shaped = [item for _, item in interactions[:10]]
    return {"slug": slug, "interactions": shaped, "count": len(interactions)}


def handle_commitments_list(payload: dict[str, Any], app: Any) -> dict[str, Any]:
    del app
    try:
        state = _clean_optional_str(payload.get("state"))
        if state is not None and state not in VALID_COMMITMENT_STATES:
            raise ValueError("invalid state")
        facet = _clean_optional_str(payload.get("facet"))
        limit = _as_int(payload.get("limit"), default=20, field_name="limit")
    except ValueError as exc:
        return {"error": str(exc)}
    kwargs: dict[str, Any] = {"top": limit}
    if state is not None:
        kwargs["state"] = state
    if facet:
        kwargs["facets"] = [facet]
    items = ledger_surface.list(**kwargs)
    return {"commitments": [_shape_commitment(item) for item in items]}


def handle_commitments_complete(payload: dict[str, Any], app: Any) -> dict[str, Any]:
    del app
    commitment_id = _clean_optional_str(payload.get("commitment_id"))
    resolution = _clean_optional_str(payload.get("resolution"))
    if commitment_id is None:
        return {"error": "commitment_id is required"}
    if resolution is None or resolution not in VALID_RESOLUTIONS:
        return {"error": "invalid resolution"}
    as_state = "dropped" if resolution == "dropped" else "closed"
    note = f"resolution: {resolution}"
    try:
        item = ledger_surface.close(commitment_id, note=note, as_state=as_state)
    except KeyError:
        return {"error": "not found"}
    return {"ok": True, "commitment": _shape_commitment(item, resolution=resolution)}


def handle_calendar_today(payload: dict[str, Any], app: Any) -> dict[str, Any]:
    del app, payload
    try:
        internal_day = _today_internal()
        events: list[dict[str, Any]] = []
        for facet_name in _list_facets(None):
            for row in load_activity_records(facet_name, internal_day):
                if row.get("source") != "anticipated":
                    continue
                attendees: list[str] = []
                for entry in row.get("participation", []) or []:
                    if not isinstance(entry, dict) or entry.get("role") != "attendee":
                        continue
                    name = entry.get("name")
                    if isinstance(name, str) and name.strip():
                        attendees.append(name.strip())
                events.append(
                    {
                        "time": str(row.get("start") or ""),
                        "title": str(row.get("title") or ""),
                        "attendees": attendees,
                        "location": str(row.get("location") or ""),
                        "prep_notes": str(
                            row.get("prep_notes") or row.get("notes") or ""
                        ),
                    }
                )
        events.sort(key=lambda item: item["time"])
        return {
            "date": _today().isoformat(),
            "events": events,
            "_nav_target": "today",
        }
    except Exception:
        logger.exception("voice calendar lookup failed")
        return {"error": "today unavailable"}


def _briefing_text(sections: dict[str, str]) -> str:
    ordered_keys = [
        "your_day",
        "yesterday",
        "needs_attention",
        "forward_look",
        "reading",
    ]
    chunks: list[str] = []
    for key in ordered_keys:
        body = sections.get(key)
        if body:
            chunks.append(_plain_text(body))
    return "\n\n".join(chunk for chunk in chunks if chunk)


def _briefing_highlights(
    sections: dict[str, str], needs_attention_items: list[str]
) -> list[str]:
    if needs_attention_items:
        return needs_attention_items[:3]
    highlights: list[str] = []
    for body in sections.values():
        for line in body.splitlines():
            stripped = line.strip()
            if stripped.startswith("- "):
                highlights.append(_plain_text(stripped[2:]))
            if len(highlights) == 3:
                return highlights
    return highlights


def handle_briefing_get(payload: dict[str, Any], app: Any) -> dict[str, Any]:
    del payload, app
    internal_day = _today_internal()
    sections, metadata, needs_attention_items = _load_briefing_md(internal_day)
    if not metadata or str(metadata.get("date")) != internal_day:
        return {"error": "no briefing today yet"}
    return {
        "date": _format_day_external(internal_day),
        "facet": "identity",
        "text": _briefing_text(sections),
        "highlights": _briefing_highlights(sections, needs_attention_items),
        "_nav_target": "today",
    }


def handle_observer_start_listening(
    payload: dict[str, Any], app: Any
) -> dict[str, Any]:
    del app
    mode = _clean_optional_str(payload.get("mode"))
    if mode is None or mode not in VALID_LISTEN_MODES:
        return {"error": "invalid mode"}
    logger.info("voice observer listen request queued mode=%s", mode)
    return {
        "status": "requested",
        "mode": mode,
        "note": "sol will start listening shortly",
        "_observer_action": {"type": "start_observer", "mode": mode},
    }


TOOL_HANDLERS: dict[str, Callable[[dict[str, Any], Any], dict[str, Any]]] = {
    "journal.get_day": handle_journal_get_day,
    "journal.search": handle_journal_search,
    "entities.get": handle_entities_get,
    "entities.recent_with": handle_entities_recent_with,
    "commitments.list": handle_commitments_list,
    "commitments.complete": handle_commitments_complete,
    "calendar.today": handle_calendar_today,
    "briefing.get": handle_briefing_get,
    "observer.start_listening": handle_observer_start_listening,
}


async def dispatch_tool_call(
    name: str,
    arguments: str,
    call_id: str,
    app: Any,
) -> str:
    handler = TOOL_HANDLERS.get(name)
    if handler is None:
        return json.dumps({"error": f"unknown tool: {name}"})
    try:
        parsed = json.loads(arguments or "{}")
    except json.JSONDecodeError:
        return json.dumps({"error": "tool arguments must be valid JSON"})
    if not isinstance(parsed, dict):
        return json.dumps({"error": "tool arguments must decode to an object"})
    try:
        result = handler(parsed, app)
    except Exception:
        logger.exception("voice tool failed: %s", name)
        result = {"error": "tool failed"}
    if not isinstance(result, dict):
        result = {"error": "tool failed"}
    nav_target = result.pop("_nav_target", None)
    if isinstance(nav_target, str) and nav_target.strip():
        get_nav_queue().push(call_id, nav_target)
    observer_action = result.pop("_observer_action", None)
    if (
        isinstance(observer_action, dict)
        and observer_action
        and isinstance(call_id, str)
        and call_id.strip()
    ):
        get_observer_queue().push(call_id, observer_action)
    return json.dumps(result)


__all__ = [
    "TOOL_MANIFEST",
    "dispatch_tool_call",
    "get_tool_manifest",
]
