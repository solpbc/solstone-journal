# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import importlib
from pathlib import Path

from solstone.think.utils import day_path
from tests.conftest import copytree_tracked

FIXTURES = Path("tests/fixtures")


def copy_day(tmp_path: Path, monkeypatch) -> Path:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    dest = day_path("20240101")
    src = FIXTURES / "journal" / "chronicle" / "20240101"
    copytree_tracked(src, dest)
    talents_dir = dest / "talents"
    talents_dir.mkdir(exist_ok=True)  # Allow existing directory
    (talents_dir / "schedule.json").write_text("[]")
    return dest


def test_scan_day(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.talents")
    day_dir = copy_day(tmp_path, monkeypatch)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    info = mod.scan_day("20240101")
    assert "talents/schedule.json" in info["processed"]
    assert "talents/daily_schedule.json" in info["repairable"]

    (day_dir / "talents" / "daily_schedule.json").write_text("[]")
    info_after = mod.scan_day("20240101")
    assert "talents/daily_schedule.json" in info_after["processed"]
    assert "talents/daily_schedule.json" not in info_after["repairable"]
