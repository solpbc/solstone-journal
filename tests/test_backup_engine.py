# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import logging
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from unittest.mock import Mock

import pytest

from solstone.think.backup import engine
from solstone.think.backup.hosted import (
    HostedBinding,
    HostedCredentials,
    HostedCredsUnavailable,
    load_hosted_binding,
    save_hosted_binding,
)
from solstone.think.backup.runner import ResticResult
from solstone.think.utils import parse_duration_seconds


def _config_path(journal: Path) -> Path:
    return journal / "config" / "journal.json"


def _write_config(journal: Path, payload: dict[str, Any]) -> None:
    config_path = _config_path(journal)
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text(json.dumps(payload), encoding="utf-8")


def _read_config(journal: Path) -> dict[str, Any]:
    return json.loads(_config_path(journal).read_text(encoding="utf-8"))


def _valid_backup_config(
    *,
    daily_key: str = "daily-secret",
    access_key_id: str = "access-key",
    secret_access_key: str = "secret-key",
    retention: dict[str, int] | None = None,
) -> dict[str, Any]:
    backup: dict[str, Any] = {
        "enabled": True,
        "destination": {
            "repository": "s3:safe-bucket/path",
            "backend": "s3",
            "credentials": {
                "access_key_id": access_key_id,
                "secret_access_key": secret_access_key,
            },
        },
        "daily_key": daily_key,
        "recovery_key": "R" * 64,
    }
    if retention is not None:
        backup["retention"] = retention
    return {"backup": backup}


def _restic_result(
    returncode: int,
    *,
    parsed_json: Any | None = None,
    args: list[str] | None = None,
    text: str = "",
) -> ResticResult:
    return ResticResult(
        returncode=returncode,
        stdout=text,
        stderr=text,
        json=parsed_json,
        argv=("restic", *(args or [])),
    )


def _utc_ts(
    year: int,
    month: int,
    day: int,
    hour: int = 12,
) -> float:
    return datetime(year, month, day, hour, tzinfo=timezone.utc).timestamp()


def test_verification_bucket_wraps_iso_week_53() -> None:
    assert engine._verification_bucket_for_iso_week(1) == 1
    assert engine._verification_bucket_for_iso_week(52) == 52
    assert engine._verification_bucket_for_iso_week(53) == 1


def test_verification_max_runtime_exceeds_subprocess_timeout() -> None:
    assert parse_duration_seconds(engine.VERIFY_MAX_RUNTIME) > (
        engine.VERIFY_TIMEOUT_SECONDS
    )


@pytest.mark.parametrize(
    "backup_config",
    [
        {"enabled": False},
        {
            "enabled": True,
            "daily_key": "daily-secret",
            "recovery_key": "R" * 64,
        },
        {
            "enabled": True,
            "destination": {
                "repository": "s3:safe-bucket/path",
                "backend": "s3",
                "credentials": {
                    "access_key_id": "access-key",
                    "secret_access_key": "secret-key",
                },
            },
        },
    ],
)
def test_run_backup_skips_when_runtime_guard_incomplete(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    backup_config: dict[str, Any],
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {"backup": backup_config})
    ensure_restic = Mock()
    run_restic = Mock()
    record_backup_result = Mock()
    monkeypatch.setattr(engine, "ensure_restic", ensure_restic)
    monkeypatch.setattr(engine, "run_restic", run_restic)
    monkeypatch.setattr(engine, "record_backup_result", record_backup_result)

    result = engine.run_backup()

    assert result == engine.BackupResult(
        status="skipped",
        snapshot_id=None,
        error_reason=None,
    )
    ensure_restic.assert_not_called()
    run_restic.assert_not_called()
    record_backup_result.assert_not_called()


@pytest.mark.parametrize(
    "backup_config",
    [
        {"enabled": False},
        {
            "enabled": True,
            "daily_key": "daily-secret",
            "recovery_key": "R" * 64,
        },
        {
            "enabled": True,
            "destination": {
                "repository": "s3:safe-bucket/path",
                "backend": "s3",
                "credentials": {
                    "access_key_id": "access-key",
                    "secret_access_key": "secret-key",
                },
            },
        },
    ],
)
def test_run_prune_skips_when_runtime_guard_incomplete(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    backup_config: dict[str, Any],
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {"backup": backup_config})
    ensure_restic = Mock()
    run_restic = Mock()
    record_backup_result = Mock()
    record_prune_result = Mock()
    monkeypatch.setattr(engine, "ensure_restic", ensure_restic)
    monkeypatch.setattr(engine, "run_restic", run_restic)
    monkeypatch.setattr(engine, "record_backup_result", record_backup_result)
    monkeypatch.setattr(engine, "record_prune_result", record_prune_result)

    result = engine.run_prune()

    assert result == engine.PruneResult(status="skipped", error_reason=None)
    ensure_restic.assert_not_called()
    run_restic.assert_not_called()
    record_backup_result.assert_not_called()
    record_prune_result.assert_not_called()


def test_run_backup_unlocks_then_calls_restic_with_expected_argv(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _valid_backup_config())
    calls: list[tuple[list[str], dict[str, Any]]] = []

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        calls.append((args, kwargs))
        if args == ["unlock"]:
            return _restic_result(0, args=args)
        return _restic_result(
            0,
            parsed_json={"message_type": "summary", "snapshot_id": "snap-1"},
            args=args,
        )

    record_backup_result = Mock()
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "run_restic", fake_run_restic)
    monkeypatch.setattr(engine, "record_backup_result", record_backup_result)
    monkeypatch.setattr(engine.time, "time", lambda: 1000.9)

    result = engine.run_backup()

    assert result == engine.BackupResult(
        status="ok",
        snapshot_id="snap-1",
        error_reason=None,
    )
    assert calls[0] == (
        ["unlock"],
        {
            "repository": "s3:safe-bucket/path",
            "password": "daily-secret",
            "restic_path": Path("/restic"),
            "backend_env": {
                "AWS_ACCESS_KEY_ID": "access-key",
                "AWS_SECRET_ACCESS_KEY": "secret-key",
            },
            "timeout": engine.UNLOCK_TIMEOUT_SECONDS,
        },
    )
    assert calls[1] == (
        [
            "backup",
            str(tmp_path),
            "--exclude",
            "*.sqlite*",
            "--exclude",
            "indexer",
            "--exclude",
            "cache",
            "--exclude",
            ".cache",
            "--exclude",
            ".removing_*",
            "--exclude",
            "*.sock",
            "--exclude",
            "*.pid",
            "--exclude",
            "*.port",
            "--exclude",
            "*.lock",
            "--exclude",
            "*.tmp",
            "--exclude",
            ".tmp*",
            "--exclude",
            "brain.json",
            "--exclude",
            "brain.log",
            "--exclude",
            "brain-fingerprint.key",
            "--exclude",
            "brain-refresh.lease",
            "--exclude",
            "supervisor.ready",
            "--exclude",
            "supervisor.start_time",
            "--exclude",
            "parakeet-cpp.placement",
            "--exclude",
            "scheduler.json",
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
            "timeout": engine.INITIAL_BACKUP_TIMEOUT_SECONDS,
        },
    )
    record_backup_result.assert_called_once_with(
        status="ok",
        time=1000,
        snapshot_id="snap-1",
        error_reason=None,
    )


