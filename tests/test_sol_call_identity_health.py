# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for ``sol call identity health``."""

import json
import time
from pathlib import Path

import pytest
from typer.testing import CliRunner

from solstone.think.cortex_client import CortexSpawnUnavailable
from solstone.think.identity import ensure_identity_directory, write_identity
from solstone.think.steward import acquire_steward_lock
from solstone.think.tools.sol import app

runner = CliRunner()


@pytest.fixture
def health_journal(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setenv("SOL_SKIP_SUPERVISOR_CHECK", "1")
    (tmp_path / "config").mkdir()
    (tmp_path / "config" / "journal.json").write_text("{}", encoding="utf-8")
    return tmp_path


def _valid_body(*, generated_at: str = "2026-05-26T17:32:18Z") -> str:
    return "\n".join(
        [
            "## Status",
            f"<!-- generated_at: {generated_at} -->",
            "Sol is well.",
            "",
            "## Needs your attention",
            "",
            "## Auto-repairs (last 7d)",
            "",
            "## Trends (last 7d)",
            "",
        ]
    )


def _write_health(journal: Path, body: str | None = None) -> Path:
    path = journal / "identity" / "health.md"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body or _valid_body(), encoding="utf-8")
    return path


def test_health_default_reads_file(health_journal):
    _write_health(health_journal, _valid_body())

    result = runner.invoke(app, ["health"])

    assert result.exit_code == 0
    assert "Sol is well." in result.output


def test_health_refresh_success(health_journal, monkeypatch):
    health_path = ensure_identity_directory() / "health.md"
    request_kwargs = {}

    monkeypatch.setattr(
        "solstone.think.tools.sol.latest_daily_run_complete_ts",
        lambda today: None,
    )

    def fake_cortex_request(**kwargs):
        request_kwargs.update(kwargs)
        return "steward-use-1"

    def fake_wait_for_uses(use_ids, timeout):
        assert use_ids == ["steward-use-1"]
        assert timeout == 600
        time.sleep(0.01)
        write_identity(
            "health.md",
            actor="steward",
            op="replace",
            section=None,
            content=_valid_body(generated_at="2026-05-26T17:33:18Z"),
            reason="test completion",
        )
        return {"steward-use-1": "finish"}, []

    monkeypatch.setattr("solstone.think.tools.sol.cortex_request", fake_cortex_request)
    monkeypatch.setattr("solstone.think.tools.sol.wait_for_uses", fake_wait_for_uses)

    result = runner.invoke(app, ["health", "--refresh"])

    assert result.exit_code == 0
    assert request_kwargs["name"] == "steward"
    assert request_kwargs["prompt"] == ""
    assert request_kwargs["config"]["output"] == "md"
    assert request_kwargs["config"]["refresh"] is True
    assert f"regenerated {health_path}" in result.output
    assert "generated_at: 2026-05-26T17:33:18Z" in result.output


def test_health_refresh_already_fresh(health_journal, monkeypatch):
    _write_health(health_journal, _valid_body(generated_at="2026-05-26T17:33:18Z"))
    monkeypatch.setattr(
        "solstone.think.tools.sol.latest_daily_run_complete_ts",
        lambda today: 1,
    )

    result = runner.invoke(app, ["health", "--refresh"])

    assert result.exit_code == 0
    assert "already fresh (generated_at: 2026-05-26T17:33:18Z)" in result.output


def test_health_refresh_cortex_unavailable(health_journal, monkeypatch):
    ensure_identity_directory()
    monkeypatch.setattr(
        "solstone.think.tools.sol.latest_daily_run_complete_ts",
        lambda today: None,
    )

    def unavailable(**kwargs):
        raise CortexSpawnUnavailable()

    monkeypatch.setattr("solstone.think.tools.sol.cortex_request", unavailable)

    result = runner.invoke(app, ["health", "--refresh"])

    assert result.exit_code == 1
    assert "Error: failed to send steward request to cortex." in result.output


def test_health_refresh_timeout(health_journal, monkeypatch):
    ensure_identity_directory()
    monkeypatch.setattr(
        "solstone.think.tools.sol.latest_daily_run_complete_ts",
        lambda today: None,
    )
    monkeypatch.setattr(
        "solstone.think.tools.sol.cortex_request",
        lambda **kwargs: "steward-use-1",
    )
    monkeypatch.setattr(
        "solstone.think.tools.sol.wait_for_uses",
        lambda use_ids, timeout: ({}, ["steward-use-1"]),
    )

    result = runner.invoke(app, ["health", "--refresh"])

    assert result.exit_code == 1
    assert "Error: steward request timed out." in result.output


def test_health_refresh_end_state_not_finish(health_journal, monkeypatch):
    ensure_identity_directory()
    monkeypatch.setattr(
        "solstone.think.tools.sol.latest_daily_run_complete_ts",
        lambda today: None,
    )
    monkeypatch.setattr(
        "solstone.think.tools.sol.cortex_request",
        lambda **kwargs: "steward-use-1",
    )
    monkeypatch.setattr(
        "solstone.think.tools.sol.wait_for_uses",
        lambda use_ids, timeout: ({"steward-use-1": "error"}, []),
    )

    result = runner.invoke(app, ["health", "--refresh"])

    assert result.exit_code == 1
    assert "Error: steward request failed: error." in result.output


def test_health_refresh_file_not_updated(health_journal, monkeypatch):
    ensure_identity_directory()
    monkeypatch.setattr(
        "solstone.think.tools.sol.latest_daily_run_complete_ts",
        lambda today: None,
    )
    monkeypatch.setattr(
        "solstone.think.tools.sol.cortex_request",
        lambda **kwargs: "steward-use-1",
    )
    monkeypatch.setattr(
        "solstone.think.tools.sol.wait_for_uses",
        lambda use_ids, timeout: ({"steward-use-1": "finish"}, []),
    )

    result = runner.invoke(app, ["health", "--refresh"])

    assert result.exit_code == 1
    assert "Error: identity/health.md was not updated." in result.output


def test_health_refresh_lock_contended(health_journal):
    ensure_identity_directory()
    fd = acquire_steward_lock()
    assert fd is not None
    try:
        result = runner.invoke(app, ["health", "--refresh"])
    finally:
        from solstone.think.steward import release_steward_lock

        release_steward_lock(fd)

    assert result.exit_code == 1
    assert "Error: steward already in flight." in result.output


def test_health_md_bootstrap_creates_via_ensure_identity_directory(health_journal):
    identity_dir = ensure_identity_directory()

    health = identity_dir / "health.md"
    assert health.exists()
    assert "not yet generated" in health.read_text(encoding="utf-8")
    history = [
        json.loads(line)
        for line in (identity_dir / "history.jsonl").read_text().splitlines()
    ]
    health_rows = [row for row in history if row["file"] == "health.md"]
    assert health_rows[-1]["actor"] == "ensure_identity_directory"
    assert health_rows[-1]["op"] == "create"
