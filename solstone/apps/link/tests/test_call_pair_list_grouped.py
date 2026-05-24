# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from typer.testing import CliRunner

from solstone.apps.link import call
from solstone.think.link.auth import AuthorizedClients
from solstone.think.link.paths import authorized_clients_path

PAIRED_AT = "2026-04-19T00:00:00Z"


def _prepare_env(tmp_path, monkeypatch) -> AuthorizedClients:
    journal = tmp_path / "journal"
    journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setattr(call, "require_solstone", lambda: None)
    return AuthorizedClients(authorized_clients_path())


def test_list_empty_store(tmp_path, monkeypatch) -> None:
    _prepare_env(tmp_path, monkeypatch)

    result = CliRunner().invoke(call.app, ["list"])

    assert result.exit_code == 0
    assert result.stdout == "No devices linked yet.\n"


def test_list_phones_only(tmp_path, monkeypatch) -> None:
    store = _prepare_env(tmp_path, monkeypatch)
    store.add("sha256:aaa", "alpha", "inst-1", paired_at=PAIRED_AT)
    store.add("sha256:bbb", "beta", "inst-1", paired_at=PAIRED_AT)

    result = CliRunner().invoke(call.app, ["list"])

    assert result.exit_code == 0
    assert "Phones:\n" in result.stdout
    assert "Observers:" not in result.stdout
    assert "Peers:" not in result.stdout
    assert result.stdout.count("- ") == 2


def test_list_all_roles(tmp_path, monkeypatch) -> None:
    store = _prepare_env(tmp_path, monkeypatch)
    store.add("sha256:aaa", "phone", "inst-1", paired_at=PAIRED_AT)
    store.add(
        "sha256:bbb", "observer-a", "inst-1", role="observer", paired_at=PAIRED_AT
    )
    store.add(
        "sha256:ccc", "observer-b", "inst-1", role="observer", paired_at=PAIRED_AT
    )
    store.add("sha256:ddd", "peer", "inst-1", role="peer", paired_at=PAIRED_AT)

    result = CliRunner().invoke(call.app, ["list"])

    assert result.exit_code == 0
    phones = result.stdout.index("Phones:")
    observers = result.stdout.index("Observers:")
    peers = result.stdout.index("Peers:")
    assert phones < observers < peers
    assert result.stdout.count("- ") == 4


def test_list_observers_only_omits_phones_heading(tmp_path, monkeypatch) -> None:
    store = _prepare_env(tmp_path, monkeypatch)
    store.add("sha256:bbb", "observer", "inst-1", role="observer", paired_at=PAIRED_AT)

    result = CliRunner().invoke(call.app, ["list"])

    assert result.exit_code == 0
    assert "Phones:" not in result.stdout
    assert "Observers:\n" in result.stdout
    assert "Peers:" not in result.stdout


def test_list_preserves_device_line_shape(tmp_path, monkeypatch) -> None:
    store = _prepare_env(tmp_path, monkeypatch)
    store.add(
        "sha256:0123456789abcdef0000",
        "alpha",
        "inst-1",
        paired_at=PAIRED_AT,
    )

    result = CliRunner().invoke(call.app, ["list"])

    assert result.exit_code == 0
    assert (
        "- alpha — added " in result.stdout
        and " — last seen never [0123456789abcdef]" in result.stdout
    )
