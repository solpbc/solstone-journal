# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for ``journal identity digest``."""

import json
import re
import time

import pytest
from typer.testing import CliRunner

from solstone.think.identity import ensure_identity_directory, write_identity
from solstone.think.talent import get_talent
from solstone.think.talents import validate_config
from solstone.think.tools.sol import app

runner = CliRunner()
_HISTORY_FIELDS = [
    "ts",
    "file",
    "actor",
    "op",
    "section",
    "reason",
    "before_hash",
    "after_hash",
    "bytes_before",
    "bytes_after",
]


def _read_history(journal_path):
    history = journal_path / "identity" / "history.jsonl"
    return [json.loads(line) for line in history.read_text().splitlines()]


def _assert_history_record(record, *, file_name, actor, op, section, reason):
    assert list(record) == _HISTORY_FIELDS
    assert re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z", record["ts"])
    assert record["file"] == file_name
    assert record["actor"] == actor
    assert record["op"] == op
    assert record["section"] == section
    assert record["reason"] == reason


@pytest.fixture
def digest_journal(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setenv("SOL_SKIP_SUPERVISOR_CHECK", "1")

    config_dir = tmp_path / "config"
    config_dir.mkdir()
    (config_dir / "journal.json").write_text("{}", encoding="utf-8")
    return tmp_path


def test_digest_write_mode_writes_via_identity(digest_journal):
    digest_path = digest_journal / "identity" / "digest.md"
    content = "digest body"

    result = runner.invoke(app, ["digest", "--write", "--value", content])

    assert result.exit_code == 0
    assert digest_path.read_text(encoding="utf-8") == content
    assert (
        f"wrote {digest_path} ({len(content.encode('utf-8'))} bytes)" in result.output
    )

    record = _read_history(digest_journal)[-1]
    _assert_history_record(
        record,
        file_name="digest.md",
        actor="journal identity digest --write",
        op="replace",
        section=None,
        reason="manual replace",
    )


def test_digest_write_mode_stdin(digest_journal):
    digest_path = digest_journal / "identity" / "digest.md"
    content = "digest from stdin"

    result = runner.invoke(app, ["digest", "--write"], input=content)

    assert result.exit_code == 0
    assert digest_path.read_text(encoding="utf-8") == content
    assert (
        f"wrote {digest_path} ({len(content.encode('utf-8'))} bytes)" in result.output
    )

    record = _read_history(digest_journal)[-1]
    _assert_history_record(
        record,
        file_name="digest.md",
        actor="journal identity digest --write",
        op="replace",
        section=None,
        reason="manual replace",
    )


def test_digest_write_mode_allows_empty_value(digest_journal):
    digest_path = digest_journal / "identity" / "digest.md"

    result = runner.invoke(app, ["digest", "--write", "--value", ""])

    assert result.exit_code == 0
    assert digest_path.read_text(encoding="utf-8") == ""
    assert f"wrote {digest_path} (0 bytes)" in result.output


def test_digest_default_mode_success(digest_journal, monkeypatch):
    digest_path = digest_journal / "identity" / "digest.md"
    request_kwargs = {}

    def fake_cortex_request(**kwargs):
        request_kwargs.update(kwargs)
        return "digest-use-1"

    def fake_wait_for_uses(use_ids, timeout):
        assert use_ids == ["digest-use-1"]
        assert timeout == 600
        time.sleep(0.01)
        write_identity(
            "digest.md",
            actor="test digest writer",
            op="replace",
            section=None,
            content="fresh digest body",
            reason="test completion",
        )
        return {"digest-use-1": "finish"}, []

    monkeypatch.setattr("solstone.think.tools.sol.cortex_request", fake_cortex_request)
    monkeypatch.setattr("solstone.think.tools.sol.wait_for_uses", fake_wait_for_uses)

    result = runner.invoke(app, ["digest"])

    assert result.exit_code == 0
    assert digest_path.read_text(encoding="utf-8") == "fresh digest body"
    assert request_kwargs == {"prompt": "", "name": "digest"}
    assert "regenerated " in result.output
    assert "digest.md" in result.output


def test_supervisor_triggered_digest_runs_with_body_only_validator(
    digest_journal, monkeypatch
):
    digest_path = ensure_identity_directory() / "digest.md"
    seed_digest = digest_path.read_text(encoding="utf-8")
    request_kwargs = {}

    config = get_talent("digest")
    assert validate_config({**config, "prompt": ""}) is None

    def fake_cortex_request(**kwargs):
        request_kwargs.update(kwargs)
        return "digest-use-1"

    def fake_wait_for_uses(use_ids, timeout):
        assert use_ids == ["digest-use-1"]
        assert timeout == 600
        write_identity(
            "digest.md",
            actor="test digest writer",
            op="replace",
            section=None,
            content="fresh startup digest",
            reason="test completion",
        )
        return {"digest-use-1": "finish"}, []

    monkeypatch.setattr("solstone.think.tools.sol.cortex_request", fake_cortex_request)
    monkeypatch.setattr("solstone.think.tools.sol.wait_for_uses", fake_wait_for_uses)

    result = runner.invoke(app, ["digest"])

    assert result.exit_code == 0
    assert request_kwargs == {"prompt": "", "name": "digest"}
    assert digest_path.read_text(encoding="utf-8") == "fresh startup digest"
    assert digest_path.read_text(encoding="utf-8") != seed_digest


def test_digest_default_mode_failure_timeout(digest_journal, monkeypatch):
    monkeypatch.setattr(
        "solstone.think.tools.sol.cortex_request",
        lambda **kwargs: "digest-use-1",
    )
    monkeypatch.setattr(
        "solstone.think.tools.sol.wait_for_uses",
        lambda use_ids, timeout: ({}, ["digest-use-1"]),
    )

    result = runner.invoke(app, ["digest"])

    assert result.exit_code == 1
    assert "Error: digest request timed out." in result.output
