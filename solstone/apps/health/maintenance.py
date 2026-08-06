# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""App-owned scheduled maintenance routines for solstone health."""

from __future__ import annotations

import argparse
import sys

from datetime import datetime, timezone

from solstone.think import retention_executor
from solstone.think.log_retention import PRUNE_LOGS_MAX_RUNTIME, prune
from solstone.think.maintenance import MaintenanceRoutine
from solstone.think.retention import _human_bytes
from solstone.think.utils import get_config, get_journal, require_solstone

#: The scheduled release of raw originals is local filesystem work. An hour is already
#: pathological.
RELEASE_RAW_MAX_RUNTIME = "60m"


def run_release_raw_routine(args: list[str]) -> int:
    """The scheduled retention pass: release raw originals the policy has released.

    🔴 The plate's headline feature, which until this routine existed had **no
    scheduler, no maintenance routine and no timer** -- so an owner's `days` or
    `processed` setting was owner-settable, UI-rendered and inert.

    ⛔ Arming it is gated on a one-time owner confirmation of the exact policy, per the
    founder ruling of 2026-08-05. An owner who set "delete raw after 30 days" months ago
    and watched nothing happen must not discover that it started from free disk space.
    Until they confirm, this routine reports what it *would* release and releases
    nothing.
    """
    require_solstone()
    parser = argparse.ArgumentParser(
        prog="journal maintenance run health:release-raw"
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="report the plan and release nothing, whatever the confirmation says.",
    )
    ns = parser.parse_args(args)

    journal = get_journal()
    retention = get_config().get("retention", {}) or {}
    # ⛔ One instant, chosen once, for both anchors -- the executor refuses the clock so
    # a verdict is reproducible from its receipt.
    now = datetime.now(timezone.utc)
    stamp = now.isoformat(timespec="seconds").replace("+00:00", "Z")
    today = now.astimezone().strftime("%Y-%m-%d")

    payload = retention_executor.policy_payload(retention)
    if not retention_executor.policy_would_release(payload):
        print("release-raw: policy keeps everything (no-op)")
        return 0

    try:
        if ns.dry_run:
            result = retention_executor.sweep(
                journal, payload, today=today, now=stamp, execute=False
            )
        else:
            result = retention_executor.scheduled_sweep(
                journal, retention, today=today, now=stamp
            )
    except retention_executor.SweepNotConfirmed as pending:
        plan = pending.plan.get("plan", {})
        print(
            "release-raw: AWAITING YOUR CONFIRMATION -- nothing was deleted.\n"
            f"  this policy would release {plan.get('files', 0)} original file(s) "
            f"({_human_bytes(int(plan.get('bytes', 0)))}) across "
            f"{plan.get('candidates', 0)} segment(s), keeping every transcript and "
            "summary derived from them.\n"
            "  it has never run before, so confirm the setting you configured before "
            "it acts."
        )
        return 0
    except retention_executor.RemovalRefused as refused:
        print("release-raw: some originals could not be released:", file=sys.stderr)
        for entry in refused.refused.entries():
            print(f"  {entry.get('entry')}: {entry.get('reason')}", file=sys.stderr)
        return 1
    except retention_executor.ExecutorUnavailable as unavailable:
        print(f"release-raw: {unavailable}", file=sys.stderr)
        return 1

    detail = result.get("detail", {})
    plan = detail.get("plan", result.get("plan", {}))
    action = "would release" if not detail.get("executed") else "released"
    print(
        f"release-raw: {action} {plan.get('files', 0)} original file(s), "
        f"{_human_bytes(int(plan.get('bytes', 0)))} across "
        f"{plan.get('candidates', 0)} segment(s); "
        f"{plan.get('skipped', 0)} segment(s) held"
    )
    for day in plan.get("unreadable_days", []) or []:
        print(f"  warning: {day} could not be read", file=sys.stderr)
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

    result = prune(days=ns.days, dry_run=ns.dry_run)
    if not result.enabled:
        print("prune-logs: disabled (no-op)")
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
        name="release-raw",
        description=(
            "release raw originals whose derived output proves they were consumed, "
            "per the owner's retention policy."
        ),
        every="daily",
        run=run_release_raw_routine,
        max_runtime=RELEASE_RAW_MAX_RUNTIME,
    ),
    MaintenanceRoutine(
        name="prune-logs",
        description="prune old operational logs and execution traces.",
        every="daily",
        run=run_prune_logs_routine,
        max_runtime=PRUNE_LOGS_MAX_RUNTIME,
    )
]
