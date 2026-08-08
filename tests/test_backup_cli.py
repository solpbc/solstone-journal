# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any
from unittest.mock import Mock

import pytest
from typer.testing import CliRunner

from solstone.think import backup_cli
from solstone.think.backup.destination import Destination, DestinationStatus
from solstone.think.backup.engine import BackupResult, PruneResult
from solstone.think.backup.hosted import hosted_binding_path, load_hosted_binding
from solstone.think.backup.keys import format_recovery_key_display
from solstone.think.backup.restore import RestoreResult
from solstone.think.backup.rotation import RotationResult
from solstone.think.backup.state import BackupKeys
from solstone.think.backup.teardown import TeardownResult
from solstone.think.offload import OffloadResult, OffloadSegmentDetail
from solstone.think.offload_restore import OffloadRestoreResult


def _config_path(journal: Path) -> Path:
    return journal / "config" / "journal.json"


def _write_config(journal: Path, payload: dict[str, Any]) -> None:
    config_path = _config_path(journal)
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text(json.dumps(payload), encoding="utf-8")


def _destination() -> Destination:
    return Destination(
        repository="s3:safe-bucket/path",
        backend="s3",
        credentials={
            "access_key_id": "access-key",
            "secret_access_key": "secret-key",
        },
    )


def _destination_config() -> dict[str, Any]:
    destination = _destination()
    return {
        "repository": destination.repository,
        "backend": destination.backend,
        "credentials": destination.credentials,
    }


def _status(reason_code: str) -> DestinationStatus:
    if reason_code == "repo_missing":
        return DestinationStatus(
            reachable=True,
            repo_exists=False,
            reason_code="repo_missing",
            message="backup destination is reachable and needs setup",
        )
    if reason_code == "auth_failed":
        return DestinationStatus(
            reachable=True,
            repo_exists=True,
            reason_code="auth_failed",
            message="repository password was rejected",
        )
    return DestinationStatus(
        reachable=True,
        repo_exists=True,
        reason_code="repo_exists",
        message="backup repository is reachable",
    )


def test_command_tree() -> None:
    runner = CliRunner()
    root_help = runner.invoke(backup_cli.app, ["--help"])
    assert root_help.exit_code == 0
    for command in (
        "enable",
        "destination",
        "run",
        "prune",
        "status",
        "recovery-key",
        "offload",
        "restore",
        "off",
    ):
        assert command in root_help.output

    destination_help = runner.invoke(backup_cli.app, ["destination", "--help"])
    assert destination_help.exit_code == 0
    assert "set" in destination_help.output
    assert "show" in destination_help.output

    recovery_help = runner.invoke(backup_cli.app, ["recovery-key", "--help"])
    assert recovery_help.exit_code == 0
    assert "show" in recovery_help.output
    assert "rotate" in recovery_help.output

    offload_help = runner.invoke(backup_cli.app, ["offload", "--help"])
    assert offload_help.exit_code == 0
    assert "status" in offload_help.output
    assert "run" in offload_help.output
    assert "restore" in offload_help.output


@pytest.mark.parametrize(
    ("reason_code", "expected_exit"),
    [
        ("repo_missing", 0),
        ("auth_failed", 1),
    ],
)
def test_destination_set_reads_stdin_and_keeps_secrets_off_argv(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    reason_code: str,
    expected_exit: int,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {})
    captured: dict[str, Any] = {}

    def fake_set_destination(destination: Destination) -> None:
        captured["destination"] = destination

    def fake_validate_destination(
        destination: Destination,
        password: str,
        *,
        restic_path: Path,
    ) -> DestinationStatus:
        captured["argv"] = list(sys.argv)
        captured["password"] = password
        captured["restic_path"] = restic_path
        return _status(reason_code)

    monkeypatch.setattr(backup_cli, "set_destination", fake_set_destination)
    monkeypatch.setattr(backup_cli, "ensure_restic", lambda: Path("/restic"))
    monkeypatch.setattr(backup_cli, "generate_daily_key", lambda: "probe-secret")
    monkeypatch.setattr(backup_cli, "validate_destination", fake_validate_destination)

    payload = {
        "repository": "s3:safe-bucket/path",
        "backend": "s3",
        "credentials": {
            "access_key_id": "AKIASECRET",
            "secret_access_key": "TOPSECRET",
        },
    }
    result = CliRunner().invoke(
        backup_cli.app,
        ["destination", "set"],
        input=json.dumps(payload),
    )

    assert result.exit_code == expected_exit
    destination = captured["destination"]
    assert destination.credentials == payload["credentials"]
    assert captured["password"] == "probe-secret"
    assert captured["restic_path"] == Path("/restic")
    assert "TOPSECRET" not in " ".join(captured["argv"])
    assert "AKIASECRET" not in " ".join(captured["argv"])
    assert "TOPSECRET" not in result.output
    assert "AKIASECRET" not in result.output


