# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Any
from unittest.mock import Mock

import pytest

import solstone.think.offload as offload
import solstone.think.offload_ledger as offload_ledger
import solstone.think.utils as think_utils
from solstone.think.backup import engine
from solstone.think.backup.runner import ResticResult
from solstone.think.offload_ledger import (
    OffloadFile,
    append_offload_event,
    summarize_segment,
)
from solstone.think.utils import parse_duration_seconds

GB = 1_000_000_000


def _use_journal(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    think_utils._journal_path_cache = None
    empty_marks = {"ok": True, "verb": "marks", "marks": {"version": 1, "marks": {}}}
    monkeypatch.setattr(
        offload.retention_executor, "marks", Mock(return_value=empty_marks)
    )
    monkeypatch.setattr(
        offload.retention_executor,
        "mark_offload",
        Mock(return_value=empty_marks),
    )
    return tmp_path


def _config_path(journal: Path) -> Path:
    return journal / "config" / "journal.json"


def _write_config(journal: Path, payload: dict[str, Any]) -> None:
    config_path = _config_path(journal)
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


def _read_config(journal: Path) -> dict[str, Any]:
    return json.loads(_config_path(journal).read_text(encoding="utf-8"))


def _ready_config(
    *,
    now: int,
    budget_bytes: int | None = 1,
    floor_bytes: int | None = None,
    last_ok_delta: int = 60,
    verification_status: str = "ok",
    verification_reason: str | None = None,
    last_offload: dict[str, Any] | None = None,
) -> dict[str, Any]:
    backup: dict[str, Any] = {
        "enabled": True,
        "mode": "byo",
        "destination": {
            "repository": "s3:safe-bucket/path",
            "backend": "s3",
            "credentials": {
                "access_key_id": "access-key",
                "secret_access_key": "secret-key",
            },
        },
        "daily_key": "daily-secret",
        "recovery_key": "R" * 64,
        "offload": {
            "enabled": True,
            "budget_bytes": budget_bytes,
            "floor_bytes": floor_bytes,
        },
        "last_backup": {
            "time": now - 120,
            "snapshot_id": "full-snapshot",
            "status": "ok",
            "error_reason": None,
        },
        "last_verification": {
            "time": now - last_ok_delta,
            "status": verification_status,
            "reason": verification_reason,
            "last_ok_time": now - last_ok_delta,
            "checked_subset": "7/52" if verification_status == "ok" else None,
        },
    }
    if last_offload is not None:
        backup["last_offload"] = last_offload
    return {"backup": backup}


def _make_segment(
    journal: Path,
    *,
    day: str = "20260101",
    stream: str = "default",
    segment: str = "120000_300",
    raw_name: str = "audio.wav",
    content: bytes = b"raw-media",
    size: int | None = None,
    complete: bool = True,
    failed: bool = False,
) -> Path:
    seg_path = journal / "chronicle" / day / stream / segment
    seg_path.mkdir(parents=True, exist_ok=True)
    raw_path = seg_path / raw_name
    if size is None:
        raw_path.write_bytes(content)
    else:
        with raw_path.open("wb") as handle:
            handle.truncate(size)
    if complete:
        if failed:
            header = {"_solstone_processing": {"state": "failed"}}
            (seg_path / "audio.jsonl").write_text(
                json.dumps(header) + "\n", encoding="utf-8"
            )
        else:
            (seg_path / "audio.jsonl").write_text(
                '{}\n{"start": 0}\n', encoding="utf-8"
            )
    (seg_path / "notes.jsonl").write_text("{}\n", encoding="utf-8")
    talents = seg_path / "talents"
    talents.mkdir()
    (talents / "keep.json").write_text("{}\n", encoding="utf-8")
    return seg_path


def _ok_confirm(
    snapshot_id: str,
    expected_sizes: dict[Path, int],
) -> engine.ArchiveCheckResult:
    return engine.ArchiveCheckResult(
        status="ok",
        error_reason=None,
        verdicts=tuple(
            engine.ArchiveFileVerdict(
                path=str(path),
                confirmed=True,
                expected_size=size,
                observed_size=size,
                snapshot_id=snapshot_id,
            )
            for path, size in expected_sizes.items()
        ),
    )


def _install_successful_archive(
    monkeypatch: pytest.MonkeyPatch,
) -> tuple[list[tuple[Path, ...]], list[dict[Path, int]]]:
    archive_calls: list[tuple[Path, ...]] = []
    confirm_calls: list[dict[Path, int]] = []

    def fake_archive(paths: list[Path]) -> engine.BackupResult:
        archive_calls.append(tuple(paths))
        return engine.BackupResult(
            status="ok",
            snapshot_id=f"archive-{len(archive_calls)}",
            error_reason=None,
        )

    def fake_confirm(
        snapshot_id: str,
        expected_sizes: dict[Path, int],
    ) -> engine.ArchiveCheckResult:
        confirm_calls.append(dict(expected_sizes))
        return _ok_confirm(snapshot_id, expected_sizes)

    monkeypatch.setattr(offload, "run_archive_backup", fake_archive)
    monkeypatch.setattr(offload, "check_archive_snapshot_files", fake_confirm)
    monkeypatch.setattr(offload, "request_verification_now", Mock(return_value=True))
    return archive_calls, confirm_calls


def _assert_no_offload_side_effects(journal: Path, raw_path: Path) -> None:
    assert raw_path.exists()
    assert not (journal / "health" / "offload").exists()
    assert not (journal / "health" / "pruning-runs").exists()


def _pruning_records(journal: Path) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    for path in sorted((journal / "health" / "pruning-runs").glob("*.jsonl")):
        records.extend(
            json.loads(line)
            for line in path.read_text(encoding="utf-8").splitlines()
            if line.strip()
        )
    return records


def test_offload_max_runtime_exceeds_retry_lock_and_archive_timeout() -> None:
    assert parse_duration_seconds(offload.OFFLOAD_MAX_RUNTIME) > (
        parse_duration_seconds(engine.ARCHIVE_RETRY_LOCK)
    )
    assert parse_duration_seconds(offload.OFFLOAD_MAX_RUNTIME) > (
        engine.ARCHIVE_BACKUP_TIMEOUT_SECONDS
    )


def test_offload_disabled_skips_without_recording_or_archiving(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    journal = _use_journal(tmp_path, monkeypatch)
    now = 1_800_000_000
    config = _ready_config(now=now)
    config["backup"]["offload"]["enabled"] = False
    _write_config(journal, config)
    raw_path = _make_segment(journal).joinpath("audio.wav")
    archive = Mock()
    monkeypatch.setattr(offload, "run_archive_backup", archive)

    result = offload.run_offload()

    assert result.status == "skipped"
    archive.assert_not_called()
    assert "last_offload" not in _read_config(journal)["backup"]
    assert raw_path.exists()


@pytest.mark.parametrize(
    ("mutate", "expected_reason", "request_expected"),
    [
        (
            lambda backup, now: backup.update({"enabled": False}),
            "backup_not_ready",
            False,
        ),
        (
            lambda backup, now: backup["last_backup"].update(
                {"status": "error", "error_reason": "incomplete", "snapshot_id": None}
            ),
            "backup_failing",
            False,
        ),
        (
            lambda backup, now: backup["last_verification"].update(
                {
                    "time": None,
                    "status": None,
                    "reason": None,
                    "last_ok_time": None,
                    "checked_subset": None,
                }
            ),
            "verification_missing",
            True,
        ),
        (
            lambda backup, now: backup["last_verification"].update(
                {"last_ok_time": now - offload.VERIFICATION_MAX_AGE_SECONDS - 1}
            ),
            "verification_overdue",
            True,
        ),
        (
            lambda backup, now: backup["last_verification"].update(
                {
                    "status": "error",
                    "reason": "integrity_failed",
                    "last_ok_time": now - offload.VERIFICATION_MAX_AGE_SECONDS - 1,
                }
            ),
            "verification_failed",
            False,
        ),
    ],
)
def test_precondition_stalls_happen_before_archive_and_request_only_when_needed(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    mutate,
    expected_reason: str,
    request_expected: bool,
) -> None:
    journal = _use_journal(tmp_path, monkeypatch)
    now = 1_800_000_000
    monkeypatch.setattr(offload.time, "time", lambda: now)
    config = _ready_config(now=now)
    mutate(config["backup"], now)
    _write_config(journal, config)
    raw_path = _make_segment(journal).joinpath("audio.wav")
    archive = Mock()
    confirm = Mock()
    request = Mock(return_value=True)
    monkeypatch.setattr(offload, "run_archive_backup", archive)
    monkeypatch.setattr(offload, "check_archive_snapshot_files", confirm)
    monkeypatch.setattr(offload, "request_verification_now", request)

    result = offload.run_offload()

    assert result.status == "stalled"
    assert result.reason == expected_reason
    archive.assert_not_called()
    confirm.assert_not_called()
    assert request.called is request_expected
    assert _read_config(journal)["backup"]["last_offload"]["reason"] == expected_reason
    assert raw_path.exists()


@pytest.mark.parametrize(
    (
        "verification_status",
        "verification_reason",
        "last_ok_delta",
        "expected_reason",
        "request_expected",
        "archive_expected",
    ),
    [
        ("ok", None, offload.VERIFICATION_MAX_AGE_SECONDS, None, False, True),
        (
            "ok",
            None,
            offload.VERIFICATION_MAX_AGE_SECONDS + 1,
            "verification_overdue",
            True,
            False,
        ),
        (
            "error",
            "integrity_failed",
            offload.VERIFICATION_MAX_AGE_SECONDS + 1,
            "verification_failed",
            False,
            False,
        ),
        ("error", "locked", 60, None, False, True),
        ("error", "timeout", 60, None, False, True),
    ],
)
def test_verification_gate_uses_last_success_freshness_and_integrity_precedence(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    verification_status: str,
    verification_reason: str | None,
    last_ok_delta: int,
    expected_reason: str | None,
    request_expected: bool,
    archive_expected: bool,
) -> None:
    journal = _use_journal(tmp_path, monkeypatch)
    now = 1_800_000_000
    monkeypatch.setattr(offload.time, "time", lambda: now)
    _write_config(
        journal,
        _ready_config(
            now=now,
            verification_status=verification_status,
            verification_reason=verification_reason,
            last_ok_delta=last_ok_delta,
        ),
    )
    _make_segment(journal, content=b"abcdef")
    archive_calls, _confirm_calls = _install_successful_archive(monkeypatch)
    request = Mock(return_value=True)
    monkeypatch.setattr(offload, "request_verification_now", request)

    result = offload.run_offload()

    assert result.reason == expected_reason
    assert bool(archive_calls) is archive_expected
    assert request.called is request_expected


def test_dry_run_excludes_marked_segments_without_hashing_or_side_effects(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    journal = _use_journal(tmp_path, monkeypatch)
    now = 1_800_000_000
    _write_config(journal, _ready_config(now=now, budget_bytes=1))
    raw_path = _make_segment(journal, content=b"abcdef").joinpath("audio.wav")
    _make_segment(journal, segment="120100_300", content=b"new-media").joinpath(
        "audio.wav"
    )
    monkeypatch.setattr(
        offload.retention_executor,
        "marks",
        Mock(
            return_value={
                "ok": True,
                "verb": "marks",
                "marks": {
                    "version": 1,
                    "marks": {
                        "marked": {
                            "id": "marked",
                            "class": "offload_raw_release",
                            "target": {
                                "day": "20260101",
                                "stream": "default",
                                "dir": "120000_300",
                            },
                            "marked_at": "2026-01-01T00:00:00Z",
                            "proposal": {
                                "bytes": 6,
                                "reason": "restic-snapshot:archive-1",
                                "names": ["audio.wav"],
                            },
                            "state": "marked",
                        }
                    },
                },
            }
        ),
    )
    before_config = _config_path(journal).read_bytes()
    archive = Mock()
    confirm = Mock()
    request = Mock(return_value=True)

    def fail_hash():
        raise AssertionError("dry-run must not hash raw media")

    monkeypatch.setattr(offload, "run_archive_backup", archive)
    monkeypatch.setattr(offload, "check_archive_snapshot_files", confirm)
    monkeypatch.setattr(offload, "request_verification_now", request)
    monkeypatch.setattr(offload.hashlib, "sha256", fail_hash)

    result = offload.run_offload(dry_run=True)

    assert result.status == "ok"
    assert result.dry_run is True
    assert result.files_marked == 0
    assert result.bytes_marked == 0
    assert result.details == (
        offload.OffloadSegmentDetail(
            day="20260101",
            stream="default",
            segment="120100_300",
            files=1,
            bytes=9,
        ),
    )
    archive.assert_not_called()
    confirm.assert_not_called()
    request.assert_not_called()
    assert _config_path(journal).read_bytes() == before_config
    _assert_no_offload_side_effects(journal, raw_path)


def test_dry_run_reports_precondition_stall_without_request_or_record(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    journal = _use_journal(tmp_path, monkeypatch)
    now = 1_800_000_000
    config = _ready_config(now=now)
    config["backup"]["last_verification"]["last_ok_time"] = None
    _write_config(journal, config)
    before_config = _config_path(journal).read_bytes()
    request = Mock(return_value=True)
    monkeypatch.setattr(offload, "request_verification_now", request)

    result = offload.run_offload(dry_run=True)

    assert result.status == "stalled"
    assert result.reason == "verification_missing"
    request.assert_not_called()
    assert _config_path(journal).read_bytes() == before_config


def test_sparse_dry_run_measurement_uses_real_fixture_sizes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    journal = _use_journal(tmp_path, monkeypatch)
    now = 1_800_000_000
    _write_config(journal, _ready_config(now=now, budget_bytes=100 * GB))
    _make_segment(journal, size=137 * GB)
    archive = Mock()
    monkeypatch.setattr(offload, "run_archive_backup", archive)

    result = offload.run_offload(dry_run=True)

    assert result.status == "ok"
    assert result.details[0].bytes == 137 * GB
    archive.assert_not_called()


def test_success_appends_ledger_before_marking_and_reuses_digest_for_audit(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    journal = _use_journal(tmp_path, monkeypatch)
    now = 1_800_000_000
    _write_config(journal, _ready_config(now=now, budget_bytes=1))
    content = b"actual media bytes"
    seg_path = _make_segment(journal, content=content)
    raw_path = seg_path / "audio.wav"
    expected_digest = hashlib.sha256(content).hexdigest()
    events: list[tuple[str, str]] = []
    archive_calls, confirm_calls = _install_successful_archive(monkeypatch)
    real_append_jsonl = offload_ledger.append_jsonl

    def recording_append_jsonl(path: Path, record: Any) -> None:
        events.append(("ledger_append", record["segment"]))
        real_append_jsonl(path, record)

    mark_offload = Mock(
        return_value={
            "ok": True,
            "verb": "mark-offload",
            "marks": {"version": 1, "marks": {}},
        }
    )

    def recording_mark_offload(**kwargs: Any) -> dict[str, Any]:
        events.append(("mark", kwargs["segment_dir"]))
        return mark_offload(**kwargs)

    monkeypatch.setattr(offload_ledger, "append_jsonl", recording_append_jsonl)
    monkeypatch.setattr(
        offload.retention_executor, "mark_offload", recording_mark_offload
    )

    result = offload.run_offload()

    assert result.status == "ok"
    assert result.files_marked == 1
    assert result.bytes_marked == len(content)
    assert archive_calls == [(raw_path,)]
    assert confirm_calls == [{raw_path: len(content)}]
    assert events.index(("ledger_append", "120000_300")) < events.index(
        ("mark", "120000_300")
    )
    mark_offload.assert_called_once()
    assert mark_offload.call_args.kwargs["reason"] == "restic-snapshot:archive-1"
    assert mark_offload.call_args.kwargs["files"] == ["audio.wav"]
    offload.retention_executor.marks.assert_called_once_with(str(journal))
    assert raw_path.exists()
    assert (seg_path / "audio.jsonl").exists()
    assert (seg_path / "notes.jsonl").exists()
    assert (seg_path / "talents" / "keep.json").exists()

    summary = summarize_segment("20260101", "default", "120000_300")
    assert summary.currently_offloaded is True
    assert summary.files[0].sha256 == expected_digest
    audit_records = _pruning_records(journal)
    assert audit_records[0]["kind"] == "raw_media_offload"
    assert audit_records[0]["files"] == [
        {"name": "audio.wav", "bytes": len(content), "hash": expected_digest}
    ]
    assert audit_records[0]["bytes_marked"] == len(content)
    assert summary.files[0].sha256 == audit_records[0]["files"][0]["hash"]


@pytest.mark.parametrize(
    ("archive_result", "expected_reason"),
    [
        (
            engine.BackupResult(status="skipped", snapshot_id=None, error_reason=None),
            "backup_not_ready",
        ),
        (
            engine.BackupResult(
                status="error", snapshot_id=None, error_reason="locked"
            ),
            "locked",
        ),
        (
            engine.BackupResult(
                status="error", snapshot_id=None, error_reason="failed"
            ),
            "archive_failed",
        ),
    ],
)
def test_archive_failure_mapping_halts_before_confirm_or_marking(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    archive_result: engine.BackupResult,
    expected_reason: str,
) -> None:
    journal = _use_journal(tmp_path, monkeypatch)
    now = 1_800_000_000
    _write_config(journal, _ready_config(now=now, budget_bytes=1))
    raw_path = _make_segment(journal, content=b"abcdef").joinpath("audio.wav")
    monkeypatch.setattr(
        offload, "run_archive_backup", Mock(return_value=archive_result)
    )
    confirm = Mock()
    monkeypatch.setattr(offload, "check_archive_snapshot_files", confirm)

    result = offload.run_offload()

    assert result.status == "stalled"
    assert result.reason == expected_reason
    assert result.files_marked == 0
    assert result.bytes_marked == 0
    confirm.assert_not_called()
    offload.retention_executor.mark_offload.assert_not_called()
    _assert_no_offload_side_effects(journal, raw_path)


@pytest.mark.parametrize(
    ("confirm_case", "expected_reason"),
    [
        ("skipped", "backup_not_ready"),
        ("locked", "locked"),
        ("other_error", "confirm_tool_failed"),
        ("unconfirmed", "confirm_failed"),
    ],
)
def test_confirm_failure_mapping_halts_before_ledger_or_marking(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    confirm_case: str,
    expected_reason: str,
) -> None:
    journal = _use_journal(tmp_path, monkeypatch)
    now = 1_800_000_000
    _write_config(journal, _ready_config(now=now, budget_bytes=1))
    raw_path = _make_segment(journal, content=b"abcdef").joinpath("audio.wav")
    monkeypatch.setattr(
        offload,
        "run_archive_backup",
        Mock(
            return_value=engine.BackupResult(
                status="ok",
                snapshot_id="archive-1",
                error_reason=None,
            )
        ),
    )

    def fake_confirm(
        snapshot_id: str,
        expected_sizes: dict[Path, int],
    ) -> engine.ArchiveCheckResult:
        if confirm_case == "skipped":
            return engine.ArchiveCheckResult(
                status="skipped",
                error_reason=None,
                verdicts=None,
            )
        if confirm_case == "locked":
            return engine.ArchiveCheckResult(
                status="error",
                error_reason="locked",
                verdicts=None,
            )
        if confirm_case == "other_error":
            return engine.ArchiveCheckResult(
                status="error",
                error_reason="failed",
                verdicts=None,
            )
        path, size = next(iter(expected_sizes.items()))
        return engine.ArchiveCheckResult(
            status="ok",
            error_reason=None,
            verdicts=(
                engine.ArchiveFileVerdict(
                    path=str(path),
                    confirmed=False,
                    expected_size=size,
                    observed_size=size + 1,
                    snapshot_id=snapshot_id,
                ),
            ),
        )

    monkeypatch.setattr(offload, "check_archive_snapshot_files", fake_confirm)

    result = offload.run_offload()

    assert result.status == "stalled"
    assert result.reason == expected_reason
    assert result.files_marked == 0
    assert result.bytes_marked == 0
    offload.retention_executor.mark_offload.assert_not_called()
    _assert_no_offload_side_effects(journal, raw_path)


def test_partial_progress_stall_records_honest_counters_and_halts(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    journal = _use_journal(tmp_path, monkeypatch)
    now = 1_800_000_000
    prior_last_ok_time = 1_700_000_000
    _write_config(
        journal,
        _ready_config(
            now=now,
            budget_bytes=1,
            last_offload={
                "time": prior_last_ok_time,
                "status": "ok",
                "reason": None,
                "last_ok_time": prior_last_ok_time,
                "files_marked": 2,
                "bytes_marked": 10,
                "ran_out_of_markable_media": False,
            },
        ),
    )
    first = _make_segment(journal, segment="090000_300", content=b"first").joinpath(
        "audio.wav"
    )
    second = _make_segment(journal, segment="091000_300", content=b"second").joinpath(
        "audio.wav"
    )
    third = _make_segment(journal, segment="092000_300", content=b"third").joinpath(
        "audio.wav"
    )
    archive_calls: list[tuple[Path, ...]] = []

    def fake_archive(paths: list[Path]) -> engine.BackupResult:
        archive_calls.append(tuple(paths))
        if len(archive_calls) == 1:
            return engine.BackupResult(
                status="ok",
                snapshot_id="archive-1",
                error_reason=None,
            )
        if len(archive_calls) == 2:
            return engine.BackupResult(
                status="error",
                snapshot_id=None,
                error_reason="failed",
            )
        raise AssertionError("offload must halt after first per-segment failure")

    monkeypatch.setattr(offload, "run_archive_backup", fake_archive)
    monkeypatch.setattr(offload, "check_archive_snapshot_files", _ok_confirm)

    result = offload.run_offload()

    assert result.status == "stalled"
    assert result.reason == "archive_failed"
    assert result.files_marked == 1
    assert result.bytes_marked == len(b"first")
    assert archive_calls == [(first,), (second,)]
    assert first.exists()
    assert second.exists()
    assert third.exists()
    last_offload = _read_config(journal)["backup"]["last_offload"]
    assert last_offload["files_marked"] == 1
    assert last_offload["bytes_marked"] == len(b"first")
    assert last_offload["last_ok_time"] == prior_last_ok_time


def test_loop_stops_after_budget_met_using_already_and_newly_marked_bytes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    journal = _use_journal(tmp_path, monkeypatch)
    now = 1_800_000_000
    _write_config(journal, _ready_config(now=now, budget_bytes=6))
    first = _make_segment(journal, segment="090000_300", content=b"first").joinpath(
        "audio.wav"
    )
    second = _make_segment(journal, segment="091000_300", content=b"second!").joinpath(
        "audio.wav"
    )
    archive_calls, _confirm_calls = _install_successful_archive(monkeypatch)
    monkeypatch.setattr(
        offload.retention_executor,
        "marks",
        Mock(
            return_value={
                "ok": True,
                "verb": "marks",
                "marks": {
                    "version": 1,
                    "marks": {
                        "prior": {
                            "id": "prior",
                            "class": "offload_raw_release",
                            "target": {
                                "day": "20251231",
                                "stream": "default",
                                "dir": "090000_300",
                            },
                            "marked_at": "2025-12-31T00:00:00Z",
                            "proposal": {
                                "bytes": 2,
                                "reason": "restic-snapshot:prior",
                                "names": ["audio.wav"],
                            },
                            "state": "marked",
                        }
                    },
                },
            }
        ),
    )

    result = offload.run_offload()

    assert result.status == "ok"
    assert result.ran_out_of_markable_media is False
    assert result.bytes_already_marked == 2
    assert result.bytes_marked == len(b"first")
    assert archive_calls == [(first,)]
    assert first.exists()
    assert second.exists()


def test_segment_matching_an_existing_mark_is_skipped_before_archiving(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    journal = _use_journal(tmp_path, monkeypatch)
    now = 1_800_000_000
    _write_config(journal, _ready_config(now=now, budget_bytes=1))
    raw_path = _make_segment(journal, content=b"new-content").joinpath("audio.wav")
    _make_segment(journal, segment="120100_300", content=b"incomplete", complete=False)
    archive = Mock()
    monkeypatch.setattr(offload, "run_archive_backup", archive)
    monkeypatch.setattr(
        offload.retention_executor,
        "marks",
        Mock(
            return_value={
                "ok": True,
                "verb": "marks",
                "marks": {
                    "version": 1,
                    "marks": {
                        "marked": {
                            "id": "marked",
                            "class": "offload_raw_release",
                            "target": {
                                "day": "20260101",
                                "stream": "default",
                                "dir": "120000_300",
                            },
                            "marked_at": "2026-01-01T00:00:00Z",
                            "proposal": {
                                "bytes": len(b"new-content"),
                                "reason": "restic-snapshot:archive-1",
                                "names": ["audio.wav"],
                            },
                            "state": "marked",
                        }
                    },
                },
            }
        ),
    )

    result = offload.run_offload()

    assert result.status == "ok"
    assert result.ran_out_of_markable_media is True
    archive.assert_not_called()
    offload.retention_executor.mark_offload.assert_not_called()
    assert raw_path.exists()


def test_ledger_event_with_media_absent_is_skipped_without_reading_summary(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    journal = _use_journal(tmp_path, monkeypatch)
    now = 1_800_000_000
    _write_config(journal, _ready_config(now=now, budget_bytes=1))
    append_offload_event(
        day="20260101",
        stream="default",
        segment="120000_300",
        snapshot_id="old-snapshot",
        files=[
            OffloadFile(
                name="audio.wav",
                bytes=999,
                sha256="a" * 64,
            )
        ],
    )
    archive = Mock()
    monkeypatch.setattr(offload, "run_archive_backup", archive)
    summary_symbols = {"summarize_segment", "summarize_day", "summarize_journal"}
    assert summary_symbols.isdisjoint(vars(offload))
    for name in summary_symbols:
        monkeypatch.setattr(
            offload_ledger,
            name,
            Mock(side_effect=AssertionError("pass must not read ledger summaries")),
            raising=True,
        )

    result = offload.run_offload()

    assert result.status == "ok"
    assert result.details == ()
    archive.assert_not_called()


def test_gate_blocks_incomplete_and_failed_segments_without_archiving(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    journal = _use_journal(tmp_path, monkeypatch)
    now = 1_800_000_000
    _write_config(journal, _ready_config(now=now, budget_bytes=1))
    incomplete = _make_segment(
        journal,
        segment="090000_300",
        content=b"incomplete",
        complete=False,
    ).joinpath("audio.wav")
    failed = _make_segment(
        journal,
        segment="091000_300",
        content=b"failed",
        failed=True,
    ).joinpath("audio.wav")
    archive = Mock()
    monkeypatch.setattr(offload, "run_archive_backup", archive)

    result = offload.run_offload()

    assert result.status == "ok"
    assert result.ran_out_of_markable_media is True
    archive.assert_not_called()
    assert incomplete.exists()
    assert failed.exists()
    assert (
        _read_config(journal)["backup"]["last_offload"]["ran_out_of_markable_media"]
        is True
    )


def test_unexpected_exception_records_unexpected_error(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    journal = _use_journal(tmp_path, monkeypatch)
    now = 1_800_000_000
    _write_config(journal, _ready_config(now=now, budget_bytes=1))
    raw_path = _make_segment(journal, content=b"abcdef").joinpath("audio.wav")

    def fail_archive(paths: list[Path]) -> engine.BackupResult:
        raise RuntimeError("boom")

    monkeypatch.setattr(offload, "run_archive_backup", fail_archive)

    result = offload.run_offload()

    assert result.status == "stalled"
    assert result.reason == "unexpected_error"
    assert _read_config(journal)["backup"]["last_offload"]["reason"] == (
        "unexpected_error"
    )
    assert raw_path.exists()


def test_mark_failure_records_ledger_without_marking(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    journal = _use_journal(tmp_path, monkeypatch)
    now = 1_800_000_000
    _write_config(journal, _ready_config(now=now, budget_bytes=1))
    raw_path = _make_segment(journal, content=b"abcdef").joinpath("audio.wav")
    _install_successful_archive(monkeypatch)
    mark_offload = Mock(
        side_effect=offload.retention_executor.ExecutorUnavailable("boom")
    )
    monkeypatch.setattr(offload.retention_executor, "mark_offload", mark_offload)

    result = offload.run_offload()

    assert result.status == "stalled"
    assert result.reason == "unexpected_error"
    assert result.files_marked == 0
    assert result.bytes_marked == 0
    assert raw_path.exists()
    mark_offload.assert_called_once()
    assert offload.retention_executor.marks.return_value["marks"]["marks"] == {}
    summary = summarize_segment("20260101", "default", "120000_300")
    assert summary.currently_offloaded is True
    assert summary.snapshot_id == "archive-1"
    assert tuple(file.name for file in summary.files) == ("audio.wav",)


def test_run_prune_keeps_archive_tag_guard_on_forget(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    journal = _use_journal(tmp_path, monkeypatch)
    _write_config(journal, _ready_config(now=1_800_000_000))
    calls: list[list[str]] = []

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        calls.append(args)
        return ResticResult(
            returncode=0,
            stdout="",
            stderr="",
            json=[],
            argv=("restic", *args),
        )

    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "run_restic", fake_run_restic)

    result = engine.run_prune()

    assert result.status == "ok"
    forget_args = next(args for args in calls if args and args[0] == "forget")
    assert forget_args[forget_args.index("--keep-tag") + 1] == engine.ARCHIVE_TAG