@pytest.mark.parametrize(
    "parsed_json",
    [
        {"message_type": "summary", "snapshot_id": "snap-dict"},
        [
            {"message_type": "status", "percent_done": 50},
            {"message_type": "summary", "snapshot_id": "snap-list"},
        ],
    ],
)
def test_run_backup_selects_summary_from_dict_or_list_and_records_ok(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    parsed_json: Any,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _valid_backup_config())
    expected_snapshot_id = (
        parsed_json["snapshot_id"]
        if isinstance(parsed_json, dict)
        else parsed_json[-1]["snapshot_id"]
    )

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        if args == ["unlock"]:
            return _restic_result(0, args=args)
        return _restic_result(0, parsed_json=parsed_json, args=args)

    record_backup_result = Mock()
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "run_restic", fake_run_restic)
    monkeypatch.setattr(engine, "record_backup_result", record_backup_result)
    monkeypatch.setattr(engine.time, "time", lambda: 123)

    result = engine.run_backup()

    assert result == engine.BackupResult(
        status="ok",
        snapshot_id=expected_snapshot_id,
        error_reason=None,
    )
    record_backup_result.assert_called_once_with(
        status="ok",
        time=123,
        snapshot_id=expected_snapshot_id,
        error_reason=None,
    )


@pytest.mark.parametrize(
    ("returncode", "parsed_json", "expected_reason", "expected_snapshot_id"),
    [
        (
            3,
            {"message_type": "summary", "snapshot_id": "partial-snapshot"},
            "incomplete",
            "partial-snapshot",
        ),
        (10, None, "repo_missing", None),
        (11, None, "locked", None),
        (12, None, "auth_failed", None),
        (124, None, "timeout", None),
        (77, None, "failed", None),
        (0, None, "unknown", None),
    ],
)
def test_run_backup_failure_paths_record_sanitized_reasons(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    returncode: int,
    parsed_json: Any | None,
    expected_reason: str,
    expected_snapshot_id: str | None,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _valid_backup_config())

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        if args == ["unlock"]:
            return _restic_result(0, args=args)
        return _restic_result(
            returncode,
            parsed_json=parsed_json,
            args=args,
            text="raw-secret-output",
        )

    record_backup_result = Mock()
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "run_restic", fake_run_restic)
    monkeypatch.setattr(engine, "record_backup_result", record_backup_result)
    monkeypatch.setattr(engine.time, "time", lambda: 456)

    result = engine.run_backup()

    assert result == engine.BackupResult(
        status="error",
        snapshot_id=expected_snapshot_id,
        error_reason=expected_reason,
    )
    record_backup_result.assert_called_once_with(
        status="error",
        time=456,
        snapshot_id=expected_snapshot_id,
        error_reason=expected_reason,
    )
    assert "raw-secret-output" != result.error_reason


def test_run_backup_restic_unavailable_records_error(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _valid_backup_config())
    run_restic = Mock()
    record_backup_result = Mock()
    monkeypatch.setattr(
        engine,
        "ensure_restic",
        Mock(side_effect=RuntimeError("download failed")),
    )
    monkeypatch.setattr(engine, "run_restic", run_restic)
    monkeypatch.setattr(engine, "record_backup_result", record_backup_result)
    monkeypatch.setattr(engine.time, "time", lambda: 789)

    result = engine.run_backup()

    assert result == engine.BackupResult(
        status="error",
        snapshot_id=None,
        error_reason="restic_unavailable",
    )
    run_restic.assert_not_called()
    record_backup_result.assert_called_once_with(
        status="error",
        time=789,
        snapshot_id=None,
        error_reason="restic_unavailable",
    )


def test_operated_backup_rclone_unavailable_records_error(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(
        tmp_path,
        {
            "backup": {
                "enabled": True,
                "mode": "operated",
                "daily_key": "dk",
                "recovery_key": "R" * 64,
            }
        },
    )
    save_hosted_binding(
        HostedBinding(
            broker_endpoint="https://broker.example",
            account_id="acct",
            instance_id="inst",
            bucket="bkt",
            prefix="users/acct/inst",
            broker_token="BTOKEN",
        )
    )
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(
        engine,
        "ensure_rclone",
        Mock(side_effect=RuntimeError("download failed")),
    )
    monkeypatch.setattr(
        engine,
        "fetch_hosted_credentials",
        Mock(
            return_value=HostedCredentials(
                access_key_id="AKID",
                secret_access_key="SAK",
                session_token="SESS",
                endpoint="https://acct.r2.cloudflarestorage.com",
                expires_at="2026-07-13T12:00:00Z",
            )
        ),
    )
    run_restic = Mock()
    record_backup_result = Mock()
    monkeypatch.setattr(engine, "run_restic", run_restic)
    monkeypatch.setattr(engine, "record_backup_result", record_backup_result)
    monkeypatch.setattr(engine.time, "time", lambda: 790)

    result = engine.run_backup()

    assert result == engine.BackupResult(
        status="error",
        snapshot_id=None,
        error_reason="rclone_unavailable",
    )
    run_restic.assert_not_called()
    record_backup_result.assert_called_once_with(
        status="error",
        time=790,
        snapshot_id=None,
        error_reason="rclone_unavailable",
    )


def test_run_prune_unlocks_then_forgets_with_prune_and_repack_bound(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(
        tmp_path,
        _valid_backup_config(
            retention={"hourly": 2, "daily": 3, "weekly": 4, "monthly": 5}
        ),
    )
    calls: list[tuple[list[str], dict[str, Any]]] = []

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        calls.append((args, kwargs))
        return _restic_result(0, args=args)

    record_prune_result = Mock()
    record_backup_result = Mock()
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "run_restic", fake_run_restic)
    monkeypatch.setattr(engine, "record_prune_result", record_prune_result)
    monkeypatch.setattr(engine, "record_backup_result", record_backup_result)
    monkeypatch.setattr(engine.time, "time", lambda: 2000.9)

    result = engine.run_prune()

    assert result == engine.PruneResult(status="ok", error_reason=None)
    assert calls[0][0] == ["unlock"]
    assert calls[0][1]["timeout"] == engine.UNLOCK_TIMEOUT_SECONDS
    forget_call = next(call for call in calls if call[0][0] == "forget")
    assert forget_call == (
        [
            "forget",
            "--keep-hourly",
            "2",
            "--keep-daily",
            "3",
            "--keep-weekly",
            "4",
            "--keep-monthly",
            "5",
            "--keep-tag",
            engine.ARCHIVE_TAG,
            "--prune",
        ],
        {
            "repository": "s3:safe-bucket/path",
            "password": "daily-secret",
            "restic_path": Path("/restic"),
            "backend_env": {
                "AWS_ACCESS_KEY_ID": "access-key",
                "AWS_SECRET_ACCESS_KEY": "secret-key",
            },
            "timeout": engine.PRUNE_TIMEOUT_SECONDS,
            "max_repack_size": engine.PRUNE_MAX_REPACK_SIZE,
        },
    )
    assert "json" not in forget_call[1]
    record_prune_result.assert_called_once_with(
        status="ok",
        time=2000,
        error_reason=None,
    )
    record_backup_result.assert_not_called()


def test_run_prune_failure_records_last_prune_not_last_backup(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _valid_backup_config())

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        if args == ["unlock"]:
            return _restic_result(0, args=args)
        return _restic_result(11, args=args, text="raw-secret-output")

    record_prune_result = Mock()
    record_backup_result = Mock()
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "run_restic", fake_run_restic)
    monkeypatch.setattr(engine, "record_prune_result", record_prune_result)
    monkeypatch.setattr(engine, "record_backup_result", record_backup_result)
    monkeypatch.setattr(engine.time, "time", lambda: 3000)

    result = engine.run_prune()

    assert result == engine.PruneResult(status="error", error_reason="locked")
    record_prune_result.assert_called_once_with(
        status="error",
        time=3000,
        error_reason="locked",
    )
    record_backup_result.assert_not_called()


