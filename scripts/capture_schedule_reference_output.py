#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Capture the frozen Python display contract for ``journal schedule``.

This is deliberately a one-time authoring tool, not a CI dependency. It uses a
known runnable reference interpreter and writes committed fixture data. The
legacy CLI has no clock-injection seam, so the short-lived child process
replaces only its imported ``scheduler.datetime`` with a fixed local civil time.
State epochs are derived from the same local values, keeping timestamp and
next-due formatting deterministic without changing product code.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from datetime import datetime
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT = ROOT / "core/fixtures/schedule_reference_output.json"
DEFAULT_PYTHON = ROOT / ".venv/bin/python"
FROZEN_NOW = "2026-03-22T10:30:00"


def run_case(python: Path, journal: Path, argv: list[str]) -> dict[str, object]:
    program = """
import os
import sys
from datetime import datetime as RealDatetime

from solstone.think import scheduler


class FrozenDatetime(RealDatetime):
    @classmethod
    def now(cls, tz=None):
        value = RealDatetime.fromisoformat(os.environ["SOLSTONE_CAPTURE_NOW"])
        if tz is None:
            return value
        return tz.fromutc(value.replace(tzinfo=tz))


scheduler.datetime = FrozenDatetime
sys.argv = [sys.argv[1], *sys.argv[2:]]
scheduler.main()
"""
    environment = os.environ | {
        "PYTHONPATH": str(ROOT),
        "SOLSTONE_JOURNAL": str(journal),
        "SOL_SKIP_SUPERVISOR_CHECK": "1",
        "SOLSTONE_CAPTURE_NOW": FROZEN_NOW,
    }
    result = subprocess.run(
        [str(python), "-c", program, "journal schedule", *argv],
        cwd=ROOT,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )
    return {"argv": argv, "exit": result.returncode, "stdout": result.stdout.replace(str(journal), "{journal}"), "stderr": result.stderr}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--python", type=Path, default=DEFAULT_PYTHON)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--allow-dirty", action="store_true")
    args = parser.parse_args()
    if not args.python.is_file():
        raise RuntimeError(f"reference interpreter is missing: {args.python}")
    dirty = subprocess.run(["git", "status", "--porcelain"], cwd=ROOT, capture_output=True, text=True, check=True).stdout
    if dirty and not args.allow_dirty:
        raise RuntimeError("refusing capture from a dirty tree")
    with tempfile.TemporaryDirectory(prefix="solstone-schedule-capture-") as directory:
        journal = Path(directory)
        (journal / "config").mkdir()
        (journal / "health").mkdir()
        (journal / "config/journal.json").write_text('{"setup":{"completed_at":1}}', encoding="utf-8")
        cases = {"help": run_case(args.python, journal, ["--help"]), "empty": run_case(args.python, journal, [])}
        table_config = {
            "daily_time": "03:00",
            "weekly_day": "monday",
            "weekly_time": "08:15",
            "daily-with-configured-time": {"cmd": ["journal", "heartbeat"], "every": "daily"},
            "sub-floor-minute": {"cmd": "journal think --cadence", "every": "2m"},
            "unsupported-interval": {"cmd": "journal check", "every": "fortnightly"},
            "very-long-schedule-name:hourly": {"cmd": ["journal", "think"], "every": "hourly"},
            "weekly-status": {"cmd": ["journal", "maintenance"], "every": "weekly"},
            "z:disabled": {"cmd": ["journal", "noop"], "every": "hourly", "enabled": False},
        }
        table_state = {
            "daily-with-configured-time": {"last_run": datetime(2026, 3, 22, 4, 0).timestamp()},
            "sub-floor-minute": {"last_run": datetime(2026, 3, 22, 10, 27).timestamp()},
            "very-long-schedule-name:hourly": {"last_run": datetime(2026, 3, 22, 10, 15).timestamp()},
            "weekly-status": {"last_run": datetime(2026, 3, 16, 9, 0).timestamp()},
        }
        (journal / "config/schedules.json").write_text(json.dumps(table_config), encoding="utf-8")
        (journal / "health/scheduler.json").write_text(json.dumps(table_state), encoding="utf-8")
        cases["table"] = run_case(args.python, journal, [])
        (journal / "config/schedules.json").write_text(
            json.dumps(
                {
                    "daily_time": "03:00",
                    "a:daily": {"cmd": ["journal", "heartbeat"], "every": "daily"},
                    "minute": {"cmd": "journal think --cadence", "every": "1m"},
                    "z:disabled": {
                        "cmd": ["journal", "noop"],
                        "every": "hourly",
                        "enabled": False,
                    },
                }
            ),
            encoding="utf-8",
        )
        (journal / "health/scheduler.json").unlink()
        cases["simple_table"] = run_case(args.python, journal, [])
        (journal / "config/schedules.json").write_text(
            json.dumps(
                {"daily-at-midnight": {"cmd": ["journal", "heartbeat"], "every": "daily"}}
            ),
            encoding="utf-8",
        )
        (journal / "health/scheduler.json").write_text(
            json.dumps({"daily-at-midnight": {"last_run": datetime(2026, 3, 22, 1, 0).timestamp()}}),
            encoding="utf-8",
        )
        cases["midnight"] = run_case(args.python, journal, [])
        (journal / "config/schedules.json").write_text(
            json.dumps(
                {
                    "daily_time": "25:00",
                    "daily-with-invalid-time": {
                        "cmd": ["journal", "heartbeat"],
                        "every": "daily",
                    },
                }
            ),
            encoding="utf-8",
        )
        (journal / "health/scheduler.json").write_text(
            json.dumps(
                {
                    "daily-with-invalid-time": {
                        "last_run": datetime(2026, 3, 22, 4, 0).timestamp()
                    }
                }
            ),
            encoding="utf-8",
        )
        cases["invalid_daily_time"] = run_case(args.python, journal, [])
    payload = {"schema": "schedule-reference-output/1", "provenance": {"captured_from_rev": subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True, check=True).stdout.strip(), "interpreter": str(args.python), "guard": "Run from a clean tree unless --allow-dirty is explicitly used while developing the native port."}, "cases": cases}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
