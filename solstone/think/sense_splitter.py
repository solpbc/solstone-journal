# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Split unified Sense agent output into per-agent file locations."""

import json
from datetime import datetime, timezone
from pathlib import Path

from solstone.think.journal_io import atomic_replace, write_text


def write_sense_outputs(
    sense_json: dict, seg_dir: Path, stream: str | None = None
) -> None:
    """Write unified Sense output into per-agent files."""
    agents_dir = seg_dir / "talents"

    density = sense_json["density"]
    activity_summary = sense_json.get("activity_summary") or ""
    entities = sense_json.get("entities") or []
    facets = sense_json.get("facets") or []
    meeting_detected = bool(sense_json.get("meeting_detected"))
    speakers = sense_json.get("speakers") or []

    write_text(agents_dir / "activity.md", activity_summary)
    atomic_replace(agents_dir / "facets.json", json.dumps(facets))
    atomic_replace(
        agents_dir / "density.json",
        json.dumps(
            {
                "classification": density,
                "timestamp": datetime.now(tz=timezone.utc).isoformat(),
            }
        ),
    )
    # Keep the structured Sense artifact and temporary markdown projection.
    # think/talent_outputs.py renders sense.json for consumers that need text,
    # but sense.md remains until follow-up work removes the dual channel.
    # Per-segment absence is normal when Sense found no entities, and consumers
    # must tolerate missing snippets.
    atomic_replace(agents_dir / "sense.json", json.dumps(sense_json))

    if entities:
        lines = ["# Sense Entities", ""]
        for entity in entities:
            if not isinstance(entity, dict):
                continue
            lines.append(
                "- "
                f"{entity.get('type', '')} — {entity.get('name', '')} "
                f"(role={entity.get('role', '')}, source={entity.get('source', '')}) "
                f"— {entity.get('context', '')}"
            )
        if len(lines) > 2:
            write_text(agents_dir / "sense.md", "\n".join(lines))

    if meeting_detected:
        atomic_replace(agents_dir / "speakers.json", json.dumps(speakers))


def write_idle_stubs(seg_dir: Path) -> None:
    """Write minimal idle output files for a segment."""
    atomic_replace(
        seg_dir / "talents" / "density.json",
        json.dumps(
            {
                "classification": "idle",
                "timestamp": datetime.now(tz=timezone.utc).isoformat(),
            }
        ),
    )


def write_change_detection(seg_dir: Path, result: dict) -> None:
    """Write segment change-detection state."""
    atomic_replace(seg_dir / "talents" / "change.json", json.dumps(result))
