# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""App-owned scheduled maintenance routines for solstone backup."""

from __future__ import annotations

import argparse

from solstone.think.backup.engine import (
    BACKUP_MAX_RUNTIME,
    PRUNE_MAX_RUNTIME,
    VERIFY_MAX_RUNTIME,
    run_backup,
    run_prune,
    run_verification,
)
from solstone.think.maintenance import MaintenanceRoutine
from solstone.think.offload import (
    OFFLOAD_MAX_RUNTIME,
    format_offload_result,
    run_offload,
)
from solstone.think.utils import require_solstone


def run_backup_routine(args: list[str]) -> int:
    require_solstone()
    parser = argparse.ArgumentParser(prog="journal maintenance run backup:run")
    parser.parse_args(args)

    result = run_backup()
    if result.status == "ok":
        print(f"backup: ok snapshot_id={result.snapshot_id}")
    elif result.status == "skipped":
        print("backup: skipped")
    else:
        print(f"backup: error reason={result.error_reason}")
    return 0


def run_prune_routine(args: list[str]) -> int:
    require_solstone()
    parser = argparse.ArgumentParser(prog="journal maintenance run backup:prune")
    parser.parse_args(args)

    result = run_prune()
    if result.status == "ok":
        print("backup prune: ok")
    elif result.status == "skipped":
        print("backup prune: skipped")
    else:
        print(f"backup prune: error reason={result.error_reason}")
    return 0


def run_verification_routine(args: list[str]) -> int:
    require_solstone()
    parser = argparse.ArgumentParser(prog="journal maintenance run backup:verify")
    parser.parse_args(args)

    result = run_verification()
    if result.status == "ok":
        print(f"backup verify: ok subset={result.checked_subset}")
    elif result.status == "skipped":
        print("backup verify: skipped")
    else:
        print(f"backup verify: error reason={result.reason}")
    return 0


def run_offload_routine(args: list[str]) -> int:
    require_solstone()
    parser = argparse.ArgumentParser(prog="journal maintenance run backup:offload")
    parser.add_argument("--dry-run", action="store_true")
    parsed = parser.parse_args(args)

    result = run_offload(dry_run=parsed.dry_run)
    print(format_offload_result(result))
    return 0


ROUTINES = [
    MaintenanceRoutine(
        name="run",
        description="run encrypted backup.",
        every="hourly",
        run=run_backup_routine,
        max_runtime=BACKUP_MAX_RUNTIME,
    ),
    MaintenanceRoutine(
        name="prune",
        description="apply encrypted backup retention policy.",
        every="daily",
        run=run_prune_routine,
        max_runtime=PRUNE_MAX_RUNTIME,
    ),
    MaintenanceRoutine(
        name="verify",
        description="verify encrypted backup read-back.",
        every="weekly",
        run=run_verification_routine,
        max_runtime=VERIFY_MAX_RUNTIME,
    ),
    MaintenanceRoutine(
        name="offload",
        description="offload verified raw media after backup.",
        every="daily",
        run=run_offload_routine,
        max_runtime=OFFLOAD_MAX_RUNTIME,
    ),
]
