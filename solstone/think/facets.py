# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Facet-specific utilities and tooling for the think module."""

import json
import logging
import os
import re
import shutil
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

from solstone.think.entities import get_identity_names
from solstone.think.utils import DATE_RE, day_path, get_journal, iter_segments


def _get_principal_display_name() -> str | None:
    """Get the display name for the principal from identity config.

    Returns the first identity name (preferred if set, else full name).
    Returns None if identity is not configured.
    """
    names = get_identity_names()
    return names[0] if names else None


def _format_principal_role(
    entities: list[dict[str, Any]],
) -> tuple[str | None, list[dict[str, Any]]]:
    """Extract principal entity and format role line if all info available.

    Args:
        entities: List of entity dicts from load_entities()

    Returns:
        Tuple of (role_line, filtered_entities) where:
        - role_line is markdown like "**Jer's Role**: Description" or None if incomplete
        - filtered_entities excludes the principal entity
    """
    # Find principal entity
    principal = None
    other_entities = []
    for entity in entities:
        if entity.get("is_principal"):
            principal = entity
        else:
            other_entities.append(entity)

    if not principal:
        return None, entities

    # Get display name and description
    display_name = _get_principal_display_name()
    description = principal.get("description", "").strip()

    # Only format if we have both name and description
    if not display_name or not description:
        return None, entities

    role_line = f"**{display_name}'s Role**: {description}"
    return role_line, other_entities


def _format_entity_name_with_aka(entity: dict[str, Any]) -> str:
    """Format entity name, appending aka values in parentheses if present.

    Args:
        entity: Entity dict with 'name' and optional 'aka' list

    Returns:
        Formatted name string, e.g. "John Smith" or "John Smith (JS, Johnny)"
    """
    name = entity.get("name", "")
    aka_list = entity.get("aka", [])
    if isinstance(aka_list, list) and aka_list:
        aka_str = ", ".join(aka_list)
        return f"{name} ({aka_str})"
    return name


def _format_activity_line(activity: dict[str, Any], *, bold_name: bool = False) -> str:
    """Format a single activity as a markdown list item.

    Args:
        activity: Activity dict with 'name'/'id', 'description', 'priority'
        bold_name: If True, wraps name in **bold**

    Returns:
        Formatted string like "**Meetings** (high): Description" or "Meetings (high): Description"
    """
    name = activity.get("name", activity.get("id", ""))
    desc = activity.get("description", "")
    priority = activity.get("priority", "normal")

    # Format with priority tag if non-normal
    if priority == "high":
        priority_suffix = " (high)"
    elif priority == "low":
        priority_suffix = " (low)"
    else:
        priority_suffix = ""

    if bold_name:
        name_part = f"**{name}**{priority_suffix}"
    else:
        name_part = f"{name}{priority_suffix}"

    if desc:
        return f"{name_part}: {desc}"
    return name_part