def test_run_prune_restic_unavailable_records_last_prune(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _valid_backup_config())
    run_restic = Mock()
    record_prune_result = Mock()
    monkeypatch.setattr(
        engine,
        "ensure_restic",
        Mock(side_effect=RuntimeError("download failed")),
    )
    monkeypatch.setattr(engine, "run_restic", run_restic)
    monkeypatch.setattr(engine, "record_prune_result", record_prune_result)
    monkeypatch.setattr(engine.time, "time", lambda: 4000)

    result = engine.run_prune()

    assert result == engine.PruneResult(
        status="error",
        error_reason="restic_unavailable",
    )
    run_restic.assert_not_called()
    record_prune_result.assert_called_once_with(
        status="error",
        time=4000,
        error_reason="restic_unavailable",
    )


def test_run_verification_uses_distinct_utc_week_buckets_in_argv(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _valid_backup_config())
    calls: list[tuple[list[str], dict[str, Any]]] = []
    all_calls: list[tuple[list[str], dict[str, Any]]] = []

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        calls.append((args, kwargs))
        all_calls.append((args, kwargs))
        return _restic_result(0, args=args)

    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "run_restic", fake_run_restic)

    monkeypatch.setattr(engine.time, "time", lambda: _utc_ts(2026, 1, 5))
    first = engine.run_verification()
    first_call = next(call for call in calls if call[0][0] == "check")
    first_args = first_call[0]
    first_subset = first_args[first_args.index("--read-data-subset") + 1]
    calls.clear()

    monkeypatch.setattr(engine.time, "time", lambda: _utc_ts(2026, 1, 12))
    second = engine.run_verification()
    second_call = next(call for call in calls if call[0][0] == "check")
    second_args = second_call[0]
    second_subset = second_args[second_args.index("--read-data-subset") + 1]

    assert first.status == "ok"
    assert second.status == "ok"
    assert first_subset != second_subset
    assert "--no-lock" not in first_args
    assert "--retry-lock" not in first_args
    assert not any("unlock" in args for args, _kwargs in all_calls)
    assert second_call[1]["timeout"] == engine.VERIFY_TIMEOUT_SECONDS


def test_run_verification_week_53_records_valid_subset(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _valid_backup_config())
    calls: list[tuple[list[str], dict[str, Any]]] = []
    now = int(_utc_ts(2026, 12, 31))

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        calls.append((args, kwargs))
        return _restic_result(0, args=args)

    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "run_restic", fake_run_restic)
    monkeypatch.setattr(engine.time, "time", lambda: now)

    result = engine.run_verification()

    check_call = next(call for call in calls if call[0][0] == "check")
    args = check_call[0]
    subset = args[args.index("--read-data-subset") + 1]
    bucket, total = subset.split("/", 1)
    assert result == engine.VerificationResult(
        status="ok",
        reason=None,
        checked_subset="1/52",
    )
    assert subset == "1/52"
    assert total == "52"
    assert 1 <= int(bucket) <= 52
    assert bucket != "0"
    assert _read_config(tmp_path)["backup"]["last_verification"] == {
        "time": now,
        "status": "ok",
        "reason": None,
        "last_ok_time": now,
        "checked_subset": "1/52",
    }


@pytest.mark.parametrize(
    ("returncode", "expected_reason"),
    [
        (1, "integrity_failed"),
        (3, "failed"),
        (10, "repo_missing"),
        (11, "locked"),
        (12, "auth_failed"),
        (124, "timeout"),
        (77, "failed"),
    ],
)
def test_run_verification_failure_paths_record_sanitized_reasons(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    returncode: int,
    expected_reason: str,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _valid_backup_config())

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        return _restic_result(returncode, args=args, text="raw-secret-output")

    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "run_restic", fake_run_restic)
    monkeypatch.setattr(engine.time, "time", lambda: 7000)

    result = engine.run_verification()

    assert result == engine.VerificationResult(
        status="error",
        reason=expected_reason,
        checked_subset=None,
    )
    assert _read_config(tmp_path)["backup"]["last_verification"] == {
        "time": 7000,
        "status": "error",
        "reason": expected_reason,
        "last_ok_time": None,
        "checked_subset": None,
    }
    assert "raw-secret-output" != result.reason


