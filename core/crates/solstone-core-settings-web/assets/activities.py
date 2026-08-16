# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Activities system for tracking common activity types per facet.

Activities provide a consistent vocabulary for tagging time segments,
screen observations, and extracted events across the journal.

Also provides utilities for activity records — completed activity spans
stored as facets/{facet}/activities/{day}.jsonl.
"""

import copy
import difflib
import json
import logging
import os
import re
from datetime import UTC, datetime
from itertools import combinations
from pathlib import Path
from typing import Any

from solstone.think.edge_sources import EdgeContext
from solstone.think.journal_io import atomic_replace, contained_path, hold_lock
from solstone.think.utils import get_journal, segment_parse

logger = logging.getLogger(__name__)
ANTICIPATION_FUZZY_THRESHOLD = 0.85
LUCIDE_ICON_NAME_RE = re.compile(r"^[a-z0-9-]+$")

# ---------------------------------------------------------------------------
# Default Activities
#
# These are predefined common activities that users can attach to facets.
# They serve as a starting vocabulary - facets must explicitly attach them.
# ---------------------------------------------------------------------------

DEFAULT_ACTIVITIES: list[dict[str, str]] = [
    {
        "id": "meeting",
        "name": "Meetings",
        "description": "Video calls, in-person meetings, and conferences",
        "emoji": "📅",
        "icon": "users",
        "always_on": True,
        "instructions": (
            "Levels: high=actively speaking/presenting, medium=listening attentively,"
            " low=muted or multitasking during call."
            " Detect via: video call UI, multiple speakers, calendar event visible."
        ),
    },
    {
        "id": "call",
        "name": "call",
        "description": "a call you have planned",
        "emoji": "📞",
        "icon": "phone",
        "instructions": "Scheduled events emitted by talent/schedule.md; not detected from sense data.",
    },
    {
        "id": "deadline",
        "name": "deadline",
        "description": "a deadline you are working toward",
        "emoji": "⏰",
        "icon": "alarm-clock",
        "instructions": "Scheduled events emitted by talent/schedule.md; not detected from sense data.",
    },
    {
        "id": "appointment",
        "name": "appointment",
        "description": "an appointment on your calendar",
        "emoji": "📌",
        "icon": "pin",
        "instructions": "Scheduled events emitted by talent/schedule.md; not detected from sense data.",
    },
    {
        "id": "event",
        "name": "event",
        "description": "an event you plan to attend",
        "emoji": "🎟️",
        "icon": "ticket",
        "instructions": "Scheduled events emitted by talent/schedule.md; not detected from sense data.",
    },
    {
        "id": "travel",
        "name": "travel",
        "description": "travel you have planned",
        "emoji": "✈️",
        "icon": "plane",
        "instructions": "Scheduled events emitted by talent/schedule.md; not detected from sense data.",
    },
    {
        "id": "reminder",
        "name": "reminder",
        "description": "a reminder for something upcoming",
        "emoji": "🔔",
        "icon": "bell",
        "instructions": "Scheduled events emitted by talent/schedule.md; not detected from sense data.",
    },
    {
        "id": "errand",
        "name": "errand",
        "description": "an errand you plan to do",
        "emoji": "🧾",
        "icon": "receipt",
        "instructions": "Scheduled events emitted by talent/schedule.md; not detected from sense data.",
    },
    {
        "id": "celebration",
        "name": "celebration",
        "description": "a celebration on the calendar",
        "emoji": "🎉",
        "icon": "party-popper",
        "instructions": "Scheduled events emitted by talent/schedule.md; not detected from sense data.",
    },
    {
        "id": "doctor_appointment",
        "name": "doctor appointment",
        "description": "a medical appointment on your calendar",
        "emoji": "🩺",
        "icon": "stethoscope",
        "instructions": "Scheduled events emitted by talent/schedule.md; not detected from sense data.",
    },
    {
        "id": "coding",
        "name": "Coding",
        "description": "Programming, code review, and debugging",
        "emoji": "💻",
        "icon": "code-xml",
        "instructions": (
            "Levels: high=writing or debugging code, medium=reading/reviewing code,"
            " low=IDE or editor open but not focused."
            " Detect via: editors, terminals with dev tools, AI coding assistants,"
            " git operations. Includes focused code reading and thinking."
        ),
    },
    {
        "id": "browsing",
        "name": "Browsing",
        "description": "Web browsing, research, and reading online",
        "emoji": "🌐",
        "icon": "globe",
        "instructions": (
            "Levels: high=actively navigating/researching, medium=reading a page,"
            " low=browser open but idle."
            " Detect via: browser tabs, URL changes, search queries."
        ),
    },
    {
        "id": "email",
        "name": "Email",
        "description": "Email reading and composition",
        "emoji": "📧",
        "icon": "mail",
        "always_on": True,
        "instructions": (
            "Levels: high=composing or actively reading email,"
            " medium=scanning inbox, low=email client visible but idle."
            " Detect via: email client UI, inbox view, compose window."
        ),
    },
    {
        "id": "messaging",
        "name": "Messaging",
        "description": "Chat, Slack, Discord, and text messaging",
        "emoji": "💬",
        "icon": "messages-square",
        "always_on": True,
        "instructions": (
            "Levels: high=active conversation, medium=reading messages,"
            " low=chat app visible but idle."
            " Detect via: chat app UI, message notifications, typing indicators."
        ),
    },
    {
        "id": "ai_conversation",
        "name": "AI Conversation",
        "description": "Conversations with AI assistants like ChatGPT, Claude, and Gemini",
        "emoji": "🤖",
        "icon": "bot",
        "instructions": (
            "Levels: high=actively prompting and reading responses,"
            " medium=reviewing AI output or refining prompts,"
            " low=AI chat open but idle."
            " Detect via: AI assistant interfaces (ChatGPT, Claude, Gemini),"
            " imported AI conversation transcripts, prompt-response patterns."
            " Do not confuse with messaging — AI conversation involves"
            " a human interacting with an AI model, not person-to-person chat."
        ),
    },
    {
        "id": "writing",
        "name": "Writing",
        "description": "Documents, notes, and long-form writing",
        "emoji": "✍️",
        "icon": "pencil-line",
        "instructions": (
            "Levels: high=actively composing text, medium=editing/revising,"
            " low=document open but not being edited."
            " Detect via: document editors, note apps, text content changing."
        ),
    },
    {
        "id": "reading",
        "name": "Reading",
        "description": "Books, PDFs, articles, highlights, and documentation",
        "emoji": "📖",
        "icon": "book-open",
        "instructions": (
            "Levels: high=focused reading, medium=skimming content,"
            " low=document open but attention elsewhere."
            " Detect via: PDF viewers, article pages, documentation sites,"
            " reading apps, imported book highlights and annotations."
            " Do not use for reading code — that is coding."
        ),
    },
    {
        "id": "video",
        "name": "Video",
        "description": "Watching videos and streaming content",
        "emoji": "🎬",
        "icon": "clapperboard",
        "instructions": (
            "Levels: high=actively watching, medium=video playing while"
            " doing something else, low=video paused or minimized."
            " Detect via: video player UI, streaming sites, playback controls."
        ),
    },
    {
        "id": "gaming",
        "name": "Gaming",
        "description": "Games and entertainment",
        "emoji": "🎮",
        "icon": "gamepad-2",
        "instructions": (
            "Levels: high=actively playing, medium=in menus or waiting,"
            " low=game open but tabbed out."
            " Detect via: game window, controller input, game UI elements."
        ),
    },
    {
        "id": "social",
        "name": "Social Media",
        "description": "Social media browsing and interaction",
        "emoji": "📱",
        "icon": "share-2",
        "instructions": (
            "Levels: high=posting or actively engaging, medium=scrolling feed,"
            " low=social app open but idle."
            " Detect via: social media sites/apps, feed content, post composition."
        ),
    },
    {
        "id": "planning",
        "name": "Planning",
        "description": "Scheduling, calendar management, meeting preparation, and agenda setting",
        "emoji": "📋",
        "icon": "calendar-check",
        "instructions": (
            "Levels: high=actively scheduling or preparing agendas,"
            " medium=reviewing calendar or event details,"
            " low=calendar visible but not being interacted with."
            " Detect via: calendar apps, scheduling interfaces, event creation,"
            " imported calendar events, meeting invitations, agenda drafting."
            " Use for scheduling and preparation work."
            " Do not confuse with meeting — planning is the preparation,"
            " meeting is the actual synchronous interaction."
        ),
    },
    {
        "id": "productivity",
        "name": "Productivity",
        "description": "Spreadsheets, slides, and task management",
        "emoji": "📊",
        "icon": "chart-column",
        "instructions": (
            "Levels: high=actively editing or organizing, medium=reviewing data,"
            " low=app open but not focused."
            " Detect via: spreadsheet/slide editors, project management tools,"
            " task boards."
        ),
    },
    {
        "id": "terminal",
        "name": "Terminal",
        "description": "Command line and shell sessions",
        "emoji": "⌨️",
        "icon": "terminal",
        "instructions": (
            "Levels: high=running commands or scripts, medium=reading output,"
            " low=terminal open but idle."
            " Detect via: shell prompts, command output, tmux/screen sessions."
            " If terminal use is clearly coding-related, prefer coding instead."
        ),
    },
    {
        "id": "design",
        "name": "Design",
        "description": "Design tools and image editing",
        "emoji": "🎨",
        "icon": "palette",
        "instructions": (
            "Levels: high=actively creating or editing, medium=reviewing designs,"
            " low=design tool open but idle."
            " Detect via: design apps (Figma, Photoshop, etc), canvas editing."
        ),
    },
    {
        "id": "music",
        "name": "Music",
        "description": "Music listening and audio",
        "emoji": "🎵",
        "icon": "music",
        "instructions": (
            "Levels: high=actively choosing or browsing music,"
            " medium=playlist running while working, low=ambient background audio."
            " Detect via: music player UI, audio playback indicators."
        ),
    },
]


def get_default_activities() -> list[dict[str, str]]:
    """Return the predefined activities list.

    These are common activities that users can attach to facets.
    Returns a copy to prevent mutation.
    """
    return [dict(a) for a in DEFAULT_ACTIVITIES]


def _lucide_name_shaped(value: object) -> bool:
    return isinstance(value, str) and bool(LUCIDE_ICON_NAME_RE.fullmatch(value))


def _normalized_icon_fields(entry: dict[str, Any]) -> tuple[str | None, str | None]:
    """Return canonical (emoji, icon) fields for a stored activity config entry."""
    emoji = entry.get("emoji")
    icon = entry.get("icon")

    if not isinstance(emoji, str) or not emoji:
        emoji = None

    if not isinstance(icon, str) or not icon:
        return emoji, None

    if _lucide_name_shaped(icon):
        return emoji, icon

    return emoji or icon, None


def _get_activities_path(facet: str) -> Path:
    """Get the path to a facet's activities.jsonl file."""
    return Path(get_journal()) / "facets" / facet / "activities" / "activities.jsonl"


