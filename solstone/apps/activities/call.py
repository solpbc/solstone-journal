# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""CLI commands for completed activity record management.

Auto-discovered by ``think.call`` and mounted as ``sol call activities ...``.
"""

from __future__ import annotations

import json
import sys
from datetime import datetime, timedelta
from typing import Any

import typer

from solstone.think.activities import (
    append_activity_record,
    append_edit,
    format_activities,
    get_activity_by_id,
    get_activity_record,
    load_activity_records,
    make_activity_id,
    mute_activity_record,
    unmute_activity_record,
    update_activity_record,
)
from solstone.think.entities.loading import load_entities
from solstone.think.entities.matching import find_matching_entity
from solstone.think.facets import get_facets, log_call_action
from solstone.think.utils import (
    get_sol_facet,
    now_ms,
    require_solstone,
    resolve_sol_day,
    resolve_sol_day_or_today,
    resolve_sol_facet,
    segment_parse,
)

_PARTICIPATION_ROLES = {"attendee", "mentioned"}
_PARTICIPATION_SOURCES = {"voice", "speaker_label", "transcript", "screen", "other"}

app = typer.Typer(help="Completed activity record management.")


@app.callback()
def _require_up() -> None:
    require_solstone()


def _read_stdin_json() -> dict[str, Any]:
    """Parse a single JSON object from stdin."""
    raw = sys.stdin.read().strip()
    if not raw:
        typer.echo("Error: expected JSON object on stdin.", err=True)
        raise typer.Exit(1)

    try:
        payload = json.loads(raw)
    except json.JSONDecodeError as exc:
        typer.echo(f"Error: invalid JSON on stdin: {exc}", err=True)
        raise typer.Exit(1) from None

    if not isinstance(payload, dict):
        typer.echo("Error: expected JSON object on stdin.", err=True)
        raise typer.Exit(1)
    return payload


def _echo_records(records: list[dict[str, Any]]) -> None:
    """Render activity records using the formatter text output."""
    chunks, _meta = format_activities(records)
    if not chunks:
        typer.echo("No activities found.")
        return
    typer.echo("\n\n".join(chunk["markdown"] for chunk in chunks))


def _echo_json(payload: Any) -> None:
    typer.echo(json.dumps(payload, indent=2, ensure_ascii=False))


def _parse_day(value: str, *, label: str) -> datetime:
    try:
        return datetime.strptime(value, "%Y%m%d")
    except ValueError:
        typer.echo(f"Error: invalid {label} '{value}'", err=True)
        raise typer.Exit(1) from None


def _iter_days(start_day: str, end_day: str) -> list[str]:
    start = _parse_day(start_day, label="day")
    end = _parse_day(end_day, label="day")
    if end < start:
        typer.echo(
            f"Error: --to ({end_day}) must not be before --from ({start_day})",
            err=True,
        )
        raise typer.Exit(1)

    days: list[str] = []
    cursor = start
    while cursor <= end:
        days.append(cursor.strftime("%Y%m%d"))
        cursor += timedelta(days=1)
    return days


def _resolve_list_facets(facet: str | None) -> list[str]:
    if facet:
        return [facet]

    env_facet = get_sol_facet()
    if env_facet:
        return [env_facet]

    return sorted(get_facets())


def _validate_segment_key(segment: str) -> str:
    start, end = segment_parse(segment)
    if start is None or end is None:
        typer.echo(
            f"Error: invalid --since-segment '{segment}' (expected HHMMSS_LEN)",
            err=True,
        )
        raise typer.Exit(1)
    return segment


def _validate_participation(value: Any) -> list[dict[str, Any]]:
    if not isinstance(value, list):
        typer.echo("Error: participation must be an array", err=True)
        raise typer.Exit(1)

    cleaned_entries: list[dict[str, Any]] = []
    for i, entry in enumerate(value):
        if not isinstance(entry, dict):
            typer.echo(f"Error: participation[{i}] must be an object", err=True)
            raise typer.Exit(1)

        name = entry.get("name")
        if not isinstance(name, str) or not name.strip():
            typer.echo(
                f"Error: participation[{i}] requires a non-empty string 'name'",
                err=True,
            )
            raise typer.Exit(1)

        role = entry.get("role")
        if role not in _PARTICIPATION_ROLES:
            typer.echo(
                f"Error: participation[{i}] has invalid role '{role}' "
                f"(must be one of {sorted(_PARTICIPATION_ROLES)})",
                err=True,
            )
            raise typer.Exit(1)

        source = entry.get("source")
        if source not in _PARTICIPATION_SOURCES:
            typer.echo(
                f"Error: participation[{i}] has invalid source '{source}' "
                f"(must be one of {sorted(_PARTICIPATION_SOURCES)})",
                err=True,
            )
            raise typer.Exit(1)

        confidence = entry.get("confidence")
        if isinstance(confidence, bool) or not isinstance(confidence, (int, float)):
            typer.echo(
                f"Error: participation[{i}] 'confidence' must be a number",
                err=True,
            )
            raise typer.Exit(1)

        context = entry.get("context")
        if not isinstance(context, str):
            typer.echo(
                f"Error: participation[{i}] 'context' must be a string",
                err=True,
            )
            raise typer.Exit(1)

        cleaned_entry = {key: item for key, item in entry.items() if key != "entity_id"}
        cleaned_entry["name"] = name.strip()
        cleaned_entry["role"] = role
        cleaned_entry["source"] = source
        cleaned_entry["confidence"] = confidence
        cleaned_entry["context"] = context
        cleaned_entries.append(cleaned_entry)

    return cleaned_entries


def _resolve_participation_entity_ids(
    entries: list[dict[str, Any]], *, facet: str, day: str
) -> list[dict[str, Any]]:
    entities_list = load_entities(facet=facet, day=day)

    resolved_entries = []
    for entry in entries:
        resolved = dict(entry)
        match = find_matching_entity(resolved["name"], entities_list)
        resolved["entity_id"] = match.get("id") if match else None
        resolved_entries.append(resolved)

    return resolved_entries


def _list_records_for_days(
    facets: list[str],
    days: list[str],
    *,
    activity: str | None,
    entity: str | None,
    source: str | None,
    include_hidden: bool,
) -> list[dict[str, Any]]:
    matches: list[dict[str, Any]] = []
    entity_query = entity.lower() if entity else None
    for facet_name in facets:
        for day in days:
            for record in load_activity_records(
                facet_name, day, include_hidden=include_hidden
            ):
                if activity and record.get("activity") != activity:
                    continue
                if source and record.get("source") != source:
                    continue
                if entity_query:
                    active_entities = record.get("active_entities", [])
                    if not any(
                        entity_query in str(active_entity).lower()
                        for active_entity in active_entities
                    ):
                        continue
                enriched = dict(record)
                enriched["facet"] = facet_name
                enriched["day"] = day
                matches.append(enriched)

    matches.sort(
        key=lambda record: (
            record.get("day", ""),
            record.get("facet", ""),
            int(record.get("created_at", 0) or 0),
            str(record.get("id", "")),
        )
    )
    return matches


@app.command("list")
def list_records(
    day: str | None = typer.Option(
        None,
        "--day",
        "-d",
        help="Journal day in YYYYMMDD format (or set SOL_DAY).",
    ),
    from_day: str | None = typer.Option(
        None,
        "--from",
        help="Start day for an inclusive range query (YYYYMMDD).",
    ),
    to_day: str | None = typer.Option(
        None,
        "--to",
        help="End day for an inclusive range query (YYYYMMDD).",
    ),
    facet: str | None = typer.Option(
        None,
        "--facet",
        "-f",
        help="Facet name (or set SOL_FACET). Omit to query all facets.",
    ),
    activity: str | None = typer.Option(
        None,
        "--activity",
        "-a",
        help="Filter by activity type.",
    ),
    entity: str | None = typer.Option(
        None,
        "--entity",
        help="Filter by active entity.",
    ),
    source: str | None = typer.Option(
        None,
        "--source",
        help="Filter by record source: anticipated, user, or cogitate.",
    ),
    include_all: bool = typer.Option(
        False,
        "--all",
        help="Include hidden activity records.",
    ),
    json_output: bool = typer.Option(False, "--json", help="Output as JSON."),
) -> None:
    """List activity records for one day or an inclusive day range."""
    if day and (from_day or to_day):
        typer.echo("Error: --day is incompatible with --from/--to.", err=True)
        raise typer.Exit(1)

    if day:
        resolved_days = [resolve_sol_day(day)]
    elif from_day or to_day:
        start_day = from_day or resolve_sol_day_or_today(None)
        end_day = to_day or start_day
        resolved_days = _iter_days(start_day, end_day)
    else:
        resolved_days = [resolve_sol_day_or_today(None)]

    if source and source not in {"anticipated", "cogitate", "user"}:
        typer.echo(
            "Error: --source must be 'anticipated', 'cogitate', or 'user'.",
            err=True,
        )
        raise typer.Exit(1)

    facets = _resolve_list_facets(facet)
    records = _list_records_for_days(
        facets,
        resolved_days,
        activity=activity,
        entity=entity,
        source=source,
        include_hidden=include_all,
    )

    if json_output:
        _echo_json(records)
    else:
        _echo_records(records)


@app.command("get")
def get_record(
    span_id: str = typer.Argument(help="Activity record ID."),
    facet: str | None = typer.Option(
        None,
        "--facet",
        "-f",
        help="Facet name (or set SOL_FACET).",
    ),
    day: str | None = typer.Option(
        None,
        "--day",
        "-d",
        help="Journal day in YYYYMMDD format (or set SOL_DAY).",
    ),
    json_output: bool = typer.Option(False, "--json", help="Output as JSON."),
) -> None:
    """Fetch one activity record by ID."""
    resolved_facet = resolve_sol_facet(facet)
    resolved_day = resolve_sol_day(day)
    record = get_activity_record(resolved_facet, resolved_day, span_id)
    if record is None:
        typer.echo(f"activity not found: {span_id}", err=True)
        raise typer.Exit(1)

    if json_output:
        _echo_json(record)
    else:
        _echo_records([record])


@app.command("create")
def create_record(
    facet: str | None = typer.Option(
        None,
        "--facet",
        "-f",
        help="Facet name (or set SOL_FACET).",
    ),
    day: str | None = typer.Option(
        None,
        "--day",
        "-d",
        help="Journal day in YYYYMMDD format (or set SOL_DAY).",
    ),
    since_segment: str | None = typer.Option(
        None,
        "--since-segment",
        help="Segment key to anchor the new activity span (HHMMSS_LEN).",
    ),
    source: str = typer.Option(
        "user",
        "--source",
        help="Record source label: user or cogitate.",
    ),
    json_output: bool = typer.Option(False, "--json", help="Output as JSON."),
) -> None:
    """Create a new synthetic activity record from JSON on stdin."""
    if source not in {"cogitate", "user"}:
        typer.echo("Error: --source must be 'cogitate' or 'user'.", err=True)
        raise typer.Exit(1)

    resolved_facet = resolve_sol_facet(facet)
    resolved_day = resolve_sol_day(day)
    payload = _read_stdin_json()
    participation_provided = "participation" in payload

    title = str(payload.get("title") or "").strip()
    if not title:
        typer.echo("Error: title is required.", err=True)
        raise typer.Exit(1)

    activity_type = str(payload.get("activity") or "").strip()
    if not activity_type:
        typer.echo("Error: activity is required.", err=True)
        raise typer.Exit(1)

    if not get_activity_by_id(resolved_facet, activity_type):
        typer.echo(
            f"Error: unknown activity for facet '{resolved_facet}': {activity_type}",
            err=True,
        )
        raise typer.Exit(1)

    if since_segment is not None:
        anchor = _validate_segment_key(since_segment)
        segments = [anchor]
    else:
        anchor = f"user_{now_ms()}"
        segments = []

    description = str(payload.get("description") or title).strip() or title
    details = str(payload.get("details") or "")
    participation: list[dict[str, Any]] = []
    if participation_provided:
        participation = _validate_participation(payload["participation"])
        participation = _resolve_participation_entity_ids(
            participation, facet=resolved_facet, day=resolved_day
        )

    actor = "cogitate:activities" if source == "cogitate" else "cli:create"
    span_id = make_activity_id(activity_type, anchor)
    record = {
        "id": span_id,
        "activity": activity_type,
        "title": title,
        "description": description,
        "details": details,
        "segments": segments,
        "active_entities": [],
        "created_at": now_ms(),
        "source": source,
        "hidden": False,
        "edits": [],
    }
    if participation_provided:
        record["participation"] = participation

    edit_fields = ["activity", "title", "description", "details", "source"]
    if participation_provided:
        edit_fields.append("participation")

    record = append_edit(
        record,
        actor=actor,
        fields=edit_fields,
        note="created",
    )

    if not append_activity_record(resolved_facet, resolved_day, record):
        typer.echo(f"Error: activity already exists: {span_id}", err=True)
        raise typer.Exit(1)

    log_call_action(
        facet=resolved_facet,
        action="activity_create",
        params={"id": span_id, "activity": activity_type, "source": source},
        day=resolved_day,
    )

    if json_output:
        _echo_json(record)
    else:
        _echo_records([record])


@app.command("update")
def update_record_command(
    span_id: str = typer.Argument(help="Activity record ID."),
    facet: str | None = typer.Option(
        None,
        "--facet",
        "-f",
        help="Facet name (or set SOL_FACET).",
    ),
    day: str | None = typer.Option(
        None,
        "--day",
        "-d",
        help="Journal day in YYYYMMDD format (or set SOL_DAY).",
    ),
    note: str | None = typer.Option(None, "--note", help="Edit note."),
    json_output: bool = typer.Option(False, "--json", help="Output as JSON."),
) -> None:
    """Apply a shallow JSON patch to one activity record."""
    resolved_facet = resolve_sol_facet(facet)
    resolved_day = resolve_sol_day(day)
    payload = _read_stdin_json()

    patch = {
        key: value
        for key, value in payload.items()
        if key in {"title", "description", "details"}
    }
    if set(payload) - set(patch):
        extra = ", ".join(sorted(set(payload) - set(patch)))
        typer.echo(f"Error: disallowed update fields: {extra}", err=True)
        raise typer.Exit(1)

    if not patch:
        typer.echo(
            "Error: update payload must include at least one mutable field.", err=True
        )
        raise typer.Exit(1)

    note_text = note or f"updated fields: {', '.join(sorted(patch))}"
    updated = update_activity_record(
        resolved_facet,
        resolved_day,
        span_id,
        patch,
        actor="cli:update",
        note=note_text,
    )
    if updated is None:
        typer.echo(f"activity not found: {span_id}", err=True)
        raise typer.Exit(1)

    log_call_action(
        facet=resolved_facet,
        action="activity_update",
        params={"id": span_id, "fields": sorted(patch)},
        day=resolved_day,
    )

    if json_output:
        _echo_json(updated)
    else:
        _echo_records([updated])


@app.command("mute")
def mute_record(
    span_id: str = typer.Argument(help="Activity record ID."),
    facet: str | None = typer.Option(
        None,
        "--facet",
        "-f",
        help="Facet name (or set SOL_FACET).",
    ),
    day: str | None = typer.Option(
        None,
        "--day",
        "-d",
        help="Journal day in YYYYMMDD format (or set SOL_DAY).",
    ),
    reason: str | None = typer.Option(None, "--reason", help="Mute reason."),
    json_output: bool = typer.Option(False, "--json", help="Output as JSON."),
) -> None:
    """Hide an activity record without deleting it."""
    resolved_facet = resolve_sol_facet(facet)
    resolved_day = resolve_sol_day(day)
    record = mute_activity_record(
        resolved_facet,
        resolved_day,
        span_id,
        actor="cli:mute",
        reason=reason,
    )
    if record is None:
        typer.echo(f"activity not found: {span_id}", err=True)
        raise typer.Exit(1)

    log_call_action(
        facet=resolved_facet,
        action="activity_mute",
        params={"id": span_id, "reason": reason},
        day=resolved_day,
    )

    if json_output:
        _echo_json(record)
    else:
        _echo_records([record])


@app.command("unmute")
def unmute_record(
    span_id: str = typer.Argument(help="Activity record ID."),
    facet: str | None = typer.Option(
        None,
        "--facet",
        "-f",
        help="Facet name (or set SOL_FACET).",
    ),
    day: str | None = typer.Option(
        None,
        "--day",
        "-d",
        help="Journal day in YYYYMMDD format (or set SOL_DAY).",
    ),
    reason: str | None = typer.Option(None, "--reason", help="Unmute reason."),
    json_output: bool = typer.Option(False, "--json", help="Output as JSON."),
) -> None:
    """Restore a previously hidden activity record."""
    resolved_facet = resolve_sol_facet(facet)
    resolved_day = resolve_sol_day(day)
    record = unmute_activity_record(
        resolved_facet,
        resolved_day,
        span_id,
        actor="cli:unmute",
        reason=reason,
    )
    if record is None:
        typer.echo(f"activity not found: {span_id}", err=True)
        raise typer.Exit(1)

    log_call_action(
        facet=resolved_facet,
        action="activity_unmute",
        params={"id": span_id, "reason": reason},
        day=resolved_day,
    )

    if json_output:
        _echo_json(record)
    else:
        _echo_records([record])
