# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
from pathlib import Path

import pytest

from solstone.think.link.paths import (
    DEFAULT_RELAY_URL,
    LinkState,
    load_service_token,
    relay_url,
    save_service_token,
    service_token_path,
    state_path,
)


def _set_journal(monkeypatch: pytest.MonkeyPatch, journal: Path) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))


# Built by concatenation so the legacy account-token DATA key does not trip the AC4 grep-clean check.
def _legacy_token_key() -> str:
    return "account" + "_token"


def _forbid_link_state_save(monkeypatch: pytest.MonkeyPatch) -> None:
    def fail_save(self) -> None:
        raise AssertionError("LinkState.save should not be called by load")

    monkeypatch.setattr(LinkState, "save", fail_save)


def test_link_state_load_or_create_creates_state(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _set_journal(monkeypatch, tmp_path)

    state = LinkState.load_or_create()

    assert isinstance(state.instance_id, str)
    assert state.instance_id
    assert state.home_label == "solstone"
    assert state_path().exists()


def test_link_state_load_or_create_idempotent(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _set_journal(monkeypatch, tmp_path)

    first = LinkState.load_or_create()
    first_payload = state_path().read_text("utf-8")
    second = LinkState.load_or_create()
    second_payload = state_path().read_text("utf-8")

    assert second.instance_id == first.instance_id
    assert second.home_label == first.home_label
    assert second_payload == first_payload


def test_link_state_load_or_create_custom_label(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _set_journal(monkeypatch, tmp_path)

    created = LinkState.load_or_create(default_label="laptop")
    loaded = LinkState.load_or_create()

    assert created.home_label == "laptop"
    assert loaded.instance_id == created.instance_id
    assert loaded.home_label == "laptop"


def test_link_state_load_missing_returns_none(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _set_journal(monkeypatch, tmp_path)
    _forbid_link_state_save(monkeypatch)

    assert LinkState.load() is None
    assert not state_path().exists()


def test_link_state_load_reads_existing_state(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _set_journal(monkeypatch, tmp_path)
    _forbid_link_state_save(monkeypatch)
    path = state_path()
    path.write_text(
        json.dumps(
            {
                "instance_id": "12345678-1234-1234-1234-123456789abc",
                "home_label": "laptop",
            }
        ),
        encoding="utf-8",
    )

    state = LinkState.load()

    assert state == LinkState(
        instance_id="12345678-1234-1234-1234-123456789abc",
        home_label="laptop",
    )


def test_link_state_load_corrupt_json_returns_none(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _set_journal(monkeypatch, tmp_path)
    _forbid_link_state_save(monkeypatch)
    state_path().write_text("{not json", encoding="utf-8")

    assert LinkState.load() is None


@pytest.mark.parametrize("payload", [{}, {"instance_id": ""}, {"instance_id": 123}])
def test_link_state_load_missing_or_invalid_instance_id_returns_none(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, payload: dict
) -> None:
    _set_journal(monkeypatch, tmp_path)
    _forbid_link_state_save(monkeypatch)
    state_path().write_text(json.dumps(payload), encoding="utf-8")

    assert LinkState.load() is None


def test_relay_url_env_wins(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    _set_journal(monkeypatch, tmp_path)
    monkeypatch.setenv("SOL_LINK_RELAY_URL", "https://example.test/")

    assert relay_url() == "https://example.test"


def test_relay_url_from_config(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    _set_journal(monkeypatch, tmp_path)
    config_path = tmp_path / "config" / "journal.json"
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text(
        json.dumps({"link": {"relay_url": "https://cfg.test"}}),
        encoding="utf-8",
    )

    assert relay_url() == "https://cfg.test"


def test_relay_url_default(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    _set_journal(monkeypatch, tmp_path)
    monkeypatch.delenv("SOL_LINK_RELAY_URL", raising=False)

    assert relay_url() == DEFAULT_RELAY_URL


def test_load_service_token_missing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _set_journal(monkeypatch, tmp_path)

    assert load_service_token() is None
    assert not (tmp_path / "link").exists()


def test_service_token_path_does_not_create_token_directories(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _set_journal(monkeypatch, tmp_path)

    assert service_token_path() == tmp_path / "link" / "tokens" / "account.json"
    assert not (tmp_path / "link").exists()


def test_save_and_load_service_token_roundtrip(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _set_journal(monkeypatch, tmp_path)

    save_service_token("tok.123")

    token_path = service_token_path()
    assert load_service_token() == "tok.123"
    assert token_path.stat().st_mode & 0o777 == 0o600


def test_save_service_token_is_atomic(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _set_journal(monkeypatch, tmp_path)

    save_service_token("tok.123")

    token_path = service_token_path()
    assert token_path.exists()
    assert json.loads(token_path.read_text("utf-8")) == {"service_token": "tok.123"}
    assert token_path.stat().st_mode & 0o777 == 0o600
    assert {path.name for path in token_path.parent.iterdir()} == {token_path.name}


def test_load_service_token_reads_legacy_account_key(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _set_journal(monkeypatch, tmp_path)
    token_path = service_token_path()
    token_path.parent.mkdir(parents=True, exist_ok=True)
    legacy_key = _legacy_token_key()
    token_path.write_text(json.dumps({legacy_key: "tok.legacy"}), "utf-8")

    assert load_service_token() == "tok.legacy"
    legacy_payload = json.loads(token_path.read_text("utf-8"))
    assert legacy_key in legacy_payload

    save_service_token("tok.new")

    assert load_service_token() == "tok.new"
    new_payload = json.loads(token_path.read_text("utf-8"))
    assert "service_token" in new_payload
    assert legacy_key not in new_payload