def test_run_verification_skip_leaves_config_bytes_identical(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    config = _valid_backup_config()
    config["backup"]["enabled"] = False
    config["backup"]["last_verification"] = {
        "time": 100,
        "status": "ok",
        "reason": None,
        "last_ok_time": 100,
        "checked_subset": "7/52",
    }
    _write_config(tmp_path, config)
    before = _config_path(tmp_path).read_bytes()
    ensure_restic = Mock()
    run_restic = Mock()
    record_verification_result = Mock()
    monkeypatch.setattr(engine, "ensure_restic", ensure_restic)
    monkeypatch.setattr(engine, "run_restic", run_restic)
    monkeypatch.setattr(
        engine,
        "record_verification_result",
        record_verification_result,
    )

    result = engine.run_verification()

    assert result == engine.VerificationResult(
        status="skipped",
        reason=None,
        checked_subset=None,
    )
    assert _config_path(tmp_path).read_bytes() == before
    ensure_restic.assert_not_called()
    run_restic.assert_not_called()
    record_verification_result.assert_not_called()


def test_run_verification_does_not_modify_backup_prune_or_offload_state(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    config = _valid_backup_config()
    config["backup"]["last_backup"] = {
        "time": 1,
        "snapshot_id": "snap-1",
        "status": "ok",
        "error_reason": None,
    }
    config["backup"]["last_prune"] = {
        "time": 2,
        "status": "ok",
        "error_reason": None,
    }
    config["backup"]["offload"] = {
        "enabled": True,
        "budget_bytes": 100,
        "floor_bytes": 50,
    }
    config["backup"]["last_offload"] = {
        "time": 3,
        "status": "stalled",
        "reason": "backup_not_ready",
        "last_ok_time": 2,
        "files_offloaded": 0,
        "bytes_offloaded": 0,
        "ran_out_of_media": False,
    }
    _write_config(tmp_path, config)
    ledger_path = tmp_path / "health" / "offload" / "20260101.jsonl"
    ledger_path.parent.mkdir(parents=True)
    ledger_bytes = b'{"event_kind":"offload"}\n'
    ledger_path.write_bytes(ledger_bytes)

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        return _restic_result(0, args=args)

    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "run_restic", fake_run_restic)
    monkeypatch.setattr(engine.time, "time", lambda: 8000)

    result = engine.run_verification()

    backup = _read_config(tmp_path)["backup"]
    assert result.status == "ok"
    assert backup["last_backup"] == config["backup"]["last_backup"]
    assert backup["last_prune"] == config["backup"]["last_prune"]
    assert backup["offload"] == config["backup"]["offload"]
    assert backup["last_offload"] == config["backup"]["last_offload"]
    assert ledger_path.read_bytes() == ledger_bytes


def test_operated_verification_uses_backup_scope_and_append_only_adapter(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(
        tmp_path,
        {
            "backup": {
                "enabled": True,
                "mode": "operated",
                "daily_key": "dk",
                "recovery_key": "R" * 64,
            }
        },
    )
    save_hosted_binding(
        HostedBinding(
            broker_endpoint="https://broker.example",
            account_id="acct",
            instance_id="inst",
            bucket="bkt",
            prefix="users/acct/inst",
            broker_token="BTOKEN",
        )
    )
    captured_scopes: list[str] = []
    calls: list[tuple[list[str], dict[str, Any]]] = []

    def fake_fetch(
        _binding: HostedBinding,
        *,
        scope: str,
    ) -> HostedCredentials:
        captured_scopes.append(scope)
        return HostedCredentials(
            access_key_id="AKID",
            secret_access_key="SAK",
            session_token="SESS",
            endpoint="https://acct.r2.cloudflarestorage.com",
            expires_at="2026-07-13T12:00:00Z",
        )

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        calls.append((args, kwargs))
        return _restic_result(0, args=args)

    monkeypatch.setattr(engine, "fetch_hosted_credentials", fake_fetch)
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "ensure_rclone", Mock(return_value=Path("/rclone")))
    monkeypatch.setattr(engine, "run_restic", fake_run_restic)
    monkeypatch.setattr(engine.time, "time", lambda: 9000)

    result = engine.run_verification()

    assert result.status == "ok"
    assert captured_scopes == ["operated"]
    check_call = next(call for call in calls if "check" in call[0])
    assert check_call[0][:4] == [
        "-o",
        "rclone.program=/rclone",
        "-o",
        "rclone.args=serve restic --stdio --append-only --config /dev/null",
    ]
    assert check_call[0][4:6] == ["check", "--read-data-subset"]
    assert check_call[0][6].endswith("/52")
    assert sum(1 for args, _kwargs in calls if "unlock" in args) == 0


def test_run_verification_restic_unavailable_records_error(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _valid_backup_config())
    run_restic = Mock()
    monkeypatch.setattr(
        engine,
        "ensure_restic",
        Mock(side_effect=RuntimeError("download failed")),
    )
    monkeypatch.setattr(engine, "run_restic", run_restic)
    monkeypatch.setattr(engine.time, "time", lambda: 9100)

    result = engine.run_verification()

    assert result == engine.VerificationResult(
        status="error",
        reason="restic_unavailable",
        checked_subset=None,
    )
    run_restic.assert_not_called()
    assert _read_config(tmp_path)["backup"]["last_verification"]["reason"] == (
        "restic_unavailable"
    )


def test_run_verification_malformed_backend_records_failed_without_restic(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    config = _valid_backup_config()
    del config["backup"]["destination"]["credentials"]["secret_access_key"]
    _write_config(tmp_path, config)
    run_restic = Mock()
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "run_restic", run_restic)
    monkeypatch.setattr(engine.time, "time", lambda: 9200)

    result = engine.run_verification()

    assert result == engine.VerificationResult(
        status="error",
        reason="failed",
        checked_subset=None,
    )
    run_restic.assert_not_called()
    assert _read_config(tmp_path)["backup"]["last_verification"]["time"] == 9200


def test_operated_verification_records_hosted_credential_error_without_restic(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(
        tmp_path,
        {
            "backup": {
                "enabled": True,
                "mode": "operated",
                "daily_key": "dk",
                "recovery_key": "R" * 64,
            }
        },
    )
    save_hosted_binding(
        HostedBinding(
            broker_endpoint="https://broker.example",
            account_id="acct",
            instance_id="inst",
            bucket="bkt",
            prefix="users/acct/inst",
            broker_token="BTOKEN",
        )
    )
    ensure_restic = Mock(return_value=Path("/restic"))
    run_restic = Mock()
    monkeypatch.setattr(
        engine,
        "fetch_hosted_credentials",
        Mock(side_effect=HostedCredsUnavailable("broker_unreachable")),
    )
    monkeypatch.setattr(engine, "ensure_restic", ensure_restic)
    monkeypatch.setattr(engine, "run_restic", run_restic)
    monkeypatch.setattr(engine.time, "time", lambda: 9300)

    result = engine.run_verification()

    assert result.reason == "broker_unreachable"
    ensure_restic.assert_not_called()
    run_restic.assert_not_called()
    assert _read_config(tmp_path)["backup"]["last_verification"]["time"] == 9300


def test_operated_verification_rclone_unavailable_records_error(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(
        tmp_path,
        {
            "backup": {
                "enabled": True,
                "mode": "operated",
                "daily_key": "dk",
                "recovery_key": "R" * 64,
            }
        },
    )
    save_hosted_binding(
        HostedBinding(
            broker_endpoint="https://broker.example",
            account_id="acct",
            instance_id="inst",
            bucket="bkt",
            prefix="users/acct/inst",
            broker_token="BTOKEN",
        )
    )
    run_restic = Mock()
    monkeypatch.setattr(
        engine,
        "fetch_hosted_credentials",
        Mock(
            return_value=HostedCredentials(
                access_key_id="AKID",
                secret_access_key="SAK",
                session_token="SESS",
                endpoint="https://acct.r2.cloudflarestorage.com",
                expires_at="2026-07-13T12:00:00Z",
            )
        ),
    )
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(
        engine,
        "ensure_rclone",
        Mock(side_effect=RuntimeError("download failed")),
    )
    monkeypatch.setattr(engine, "run_restic", run_restic)
    monkeypatch.setattr(engine.time, "time", lambda: 9400)

    result = engine.run_verification()

    assert result.reason == "rclone_unavailable"
    run_restic.assert_not_called()
    assert _read_config(tmp_path)["backup"]["last_verification"]["time"] == 9400


def test_archive_backup_uses_explicit_targets_and_tag_matches_prune_keep_tag(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(
        tmp_path,
        _valid_backup_config(
            retention={"hourly": 2, "daily": 3, "weekly": 4, "monthly": 5}
        ),
    )
    before_config = _config_path(tmp_path).read_bytes()
    targets = [Path("/archive/a.mov"), Path("/archive/b.wav")]
    calls: list[tuple[list[str], dict[str, Any]]] = []

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        calls.append((args, kwargs))
        if args == ["unlock"]:
            return _restic_result(0, args=args)
        if "backup" in args:
            return _restic_result(
                0,
                parsed_json={
                    "message_type": "summary",
                    "snapshot_id": "archive-snap",
                },
                args=args,
            )
        return _restic_result(0, args=args)

    record_backup_result = Mock()
    record_prune_result = Mock()
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "run_restic", fake_run_restic)
    monkeypatch.setattr(engine, "record_backup_result", record_backup_result)
    monkeypatch.setattr(engine, "record_prune_result", record_prune_result)
    monkeypatch.setattr(engine.time, "time", lambda: 7000)

    archive_result = engine.run_archive_backup(targets)

    assert archive_result == engine.BackupResult(
        status="ok",
        snapshot_id="archive-snap",
        error_reason=None,
    )
    assert calls[0][0] == ["unlock"]
    archive_call = next(call for call in calls if "backup" in call[0])
    archive_args = archive_call[0]
    assert archive_call == (
        [
            "--retry-lock",
            engine.ARCHIVE_RETRY_LOCK,
            "backup",
            str(targets[0]),
            str(targets[1]),
            "--tag",
            engine.ARCHIVE_TAG,
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
            "timeout": engine.ARCHIVE_BACKUP_TIMEOUT_SECONDS,
        },
    )
    assert str(tmp_path) not in archive_args
    assert _config_path(tmp_path).read_bytes() == before_config
    record_backup_result.assert_not_called()
    record_prune_result.assert_not_called()

    prune_result = engine.run_prune()

    assert prune_result == engine.PruneResult(status="ok", error_reason=None)
    forget_call = next(call for call in calls if call[0][0] == "forget")
    forget_args = forget_call[0]
    archive_tag = archive_args[archive_args.index("--tag") + 1]
    prune_tag = forget_args[forget_args.index("--keep-tag") + 1]
    assert prune_tag == archive_tag
    assert archive_tag == engine.ARCHIVE_TAG
    assert _config_path(tmp_path).read_bytes() == before_config
    record_backup_result.assert_not_called()


@pytest.mark.parametrize(
    ("returncode", "parsed_json", "expected_reason"),
    [
        (0, None, "unknown"),
        (
            3,
            {"message_type": "summary", "snapshot_id": "partial-archive"},
            "incomplete",
        ),
        (11, None, "locked"),
        (77, None, "failed"),
    ],
)
def test_archive_backup_failures_do_not_record_or_persist(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    returncode: int,
    parsed_json: Any | None,
    expected_reason: str,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _valid_backup_config())
    before_config = _config_path(tmp_path).read_bytes()
    calls: list[tuple[list[str], dict[str, Any]]] = []

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        calls.append((args, kwargs))
        if args == ["unlock"]:
            return _restic_result(0, args=args)
        return _restic_result(returncode, parsed_json=parsed_json, args=args)

    record_backup_result = Mock()
    record_prune_result = Mock()
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "run_restic", fake_run_restic)
    monkeypatch.setattr(engine, "record_backup_result", record_backup_result)
    monkeypatch.setattr(engine, "record_prune_result", record_prune_result)

    result = engine.run_archive_backup([Path("/archive/a.mov")])

    assert result == engine.BackupResult(
        status="error",
        snapshot_id=None,
        error_reason=expected_reason,
    )
    assert calls[0][0] == ["unlock"]
    assert calls[1][0] == [
        "--retry-lock",
        engine.ARCHIVE_RETRY_LOCK,
        "backup",
        "/archive/a.mov",
        "--tag",
        engine.ARCHIVE_TAG,
    ]
    assert calls[1][1]["json"] is True
    assert calls[1][1]["timeout"] == engine.ARCHIVE_BACKUP_TIMEOUT_SECONDS
    assert _config_path(tmp_path).read_bytes() == before_config
    record_backup_result.assert_not_called()
    record_prune_result.assert_not_called()


@pytest.mark.parametrize(
    "backup_config",
    [
        {"enabled": False},
        {
            "enabled": True,
            "daily_key": "daily-secret",
            "recovery_key": "R" * 64,
        },
        {
            "enabled": True,
            "destination": {
                "repository": "s3:safe-bucket/path",
                "backend": "s3",
                "credentials": {
                    "access_key_id": "access-key",
                    "secret_access_key": "secret-key",
                },
            },
        },
    ],
)
def test_archive_backup_skips_when_runtime_guard_incomplete(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    backup_config: dict[str, Any],
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {"backup": backup_config})
    before_config = _config_path(tmp_path).read_bytes()
    ensure_restic = Mock()
    run_restic = Mock()
    record_backup_result = Mock()
    record_prune_result = Mock()
    monkeypatch.setattr(engine, "ensure_restic", ensure_restic)
    monkeypatch.setattr(engine, "run_restic", run_restic)
    monkeypatch.setattr(engine, "record_backup_result", record_backup_result)
    monkeypatch.setattr(engine, "record_prune_result", record_prune_result)

    result = engine.run_archive_backup([Path("/archive/a.mov")])

    assert result == engine.BackupResult(
        status="skipped",
        snapshot_id=None,
        error_reason=None,
    )
    ensure_restic.assert_not_called()
    run_restic.assert_not_called()
    record_backup_result.assert_not_called()
    record_prune_result.assert_not_called()
    assert _config_path(tmp_path).read_bytes() == before_config


@pytest.mark.parametrize(
    ("config", "ensure_restic", "expected_reason"),
    [
        (
            _valid_backup_config(),
            Mock(side_effect=RuntimeError("download failed")),
            "restic_unavailable",
        ),
        (
            {
                "backup": {
                    "enabled": True,
                    "destination": {
                        "repository": "s3:safe-bucket/path",
                        "backend": "s3",
                        "credentials": {"access_key_id": "access-key"},
                    },
                    "daily_key": "daily-secret",
                    "recovery_key": "R" * 64,
                }
            },
            Mock(return_value=Path("/restic")),
            "failed",
        ),
    ],
)
def test_archive_backup_resolution_errors_do_not_record_or_persist(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    config: dict[str, Any],
    ensure_restic: Mock,
    expected_reason: str,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, config)
    before_config = _config_path(tmp_path).read_bytes()
    run_restic = Mock()
    record_backup_result = Mock()
    record_prune_result = Mock()
    monkeypatch.setattr(engine, "ensure_restic", ensure_restic)
    monkeypatch.setattr(engine, "run_restic", run_restic)
    monkeypatch.setattr(engine, "record_backup_result", record_backup_result)
    monkeypatch.setattr(engine, "record_prune_result", record_prune_result)

    result = engine.run_archive_backup([Path("/archive/a.mov")])

    assert result == engine.BackupResult(
        status="error",
        snapshot_id=None,
        error_reason=expected_reason,
    )
    run_restic.assert_not_called()
    record_backup_result.assert_not_called()
    record_prune_result.assert_not_called()
    assert _config_path(tmp_path).read_bytes() == before_config


def test_check_archive_snapshot_files_confirms_exact_size_and_uses_header_id(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _valid_backup_config())
    before_config = _config_path(tmp_path).read_bytes()
    target = Path("/archive/file.wav")
    full_snapshot_id = "abc123fullsnapshotid"
    calls: list[tuple[list[str], dict[str, Any]]] = []

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        calls.append((args, kwargs))
        return _restic_result(
            0,
            parsed_json=[
                {
                    "message_type": "snapshot",
                    "struct_type": "snapshot",
                    "id": full_snapshot_id,
                },
                {
                    "message_type": "node",
                    "struct_type": "node",
                    "path": str(target),
                    "name": target.name,
                    "type": "file",
                    "size": 42,
                },
            ],
            args=args,
        )

    record_backup_result = Mock()
    record_prune_result = Mock()
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "run_restic", fake_run_restic)
    monkeypatch.setattr(engine, "record_backup_result", record_backup_result)
    monkeypatch.setattr(engine, "record_prune_result", record_prune_result)

    result = engine.check_archive_snapshot_files("abc123", {target: 42})

    assert result == engine.ArchiveCheckResult(
        status="ok",
        error_reason=None,
        verdicts=(
            engine.ArchiveFileVerdict(
                path=str(target),
                confirmed=True,
                expected_size=42,
                observed_size=42,
                snapshot_id=full_snapshot_id,
            ),
        ),
    )
    assert calls == [
        (
            ["ls", "--long", "abc123"],
            {
                "repository": "s3:safe-bucket/path",
                "password": "daily-secret",
                "restic_path": Path("/restic"),
                "backend_env": {
                    "AWS_ACCESS_KEY_ID": "access-key",
                    "AWS_SECRET_ACCESS_KEY": "secret-key",
                },
                "json": True,
                "timeout": engine.ARCHIVE_LS_TIMEOUT_SECONDS,
            },
        )
    ]
    assert _config_path(tmp_path).read_bytes() == before_config
    record_backup_result.assert_not_called()
    record_prune_result.assert_not_called()


def test_check_archive_snapshot_files_reports_missing_and_size_mismatch(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _valid_backup_config())
    before_config = _config_path(tmp_path).read_bytes()
    mismatch = Path("/archive/file.wav")
    directory = Path("/archive/dir")
    missing = Path("/archive/missing.wav")

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        return _restic_result(
            0,
            parsed_json=[
                {
                    "message_type": "snapshot",
                    "struct_type": "snapshot",
                    "id": "full-snap",
                },
                {
                    "message_type": "node",
                    "struct_type": "node",
                    "path": str(mismatch),
                    "name": mismatch.name,
                    "type": "file",
                    "size": 7,
                },
                {
                    "message_type": "node",
                    "struct_type": "node",
                    "path": str(directory),
                    "name": directory.name,
                    "type": "dir",
                },
            ],
            args=args,
        )

    record_backup_result = Mock()
    record_prune_result = Mock()
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "run_restic", fake_run_restic)
    monkeypatch.setattr(engine, "record_backup_result", record_backup_result)
    monkeypatch.setattr(engine, "record_prune_result", record_prune_result)

    result = engine.check_archive_snapshot_files(
        "full-snap",
        {
            mismatch: 42,
            directory: 0,
            missing: 5,
        },
    )

    assert result == engine.ArchiveCheckResult(
        status="ok",
        error_reason=None,
        verdicts=(
            engine.ArchiveFileVerdict(
                path=str(mismatch),
                confirmed=False,
                expected_size=42,
                observed_size=7,
                snapshot_id="full-snap",
            ),
            engine.ArchiveFileVerdict(
                path=str(directory),
                confirmed=False,
                expected_size=0,
                observed_size=None,
                snapshot_id="full-snap",
            ),
            engine.ArchiveFileVerdict(
                path=str(missing),
                confirmed=False,
                expected_size=5,
                observed_size=None,
                snapshot_id="full-snap",
            ),
        ),
    )
    assert _config_path(tmp_path).read_bytes() == before_config
    record_backup_result.assert_not_called()
    record_prune_result.assert_not_called()


