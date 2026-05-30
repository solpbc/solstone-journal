# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import os
from datetime import date, timedelta
from pathlib import Path
from unittest.mock import Mock

import pytest

from solstone.think import reprocess

DAY = "20250115"
SEGMENT = "120000_300"
UNREACHABLE = "supervisor not reachable - start it (journal start), then retry\n"


def _invoke_reprocess(monkeypatch, capsys, journal: Path, *argv: str):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setattr("sys.argv", ["sol reprocess", *argv])

    exit_code = 0
    try:
        reprocess.main()
    except SystemExit as exc:
        if isinstance(exc.code, int):
            exit_code = exc.code
        elif exc.code is None:
            exit_code = 0
        else:
            exit_code = 1

    captured = capsys.readouterr()
    return exit_code, captured.out, captured.err


def _seed_segment(journal: Path, day: str = DAY) -> Path:
    segment_dir = journal / "chronicle" / day / "default" / SEGMENT
    segment_dir.mkdir(parents=True)
    return segment_dir


def _touch_marker(journal: Path, day: str, name: str, ns: int) -> Path:
    marker = journal / "chronicle" / day / "health" / name
    marker.parent.mkdir(parents=True, exist_ok=True)
    marker.touch()
    os.utime(marker, ns=(ns, ns))
    return marker


def test_process_now_pending_day_sends_drain_and_preserves_marker(
    tmp_path, monkeypatch, capsys
):
    journal = tmp_path / "journal"
    _seed_segment(journal)
    stream = _touch_marker(journal, DAY, "stream.updated", 2_000_000_000)
    before = stream.stat().st_mtime_ns
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    code, out, err = _invoke_reprocess(monkeypatch, capsys, journal, DAY)

    assert code == 0
    assert out == f"reprocess (process-now) submitted for {DAY}\n"
    assert err == ""
    send.assert_called_once_with("supervisor", "drain", day=DAY)
    assert stream.stat().st_mtime_ns == before
    assert not (stream.parent / "daily.updated").exists()


def test_process_now_complete_day_is_noop_and_preserves_markers(
    tmp_path, monkeypatch, capsys
):
    journal = tmp_path / "journal"
    _seed_segment(journal)
    stream = _touch_marker(journal, DAY, "stream.updated", 1_000_000_000)
    daily = _touch_marker(journal, DAY, "daily.updated", 2_000_000_000)
    before = (stream.stat().st_mtime_ns, daily.stat().st_mtime_ns)
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    code, out, err = _invoke_reprocess(monkeypatch, capsys, journal, DAY)

    assert code == 0
    assert (
        out
        == f"day {DAY} already complete; use --from-scratch to force a full re-run\n"
    )
    assert err == ""
    send.assert_not_called()
    assert (stream.stat().st_mtime_ns, daily.stat().st_mtime_ns) == before


def test_from_scratch_sends_request_and_preserves_marker(tmp_path, monkeypatch, capsys):
    journal = tmp_path / "journal"
    _seed_segment(journal)
    stream = _touch_marker(journal, DAY, "stream.updated", 1_000_000_000)
    _touch_marker(journal, DAY, "daily.updated", 2_000_000_000)
    before = stream.stat().st_mtime_ns
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    code, out, err = _invoke_reprocess(
        monkeypatch, capsys, journal, DAY, "--from-scratch"
    )

    assert code == 0
    assert out == f"reprocess (from-scratch) submitted for {DAY}\n"
    assert err == ""
    send.assert_called_once_with(
        "supervisor",
        "request",
        cmd=["journal", "think", "-v", "--day", DAY, "--from-scratch"],
        day=DAY,
    )
    assert stream.stat().st_mtime_ns == before


@pytest.mark.parametrize("day", ["2025011", "20250230"])
def test_malformed_day_exits_without_send(tmp_path, monkeypatch, capsys, day):
    journal = tmp_path / "journal"
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    code, out, err = _invoke_reprocess(monkeypatch, capsys, journal, day)

    assert code == 1
    assert out == ""
    assert err == "expected day in YYYYMMDD format\n"
    send.assert_not_called()


def test_missing_day_exits_without_send_or_materializing_day(
    tmp_path, monkeypatch, capsys
):
    journal = tmp_path / "journal"
    day_dir = journal / "chronicle" / DAY
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    code, out, err = _invoke_reprocess(monkeypatch, capsys, journal, DAY)

    assert code == 1
    assert out == ""
    assert err == f"no data for day {DAY}\n"
    send.assert_not_called()
    assert not day_dir.exists()


def test_empty_day_exits_without_send(tmp_path, monkeypatch, capsys):
    journal = tmp_path / "journal"
    (journal / "chronicle" / DAY / "health").mkdir(parents=True)
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    code, out, err = _invoke_reprocess(monkeypatch, capsys, journal, DAY)

    assert code == 1
    assert out == ""
    assert err == f"no data for day {DAY}\n"
    send.assert_not_called()


@pytest.mark.parametrize(
    "day",
    [
        date.today().strftime("%Y%m%d"),
        (date.today() + timedelta(days=1)).strftime("%Y%m%d"),
    ],
)
def test_today_and_future_exit_without_send_or_marker_touch(
    tmp_path, monkeypatch, capsys, day
):
    journal = tmp_path / "journal"
    _seed_segment(journal, day)
    stream = _touch_marker(journal, day, "stream.updated", 1_000_000_000)
    before = stream.stat().st_mtime_ns
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    code, out, err = _invoke_reprocess(monkeypatch, capsys, journal, day)

    assert code == 1
    assert out == ""
    assert err == "reprocess is past-only (cannot reprocess today or a future day)\n"
    send.assert_not_called()
    assert stream.stat().st_mtime_ns == before


def test_supervisor_unreachable_exits_nonzero(tmp_path, monkeypatch, capsys):
    journal = tmp_path / "journal"
    _seed_segment(journal)
    _touch_marker(journal, DAY, "stream.updated", 2_000_000_000)
    send = Mock(return_value=False)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    code, out, err = _invoke_reprocess(monkeypatch, capsys, journal, DAY)

    assert code == 1
    assert out == ""
    assert err == UNREACHABLE
    send.assert_called_once_with("supervisor", "drain", day=DAY)
