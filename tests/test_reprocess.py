# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import os
import time
from datetime import date, datetime, timedelta
from pathlib import Path
from unittest.mock import Mock

import pytest

from solstone.think import catchup_state, reprocess

DAY = "20250115"
SEGMENT = "120000_300"
UNREACHABLE = "supervisor not reachable - start it (journal start), then retry\n"


def _invoke_reprocess(monkeypatch, capsys, journal: Path, *argv: str):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setattr("sys.argv", ["journal reprocess", *argv])

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


def _write_catchup_record(
    journal: Path,
    day: str,
    kind: str,
    *,
    fingerprint: str | None,
    next_retry_at: float,
    active: dict | None = None,
) -> None:
    state_path = journal / "health" / "catchup-state.json"
    state_path.parent.mkdir(parents=True, exist_ok=True)
    state_path.write_text(
        json.dumps(
            {
                "version": catchup_state.STATE_VERSION,
                "entries": {
                    f"{day}:{kind}": {
                        "day": day,
                        "command_kind": kind,
                        "attempts": 3,
                        "consecutive_non_completion": 3,
                        "last_attempt_at": 0,
                        "last_outcome": "timeout",
                        "next_retry_at": next_retry_at,
                        "entered_backoff_at": next_retry_at - 600,
                        "notified_at": next_retry_at - 600,
                        "fingerprint": fingerprint,
                        "active": active,
                        "reason_code": None,
                        "timeout_seconds": None,
                        "bounded": None,
                        "cleared": None,
                        "remaining": None,
                        "exit_reason": None,
                        "daily_progress": None,
                    }
                },
            }
        ),
        encoding="utf-8",
    )


def test_format_retry_when_local_day_labels():
    today = date.today()
    tomorrow = today + timedelta(days=1)
    later = today + timedelta(days=2)

    same_day = datetime(today.year, today.month, today.day, 20, 42)
    next_day = datetime(tomorrow.year, tomorrow.month, tomorrow.day, 20, 42)
    midnight = datetime(tomorrow.year, tomorrow.month, tomorrow.day, 0, 3)
    beyond_tomorrow = datetime(later.year, later.month, later.day, 20, 42)

    assert reprocess._format_retry_when(same_day.timestamp()) == "today at 8:42pm"
    assert reprocess._format_retry_when(next_day.timestamp()) == "tomorrow at 8:42pm"
    assert reprocess._format_retry_when(midnight.timestamp()) == "tomorrow at 12:03am"
    assert (
        reprocess._format_retry_when(beyond_tomorrow.timestamp())
        == f"{beyond_tomorrow:%b}".lower() + f" {later.day} at 8:42pm"
    )


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


def test_process_now_held_by_backoff_prints_stdout_and_exits_zero(
    tmp_path, monkeypatch, capsys
):
    journal = tmp_path / "journal"
    segment = _seed_segment(journal)
    (segment / "audio.jsonl").write_text("one\n", encoding="utf-8")
    _touch_marker(journal, DAY, "stream.updated", 2_000_000_000)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    fingerprint = catchup_state.read_raw_input_fingerprint(DAY)
    retry_at = time.time() + 3600
    _write_catchup_record(
        journal,
        DAY,
        catchup_state.KIND_DAILY_CATCHUP,
        fingerprint=fingerprint,
        next_retry_at=retry_at,
    )
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    code, out, err = _invoke_reprocess(monkeypatch, capsys, journal, DAY)

    assert code == 0
    assert (
        out == "day "
        f"{DAY} is held until {reprocess._format_retry_when(retry_at)}; "
        "use --from-scratch to start it over now\n"
    )
    assert err == ""
    send.assert_not_called()


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
        queue_if_active_cmd_differs=True,
    )
    assert stream.stat().st_mtime_ns == before