@pytest.mark.parametrize(
    ("returncode", "parsed_json", "expected_reason"),
    [
        (11, None, "locked"),
        (0, None, "failed"),
        (
            0,
            {
                "message_type": "snapshot",
                "struct_type": "snapshot",
                "id": "header-only",
            },
            "failed",
        ),
        (
            0,
            [
                {
                    "message_type": "node",
                    "struct_type": "node",
                    "path": "/archive/file.wav",
                    "type": "file",
                    "size": 5,
                }
            ],
            "failed",
        ),
    ],
)
def test_check_archive_snapshot_files_tool_failures_have_no_verdicts(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    returncode: int,
    parsed_json: Any | None,
    expected_reason: str,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, _valid_backup_config())
    before_config = _config_path(tmp_path).read_bytes()
    calls: list[tuple[list[str], dict[str, Any]]] = []

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        calls.append((args, kwargs))
        return _restic_result(returncode, parsed_json=parsed_json, args=args)

    record_backup_result = Mock()
    record_prune_result = Mock()
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "run_restic", fake_run_restic)
    monkeypatch.setattr(engine, "record_backup_result", record_backup_result)
    monkeypatch.setattr(engine, "record_prune_result", record_prune_result)

    result = engine.check_archive_snapshot_files(
        "snap",
        {Path("/archive/file.wav"): 5},
    )

    assert result == engine.ArchiveCheckResult(
        status="error",
        error_reason=expected_reason,
        verdicts=None,
    )
    assert calls[0][0] == ["ls", "--long", "snap"]
    assert calls[0][1]["json"] is True
    assert calls[0][1]["timeout"] == engine.ARCHIVE_LS_TIMEOUT_SECONDS
    assert _config_path(tmp_path).read_bytes() == before_config
    record_backup_result.assert_not_called()
    record_prune_result.assert_not_called()


