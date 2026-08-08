# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Heal stale sol-surface schedule commands to the journal service surface."""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass

from solstone.think.journal_io.errors import LockTimeout, MalformedDataError
from solstone.think.schedule_config import get_schedules_path, set_schedule_entries
from solstone.think.utils import setup_cli

# Frozen migration input. Runtime journal command ownership now lives in the
# Rust `solstone-core-cli-boundary` crate; this one-shot migration must not
# restore a Python command registry merely to recognize old schedule entries.
JOURNAL_SERVICE_COMMANDS = frozenset(
    {
        "backfill-processing-records",
        "backup",
        "brain",
        "config",
        "convey",
        "cortex",
        "depict",
        "describe",
        "down",
        "engage",
        "export",
        "facet-candidates",
        "grab",
        "health",
        "heartbeat",
        "identity",
        "importer",
        "indexer",
        "install-models",
        "install-provider",
        "journal-stats",
        "maint",
        "maintenance",
        "navigate",
        "observer",
        "reprocess",
        "restart-convey",
        "schedule",
        "segment",
        "sense",
        "service",
        "settings",
        "setup",
        "spl",
        "start",
        "streams",
        "supervisor",
        "talent",
        "think",
        "top",
        "transcribe",
        "transfer",
        "up",
        "warm",
    }
)


@dataclass
class MigrationSummary:
    discovered: int = 0
    rewritten: int = 0
    preserved: int = 0
    errors: int = 0
    skipped_reason: str | None = None


def _rewritten_cmd(value: object) -> list | None:
    """Return the healed cmd list for a stale sol-surface entry, else None."""
    if not isinstance(value, dict):
        return None
    cmd = value.get("cmd")
    if not isinstance(cmd, list) or len(cmd) < 2 or cmd[0] != "sol":
        return None
    verb = cmd[1]
    # Special case: the `--sync` subfunction of the access-surface `import`
    # verb moved to the `importer` service command.
    if verb == "import" and "--sync" in cmd:
        return ["journal", "importer", *cmd[2:]]
    # General rule: surface-driven. Any verb whose registered surface is
    # "service" is now invoked via `journal <verb>`.
    if verb in JOURNAL_SERVICE_COMMANDS:
        new_cmd = cmd[:]
        new_cmd[0] = "journal"
        return new_cmd
    return None


def run_migration(*, dry_run: bool) -> MigrationSummary:
    summary = MigrationSummary()
    schedules_path = get_schedules_path()

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

    rewritten_names: list[str] = []
    for name, value in raw.items():
        new_cmd = _rewritten_cmd(value)
        if new_cmd is not None:
            old_cmd = value["cmd"]
            value["cmd"] = new_cmd
            rewritten_names.append(name)
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
        set_schedule_entries({name: raw[name] for name in rewritten_names})
    except (OSError, MalformedDataError, LockTimeout) as exc:
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
        description="Heal stale sol-surface schedule commands to the journal service surface."
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Preview planned rewrites without writing files.",
    )
    args = setup_cli(parser)

    summary = run_migration(dry_run=args.dry_run)

    _print_summary(summary)
    if summary.errors:
        sys.exit(1)


if __name__ == "__main__":
    main()
