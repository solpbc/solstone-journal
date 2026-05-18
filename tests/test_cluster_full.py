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
    return dest


def test_cluster_full(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.cluster")
    copy_day(tmp_path, monkeypatch)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    md, counts = mod.cluster(
        "20240101", sources={"transcripts": True, "percepts": False, "agents": True}
    )
    # Transcript entries come from 2 segments on 20240101 (default + import.apple)
    assert counts["transcripts"] == 2
    assert counts["agents"] == 2  # audio.md + screen.md
    assert "### Transcript" in md
    # Now uses insight format: "### {stem} summary"
    assert "### screen summary" in md
    assert "### audio summary" in md


def test_cluster_default_sources(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.cluster")
    copy_day(tmp_path, monkeypatch)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    out, _counts = mod.cluster(
        "20240101", sources={"transcripts": True, "percepts": False, "agents": True}
    )
    # Now uses insight format: "### {stem} summary"
    assert "### screen summary" in out


def test_cluster_range_raw_screen(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.cluster")
    copy_day(tmp_path, monkeypatch)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    out = mod.cluster_range(
        "20240101",
        "123456",
        "123556",
        sources={"transcripts": True, "percepts": True, "agents": False},
    )
    # Range mode with screen=True uses raw screen data.
    assert "### Screen Activity" in out
    assert "IDE with auth.py open" in out
