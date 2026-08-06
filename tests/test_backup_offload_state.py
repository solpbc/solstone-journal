# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
from pathlib import Path

import pytest

from solstone.think.backup import state


def _config_path(journal: Path) -> Path:
    return journal / "config" / "journal.json"


def _write_config(journal: Path, payload: dict) -> None:
    config_path = _config_path(journal)
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


def _read_config(journal: Path) -> dict:
    return json.loads(_config_path(journal).read_text(encoding="utf-8"))


def test_missing_offload_key_reads_pinned_defaults(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {"backup": {"enabled": True}})

    assert state.get_backup_config()["offload"] == {
        "enabled": False,
        "budget_bytes": None,
        "floor_bytes": None,
    }


def test_set_offload_valid_write_round_trips(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {})
    offload = {
        "enabled": True,
        "budget_bytes": 500_000_000_000,
        "floor_bytes": None,
    }

    state.set_offload(offload)

    assert _read_config(tmp_path)["backup"]["offload"] == offload
    assert state.get_backup_config()["offload"] == offload


@pytest.mark.parametrize(
    "offload",
    [
        {"enabled": 1, "budget_bytes": None, "floor_bytes": None},
        {"enabled": "true", "budget_bytes": None, "floor_bytes": None},
        {"enabled": True, "budget_bytes": True, "floor_bytes": None},
        {"enabled": True, "budget_bytes": False, "floor_bytes": None},
        {"enabled": True, "budget_bytes": 0, "floor_bytes": None},
        {"enabled": True, "budget_bytes": -1, "floor_bytes": None},
        {"enabled": True, "budget_bytes": "1", "floor_bytes": None},
        {"enabled": True, "budget_bytes": None, "floor_bytes": 0},
        {"enabled": True, "budget_bytes": None, "floor_bytes": -1},
        {"enabled": True, "budget_bytes": None, "floor_bytes": "1"},
        {"enabled": True, "budget_bytes": None},
        {
            "enabled": True,
            "budget_bytes": None,
            "floor_bytes": None,
            "extra": 1,
        },
    ],
)
def test_set_offload_rejects_invalid_shapes_without_writing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, offload: dict
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    original = {"backup": {"enabled": False}}
    _write_config(tmp_path, original)
    before = _read_config(tmp_path)

    with pytest.raises(ValueError):
        state.set_offload(offload)

    assert _read_config(tmp_path) == before