def _load_activities_jsonl(facet: str) -> list[dict[str, Any]]:
    """Load raw activities from a facet's JSONL file.

    Returns empty list if file doesn't exist.
    """
    path = _get_activities_path(facet)
    if not path.exists():
        return []

    activities = []
    with open(path, "r", encoding="utf-8") as f:
        for line_num, line in enumerate(f, 1):
            line = line.strip()
            if line:
                try:
                    activities.append(json.loads(line))
                except json.JSONDecodeError as e:
                    logger.warning(
                        "Skipping malformed line %d in %s: %s", line_num, path, e
                    )
                    continue
    return activities


def _save_activities_jsonl(facet: str, activities: list[dict[str, Any]]) -> None:
    """Save activities to a facet's JSONL file."""
    path = _get_activities_path(facet)

    def modify_fn(_existing: list[dict[str, Any]]) -> list[dict[str, Any]]:
        return [dict(activity) for activity in activities]

    locked_modify(path, modify_fn, create_if_missing=True)


def get_facet_activities(facet: str) -> list[dict[str, Any]]:
    """Load activities attached to a facet.

    Returns activities explicitly attached to the facet plus any default
    activities marked ``always_on``. Always-on activities are auto-included
    even if the facet's ``activities.jsonl`` does not list them.

    Args:
        facet: Facet name

    Returns:
        List of activity dicts with keys:
        - id: Activity identifier
        - name: Display name
        - description: Activity description
        - emoji: Glyph fallback
        - icon: Lucide icon name
        - priority: "high", "normal", or "low"
        - custom: True if user-created (not in defaults)
        - always_on: True if auto-included from defaults
    """
    # Build lookup for defaults
    defaults_by_id = {a["id"]: a for a in DEFAULT_ACTIVITIES}

    # Load facet-specific activities
    facet_activities = _load_activities_jsonl(facet)

    # If no explicit activities configured, use all defaults as the vocabulary
    if not facet_activities:
        result = []
        for default in DEFAULT_ACTIVITIES:
            activity = dict(default)
            activity["custom"] = False
            activity.setdefault("priority", "normal")
            result.append(activity)
        return result

    seen_ids: set[str] = set()
    result = []
    for fa in facet_activities:
        activity_id = fa.get("id")
        if not activity_id:
            continue

        seen_ids.add(activity_id)

        # Start with default metadata if predefined
        if activity_id in defaults_by_id:
            activity = dict(defaults_by_id[activity_id])
            activity["custom"] = False
        else:
            activity = {"id": activity_id, "custom": True}

        emoji, icon = _normalized_icon_fields(fa)

        # Apply facet overrides
        if "name" in fa:
            activity["name"] = fa["name"]
        if "description" in fa:
            activity["description"] = fa["description"]
        if "priority" in fa:
            activity["priority"] = fa["priority"]
        if emoji is not None:
            activity["emoji"] = emoji
        if icon is not None:
            activity["icon"] = icon
        if "instructions" in fa:
            activity["instructions"] = fa["instructions"]

        # Ensure required fields have defaults
        activity.setdefault("name", activity_id.replace("_", " ").title())
        activity.setdefault("description", "")
        activity.setdefault("priority", "normal")

        result.append(activity)

    # Auto-include always-on defaults not already attached
    for default in DEFAULT_ACTIVITIES:
        if default.get("always_on") and default["id"] not in seen_ids:
            activity = dict(default)
            activity["custom"] = False
            activity.setdefault("priority", "normal")
            result.append(activity)

    return result


