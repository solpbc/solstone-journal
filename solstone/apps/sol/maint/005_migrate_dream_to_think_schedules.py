# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Rewrite stale `sol dream` schedule commands to `journal think`."""

from __future__ import annotations

import argparse
import json
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

from solstone.think.utils import get_journal, setup_cli


@dataclass
class MigrationSummary:
    discovered: int = 0
    rewritten: int = 0
    preserved: int = 0
    errors: int = 0
    skipped_reason: str | None = None


def _is_dream_schedule_cmd(value: object) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("cmd"), list)
        and len(value["cmd"]) >= 2
        and value["cmd"][0] == "sol"
        and value["cmd"][1] == "dream"
    )


def run_migration(journal_path: Path, *, dry_run: bool) -> MigrationSummary:
    summary = MigrationSummary()
    schedules_path = journal_path / "config" / "schedules.json"

    if not schedules_path.exists():
        summary.skipped_reason = "no file"
        return summary

    try:
        raw_bytes = schedules_path.read_bytes()
    except Exception as exc:
        summary.errors += 1
        print(f"[ERROR] read failed: {schedules_path}: {exc}")
        return summary

    if not raw_bytes.strip():
        summary.skipped_reason = "empty file"
        return summary

    try:
        raw = json.loads(raw_bytes)
    except json.JSONDecodeError:
        summary.skipped_reason = "unparseable"
        return summary

    if not isinstance(raw, dict):
        summary.skipped_reason = "unparseable"
        return summary

    for name, value in raw.items():
        if _is_dream_schedule_cmd(value):
            old_cmd = value["cmd"][:]
            new_cmd = old_cmd[:]
            new_cmd[1] = "think"
            value["cmd"] = new_cmd
            summary.discovered += 1
            summary.rewritten += 1
            print(
                f"{'[DRY-RUN] ' if dry_run else ''}rewrite {name}: {old_cmd!r} -> {new_cmd!r}"
            )
        else:
            summary.preserved += 1

    if summary.discovered == 0:
        return summary

    if dry_run:
        return summary

    try:
        config_dir = schedules_path.parent
        # Atomic write
        fd, tmp_path = tempfile.mkstemp(
            dir=config_dir, suffix=".tmp", prefix=".schedules_"
        )
        tmp_file = Path(tmp_path)
        try:
            with open(fd, "w", encoding="utf-8") as f:
                json.dump(raw, f, indent=2)
            tmp_file.replace(schedules_path)
        except BaseException:
            tmp_file.unlink(missing_ok=True)
            raise
    except Exception as exc:
        summary.errors += 1
        print(f"[ERROR] write failed: {schedules_path}: {exc}")

    return summary


def _print_summary(summary: MigrationSummary) -> None:
    print("Summary")
    print(f"  discovered: {summary.discovered}")
    print(f"  rewritten:  {summary.rewritten}")
    print(f"  preserved:  {summary.preserved}")
    print(f"  errors:     {summary.errors}")
    if summary.skipped_reason is not None:
        print(f"  skipped:    {summary.skipped_reason}")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Rewrite stale sol dream schedule commands to journal think."
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Preview planned renames without writing files.",
    )
    args = setup_cli(parser)

    journal_path = Path(get_journal())
    summary = run_migration(journal_path, dry_run=args.dry_run)

    _print_summary(summary)
    if summary.errors:
        sys.exit(1)


if __name__ == "__main__":
    main()
