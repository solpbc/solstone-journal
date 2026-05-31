# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Submit past journal days for daily reprocessing."""

from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass
from datetime import date, datetime
from enum import Enum

from solstone.think.callosum import callosum_send
from solstone.think.utils import (
    DATE_RE,
    day_is_complete,
    day_path,
    iter_segments,
    setup_cli,
)

UNREACHABLE_MESSAGE = "supervisor not reachable - start it (journal start), then retry"
FLAVOR_PROCESS_NOW = "process-now"
FLAVOR_FROM_SCRATCH = "from-scratch"


class ReprocessCode(Enum):
    MALFORMED_DAY = "malformed_day"
    PAST_ONLY = "past_only"
    NO_DATA = "no_data"
    FROM_SCRATCH_SUBMITTED = "from_scratch_submitted"
    ALREADY_COMPLETE = "already_complete"
    PROCESS_NOW_SUBMITTED = "process_now_submitted"
    UNREACHABLE = "unreachable"


@dataclass(frozen=True)
class ReprocessOutcome:
    code: ReprocessCode


_CLI_STDOUT = {
    ReprocessCode.PROCESS_NOW_SUBMITTED: "reprocess (process-now) submitted for {day}",
    ReprocessCode.FROM_SCRATCH_SUBMITTED: (
        "reprocess (from-scratch) submitted for {day}"
    ),
    ReprocessCode.ALREADY_COMPLETE: (
        "day {day} already complete; use --from-scratch to force a full re-run"
    ),
}

_CLI_STDERR = {
    ReprocessCode.MALFORMED_DAY: "expected day in YYYYMMDD format",
    ReprocessCode.PAST_ONLY: (
        "reprocess is past-only (cannot reprocess today or a future day)"
    ),
    ReprocessCode.NO_DATA: "no data for day {day}",
    ReprocessCode.UNREACHABLE: UNREACHABLE_MESSAGE,
}


def reprocess_day(day: str, flavor: str) -> ReprocessOutcome:
    if not DATE_RE.fullmatch(day):
        return ReprocessOutcome(ReprocessCode.MALFORMED_DAY)
    try:
        parsed = datetime.strptime(day, "%Y%m%d").date()
    except ValueError:
        return ReprocessOutcome(ReprocessCode.MALFORMED_DAY)
    if parsed >= date.today():
        return ReprocessOutcome(ReprocessCode.PAST_ONLY)
    day_dir = day_path(day, create=False)
    if not day_dir.is_dir() or not iter_segments(day):
        return ReprocessOutcome(ReprocessCode.NO_DATA)

    if flavor == FLAVOR_FROM_SCRATCH:
        # Supervisor request dedups by the "daily" command partition, not by day.
        # A successful send means the request reached supervisor, not that it ran.
        ok = callosum_send(
            "supervisor",
            "request",
            cmd=["journal", "think", "-v", "--day", day, "--from-scratch"],
            day=day,
        )
        return ReprocessOutcome(
            ReprocessCode.FROM_SCRATCH_SUBMITTED if ok else ReprocessCode.UNREACHABLE
        )

    if day_is_complete(day):
        return ReprocessOutcome(ReprocessCode.ALREADY_COMPLETE)

    ok = callosum_send("supervisor", "drain", day=day)
    return ReprocessOutcome(
        ReprocessCode.PROCESS_NOW_SUBMITTED if ok else ReprocessCode.UNREACHABLE
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Submit a past journal day for reprocessing"
    )
    parser.add_argument("day", help="Past day in YYYYMMDD format")
    parser.add_argument(
        "--from-scratch",
        action="store_true",
        help="Re-run both segment and daily units that already completed",
    )

    args = setup_cli(parser)
    flavor = FLAVOR_FROM_SCRATCH if args.from_scratch else FLAVOR_PROCESS_NOW
    outcome = reprocess_day(args.day, flavor)
    code = outcome.code
    if code in _CLI_STDOUT:
        print(_CLI_STDOUT[code].format(day=args.day))
        return

    print(_CLI_STDERR[code].format(day=args.day), file=sys.stderr)
    raise SystemExit(1)