def save_facet_activities(facet: str, activities: list[dict[str, Any]]) -> None:
    """Save activities configuration for a facet.

    Args:
        facet: Facet name
        activities: List of activity dicts to save. Each should have at minimum:
            - id: Activity identifier
            For custom activities, also include:
            - name: Display name
            - description: Activity description
            Optional for all:
            - priority: "high", "normal", or "low"
            - emoji: Glyph fallback
            - icon: Lucide icon name
            - instructions: Detection/level instructions for the LLM
    """
    # Build lookup for defaults to determine what needs to be stored
    defaults_by_id = {a["id"]: a for a in DEFAULT_ACTIVITIES}

    entries = []
    for activity in activities:
        activity_id = activity.get("id")
        if not activity_id:
            continue

        entry: dict[str, Any] = {"id": activity_id}

        # For predefined activities, only store overrides
        if activity_id in defaults_by_id:
            default = defaults_by_id[activity_id]

            # Store description only if different from default
            if activity.get("description") and activity["description"] != default.get(
                "description"
            ):
                entry["description"] = activity["description"]

            # Store instructions only if different from default
            if activity.get("instructions") and activity["instructions"] != default.get(
                "instructions"
            ):
                entry["instructions"] = activity["instructions"]

            # Store priority if set
            if activity.get("priority") and activity["priority"] != "normal":
                entry["priority"] = activity["priority"]

        else:
            # Custom activity - store all fields
            entry["custom"] = True
            if activity.get("name"):
                entry["name"] = activity["name"]
            if activity.get("description"):
                entry["description"] = activity["description"]
            if activity.get("instructions"):
                entry["instructions"] = activity["instructions"]
            if activity.get("priority"):
                entry["priority"] = activity["priority"]
            if activity.get("emoji"):
                entry["emoji"] = activity["emoji"]
            if activity.get("icon"):
                entry["icon"] = activity["icon"]

        entries.append(entry)

    _save_activities_jsonl(facet, entries)


def migrate_custom_activity_icons_to_emoji(
    *, dry_run: bool = False
) -> dict[str, int | bool]:
    """Migrate legacy custom activity glyphs from icon to emoji.

    Stored custom activities used to treat ``icon`` as an emoji glyph. The
    current shape reserves ``icon`` for Lucide names and stores glyphs in
    ``emoji``. Predefined activities are intentionally left alone because
    public write paths never stored icon overrides for them.
    """
    defaults_by_id = {a["id"]: a for a in DEFAULT_ACTIVITIES}
    facets_dir = Path(get_journal()) / "facets"
    result: dict[str, int | bool] = {
        "dry_run": dry_run,
        "files_scanned": 0,
        "files_changed": 0,
        "records_changed": 0,
    }
    if not facets_dir.exists():
        return result

    for path in sorted(facets_dir.glob("*/activities/activities.jsonl")):
        facet = path.parent.parent.name
        records = _load_activities_jsonl(facet)
        result["files_scanned"] = int(result["files_scanned"]) + 1
        changed = False

        for record in records:
            activity_id = record.get("id")
            if not activity_id:
                continue
            if not (record.get("custom") or activity_id not in defaults_by_id):
                continue

            if record.get("emoji"):
                logger.info(
                    "skipping activity icon migration for facet %s activity %s; "
                    "emoji already present",
                    facet,
                    activity_id,
                )
                continue

            record_changed = False
            emoji, icon = _normalized_icon_fields(record)
            if emoji is not None and record.get("emoji") != emoji:
                record["emoji"] = emoji
                record_changed = True
            if icon is None and "icon" in record:
                record.pop("icon", None)
                record_changed = True
            elif icon is not None and record.get("icon") != icon:
                record["icon"] = icon
                record_changed = True

            if record_changed:
                changed = True
                result["records_changed"] = int(result["records_changed"]) + 1

        if changed:
            result["files_changed"] = int(result["files_changed"]) + 1
            if not dry_run:
                _save_activities_jsonl(facet, records)

    return result


def get_default_activity_by_id(activity_id: str) -> dict[str, Any] | None:
    """Look up a predefined default activity by ID."""
    for activity in DEFAULT_ACTIVITIES:
        if activity.get("id") == activity_id:
            return dict(activity)
    return None


def get_activity_by_id(facet: str, activity_id: str) -> dict[str, Any] | None:
    """Look up a specific activity by ID.

    Args:
        facet: Facet name
        activity_id: Activity identifier

    Returns:
        Activity dict if found, None otherwise
    """
    activities = get_facet_activities(facet)
    for activity in activities:
        if activity.get("id") == activity_id:
            return activity
    return None


def generate_activity_id(name: str) -> str:
    """Generate a slug ID from an activity name.

    Args:
        name: Activity display name

    Returns:
        Slug identifier (lowercase, underscores)
    """
    # Lowercase and replace non-alphanumeric with underscores
    slug = re.sub(r"[^a-z0-9]+", "_", name.lower())
    # Remove leading/trailing underscores
    slug = slug.strip("_")
    return slug or "activity"


def add_activity_to_facet(
    facet: str,
    activity_id: str,
    *,
    name: str | None = None,
    description: str | None = None,
    instructions: str | None = None,
    priority: str = "normal",
    emoji: str | None = None,
    icon: str | None = None,
) -> dict[str, Any]:
    """Add an activity to a facet.

    For predefined activities, only activity_id is required.
    For custom activities, name and description should be provided.

    Args:
        facet: Facet name
        activity_id: Activity identifier
        name: Display name (required for custom activities)
        description: Activity description
        instructions: Detection/level instructions for the LLM
        priority: "high", "normal", or "low"
        emoji: Glyph fallback
        icon: Lucide icon name

    Returns:
        The added activity dict
    """
    # Check if already explicitly attached (in JSONL, not just defaults)
    existing_raw = _load_activities_jsonl(facet)
    for entry in existing_raw:
        if entry.get("id") == activity_id:
            # Already attached - return full activity with defaults merged
            return get_activity_by_id(facet, activity_id) or entry

    # Build new activity entry
    defaults_by_id = {a["id"]: a for a in DEFAULT_ACTIVITIES}

    if activity_id in defaults_by_id:
        # Predefined activity
        activity: dict[str, Any] = {"id": activity_id}
        if description:
            activity["description"] = description
        if instructions:
            activity["instructions"] = instructions
        if priority and priority != "normal":
            activity["priority"] = priority
    else:
        # Custom activity
        activity = {
            "id": activity_id,
            "custom": True,
            "name": name or activity_id.replace("_", " ").title(),
            "description": description or "",
        }
        if instructions:
            activity["instructions"] = instructions
        if priority and priority != "normal":
            activity["priority"] = priority
        if emoji:
            activity["emoji"] = emoji
        if icon:
            activity["icon"] = icon

    # Add to existing activities and save
    existing_raw = _load_activities_jsonl(facet)
    existing_raw.append(activity)
    _save_activities_jsonl(facet, existing_raw)

    # Return the full activity with defaults merged
    return get_activity_by_id(facet, activity_id) or activity


def remove_activity_from_facet(facet: str, activity_id: str) -> bool:
    """Remove an activity from a facet.

    Args:
        facet: Facet name
        activity_id: Activity identifier to remove

    Returns:
        True if activity was removed, False if not found
    """
    existing = _load_activities_jsonl(facet)
    new_list = [a for a in existing if a.get("id") != activity_id]

    if len(new_list) == len(existing):
        # Nothing removed
        return False

    _save_activities_jsonl(facet, new_list)
    return True


