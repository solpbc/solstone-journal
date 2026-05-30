# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Submit past journal days for daily reprocessing."""

from __future__ import annotations

import argparse
import sys
from datetime import date, datetime

from solstone.think.callosum import callosum_send
from solstone.think.utils import (
    DATE_RE,
    day_is_complete,
    day_path,
    iter_segments,
    setup_cli,
)

UNREACHABLE_MESSAGE = "supervisor not reachable - start it (journal start), then retry"


def _fail(message: str) -> None:
    print(message, file=sys.stderr)
    raise SystemExit(1)


def _parse_day(day: str) -> date:
    if not DATE_RE.fullmatch(day):
        _fail("expected day in YYYYMMDD format")
    try:
        return datetime.strptime(day, "%Y%m%d").date()
    except ValueError:
        _fail("expected day in YYYYMMDD format")


def _validate_day_has_data(day: str) -> None:
    day_dir = day_path(day, create=False)
    if not day_dir.is_dir() or not iter_segments(day):
        _fail(f"no data for day {day}")


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
    parsed_day = _parse_day(args.day)
    if parsed_day >= date.today():
        _fail("reprocess is past-only (cannot reprocess today or a future day)")

    _validate_day_has_data(args.day)

    if args.from_scratch:
        # Supervisor request dedups by the "daily" command partition, not by day.
        # A successful send means the request reached supervisor, not that it ran.
        ok = callosum_send(
            "supervisor",
            "request",
            cmd=["journal", "think", "-v", "--day", args.day, "--from-scratch"],
            day=args.day,
        )
        if not ok:
            _fail(UNREACHABLE_MESSAGE)
        print(f"reprocess (from-scratch) submitted for {args.day}")
        return

    if day_is_complete(args.day):
        print(
            f"day {args.day} already complete; use --from-scratch to force a full re-run"
        )
        return

    ok = callosum_send("supervisor", "drain", day=args.day)
    if not ok:
        _fail(UNREACHABLE_MESSAGE)
    print(f"reprocess (process-now) submitted for {args.day}")
