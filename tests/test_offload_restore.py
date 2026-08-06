# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Any
from unittest.mock import Mock

import pytest

from solstone.think import offload, offload_restore
from solstone.think.backup import engine
from solstone.think.backup.hosted import (
    HostedBinding,
    HostedCredentials,
    save_hosted_binding,
)
from solstone.think.backup.runner import ResticResult
from solstone.think.offload_ledger import (
    OffloadFile,
    append_offload_event,
    append_restore_event,
    ledger_path_for_day,
    summarize_day,
)
from solstone.think.retention import get_raw_media_files
from solstone.think.utils import DEFAULT_STREAM

DAY = "20260101"
SEGMENT = "090000_300"
CONTENT = b"audio-v1"
SHA = hashlib.sha256(CONTENT).hexdigest()


@pytest.fixture(autouse=True)
def _stub_offload_mark_resolution(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        offload_restore.retention_executor,
        "resolve_offload_mark",
        Mock(return_value={}),
    )


def _config_path(journal: Path) -> Path:
    return journal / "config" / "journal.json"


def _write_config(journal: Path, backup: dict[str, Any]) -> None:
    path = _config_path(journal)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"backup": backup}), encoding="utf-8")


def _backup_config(*, mode: str = "byo", enabled: bool = True) -> dict[str, Any]:
    backup: dict[str, Any] = {
        "enabled": enabled,
        "mode": mode,
        "daily_key": "daily-secret",
        "recovery_key": "R" * 64,
        "confirmed_recovery_key": True,
        "offload": {
            "enabled": True,
            "budget_bytes": 10_000_000_000,
            "floor_bytes": 1,
        },
    }
    if mode == "byo":
        backup["destination"] = {
            "repository": "s3:safe-bucket/path",
            "backend": "s3",
            "credentials": {
                "access_key_id": "access-key",
                "secret_access_key": "secret-key",
            },
        }
    return backup


def _offload_ready_config(*, budget_bytes: int = 1) -> dict[str, Any]:
    backup = _backup_config()
    backup["last_backup"] = {
        "time": 100,
        "snapshot_id": "full-snapshot",
        "status": "ok",
        "error_reason": None,
    }
    backup["last_verification"] = {
        "time": 100,
        "status": "ok",
        "reason": None,
        "last_ok_time": 2_000_000_000,
        "checked_subset": "all",
    }
    backup["offload"] = {
        "enabled": True,
        "budget_bytes": budget_bytes,
        "floor_bytes": None,
    }
    return backup


def _offload_file(name: str, content: bytes) -> OffloadFile:
    return OffloadFile(
        name=name,
        bytes=len(content),
        sha256=hashlib.sha256(content).hexdigest(),
    )


def _segment_dir(journal: Path, *, stream: str = DEFAULT_STREAM) -> Path:
    if stream == DEFAULT_STREAM:
        path = journal / "chronicle" / DAY / SEGMENT
    else:
        path = journal / "chronicle" / DAY / stream / SEGMENT
    path.mkdir(parents=True, exist_ok=True)
    return path


def _segment_dir_for(
    journal: Path,
    day: str,
    segment: str,
    *,
    stream: str = DEFAULT_STREAM,
) -> Path:
    if stream == DEFAULT_STREAM:
        path = journal / "chronicle" / day / segment
    else:
        path = journal / "chronicle" / day / stream / segment
    path.mkdir(parents=True, exist_ok=True)
    return path


def _seed_ledger(stream: str = DEFAULT_STREAM, *, snapshot_id: str = "snap1") -> None:
    append_offload_event(
        day=DAY,
        stream=stream,
        segment=SEGMENT,
        snapshot_id=snapshot_id,
        files=[OffloadFile(name="audio.wav", bytes=len(CONTENT), sha256=SHA)],
        time=100,
    )


def _restic_result(returncode: int, args: list[str]) -> ResticResult:
    return ResticResult(
        returncode=returncode,
        stdout="",
        stderr="",
        json=None,
        argv=tuple(args),
    )


def _binding() -> HostedBinding:
    return HostedBinding(
        broker_endpoint="https://broker.example",
        account_id="acct",
        instance_id="inst",
        bucket="bkt",
        prefix="users/acct/inst",
        broker_token="BTOKEN",
    )


