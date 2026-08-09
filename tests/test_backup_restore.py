# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import Any

import pytest

from solstone.think import core_handshake, journal_config
from solstone.think.backup import restore
from solstone.think.backup.destination import Destination
from solstone.think.backup.hosted import HostedBinding, HostedCredentials
from solstone.think.backup.runner import ResticResult
from solstone.think.indexer.journal import ScanReport


def _config_path(journal: Path) -> Path:
    return journal / "config" / "journal.json"


def _write_config(journal: Path, payload: dict[str, Any]) -> None:
    config_path = _config_path(journal)
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text(json.dumps(payload), encoding="utf-8")


def _read_config(journal: Path) -> dict[str, Any]:
    return json.loads(_config_path(journal).read_text(encoding="utf-8"))


def _destination() -> Destination:
    return Destination(
        repository="s3:safe-bucket/path",
        backend="s3",
        credentials={
            "access_key_id": "access-key",
            "secret_access_key": "secret-key",
        },
    )


def _operated_destination() -> Destination:
    return Destination(
        repository="s3:https://r2.example/journal-backups/users/acct/inst/",
        backend="s3",
        credentials={
            "access_key_id": "AKID-OPERATED",
            "secret_access_key": "SAK-OPERATED",
            "session_token": "SESSION-OPERATED",
        },
    )


def _operated_binding() -> HostedBinding:
    return HostedBinding(
        broker_endpoint="https://broker.example",
        account_id="acct",
        instance_id="inst",
        bucket="journal-backups",
        prefix="users/acct/inst/",
        broker_token="BTOKEN-OPERATED",
    )


def _operated_credentials() -> HostedCredentials:
    return HostedCredentials(
        access_key_id="AKID-OPERATED",
        secret_access_key="SAK-OPERATED",
        session_token="SESSION-OPERATED",
        endpoint="https://r2.example",
        expires_at="2026-07-13T12:00:00Z",
    )


def _result(returncode: int, parsed_json: Any | None = None) -> ResticResult:
    return ResticResult(
        returncode=returncode,
        stdout="",
        stderr="",
        json=parsed_json,
        argv=("restic",),
    )


@pytest.fixture(autouse=True)
def _native_body_rebuild_succeeds(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(restore, "rebuild_body_store", lambda journal: {})


def test_restore_success_normalizes_key_assembles_env_and_reindexes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {"backup": {"daily_key": "daily-secret"}})
    canonical = ("0" * 32) + ("1" * 32)
    entered = ("O" * 32) + ("I" * 32)
    destination = _destination()
    responses = iter(
        [
            _result(
                0,
                [{"paths": ["/old/journal"], "id": "snapshot-id"}],
            ),
            _result(
                0,
                {
                    "message_type": "summary",
                    "bytes_restored": 123,
                    "files_restored": 4,
                },
            ),
            _result(0),
        ]
    )
    order: list[str] = []
    calls: list[tuple[list[str], dict[str, Any]]] = []

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        order.append(
            next(arg for arg in args if arg in {"snapshots", "restore", "check"})
        )
        calls.append((args, kwargs))
        return next(responses)

    def fake_set_destination(value: Destination) -> None:
        order.append("set_destination")
        assert value == destination

    def fake_set_recovery_key(value: str) -> None:
        order.append("set_recovery_key")
        assert value == canonical

    def fake_set_recovery_key_confirmed(value: bool) -> None:
        order.append("set_recovery_key_confirmed")
        assert value is True

    def fake_scan_journal(journal: str, **kwargs: Any) -> ScanReport:
        order.append("scan_journal")
        assert journal == str(tmp_path)
        assert kwargs == {"full": True}
        return ScanReport(changed=True, edge_rows_inserted=0)

    def fake_rebuild_body_store(journal: Path) -> dict[str, Any]:
        order.append("rebuild_body_store")
        assert Path(journal) == tmp_path
        return {"rows": 0}

    monkeypatch.setattr(restore, "ensure_restic", lambda: Path("/restic"))
    monkeypatch.setattr(restore, "run_restic", fake_run_restic)
    monkeypatch.setattr(restore, "set_destination", fake_set_destination)
    monkeypatch.setattr(restore, "set_recovery_key", fake_set_recovery_key)
    monkeypatch.setattr(
        restore,
        "set_recovery_key_confirmed",
        fake_set_recovery_key_confirmed,
    )
    monkeypatch.setattr(restore, "get_backup_config", lambda: {"daily_key": "daily"})
    monkeypatch.setattr(restore, "rebuild_body_store", fake_rebuild_body_store)
    monkeypatch.setattr(restore, "scan_journal", fake_scan_journal)

    result = restore.restore_journal(destination, entered)

    assert result == restore.RestoreResult(
        status="ok",
        reason_code=None,
        integrity_ok=True,
        resumable=True,
        bytes_restored=123,
    )
    assert order == [
        "snapshots",
        "restore",
        "check",
        "rebuild_body_store",
        "set_destination",
        "set_recovery_key",
        "set_recovery_key_confirmed",
        "scan_journal",
    ]
    assert calls[0][0] == ["snapshots", "latest"]
    assert calls[0][1]["password"] == canonical
    assert calls[0][1]["repository"] == destination.repository
    assert "access-key" not in calls[0][1]["repository"]
    assert "secret-key" not in calls[0][1]["repository"]
    assert calls[0][1]["backend_env"] == {
        "AWS_ACCESS_KEY_ID": "access-key",
        "AWS_SECRET_ACCESS_KEY": "secret-key",
    }
    assert calls[0][1]["json"] is True
    assert calls[1][0] == [
        "restore",
        "latest:/old/journal",
        "--target",
        str(tmp_path),
    ]
    assert calls[1][1]["json"] is True
    assert calls[2][0] == ["check"]
    assert "json" not in calls[2][1]