def update_activity_in_facet(
    facet: str,
    activity_id: str,
    *,
    description: str | None = None,
    instructions: str | None = None,
    priority: str | None = None,
    name: str | None = None,
    emoji: str | None = None,
    icon: str | None = None,
) -> dict[str, Any] | None:
    """Update an activity's configuration in a facet.

    Args:
        facet: Facet name
        activity_id: Activity identifier
        description: New description (None to keep existing)
        instructions: New detection/level instructions (None to keep existing)
        priority: New priority (None to keep existing)
        name: New name - only applies to custom activities
        emoji: New glyph fallback - only applies to custom activities
        icon: New Lucide icon name - only applies to custom activities

    Returns:
        Updated activity dict, or None if not found
    """
    existing = _load_activities_jsonl(facet)
    defaults_by_id = {a["id"]: a for a in DEFAULT_ACTIVITIES}

    found = False
    for activity in existing:
        if activity.get("id") == activity_id:
            found = True

            if description is not None:
                if description == "" and activity_id in defaults_by_id:
                    activity.pop("description", None)
                else:
                    activity["description"] = description
            if instructions is not None:
                if instructions == "" and activity_id in defaults_by_id:
                    activity.pop("instructions", None)
                else:
                    activity["instructions"] = instructions
            if priority is not None:
                if priority == "normal" and activity_id in defaults_by_id:
                    activity.pop("priority", None)
                else:
                    activity["priority"] = priority

            # Only allow name/emoji/icon changes for custom activities
            if activity.get("custom") or activity_id not in defaults_by_id:
                if name is not None:
                    activity["name"] = name
                if emoji is not None:
                    if emoji:
                        activity["emoji"] = emoji
                    else:
                        activity.pop("emoji", None)
                if icon is not None:
                    if icon:
                        activity["icon"] = icon
                    else:
                        activity.pop("icon", None)

            break

    if not found:
        return None

    _save_activities_jsonl(facet, existing)
    return get_activity_by_id(facet, activity_id)


# ---------------------------------------------------------------------------
# Activity State — per-segment activity state loading
# ---------------------------------------------------------------------------


def load_segment_activity_state(
    day: str, segment: str, facet: str, activity_type: str
) -> dict[str, Any] | None:
    """Load activity state for a specific activity from a segment.

    Reads the activity_state.json written by the activity_state generator
    for a given segment and facet, and returns the entry matching the
    requested activity type.

    Args:
        day: Day in YYYYMMDD format
        segment: Segment key (HHMMSS_LEN)
        facet: Facet name
        activity_type: Activity type to find (e.g., "coding", "meeting")

    Returns:
        Activity state dict with keys like activity, state, description,
        level, active_entities — or None if not found.
    """
    from solstone.think.cluster import _find_segment_dir

    stream = os.environ.get("SOL_STREAM")
    seg_dir = _find_segment_dir(day, segment, stream)
    if not seg_dir:
        return None

    state_path = seg_dir / "talents" / facet / "activity_state.json"
    if not state_path.exists():
        return None

    try:
        with open(state_path, "r", encoding="utf-8") as f:
            states = json.load(f)
    except (json.JSONDecodeError, OSError):
        return None

    if not isinstance(states, list):
        return None

    for entry in states:
        if entry.get("activity") == activity_type:
            return entry

    return None


# ---------------------------------------------------------------------------
# Activity Records — completed activity spans
# ---------------------------------------------------------------------------


def make_activity_id(activity_type: str, since_segment: str) -> str:
    """Build activity record ID from type and start segment key.

    Format: {activity_type}_{since_segment}, e.g. "coding_095809_303".
    Used by both activity_state (live tracking) and activities (records).
    """
    return f"{activity_type}_{since_segment}"


LEVEL_VALUES = {"high": 1.0, "medium": 0.5, "low": 0.25}


def _get_records_path(facet: str, day: str) -> Path:
    """Get path to a facet's activity records file for a day."""
    return Path(get_journal()) / "facets" / facet / "activities" / f"{day}.jsonl"


def _read_jsonl_records(path: Path) -> list[dict[str, Any]]:
    """Load JSONL entries from *path*, skipping malformed lines."""
    if not path.exists():
        return []

    records: list[dict[str, Any]] = []
    with open(path, "r", encoding="utf-8") as handle:
        for line_num, line in enumerate(handle, 1):
            line = line.strip()
            if not line:
                continue
            try:
                data = json.loads(line)
            except json.JSONDecodeError as exc:
                logger.warning(
                    "Skipping malformed line %d in %s: %s", line_num, path, exc
                )
                continue
            if isinstance(data, dict):
                records.append(data)
    return records


def _write_jsonl_records(path: Path, records: list[dict[str, Any]]) -> None:
    """Atomically write JSONL entries to *path*."""
    atomic_replace(
        path,
        "".join(json.dumps(record, ensure_ascii=False) + "\n" for record in records),
    )


def _fallback_activity_title(record: dict[str, Any]) -> str:
    """Return the best available title for an activity record."""
    title = str(record.get("title") or "").strip()
    if title:
        return title

    description = str(record.get("description") or "").strip()
    if description:
        return description

    activity = str(record.get("activity") or record.get("id") or "").strip()
    if activity:
        return activity.replace("_", " ").title()

    return "untitled activity"


def _normalize_activity_record(record: dict[str, Any]) -> dict[str, Any]:
    """Return a normalized activity record copy with schema defaults."""
    normalized = dict(record)
    normalized["title"] = _fallback_activity_title(record)
    normalized["details"] = str(record.get("details") or "")
    normalized["hidden"] = bool(record.get("hidden", False))

    edits = record.get("edits")
    normalized["edits"] = (
        [dict(edit) for edit in edits if isinstance(edit, dict)]
        if isinstance(edits, list)
        else []
    )
    return normalized


def locked_modify(
    path: Path,
    modify_fn: Any,
    *,
    create_if_missing: bool = False,
) -> None:
    """Perform a locked load-modify-save cycle on a JSONL file."""
    with hold_lock(path):
        existed = path.exists()
        if not existed and not create_if_missing:
            raise FileNotFoundError(path)
        current = _read_jsonl_records(path) if existed else []
        updated = modify_fn([dict(item) for item in current])
        if not isinstance(updated, list):
            raise TypeError("modify_fn must return list[dict]")
        if not existed and not updated:
            return
        if existed and updated == current:
            return
        _write_jsonl_records(path, updated)


def _activity_record_paths() -> list[Path]:
    facets_dir = Path(get_journal()) / "facets"
    if not facets_dir.is_dir():
        return []
    return sorted(
        path for path in facets_dir.glob("*/activities/*.jsonl") if path.is_file()
    )


def _remap_activity_record_entity_ids(
    record: dict[str, Any],
    source_id: str,
    target_id: str,
) -> tuple[dict[str, Any], int]:
    updated = copy.deepcopy(record)
    rewrites = 0

    active_entities = updated.get("active_entities")
    if isinstance(active_entities, list):
        remapped_active_entities: list[Any] = []
        changed = False
        for value in active_entities:
            if value == source_id:
                remapped_active_entities.append(target_id)
                rewrites += 1
                changed = True
            else:
                remapped_active_entities.append(value)
        if changed:
            updated["active_entities"] = remapped_active_entities

    list_fields = {
        "participation": ("entity_id",),
        "commitments": ("owner_entity_id", "counterparty_entity_id"),
        "closures": ("owner_entity_id", "counterparty_entity_id"),
        "decisions": ("owner_entity_id", "counterparty_entity_id"),
        "relations": ("from_entity_id", "to_entity_id"),
    }
    for list_field, id_fields in list_fields.items():
        items = updated.get(list_field)
        if not isinstance(items, list):
            continue
        for item in items:
            if not isinstance(item, dict):
                continue
            for id_field in id_fields:
                if item.get(id_field) == source_id:
                    item[id_field] = target_id
                    rewrites += 1

    if rewrites == 0:
        return record, 0
    return updated, rewrites