def _creds() -> HostedCredentials:
    return HostedCredentials(
        access_key_id="AKID",
        secret_access_key="SAK",
        session_token="SESS",
        endpoint="https://acct.r2.cloudflarestorage.com",
        expires_at="2026-07-13T12:00:00Z",
    )


def test_restore_day_uses_daily_key_default_layout_and_no_pipeline(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _backup_config())
    segment_dir = _segment_dir(tmp_path)
    _seed_ledger()
    calls: list[tuple[list[str], dict[str, Any]]] = []
    fetch_hosted_credentials = Mock()
    callosum_send = Mock()

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        calls.append((args, kwargs))
        (segment_dir / "audio.wav").write_bytes(CONTENT)
        return _restic_result(0, args)

    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "fetch_hosted_credentials", fetch_hosted_credentials)
    monkeypatch.setattr(engine, "callosum_send", callosum_send)
    monkeypatch.setattr(offload_restore, "run_restic", fake_run_restic)
    monkeypatch.setattr(offload_restore, "device_free_bytes", lambda: 5_000_000_000)
    monkeypatch.setattr(offload_restore.time, "time", lambda: 200)

    result = offload_restore.restore_day(DAY)

    assert result.status == "ok"
    assert calls == [
        (
            [
                "restore",
                f"snap1:{segment_dir}",
                "--target",
                str(segment_dir),
                "--include",
                "/audio.wav",
            ],
            {
                "repository": "s3:safe-bucket/path",
                "password": "daily-secret",
                "restic_path": Path("/restic"),
                "backend_env": {
                    "AWS_ACCESS_KEY_ID": "access-key",
                    "AWS_SECRET_ACCESS_KEY": "secret-key",
                },
                "json": True,
                "timeout": offload_restore.OFFLOAD_RESTORE_TIMEOUT_SECONDS,
            },
        )
    ]
    assert "R" * 64 not in json.dumps(calls, default=str)
    fetch_hosted_credentials.assert_not_called()
    callosum_send.assert_not_called()
    assert json.loads(_config_path(tmp_path).read_text(encoding="utf-8"))["backup"][
        "last_restore"
    ] == {
        "time": 200,
        "status": "ok",
        "reason": None,
        "scope": "day",
        "day": DAY,
        "segments_selected": 1,
        "segments_restored": 1,
        "files_expected": 1,
        "files_restored": 1,
        "bytes_expected": len(CONTENT),
        "bytes_restored": len(CONTENT),
    }
    assert get_raw_media_files(segment_dir) == [segment_dir / "audio.wav"]


def test_restore_day_operated_uses_backup_scope_and_append_only_session(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _backup_config(mode="operated"))
    save_hosted_binding(_binding())
    segment_dir = _segment_dir(tmp_path, stream="camera")
    _seed_ledger("camera")
    captured_scopes: list[str] = []
    calls: list[list[str]] = []

    def fake_fetch(
        _binding: HostedBinding,
        *,
        scope: str,
    ) -> HostedCredentials:
        captured_scopes.append(scope)
        return _creds()

    def fake_run_restic(args: list[str], **_kwargs: Any) -> ResticResult:
        calls.append(args)
        (segment_dir / "audio.wav").write_bytes(CONTENT)
        return _restic_result(0, args)

    monkeypatch.setattr(engine, "fetch_hosted_credentials", fake_fetch)
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "ensure_rclone", Mock(return_value=Path("/rclone")))
    monkeypatch.setattr(offload_restore, "run_restic", fake_run_restic)
    monkeypatch.setattr(offload_restore, "device_free_bytes", lambda: 5_000_000_000)

    result = offload_restore.restore_day(DAY)

    assert result.status == "ok"
    assert captured_scopes == ["operated"]
    assert calls[0][:4] == [
        "-o",
        "rclone.program=/rclone",
        "-o",
        "rclone.args=serve restic --stdio --append-only --config /dev/null",
    ]
    assert calls[0][4:7] == ["restore", f"snap1:{segment_dir}", "--target"]


