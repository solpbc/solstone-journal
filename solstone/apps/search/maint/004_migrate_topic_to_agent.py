# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Migrate journal event/stats keys from topic naming to agent naming."""

from __future__ import annotations

import argparse
import json
from dataclasses import dataclass
from pathlib import Path

from solstone.think.utils import get_journal, setup_cli

STATS_KEY_RENAMES = {
    "topic_data": "agent_data",
    "topic_counts": "agent_counts",
    "topic_minutes": "agent_minutes",
    "topic_counts_by_day": "agent_counts_by_day",
}


@dataclass
class MigrationCounters:
    """Mutable counters for migration operations."""

    events_modified: int = 0
    events_skipped: int = 0
    events_errors: int = 0
    stats_modified: int = 0
    stats_skipped: int = 0
    stats_errors: int = 0
    event_records_renamed: int = 0
    stats_keys_renamed: int = 0


def _migrate_event_file(
    file_path: Path, *, dry_run: bool, counters: MigrationCounters
) -> None:
    """Migrate one events JSONL file from topic key to agent key."""
    try:
        lines = file_path.read_text(encoding="utf-8").splitlines()
        out_lines: list[str] = []
        modified = False

        for line in lines:
            stripped = line.strip()
            if not stripped:
                continue

            row = json.loads(stripped)
            if isinstance(row, dict) and "topic" in row:
                if "agent" not in row:
                    row["agent"] = row["topic"]
                del row["topic"]
                modified = True
                counters.event_records_renamed += 1

            out_lines.append(json.dumps(row, ensure_ascii=False))

        if not modified:
            counters.events_skipped += 1
            return

        counters.events_modified += 1
        if dry_run:
            print(f"[DRY-RUN] update {file_path}")
            return

        payload = "\n".join(out_lines)
        if payload:
            payload += "\n"
        file_path.write_text(payload, encoding="utf-8")
    except Exception as exc:
        counters.events_errors += 1
        print(f"[ERROR] events migration failed for {file_path}: {exc}")


def _migrate_stats_file(
    file_path: Path, *, dry_run: bool, counters: MigrationCounters
) -> None:
    """Migrate one stats.json file from topic keys to agent keys."""
    try:
        data = json.loads(file_path.read_text(encoding="utf-8"))
        if not isinstance(data, dict):
            counters.stats_skipped += 1
            return

        modified = False
        for old_key, new_key in STATS_KEY_RENAMES.items():
            if old_key not in data:
                continue

            if new_key not in data:
                data[new_key] = data[old_key]
            del data[old_key]
            modified = True
            counters.stats_keys_renamed += 1

        if not modified:
            counters.stats_skipped += 1
            return

        counters.stats_modified += 1
        if dry_run:
            print(f"[DRY-RUN] update {file_path}")
            return

        file_path.write_text(
            json.dumps(data, indent=2, ensure_ascii=False) + "\n",
            encoding="utf-8",
        )
    except Exception as exc:
        counters.stats_errors += 1
        print(f"[ERROR] stats migration failed for {file_path}: {exc}")


def migrate_topic_to_agent(*, journal: str, dry_run: bool) -> MigrationCounters:
    """Run topic->agent key migration for events JSONL and stats JSON files."""
    counters = MigrationCounters()
    journal_path = Path(journal)

    event_files = sorted(journal_path.glob("facets/*/events/*.jsonl"))
    stats_files = sorted(journal_path.rglob("stats.json"))

    for file_path in event_files:
        _migrate_event_file(file_path, dry_run=dry_run, counters=counters)

    for file_path in stats_files:
        _migrate_stats_file(file_path, dry_run=dry_run, counters=counters)

    print("Migration complete")
    print(f"  events modified: {counters.events_modified}")
    print(f"  events skipped:  {counters.events_skipped}")
    print(f"  events errors:   {counters.events_errors}")
    print(f"  stats modified:  {counters.stats_modified}")
    print(f"  stats skipped:   {counters.stats_skipped}")
    print(f"  stats errors:    {counters.stats_errors}")
    print(f"  event rows renamed: {counters.event_records_renamed}")
    print(f"  stats keys renamed: {counters.stats_keys_renamed}")
    print("After migration, run: journal indexer --rebuild")

    return counters


def main() -> None:
    """CLI entry point for topic->agent migration."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Preview file changes without writing them.",
    )
    args = setup_cli(parser)

    if args.dry_run:
        print("[DRY-RUN] No files will be modified.")

    migrate_topic_to_agent(journal=get_journal(), dry_run=args.dry_run)


if __name__ == "__main__":
    main()
