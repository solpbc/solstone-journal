# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace

import pytest


@pytest.fixture(autouse=True)
def observer_install_env(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    home = tmp_path / "home"
    journal = tmp_path / "journal"
    home.mkdir()
    journal.mkdir()
    monkeypatch.setattr(Path, "home", lambda: home)
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    import solstone.convey.state as convey_state
    from solstone.observe.observer_install import linux, tmux

    convey_state.journal_root = ""
    monkeypatch.setattr(
        linux,
        "CONFIG_PATH",
        home / ".local" / "share" / "solstone-linux" / "config" / "config.json",
    )
    monkeypatch.setattr(
        tmux,
        "CONFIG_PATH",
        home / ".local" / "share" / "solstone-tmux" / "config" / "config.json",
    )
    return SimpleNamespace(home=home, journal=journal)


@pytest.fixture
def args_factory():
    def build(**overrides):
        data = {
            "name": "archon",
            "platform": "linux",
            "server_url": "http://127.0.0.1:5015",
            "dry_run": False,
            "force": False,
            "json_output": False,
            "observer_version": None,
        }
        data.update(overrides)
        return SimpleNamespace(**data)

    return build
