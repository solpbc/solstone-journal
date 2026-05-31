# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared rendering helpers for routine state template vars."""

from __future__ import annotations

from datetime import date, timedelta
from typing import Any

_TEMPLATE_TRIGGERS = {
    "morning-briefing": {
        "patterns": [
            "calendar",
            "schedule",
            "agenda",
            "what do i have today",
            "what's on my calendar",
            "whats on my calendar",
            "what's happening today",
            "whats happening today",
        ],
        "threshold": 3,
        "description": "asked about your calendar or schedule",
    },
    "weekly-review": {
        "patterns": [
            "this week",
            "last week",
            "past few days",
            "how did my week",
            "what happened this week",
            "how was my week",
        ],
        "threshold": 3,
        "description": "asked for week-scale synthesis",
    },
    "domain-watch": {
        "patterns": [
            "track",
            "watch",
            "keep an eye on",
            "follow",
            "across days",
            "over time",
            "lately",
            "trend",
            "trends",
        ],
        "threshold": 3,
        "description": "revisited the same topic across multiple days",
    },
    "relationship-pulse": {
        "patterns": [
            "who haven't i",
            "who havent i",
            "relationship",
            "when did i last talk to",
            "catch up with",
        ],
        "threshold": 2,
        "description": "asked about relationships",
    },
    "commitment-audit": {
        "patterns": [
            "follow up",
            "follow-up",
            "commitment",
            "dropped",
            "overdue",
            "what did i promise",
            "pending",
        ],
        "threshold": 2,
        "description": "asked about commitments or follow-ups",
    },
    "meeting-prep": {
        "patterns": [
            "brief me",
            "who am i meeting",
            "meeting with",
            "prepare me for",
            "prep for my meeting",
            "prep me for",
            "meeting prep",
        ],
        "threshold": 3,
        "description": "asked for meeting briefings",
    },
}


def render_active_routines() -> str:
    """Render the active routines template var."""
    from solstone.think.routines import get_routine_state

    routines = get_routine_state()
    if not routines:
        return ""

    lines = ["## Active Routines\n"]
    for routine in routines:
        status = "on" if routine["enabled"] else "paused"
        if routine.get("paused_until"):
            status = f"paused until {routine['paused_until']}"
        line = f"- **{routine['name']}** ({routine['cadence']}) — {status}"
        if routine.get("output_summary"):
            line += f" | recent: {routine['output_summary']}"
        lines.append(line)
    return "\n".join(lines)


def get_eligible_suggestion(
    routines_config: dict[str, Any], journal_config: dict[str, Any]
) -> dict[str, Any] | None:
    """Evaluate the routine suggestion gates and return the best candidate."""
    meta = routines_config.get("_meta", {})

    if not meta.get("suggestions_enabled", True):
        return None

    name_status = journal_config.get("agent", {}).get("name_status", "default")
    if name_status == "default":
        return None

    last_date_str = meta.get("last_suggestion_date")
    if last_date_str:
        try:
            last_date = date.fromisoformat(last_date_str)
            if (date.today() - last_date) < timedelta(days=7):
                return None
        except ValueError:
            pass

    suggestions = meta.get("suggestions", {})
    active_templates = {
        value.get("template")
        for value in routines_config.values()
        if isinstance(value, dict) and value.get("id")
    }

    candidates = []

    for template_name, entry in suggestions.items():
        if template_name in active_templates:
            continue
        if entry.get("response") == "declined":
            continue

        info = _TEMPLATE_TRIGGERS.get(template_name)
        if info and entry.get("trigger_count", 0) >= info["threshold"]:
            candidates.append(
                {
                    "template_name": template_name,
                    "trigger_count": entry["trigger_count"],
                    "first_trigger": entry.get("first_trigger"),
                    "pattern_description": info["description"],
                }
            )

    if "monthly-patterns" not in active_templates:
        mp_entry = suggestions.get("monthly-patterns", {})
        if mp_entry.get("response") != "declined":
            from solstone.think.utils import day_dirs

            days = day_dirs()
            if days:
                earliest = min(days.keys())
                earliest_date = date(
                    int(earliest[:4]),
                    int(earliest[4:6]),
                    int(earliest[6:8]),
                )
                if (date.today() - earliest_date) >= timedelta(days=30):
                    candidates.append(
                        {
                            "template_name": "monthly-patterns",
                            "trigger_count": 0,
                            "first_trigger": (
                                f"{earliest[:4]}-{earliest[4:6]}-{earliest[6:8]}"
                            ),
                            "pattern_description": (
                                "your journal has 30+ days of history"
                            ),
                        }
                    )

    if not candidates:
        return None

    candidates.sort(key=lambda candidate: candidate["trigger_count"], reverse=True)
    return candidates[0]


def render_routine_suggestion() -> str:
    """Render the routine suggestion template var."""
    from solstone.think.routines import get_config as get_routines_config
    from solstone.think.utils import get_config as get_journal_config

    suggestion = get_eligible_suggestion(get_routines_config(), get_journal_config())
    if not suggestion:
        return ""

    if suggestion["trigger_count"] == 0:
        pattern_line = (
            f"Pattern: {suggestion['pattern_description']} "
            f"since {suggestion['first_trigger']}."
        )
    else:
        pattern_line = (
            f"Pattern: You've {suggestion['pattern_description']} "
            f"{suggestion['trigger_count']} times since "
            f"{suggestion['first_trigger']}."
        )

    return (
        "## Routine Suggestion Eligible\n\n"
        f"Template: {suggestion['template_name']}\n"
        f"{pattern_line}\n"
        f"Trigger count: {suggestion['trigger_count']}\n"
        f"First seen: {suggestion['first_trigger']}\n\n"
        "### Etiquette\n"
        "- Mention this ONCE, naturally, at the end of your response\n"
        '- Frame as observation: "I\'ve noticed you often... — would a routine help?"\n'
        "- If $name declines or ignores, do not bring it up again this conversation\n"
        "- After suggesting, run: `journal routines suggest-respond "
        f"{suggestion['template_name']} --accepted` or `--declined`"
    )