def _rank_entities_by_signal(
    facet: str,
    entities: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    """Return entities ranked by observation count, recency, and name."""
    from solstone.think.entities import load_observations

    ranked_items: list[tuple[int, str, str, dict[str, Any]]] = []
    for entity in entities:
        name = entity.get("name", "")
        observations = load_observations(facet, name)
        observation_count = len(observations)
        last_observed = max(
            (
                observation.get("observed_at")
                for observation in observations
                if observation.get("observed_at")
            ),
            default=None,
        )
        last_observed_sort = "" if last_observed is None else str(last_observed)
        ranked_items.append(
            (
                observation_count,
                last_observed_sort,
                name.casefold(),
                entity,
            )
        )

    ranked_items.sort(key=lambda item: item[2])
    ranked_items.sort(key=lambda item: item[1], reverse=True)
    ranked_items.sort(key=lambda item: item[0], reverse=True)
    return [entity for _count, _last_observed, _name, entity in ranked_items]


def _write_action_log(
    facet: str | None,
    action: str,
    params: dict[str, Any],
    source: str,
    actor: str,
    day: str | None = None,
    use_id: str | None = None,
) -> None:
    """Write action to the daily audit log.

    Internal function that writes JSONL log entries. When facet is provided,
    writes to facets/{facet}/logs/{day}.jsonl. When facet is None, writes to
    config/actions/{day}.jsonl for journal-level actions.

    Use log_call_action() for CLI call commands or log_app_action() for web apps.

    Args:
        facet: Facet name where the action occurred, or None for journal-level
        action: Action type (e.g., "todo_add", "entity_attach")
        params: Dictionary of action-specific parameters
        source: Origin type - "tool" for agents, "app" for web UI
        actor: For tools: agent name. For apps: app name
        day: Day in YYYYMMDD format (defaults to today)
        use_id: Optional agent ID (only for tool actions)
    """
    journal = get_journal()

    if day is None:
        day = datetime.now().strftime("%Y%m%d")

    # Build log file path based on whether facet is provided
    if facet is not None:
        log_path = Path(journal) / "facets" / facet / "logs" / f"{day}.jsonl"
    else:
        log_path = Path(journal) / "config" / "actions" / f"{day}.jsonl"

    # Ensure parent directory exists
    log_path.parent.mkdir(parents=True, exist_ok=True)

    # Create log entry
    entry = {
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "source": source,
        "actor": actor,
        "action": action,
        "params": params,
    }

    # Add facet only if provided
    if facet is not None:
        entry["facet"] = facet

    # Add use_id only if available
    if use_id is not None:
        entry["use_id"] = use_id

    # Append to log file
    with open(log_path, "a", encoding="utf-8") as f:
        f.write(json.dumps(entry, ensure_ascii=False) + "\n")


def log_call_action(
    facet: str | None,
    action: str,
    params: dict[str, Any],
    *,
    day: str | None = None,
) -> None:
    """Log an action from a ``sol call`` CLI command.

    Creates a JSONL log entry for tracking successful modifications made via
    ``sol call`` subcommands (entities, todos, etc.).

    When facet is provided, writes to facets/{facet}/logs/{day}.jsonl.
    When facet is None, writes to config/actions/{day}.jsonl for journal-level
    actions (settings changes, system operations, etc.).

    Args:
        facet: Facet name where the action occurred, or None for journal-level
        action: Action type (e.g., "todo_add", "entity_attach")
        params: Dictionary of action-specific parameters
        day: Day in YYYYMMDD format (defaults to today)
    """
    _write_action_log(
        facet=facet,
        action=action,
        params=params,
        source="call",
        actor="agent",
        day=day,
    )


def get_facets() -> dict[str, dict[str, object]]:
    """Return available facets with metadata.

    Each key is the facet name. The value contains the facet metadata
    from facet.json including title, description, and the facet path.
    """
    facets_dir = Path(get_journal()) / "facets"
    facets: dict[str, dict[str, object]] = {}

    if not facets_dir.exists():
        return facets

    for facet_path in sorted(facets_dir.iterdir()):
        if not facet_path.is_dir():
            continue

        facet_name = facet_path.name
        facet_json = facet_path / "facet.json"

        if not facet_json.exists():
            continue

        try:
            with open(facet_json, "r", encoding="utf-8") as f:
                facet_data = json.load(f)

            if isinstance(facet_data, dict):
                facet_info = {
                    "path": str(facet_path),
                    "title": facet_data.get("title", facet_name),
                    "description": facet_data.get("description", ""),
                    "color": facet_data.get("color", ""),
                    "emoji": facet_data.get("emoji", ""),
                    "muted": facet_data.get("muted", False),
                }

                facets[facet_name] = facet_info
        except (
            OSError,
            json.JSONDecodeError,
        ) as exc:  # pragma: no cover - metadata optional
            logging.warning("Failed to read facet metadata %s: %s", facet_json, exc)

    return facets


def get_enabled_facets() -> dict[str, dict[str, object]]:
    """Return non-muted facets only.

    Convenience wrapper around get_facets() that filters out muted facets.
    Used by scheduled agents to skip processing for muted facets.
    """
    return {k: v for k, v in get_facets().items() if not v.get("muted", False)}


def facet_summary(facet: str, *, detailed: bool = True) -> str:
    """Generate a nicely formatted markdown summary of a facet.

    Args:
        facet: The facet name to summarize
        detailed: If True (default), include full descriptions for entities
            and activities. If False, show names only.

    Returns:
        Formatted markdown string with facet title, description, entities,
        and activities

    Raises:
        FileNotFoundError: If the facet doesn't exist
    """
    from solstone.think.activities import get_facet_activities
    from solstone.think.entities import load_entities

    facet_path = Path(get_journal()) / "facets" / facet
    if not facet_path.exists():
        raise FileNotFoundError(f"Facet '{facet}' not found at {facet_path}")

    # Load facet metadata
    facet_json_path = facet_path / "facet.json"
    if not facet_json_path.exists():
        raise FileNotFoundError(f"facet.json not found for facet '{facet}'")

    with open(facet_json_path, "r", encoding="utf-8") as f:
        facet_data = json.load(f)

    # Extract metadata
    title = facet_data.get("title", facet)
    description = facet_data.get("description", "")
    color = facet_data.get("color", "")

    # Build markdown summary
    lines = []

    # Title without emoji
    lines.append(f"# {title}")

    # Add color as a badge if available
    if color:
        lines.append(f"![Color]({color})")
        lines.append("")

    # Description
    if description:
        lines.append(f"**Description:** {description}")
        lines.append("")

    # Load entities if available
    entities = load_entities(facet)
    if entities:
        # Extract principal role line and filter principal from list
        role_line, display_entities = _format_principal_role(entities)

        if role_line:
            lines.append(role_line)
            lines.append("")

        if display_entities:
            if detailed:
                lines.append("## Entities")
                lines.append("")
                for entity in display_entities:
                    entity_type = entity.get("type", "")
                    formatted_name = _format_entity_name_with_aka(entity)
                    desc = entity.get("description", "")

                    if desc:
                        lines.append(f"- **{entity_type}**: {formatted_name} - {desc}")
                    else:
                        lines.append(f"- **{entity_type}**: {formatted_name}")
                lines.append("")
            else:
                # Short mode: names only as semicolon-separated list
                entity_names = "; ".join(
                    _format_entity_name_with_aka(e) for e in display_entities
                )
                lines.append(f"**Entities**: {entity_names}")
                lines.append("")

    # Load activities if available
    activities = get_facet_activities(facet)
    if activities:
        if detailed:
            lines.append("## Activities")
            lines.append("")
            for activity in activities:
                lines.append(f"- {_format_activity_line(activity, bold_name=True)}")
            lines.append("")
        else:
            # Short mode: names only as semicolon-separated list
            activity_names = "; ".join(
                a.get("name", a.get("id", "")) for a in activities
            )
            lines.append(f"**Activities**: {activity_names}")
            lines.append("")

    return "\n".join(lines)


def get_facet_news(
    facet: str,
    *,
    cursor: Optional[str] = None,
    limit: int = 1,
    day: Optional[str] = None,
) -> dict[str, Any]:
    """Return facet news entries grouped by day, newest first.

    Parameters
    ----------
    facet:
        Facet name containing the news directory.
    cursor:
        Optional date string (``YYYYMMDD``). When provided, only news files with
        a date strictly earlier than the cursor are returned. This supports
        pagination in the UI where older entries are fetched on demand.
    limit:
        Maximum number of news days to return. Defaults to one day per request.
    day:
        Optional specific day (``YYYYMMDD``) to return. When provided, returns
        only news for that specific day if it exists. Overrides cursor and limit.

    Returns
    -------
    dict[str, Any]
        Dictionary with ``days`` (list of news day payloads), ``next_cursor``
        (date string for subsequent requests) and ``has_more`` boolean flag.
    """
    news_dir = Path(get_journal()) / "facets" / facet / "news"
    if not news_dir.exists():
        return {"days": [], "next_cursor": None, "has_more": False}

    # If specific day requested, check for that file directly
    if day:
        news_path = news_dir / f"{day}.md"
        if news_path.exists() and news_path.is_file():
            selected = [news_path]
        else:
            return {"days": [], "next_cursor": None, "has_more": False}
    else:
        news_files = [
            path
            for path in news_dir.iterdir()
            if path.is_file() and re.fullmatch(r"\d{8}\.md", path.name)
        ]

        # Sort newest first by file name (YYYYMMDD.md)
        news_files.sort(key=lambda p: p.stem, reverse=True)

        if cursor:
            news_files = [path for path in news_files if path.stem < cursor]

        if limit is not None and limit > 0:
            selected = news_files[:limit]
        else:
            selected = news_files

    days: list[dict[str, Any]] = []

    for news_path in selected:
        date_key = news_path.stem

        # Read the raw markdown content
        raw_content = ""
        try:
            raw_content = news_path.read_text(encoding="utf-8")
        except Exception:
            pass

        days.append(
            {
                "date": date_key,
                "raw_content": raw_content,
            }
        )

    # When specific day requested, no pagination
    if day:
        has_more = False
        next_cursor = None
    else:
        has_more = len(news_files) > len(selected)
        next_cursor = selected[-1].stem if has_more and selected else None

    return {"days": days, "next_cursor": next_cursor, "has_more": has_more}


def is_facet_muted(facet: str) -> bool:
    """Check if a facet is currently muted.

    Args:
        facet: Facet name to check

    Returns:
        True if facet is muted, False if unmuted or facet doesn't exist
    """
    facets = get_facets()
    if facet not in facets:
        return False
    return bool(facets[facet].get("muted", False))


def load_segment_facets(day: str, segment: str, stream: str | None = None) -> list[str]:
    """Load facet IDs from a segment's facets.json output.

    Args:
        day: Day in YYYYMMDD format
        segment: Segment key (HHMMSS_LEN format)
        stream: Optional stream name. If None, searches all streams for the segment.

    Returns:
        List of facet ID strings found in the segment's facets.json
    """
    if stream:
        candidates = [day_path(day) / stream / segment / "talents" / "facets.json"]
    else:
        # Search all streams for this segment
        candidates = []
        for _s, seg_key, seg_path in iter_segments(day):
            if seg_key == segment:
                candidates.append(seg_path / "talents" / "facets.json")

    for facets_file in candidates:
        if not facets_file.exists():
            continue

        try:
            content = facets_file.read_text().strip()
            if not content:
                continue

            data = json.loads(content)
            if not isinstance(data, list):
                logging.warning(f"facets.json is not an array: {facets_file}")
                continue

            result = [item.get("facet") for item in data if item.get("facet")]
            if result:
                return result

        except json.JSONDecodeError as e:
            logging.error(f"Failed to parse facets.json for {segment}: {e}")
        except Exception as e:
            logging.error(f"Error reading facets.json for {segment}: {e}")

    logging.debug(f"No facets.json found for segment {segment}")
    return []


def get_active_facets(day: str) -> set[str]:
    """Return facets that had activity on a given day.

    Scans segment-level ``facets.json`` files produced by the facets
    classifier agent during recording.

    Args:
        day: Day in YYYYMMDD format

    Returns:
        Set of facet names that appeared in at least one segment's facets.json
    """
    active: set[str] = set()

    for stream_name, seg_key, seg_path in iter_segments(day):
        active.update(load_segment_facets(day, seg_key, stream=stream_name))

    return active


def aggregate_speculative_facets(days: list[str] | None = None) -> list[dict]:
    """Aggregate speculative facet outputs from segment classifiers across days.

    Scans per-segment agents/facets.json files produced by the facets classifier
    and counts facet name frequency. Useful during onboarding to suggest journal
    organization to the user.

    Args:
        days: Optional list of days in YYYYMMDD format. If None, scans all days.

    Returns:
        List of dicts with keys:
            - "facet": facet name (str)
            - "count": number of segments where this facet appeared (int)
            - "sample_activities": up to 3 activity descriptions for this facet (list[str])
        Sorted by count descending, capped at 8 entries.
    """
    journal_path = Path(get_journal())

    if days is not None:
        scan_days = days
    else:
        scan_days = []
        if journal_path.exists():
            for entry in sorted(journal_path.iterdir()):
                if entry.is_dir() and DATE_RE.fullmatch(entry.name):
                    scan_days.append(entry.name)

    facet_counts: dict[str, int] = {}
    facet_activities: dict[str, list[str]] = {}

    for day in scan_days:
        for _stream, _seg_key, seg_path in iter_segments(day):
            facets_file = seg_path / "talents" / "facets.json"
            if not facets_file.exists():
                continue

            try:
                content = facets_file.read_text().strip()
                if not content:
                    continue

                data = json.loads(content)
                if not isinstance(data, list):
                    continue

                for item in data:
                    if not isinstance(item, dict):
                        continue
                    facet_name = item.get("facet")
                    if not facet_name:
                        continue
                    facet_counts[facet_name] = facet_counts.get(facet_name, 0) + 1

                    activity = item.get("activity", "")
                    if activity:
                        samples = facet_activities.setdefault(facet_name, [])
                        if len(samples) < 3:
                            samples.append(activity)

            except (json.JSONDecodeError, OSError):
                continue

    result = [
        {
            "facet": facet_name,
            "count": count,
            "sample_activities": facet_activities.get(facet_name, []),
        }
        for facet_name, count in sorted(
            facet_counts.items(), key=lambda x: x[1], reverse=True
        )
    ]
    return result[:8]


def set_facet_muted(facet: str, muted: bool) -> None:
    """Mute or unmute a facet by updating facet.json.

    Creates an audit log entry when the state changes.

    Args:
        facet: Facet name to modify
        muted: True to mute, False to unmute

    Raises:
        FileNotFoundError: If facet doesn't exist
    """
    facet_path = Path(get_journal()) / "facets" / facet
    if not facet_path.exists():
        raise FileNotFoundError(f"Facet '{facet}' not found at {facet_path}")

    facet_json_path = facet_path / "facet.json"
    if not facet_json_path.exists():
        raise FileNotFoundError(f"facet.json not found for facet '{facet}'")

    # Load current config
    with open(facet_json_path, "r", encoding="utf-8") as f:
        facet_data = json.load(f)

    # Check if state is actually changing
    current_state = bool(facet_data.get("muted", False))
    if current_state == muted:
        # No change needed
        return

    # Update muted field
    if muted:
        facet_data["muted"] = True
    else:
        # Remove the field when unmuting (cleaner for default case)
        facet_data.pop("muted", None)

    # Write back atomically
    import tempfile

    temp_fd, temp_path = tempfile.mkstemp(
        dir=facet_json_path.parent, suffix=".json", text=True
    )
    try:
        with os.fdopen(temp_fd, "w", encoding="utf-8") as f:
            json.dump(facet_data, f, indent=2, ensure_ascii=False)
            f.write("\n")
        os.replace(temp_path, facet_json_path)
    except Exception:
        # Clean up temp file on error
        try:
            os.unlink(temp_path)
        except Exception:
            pass
        raise

    # Log the change
    action = "facet_mute" if muted else "facet_unmute"
    log_call_action(
        facet=facet,
        action=action,
        params={"muted": muted},
    )


def create_facet(
    title: str,
    emoji: str = "📦",
    color: str = "#667eea",
    description: str = "",
    *,
    consent: bool = False,
) -> str:
    """Create a new facet directory with facet.json.

    Args:
        title: Display title for the facet
        emoji: Icon emoji (default: "📦")
        color: Hex color (default: "#667eea")
        description: Facet description

    Returns:
        The generated slug name for the facet

    Raises:
        ValueError: If title is empty, slug is invalid, or facet already exists
    """
    title = title.strip()
    if not title:
        raise ValueError("Facet title is required.")

    slug = re.sub(r"[^a-z0-9]+", "-", title.lower()).strip("-")
    if not re.fullmatch(r"[a-z][a-z0-9_-]*", slug):
        raise ValueError(
            f"Invalid facet name '{slug}': must be lowercase, start with a letter, "
            "and contain only letters, digits, hyphens, or underscores"
        )

    if slug in get_facets():
        raise ValueError(f"Facet '{slug}' already exists")

    facet_path = Path(get_journal()) / "facets" / slug
    facet_path.mkdir(parents=True, exist_ok=True)
    facet_json_path = facet_path / "facet.json"

    facet_data = {
        "title": title,
        "description": description,
        "color": color,
        "emoji": emoji,
    }

    import tempfile

    temp_fd, temp_path = tempfile.mkstemp(
        dir=facet_json_path.parent, suffix=".json", text=True
    )
    try:
        with os.fdopen(temp_fd, "w", encoding="utf-8") as f:
            json.dump(facet_data, f, indent=2, ensure_ascii=False)
            f.write("\n")
        os.replace(temp_path, facet_json_path)
    except Exception:
        try:
            os.unlink(temp_path)
        except Exception:
            pass
        raise

    log_params: dict = {
        "title": title,
        "emoji": emoji,
        "color": color,
        "description": description,
    }
    if consent:
        log_params["consent"] = True
    log_call_action(
        facet=slug,
        action="facet_create",
        params=log_params,
    )
    return slug


def update_facet(name: str, **kwargs: Any) -> dict[str, Any]:
    """Update facet.json fields for an existing facet.

    Args:
        name: Facet name
        **kwargs: Fields to update (title, description, emoji, color)

    Returns:
        Dict of changed fields {field: {"old": ..., "new": ...}}

    Raises:
        FileNotFoundError: If facet doesn't exist
        ValueError: If no valid fields provided
    """
    facet_path = Path(get_journal()) / "facets" / name
    if not facet_path.exists():
        raise FileNotFoundError(f"Facet '{name}' not found at {facet_path}")

    facet_json_path = facet_path / "facet.json"
    if facet_json_path.exists():
        with open(facet_json_path, "r", encoding="utf-8") as f:
            facet_data = json.load(f)
    else:
        facet_data = {}

    allowed_fields = {"title", "description", "color", "emoji"}
    changed_fields: dict[str, Any] = {}
    filtered = {k: v for k, v in kwargs.items() if k in allowed_fields}
    if not filtered:
        raise ValueError("No valid fields to update")

    for field, new_value in filtered.items():
        old_value = facet_data.get(field)
        if old_value != new_value:
            changed_fields[field] = {"old": old_value, "new": new_value}
            facet_data[field] = new_value

    import tempfile

    temp_fd, temp_path = tempfile.mkstemp(
        dir=facet_json_path.parent, suffix=".json", text=True
    )
    try:
        with os.fdopen(temp_fd, "w", encoding="utf-8") as f:
            json.dump(facet_data, f, indent=2, ensure_ascii=False)
            f.write("\n")
        os.replace(temp_path, facet_json_path)
    except Exception:
        try:
            os.unlink(temp_path)
        except Exception:
            pass
        raise

    if changed_fields:
        log_call_action(
            facet=name,
            action="facet_update",
            params={"changed_fields": changed_fields},
        )

    return changed_fields


def delete_facet(name: str, *, consent: bool = False) -> None:
    """Delete a facet directory and clean up references.

    Removes the facet directory tree and updates convey.json and chat metadata.

    Args:
        name: Facet name to delete

    Raises:
        FileNotFoundError: If facet doesn't exist
    """
    facet_path = Path(get_journal()) / "facets" / name
    if not facet_path.exists():
        raise FileNotFoundError(f"Facet '{name}' not found at {facet_path}")

    convey_config_path = Path(get_journal()) / "config" / "convey.json"
    if convey_config_path.exists():
        try:
            with open(convey_config_path, "r", encoding="utf-8") as f:
                config = json.load(f)

            changed = False
            facets_config = config.get("facets", {})

            if facets_config.get("selected") == name:
                facets_config["selected"] = ""
                changed = True

            order = facets_config.get("order", [])
            if name in order:
                facets_config["order"] = [item for item in order if item != name]
                changed = True

            if changed:
                config["facets"] = facets_config
                with open(convey_config_path, "w", encoding="utf-8") as f:
                    json.dump(config, f, indent=2, ensure_ascii=False)
                    f.write("\n")
        except (json.JSONDecodeError, OSError):
            pass

    log_params: dict = {"name": name}
    if consent:
        log_params["consent"] = True
    log_call_action(
        facet=None,
        action="facet_delete",
        params=log_params,
    )
    shutil.rmtree(facet_path)


def facet_summaries(
    *,
    detailed: bool = False,
    max_entities_per_facet: int | None = 20,
    max_activities_per_facet: int | None = 15,
) -> str:
    """Generate a formatted list summary of enabled (non-muted) facets for use in agent prompts.

    Returns a markdown-formatted string with each facet as a list item including:
    - Facet name and hashtag ID
    - Description
    - Entity names (if available)
    - Activity names (if available)

    Parameters
    ----------
    detailed:
        If True, includes full entity and activity details (name: description).
        If False (default), includes only names as semicolon-separated lists.
    max_entities_per_facet:
        Maximum entities to render per facet; defaults to 20, or None for no cap.
    max_activities_per_facet:
        Maximum activities to render per facet; defaults to 15, or None for no cap.

    Returns
    -------
    str
        Formatted markdown string with enabled facets, entities, and activities
    """
    from solstone.think.activities import get_facet_activities
    from solstone.think.entities import load_entities

    facets = get_enabled_facets()
    if not facets:
        return "No facets found."

    lines = []
    lines.append("## Available Facets\n")

    for facet_name, facet_info in sorted(facets.items()):
        # Build facet header with name in parentheses
        title = facet_info.get("title", facet_name)
        description = facet_info.get("description", "")

        # Main list item for facet
        lines.append(f"- **{title}** (`{facet_name}`)")

        if description:
            lines.append(f"  {description}")

        # Load entities for this facet
        try:
            entities = load_entities(facet_name)
            if entities:
                role_line, remaining_entities = _format_principal_role(entities)
                ranked_entities = _rank_entities_by_signal(
                    facet_name,
                    remaining_entities,
                )
                if (
                    max_entities_per_facet is not None
                    and len(ranked_entities) > max_entities_per_facet
                ):
                    shown_entities = ranked_entities[:max_entities_per_facet]
                    entity_overflow = len(ranked_entities) - max_entities_per_facet
                else:
                    shown_entities = ranked_entities
                    entity_overflow = 0

                if role_line:
                    lines.append(f"  - {role_line}")

                if shown_entities:
                    if detailed:
                        lines.append(f"  - **{title} Entities**:")
                        for entity in shown_entities:
                            formatted_name = _format_entity_name_with_aka(entity)
                            desc = entity.get("description", "")

                            if desc:
                                lines.append(f"    - {formatted_name}: {desc}")
                            else:
                                lines.append(f"    - {formatted_name}")

                        if entity_overflow:
                            lines.append(f"    - _and {entity_overflow} more entities_")
                    else:
                        if entity_overflow:
                            lines.append(f"  - **{title} Entities**:")
                            for entity in shown_entities:
                                lines.append(f"    - {entity.get('name', '')}")
                            lines.append(f"    - _and {entity_overflow} more entities_")
                        else:
                            entity_names = "; ".join(
                                entity.get("name", "") for entity in shown_entities
                            )
                            lines.append(f"  - **{title} Entities**: {entity_names}")

        except Exception:
            # No entities file or error loading - that's fine, skip it
            pass

        # Load activities for this facet
        try:
            activities = get_facet_activities(facet_name)
            if activities:
                if (
                    max_activities_per_facet is not None
                    and len(activities) > max_activities_per_facet
                ):
                    shown_activities = activities[:max_activities_per_facet]
                    activity_overflow = len(activities) - max_activities_per_facet
                else:
                    shown_activities = activities
                    activity_overflow = 0

                if detailed:
                    lines.append(f"  - **{title} Activities**:")
                    for activity in shown_activities:
                        lines.append(
                            f"    - {_format_activity_line(activity, bold_name=False)}"
                        )
                    if activity_overflow:
                        lines.append(f"    - _and {activity_overflow} more activities_")
                else:
                    if activity_overflow:
                        lines.append(f"  - **{title} Activities**:")
                        for activity in shown_activities:
                            lines.append(
                                f"    - {activity.get('name', activity.get('id', ''))}"
                            )
                        lines.append(f"    - _and {activity_overflow} more activities_")
                    else:
                        activity_names = "; ".join(
                            activity.get("name", activity.get("id", ""))
                            for activity in shown_activities
                        )
                        lines.append(f"  - **{title} Activities**: {activity_names}")
        except Exception:
            # No activities file or error loading - that's fine, skip it
            pass

        lines.append("")  # Empty line between facets

    return "\n".join(lines).strip()


def format_logs(
    entries: list[dict],
    context: dict | None = None,
) -> tuple[list[dict], dict]:
    """Format action log JSONL entries to markdown chunks.

    This is the formatter function used by the formatters registry.
    Handles both facet-scoped logs (facets/{facet}/logs/) and journal-level
    logs (config/actions/).

    Args:
        entries: Raw JSONL entries (one action log per line)
        context: Optional context with:
            - file_path: Path to JSONL file (for extracting facet name and day)

    Returns:
        Tuple of (chunks, meta) where:
            - chunks: List of dicts with keys:
                - timestamp: int (unix ms)
                - markdown: str
                - source: dict (original log entry)
            - meta: Dict with optional "header" and "error" keys
    """
    ctx = context or {}
    file_path = ctx.get("file_path")
    meta: dict[str, Any] = {}
    chunks: list[dict[str, Any]] = []
    skipped_count = 0

    # Extract facet name and day from path
    facet_name: str | None = None
    day_str: str | None = None
    is_journal_level = False

    if file_path:
        file_path = Path(file_path)
        path_str = str(file_path)

        # Check for journal-level logs: config/actions/YYYYMMDD.jsonl
        if "config/actions" in path_str or "config\\actions" in path_str:
            is_journal_level = True
        else:
            # Extract facet name from path: facets/{facet}/logs/YYYYMMDD.jsonl
            facet_match = re.search(r"facets/([^/]+)/logs", path_str)
            if facet_match:
                facet_name = facet_match.group(1)

        # Extract day from filename
        if file_path.stem.isdigit() and len(file_path.stem) == 8:
            day_str = file_path.stem

    # Build header
    if day_str:
        formatted_day = f"{day_str[:4]}-{day_str[4:6]}-{day_str[6:8]}"
        if is_journal_level:
            meta["header"] = f"# Journal Action Log ({formatted_day})"
        elif facet_name:
            meta["header"] = f"# Action Log: {facet_name} ({formatted_day})"
        else:
            meta["header"] = f"# Action Log ({formatted_day})"
    else:
        if is_journal_level:
            meta["header"] = "# Journal Action Log"
        elif facet_name:
            meta["header"] = f"# Action Log: {facet_name}"
        else:
            meta["header"] = "# Action Log"

    # Format each log entry as a chunk
    for entry in entries:
        # Skip entries without action field
        action = entry.get("action")
        if not action:
            skipped_count += 1
            continue

        # Parse timestamp
        ts = 0
        timestamp_str = entry.get("timestamp", "")
        time_display = ""
        if timestamp_str:
            try:
                dt = datetime.fromisoformat(timestamp_str)
                ts = int(dt.timestamp() * 1000)
                time_display = dt.strftime("%H:%M:%S")
            except (ValueError, TypeError):
                pass

        # Extract fields
        source = entry.get("source", "unknown")
        actor = entry.get("actor", "unknown")
        params = entry.get("params", {})
        use_id = entry.get("use_id")

        # Format action name for display (e.g., "todo_add" -> "Todo Add")
        action_display = action.replace("_", " ").title()

        # Build markdown
        lines = [f"### {action_display} by {actor}", ""]

        # Metadata line
        meta_parts = [f"**Source:** {source}"]
        if time_display:
            meta_parts.append(f"**Time:** {time_display}")
        lines.append(" | ".join(meta_parts))

        # Agent link if present
        if use_id:
            lines.append(f"**Talent:** [{use_id}](/app/sol/{use_id})")

        lines.append("")

        # Parameters
        if params and isinstance(params, dict):
            lines.append("**Parameters:**")
            for key, value in params.items():
                # Format value - truncate long strings
                if isinstance(value, str) and len(value) > 100:
                    value = value[:100] + "..."
                lines.append(f"- {key}: {value}")
            lines.append("")

        chunks.append(
            {
                "timestamp": ts,
                "markdown": "\n".join(lines),
                "source": entry,
            }
        )

    # Report skipped entries
    if skipped_count > 0:
        error_msg = f"Skipped {skipped_count} entries missing 'action' field"
        meta["error"] = error_msg
        logging.info(error_msg)

    # Indexer metadata - agent is "action" for action logs
    meta["indexer"] = {"agent": "action"}

    return chunks, meta


def rename_facet(old_name: str, new_name: str) -> None:
    """Rename a facet by updating its directory and config references.

    Performs the following steps:
    1. Rename facets/{old}/ directory to facets/{new}/
    2. Update config/convey.json (facets.selected, facets.order)
    3. Print instruction to rebuild the search index

    Args:
        old_name: Current facet name (must exist)
        new_name: New facet name (must not already exist)

    Raises:
        ValueError: If names are invalid or preconditions fail
    """
    journal = get_journal()
    facets_dir = Path(journal) / "facets"

    # Validate new name format (lowercase alphanumeric + hyphens/underscores)
    if not re.fullmatch(r"[a-z][a-z0-9_-]*", new_name):
        raise ValueError(
            f"Invalid facet name '{new_name}': must be lowercase, start with a letter, "
            "and contain only letters, digits, hyphens, or underscores"
        )

    old_path = facets_dir / old_name
    new_path = facets_dir / new_name

    if not old_path.is_dir():
        raise ValueError(f"Facet '{old_name}' does not exist")
    if new_path.exists():
        raise ValueError(f"Facet '{new_name}' already exists")

    # Step 1: Rename the directory
    print(f"Renaming facets/{old_name}/ → facets/{new_name}/")
    os.rename(old_path, new_path)

    # Step 2: Update config/convey.json
    convey_config_path = Path(journal) / "config" / "convey.json"
    if convey_config_path.exists():
        try:
            with open(convey_config_path, "r", encoding="utf-8") as f:
                config = json.load(f)

            changed = False
            facets_config = config.get("facets", {})

            if facets_config.get("selected") == old_name:
                facets_config["selected"] = new_name
                changed = True

            order = facets_config.get("order", [])
            if old_name in order:
                facets_config["order"] = [
                    new_name if name == old_name else name for name in order
                ]
                changed = True

            if changed:
                config["facets"] = facets_config
                with open(convey_config_path, "w", encoding="utf-8") as f:
                    json.dump(config, f, indent=2, ensure_ascii=False)
                    f.write("\n")
                print("Updated config/convey.json")
            else:
                print("No changes needed in config/convey.json")
        except (json.JSONDecodeError, OSError) as exc:
            logging.warning("Failed to update convey config: %s", exc)

    # Step 3: Advise index rebuild
    print(
        "Facet renamed. Rebuild the search index with: journal indexer --reset --rescan-full"
    )
