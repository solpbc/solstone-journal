# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import base64
import json
import stat
import urllib.error
from pathlib import Path
from typing import Any

import pytest

from solstone.think.journal_config import write_journal_config
from solstone.think.link import relay_client
from solstone.think.link.paths import (
    authorized_clients_path,
    load_service_token,
    load_totp_secret,
    save_totp_secret,
    totp_secret_path,
)
from solstone.think.services import spl


def _config_path(journal_copy: Path) -> Path:
    return journal_copy / "config" / "journal.json"


def _read_config(journal_copy: Path) -> dict[str, Any]:
    return json.loads(_config_path(journal_copy).read_text("utf-8"))


def _write_posture(journal_copy: Path, posture: str) -> None:
    config = _read_config(journal_copy)
    config.setdefault("link", {})["posture"] = posture
    write_journal_config(config)


def _install_relay(
    monkeypatch: pytest.MonkeyPatch,
    captured: list[tuple[str, dict[str, Any]]],
) -> None:
    monkeypatch.setenv("SOL_LINK_RELAY_URL", "https://relay.test")

    def post_json(url: str, body: dict[str, Any]) -> dict[str, str]:
        captured.append((url, body))
        return {"service_token": "tok.spl"}

    monkeypatch.setattr(relay_client, "_post_json_sync", post_json)


def test_enable_spl_writes_posture_secret_and_service_token(
    journal_copy: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    captured: list[tuple[str, dict[str, Any]]] = []
    _install_relay(monkeypatch, captured)

    spl.enable_spl()

    config = _read_config(journal_copy)
    secret = load_totp_secret()
    assert config["link"]["posture"] == "spl"
    assert secret is not None
    assert stat.S_IMODE(totp_secret_path().stat().st_mode) == 0o600
    decoded = base64.b32decode(secret + "=" * ((8 - len(secret) % 8) % 8))
    assert len(decoded) >= 20
    assert captured[0][0] == "https://relay.test/enroll/home"
    assert captured[0][1]["totp_secret"] == secret
    assert captured[0][1]["instance_id"]
    assert captured[0][1]["ca_pubkey"]
    assert captured[0][1]["home_label"]
    assert load_service_token() == "tok.spl"


def test_enable_spl_does_not_write_secret_to_journal_config(
    journal_copy: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    captured: list[tuple[str, dict[str, Any]]] = []
    _install_relay(monkeypatch, captured)

    spl.enable_spl()

    config_text = json.dumps(_read_config(journal_copy))
    assert "totp" not in config_text


def test_enable_spl_does_not_regenerate_existing_secret(
    journal_copy: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    captured: list[tuple[str, dict[str, Any]]] = []
    _install_relay(monkeypatch, captured)

    spl.enable_spl()
    first_secret = load_totp_secret()
    spl.enable_spl()

    assert load_totp_secret() == first_secret
    assert captured[0][1]["totp_secret"] == first_secret
    assert captured[1][1]["totp_secret"] == first_secret
    assert spl.is_spl_enabled()


def test_disable_spl_when_enabled_parks_relay_state(
    journal_copy: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    captured: list[tuple[str, dict[str, Any]]] = []
    _install_relay(monkeypatch, captured)
    spl.enable_spl()
    authorized_clients_path().write_text('{"clients": []}\n', encoding="utf-8")
    authorized_text = authorized_clients_path().read_text("utf-8")

    outcome = spl.disable_spl()

    assert outcome == spl.SplDisableOutcome(was_enabled=True)
    assert _read_config(journal_copy)["link"]["posture"] == "direct"
    assert totp_secret_path().exists()
    assert load_service_token() == "tok.spl"
    assert authorized_clients_path().read_text("utf-8") == authorized_text


def test_disable_spl_when_already_direct_returns_was_enabled_false(
    journal_copy: Path,
) -> None:
    _write_posture(journal_copy, "direct")

    outcome = spl.disable_spl()

    assert outcome == spl.SplDisableOutcome(was_enabled=False)


def test_is_spl_enabled_matrix(journal_copy: Path) -> None:
    _write_posture(journal_copy, "direct")
    assert not spl.is_spl_enabled()

    _write_posture(journal_copy, "spl")
    assert not spl.is_spl_enabled()

    _write_posture(journal_copy, "direct")
    save_totp_secret("SECRET")
    assert not spl.is_spl_enabled()

    _write_posture(journal_copy, "spl")
    assert spl.is_spl_enabled()


def test_enable_spl_relay_down_leaves_posture_direct(
    journal_copy: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _write_posture(journal_copy, "direct")
    monkeypatch.setenv("SOL_LINK_RELAY_URL", "https://relay.test")

    def post_json(_url: str, _body: dict[str, Any]) -> dict[str, str]:
        raise urllib.error.URLError("down")

    monkeypatch.setattr(relay_client, "_post_json_sync", post_json)

    with pytest.raises(spl.RelayUnreachableError):
        spl.enable_spl()

    assert _read_config(journal_copy)["link"]["posture"] == "direct"


def test_require_journal_config_raises_on_uninitialized_journal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    journal = tmp_path / "journal"
    (journal / "config").mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    with pytest.raises(spl.JournalNotInitializedError):
        spl._require_journal_config()