def test_mark_updated_pending_day_touches_marker_and_sends_drain(
    tmp_path, monkeypatch, capsys
):
    journal = tmp_path / "journal"
    _seed_segment(journal)
    stream = _touch_marker(journal, DAY, "stream.updated", 1_000_000_000)
    before = stream.stat().st_mtime_ns
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    code, out, err = _invoke_reprocess(
        monkeypatch, capsys, journal, DAY, "--mark-updated"
    )

    assert code == 0
    assert out == f"reprocess (mark-updated) submitted for {DAY}\n"
    assert err == ""
    send.assert_called_once_with("supervisor", "drain", day=DAY)
    assert stream.stat().st_mtime_ns > before


def test_mark_updated_complete_day_still_touches_marker_and_sends_drain(
    tmp_path, monkeypatch, capsys
):
    journal = tmp_path / "journal"
    _seed_segment(journal)
    stream = _touch_marker(journal, DAY, "stream.updated", 1_000_000_000)
    daily = _touch_marker(journal, DAY, "daily.updated", 2_000_000_000)
    before = stream.stat().st_mtime_ns
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    code, out, err = _invoke_reprocess(
        monkeypatch, capsys, journal, DAY, "--mark-updated"
    )

    assert code == 0
    assert out == f"reprocess (mark-updated) submitted for {DAY}\n"
    assert err == ""
    send.assert_called_once_with("supervisor", "drain", day=DAY)
    assert stream.stat().st_mtime_ns > before
    assert daily.exists()


def test_mark_updated_and_from_scratch_are_mutually_exclusive(
    tmp_path, monkeypatch, capsys
):
    journal = tmp_path / "journal"
    _seed_segment(journal)
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    code, out, err = _invoke_reprocess(
        monkeypatch,
        capsys,
        journal,
        DAY,
        "--mark-updated",
        "--from-scratch",
    )

    assert code == 2
    assert out == ""
    assert "not allowed with" in err
    send.assert_not_called()


def test_mark_updated_today_exits_without_send_or_marker_touch(
    tmp_path, monkeypatch, capsys
):
    day = date.today().strftime("%Y%m%d")
    journal = tmp_path / "journal"
    _seed_segment(journal, day)
    stream = _touch_marker(journal, day, "stream.updated", 1_000_000_000)
    before = stream.stat().st_mtime_ns
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    code, out, err = _invoke_reprocess(
        monkeypatch, capsys, journal, day, "--mark-updated"
    )

    assert code == 1
    assert out == ""
    assert err == "reprocess is past-only (cannot reprocess today or a future day)\n"
    send.assert_not_called()
    assert stream.stat().st_mtime_ns == before


def test_mark_updated_missing_day_exits_without_send_or_marker_touch(
    tmp_path, monkeypatch, capsys
):
    journal = tmp_path / "journal"
    marker = journal / "chronicle" / DAY / "health" / "stream.updated"
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    code, out, err = _invoke_reprocess(
        monkeypatch, capsys, journal, DAY, "--mark-updated"
    )

    assert code == 1
    assert out == ""
    assert err == f"no data for day {DAY}\n"
    send.assert_not_called()
    assert not marker.exists()


def test_mark_updated_unreachable_touches_marker_and_exits_nonzero(
    tmp_path, monkeypatch, capsys
):
    journal = tmp_path / "journal"
    _seed_segment(journal)
    stream = _touch_marker(journal, DAY, "stream.updated", 1_000_000_000)
    before = stream.stat().st_mtime_ns
    send = Mock(return_value=False)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    code, out, err = _invoke_reprocess(
        monkeypatch, capsys, journal, DAY, "--mark-updated"
    )

    assert code == 1
    assert out == ""
    assert err == UNREACHABLE
    send.assert_called_once_with("supervisor", "drain", day=DAY)
    assert stream.stat().st_mtime_ns > before


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


@pytest.mark.parametrize("day", ["2025011", "20250230"])
def test_reprocess_day_malformed_day_returns_code(tmp_path, monkeypatch, day):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path / "journal"))
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    outcome = reprocess.reprocess_day(day, reprocess.FLAVOR_PROCESS_NOW)

    assert outcome.code is reprocess.ReprocessCode.MALFORMED_DAY
    send.assert_not_called()


