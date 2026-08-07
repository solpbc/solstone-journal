# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Context assembly for entity_observer generate agent.

Pre-computes the observation context that the cogitate version built
through sequential tool calls. Used by the entity_observer pre-hook
to inject $observer_context into the prompt.
"""

from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import Any

from solstone.think.entities.core import entity_slug
from solstone.think.entities.loading import load_entities
from solstone.think.entities.matching import find_matching_entity
from solstone.think.entities.observations import load_observations
from solstone.think.indexer.journal import search_journal
from solstone.think.utils import get_journal

logger = logging.getLogger(__name__)

MAX_OBSERVER_ENTITIES = 6
TOP_SEGMENTS_PER_ENTITY = 3
TRANSCRIPT_LINES_PER_SEGMENT = 12
TRANSCRIPT_LINES_FLOOR = 4
TRANSCRIPT_LINES_STEP = 2
MAX_SEARCH_SNIPPETS = 4
PER_ENTITY_CHAR_BUDGET = 6000
TOTAL_CHAR_BUDGET = 24000
NOISE_AGENTS = {
    "action",
    "chat",
    "news",
    "decisions",
    "followups",
    "morning_briefing",
    "_todos_todo",
}
THIN_SOURCE_MARKER = (
    "Thin source: no fresh sense, transcript, or search evidence available."
)
SEGMENT_SOURCE_UNAVAILABLE = "(segment {composite}: source unavailable)"


def _active_entity_ids(facet: str, day: str, attached: list[dict]) -> set[str]:
    active_ids: set[str] = set()

    for detected in load_entities(facet, day):
        match = find_matching_entity(detected.get("name", ""), attached)
        if match:
            entity_id = match.get("id")
            if entity_id:
                active_ids.add(entity_id)

    return active_ids


def _format_observations(facet: str, entity_id: str) -> list[str]:
    try:
        observations = load_observations(facet, entity_id)
    except Exception as exc:
        logger.warning(
            "entity_observer: could not load observations for %s/%s: %s",
            facet,
            entity_id,
            exc,
        )
        return ["No current observations."]
    if not observations:
        return ["No current observations."]

    lines = []
    for index, observation in enumerate(observations):
        content = str(observation.get("content", "")).strip()
        if not content:
            continue
        source_day = observation.get("source_day") or "unknown"
        lines.append(f"{index}. {content} (source: {source_day})")

    return lines or ["No current observations."]


def _detection_rows_by_entity_id(
    facet: str, day: str, attached: list[dict]
) -> dict[str, dict]:
    rows: dict[str, dict] = {}
    try:
        detected_rows = load_entities(facet, day)
    except Exception as exc:
        logger.warning(
            "entity_observer: could not load detections for %s/%s: %s",
            facet,
            day,
            exc,
        )
        return rows

    for detected in detected_rows:
        match = find_matching_entity(detected.get("name", ""), attached)
        if not match:
            continue
        entity_id = match.get("id")
        if entity_id:
            rows[str(entity_id)] = detected
    return rows


def _selected_segments(detection_row: dict | None) -> list[str]:
    if not detection_row:
        return []
    segments = detection_row.get("segments")
    if not isinstance(segments, list):
        return []
    return sorted(str(segment) for segment in segments if segment)[
        -TOP_SEGMENTS_PER_ENTITY:
    ]


def _segment_dir(composite: str) -> Path:
    return Path(get_journal()) / "chronicle" / composite


def _sense_entity_matches(entity: dict, sense_entity: dict[str, Any]) -> bool:
    entity_name = str(entity.get("name") or "").strip()
    sense_name = str(sense_entity.get("name") or "").strip()
    if not entity_name or not sense_name:
        return False
    return entity_name.casefold() == sense_name.casefold() or entity_slug(
        entity_name
    ) == entity_slug(sense_name)


def _read_sense_context(seg_dir: Path, composite: str, entity: dict) -> list[str]:
    sense_path = seg_dir / "talents" / "sense.json"
    try:
        with open(sense_path, "r", encoding="utf-8") as f:
            sense = json.load(f)
    except FileNotFoundError:
        logger.debug("entity_observer: missing sense.json for %s", composite)
        return []
    except (OSError, json.JSONDecodeError, UnicodeDecodeError) as exc:
        logger.warning(
            "entity_observer: could not read sense.json for %s: %s", composite, exc
        )
        return []

    if not isinstance(sense, dict):
        logger.warning("entity_observer: malformed sense.json for %s", composite)
        return []

    lines: list[str] = []
    activity_summary = str(sense.get("activity_summary") or "").strip()
    entities = sense.get("entities")
    if isinstance(entities, list):
        for item in entities:
            if not isinstance(item, dict) or not _sense_entity_matches(entity, item):
                continue
            context = str(item.get("context") or "").strip()
            if context:
                lines.append(f"- {composite}: {context}")
            break
    if activity_summary:
        lines.append(f"- {composite} activity: {activity_summary}")
    return lines


def _read_transcript_excerpt(
    seg_dir: Path,
    composite: str,
    entity_name: str,
    line_cap: int,
) -> list[str]:
    audio_path = seg_dir / "audio.jsonl"
    try:
        lines = audio_path.read_text(encoding="utf-8").splitlines()
    except FileNotFoundError:
        logger.debug("entity_observer: missing audio.jsonl for %s", composite)
        return []
    except (OSError, UnicodeDecodeError) as exc:
        logger.warning(
            "entity_observer: could not read audio.jsonl for %s: %s", composite, exc
        )
        return []

    records: list[dict[str, Any]] = []
    for line in lines[1:]:
        if not line.strip():
            continue
        try:
            item = json.loads(line)
        except json.JSONDecodeError as exc:
            logger.warning(
                "entity_observer: malformed audio.jsonl row for %s: %s",
                composite,
                exc,
            )
            continue
        if isinstance(item, dict) and str(item.get("text") or "").strip():
            records.append(item)

    if not records:
        return []

    entity_key = entity_name.casefold()
    matching = [
        item
        for item in records
        if entity_key and entity_key in str(item.get("text") or "").casefold()
    ]
    selected = matching[:line_cap] if matching else records[:line_cap]
    transcript_lines = []
    for item in selected:
        start = str(item.get("start") or "").strip()
        text = str(item.get("text") or "").strip()
        prefix = f"{start} " if start else ""
        transcript_lines.append(f"- {composite} {prefix}{text}".strip())
    return transcript_lines


def _path_matches_selected_segment(path: str, segments: set[str]) -> bool:
    if not path:
        return False
    for composite in segments:
        if path == composite or path.startswith(f"{composite}/"):
            return True
    parts = path.split("/")
    if len(parts) >= 3 and "/".join(parts[:3]) in segments:
        return True
    return False


def _search_related_snippets(
    entity_name: str,
    day: str,
    facet: str,
    selected_segments: list[str],
) -> list[str]:
    try:
        _, results = search_journal(
            entity_name,
            day=day,
            facet=facet,
            limit=MAX_SEARCH_SNIPPETS,
            include_total=False,
        )
    except Exception as exc:
        logger.warning(
            "entity_observer: journal search failed for %s: %s", entity_name, exc
        )
        return []

    pulled_segments = set(selected_segments)
    snippets: list[str] = []
    for result in results:
        if not isinstance(result, dict):
            continue
        metadata = result.get("metadata") or {}
        if not isinstance(metadata, dict):
            metadata = {}
        if str(metadata.get("agent") or "") in NOISE_AGENTS:
            continue
        path = str(metadata.get("path") or "")
        if _path_matches_selected_segment(path, pulled_segments):
            continue
        text = str(result.get("text") or "").strip()
        if text:
            snippets.append(f"- {text}")
        if len(snippets) >= MAX_SEARCH_SNIPPETS:
            break
    return snippets


def _transcript_line_cap(active_count: int) -> int:
    return max(
        TRANSCRIPT_LINES_FLOOR,
        TRANSCRIPT_LINES_PER_SEGMENT - (active_count - 1) * TRANSCRIPT_LINES_STEP,
    )


def _build_section_lines(
    mandatory_lines: list[str],
    sense_lines: list[str],
    source_notes: list[str],
    transcript_lines: list[str],
    search_lines: list[str],
    rich_present: bool,
) -> list[str]:
    lines = list(mandatory_lines)
    if sense_lines:
        lines.extend(["", "Sense context:"])
        lines.extend(sense_lines)
    if source_notes:
        lines.extend(["", "Source notes:"])
        lines.extend(source_notes)
    if transcript_lines:
        lines.extend(["", "Transcript excerpts:"])
        lines.extend(transcript_lines)
    if search_lines:
        lines.extend(["", "Related journal evidence:"])
        lines.extend(search_lines)
    if not rich_present:
        lines.extend(["", THIN_SOURCE_MARKER])
    return lines


def _format_entity_section(
    facet: str,
    day: str,
    entity: dict,
    detection_row: dict | None,
    active_count: int,
) -> str:
    entity_id = str(entity.get("id") or "")
    entity_name = str(entity.get("name") or entity_id)
    description = str(entity.get("description") or "")

    mandatory_lines = [
        f"#### {entity_name} ({entity_id})",
        f"- Type: {entity.get('type', '')}",
        f"- Description: {description}",
    ]

    aka_list = entity.get("aka")
    if isinstance(aka_list, list) and aka_list:
        mandatory_lines.append(
            f"- AKA: {', '.join(str(item) for item in aka_list if item)}"
        )

    mandatory_lines.append("")
    mandatory_lines.append("Current observations:")
    mandatory_lines.extend(_format_observations(facet, entity_id))

    selected_segments = _selected_segments(detection_row)
    line_cap = _transcript_line_cap(active_count)
    sense_lines: list[str] = []
    transcript_lines: list[str] = []
    source_notes: list[str] = []
    for composite in selected_segments:
        seg_dir = _segment_dir(composite)
        segment_sense = _read_sense_context(seg_dir, composite, entity)
        segment_transcript = _read_transcript_excerpt(
            seg_dir, composite, entity_name, line_cap
        )
        sense_lines.extend(segment_sense)
        transcript_lines.extend(segment_transcript)
        if not segment_sense and not segment_transcript:
            source_notes.append(SEGMENT_SOURCE_UNAVAILABLE.format(composite=composite))

    search_lines = _search_related_snippets(entity_name, day, facet, selected_segments)
    rich_present = bool(sense_lines or transcript_lines or search_lines)

    section_lines = _build_section_lines(
        mandatory_lines,
        sense_lines,
        source_notes,
        transcript_lines,
        search_lines,
        rich_present,
    )
    for lever in ("transcript", "search"):
        if len("\n".join(section_lines)) <= PER_ENTITY_CHAR_BUDGET:
            break
        if lever == "transcript" and transcript_lines:
            transcript_lines = []
        elif lever == "search" and search_lines:
            search_lines = []
        else:
            continue
        section_lines = _build_section_lines(
            mandatory_lines,
            sense_lines,
            source_notes,
            transcript_lines,
            search_lines,
            rich_present,
        )

    return "\n".join(section_lines)


def assemble_observer_context(facet: str, day: str) -> str:
    """Assemble structured observation context for a facet/day run."""
    try:
        attached = load_entities(facet)
    except Exception as exc:
        logger.warning(
            "entity_observer: could not load attached entities for %s: %s", facet, exc
        )
        return "No active entities found for this day."

    try:
        active_ids = _active_entity_ids(facet, day, attached)
    except Exception as exc:
        logger.warning(
            "entity_observer: could not resolve active entities for %s/%s: %s",
            facet,
            day,
            exc,
        )
        active_ids = set()

    if not active_ids:
        return "No active entities found for this day."

    active_entities = sorted(
        [entity for entity in attached if entity.get("id") in active_ids],
        key=lambda item: str(item.get("id") or ""),
    )
    deep_entities = active_entities[:MAX_OBSERVER_ENTITIES]
    omitted_count = max(0, len(active_entities) - len(deep_entities))
    detection_rows = _detection_rows_by_entity_id(facet, day, attached)
    active_count = len(deep_entities)

    lines = [
        "# Entity Observer Context",
        "",
        f"## Facet: {facet}",
        f"## Day: {day}",
        f"## Active Entities: {len(deep_entities)} of {len(active_entities)} active",
        "",
        "### Entities",
        "",
    ]
    if omitted_count:
        lines.append(
            f"{omitted_count} additional active entities omitted for budget this run."
        )
        lines.append("")

    for index, entity in enumerate(deep_entities):
        if index:
            lines.extend(["", "---", ""])
        entity_id = str(entity.get("id") or "")
        lines.append(
            _format_entity_section(
                facet,
                day,
                entity,
                detection_rows.get(entity_id),
                active_count,
            )
        )

    return "\n".join(lines)[:TOTAL_CHAR_BUDGET]