@pytest.mark.parametrize(
    "backup_config",
    [
        {"enabled": False},
        {
            "enabled": True,
            "daily_key": "daily-secret",
            "recovery_key": "R" * 64,
        },
        {
            "enabled": True,
            "destination": {
                "repository": "s3:safe-bucket/path",
                "backend": "s3",
                "credentials": {
                    "access_key_id": "access-key",
                    "secret_access_key": "secret-key",
                },
            },
        },
    ],
)
def test_check_archive_snapshot_files_skips_when_runtime_guard_incomplete(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    backup_config: dict[str, Any],
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {"backup": backup_config})
    before_config = _config_path(tmp_path).read_bytes()
    ensure_restic = Mock()
    run_restic = Mock()
    record_backup_result = Mock()
    record_prune_result = Mock()
    monkeypatch.setattr(engine, "ensure_restic", ensure_restic)
    monkeypatch.setattr(engine, "run_restic", run_restic)
    monkeypatch.setattr(engine, "record_backup_result", record_backup_result)
    monkeypatch.setattr(engine, "record_prune_result", record_prune_result)

    result = engine.check_archive_snapshot_files(
        "snap",
        {Path("/archive/file.wav"): 5},
    )

    assert result == engine.ArchiveCheckResult(
        status="skipped",
        error_reason=None,
        verdicts=None,
    )
    ensure_restic.assert_not_called()
    run_restic.assert_not_called()
    record_backup_result.assert_not_called()
    record_prune_result.assert_not_called()
    assert _config_path(tmp_path).read_bytes() == before_config