@pytest.mark.parametrize(
    "day",
    [
        date.today().strftime("%Y%m%d"),
        (date.today() + timedelta(days=1)).strftime("%Y%m%d"),
    ],
)
def test_reprocess_day_today_and_future_return_past_only(tmp_path, monkeypatch, day):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path / "journal"))
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    outcome = reprocess.reprocess_day(day, reprocess.FLAVOR_PROCESS_NOW)

    assert outcome.code is reprocess.ReprocessCode.PAST_ONLY
    send.assert_not_called()


def test_reprocess_day_missing_day_returns_no_data(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path / "journal"))
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    outcome = reprocess.reprocess_day(DAY, reprocess.FLAVOR_PROCESS_NOW)

    assert outcome.code is reprocess.ReprocessCode.NO_DATA
    send.assert_not_called()


def test_reprocess_day_empty_day_returns_no_data(tmp_path, monkeypatch):
    journal = tmp_path / "journal"
    (journal / "chronicle" / DAY / "health").mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    outcome = reprocess.reprocess_day(DAY, reprocess.FLAVOR_PROCESS_NOW)

    assert outcome.code is reprocess.ReprocessCode.NO_DATA
    send.assert_not_called()


def test_reprocess_day_process_now_submitted(tmp_path, monkeypatch):
    journal = tmp_path / "journal"
    _seed_segment(journal)
    _touch_marker(journal, DAY, "stream.updated", 2_000_000_000)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    outcome = reprocess.reprocess_day(DAY, reprocess.FLAVOR_PROCESS_NOW)

    assert outcome.code is reprocess.ReprocessCode.PROCESS_NOW_SUBMITTED
    send.assert_called_once_with("supervisor", "drain", day=DAY)


def test_reprocess_day_process_now_held_by_daily_backoff_returns_held_without_send(
    tmp_path, monkeypatch
):
    journal = tmp_path / "journal"
    segment = _seed_segment(journal)
    (segment / "audio.jsonl").write_text("one\n", encoding="utf-8")
    _touch_marker(journal, DAY, "stream.updated", 2_000_000_000)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    fingerprint = catchup_state.read_raw_input_fingerprint(DAY)
    retry_at = time.time() + 3600
    _write_catchup_record(
        journal,
        DAY,
        catchup_state.KIND_DAILY_CATCHUP,
        fingerprint=fingerprint,
        next_retry_at=retry_at,
    )
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    outcome = reprocess.reprocess_day(DAY, reprocess.FLAVOR_PROCESS_NOW)

    assert (outcome.code.value, send.call_args_list) == ("held_by_backoff", [])
    assert outcome.when
    assert outcome.when == reprocess._format_retry_when(retry_at)


def test_reprocess_day_active_backoff_record_falls_through_and_sends(
    tmp_path, monkeypatch
):
    journal = tmp_path / "journal"
    segment = _seed_segment(journal)
    (segment / "audio.jsonl").write_text("one\n", encoding="utf-8")
    _touch_marker(journal, DAY, "stream.updated", 2_000_000_000)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    fingerprint = catchup_state.read_raw_input_fingerprint(DAY)
    _write_catchup_record(
        journal,
        DAY,
        catchup_state.KIND_DAILY_CATCHUP,
        fingerprint=fingerprint,
        next_retry_at=time.time() + 3600,
        active={"ref": "daily", "started_at": 1},
    )
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    outcome = reprocess.reprocess_day(DAY, reprocess.FLAVOR_PROCESS_NOW)

    assert outcome.code is reprocess.ReprocessCode.PROCESS_NOW_SUBMITTED
    send.assert_called_once_with("supervisor", "drain", day=DAY)


