# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import argparse
import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from solstone.apps.observer.utils import list_observers, save_observer
from solstone.observe import observer_cli


@pytest.fixture
def observer_cli_env(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    home = tmp_path / "home"
    journal = tmp_path / "journal"
    home.mkdir()
    journal.mkdir()
    monkeypatch.setattr(Path, "home", lambda: home)
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    import solstone.convey.state as convey_state

    convey_state.journal_root = ""
    return SimpleNamespace(home=home, journal=journal)


def _observer(name: str = "archon", key: str = "existing-key-abcdef") -> dict:
    return {
        "key": key,
        "name": name,
        "created_at": 1,
        "last_seen": None,
        "last_segment": None,
        "enabled": True,
        "stats": {"segments_received": 0, "bytes_received": 0},
    }


def test_create_observer_record_reuses_existing_without_create_side_effects(
    observer_cli_env,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    existing = _observer()
    assert save_observer(existing)
    monkeypatch.setattr(
        observer_cli,
        "_generate_key",
        lambda: pytest.fail("reuse must not generate a new key"),
    )
    monkeypatch.setattr(
        observer_cli,
        "save_observer",
        lambda _data: pytest.fail("reuse must not save"),
    )
    monkeypatch.setattr(
        observer_cli,
        "log_app_action",
        lambda **_kwargs: pytest.fail("reuse must not log observer_create"),
    )

    record, key, reused = observer_cli.create_observer_record(
        "archon", reuse_existing=True
    )

    assert record == existing
    assert key == "existing-key-abcdef"
    assert reused is True
    assert list_observers() == [existing]


def test_create_observer_record_fresh_create_returns_reused_false_and_logs(
    observer_cli_env,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    logs = []
    monkeypatch.setattr(observer_cli, "_generate_key", lambda: "fresh-key-abcdef")
    monkeypatch.setattr(
        observer_cli, "log_app_action", lambda **kwargs: logs.append(kwargs)
    )

    record, key, reused = observer_cli.create_observer_record("archon")

    assert key == "fresh-key-abcdef"
    assert reused is False
    assert record["name"] == "archon"
    assert list_observers()[0]["key"] == "fresh-key-abcdef"
    assert logs == [
        {
            "app": "observer",
            "facet": None,
            "action": "observer_create",
            "params": {"name": "archon", "key_prefix": "fresh-ke"},
        }
    ]


def test_create_observer_record_duplicate_without_reuse_still_fails(
    observer_cli_env,
) -> None:
    assert save_observer(_observer())

    with pytest.raises(ValueError, match="observer already exists: archon"):
        observer_cli.create_observer_record("archon")


def test_cmd_create_duplicate_without_reuse_exits_one(
    observer_cli_env,
    capsys: pytest.CaptureFixture[str],
) -> None:
    assert save_observer(_observer())
    args = argparse.Namespace(
        name="archon",
        json_output=False,
        reuse_existing=False,
    )

    rc = observer_cli.cmd_create(args)

    captured = capsys.readouterr()
    assert rc == 1
    assert captured.out == ""
    assert captured.err == "Error: observer 'archon' already exists\n"


def test_cmd_create_reuse_existing_json_shape(
    observer_cli_env,
    capsys: pytest.CaptureFixture[str],
) -> None:
    existing = _observer()
    assert save_observer(existing)
    args = argparse.Namespace(
        name="archon",
        json_output=True,
        reuse_existing=True,
    )

    rc = observer_cli.cmd_create(args)

    captured = capsys.readouterr()
    assert rc == 0
    assert captured.err == ""
    assert captured.out == (
        json.dumps(
            {
                "name": "archon",
                "key": "existing-key-abcdef",
                "prefix": "existing",
            }
        )
        + "\n"
    )


def test_cmd_create_reuse_existing_human_header(
    observer_cli_env,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    existing = _observer()
    assert save_observer(existing)
    monkeypatch.setattr(
        observer_cli,
        "get_config",
        lambda: {"convey": {"allow_network_access": True}},
    )
    args = argparse.Namespace(
        name="archon",
        json_output=False,
        reuse_existing=True,
    )

    rc = observer_cli.cmd_create(args)

    captured = capsys.readouterr()
    assert rc == 0
    assert captured.err == ""
    assert "Reusing existing observer:" in captured.out
    assert "Observer created:" not in captured.out
    assert "  api key:     existing-key-abcdef" in captured.out


def test_cmd_create_reuse_existing_creates_normally_when_absent(
    observer_cli_env,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    logs = []
    monkeypatch.setattr(observer_cli, "_generate_key", lambda: "fresh-key-abcdef")
    monkeypatch.setattr(
        observer_cli, "log_app_action", lambda **kwargs: logs.append(kwargs)
    )
    monkeypatch.setattr(
        observer_cli,
        "get_config",
        lambda: {"convey": {"allow_network_access": True}},
    )
    args = argparse.Namespace(
        name="archon",
        json_output=False,
        reuse_existing=True,
    )

    rc = observer_cli.cmd_create(args)

    captured = capsys.readouterr()
    assert rc == 0
    assert captured.err == ""
    assert "Observer created:" in captured.out
    assert "Reusing existing observer:" not in captured.out
    assert "  api key:     fresh-key-abcdef" in captured.out
    assert list_observers()[0]["key"] == "fresh-key-abcdef"
    assert logs == [
        {
            "app": "observer",
            "facet": None,
            "action": "observer_create",
            "params": {"name": "archon", "key_prefix": "fresh-ke"},
        }
    ]