@pytest.mark.parametrize(
    ("returncode", "reason"),
    [
        (10, "repo_missing"),
        (12, "auth_failed"),
        (11, "locked"),
        (124, "timeout"),
        (77, "failed"),
    ],
)
def test_restore_day_maps_restic_returncode_reasons(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    returncode: int,
    reason: str,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _backup_config())
    _segment_dir(tmp_path)
    _seed_ledger()
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(offload_restore, "device_free_bytes", lambda: 5_000_000_000)
    monkeypatch.setattr(
        offload_restore,
        "run_restic",
        lambda args, **_kwargs: _restic_result(returncode, args),
    )

    result = offload_restore.restore_day(DAY)

    assert result.status == "error"
    assert result.reason == reason
    last_restore = json.loads(_config_path(tmp_path).read_text(encoding="utf-8"))[
        "backup"
    ]["last_restore"]
    assert last_restore["status"] == "error"
    assert last_restore["reason"] == reason


def test_missing_include_exit_zero_is_verified_as_error(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _backup_config())
    _segment_dir(tmp_path)
    _seed_ledger()
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(offload_restore, "device_free_bytes", lambda: 5_000_000_000)
    monkeypatch.setattr(
        offload_restore,
        "run_restic",
        lambda args, **_kwargs: _restic_result(0, args),
    )

    result = offload_restore.restore_day(DAY)

    assert result.status == "error"
    assert result.reason == "missing_file_after_restore"


def test_verification_failure_rolls_back_recorded_attempted_files(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _backup_config())
    segment_dir = _segment_dir(tmp_path)
    _seed_ledger()
    unrelated = segment_dir / "notes.txt"
    unrelated.write_text("keep me", encoding="utf-8")
    ledger_before = ledger_path_for_day(DAY).read_text(encoding="utf-8")

    def fake_run_restic(args: list[str], **_kwargs: Any) -> ResticResult:
        (segment_dir / "audio.wav").write_bytes(b"audio-v2")
        return _restic_result(0, args)

    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(offload_restore, "device_free_bytes", lambda: 5_000_000_000)
    monkeypatch.setattr(offload_restore, "run_restic", fake_run_restic)

    result = offload_restore.restore_day(DAY)

    assert result.status == "error"
    assert result.reason == "verification_failed"
    assert not (segment_dir / "audio.wav").exists()
    assert unrelated.read_text(encoding="utf-8") == "keep me"
    assert ledger_path_for_day(DAY).read_text(encoding="utf-8") == ledger_before