def remap_activity_entity_ids(
    source_id: str,
    target_id: str,
    *,
    commit: bool = False,
) -> dict[str, Any]:
    """Plan or apply source->target entity-id rewrites in activity records."""
    result: dict[str, Any] = {
        "records_rewritten": 0,
        "fields_rewritten": 0,
        "files_scanned": 0,
        "files_rewritten": 0,
        "errors": [],
    }

    for path in _activity_record_paths():
        result["files_scanned"] += 1
        file_records_rewritten = 0
        file_fields_rewritten = 0

        def modify(records: list[dict[str, Any]]) -> list[dict[str, Any]]:
            nonlocal file_records_rewritten, file_fields_rewritten
            updated_records: list[dict[str, Any]] = []
            for record in records:
                updated, rewrites = _remap_activity_record_entity_ids(
                    record,
                    source_id,
                    target_id,
                )
                if rewrites:
                    file_records_rewritten += 1
                    file_fields_rewritten += rewrites
                updated_records.append(updated)
            return updated_records

        if commit:
            locked_modify(path, modify)
        else:
            try:
                modify(_read_jsonl_records(path))
            except OSError as exc:
                result["errors"].append(
                    {
                        "path": str(path),
                        "message": str(exc),
                    }
                )
                continue

        if file_fields_rewritten:
            result["records_rewritten"] += file_records_rewritten
            result["fields_rewritten"] += file_fields_rewritten
            result["files_rewritten"] += 1

    return result


