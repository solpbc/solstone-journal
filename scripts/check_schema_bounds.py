#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Ratcheting guard for generation-schema array and free-text bounds."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent

if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from solstone.apps.timeline.rollup import build_rollup_schema  # noqa: E402
from solstone.think.schema_bounds import unbounded_nodes  # noqa: E402

ALLOWLIST: dict[str, str] = {
    "build_rollup_schema(3)": "documents follow-on work",
    "solstone/apps/entities/talent/detection.schema.json": (
        "entity_observer follow-on work"
    ),
    "solstone/apps/entities/talent/entities_review.schema.json": (
        "entity_observer follow-on work"
    ),
    "solstone/apps/timeline/talent/segment_summary.schema.json": (
        "documents follow-on work"
    ),
    "solstone/observe/categories/meeting.schema.json": "screen follow-on work",
    "solstone/observe/describe.schema.json": "screen follow-on work",
    "solstone/observe/extract.schema.json": "screen follow-on work",
    "solstone/talent/chat.schema.json": "messaging follow-on work",
    "solstone/talent/participation.schema.json": "calendar follow-on work",
    "solstone/talent/participation_entry.schema.json": "calendar follow-on work",
    "solstone/talent/pulse.schema.json": "morning_briefing follow-on work",
    "solstone/talent/schedule.schema.json": "calendar follow-on work",
    "solstone/talent/sense.schema.json": "sense follow-on work",
    "solstone/talent/speaker_attribution.schema.json": (
        "speaker attribution follow-on work"
    ),
    "solstone/talent/steward.schema.json": "morning_briefing follow-on work",
    "solstone/think/detect_created.schema.json": "created-detection follow-on work",
}


# The shipped payload lives outside the package tree, so the talent and app
# schemas are found under a second root. Keying each by its path relative to the
# root it was found in keeps every allowlist entry below spelled the way the
# installed layout spells it.
PAYLOAD_SRC_ROOT = "core/payload"


def discover_schemas(root: Path) -> dict[str, dict[str, Any]]:
    """Return generation schemas keyed by stable schema id."""
    discovered: dict[str, dict[str, Any]] = {}
    for base in (root, root / PAYLOAD_SRC_ROOT):
        for path in sorted((base / "solstone").glob("**/*.schema.json")):
            schema = json.loads(path.read_text(encoding="utf-8"))
            if isinstance(schema.get("x-journal-contract"), dict):
                continue
            discovered[path.relative_to(base).as_posix()] = schema
    discovered["build_rollup_schema(3)"] = build_rollup_schema(3)
    return discovered


def evaluate(
    root: Path, allowlist: dict[str, str]
) -> tuple[list[str], list[str], list[str]]:
    """Return ``(new, stale, tracked)`` human-readable lines."""
    schemas = discover_schemas(root)
    live = {}
    for schema_id, schema in schemas.items():
        hits = unbounded_nodes(schema)
        if hits:
            live[schema_id] = hits

    new: list[str] = []
    stale: list[str] = []
    tracked: list[str] = []

    for schema_id in sorted(set(live) | set(allowlist)):
        hits = live.get(schema_id, [])
        reason = allowlist.get(schema_id)
        if hits and reason is None:
            joined = ", ".join(hits)
            new.append(
                f"{schema_id}: {len(hits)} unbounded node(s): {joined} - "
                "add generation bounds or add a temporary allowlist reason."
            )
        elif not hits and reason is not None:
            stale.append(
                f"{schema_id}: allowlisted but now clean - delete the entry "
                "from check_schema_bounds.py."
            )
        elif hits and reason is not None:
            tracked.append(
                f"{schema_id}: {len(hits)} unbounded node(s) ({reason}; allowlisted)"
            )

    return new, stale, tracked


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="generation schema bounds lint")
    parser.add_argument(
        "--root",
        type=Path,
        default=ROOT,
        help="Repository root to scan (defaults to the checkout root).",
    )
    args = parser.parse_args(argv)

    new, stale, tracked = evaluate(args.root, ALLOWLIST)

    if tracked:
        print("schema-bounds: known unbounded schemas (allowlisted):")
        for line in tracked:
            print(f"  {line}")
        print()

    if new or stale:
        if new:
            print("schema-bounds: NEW violations:", file=sys.stderr)
            for line in new:
                print(f"  {line}", file=sys.stderr)
            print(file=sys.stderr)
        if stale:
            print("schema-bounds: STALE allowlist entries:", file=sys.stderr)
            for line in stale:
                print(f"  {line}", file=sys.stderr)
            print(file=sys.stderr)
        print(
            "Generation schemas need maxItems on arrays and maxLength on "
            "free-text strings; remove stale allowlist entries as schemas are "
            "bounded.",
            file=sys.stderr,
        )
        return 1

    print("schema-bounds: pass")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
