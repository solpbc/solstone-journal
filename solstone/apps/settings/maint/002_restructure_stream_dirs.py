# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Restructure journal to day/stream/segment/ directory layout.

Moves segment directories from the flat layout (YYYYMMDD/HHMMSS_LEN/)
into stream subdirectories (YYYYMMDD/stream/HHMMSS_LEN/) based on
each segment's stream.json marker.

Prerequisites:
- All segments must have stream.json markers (run 001_backfill_streams first)
- All services should be stopped during migration

This migration is a one-way operation — the old flat layout is not
supported after completion.
"""

from __future__ import annotations

import argparse
import shutil
from pathlib import Path

from solstone.think.streams import read_segment_stream
from solstone.think.utils import day_dirs, get_journal, segment_key, setup_cli


def _is_empty_segment(seg_dir: Path) -> bool:
    """Check if a segment directory has no content files."""
    return not any(f.is_file() and f.name != "stream.json" for f in seg_dir.iterdir())


def _count_segments(days: dict[str, str]) -> tuple[int, int, int]:
    """Count total segments, those missing stream.json, and empty ones."""
    total = 0
    missing = 0
    empty = 0
    for day_path_str in days.values():
        day_dir = Path(day_path_str)
        for entry in day_dir.iterdir():
            if not entry.is_dir() or not segment_key(entry.name):
                continue
            if _is_empty_segment(entry):
                empty += 1
                continue
            total += 1
            marker = read_segment_stream(entry)
            if not marker or not marker.get("stream"):
                missing += 1
    return total, missing, empty


def _is_already_restructured(days: dict[str, str]) -> bool:
    """Check if the journal already uses the stream directory layout.

    Returns True if at least one day has segments inside stream subdirectories
    and no segments exist as direct children of day directories.
    """
    found_nested = False
    for day_path_str in days.values():
        day_dir = Path(day_path_str)
        for entry in day_dir.iterdir():
            if not entry.is_dir():
                continue
            if segment_key(entry.name):
                # Found a segment as direct child — still flat layout
                return False
            # Potential stream directory — check for segments inside
            for sub in entry.iterdir():
                if sub.is_dir() and segment_key(sub.name):
                    found_nested = True
    return found_nested


def restructure(journal_root: Path, dry_run: bool) -> None:
    """Move segments into stream subdirectories."""
    days = day_dirs()
    if not days:
        print("No day directories found.")
        return

    # Check if already restructured
    if _is_already_restructured(days):
        print("Journal already uses stream directory layout. Nothing to do.")
        return

    # Pre-check: all segments must have stream.json
    total, missing, empty = _count_segments(days)
    if total == 0:
        print("No segments found.")
        return

    print(f"Found {total} segments across {len(days)} days")
    if empty > 0:
        print(f"Skipping {empty} empty segment directories")

    if missing > 0:
        print(
            f"\nERROR: {missing} segments are missing stream.json markers.\n"
            "Run 'journal maint settings:001_backfill_streams' first to tag all segments."
        )
        raise SystemExit(1)

    # Move segments into stream directories
    moved = 0
    removed = 0
    streams_seen: set[str] = set()

    for day in sorted(days):
        day_dir = Path(days[day])
        # Collect segments first to avoid modifying dir while iterating
        segments = []
        for entry in sorted(day_dir.iterdir()):
            if entry.is_dir() and segment_key(entry.name):
                segments.append(entry)

        for seg_dir in segments:
            if _is_empty_segment(seg_dir):
                if dry_run:
                    print(f"  [dry-run] {day}/{seg_dir.name} -> removed (empty)")
                else:
                    shutil.rmtree(seg_dir)
                removed += 1
                continue
            marker = read_segment_stream(seg_dir)
            stream_name = marker["stream"]
            streams_seen.add(stream_name)

            target_parent = day_dir / stream_name
            target = target_parent / seg_dir.name

            if dry_run:
                print(
                    f"  [dry-run] {day}/{seg_dir.name} -> {day}/{stream_name}/{seg_dir.name}"
                )
            else:
                target_parent.mkdir(parents=True, exist_ok=True)
                shutil.move(str(seg_dir), str(target))

            moved += 1

    action = "Would move" if dry_run else "Moved"
    print(f"\n{action} {moved} segments into {len(streams_seen)} streams")
    if removed > 0:
        rm_action = "Would remove" if dry_run else "Removed"
        print(f"{rm_action} {removed} empty segment directories")

    if not dry_run:
        # Verify: count segments in new layout
        post_count = 0
        for day_path_str in days.values():
            day_dir = Path(day_path_str)
            for stream_dir in day_dir.iterdir():
                if not stream_dir.is_dir():
                    continue
                for seg_dir in stream_dir.iterdir():
                    if seg_dir.is_dir() and segment_key(seg_dir.name):
                        post_count += 1

        if post_count == total:
            print(f"Verified: {post_count} segments in new layout")
        else:
            print(
                f"WARNING: Expected {total} segments but found {post_count} "
                "after restructure. Check for errors."
            )

        print("\nRun 'sol indexer --rebuild' to reindex with new paths.")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show what would be moved without making changes",
    )

    args = setup_cli(parser)
    journal_root = Path(get_journal())
    restructure(journal_root, args.dry_run)


if __name__ == "__main__":
    main()