def apply_entity_merge_activity_inverse(
    entries: list[dict[str, Any]],
    *,
    source_id: str,
    target_id: str,
) -> dict[str, Any]:
    """Undo recorded entity-id rewrites in activity records under owner locks.

    Each entry is matched by record id plus a recorded preimage for the list item
    or active entity occurrence. A stale or shifted locator raises instead of
    rewriting the wrong row.
    """

    result: dict[str, Any] = {
        "records_rewritten": 0,
        "fields_rewritten": 0,
        "files_scanned": 0,
        "files_rewritten": 0,
        "errors": [],
    }
    by_path: dict[str, list[dict[str, Any]]] = {}
    for entry in entries:
        by_path.setdefault(str(entry["path"]), []).append(entry)

    journal = Path(get_journal())
    for path_rel, path_entries in by_path.items():
        path = contained_path(journal, path_rel)
        file_fields = 0
        file_records: set[Any] = set()

        def modify(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
            nonlocal file_fields
            updated = copy.deepcopy(rows)
            for entry in path_entries:
                record = _find_activity_record_for_inverse(updated, entry)
                if _apply_activity_inverse_entry(record, entry, source_id, target_id):
                    file_fields += 1
                    file_records.add(entry.get("record_id"))
            return updated

        locked_modify(path, modify)
        result["files_scanned"] += 1
        if file_fields:
            result["files_rewritten"] += 1
            result["fields_rewritten"] += file_fields
            result["records_rewritten"] += len(file_records)

    return result


def _find_activity_record_for_inverse(
    rows: list[dict[str, Any]], entry: dict[str, Any]
) -> dict[str, Any]:
    record_id = entry.get("record_id")
    matches = [row for row in rows if row.get("id") == record_id]
    if not matches:
        raise ValueError(f"activity inverse locator missing record id {record_id!r}")
    if len(matches) > 1:
        raise ValueError(f"activity inverse locator is ambiguous for {record_id!r}")
    return matches[0]


def _apply_activity_inverse_entry(
    record: dict[str, Any],
    entry: dict[str, Any],
    source_id: str,
    target_id: str,
) -> bool:
    field_path = entry.get("field_path", [])
    if field_path[:1] == ["active_entities"]:
        values = record.get("active_entities")
        if not isinstance(values, list):
            raise ValueError("activity inverse active_entities locator missing list")
        occurrence = int(entry.get("occurrence", 0))
        seen = 0
        for index, value in enumerate(values):
            if value != target_id:
                continue
            if seen == occurrence:
                values[index] = source_id
                return True
            seen += 1
        raise ValueError("activity inverse active_entities preimage not found")

    if len(field_path) != 3:
        raise ValueError(f"activity inverse field_path is invalid: {field_path!r}")
    list_field = str(field_path[0])
    id_field = str(field_path[2])
    items = record.get(list_field)
    if not isinstance(items, list):
        raise ValueError(f"activity inverse locator missing list {list_field!r}")
    item_preimage = entry.get("item_preimage")
    for item in items:
        if not isinstance(item, dict):
            continue
        expected = copy.deepcopy(item_preimage)
        if not isinstance(expected, dict):
            raise ValueError("activity inverse item preimage missing")
        expected[id_field] = target_id
        if item == expected:
            item[id_field] = source_id
            return True
    raise ValueError("activity inverse item preimage not found")


def append_edit(
    record: dict[str, Any],
    *,
    actor: str,
    fields: list[str],
    note: str | None,
    payload: dict[str, Any]
    | None = None,  # Additive: Ledger close writes a `ledger_close` sub-dict alongside the audit edit; keep spread so edit readers see a flat entry.
) -> dict[str, Any]:
    """Append an edit entry to an activity record and return the record."""
    normalized = _normalize_activity_record(record)
    edits = [dict(edit) for edit in normalized.get("edits", [])]
    edit_entry: dict[str, Any] = {
        "timestamp": datetime.now(UTC).isoformat().replace("+00:00", "Z"),
        "actor": actor,
        "fields": list(fields),
        "note": note,
    }
    if payload is not None:
        if not isinstance(payload, dict):
            raise TypeError("payload must be dict[str, Any] when provided")
        collision_keys = sorted(set(payload) & set(edit_entry))
        if collision_keys:
            raise ValueError(
                "payload cannot overwrite canonical edit fields: "
                + ", ".join(collision_keys)
            )
        edit_entry = {**edit_entry, **payload}
    edits.append(edit_entry)
    normalized["edits"] = edits
    return normalized


def get_activity_output_path(
    facet: str,
    day: str,
    activity_id: str,
    key: str,
    output_format: str | None = None,
) -> Path:
    """Return output path for an activity-scheduled agent.

    Output lives under the facet's activities directory, grouped by day
    and activity record ID:

        facets/{facet}/activities/{day}/{activity_id}/{agent}.{ext}

    Args:
        facet: Facet name
        day: Day in YYYYMMDD format
        activity_id: Activity record ID (e.g., "coding_095809_303")
        key: Agent key (e.g., "session_review", "chat:analysis")
        output_format: "json" for JSON, anything else for markdown

    Returns:
        Absolute path for the output file
    """
    from solstone.think.talent import get_output_name

    output_name = get_output_name(key)
    ext = "json" if output_format == "json" else "md"
    return (
        Path(get_journal())
        / "facets"
        / facet
        / "activities"
        / day
        / activity_id
        / f"{output_name}.{ext}"
    )


def _scalar(value: object) -> str:
    return str(value) if value else "none"


def _joined(values: object) -> str:
    if not values:
        return "none"
    return ", ".join(values) or "none"


def _segment_key(activity_id: str) -> str:
    parts = activity_id.rsplit("_", 2)
    if len(parts) == 3:
        candidate = f"{parts[1]}_{parts[2]}"
        if len(parts[1]) == 6 and parts[1].isdigit() and parts[2].isdigit():
            return candidate
    return activity_id


def load_activity_records(
    facet: str, day: str, *, include_hidden: bool = False
) -> list[dict[str, Any]]:
    """Load activity records for a facet and day.

    Returns list of record dicts, empty list if file doesn't exist.
    """
    path = _get_records_path(facet, day)
    records = [
        _normalize_activity_record(record) for record in _read_jsonl_records(path)
    ]
    if include_hidden:
        return records
    return [record for record in records if not record.get("hidden", False)]


def assemble_activity_records_and_narratives(facet: str, day: str) -> str:
    records = sorted(
        load_activity_records(facet, day),
        key=lambda r: (r.get("created_at") or 0, r.get("id") or ""),
    )
    record_lines = [
        (
            f"- id={_scalar(record.get('id'))} | "
            f"activity={_scalar(record.get('activity'))} | "
            f"title={_scalar(record.get('title'))} | "
            f"description={_scalar(record.get('description'))} | "
            f"segments={_joined(record.get('segments'))} | "
            f"active_entities={_joined(record.get('active_entities'))}"
        )
        for record in records
    ]
    records_section = (
        "\n".join(record_lines) if record_lines else "No existing activity records."
    )

    base = Path(get_journal()) / "facets" / facet / "activities" / day
    narrative_blocks = []
    for md_path in sorted(base.glob("*/*.md")):
        activity_id = md_path.parent.name
        filename = md_path.name
        try:
            body = md_path.read_text(encoding="utf-8").strip()
        except (UnicodeDecodeError, OSError) as exc:
            logger.warning("Skipping unreadable narrative %s: %s", md_path, exc)
            continue
        narrative_blocks.append(
            f"### {activity_id}/{filename}\n"
            f"segment_key={_segment_key(activity_id)}\n\n"
            f"{body}"
        )
    narratives_section = (
        "\n\n".join(narrative_blocks) if narrative_blocks else "No per-span narratives."
    )

    return f"## Existing records\n{records_section}\n\n## Per-span narratives\n{narratives_section}"


def make_anticipation_id(
    activity_type: str,
    start: str | None,
    target_date: str,
) -> str:
    """Build the stable ID used for schedule-generated anticipated records."""
    activity_key = str(activity_type or "").strip()
    if not activity_key:
        raise ValueError("activity_type must be non-empty")

    try:
        parsed_target = datetime.strptime(target_date, "%Y-%m-%d")
    except ValueError as exc:
        raise ValueError("target_date must match YYYY-MM-DD") from exc

    if start is None:
        start_key = "000000"
    else:
        if not re.fullmatch(r"\d{2}:\d{2}:\d{2}", start):
            raise ValueError("start must match HH:MM:SS")
        start_key = start.replace(":", "")

    return f"anticipated_{activity_key}_{start_key}_{parsed_target.strftime('%m%d')}"


def dedup_anticipation(
    facet: str,
    target_day: str,
    new_record: dict[str, Any],
    *,
    threshold: float = ANTICIPATION_FUZZY_THRESHOLD,
) -> tuple[bool, list[str]]:
    """Check a new anticipated record for collisions and fuzzy supersedes."""

    new_id = str(new_record.get("id") or "").strip()
    if not new_id:
        raise ValueError("new_record.id is required")

    def _normalize_title(value: Any) -> str:
        return " ".join(str(value or "").lower().split())

    new_title = _normalize_title(new_record.get("title"))
    superseded_ids: list[str] = []

    for record in load_activity_records(facet, target_day, include_hidden=False):
        if record.get("source") != "anticipated":
            continue

        existing_id = str(record.get("id") or "").strip()
        if existing_id == new_id:
            return False, []

        existing_title = _normalize_title(record.get("title"))
        ratio = difflib.SequenceMatcher(None, new_title, existing_title).ratio()
        if ratio >= threshold:
            superseded_ids.append(existing_id)

    return True, superseded_ids


def load_record_ids(facet: str, day: str) -> set[str]:
    """Load just the IDs of existing activity records for idempotency checks."""
    return {
        r["id"]
        for r in load_activity_records(facet, day, include_hidden=True)
        if "id" in r
    }


def append_activity_record(
    facet: str, day: str, record: dict[str, Any], *, _checked: bool = False
) -> bool:
    """Append an activity record to the facet's day file.

    Checks for duplicate ID — returns False if record already exists.

    Args:
        facet: Facet name
        day: Day in YYYYMMDD format
        record: Activity record dict (must have 'id' field)
        _checked: If True, skip the duplicate ID check (caller already verified).

    Returns:
        True if record was written, False if duplicate ID found.
    """
    del _checked  # retained for compatibility; duplicate checks now happen under lock
    path = _get_records_path(facet, day)
    written = False

    def modify_fn(records: list[dict[str, Any]]) -> list[dict[str, Any]]:
        nonlocal written
        record_id = record.get("id")
        if record_id and any(item.get("id") == record_id for item in records):
            return records
        written = True
        return records + [_normalize_activity_record(record)]

    locked_modify(path, modify_fn, create_if_missing=True)
    return written


def update_record_fields(
    facet: str, day: str, record_id: str, fields: dict[str, Any]
) -> bool:
    """Update fields on an existing activity record.

    Rewrites the JSONL file atomically (write temp + rename) with the updated
    fields for the matching record.

    Returns True if record was found and updated, False otherwise.
    """
    path = _get_records_path(facet, day)
    updated = False

    try:

        def modify_fn(records: list[dict[str, Any]]) -> list[dict[str, Any]]:
            nonlocal updated
            new_records: list[dict[str, Any]] = []
            for record in records:
                if record.get("id") == record_id:
                    merged = dict(record)
                    merged.update(fields)
                    new_records.append(_normalize_activity_record(merged))
                    updated = True
                else:
                    new_records.append(record)
            return new_records

        locked_modify(path, modify_fn)
    except FileNotFoundError:
        return False

    return updated


def update_record_description(
    facet: str,
    day: str,
    record_id: str,
    description: str,
    *,
    title: str | None = None,
    details: str | None = None,
) -> bool:
    """Update the description of an existing activity record."""
    patch: dict[str, Any] = {"description": description}
    current = get_activity_record(facet, day, record_id)
    if title is not None:
        patch["title"] = title
    elif current is not None:
        current_title = str(current.get("title") or "").strip()
        current_description = str(current.get("description") or "").strip()
        if not current_title or current_title == current_description:
            patch["title"] = description
    if details is not None:
        patch["details"] = details
    return update_record_fields(facet, day, record_id, patch)


def get_activity_record(facet: str, day: str, record_id: str) -> dict[str, Any] | None:
    """Return one activity record by ID, including hidden records."""
    for record in load_activity_records(facet, day, include_hidden=True):
        if record.get("id") == record_id:
            return record
    return None


def update_activity_record(
    facet: str,
    day: str,
    record_id: str,
    patch: dict[str, Any],
    *,
    actor: str,
    note: str,
) -> dict[str, Any] | None:
    """Apply a shallow patch to an activity record and append one edit."""
    allowed_fields = {"title", "description", "details"}
    if not patch:
        raise ValueError("patch cannot be empty")

    disallowed = sorted(set(patch) - allowed_fields)
    if disallowed:
        raise ValueError(f"patch contains disallowed fields: {', '.join(disallowed)}")

    updated_record: dict[str, Any] | None = None

    def modify_fn(records: list[dict[str, Any]]) -> list[dict[str, Any]]:
        nonlocal updated_record
        new_records: list[dict[str, Any]] = []
        for record in records:
            if record.get("id") == record_id:
                merged = _normalize_activity_record({**record, **patch})
                merged = append_edit(
                    merged,
                    actor=actor,
                    fields=list(patch.keys()),
                    note=note,
                )
                updated_record = merged
                new_records.append(merged)
            else:
                new_records.append(record)
        return new_records

    try:
        locked_modify(_get_records_path(facet, day), modify_fn)
    except FileNotFoundError:
        return None

    return updated_record


def merge_story_fields(
    facet: str,
    day: str,
    record_id: str,
    *,
    story: dict,
    commitments: list[dict],
    closures: list[dict],
    decisions: list[dict],
    relations: list[dict],
    actor: str,
    note: str | None = None,
) -> bool:
    """Replace story-derived fields on an activity record and append one edit."""
    updated = False
    path = _get_records_path(facet, day)

    def modify_fn(records: list[dict[str, Any]]) -> list[dict[str, Any]]:
        nonlocal updated
        new_records: list[dict[str, Any]] = []
        for record in records:
            if record.get("id") == record_id:
                merged = _normalize_activity_record(record)
                merged["story"] = dict(story)
                merged["commitments"] = [dict(entry) for entry in commitments]
                merged["closures"] = [dict(entry) for entry in closures]
                merged["decisions"] = [dict(entry) for entry in decisions]
                merged["relations"] = [dict(entry) for entry in relations]
                merged = append_edit(
                    merged,
                    actor=actor,
                    fields=[
                        "story",
                        "commitments",
                        "closures",
                        "decisions",
                        "relations",
                    ],
                    note=note,
                )
                new_records.append(merged)
                updated = True
            else:
                new_records.append(record)
        return new_records

    try:
        locked_modify(path, modify_fn, create_if_missing=False)
    except FileNotFoundError:
        logger.warning("story hook: activity record not found: %s", record_id)
        return False

    if not updated:
        logger.warning("story hook: activity record not found: %s", record_id)
    return updated


def append_ledger_close_edit(
    facet: str,
    day: str,
    record_id: str,
    *,
    item_id: str,
    note: str,
    as_state: str,
) -> dict[str, Any] | None:
    """Append one ledger-close audit edit to an activity record."""
    updated_record: dict[str, Any] | None = None

    def modify_fn(records: list[dict[str, Any]]) -> list[dict[str, Any]]:
        nonlocal updated_record
        new_records: list[dict[str, Any]] = []
        for record in records:
            if record.get("id") != record_id:
                new_records.append(record)
                continue

            normalized = _normalize_activity_record(record)
            edits = normalized.get("edits", [])
            already_closed = any(
                edit.get("fields") == ["ledger_close"]
                and isinstance(edit.get("ledger_close"), dict)
                and edit["ledger_close"].get("item_id") == item_id
                and edit["ledger_close"].get("as_state") == as_state
                for edit in edits
                if isinstance(edit, dict)
            )
            if already_closed:
                updated_record = normalized
                new_records.append(normalized)
                continue

            normalized = append_edit(
                normalized,
                actor="cli:ledger_close",
                fields=["ledger_close"],
                note=note,
                payload={"ledger_close": {"item_id": item_id, "as_state": as_state}},
            )
            updated_record = normalized
            new_records.append(normalized)
        return new_records

    try:
        locked_modify(_get_records_path(facet, day), modify_fn)
    except FileNotFoundError:
        return None

    return updated_record


def _set_activity_hidden_state(
    facet: str,
    day: str,
    record_id: str,
    *,
    hidden: bool,
    actor: str,
    reason: str | None,
) -> dict[str, Any] | None:
    updated_record: dict[str, Any] | None = None

    def modify_fn(records: list[dict[str, Any]]) -> list[dict[str, Any]]:
        nonlocal updated_record
        new_records: list[dict[str, Any]] = []
        for record in records:
            if record.get("id") != record_id:
                new_records.append(record)
                continue

            normalized = _normalize_activity_record(record)
            if normalized.get("hidden", False) == hidden:
                updated_record = normalized
                new_records.append(normalized)
                continue

            normalized["hidden"] = hidden
            normalized = append_edit(
                normalized,
                actor=actor,
                fields=["hidden"],
                note=reason or ("muted" if hidden else "unmuted"),
            )
            updated_record = normalized
            new_records.append(normalized)
        return new_records

    try:
        locked_modify(_get_records_path(facet, day), modify_fn)
    except FileNotFoundError:
        return None

    return updated_record


def mute_activity_record(
    facet: str,
    day: str,
    record_id: str,
    *,
    actor: str,
    reason: str | None,
) -> dict[str, Any] | None:
    """Hide an activity record without deleting it."""
    return _set_activity_hidden_state(
        facet,
        day,
        record_id,
        hidden=True,
        actor=actor,
        reason=reason,
    )


def unmute_activity_record(
    facet: str,
    day: str,
    record_id: str,
    *,
    actor: str,
    reason: str | None,
) -> dict[str, Any] | None:
    """Restore a previously hidden activity record."""
    return _set_activity_hidden_state(
        facet,
        day,
        record_id,
        hidden=False,
        actor=actor,
        reason=reason,
    )


def estimate_duration_minutes(segments: list[str]) -> int:
    """Estimate total duration in minutes from a list of segment keys.

    Parses each HHMMSS_LEN segment key, sums the durations, returns minutes.
    Returns 1 as a minimum (for empty or unparseable inputs).
    """
    from datetime import datetime as dt

    total_seconds = 0
    for seg in segments:
        start, end = segment_parse(seg)
        if start is not None and end is not None:
            dt_start = dt(2000, 1, 1, start.hour, start.minute, start.second)
            dt_end = dt(2000, 1, 1, end.hour, end.minute, end.second)
            total_seconds += (dt_end - dt_start).total_seconds()
    return max(1, int(total_seconds / 60))


def level_avg(levels: list[str]) -> float:
    """Compute average engagement level from a list of level strings.

    Maps: high=1.0, medium=0.5, low=0.25. Unknown values use 0.5.
    Returns rounded to 2 decimal places.
    """
    if not levels:
        return 0.5
    values = [LEVEL_VALUES.get(level, 0.5) for level in levels]
    return round(sum(values) / len(values), 2)


def _extract_activity_header(file_path: str | os.PathLike[str] | None) -> str:
    """Build a formatter header from an activities file path."""
    if not file_path:
        return "# Activities"

    path = Path(file_path)
    parts = path.parts
    try:
        facet_idx = parts.index("facets")
        facet_name = parts[facet_idx + 1]
    except (ValueError, IndexError):
        facet_name = "unknown"

    stem = path.stem
    if stem.isdigit() and len(stem) == 8:
        return f"# Activities: {facet_name} ({stem[:4]}-{stem[4:6]}-{stem[6:8]})"
    return f"# Activities: {facet_name}"


def _activity_time_range(segments: list[str]) -> str | None:
    """Return a compact HH:MM-HH:MM label for a list of segment keys."""
    if not segments:
        return None

    start_time, _ = segment_parse(segments[0])
    _, end_time = segment_parse(segments[-1])
    if start_time is None or end_time is None:
        return None

    return f"{start_time.strftime('%H:%M')}-{end_time.strftime('%H:%M')}"


def _format_participation(record: dict[str, Any]) -> str | None:
    """Format participation names for display."""
    participation = record.get("participation")
    if not isinstance(participation, list) or not participation:
        return None

    names = []
    for entry in participation:
        if not isinstance(entry, dict):
            continue
        name = str(entry.get("name") or entry.get("entity_id") or "").strip()
        if name:
            names.append(name)

    if not names:
        return None
    return ", ".join(names)


def format_activities(
    entries: list[dict],
    context: dict | None = None,
) -> tuple[list[dict], dict]:
    """Format activity JSONL entries into markdown chunks."""
    ctx = context or {}
    meta: dict[str, Any] = {
        "header": _extract_activity_header(ctx.get("file_path")),
        "indexer": {"agent": "activity"},
    }
    chunks: list[dict[str, Any]] = []

    for entry in entries:
        if not isinstance(entry, dict):
            continue

        record = _normalize_activity_record(entry)
        lines = [f"### {_fallback_activity_title(record)}"]

        activity_type = str(record.get("activity") or record.get("id") or "").strip()
        if activity_type:
            lines.append(f"- Activity: {activity_type}")

        facet = str(record.get("facet") or "").strip()
        if facet:
            lines.append(f"- Facet: {facet}")

        day = str(record.get("day") or "").strip()
        if day:
            lines.append(f"- Day: {day}")

        time_range = _activity_time_range(record.get("segments", []))
        if time_range:
            lines.append(f"- Time: {time_range}")

        if "level_avg" in record:
            lines.append(f"- Level: {record['level_avg']}")

        description = str(record.get("description") or "").strip()
        if description:
            lines.append(f"- Description: {description}")

        details = str(record.get("details") or "").strip()
        if details:
            lines.append(f"- Details: {details}")

        participants = _format_participation(record)
        if participants:
            lines.append(f"- Participation: {participants}")

        story = record.get("story")
        if isinstance(story, dict):
            body = story.get("body")
            if isinstance(body, str) and body.strip():
                lines.append("")
                lines.append(body.strip())

            topics = story.get("topics")
            if isinstance(topics, list):
                topic_values = [
                    topic.strip()
                    for topic in topics
                    if isinstance(topic, str) and topic.strip()
                ]
                if topic_values:
                    lines.append(f"Topics: {', '.join(topic_values)}")

        if record.get("hidden", False):
            lines.append("- Hidden: yes")

        chunks.append(
            {
                "timestamp": int(record.get("created_at", 0) or 0),
                "markdown": "\n".join(lines),
                "source": record,
            }
        )

    return chunks, meta


def _edge_int(value: Any) -> int:
    try:
        return int(value or 0)
    except (TypeError, ValueError):
        return 0


def _edge_str(value: Any) -> str:
    return str(value or "").strip()


def extract_activity_edges(entries: list[dict], ctx: EdgeContext) -> list[dict]:
    """Extract deterministic entity edges from activity records."""
    rows: list[dict[str, Any]] = []

    for entry in entries:
        if not isinstance(entry, dict):
            continue

        record = _normalize_activity_record(entry)
        record_id = _edge_str(record.get("id"))
        title = _edge_str(record.get("title"))
        ts = _edge_int(record.get("created_at"))

        participation = record.get("participation")
        if isinstance(participation, list):
            attendees: list[str] = []
            seen: set[str] = set()
            for part in participation:
                if not isinstance(part, dict):
                    continue
                entity_id = part.get("entity_id")
                if (
                    part.get("role") == "attendee"
                    and isinstance(entity_id, str)
                    and entity_id
                    and entity_id not in seen
                ):
                    attendees.append(entity_id)
                    seen.add(entity_id)

            for src, dst in combinations(attendees, 2):
                rows.append(
                    {
                        "src": src,
                        "dst": dst,
                        "kind": "attended-with",
                        "src_name": None,
                        "dst_name": None,
                        "day": ctx.day,
                        "facet": ctx.facet,
                        "source": "participation",
                        "path": ctx.path,
                        "anchor": record_id,
                        "label": title,
                        "ts": ts,
                        "weight": 1,
                    }
                )

        commitments = record.get("commitments")
        if isinstance(commitments, list):
            rows.extend(
                _extract_story_edge_rows(
                    commitments,
                    ctx=ctx,
                    kind="committed-to",
                    source="commitment",
                    anchor=record_id,
                    ts=ts,
                )
            )

        closures = record.get("closures")
        if isinstance(closures, list):
            rows.extend(
                _extract_story_edge_rows(
                    closures,
                    ctx=ctx,
                    kind="committed-to",
                    source="closure",
                    anchor=record_id,
                    ts=ts,
                )
            )

        decisions = record.get("decisions")
        if isinstance(decisions, list):
            rows.extend(
                _extract_story_edge_rows(
                    decisions,
                    ctx=ctx,
                    kind="decided-with",
                    source="decision",
                    anchor=record_id,
                    ts=ts,
                )
            )

        relations = record.get("relations")
        if isinstance(relations, list):
            for relation in relations:
                if not isinstance(relation, dict):
                    continue
                src = relation.get("from_entity_id")
                dst = relation.get("to_entity_id")
                if not isinstance(src, str) or not src:
                    continue
                if not isinstance(dst, str) or not dst:
                    continue
                if src == dst:
                    continue
                rows.append(
                    {
                        "src": src,
                        "dst": dst,
                        "kind": relation.get("kind"),
                        "src_name": _edge_str(relation.get("from")),
                        "dst_name": _edge_str(relation.get("to")),
                        "day": ctx.day,
                        "facet": ctx.facet,
                        "source": "relation",
                        "path": ctx.path,
                        "anchor": record_id,
                        "label": _relation_label(
                            relation.get("note"),
                            relation.get("quote"),
                        ),
                        "ts": ts,
                        "weight": 1,
                    }
                )

    return rows


def _relation_label(note: Any, quote: Any) -> str:
    """Join relation note and quote into the canonical edge label."""
    parts: list[str] = []
    note_text = _edge_str(note)
    quote_text = _edge_str(quote)
    if note_text:
        parts.append(note_text)
    if quote_text:
        parts.append(f'"{quote_text}"')
    return " — ".join(parts)


def _extract_story_edge_rows(
    entries: list[Any],
    *,
    ctx: EdgeContext,
    kind: str,
    source: str,
    anchor: str,
    ts: int,
) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for item in entries:
        if not isinstance(item, dict):
            continue
        owner_id = item.get("owner_entity_id")
        counterparty_id = item.get("counterparty_entity_id")
        if not isinstance(owner_id, str) or not owner_id:
            continue
        if not isinstance(counterparty_id, str) or not counterparty_id:
            continue
        if owner_id == counterparty_id:
            continue
        rows.append(
            {
                "src": owner_id,
                "dst": counterparty_id,
                "kind": kind,
                "src_name": None,
                "dst_name": None,
                "day": ctx.day,
                "facet": ctx.facet,
                "source": source,
                "path": ctx.path,
                "anchor": anchor,
                "label": _edge_str(item.get("action")),
                "ts": ts,
                "weight": 1,
            }
        )
    return rows