def test_set_offload_preserves_existing_backup_section_without_materializing_defaults(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    backup = {
        "enabled": False,
        "mode": "operated",
        "retention": {"hourly": 2, "daily": 3, "weekly": 4, "monthly": 5},
        "custom_marker": {"nested": ["keep", 7]},
    }
    _write_config(tmp_path, {"backup": backup})
    before_backup = _read_config(tmp_path)["backup"]

    state.set_offload(
        {
            "enabled": True,
            "budget_bytes": 10,
            "floor_bytes": 5,
        }
    )

    after_backup = _read_config(tmp_path)["backup"]
    assert set(after_backup) == {*before_backup, "offload"}
    for key, value in before_backup.items():
        assert json.dumps(after_backup[key], sort_keys=True) == json.dumps(
            value, sort_keys=True
        )
    assert "destination" not in after_backup
    assert "last_prune" not in after_backup


def test_set_offload_has_no_cross_field_backup_enabled_rule(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {"backup": {"enabled": False}})

    state.set_offload(
        {
            "enabled": True,
            "budget_bytes": 10,
            "floor_bytes": 20,
        }
    )

    assert _read_config(tmp_path)["backup"]["enabled"] is False
    assert _read_config(tmp_path)["backup"]["offload"] == {
        "enabled": True,
        "budget_bytes": 10,
        "floor_bytes": 20,
    }


def test_offload_state_records_and_status_view(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {"backup": {}})

    config = state.get_backup_config()
    assert config["last_offload"] == {
        "time": None,
        "status": None,
        "reason": None,
        "last_ok_time": None,
        "files_marked": 0,
        "bytes_marked": 0,
        "ran_out_of_markable_media": False,
    }
    assert config["last_verification"] == {
        "time": None,
        "status": None,
        "reason": None,
        "last_ok_time": None,
        "checked_subset": None,
    }
    assert config["last_restore"] == {
        "time": None,
        "status": None,
        "reason": None,
        "scope": None,
        "day": None,
        "segments_selected": 0,
        "segments_restored": 0,
        "files_expected": 0,
        "files_restored": 0,
        "bytes_expected": 0,
        "bytes_restored": 0,
    }

    state.record_offload_result(
        status="ok",
        time=100,
        reason=None,
        files_marked=2,
        bytes_marked=50,
        ran_out_of_markable_media=False,
    )
    state.record_offload_result(
        status="stalled",
        time=123,
        reason="backup_not_ready",
        files_marked=1,
        bytes_marked=25,
        ran_out_of_markable_media=False,
    )
    state.record_verification_result(status="skipped", time=None, reason="disabled")
    state.record_restore_result(
        status="ok",
        time=200,
        reason=None,
        scope="day",
        day="20260101",
        segments_selected=2,
        segments_restored=2,
        files_expected=5,
        files_restored=5,
        bytes_expected=500,
        bytes_restored=500,
    )
    state.record_restore_result(
        status="error",
        time=210,
        reason="verification_failed",
        scope="day",
        day="20260102",
        segments_selected=1,
        segments_restored=0,
        files_expected=2,
        files_restored=0,
        bytes_expected=200,
        bytes_restored=0,
    )

    backup = _read_config(tmp_path)["backup"]
    assert backup["last_offload"] == {
        "time": 123,
        "status": "stalled",
        "reason": "backup_not_ready",
        "last_ok_time": 100,
        "files_marked": 1,
        "bytes_marked": 25,
        "ran_out_of_markable_media": False,
    }
    assert backup["last_verification"] == {
        "time": None,
        "status": "skipped",
        "reason": "disabled",
        "last_ok_time": None,
        "checked_subset": None,
    }
    assert backup["last_restore"] == {
        "time": 210,
        "status": "error",
        "reason": "verification_failed",
        "scope": "day",
        "day": "20260102",
        "segments_selected": 1,
        "segments_restored": 0,
        "files_expected": 2,
        "files_restored": 0,
        "bytes_expected": 200,
        "bytes_restored": 0,
    }
    with pytest.raises(ValueError):
        state.record_offload_result(
            status="degraded",
            time=1,
            files_marked=0,
            bytes_marked=0,
            ran_out_of_markable_media=False,
        )
    with pytest.raises(ValueError):
        state.record_verification_result(status="stalled", time=1)
    with pytest.raises(ValueError):
        state.record_restore_result(
            status="skipped",
            time=1,
            reason=None,
            scope="day",
            day="20260101",
            segments_selected=0,
            segments_restored=0,
            files_expected=0,
            files_restored=0,
            bytes_expected=0,
            bytes_restored=0,
        )

    view = state.status_view()
    assert view["offload"] == {
        "enabled": False,
        "budget_bytes": None,
        "floor_bytes": None,
    }
    assert view["last_offload"] == backup["last_offload"]
    assert view["last_verification"] == backup["last_verification"]
    assert view["last_restore"] == backup["last_restore"]


def test_verification_record_preserves_last_ok_and_clears_failed_subset(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {"backup": {}})

    state.record_verification_result(
        status="ok",
        time=100,
        reason=None,
        checked_subset="7/52",
    )
    assert _read_config(tmp_path)["backup"]["last_verification"] == {
        "time": 100,
        "status": "ok",
        "reason": None,
        "last_ok_time": 100,
        "checked_subset": "7/52",
    }

    state.record_verification_result(
        status="error",
        time=200,
        reason="locked",
        checked_subset="8/52",
    )

    assert _read_config(tmp_path)["backup"]["last_verification"] == {
        "time": 200,
        "status": "error",
        "reason": "locked",
        "last_ok_time": 100,
        "checked_subset": None,
    }