def test_operated_archive_backup_and_check_use_append_only_scope(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(
        tmp_path,
        {
            "backup": {
                "enabled": True,
                "mode": "operated",
                "daily_key": "dk",
                "recovery_key": "R" * 64,
            }
        },
    )
    save_hosted_binding(
        HostedBinding(
            broker_endpoint="https://broker.example",
            account_id="acct",
            instance_id="inst",
            bucket="bkt",
            prefix="users/acct/inst",
            broker_token="BTOKEN",
        )
    )
    before_config = _config_path(tmp_path).read_bytes()
    captured_scopes: list[str] = []
    calls: list[tuple[list[str], dict[str, Any]]] = []

    def fake_fetch(
        _binding: HostedBinding,
        *,
        scope: str,
    ) -> HostedCredentials:
        captured_scopes.append(scope)
        return HostedCredentials(
            access_key_id="AKID",
            secret_access_key="SAK",
            session_token="SESS",
            endpoint="https://acct.r2.cloudflarestorage.com",
            expires_at="2026-07-13T12:00:00Z",
        )

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        calls.append((args, kwargs))
        if "unlock" in args:
            return _restic_result(0, args=args)
        if "backup" in args:
            return _restic_result(
                0,
                parsed_json={
                    "message_type": "summary",
                    "snapshot_id": "archive-snap",
                },
                args=args,
            )
        return _restic_result(
            0,
            parsed_json=[
                {
                    "message_type": "snapshot",
                    "struct_type": "snapshot",
                    "id": "archive-snap-full",
                }
            ],
            args=args,
        )

    record_backup_result = Mock()
    record_prune_result = Mock()
    monkeypatch.setattr(engine, "fetch_hosted_credentials", fake_fetch)
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "ensure_rclone", Mock(return_value=Path("/rclone")))
    monkeypatch.setattr(engine, "run_restic", fake_run_restic)
    monkeypatch.setattr(engine, "record_backup_result", record_backup_result)
    monkeypatch.setattr(engine, "record_prune_result", record_prune_result)

    archive_result = engine.run_archive_backup([Path("/archive/a.mov")])
    check_result = engine.check_archive_snapshot_files(
        "archive-snap",
        {Path("/archive/a.mov"): 5},
    )

    assert archive_result.status == "ok"
    assert check_result.status == "ok"
    assert captured_scopes == ["operated", "operated"]
    archive_call = next(call for call in calls if "backup" in call[0])
    assert archive_call[0][:4] == [
        "-o",
        "rclone.program=/rclone",
        "-o",
        "rclone.args=serve restic --stdio --append-only --config /dev/null",
    ]
    assert archive_call[0][4:7] == [
        "--retry-lock",
        engine.ARCHIVE_RETRY_LOCK,
        "backup",
    ]
    ls_call = next(call for call in calls if "ls" in call[0])
    assert ls_call[0][:4] == [
        "-o",
        "rclone.program=/rclone",
        "-o",
        "rclone.args=serve restic --stdio --append-only --config /dev/null",
    ]
    assert ls_call[0][4:7] == ["ls", "--long", "archive-snap"]
    assert ls_call[1]["json"] is True
    assert ls_call[1]["timeout"] == engine.ARCHIVE_LS_TIMEOUT_SECONDS
    assert sum(1 for args, _kwargs in calls if "unlock" in args) == 1
    assert _config_path(tmp_path).read_bytes() == before_config
    record_backup_result.assert_not_called()
    record_prune_result.assert_not_called()


def test_malformed_backend_env_records_failed_without_raw_exception(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    config = _valid_backup_config()
    del config["backup"]["destination"]["credentials"]["secret_access_key"]
    _write_config(tmp_path, config)
    run_restic = Mock()
    record_backup_result = Mock()
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "run_restic", run_restic)
    monkeypatch.setattr(engine, "record_backup_result", record_backup_result)
    monkeypatch.setattr(engine.time, "time", lambda: 5000)

    result = engine.run_backup()

    assert result == engine.BackupResult(
        status="error",
        snapshot_id=None,
        error_reason="failed",
    )
    run_restic.assert_not_called()
    record_backup_result.assert_called_once_with(
        status="error",
        time=5000,
        snapshot_id=None,
        error_reason="failed",
    )


def test_backup_and_prune_failures_do_not_persist_or_log_secrets(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    caplog: pytest.LogCaptureFixture,
) -> None:
    daily_key = "SECRET-DAILY"
    access_key_id = "SECRET-ACCESS"
    secret_access_key = "SECRET-BACKEND"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(
        tmp_path,
        _valid_backup_config(
            daily_key=daily_key,
            access_key_id=access_key_id,
            secret_access_key=secret_access_key,
        ),
    )

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        if args == ["unlock"]:
            return _restic_result(0, args=args)
        return _restic_result(
            12,
            args=args,
            text=f"{daily_key} {access_key_id} {secret_access_key}",
        )

    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "run_restic", fake_run_restic)
    monkeypatch.setattr(engine.time, "time", lambda: 6000)
    caplog.set_level(logging.WARNING, logger="solstone.backup.engine")

    backup_result = engine.run_backup()
    prune_result = engine.run_prune()
    verification_result = engine.run_verification()

    config = _read_config(tmp_path)
    serialized_results = json.dumps(
        {
            "last_backup": config["backup"]["last_backup"],
            "last_prune": config["backup"]["last_prune"],
            "last_verification": config["backup"]["last_verification"],
        }
    )
    for secret in (daily_key, access_key_id, secret_access_key):
        assert secret not in serialized_results
        assert secret not in caplog.text
    assert backup_result.error_reason == "auth_failed"
    assert prune_result.error_reason == "auth_failed"
    assert verification_result.reason == "auth_failed"


def test_operated_backup_fetches_creds_and_builds_repo(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(
        tmp_path,
        {
            "backup": {
                "enabled": True,
                "mode": "operated",
                "daily_key": "dk",
                "recovery_key": "R" * 64,
            }
        },
    )
    save_hosted_binding(
        HostedBinding(
            broker_endpoint="https://broker.example",
            account_id="acct",
            instance_id="inst",
            bucket="bkt",
            prefix="users/acct/inst",
            broker_token="BTOKEN",
        )
    )
    captured: dict[str, str] = {}
    calls: list[tuple[list[str], dict[str, Any]]] = []

    def fake_fetch(
        _binding: HostedBinding,
        *,
        scope: str,
    ) -> HostedCredentials:
        captured["scope"] = scope
        return HostedCredentials(
            access_key_id="AKID",
            secret_access_key="SAK",
            session_token="SESS",
            endpoint="https://acct.r2.cloudflarestorage.com",
            expires_at="2026-07-13T12:00:00Z",
        )

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        calls.append((args, kwargs))
        if args == ["unlock"]:
            return _restic_result(0, args=args)
        return _restic_result(
            0,
            parsed_json={"message_type": "summary", "snapshot_id": "snap1"},
            args=args,
        )

    monkeypatch.setattr(engine, "fetch_hosted_credentials", fake_fetch)
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "ensure_rclone", Mock(return_value=Path("/rclone")))
    monkeypatch.setattr(engine, "run_restic", fake_run_restic)

    result = engine.run_backup()

    assert result.status == "ok"
    backup_call = next(call for call in calls if "backup" in call[0])
    backup_args = backup_call[0]
    backup_kwargs = backup_call[1]
    backend_env = backup_kwargs["backend_env"]
    assert backup_kwargs["repository"] == "rclone:spb:bkt/users/acct/inst"
    assert backup_args[:4] == [
        "-o",
        "rclone.program=/rclone",
        "-o",
        "rclone.args=serve restic --stdio --append-only --config /dev/null",
    ]
    assert backup_args[4] == "backup"
    assert backend_env["RCLONE_CONFIG_SPB_TYPE"] == "s3"
    assert backend_env["RCLONE_CONFIG_SPB_PROVIDER"] == "Cloudflare"
    assert backend_env["RCLONE_CONFIG_SPB_ENV_AUTH"] == "false"
    assert backend_env["RCLONE_CONFIG_SPB_ACCESS_KEY_ID"] == "AKID"
    assert backend_env["RCLONE_CONFIG_SPB_SECRET_ACCESS_KEY"] == "SAK"
    assert backend_env["RCLONE_CONFIG_SPB_SESSION_TOKEN"] == "SESS"
    assert "AWS_CONTAINER_CREDENTIALS_FULL_URI" not in backend_env
    assert "AWS_CONTAINER_AUTHORIZATION_TOKEN" not in backend_env
    for secret in ("AKID", "SAK", "SESS"):
        assert secret not in backup_kwargs["repository"]
    assert captured["scope"] == "operated"