def test_destination_set_hosted_writes_binding_0600_without_leaking_token(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    payload = {
        "broker_endpoint": "https://broker.example",
        "account_id": "acct",
        "instance_id": "inst",
        "bucket": "bkt",
        "prefix": "users/acct/inst/",
        "broker_token": "BTOKEN",
    }

    result = CliRunner().invoke(
        backup_cli.app,
        ["destination", "set-hosted"],
        input=json.dumps(payload),
    )

    assert result.exit_code == 0
    assert "BTOKEN" not in result.stdout
    binding = load_hosted_binding()
    assert binding is not None
    assert binding.prefix == "users/acct/inst/"
    assert hosted_binding_path().stat().st_mode & 0o777 == 0o600


def test_destination_set_hosted_rejects_missing_field(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    payload = {
        "broker_endpoint": "https://broker.example",
        "account_id": "acct",
        "instance_id": "inst",
        "bucket": "bkt",
        "prefix": "users/acct/inst/",
    }

    result = CliRunner().invoke(
        backup_cli.app,
        ["destination", "set-hosted"],
        input=json.dumps(payload),
    )

    assert result.exit_code != 0


def test_status_and_destination_show_are_redacted(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    recovery_key = "A" * 64
    _write_config(
        tmp_path,
        {
            "backup": {
                "destination": {
                    "repository": "s3:safe-bucket/path",
                    "backend": "s3",
                    "credentials": {
                        "access_key_id": "ACCESSSECRET",
                        "secret_access_key": "BACKENDSECRET",
                    },
                },
                "daily_key": "DAILYSECRET",
                "recovery_key": recovery_key,
            }
        },
    )
    runner = CliRunner()

    status_result = runner.invoke(backup_cli.app, ["status"])
    destination_result = runner.invoke(backup_cli.app, ["destination", "show"])

    assert status_result.exit_code == 0
    assert destination_result.exit_code == 0
    output = status_result.output + destination_result.output
    for secret in ("DAILYSECRET", recovery_key, "ACCESSSECRET", "BACKENDSECRET"):
        assert secret not in output
    assert "credentials_set" in output


def test_enable_accepts_lookalike_recovery_key_confirmation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {"backup": {"destination": _destination_config()}})
    recovery_key = ("0" * 32) + ("1" * 32)
    display = format_recovery_key_display(recovery_key)
    keys = BackupKeys(
        daily_key="daily-secret",
        recovery_key=recovery_key,
        recovery_key_display=display,
    )
    calls: dict[str, Any] = {}

    def fake_set_recovery_key_confirmed(confirmed: bool = True) -> None:
        calls["confirmed"] = confirmed

    def fake_set_enabled(enabled: bool) -> None:
        calls["enabled"] = enabled

    def fake_init_repository(
        destination: Destination,
        *,
        daily_key: str,
        recovery_key: str,
        restic_path: Path,
    ) -> None:
        calls["init"] = {
            "destination": destination,
            "daily_key": daily_key,
            "recovery_key": recovery_key,
            "restic_path": restic_path,
        }

    monkeypatch.setattr(backup_cli, "generate_and_store_keys", lambda: keys)
    monkeypatch.setattr(backup_cli, "ensure_restic", lambda: Path("/restic"))
    monkeypatch.setattr(
        backup_cli,
        "set_recovery_key_confirmed",
        fake_set_recovery_key_confirmed,
    )
    monkeypatch.setattr(backup_cli, "set_enabled", fake_set_enabled)
    monkeypatch.setattr(backup_cli, "init_repository", fake_init_repository)

    entered = display.replace("0", "O").replace("1", "I")
    result = CliRunner().invoke(backup_cli.app, ["enable"], input=entered)

    assert result.exit_code == 0
    assert calls["confirmed"] is True
    assert calls["enabled"] is True
    assert calls["init"]["daily_key"] == "daily-secret"
    assert calls["init"]["recovery_key"] == recovery_key


def test_off_requires_yes_before_teardown(monkeypatch: pytest.MonkeyPatch) -> None:
    teardown_backup = Mock(return_value=TeardownResult(status="ok", reason_code=None))
    monkeypatch.setattr(backup_cli, "teardown_backup", teardown_backup)
    runner = CliRunner()

    refused = runner.invoke(backup_cli.app, ["off"])
    accepted = runner.invoke(backup_cli.app, ["off", "--yes"])

    assert refused.exit_code == 1
    assert "Refusing" in refused.output
    teardown_backup.assert_called_once_with()
    assert accepted.exit_code == 0
    assert "Backup turned off." in accepted.output


def test_enable_power_user_skips_ceremony(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(
        tmp_path,
        {
            "backup": {
                "destination": _destination_config(),
                "daily_key": "manual-daily",
                "recovery_key": None,
            }
        },
    )
    set_enabled = Mock()
    monkeypatch.setattr(
        backup_cli,
        "generate_and_store_keys",
        lambda: pytest.fail("ceremony should be skipped"),
    )
    monkeypatch.setattr(
        backup_cli,
        "set_recovery_key_confirmed",
        lambda *_args, **_kwargs: pytest.fail("confirmation should not be set"),
    )
    monkeypatch.setattr(backup_cli, "ensure_restic", lambda: Path("/restic"))
    monkeypatch.setattr(
        backup_cli, "validate_destination", lambda *a, **k: _status("ok")
    )
    monkeypatch.setattr(backup_cli, "set_enabled", set_enabled)

    result = CliRunner().invoke(backup_cli.app, ["enable"])

    assert result.exit_code == 0
    assert "Your recovery key" not in result.output
    set_enabled.assert_called_once_with(True)


def test_enable_power_user_requires_existing_repository(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(
        tmp_path,
        {
            "backup": {
                "destination": _destination_config(),
                "daily_key": "manual-daily",
                "recovery_key": None,
            }
        },
    )
    set_enabled = Mock()
    monkeypatch.setattr(backup_cli, "ensure_restic", lambda: Path("/restic"))
    monkeypatch.setattr(
        backup_cli,
        "validate_destination",
        lambda *a, **k: _status("repo_missing"),
    )
    monkeypatch.setattr(backup_cli, "set_enabled", set_enabled)

    result = CliRunner().invoke(backup_cli.app, ["enable"])

    assert result.exit_code == 1
    assert "Repository not found" in result.output
    set_enabled.assert_not_called()


@pytest.mark.parametrize(
    ("result", "expected_exit", "expected_text"),
    [
        (BackupResult("ok", "snap123", None), 0, "snap123"),
        (BackupResult("skipped", None, None), 0, "skipped"),
        (BackupResult("error", None, "auth_failed"), 1, "auth_failed"),
    ],
)
def test_run_maps_engine_status(
    monkeypatch: pytest.MonkeyPatch,
    result: BackupResult,
    expected_exit: int,
    expected_text: str,
) -> None:
    monkeypatch.setattr(backup_cli, "run_backup", lambda: result)

    invoke_result = CliRunner().invoke(backup_cli.app, ["run"])

    assert invoke_result.exit_code == expected_exit
    assert expected_text in invoke_result.output


@pytest.mark.parametrize(
    ("result", "expected_exit", "expected_text"),
    [
        (PruneResult("ok", None), 0, "Retention prune complete."),
        (PruneResult("skipped", None), 0, "skipped"),
        (PruneResult("error", "auth_failed"), 1, "auth_failed"),
    ],
)
def test_prune_maps_engine_status(
    monkeypatch: pytest.MonkeyPatch,
    result: PruneResult,
    expected_exit: int,
    expected_text: str,
) -> None:
    monkeypatch.setattr(backup_cli, "run_prune", lambda: result)

    invoke_result = CliRunner().invoke(backup_cli.app, ["prune"])

    assert invoke_result.exit_code == expected_exit
    assert expected_text in invoke_result.output


def test_offload_status_json_delegates_to_status_builder(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    payload = {
        "offload": {"enabled": True, "budget_bytes": 100, "floor_bytes": 50},
        "raw_media": {"total_bytes": 30, "total_files": 3},
        "backup_only": {"total_bytes": 20, "degraded": False},
        "pending_release": {"total_bytes": 10, "total_files": 1},
    }
    build_offload_status = Mock(return_value=payload)
    monkeypatch.setattr(backup_cli, "build_offload_status", build_offload_status)

    result = CliRunner().invoke(backup_cli.app, ["offload", "status", "--json"])

    assert result.exit_code == 0
    assert json.loads(result.output) == payload
    build_offload_status.assert_called_once_with()


def test_offload_run_delegates_to_existing_pass_and_formats_like_maintenance(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: dict[str, Any] = {}

    def fake_run_offload(*, dry_run: bool) -> OffloadResult:
        captured["dry_run"] = dry_run
        return OffloadResult(
            status="ok",
            reason=None,
            files_marked=0,
            bytes_marked=0,
            files_already_marked=0,
            bytes_already_marked=0,
            ran_out_of_markable_media=False,
            dry_run=True,
            details=(
                OffloadSegmentDetail(
                    day="20260101",
                    stream="_default",
                    segment="090000_300",
                    files=2,
                    bytes=50,
                ),
            ),
        )

    monkeypatch.setattr(backup_cli, "run_offload", fake_run_offload)

    result = CliRunner().invoke(backup_cli.app, ["offload", "run", "--dry-run"])

    assert result.exit_code == 0
    assert captured == {"dry_run": True}
    assert (
        "backup offload: ok dry_run=true selected_files=2 selected_bytes=50 "
        "ran_out_of_markable_media=False segments=20260101/_default/090000_300:50"
        in result.output
    )


def test_offload_restore_day_json_delegates_to_restore_engine(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    restore_day = Mock(
        return_value=OffloadRestoreResult(
            status="ok",
            reason=None,
            scope="day",
            day="20260228",
            segments_selected=1,
            segments_restored=1,
            files_expected=2,
            files_restored=2,
            bytes_expected=50,
            bytes_restored=50,
            details=(),
        )
    )
    monkeypatch.setattr(backup_cli, "restore_day", restore_day)

    result = CliRunner().invoke(
        backup_cli.app,
        ["offload", "restore", "20260228", "--json"],
    )

    assert result.exit_code == 0
    assert json.loads(result.output)["day"] == "20260228"
    restore_day.assert_called_once_with("20260228")


def test_offload_restore_all_and_invalid_scope(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    restore_all = Mock(
        return_value=OffloadRestoreResult(
            status="no_op",
            reason="nothing_to_restore",
            scope="all",
            day=None,
            segments_selected=0,
            segments_restored=0,
            files_expected=0,
            files_restored=0,
            bytes_expected=0,
            bytes_restored=0,
            details=(),
        )
    )
    monkeypatch.setattr(backup_cli, "restore_all", restore_all)

    all_result = CliRunner().invoke(backup_cli.app, ["offload", "restore", "--all"])
    mixed_result = CliRunner().invoke(
        backup_cli.app,
        ["offload", "restore", "20260228", "--all"],
    )
    missing_result = CliRunner().invoke(backup_cli.app, ["offload", "restore"])

    assert all_result.exit_code == 0
    assert "status=no_op reason=nothing_to_restore" in all_result.output
    restore_all.assert_called_once_with()
    assert mixed_result.exit_code == 1
    assert "Use either a day or --all" in mixed_result.output
    assert missing_result.exit_code == 1
    assert "Provide a day or --all" in missing_result.output


def test_recovery_key_show_prints_display_only(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    recovery_key = "A" * 64
    display = format_recovery_key_display(recovery_key)
    _write_config(
        tmp_path,
        {
            "backup": {
                "daily_key": "daily-secret",
                "recovery_key": recovery_key,
            }
        },
    )

    result = CliRunner().invoke(backup_cli.app, ["recovery-key", "show"])

    assert result.exit_code == 0
    assert "AAAA AAAA AAAA AAAA" in result.output
    assert recovery_key not in result.output
    assert display.replace(" ", "") not in result.output


def test_recovery_key_show_errors_without_key(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {})

    result = CliRunner().invoke(backup_cli.app, ["recovery-key", "show"])

    assert result.exit_code == 1
    assert "No recovery key is set." in result.output


def test_recovery_key_rotate_prints_display_not_canonical(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    canonical = "B" * 64
    display = format_recovery_key_display(canonical)
    monkeypatch.setattr(
        backup_cli,
        "rotate_recovery_key",
        lambda: RotationResult(
            status="ok",
            reason_code=None,
            recovery_key=canonical,
            recovery_key_display=display,
        ),
    )

    result = CliRunner().invoke(backup_cli.app, ["recovery-key", "rotate"])

    assert result.exit_code == 0
    assert "BBBB BBBB BBBB BBBB" in result.output
    assert canonical not in result.output


def test_recovery_key_rotate_errors(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        backup_cli,
        "rotate_recovery_key",
        lambda: RotationResult(
            status="error",
            reason_code="auth_failed",
            recovery_key=None,
            recovery_key_display=None,
        ),
    )

    result = CliRunner().invoke(backup_cli.app, ["recovery-key", "rotate"])

    assert result.exit_code == 1
    assert "auth_failed" in result.output


def test_restore_reads_secret_from_stdin_and_reports_result(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: dict[str, Any] = {}

    def fake_restore_journal(
        destination: Destination,
        recovery_key: str,
    ) -> RestoreResult:
        captured["destination"] = destination
        captured["recovery_key"] = recovery_key
        captured["argv"] = list(sys.argv)
        return RestoreResult(
            status="ok",
            reason_code=None,
            integrity_ok=True,
            resumable=True,
            bytes_restored=123,
        )

    monkeypatch.setattr(backup_cli, "restore_journal", fake_restore_journal)
    payload = {
        "repository": "b2:bucket:path",
        "backend": "b2",
        "credentials": {
            "account_id": "account-id",
            "account_key": "account-secret",
        },
        "recovery_key": "RECOVERYSECRET",
    }

    result = CliRunner().invoke(
        backup_cli.app,
        ["restore"],
        input=json.dumps(payload),
    )

    assert result.exit_code == 0
    assert captured["destination"].backend == "b2"
    assert captured["recovery_key"] == "RECOVERYSECRET"
    assert "RECOVERYSECRET" not in " ".join(captured["argv"])
    assert "RECOVERYSECRET" not in result.output
    assert (
        "Restore complete: 123 bytes, integrity_ok=True, resumable=True."
        in result.output
    )


@pytest.mark.parametrize(
    ("reason_code", "detail"),
    [
        (
            "integrity_unverified",
            "integrity verification could not run "
            "(the repository was busy or timed out)",
        ),
        (
            "integrity_failed",
            "integrity verification failed — the backup copy may be damaged",
        ),
    ],
)
def test_restore_maps_degraded(
    monkeypatch: pytest.MonkeyPatch,
    reason_code: str,
    detail: str,
) -> None:
    monkeypatch.setattr(
        backup_cli,
        "restore_journal",
        lambda *_args: RestoreResult(
            status="degraded",
            reason_code=reason_code,
            integrity_ok=False,
            resumable=True,
            bytes_restored=123,
        ),
    )
    payload = {
        "repository": "s3:safe-bucket/path",
        "backend": "s3",
        "credentials": {
            "access_key_id": "access-key",
            "secret_access_key": "secret-key",
        },
        "recovery_key": "RECOVERYSECRET",
    }

    result = CliRunner().invoke(
        backup_cli.app,
        ["restore"],
        input=json.dumps(payload),
    )

    assert result.exit_code == 1
    assert (
        f"Restored 123 bytes and saved the recovery key, but {detail} "
        f"(reason_code={reason_code})."
    ) in result.output
    assert "Restore failed" not in result.output
    assert "RECOVERYSECRET" not in result.output


def test_restore_maps_error(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        backup_cli,
        "restore_journal",
        lambda *_args: RestoreResult(
            status="error",
            reason_code="invalid_key",
            integrity_ok=False,
            resumable=False,
            bytes_restored=None,
        ),
    )
    payload = {
        "repository": "s3:safe-bucket/path",
        "backend": "s3",
        "credentials": {
            "access_key_id": "access-key",
            "secret_access_key": "secret-key",
        },
        "recovery_key": "RECOVERYSECRET",
    }

    result = CliRunner().invoke(
        backup_cli.app,
        ["restore"],
        input=json.dumps(payload),
    )

    assert result.exit_code == 1
    assert "invalid_key" in result.output
    assert "RECOVERYSECRET" not in result.output
