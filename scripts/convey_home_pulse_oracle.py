#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Capture the reference home API responses against committed journal fixtures."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path

os.environ["TZ"] = "UTC"
if hasattr(time, "tzset"):
    time.tzset()

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT))

from flask import Flask  # noqa: E402
from solstone.apps.home import routes as home  # noqa: E402
from solstone.convey import state  # noqa: E402

CASES = (
    (
        "convey_home_empty_journal",
        "reference-pulse-empty-journal.json",
        datetime(2026, 8, 14, 22, 28, 35, 430840, tzinfo=timezone.utc),
    ),
    (
        "convey_home_seeded_journal",
        "reference-pulse-seeded-journal.json",
        datetime(2026, 8, 14, 23, 25, 13, 793525, tzinfo=timezone.utc),
    ),
)


def _fixed_datetime(instant: datetime) -> type[datetime]:
    class FixedDateTime(datetime):
        @classmethod
        def now(cls, tz: timezone | None = None) -> datetime:
            if tz is None:
                return instant.replace(tzinfo=None)
            return instant.astimezone(tz)

    return FixedDateTime


def _capture(source: Path, instant: datetime) -> dict[str, object]:
    with tempfile.TemporaryDirectory(prefix="convey-home-pulse-") as temporary:
        journal = Path(temporary) / "journal"
        shutil.copytree(source, journal)
        os.environ["SOLSTONE_JOURNAL"] = str(journal)
        state.journal_root = str(journal)
        original_datetime = home.datetime
        original_brain_snapshot = home.build_brain_snapshot
        home.datetime = _fixed_datetime(instant)
        # The reference brain transport cannot run from this source checkout because
        # its companion distribution metadata is unavailable. Pin this dependency to
        # the native fixture projection so the route capture remains deterministic;
        # the empty fixture uses the native unavailable shape.
        home.build_brain_snapshot = lambda _now, surface: _pinned_brain_snapshot(source)

        try:
            app = Flask("convey_home_pulse_oracle")
            app.register_blueprint(home.home_bp)
            client = app.test_client()
            pulse = client.get("/app/home/api/pulse")
            briefing = client.get("/app/home/api/briefing")
        finally:
            home.datetime = original_datetime
            home.build_brain_snapshot = original_brain_snapshot
        if pulse.status_code != 200 or briefing.status_code != 200:
            raise RuntimeError(
                f"reference response failed: pulse={pulse.status_code}, briefing={briefing.status_code}"
            )
        return {"pulse": pulse.get_json(), "briefing": briefing.get_json()}


def _pinned_brain_snapshot(source: Path) -> dict[str, object]:
    if source.name == "convey_home_seeded_journal":
        return {"state": "ready"}
    return {
            "state": "unknown",
            "headline": "thinking status unavailable",
            "reason_code": "brain_record_unavailable",
            "reason_text": "brain record unavailable",
            "failing_component": None,
            "action": {"label": "check again", "refresh": True},
            "identity": {"lane": None, "provider": None, "model": None},
            "evidence": {"observed_at": None, "age_seconds": None, "age_text": None},
            "components": {
                "generate": {"status": None, "reason_code": None, "reason_text": "unknown", "observed_at": None},
                "cogitate": {"status": None, "reason_code": None, "reason_text": "unknown", "observed_at": None},
            },
            "progressing": False,
        }


def _render(value: object) -> str:
    return json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--print", action="store_true", dest="print_output")
    args = parser.parse_args()
    rendered: dict[Path, str] = {}
    for fixture, output, instant in CASES:
        rendered[REPO_ROOT / "core" / "fixtures" / output] = _render(
            _capture(REPO_ROOT / "core" / "fixtures" / fixture, instant)
        )
    if args.print_output:
        print(json.dumps({path.name: json.loads(text) for path, text in rendered.items()}, indent=2, sort_keys=True, ensure_ascii=False))
        return 0
    stale = [
        path
        for path, text in rendered.items()
        if not path.exists() or json.loads(path.read_text()) != json.loads(text)
    ]
    if args.check:
        if stale:
            print("convey home pulse oracle is stale: " + ", ".join(path.name for path in stale))
            return 1
        print("convey home pulse oracle is current")
        return 0
    for path, text in rendered.items():
        path.write_text(text)
    print("wrote " + ", ".join(path.name for path in rendered))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
