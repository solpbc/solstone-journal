# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json
from pathlib import Path

import pytest
from typer.testing import CliRunner

from solstone.think.tools.call import retention_app
from tests.helpers.journal_config import seed_journal_config

runner = CliRunner()


@pytest.fixture
def journal_env(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    return tmp_path


def _write_config(journal_path: Path, config: dict) -> None:
    seed_journal_config(config, journal_path)


def _load_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def test_show_default(journal_env):
    result = runner.invoke(retention_app, ["config"])

    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert payload == {"default": {"mode": "keep", "days": None}, "per_stream": {}}


def test_show_custom(journal_env):
    _write_config(journal_env, {"retention": {"raw_media": "keep"}})

    result = runner.invoke(retention_app, ["config"])

    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert payload["default"]["mode"] == "keep"


def test_set_mode_and_days(journal_env):
    result = runner.invoke(
        retention_app, ["config", "--mode", "days", "--days", "30"]
    )

    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert payload["default"] == {"mode": "days", "days": 30}

    config_path = journal_env / "config" / "journal.json"
    saved = _load_json(config_path)
    assert saved["retention"]["raw_media"] == "days"
    assert saved["retention"]["raw_media_days"] == 30
    assert config_path.stat().st_mode & 0o777 == 0o600


def test_set_mode_days_without_days_flag(journal_env):
    result = runner.invoke(
        retention_app, ["config", "--mode", "days"]
    )

    assert result.exit_code == 1
    assert "--days is required when mode is 'days'." in result.output


def test_set_per_stream(journal_env):
    result = runner.invoke(
        retention_app,
        [
            "config",
            "--stream",
            "plaud",
            "--mode",
            "processed",
        ],
    )

    assert result.exit_code == 0
    saved = _load_json(journal_env / "config" / "journal.json")
    assert saved["retention"]["per_stream"]["plaud"]["raw_media"] == "processed"


def test_clear_per_stream(journal_env):
    _write_config(
        journal_env,
        {
            "retention": {
                "raw_media": "days",
                "raw_media_days": 7,
                "per_stream": {"plaud": {"raw_media": "processed"}},
            }
        },
    )

    result = runner.invoke(
        retention_app,
        ["config", "--stream", "plaud", "--clear"],
    )

    assert result.exit_code == 0
    saved = _load_json(journal_env / "config" / "journal.json")
    assert saved["retention"].get("per_stream") is None


def test_clear_without_stream(journal_env):
    result = runner.invoke(retention_app, ["config", "--clear"])

    assert result.exit_code == 1
    assert "--clear requires --stream" in result.output


def test_invalid_mode(journal_env):
    result = runner.invoke(
        retention_app, ["config", "--mode", "invalid"]
    )

    assert result.exit_code == 1
    assert "Invalid mode: invalid. Must be keep, days, or processed." in result.output


def test_clear_with_mode_rejected(journal_env):
    result = runner.invoke(
        retention_app,
        [
            "config",
            "--stream",
            "plaud",
            "--clear",
            "--mode",
            "keep",
        ],
    )

    assert result.exit_code == 1
    assert "--clear cannot be combined with --mode or --days" in result.output


def test_negative_days_rejected(journal_env):
    result = runner.invoke(
        retention_app, ["config", "--mode", "keep", "--days", "-1"]
    )

    assert result.exit_code == 1
    assert "--days must be a positive integer" in result.output


def test_action_logged(journal_env):
    result = runner.invoke(
        retention_app, ["config", "--mode", "days", "--days", "7"]
    )

    assert result.exit_code == 0

    action_files = list((journal_env / "config" / "actions").glob("*.jsonl"))
    assert len(action_files) == 1
    entries = action_files[0].read_text(encoding="utf-8").strip().splitlines()
    payload = json.loads(entries[-1])
    assert payload["action"] == "retention_config"
