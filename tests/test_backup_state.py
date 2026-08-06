# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import stat
from collections.abc import Callable
from pathlib import Path

import pytest

from solstone.think.backup import state
from solstone.think.backup.destination import Destination
from solstone.think.backup.hosted import HostedBinding, save_hosted_binding
from tests.helpers.journal_config import seed_journal_config


def _config_path(journal: Path) -> Path:
    return journal / "config" / "journal.json"


def _write_config(journal: Path, payload: dict) -> None:
    seed_journal_config(payload, journal)


def _read_config(journal: Path) -> dict:
    return json.loads(_config_path(journal).read_text(encoding="utf-8"))


def _count_transactions(monkeypatch: pytest.MonkeyPatch) -> Callable[[], int]:
    entries = 0
    real_mutate = state.mutate_journal_config

    def recording_mutate(mutator, *args, **kwargs):
        nonlocal entries
        entries += 1
        return real_mutate(mutator, *args, **kwargs)

    monkeypatch.setattr(state, "mutate_journal_config", recording_mutate)
    return lambda: entries


def test_missing_backup_section_defaults(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {"identity": {"name": "Test"}})

    config = state.get_backup_config()

    assert config == state.BACKUP_DEFAULTS
    assert state.get_destination() is None
    assert state.get_keys() is None


def test_partial_backup_section_gets_per_field_defaults(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {"backup": {"enabled": True}})

    config = state.get_backup_config()

    assert config["enabled"] is True
    assert config["mode"] == "byo"
    assert config["destination"] == state.BACKUP_DEFAULTS["destination"]
    assert config["retention"] == state.BACKUP_DEFAULTS["retention"]
    assert config["schedule"] == state.BACKUP_DEFAULTS["schedule"]
    assert config["last_backup"] == state.BACKUP_DEFAULTS["last_backup"]


def test_merge_backup_config_applies_defaults_to_raw_config() -> None:
    config = state.merge_backup_config(
        {
            "backup": {
                "offload": {"enabled": True},
                "last_offload": {
                    "status": "stalled",
                    "reason": "backup_failing",
                },
            }
        }
    )

    assert config["offload"] == {
        "enabled": True,
        "budget_bytes": None,
        "floor_bytes": None,
    }
    assert config["last_offload"] == {
        "time": None,
        "status": "stalled",
        "reason": "backup_failing",
        "last_ok_time": None,
        "files_marked": 0,
        "bytes_marked": 0,
        "ran_out_of_markable_media": False,
    }


def test_generate_and_store_keys_get_or_create(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {})
    monkeypatch.setattr(state, "generate_daily_key", lambda: "generated-daily")
    monkeypatch.setattr(state, "generate_recovery_key", lambda: "A" * 64)

    first = state.generate_and_store_keys()

    assert first.daily_key == "generated-daily"
    assert first.recovery_key == "A" * 64
    assert _read_config(tmp_path)["backup"]["daily_key"] == "generated-daily"

    monkeypatch.setattr(state, "generate_daily_key", lambda: "new-daily")
    monkeypatch.setattr(state, "generate_recovery_key", lambda: "B" * 64)

    second = state.generate_and_store_keys()

    assert second == first
    assert _read_config(tmp_path)["backup"]["recovery_key"] == "A" * 64