def test_reprocess_day_process_now_held_by_segment_repair_backoff(
    tmp_path, monkeypatch
):
    journal = tmp_path / "journal"
    segment = _seed_segment(journal)
    (segment / "audio.jsonl").write_text("one\n", encoding="utf-8")
    _touch_marker(journal, DAY, "stream.updated", 2_000_000_000)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    fingerprint = catchup_state.read_raw_input_fingerprint(DAY)
    retry_at = time.time() + 7200
    _write_catchup_record(
        journal,
        DAY,
        catchup_state.KIND_SEGMENT_REPAIR,
        fingerprint=fingerprint,
        next_retry_at=retry_at,
    )
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    outcome = reprocess.reprocess_day(DAY, reprocess.FLAVOR_PROCESS_NOW)

    assert (outcome.code.value, send.call_args_list) == ("held_by_backoff", [])
    assert outcome.when
    assert outcome.when == reprocess._format_retry_when(retry_at)


def test_reprocess_day_backoff_fingerprint_change_sends(tmp_path, monkeypatch):
    journal = tmp_path / "journal"
    segment = _seed_segment(journal)
    raw = segment / "audio.jsonl"
    raw.write_text("one\n", encoding="utf-8")
    _touch_marker(journal, DAY, "stream.updated", 2_000_000_000)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    fingerprint = catchup_state.read_raw_input_fingerprint(DAY)
    _write_catchup_record(
        journal,
        DAY,
        catchup_state.KIND_DAILY_CATCHUP,
        fingerprint=fingerprint,
        next_retry_at=time.time() + 3600,
    )
    raw.write_text("two\n", encoding="utf-8")
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    outcome = reprocess.reprocess_day(DAY, reprocess.FLAVOR_PROCESS_NOW)

    assert outcome.code is reprocess.ReprocessCode.PROCESS_NOW_SUBMITTED
    send.assert_called_once_with("supervisor", "drain", day=DAY)


def test_reprocess_day_from_scratch_submitted_before_complete_check(
    tmp_path, monkeypatch
):
    journal = tmp_path / "journal"
    _seed_segment(journal)
    _touch_marker(journal, DAY, "stream.updated", 1_000_000_000)
    _touch_marker(journal, DAY, "daily.updated", 2_000_000_000)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    outcome = reprocess.reprocess_day(DAY, reprocess.FLAVOR_FROM_SCRATCH)

    assert outcome.code is reprocess.ReprocessCode.FROM_SCRATCH_SUBMITTED
    send.assert_called_once_with(
        "supervisor",
        "request",
        cmd=["journal", "think", "-v", "--day", DAY, "--from-scratch"],
        day=DAY,
        queue_if_active_cmd_differs=True,
    )


def test_reprocess_day_mark_updated_submitted(tmp_path, monkeypatch):
    journal = tmp_path / "journal"
    _seed_segment(journal)
    stream = _touch_marker(journal, DAY, "stream.updated", 1_000_000_000)
    before = stream.stat().st_mtime_ns
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    outcome = reprocess.reprocess_day(DAY, reprocess.FLAVOR_MARK_UPDATED)

    assert outcome.code is reprocess.ReprocessCode.MARK_UPDATED_SUBMITTED
    send.assert_called_once_with("supervisor", "drain", day=DAY)
    assert stream.stat().st_mtime_ns > before


def test_reprocess_day_already_complete_returns_noop(tmp_path, monkeypatch):
    journal = tmp_path / "journal"
    _seed_segment(journal)
    _touch_marker(journal, DAY, "stream.updated", 1_000_000_000)
    _touch_marker(journal, DAY, "daily.updated", 2_000_000_000)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    outcome = reprocess.reprocess_day(DAY, reprocess.FLAVOR_PROCESS_NOW)

    assert outcome.code is reprocess.ReprocessCode.ALREADY_COMPLETE
    send.assert_not_called()


def test_reprocess_day_process_now_unreachable(tmp_path, monkeypatch):
    journal = tmp_path / "journal"
    _seed_segment(journal)
    _touch_marker(journal, DAY, "stream.updated", 2_000_000_000)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    send = Mock(return_value=False)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    outcome = reprocess.reprocess_day(DAY, reprocess.FLAVOR_PROCESS_NOW)

    assert outcome.code is reprocess.ReprocessCode.UNREACHABLE
    send.assert_called_once_with("supervisor", "drain", day=DAY)


