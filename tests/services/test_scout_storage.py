# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import stat
import threading

import pytest

from solstone.think import journal_config
from solstone.think.journal_config import write_journal_config
from solstone.think.services.scout import (
    JournalNotInitializedError,
    is_manual_key_present,
    is_scout_enabled,
    provision_scout_handoff,
    scout_provenance,
)


def _payload(suffix: str = "one") -> dict[str, str]:
    return {
        "google_api_key": f"google-{suffix}",
        "dispatch_token": f"dispatch-{suffix}",
        "account_id": f"acct-{suffix}",
        "created_at": f"2026-05-24T00:00:0{len(suffix)}Z",
    }


def _config_path(journal_copy):
    return journal_copy / "config" / "journal.json"


def _read_config(journal_copy):
    return json.loads(_config_path(journal_copy).read_text())


def test_provision_scout_handoff_round_trip_preserves_config(journal_copy) -> None:
    before = _read_config(journal_copy)

    provision_scout_handoff(_payload())

    after = _read_config(journal_copy)
    assert after["identity"] == before["identity"]
    if "retention" in before:
        assert after["retention"] == before["retention"]
    assert after["convey"] == before["convey"]
    assert after["env"]["GOOGLE_API_KEY"] == "google-one"
    assert after["services"]["scout"]["account_id"] == "acct-one"
    assert after["services"]["scout"]["key_created_at"] == _payload()["created_at"]
    assert after["services"]["scout"]["dispatch_token"] == "dispatch-one"
    assert scout_provenance() == after["services"]["scout"]
    assert stat.S_IMODE(_config_path(journal_copy).stat().st_mode) == 0o600


def test_scout_three_state_matrix(journal_copy) -> None:
    config = _read_config(journal_copy)
    config.setdefault("env", {}).pop("GOOGLE_API_KEY", None)
    config.pop("services", None)
    write_journal_config(config)
    assert not is_scout_enabled()
    assert not is_manual_key_present()
    assert scout_provenance() is None

    config = _read_config(journal_copy)
    config.setdefault("env", {})["GOOGLE_API_KEY"] = "manual"
    write_journal_config(config)
    assert not is_scout_enabled()
    assert is_manual_key_present()

    provision_scout_handoff(_payload("two"))
    assert is_scout_enabled()
    assert not is_manual_key_present()


@pytest.mark.parametrize("field", list(_payload().keys()))
def test_payload_validation_missing_field(journal_copy, field: str) -> None:
    payload = _payload()
    payload.pop(field)

    with pytest.raises(
        ValueError, match=f"malformed handoff payload: missing field '{field}'"
    ):
        provision_scout_handoff(payload)


@pytest.mark.parametrize("field", list(_payload().keys()))
@pytest.mark.parametrize("value", [None, 123, ""])
def test_payload_validation_non_empty_string(journal_copy, field: str, value) -> None:
    payload = _payload()
    payload[field] = value

    with pytest.raises(
        ValueError,
        match=f"malformed handoff payload: field '{field}' must be a non-empty string",
    ):
        provision_scout_handoff(payload)


@pytest.mark.parametrize(
    ("field", "bad_value"),
    [
        ("google_api_key", []),
        ("google_api_key", {}),
        ("google_api_key", 123),
        ("dispatch_token", None),
        ("account_id", ["x"]),
        ("created_at", {"nested": True}),
    ],
)
def test_payload_validation_wrong_type_values(
    journal_copy, field: str, bad_value
) -> None:
    payload = _payload()
    payload[field] = bad_value

    with pytest.raises(ValueError):
        provision_scout_handoff(payload)


def test_provision_preserves_other_env_and_top_level_keys(journal_copy) -> None:
    config = _read_config(journal_copy)
    config["env"] = {
        "GOOGLE_API_KEY": "old",
        "ANTHROPIC_API_KEY": "keep-me",
        "OPENAI_API_KEY": "keep-me-too",
    }
    config["convey"] = {"port": 5015}
    config["custom_block"] = {"survives": True}
    write_journal_config(config)

    provision_scout_handoff(_payload("new"))

    saved = _read_config(journal_copy)
    assert saved["env"]["GOOGLE_API_KEY"] == "google-new"
    assert saved["env"]["ANTHROPIC_API_KEY"] == "keep-me"
    assert saved["env"]["OPENAI_API_KEY"] == "keep-me-too"
    assert saved["convey"] == {"port": 5015}
    assert saved["custom_block"] == {"survives": True}


def test_provision_requires_initialized_journal(tmp_path, monkeypatch) -> None:
    journal = tmp_path / "journal"
    (journal / "config").mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    with pytest.raises(JournalNotInitializedError):
        provision_scout_handoff(_payload())


def test_locked_parallel_writes_do_not_corrupt_config(journal_copy) -> None:
    errors: list[BaseException] = []

    def write_payload(suffix: str) -> None:
        try:
            provision_scout_handoff(_payload(suffix))
        except BaseException as exc:  # pragma: no cover - asserted below
            errors.append(exc)

    threads = [
        threading.Thread(target=write_payload, args=("alpha",)),
        threading.Thread(target=write_payload, args=("bravo",)),
    ]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()

    assert errors == []
    config = _read_config(journal_copy)
    scout = config["services"]["scout"]
    assert scout["account_id"] in {"acct-alpha", "acct-bravo"}
    suffix = scout["account_id"].removeprefix("acct-")
    assert config["env"]["GOOGLE_API_KEY"] == f"google-{suffix}"


def test_atomic_write_leaves_existing_config_on_replace_failure(
    journal_copy, monkeypatch
) -> None:
    original_text = _config_path(journal_copy).read_text()
    config = _read_config(journal_copy)
    config.setdefault("env", {})["GOOGLE_API_KEY"] = "after"

    def fail_replace(_tmp, _path) -> None:
        raise OSError("replace failed")

    monkeypatch.setattr(journal_config.os, "replace", fail_replace)

    with pytest.raises(OSError, match="replace failed"):
        write_journal_config(config)

    assert _config_path(journal_copy).read_text() == original_text
    tmp_path = _config_path(journal_copy).with_suffix(".json.tmp")
    assert json.loads(tmp_path.read_text())["env"]["GOOGLE_API_KEY"] == "after"
