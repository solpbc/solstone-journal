# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""App-owned scheduled maintenance routines for solstone health."""

from __future__ import annotations

import argparse
import sys
from datetime import datetime, timezone

from solstone.think import retention_executor
from solstone.think.maintenance import MaintenanceRoutine
from solstone.think.retention import _human_bytes
from solstone.think.utils import get_config, get_journal, require_solstone

#: The scheduled marking pass is local filesystem work. An hour is already
#: pathological.
MARK_RAW_MAX_RUNTIME = "60m"


def run_mark_raw_routine(args: list[str]) -> int:
    """List eligible originals as durable removal marks."""
    require_solstone()
    parser = argparse.ArgumentParser(prog="journal maintenance run health:mark-raw")
    parser.parse_args(args)

    journal = get_journal()
    retention = get_config().get("retention", {}) or {}
    now = datetime.now(timezone.utc)
    stamp = now.isoformat(timespec="seconds").replace("+00:00", "Z")
    today = now.astimezone().strftime("%Y-%m-%d")

    payload = retention_executor.policy_payload(retention)
    if not retention_executor.policy_would_release(payload):
        print("mark-raw: your retention settings keep all original media.")
        return 0

    def policy_mark_ids(receipt):
        return {
            mark_id
            for mark_id, mark in receipt["marks"]["marks"].items()
            if mark["class"] == "policy_raw_release"
        }

    try:
        before = retention_executor.marks(journal)
        before_ids = policy_mark_ids(before)
        after = retention_executor.mark(journal, payload, today=today, now=stamp)
    except retention_executor.RemovalRefused as refused:
        print("mark-raw: some items could not be listed:", file=sys.stderr)
        for entry in refused.refused.entries():
            print(f"  {entry.get('entry')}: {entry.get('reason')}", file=sys.stderr)
        return 1
    except retention_executor.ExecutorUnavailable as unavailable:
        print(f"mark-raw: could not build the list: {unavailable}", file=sys.stderr)
        return 1

    after_marks = after["marks"]["marks"]
    after_ids = policy_mark_ids(after)
    new_ids = sorted(after_ids - before_ids)

    print(f"mark-raw: new items: {len(new_ids)}.")
    print(f"  standing total: {len(after_ids)}.")
    for mark_id in new_ids:
        print(f"  {mark_id}: {after_marks[mark_id]['proposal']['reason']}")
    return 0


def run_prune_logs_routine(args: list[str]) -> int:
    require_solstone()
    parser = argparse.ArgumentParser(prog="journal maintenance run health:prune-logs")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--days", type=int, default=None)
    ns = parser.parse_args(args)
    if ns.days is not None and ns.days < 1:
        print("prune-logs: --days must be a positive integer", file=sys.stderr)
        return 1

    journal = get_journal()
    try:
        result = retention_executor.prune_logs(
            journal, days=ns.days, dry_run=ns.dry_run
        )
    # ⛔ This unattended Approval::NotRequired routine reports individual retention
    # failures but stays green, matching the legacy pruner's partial-error behavior.
    except retention_executor.RemovalRefused as refused:
        print(
            f"prune-logs: some logs could not be pruned: {refused.refused.summary()}",
            file=sys.stderr,
        )
        return 0
    except retention_executor.ExecutorUnavailable as unavailable:
        print(f"prune-logs: could not prune logs: {unavailable}", file=sys.stderr)
        return 0
    if not result.enabled:
        print("prune-logs: disabled")
        return 0

    action = "would prune" if result.dry_run else "pruned"
    print(
        "prune-logs: "
        f"{action} {result.files_deleted} operational-log file(s), "
        f"{result.dirs_deleted} cache dir(s), {_human_bytes(result.bytes_freed)} "
        f"cutoff={result.cutoff_day}"
    )
    for class_name, stats in result.by_class.items():
        files = int(stats.get("files_deleted", 0))
        dirs = int(stats.get("dirs_deleted", 0))
        bytes_freed = int(stats.get("bytes_freed", 0))
        if files == 0 and dirs == 0 and bytes_freed == 0:
            continue
        print(
            f"  {class_name}: {files} file(s), {dirs} cache dir(s), "
            f"{_human_bytes(bytes_freed)}"
        )
    root_stats = result.root_task_log
    root_lines = int(root_stats.get("lines_removed", 0))
    if root_lines:
        root_action = "would compact" if result.dry_run else "compacted"
        print(
            f"  root_task_log: {root_action} {root_lines} line(s), "
            f"{_human_bytes(int(root_stats.get('bytes_freed', 0)))}"
        )
    for error in result.errors:
        hint = f" hint={error['hint']}" if error.get("hint") else ""
        print(f"  error: {error['reason']}: {error['message']}{hint}")
    return 0


ROUTINES = [
    MaintenanceRoutine(
        name="mark-raw",
        description="list original media ready for removal.",
        every="daily",
        run=run_mark_raw_routine,
        max_runtime=MARK_RAW_MAX_RUNTIME,
    ),
    MaintenanceRoutine(
        name="prune-logs",
        description="prune old operational logs.",
        every="daily",
        run=run_prune_logs_routine,
        max_runtime=retention_executor.PRUNE_LOGS_MAX_RUNTIME,
    )
]