def test_restore_wrong_key_returns_auth_failed_without_persisting_secrets(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    caplog: pytest.LogCaptureFixture,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    original_config = {"backup": {"daily_key": "daily-secret"}}
    _write_config(tmp_path, original_config)
    destination = _destination()
    recovery_key = "A" * 64

    monkeypatch.setattr(restore, "ensure_restic", lambda: Path("/restic"))
    monkeypatch.setattr(
        restore,
        "run_restic",
        lambda args, **kwargs: _result(12),
    )
    monkeypatch.setattr(
        restore,
        "set_destination",
        lambda destination: pytest.fail("must not persist destination"),
    )
    monkeypatch.setattr(
        restore,
        "set_recovery_key",
        lambda key: pytest.fail("must not persist key"),
    )
    caplog.set_level(logging.WARNING, logger="solstone.backup.restore")

    result = restore.restore_journal(destination, recovery_key)

    assert result.reason_code == "auth_failed"
    assert _read_config(tmp_path) == original_config
    serialized = json.dumps(result.__dict__)
    for secret in (recovery_key, "access-key", "secret-key"):
        assert secret not in serialized
        assert secret not in caplog.text


def test_restore_invalid_key_persists_nothing(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    original_config = {"backup": {"daily_key": "daily-secret"}}
    _write_config(tmp_path, original_config)
    monkeypatch.setattr(
        restore,
        "ensure_restic",
        lambda: pytest.fail("restic should not be resolved"),
    )

    result = restore.restore_journal(_destination(), "too-short")

    assert result.reason_code == "invalid_key"
    assert _read_config(tmp_path) == original_config


def test_restore_timeout_reason_from_snapshots_call(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {"backup": {"daily_key": "daily-secret"}})
    monkeypatch.setattr(restore, "ensure_restic", lambda: Path("/restic"))
    monkeypatch.setattr(restore, "run_restic", lambda args, **kwargs: _result(124))

    result = restore.restore_journal(_destination(), "A" * 64)

    assert result == restore.RestoreResult(
        status="error",
        reason_code="timeout",
        integrity_ok=False,
        resumable=False,
        bytes_restored=None,
    )


def test_restore_backend_invalid_returns_failed(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {"backup": {"daily_key": "daily-secret"}})
    destination = _destination()
    del destination.credentials["secret_access_key"]
    monkeypatch.setattr(
        restore,
        "ensure_restic",
        lambda: pytest.fail("restic should not be resolved"),
    )

    result = restore.restore_journal(destination, "A" * 64)

    assert result.reason_code == "failed"


def test_restore_restic_unavailable_returns_reason(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {"backup": {"daily_key": "daily-secret"}})
    monkeypatch.setattr(
        restore,
        "ensure_restic",
        lambda: (_ for _ in ()).throw(RuntimeError("missing")),
    )
    monkeypatch.setattr(
        restore,
        "run_restic",
        lambda *args, **kwargs: pytest.fail("restic should not run"),
    )

    result = restore.restore_journal(_destination(), "A" * 64)

    assert result.reason_code == "restic_unavailable"


def test_restore_malformed_snapshots_json_returns_failed(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {"backup": {"daily_key": "daily-secret"}})
    monkeypatch.setattr(restore, "ensure_restic", lambda: Path("/restic"))
    monkeypatch.setattr(
        restore,
        "run_restic",
        lambda args, **kwargs: _result(0, {"unexpected": True}),
    )

    result = restore.restore_journal(_destination(), "A" * 64)

    assert result.reason_code == "failed"


@pytest.mark.parametrize(
    ("check_returncode", "reason_code"),
    [
        (11, "integrity_unverified"),
        (1, "integrity_failed"),
    ],
)
def test_restore_check_failure_reports_degraded_and_keeps_side_effects(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    check_returncode: int,
    reason_code: str,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {"backup": {"daily_key": "daily-secret"}})
    destination = _destination()
    recovery_key = "A" * 64
    responses = iter(
        [
            _result(0, [{"paths": ["/old/journal"]}]),
            _result(0, {"message_type": "summary", "bytes_restored": 5}),
            _result(check_returncode),
        ]
    )
    order: list[str] = []
    calls: list[tuple[list[str], dict[str, Any]]] = []

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        order.append(args[0])
        calls.append((args, kwargs))
        return next(responses)

    def fake_set_destination(value: Destination) -> None:
        order.append("set_destination")
        assert value == destination

    def fake_set_recovery_key(value: str) -> None:
        order.append("set_recovery_key")
        assert value == recovery_key

    def fake_set_recovery_key_confirmed(value: bool) -> None:
        order.append("set_recovery_key_confirmed")
        assert value is True

    def fake_scan_journal(journal: str, **kwargs: Any) -> ScanReport:
        order.append("scan_journal")
        assert journal == str(tmp_path)
        assert kwargs == {"full": True}
        return ScanReport(changed=True, edge_rows_inserted=0)

    monkeypatch.setattr(restore, "ensure_restic", lambda: Path("/restic"))
    monkeypatch.setattr(restore, "run_restic", fake_run_restic)
    monkeypatch.setattr(restore, "set_destination", fake_set_destination)
    monkeypatch.setattr(restore, "set_recovery_key", fake_set_recovery_key)
    monkeypatch.setattr(
        restore,
        "set_recovery_key_confirmed",
        fake_set_recovery_key_confirmed,
    )
    monkeypatch.setattr(restore, "get_backup_config", lambda: {"daily_key": "daily"})
    monkeypatch.setattr(restore, "scan_journal", fake_scan_journal)

    result = restore.restore_journal(destination, recovery_key)

    assert result == restore.RestoreResult(
        status="degraded",
        reason_code=reason_code,
        integrity_ok=False,
        resumable=True,
        bytes_restored=5,
    )
    assert result.integrity_ok is False
    assert order == [
        "snapshots",
        "restore",
        "check",
        "set_destination",
        "set_recovery_key",
        "set_recovery_key_confirmed",
        "scan_journal",
    ]
    assert calls[2][0] == ["check"]


def test_restore_missing_daily_key_is_not_resumable(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {"backup": {}})
    responses = iter(
        [
            _result(0, [{"paths": ["/old/journal"]}]),
            _result(0, {"message_type": "summary", "bytes_restored": 5}),
            _result(0),
        ]
    )
    monkeypatch.setattr(restore, "ensure_restic", lambda: Path("/restic"))
    monkeypatch.setattr(restore, "run_restic", lambda args, **kwargs: next(responses))
    monkeypatch.setattr(restore, "set_destination", lambda destination: None)
    monkeypatch.setattr(restore, "set_recovery_key", lambda key: None)
    monkeypatch.setattr(restore, "set_recovery_key_confirmed", lambda confirmed: None)
    monkeypatch.setattr(restore, "get_backup_config", lambda: {"daily_key": None})
    monkeypatch.setattr(
        restore,
        "scan_journal",
        lambda journal, **kwargs: ScanReport(changed=True, edge_rows_inserted=0),
    )

    result = restore.restore_journal(_destination(), "A" * 64)

    assert result.status == "ok"
    assert result.resumable is False


def test_restore_operated_success_persists_mode_and_key_without_destination(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {"backup": {"daily_key": "daily-secret"}})
    canonical = ("0" * 32) + ("1" * 32)
    entered = ("O" * 32) + ("I" * 32)
    destination = _operated_destination()
    responses = iter(
        [
            _result(0, [{"paths": ["/old/journal"]}]),
            _result(0, {"message_type": "summary", "bytes_restored": 456}),
            _result(0),
        ]
    )
    order: list[str] = []
    calls: list[tuple[list[str], dict[str, Any]]] = []
    real_set_mode = restore.set_mode
    real_set_recovery_key = restore.set_recovery_key
    real_set_recovery_key_confirmed = restore.set_recovery_key_confirmed

    def fake_run_restic(args: list[str], **kwargs: Any) -> ResticResult:
        order.append(
            next(arg for arg in args if arg in {"snapshots", "restore", "check"})
        )
        calls.append((args, kwargs))
        return next(responses)

    def fake_set_mode(mode: str) -> None:
        order.append("set_mode")
        assert mode == "operated"
        real_set_mode(mode)

    def fake_set_recovery_key(value: str) -> None:
        order.append("set_recovery_key")
        assert value == canonical
        real_set_recovery_key(value)

    def fake_set_recovery_key_confirmed(value: bool) -> None:
        order.append("set_recovery_key_confirmed")
        assert value is True
        real_set_recovery_key_confirmed(value)

    def fake_scan_journal(journal: str, **kwargs: Any) -> ScanReport:
        order.append("scan_journal")
        assert journal == str(tmp_path)
        assert kwargs == {"full": True}
        return ScanReport(changed=True, edge_rows_inserted=0)

    monkeypatch.setattr(restore, "ensure_restic", lambda: Path("/restic"))
    monkeypatch.setattr(restore, "ensure_rclone", lambda: Path("/rclone"))
    monkeypatch.setattr(restore, "run_restic", fake_run_restic)
    monkeypatch.setattr(
        restore,
        "set_destination",
        lambda destination: pytest.fail(
            "operated restore must not persist destination"
        ),
    )
    monkeypatch.setattr(restore, "set_mode", fake_set_mode)
    monkeypatch.setattr(restore, "set_recovery_key", fake_set_recovery_key)
    monkeypatch.setattr(
        restore,
        "set_recovery_key_confirmed",
        fake_set_recovery_key_confirmed,
    )
    monkeypatch.setattr(restore, "scan_journal", fake_scan_journal)
    helper = (
        Path(__file__).resolve().parents[1]
        / "core"
        / "target"
        / "debug"
        / "solstone-core"
    )
    monkeypatch.setattr(
        journal_config.core_handshake,
        "check_solstone_core_handshake",
        lambda: core_handshake.CoreHandshakeResult("ok"),
    )
    monkeypatch.setattr(
        journal_config.core_handshake,
        "helper_path_for_executable",
        lambda: helper,
    )

    result = restore.restore_journal_operated(
        _operated_binding(),
        _operated_credentials(),
        entered,
    )

    assert result == restore.RestoreResult(
        status="ok",
        reason_code=None,
        integrity_ok=True,
        resumable=True,
        bytes_restored=456,
    )
    assert order == [
        "snapshots",
        "restore",
        "check",
        "set_mode",
        "set_recovery_key",
        "set_recovery_key_confirmed",
        "scan_journal",
    ]
    assert calls[0][1]["backend_env"]["RCLONE_CONFIG_SPB_ACCESS_KEY_ID"] == (
        "AKID-OPERATED"
    )
    assert calls[0][1]["backend_env"]["RCLONE_CONFIG_SPB_SECRET_ACCESS_KEY"] == (
        "SAK-OPERATED"
    )
    assert calls[0][1]["backend_env"]["RCLONE_CONFIG_SPB_SESSION_TOKEN"] == (
        "SESSION-OPERATED"
    )
    assert calls[0][0][:4] == [
        "-o",
        "rclone.program=/rclone",
        "-o",
        "rclone.args=serve restic --stdio --append-only --config /dev/null",
    ]
    assert calls[0][1]["repository"] == ("rclone:spb:journal-backups/users/acct/inst/")
    assert calls[0][1]["backend_env"]["RCLONE_CONFIG_SPB_ENV_AUTH"] == "false"
    config = _read_config(tmp_path)
    serialized = json.dumps(config)
    assert config["backup"]["mode"] == "operated"
    assert config["backup"]["recovery_key"] == canonical
    assert config["backup"]["confirmed_recovery_key"] is True
    assert "destination" not in config["backup"]
    for secret in (
        "AKID-OPERATED",
        "SAK-OPERATED",
        "SESSION-OPERATED",
        destination.repository,
    ):
        assert secret not in serialized


def test_restore_operated_invalid_key_persists_nothing(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    original_config = {"backup": {"daily_key": "daily-secret"}}
    _write_config(tmp_path, original_config)
    monkeypatch.setattr(
        restore,
        "ensure_restic",
        lambda: pytest.fail("restic should not be resolved"),
    )
    monkeypatch.setattr(
        restore,
        "set_mode",
        lambda mode: pytest.fail("must not persist operated mode"),
    )
    monkeypatch.setattr(
        restore,
        "set_destination",
        lambda destination: pytest.fail("must not persist destination"),
    )

    result = restore.restore_journal_operated(
        _operated_binding(),
        _operated_credentials(),
        "too-short",
    )

    assert result.reason_code == "invalid_key"
    assert _read_config(tmp_path) == original_config


def test_restore_operated_rclone_unavailable_persists_nothing(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    original_config = {"backup": {"daily_key": "daily-secret"}}
    _write_config(tmp_path, original_config)
    monkeypatch.setattr(
        restore,
        "ensure_rclone",
        lambda: (_ for _ in ()).throw(RuntimeError("missing")),
    )
    monkeypatch.setattr(
        restore,
        "ensure_restic",
        lambda: pytest.fail("restic should not be resolved"),
    )

    result = restore.restore_journal_operated(
        _operated_binding(),
        _operated_credentials(),
        "A" * 64,
    )

    assert result.reason_code == "rclone_unavailable"
    assert _read_config(tmp_path) == original_config


def test_restore_operated_restic_failure_persists_nothing(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    original_config = {"backup": {"daily_key": "daily-secret"}}
    _write_config(tmp_path, original_config)
    monkeypatch.setattr(restore, "ensure_restic", lambda: Path("/restic"))
    monkeypatch.setattr(restore, "ensure_rclone", lambda: Path("/rclone"))
    monkeypatch.setattr(restore, "run_restic", lambda args, **kwargs: _result(12))
    monkeypatch.setattr(
        restore,
        "set_mode",
        lambda mode: pytest.fail("must not persist operated mode"),
    )
    monkeypatch.setattr(
        restore,
        "set_destination",
        lambda destination: pytest.fail("must not persist destination"),
    )
    monkeypatch.setattr(
        restore,
        "set_recovery_key",
        lambda key: pytest.fail("must not persist key"),
    )

    result = restore.restore_journal_operated(
        _operated_binding(),
        _operated_credentials(),
        "A" * 64,
    )

    assert result.reason_code == "auth_failed"
    assert _read_config(tmp_path) == original_config


def test_restore_exports_operated_entrypoint() -> None:
    assert "restore_journal_operated" in restore.__all__