def test_operated_prune_requests_maintenance_scope(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(
        tmp_path,
        {
            "backup": {
                "enabled": True,
                "mode": "operated",
                "daily_key": "dk",
                "recovery_key": "R" * 64,
            }
        },
    )
    save_hosted_binding(
        HostedBinding(
            broker_endpoint="https://broker.example",
            account_id="acct",
            instance_id="inst",
            bucket="bkt",
            prefix="users/acct/inst",
            broker_token="BTOKEN",
        )
    )
    captured: dict[str, str] = {}
    calls: list[tuple[list[str], dict[str, Any]]] = []

    def fake_fetch(
        _binding: HostedBinding,
        *,
        scope: str,
    ) -> HostedCredentials:
        captured["scope"] = scope
        return HostedCredentials(
            access_key_id="AKID",
            secret_access_key="SAK",
            session_token="SESS",
            endpoint="https://acct.r2.cloudflarestorage.com",
            expires_at="2026-07-13T12:00:00Z",
        )

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        calls.append((args, kwargs))
        return _restic_result(0, args=args)

    monkeypatch.setattr(engine, "fetch_hosted_credentials", fake_fetch)
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "run_restic", fake_run_restic)

    result = engine.run_prune()

    assert result.status == "ok"
    forget_call = next(call for call in calls if call[0][0] == "forget")
    assert captured["scope"] == "maintenance"
    assert forget_call[1]["backend_env"] == {
        "AWS_ACCESS_KEY_ID": "AKID",
        "AWS_SECRET_ACCESS_KEY": "SAK",
        "AWS_SESSION_TOKEN": "SESS",
    }


def test_backup_timeout_is_long_only_until_first_snapshot(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    config = _valid_backup_config()
    _write_config(tmp_path, config)

    assert engine._backup_timeout() == engine.INITIAL_BACKUP_TIMEOUT_SECONDS

    config["backup"]["last_backup"] = {
        "status": "error",
        "snapshot_id": "partial-snap",
    }
    _write_config(tmp_path, config)

    assert engine._backup_timeout() == engine.INITIAL_BACKUP_TIMEOUT_SECONDS

    config["backup"]["last_backup"] = {"status": "ok", "snapshot_id": "snap-1"}
    _write_config(tmp_path, config)

    assert engine._backup_timeout() == engine.BACKUP_TIMEOUT_SECONDS


def _assert_operated_backup_degrades_on_hosted_credential_error(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    reason_code: str,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(
        tmp_path,
        {
            "backup": {
                "enabled": True,
                "mode": "operated",
                "daily_key": "dk",
                "recovery_key": "R" * 64,
            }
        },
    )
    save_hosted_binding(
        HostedBinding(
            broker_endpoint="https://broker.example",
            account_id="acct",
            instance_id="inst",
            bucket="bkt",
            prefix="users/acct/inst",
            broker_token="BTOKEN",
        )
    )
    run_restic = Mock()
    record_backup_result = Mock()
    monkeypatch.setattr(
        engine,
        "fetch_hosted_credentials",
        Mock(side_effect=HostedCredsUnavailable(reason_code)),
    )
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "run_restic", run_restic)
    monkeypatch.setattr(engine, "record_backup_result", record_backup_result)

    result = engine.run_backup()

    assert result.status == "error"
    assert result.error_reason == reason_code
    run_restic.assert_not_called()
    assert record_backup_result.call_args.kwargs["error_reason"] == reason_code


def test_operated_backup_degrades_on_entitlement_inactive(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _assert_operated_backup_degrades_on_hosted_credential_error(
        tmp_path,
        monkeypatch,
        "hosted_entitlement_inactive",
    )


def test_operated_backup_degrades_on_broker_unreachable(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _assert_operated_backup_degrades_on_hosted_credential_error(
        tmp_path,
        monkeypatch,
        "broker_unreachable",
    )


def test_operated_degrade_is_non_destructive(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    byo_destination = {
        "repository": "s3:byo/path",
        "backend": "s3",
        "credentials": {"access_key_id": "a", "secret_access_key": "b"},
    }
    _write_config(
        tmp_path,
        {
            "backup": {
                "enabled": True,
                "mode": "operated",
                "daily_key": "dk",
                "recovery_key": "R" * 64,
                "destination": byo_destination,
            }
        },
    )
    save_hosted_binding(
        HostedBinding(
            broker_endpoint="https://broker.example",
            account_id="acct",
            instance_id="inst",
            bucket="bkt",
            prefix="users/acct/inst",
            broker_token="BTOKEN",
        )
    )
    monkeypatch.setattr(
        engine,
        "fetch_hosted_credentials",
        Mock(side_effect=HostedCredsUnavailable("broker_unreachable")),
    )

    result = engine.run_backup()

    backup = _read_config(tmp_path)["backup"]
    assert result.error_reason == "broker_unreachable"
    assert backup["daily_key"] == "dk"
    assert backup["recovery_key"] == "R" * 64
    assert backup["destination"] == byo_destination
    assert backup["last_backup"]["status"] == "error"
    assert load_hosted_binding() is not None


def test_operated_does_not_persist_or_log_secrets(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    caplog: pytest.LogCaptureFixture,
) -> None:
    secrets = (
        "dk-secret",
        "AKID-SECRET",
        "SAK-SECRET",
        "SESS-SECRET",
        "BTOKEN-SECRET",
    )
    secret_text = " ".join(secrets)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(
        tmp_path,
        {
            "backup": {
                "enabled": True,
                "mode": "operated",
                "daily_key": "dk-secret",
                "recovery_key": "R" * 64,
            }
        },
    )
    save_hosted_binding(
        HostedBinding(
            broker_endpoint="https://broker.example",
            account_id="acct",
            instance_id="inst",
            bucket="bkt",
            prefix="users/acct/inst",
            broker_token="BTOKEN-SECRET",
        )
    )

    def fake_fetch(
        _binding: HostedBinding,
        *,
        scope: str,
    ) -> HostedCredentials:
        assert scope in {"operated", "maintenance"}
        return HostedCredentials(
            access_key_id="AKID-SECRET",
            secret_access_key="SAK-SECRET",
            session_token="SESS-SECRET",
            endpoint="https://acct.r2.cloudflarestorage.com",
            expires_at="2026-07-13T12:00:00Z",
        )

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        if "unlock" in args:
            return _restic_result(0, args=args, text=secret_text)
        if "backup" in args:
            return _restic_result(
                0,
                parsed_json={"message_type": "summary", "snapshot_id": "snap1"},
                args=args,
                text=secret_text,
            )
        return _restic_result(12, args=args, text=secret_text)

    monkeypatch.setattr(engine, "fetch_hosted_credentials", fake_fetch)
    monkeypatch.setattr(engine, "ensure_restic", Mock(return_value=Path("/restic")))
    monkeypatch.setattr(engine, "ensure_rclone", Mock(return_value=Path("/rclone")))
    monkeypatch.setattr(engine, "run_restic", fake_run_restic)
    caplog.set_level(logging.WARNING, logger="solstone.backup.engine")

    engine.run_backup()
    engine.run_prune()

    config = _read_config(tmp_path)
    serialized = json.dumps(
        {
            "last_backup": config["backup"]["last_backup"],
            "last_prune": config["backup"]["last_prune"],
        }
    )
    for secret in secrets:
        assert secret not in serialized
        assert secret not in caplog.text
