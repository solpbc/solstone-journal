# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import stat
from pathlib import Path

import pytest

from solstone.think import journal_config, utils


@pytest.fixture(autouse=True)
def reset_default_config(monkeypatch):
    monkeypatch.setattr(utils, "_default_config", None)


@pytest.fixture(autouse=True)
def use_built_core(monkeypatch):
    helper = Path(__file__).resolve().parents[1] / "core" / "target" / "debug" / "solstone-core"
    monkeypatch.setattr(
        journal_config.core_handshake,
        "check_solstone_core_handshake",
        lambda: journal_config.core_handshake.CoreHandshakeResult("ok"),
    )
    monkeypatch.setattr(
        journal_config.core_handshake,
        "helper_path_for_executable",
        lambda: helper,
    )


def _config_path(journal):
    return journal / "config" / "journal.json"


def test_ensure_journal_config_is_idempotent(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    journal_config.ensure_journal_config()
    config_path = _config_path(tmp_path)
    first = config_path.stat()
    journal_config.ensure_journal_config()
    second = config_path.stat()

    assert second.st_ino == first.st_ino
    assert second.st_size == first.st_size


def test_ensure_journal_config_file_mode_is_private(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    journal_config.ensure_journal_config()

    assert stat.S_IMODE(_config_path(tmp_path).stat().st_mode) == 0o600


def test_ensure_journal_config_reads_existing_config_without_touching_identity(
    tmp_path, monkeypatch
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    config_path = _config_path(tmp_path)
    config_path.parent.mkdir(parents=True)
    staged = {
        "identity": {
            "name": "Existing User",
            "preferred": "Existing",
            "timezone": "UTC",
        },
    }
    config_path.write_text(json.dumps(staged), encoding="utf-8")

    config = journal_config.ensure_journal_config()

    assert config == staged


def test_ensure_journal_config_raises_on_corrupt_existing_config_without_writing(
    tmp_path, monkeypatch
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    config_path = _config_path(tmp_path)
    config_path.parent.mkdir(parents=True)
    config_path.write_bytes(b"{ invalid json }")
    before = config_path.read_bytes()

    with pytest.raises(utils.CorruptConfigError):
        journal_config.ensure_journal_config()

    assert config_path.read_bytes() == before


def test_ensure_journal_config_returned_dict_does_not_mutate_defaults(
    tmp_path, monkeypatch
):
    first_journal = tmp_path / "first"
    second_journal = tmp_path / "second"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(first_journal))
    config = journal_config.ensure_journal_config()
    config["identity"]["name"] = "Mutated"

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(second_journal))
    fresh = utils.get_config()

    assert fresh["identity"]["name"] == ""