def test_reprocess_day_mark_updated_unreachable(tmp_path, monkeypatch):
    journal = tmp_path / "journal"
    _seed_segment(journal)
    stream = _touch_marker(journal, DAY, "stream.updated", 1_000_000_000)
    before = stream.stat().st_mtime_ns
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    send = Mock(return_value=False)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    outcome = reprocess.reprocess_day(DAY, reprocess.FLAVOR_MARK_UPDATED)

    assert outcome.code is reprocess.ReprocessCode.UNREACHABLE
    send.assert_called_once_with("supervisor", "drain", day=DAY)
    assert stream.stat().st_mtime_ns > before


def test_reprocess_day_from_scratch_unreachable(tmp_path, monkeypatch):
    journal = tmp_path / "journal"
    _seed_segment(journal)
    _touch_marker(journal, DAY, "stream.updated", 2_000_000_000)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    send = Mock(return_value=False)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    outcome = reprocess.reprocess_day(DAY, reprocess.FLAVOR_FROM_SCRATCH)

    assert outcome.code is reprocess.ReprocessCode.UNREACHABLE
    send.assert_called_once_with(
        "supervisor",
        "request",
        cmd=["journal", "think", "-v", "--day", DAY, "--from-scratch"],
        day=DAY,
        queue_if_active_cmd_differs=True,
    )


def test_range_from_scratch_without_yes_prints_plan_and_sends_nothing(
    tmp_path, monkeypatch, capsys
):
    journal = tmp_path / "journal"
    _seed_segment(journal, "20250115")
    _seed_segment(journal, "20250116")
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    code, out, err = _invoke_reprocess(
        monkeypatch,
        capsys,
        journal,
        "20250115",
        "--from-scratch",
        "--through",
        "20250116",
    )

    assert code == 0
    assert out == (
        "from-scratch reprocess plan:\n"
        "2 days with data (0 segments) will be queued. Progress will be visible "
        "in journal top or journal health. Queued days do not survive a supervisor "
        "restart.\n"
        "These days run one at a time and can take hours; today's own journal "
        "processing waits until the whole range finishes.\n"
        "re-run with --yes to proceed\n"
    )
    assert err == ""
    send.assert_not_called()


@pytest.mark.parametrize("with_yes", [False, True])
def test_range_from_scratch_zero_data_exits_nonzero_without_plan_or_send(
    tmp_path, monkeypatch, capsys, with_yes
):
    journal = tmp_path / "journal"
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)
    argv = [DAY, "--from-scratch", "--through", "20250116"]
    if with_yes:
        argv.append("--yes")

    code, out, err = _invoke_reprocess(monkeypatch, capsys, journal, *argv)

    assert code == 1
    assert out == ""
    assert err == f"no data for days {DAY} through 20250116\n"
    send.assert_not_called()


@pytest.mark.parametrize(
    ("argv", "expected_err"),
    [
        ([DAY, "--through", "20250116"], "--through requires --from-scratch\n"),
        (
            [DAY, "--mark-updated", "--through", "20250116"],
            "--through requires --from-scratch\n",
        ),
        (
            [DAY, "--from-scratch", "--through", "2025011"],
            "expected day in YYYYMMDD format\n",
        ),
        (
            [
                DAY,
                "--from-scratch",
                "--through",
                date.today().strftime("%Y%m%d"),
            ],
            "reprocess is past-only (cannot reprocess today or a future day)\n",
        ),
        (
            [DAY, "--from-scratch", "--through", "20250114"],
            "--through must be on or after the start day\n",
        ),
    ],
)
def test_range_from_scratch_through_validation_failures_queue_nothing(
    tmp_path, monkeypatch, capsys, argv, expected_err
):
    journal = tmp_path / "journal"
    send = Mock(return_value=True)
    monkeypatch.setattr(reprocess, "callosum_send", send)

    code, out, err = _invoke_reprocess(monkeypatch, capsys, journal, *argv)

    assert code == 1
    assert out == ""
    assert err == expected_err
    send.assert_not_called()