def test_generate_and_store_keys_preserves_hand_set_keys(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(
        tmp_path,
        {
            "backup": {
                "daily_key": "manual-daily",
                "recovery_key": "B" * 64,
            }
        },
    )

    monkeypatch.setattr(state, "generate_daily_key", lambda: "unused-generated")
    monkeypatch.setattr(state, "generate_recovery_key", lambda: "C" * 64)

    keys = state.generate_and_store_keys()

    assert keys.daily_key == "manual-daily"
    assert keys.recovery_key == "B" * 64
    assert _read_config(tmp_path)["backup"]["daily_key"] == "manual-daily"
    assert _read_config(tmp_path)["backup"]["recovery_key"] == "B" * 64


def test_set_destination_writes_private_config_mode(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {})

    state.set_destination(
        Destination(
            repository="s3:safe-bucket/path",
            backend="s3",
            credentials={
                "access_key_id": "access-key",
                "secret_access_key": "secret-key",
            },
        )
    )

    assert stat.S_IMODE(_config_path(tmp_path).stat().st_mode) == 0o600


def test_setters_round_trip_under_config_lock(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {})
    transaction_count = _count_transactions(monkeypatch)
    destination = Destination(
        repository="b2:bucket:path",
        backend="b2",
        credentials={
            "account_id": "account-id",
            "account_key": "account-key",
        },
    )

    state.set_destination(destination)
    state.set_recovery_key_confirmed()

    assert transaction_count() == 2
    assert state.get_destination() == destination
    assert _read_config(tmp_path)["backup"]["confirmed_recovery_key"] is True


def test_set_enabled_round_trips_under_config_lock(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {})
    transaction_count = _count_transactions(monkeypatch)

    state.set_enabled(True)
    state.set_enabled(False)

    assert transaction_count() == 2
    assert _read_config(tmp_path)["backup"]["enabled"] is False
    assert state.get_backup_config()["enabled"] is False
    assert stat.S_IMODE(_config_path(tmp_path).stat().st_mode) == 0o600


def test_set_mode_round_trips_under_config_lock(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {})
    transaction_count = _count_transactions(monkeypatch)

    state.set_mode("operated")
    state.set_mode("byo")

    assert transaction_count() == 2
    assert _read_config(tmp_path)["backup"]["mode"] == "byo"
    assert state.get_backup_config()["mode"] == "byo"
    assert stat.S_IMODE(_config_path(tmp_path).stat().st_mode) == 0o600


def test_set_mode_rejects_invalid_value_without_writing(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    original = {"backup": {"mode": "byo"}}
    _write_config(tmp_path, original)
    before = _read_config(tmp_path)
    transaction_count = _count_transactions(monkeypatch)

    with pytest.raises(ValueError):
        state.set_mode("invalid")

    assert transaction_count() == 0
    assert _read_config(tmp_path) == before


def test_set_retention_round_trips_under_config_lock(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {})
    transaction_count = _count_transactions(monkeypatch)

    state.set_retention({"hourly": 1, "daily": 2, "weekly": 3, "monthly": 4})

    assert transaction_count() == 1
    assert _read_config(tmp_path)["backup"]["retention"] == {
        "hourly": 1,
        "daily": 2,
        "weekly": 3,
        "monthly": 4,
    }
    assert state.get_backup_config()["retention"] == {
        "hourly": 1,
        "daily": 2,
        "weekly": 3,
        "monthly": 4,
    }
    assert stat.S_IMODE(_config_path(tmp_path).stat().st_mode) == 0o600


@pytest.mark.parametrize(
    "retention",
    [
        {"hourly": 1, "daily": 2, "weekly": 3},
        {"hourly": 1, "daily": 2, "weekly": 3, "monthly": 4, "yearly": 5},
        {"hourly": True, "daily": 2, "weekly": 3, "monthly": 4},
        {"hourly": "1", "daily": 2, "weekly": 3, "monthly": 4},
        {"hourly": -1, "daily": 2, "weekly": 3, "monthly": 4},
    ],
)
def test_set_retention_rejects_invalid_values_without_writing(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    retention: dict[str, object],
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    original = {
        "backup": {"retention": {"hourly": 9, "daily": 8, "weekly": 7, "monthly": 6}}
    }
    _write_config(tmp_path, original)
    before = _read_config(tmp_path)
    transaction_count = _count_transactions(monkeypatch)

    with pytest.raises(ValueError):
        state.set_retention(retention)  # type: ignore[arg-type]

    assert transaction_count() == 0
    assert _read_config(tmp_path) == before


def test_set_recovery_key_writes_known_key_only(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(
        tmp_path,
        {
            "backup": {
                "daily_key": "daily-secret",
                "recovery_key": "A" * 64,
                "confirmed_recovery_key": True,
            }
        },
    )

    state.set_recovery_key("B" * 64)

    backup = _read_config(tmp_path)["backup"]
    assert backup["daily_key"] == "daily-secret"
    assert backup["recovery_key"] == "B" * 64
    assert backup["confirmed_recovery_key"] is True
    assert stat.S_IMODE(_config_path(tmp_path).stat().st_mode) == 0o600


def test_clear_backup_config_resets_backup_section(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(
        tmp_path,
        {
            "identity": {"name": "Test"},
            "backup": {
                "enabled": True,
                "daily_key": "daily-secret",
                "recovery_key": "C" * 64,
            },
        },
    )

    state.clear_backup_config()

    config = _read_config(tmp_path)
    assert config["identity"] == {"name": "Test"}
    assert config["backup"] == state.BACKUP_DEFAULTS
    assert stat.S_IMODE(_config_path(tmp_path).stat().st_mode) == 0o600


def test_record_backup_result_writes_last_backup_under_config_lock(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {})
    transaction_count = _count_transactions(monkeypatch)

    state.record_backup_result(
        status="error",
        time=123,
        snapshot_id="partial-snapshot",
        error_reason="incomplete",
    )

    assert transaction_count() == 1
    assert _read_config(tmp_path)["backup"]["last_backup"] == {
        "time": 123,
        "snapshot_id": "partial-snapshot",
        "status": "error",
        "error_reason": "incomplete",
    }
    assert stat.S_IMODE(_config_path(tmp_path).stat().st_mode) == 0o600


def test_record_prune_result_writes_last_prune(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {})
    transaction_count = _count_transactions(monkeypatch)

    state.record_prune_result(
        status="error",
        time=456,
        error_reason="timeout",
    )

    assert transaction_count() == 1
    assert _read_config(tmp_path)["backup"]["last_prune"] == {
        "time": 456,
        "status": "error",
        "error_reason": "timeout",
    }
    assert "snapshot_id" not in _read_config(tmp_path)["backup"]["last_prune"]


def test_status_view_redacts_all_secrets(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(
        tmp_path,
        {
            "backup": {
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
                "recovery_key": "C" * 64,
                "confirmed_recovery_key": True,
            }
        },
    )

    view = state.status_view()
    serialized = json.dumps(view)

    for secret in ("daily-secret", "C" * 64, "access-key", "secret-key"):
        assert secret not in serialized
    assert view["destination"] == {
        "repository": "s3:safe-bucket/path",
        "backend": "s3",
        "credentials_set": True,
    }
    assert view["daily_key_set"] is True
    assert view["recovery_key_set"] is True
    assert view["recovery_key_confirmed"] is True
    assert view["last_prune"] == state.BACKUP_DEFAULTS["last_prune"]


def test_status_view_reports_hosted_binding_without_secrets(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {})

    unbound = state.status_view()
    unbound_serialized = json.dumps(unbound)

    assert unbound["hosted"] == {"bound": False}
    assert "broker_token" not in unbound_serialized

    save_hosted_binding(
        HostedBinding(
            broker_endpoint="https://broker.example",
            account_id="acct",
            instance_id="inst",
            bucket="bkt",
            prefix="users/acct/inst/",
            broker_token="BTOKEN",
        )
    )

    bound = state.status_view()
    bound_serialized = json.dumps(bound)

    assert "broker_token" not in bound_serialized
    assert "broker_endpoint" not in bound_serialized
    assert "account_id" not in bound_serialized
    assert "instance_id" not in bound_serialized
    assert "BTOKEN" not in bound_serialized
    assert bound["hosted"] == {
        "bound": True,
        "bucket": "bkt",
        "prefix": "users/acct/inst/",
    }


def test_state_exports_set_mode() -> None:
    assert "set_mode" in state.__all__