def test_tool_failure_rolls_back_remnant_before_later_offload_can_replace_ledger(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _offload_ready_config())
    segment_dir = _segment_dir(tmp_path)
    (segment_dir / "audio.jsonl").write_text(
        json.dumps({"raw": "audio.wav"})
        + "\n"
        + json.dumps({"start": "00:00:00", "text": "ok"})
        + "\n",
        encoding="utf-8",
    )
    recorded_files = (
        _offload_file("audio.wav", b"one"),
        _offload_file("call.wav", b"two"),
        _offload_file("room.wav", b"tre"),
    )
    append_offload_event(
        day=DAY,
        stream=DEFAULT_STREAM,
        segment=SEGMENT,
        snapshot_id="snap-original",
        files=recorded_files,
        time=100,
    )
    ledger_before = ledger_path_for_day(DAY).read_text(encoding="utf-8")

    def fake_restore(args: list[str], **_kwargs: Any) -> ResticResult:
        (segment_dir / "audio.wav").write_bytes(b"one")
        return _restic_result(77, args)

    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(offload_restore, "device_free_bytes", lambda: 5_000_000_000)
    monkeypatch.setattr(offload_restore, "run_restic", fake_restore)

    result = offload_restore.restore_day(DAY)

    assert result.status == "error"
    assert result.reason == "failed"
    assert not (segment_dir / "audio.wav").exists()
    assert ledger_path_for_day(DAY).read_text(encoding="utf-8") == ledger_before
    summary = summarize_day(DAY)
    assert summary.offloaded_file_count == 3
    assert tuple(file.name for file in summary.segments[0].files) == (
        "audio.wav",
        "call.wav",
        "room.wav",
    )

    archive_calls: list[list[Path]] = []

    def fake_archive(paths: list[Path]) -> engine.BackupResult:
        archive_calls.append(paths)
        return engine.BackupResult(
            status="ok",
            snapshot_id="snap-remnant",
            error_reason=None,
        )

    def fake_check(
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

    monkeypatch.setattr(offload, "run_archive_backup", fake_archive)
    monkeypatch.setattr(offload, "check_archive_snapshot_files", fake_check)
    monkeypatch.setattr(
        offload.retention_executor,
        "marks",
        Mock(return_value={"ok": True, "verb": "marks", "marks": {"marks": {}}}),
    )

    offload_result = offload.run_offload()

    assert offload_result.status == "ok"
    assert archive_calls == []
    assert ledger_path_for_day(DAY).read_text(encoding="utf-8") == ledger_before
    after = summarize_day(DAY)
    assert after.offloaded_file_count == 3
    assert tuple(file.name for file in after.segments[0].files) == (
        "audio.wav",
        "call.wav",
        "room.wav",
    )


def test_restore_only_requests_missing_files_and_preserves_preexisting_failure(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _backup_config())
    segment_dir = _segment_dir(tmp_path)
    recorded_files = (
        _offload_file("audio.wav", b"original"),
        _offload_file("call.wav", b"missing"),
    )
    append_offload_event(
        day=DAY,
        stream=DEFAULT_STREAM,
        segment=SEGMENT,
        snapshot_id="snap1",
        files=recorded_files,
        time=100,
    )
    original = segment_dir / "audio.wav"
    original.write_bytes(b"corrupted")
    restored = segment_dir / "call.wav"
    calls: list[list[str]] = []

    def fake_run_restic(args: list[str], **_kwargs: Any) -> ResticResult:
        calls.append(args)
        restored.write_bytes(b"missing")
        return _restic_result(0, args)

    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(offload_restore, "device_free_bytes", lambda: 5_000_000_000)
    monkeypatch.setattr(offload_restore, "run_restic", fake_run_restic)

    result = offload_restore.restore_day(DAY)

    assert result.status == "error"
    assert result.reason == "verification_failed"
    assert calls[0][-2:] == ["--include", "/call.wav"]
    assert "/audio.wav" not in calls[0]
    assert original.read_bytes() == b"corrupted"
    assert not restored.exists()
    offload_restore.retention_executor.resolve_offload_mark.assert_not_called()


def test_restore_all_present_files_skips_restic_and_resolves_mark(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _backup_config())
    segment_dir = _segment_dir(tmp_path)
    _seed_ledger()
    (segment_dir / "audio.wav").write_bytes(CONTENT)
    run_restic = Mock()
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(offload_restore, "device_free_bytes", lambda: 5_000_000_000)
    monkeypatch.setattr(offload_restore, "run_restic", run_restic)

    result = offload_restore.restore_day(DAY)

    assert result.status == "ok"
    run_restic.assert_not_called()
    offload_restore.retention_executor.resolve_offload_mark.assert_called_once_with(
        journal=str(tmp_path),
        day=DAY,
        segment_dir=SEGMENT,
        files=["audio.wav"],
        stream=DEFAULT_STREAM,
    )


def test_restore_all_degraded_after_partial_success_continues(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _backup_config())
    first = _segment_dir(tmp_path)
    second_segment = "091000_300"
    second = tmp_path / "chronicle" / DAY / second_segment
    second.mkdir(parents=True)
    append_offload_event(
        day=DAY,
        stream=DEFAULT_STREAM,
        segment=SEGMENT,
        snapshot_id="snap1",
        files=[OffloadFile(name="audio.wav", bytes=len(CONTENT), sha256=SHA)],
        time=100,
    )
    append_offload_event(
        day=DAY,
        stream=DEFAULT_STREAM,
        segment=second_segment,
        snapshot_id="snap2",
        files=[OffloadFile(name="audio.wav", bytes=len(CONTENT), sha256=SHA)],
        time=101,
    )
    call_count = 0

    def fake_run_restic(args: list[str], **_kwargs: Any) -> ResticResult:
        nonlocal call_count
        call_count += 1
        if call_count == 1:
            (first / "audio.wav").write_bytes(CONTENT)
        else:
            (second / "audio.wav").write_bytes(b"wrong")
        return _restic_result(0, args)

    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(offload_restore, "device_free_bytes", lambda: 5_000_000_000)
    monkeypatch.setattr(offload_restore, "run_restic", fake_run_restic)

    result = offload_restore.restore_all()

    assert result.status == "degraded"
    assert result.reason == "verification_failed"
    assert result.segments_restored == 1
    assert call_count == 2
    assert (first / "audio.wav").exists()
    assert not (second / "audio.wav").exists()
    last_restore = json.loads(_config_path(tmp_path).read_text(encoding="utf-8"))[
        "backup"
    ]["last_restore"]
    assert last_restore["status"] == "degraded"
    assert last_restore["reason"] == "verification_failed"


def test_restore_all_runs_oldest_first(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _backup_config())
    newer_day = "20260102"
    older_day = "20260101"
    newer_dir = _segment_dir_for(tmp_path, newer_day, SEGMENT)
    older_dir = _segment_dir_for(tmp_path, older_day, SEGMENT)
    append_offload_event(
        day=newer_day,
        stream=DEFAULT_STREAM,
        segment=SEGMENT,
        snapshot_id="snap-new",
        files=[OffloadFile(name="audio.wav", bytes=len(CONTENT), sha256=SHA)],
        time=100,
    )
    append_offload_event(
        day=older_day,
        stream=DEFAULT_STREAM,
        segment=SEGMENT,
        snapshot_id="snap-old",
        files=[OffloadFile(name="audio.wav", bytes=len(CONTENT), sha256=SHA)],
        time=101,
    )
    restored_targets: list[Path] = []

    def fake_run_restic(args: list[str], **_kwargs: Any) -> ResticResult:
        target = Path(args[args.index("--target") + 1])
        restored_targets.append(target)
        (target / "audio.wav").write_bytes(CONTENT)
        return _restic_result(0, args)

    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(offload_restore, "device_free_bytes", lambda: 5_000_000_000)
    monkeypatch.setattr(offload_restore, "run_restic", fake_run_restic)

    result = offload_restore.restore_all()

    assert result.status == "ok"
    assert restored_targets == [older_dir, newer_dir]


@pytest.mark.parametrize(
    ("free_bytes", "expected_status", "restic_calls"),
    [
        (3_000_000_000, "error", 1),
        (2_999_999_999, "refused", 0),
    ],
)
def test_restore_free_space_guard_checks_both_sides_of_boundary(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    free_bytes: int,
    expected_status: str,
    restic_calls: int,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _backup_config())
    _segment_dir(tmp_path)
    expected_bytes = 2_000_000_000
    assert expected_bytes + offload_restore.RESTORE_RESERVE_BYTES == 3_000_000_000
    append_offload_event(
        day=DAY,
        stream=DEFAULT_STREAM,
        segment=SEGMENT,
        snapshot_id="snap1",
        files=[OffloadFile(name="huge.wav", bytes=expected_bytes, sha256=SHA)],
        time=100,
    )
    run_restic = Mock(side_effect=lambda args, **_kwargs: _restic_result(77, args))
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(offload_restore, "device_free_bytes", lambda: free_bytes)
    monkeypatch.setattr(offload_restore, "run_restic", run_restic)

    result = offload_restore.restore_day(DAY)

    assert result.status == expected_status
    assert run_restic.call_count == restic_calls
    last_restore = json.loads(_config_path(tmp_path).read_text(encoding="utf-8"))[
        "backup"
    ]["last_restore"]
    assert last_restore["status"] == expected_status
    if expected_status == "refused":
        assert result.reason == "insufficient_free_space"
        assert last_restore["reason"] == "insufficient_free_space"
    else:
        assert result.reason == "failed"
        assert last_restore["reason"] == "failed"


def test_restore_all_refuses_when_sum_exceeds_guard_even_if_each_day_fits(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _backup_config())
    per_day_bytes = 2_000_000_000
    free_bytes = 3_000_000_000
    assert per_day_bytes + offload_restore.RESTORE_RESERVE_BYTES <= free_bytes
    assert per_day_bytes * 2 + offload_restore.RESTORE_RESERVE_BYTES > free_bytes
    for day in ("20260101", "20260102"):
        append_offload_event(
            day=day,
            stream=DEFAULT_STREAM,
            segment=SEGMENT,
            snapshot_id=f"snap-{day}",
            files=[OffloadFile(name="huge.wav", bytes=per_day_bytes, sha256=SHA)],
            time=100,
        )
    run_restic = Mock()
    monkeypatch.setattr(offload_restore, "device_free_bytes", lambda: free_bytes)
    monkeypatch.setattr(offload_restore, "run_restic", run_restic)

    result = offload_restore.restore_all()

    assert result.status == "refused"
    assert result.reason == "insufficient_free_space"
    assert result.segments_selected == 2
    run_restic.assert_not_called()


def test_restore_no_op_and_backup_not_ready_reasons(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _backup_config())
    run_restic = Mock()
    monkeypatch.setattr(offload_restore, "run_restic", run_restic)

    no_op = offload_restore.restore_day(DAY)

    assert no_op.status == "no_op"
    assert no_op.reason == "nothing_to_restore"
    run_restic.assert_not_called()
    last_restore = json.loads(_config_path(tmp_path).read_text(encoding="utf-8"))[
        "backup"
    ]["last_restore"]
    assert last_restore["status"] == "no_op"
    assert last_restore["reason"] == "nothing_to_restore"

    _segment_dir(tmp_path)
    _seed_ledger()
    append_restore_event(day=DAY, stream=DEFAULT_STREAM, segment=SEGMENT, time=101)

    already_restored = offload_restore.restore_day(DAY)

    assert already_restored.status == "no_op"
    assert already_restored.reason == "nothing_to_restore"
    run_restic.assert_not_called()

    _write_config(tmp_path, _backup_config(enabled=False))
    _seed_ledger()

    not_ready = offload_restore.restore_day(DAY)

    assert not_ready.status == "error"
    assert not_ready.reason == "backup_not_ready"
    last_restore = json.loads(_config_path(tmp_path).read_text(encoding="utf-8"))[
        "backup"
    ]["last_restore"]
    assert last_restore["status"] == "error"
    assert last_restore["reason"] == "backup_not_ready"


def test_restore_reports_segment_missing_without_restic(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _backup_config())
    _seed_ledger()
    run_restic = Mock()
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(offload_restore, "device_free_bytes", lambda: 5_000_000_000)
    monkeypatch.setattr(offload_restore, "run_restic", run_restic)

    result = offload_restore.restore_day(DAY)

    assert result.status == "error"
    assert result.reason == "segment_missing"
    run_restic.assert_not_called()


def test_restore_tool_unavailable_and_ledger_degraded_reasons(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _backup_config())
    _segment_dir(tmp_path)
    _seed_ledger()
    monkeypatch.setattr(offload_restore, "device_free_bytes", lambda: 5_000_000_000)
    monkeypatch.setattr(
        engine,
        "ensure_restic",
        Mock(side_effect=RuntimeError("missing")),
    )

    restic_unavailable = offload_restore.restore_day(DAY)

    assert restic_unavailable.reason == "restic_unavailable"

    ledger_path = tmp_path / "health" / "offload" / f"{DAY}.jsonl"
    ledger_path.write_bytes(b"\xff")

    degraded = offload_restore.restore_day(DAY)

    assert degraded.status == "error"
    assert degraded.reason == "ledger_degraded"


def test_skipped_record_degrades_and_refuses_restore(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _backup_config())
    _segment_dir(tmp_path)
    _seed_ledger()
    ledger_path = tmp_path / "health" / "offload" / f"{DAY}.jsonl"
    with open(ledger_path, "a", encoding="utf-8") as handle:
        handle.write("{bad\n")

    status = offload_restore.build_offload_status()

    assert status["backup_only"]["degraded"] is True
    assert status["days"][0]["degraded"] is True

    run_restic = Mock()
    monkeypatch.setattr(offload_restore, "run_restic", run_restic)
    monkeypatch.setattr(offload_restore, "device_free_bytes", lambda: 5_000_000_000)

    day_result = offload_restore.restore_day(DAY)

    assert day_result.status == "error"
    assert day_result.reason == "ledger_degraded"
    run_restic.assert_not_called()

    from solstone.think.backup.state import get_backup_config

    last_restore = get_backup_config()["last_restore"]
    assert last_restore["reason"] == "ledger_degraded"
    assert last_restore["status"] == "error"

    all_result = offload_restore.restore_all()

    assert all_result.status == "error"
    assert all_result.reason == "ledger_degraded"
    run_restic.assert_not_called()


def test_operated_restore_reports_rclone_unavailable(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _backup_config(mode="operated"))
    save_hosted_binding(_binding())
    _segment_dir(tmp_path)
    _seed_ledger()
    monkeypatch.setattr(offload_restore, "device_free_bytes", lambda: 5_000_000_000)
    monkeypatch.setattr(engine, "fetch_hosted_credentials", Mock(return_value=_creds()))
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(
        engine,
        "ensure_rclone",
        Mock(side_effect=RuntimeError("missing")),
    )

    result = offload_restore.restore_day(DAY)

    assert result.status == "error"
    assert result.reason == "rclone_unavailable"
