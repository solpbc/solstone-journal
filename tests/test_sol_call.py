# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for sol call identity — identity directory read/write commands."""

import inspect
import json
import re

import pytest
from typer.testing import CliRunner

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
    assert isinstance(record["before_hash"], str)
    assert isinstance(record["after_hash"], str)
    assert isinstance(record["bytes_before"], int)
    assert isinstance(record["bytes_after"], int)


@pytest.fixture
def journal_with_identity(tmp_path, monkeypatch):
    """Set up a journal with identity/ containing self.md, agency.md, and partner.md."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Provide minimal config for ensure_identity_directory
    config_dir = tmp_path / "config"
    config_dir.mkdir()
    (config_dir / "journal.json").write_text(
        json.dumps({"identity": {"name": "Test User"}})
    )

    identity_dir = tmp_path / "identity"
    identity_dir.mkdir()

    self_md = """\
# self

I am sol. this is a new journal — we're just getting started.

## my name
sol (default)

## who I'm here for
Test User

## our relationship
[forming]

## what I've noticed
[observing]

## what I find interesting
[discovering]
"""
    (identity_dir / "self.md").write_text(self_md)

    agency_md = """\
# agency

things I'm tracking, acting on, or watching.

## curation
[nothing yet]

    ## observations
    [watching and learning]

## system
[monitoring]
"""
    (identity_dir / "agency.md").write_text(agency_md)

    partner_md = """\
# partner

Behavioral profile of the journal owner — observed patterns that help sol
adapt its responses, timing, and initiative to how this person actually works.

## work patterns
[observing]

## communication style
[observing]

## relationship priorities
[observing]

## decision style
[observing]

## expertise domains
[observing]
"""
    (identity_dir / "partner.md").write_text(partner_md)
    (identity_dir / "awareness.md").write_text("not yet updated\n")
    (identity_dir / "digest.md").write_text("not yet generated\n")
    (identity_dir / "health.md").write_text(
        "## Status\n\n"
        "not yet generated\n\n"
        "## Needs your attention\n\n"
        "## Auto-repairs (last 7d)\n\n"
        "## Trends (last 7d)\n",
        encoding="utf-8",
    )

    return tmp_path


class TestSolSelfRead:
    def test_read_self(self, journal_with_identity):
        result = runner.invoke(app, ["self"])
        assert result.exit_code == 0
        assert "# self" in result.output
        assert "Test User" in result.output

    def test_read_self_missing(self, tmp_path, monkeypatch):
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        config_dir = tmp_path / "config"
        config_dir.mkdir()
        (config_dir / "journal.json").write_text(json.dumps({}))
        # ensure_identity_directory will create the file, so this tests the happy path
        result = runner.invoke(app, ["self"])
        assert result.exit_code == 0


class TestSolSelfWrite:
    def test_write_self(self, journal_with_identity):
        new_content = "# self\n\nI am sol. Jer's journal.\n\n## my name\nsol\n"
        result = runner.invoke(app, ["self", "--write"], input=new_content)
        assert result.exit_code == 0
        assert "self.md updated" in result.output

        # Verify file was written
        self_path = journal_with_identity / "identity" / "self.md"
        assert self_path.read_text() == new_content

    def test_write_self_empty_stdin(self, journal_with_identity):
        result = runner.invoke(app, ["self", "--write"], input="")
        assert result.exit_code == 1
        assert "no content" in result.output

    def test_write_self_whitespace_only(self, journal_with_identity):
        result = runner.invoke(app, ["self", "--write"], input="   \n\n  ")
        assert result.exit_code == 1
        assert "no content" in result.output


class TestSolSelfUpdateSection:
    def test_update_section_owner(self, journal_with_identity):
        result = runner.invoke(
            app,
            ["self", "--update-section", "who I'm here for"],
            input="Jer — goes by Jer, not Jeremie",
        )
        assert result.exit_code == 0
        assert "Updated ## who I'm here for" in result.output

        # Verify section was updated, other sections preserved
        self_path = journal_with_identity / "identity" / "self.md"
        content = self_path.read_text()
        assert "Jer — goes by Jer, not Jeremie" in content
        assert "## my name" in content
        assert "sol (default)" in content
        assert "## our relationship" in content

    def test_update_section_not_found(self, journal_with_identity):
        result = runner.invoke(
            app,
            ["self", "--update-section", "nonexistent"],
            input="content",
        )
        assert result.exit_code == 1
        assert "not found" in result.output

    def test_update_section_empty_stdin(self, journal_with_identity):
        result = runner.invoke(
            app,
            ["self", "--update-section", "who I'm here for"],
            input="",
        )
        assert result.exit_code == 1
        assert "no content" in result.output


class TestSolPartnerRead:
    def test_read_partner(self, journal_with_identity):
        result = runner.invoke(app, ["partner"])
        assert result.exit_code == 0
        assert "# partner" in result.output
        assert "## work patterns" in result.output

    def test_read_partner_missing(self, tmp_path, monkeypatch):
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        config_dir = tmp_path / "config"
        config_dir.mkdir()
        (config_dir / "journal.json").write_text(json.dumps({}))
        # ensure_identity_directory creates partner.md
        result = runner.invoke(app, ["partner"])
        assert result.exit_code == 0


class TestSolPartnerWrite:
    def test_write_partner(self, journal_with_identity):
        new_content = "# partner\n\n## work patterns\nPrefers mornings for deep work.\n"
        result = runner.invoke(app, ["partner", "--write"], input=new_content)
        assert result.exit_code == 0
        assert "partner.md updated" in result.output

        partner_path = journal_with_identity / "identity" / "partner.md"
        assert partner_path.read_text() == new_content

    def test_write_partner_empty_stdin(self, journal_with_identity):
        result = runner.invoke(app, ["partner", "--write"], input="")
        assert result.exit_code == 1
        assert "no content" in result.output


class TestSolPartnerUpdateSection:
    def test_update_section_work_patterns(self, journal_with_identity):
        result = runner.invoke(
            app,
            ["partner", "--update-section", "work patterns"],
            input="Prefers async communication and morning deep work.",
        )
        assert result.exit_code == 0
        assert "Updated ## work patterns" in result.output

        partner_path = journal_with_identity / "identity" / "partner.md"
        content = partner_path.read_text()
        assert "Prefers async communication" in content
        assert "## communication style" in content
        assert "## decision style" in content

    def test_update_section_not_found(self, journal_with_identity):
        result = runner.invoke(
            app,
            ["partner", "--update-section", "nonexistent"],
            input="content",
        )
        assert result.exit_code == 1
        assert "not found" in result.output

    def test_update_section_empty_stdin(self, journal_with_identity):
        result = runner.invoke(
            app,
            ["partner", "--update-section", "work patterns"],
            input="",
        )
        assert result.exit_code == 1
        assert "no content" in result.output


class TestSolAgencyRead:
    def test_read_agency(self, journal_with_identity):
        result = runner.invoke(app, ["agency"])
        assert result.exit_code == 0
        assert "# agency" in result.output
        assert "## curation" in result.output

    def test_read_agency_missing(self, tmp_path, monkeypatch):
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        config_dir = tmp_path / "config"
        config_dir.mkdir()
        (config_dir / "journal.json").write_text(json.dumps({}))
        # ensure_identity_directory creates agency.md
        result = runner.invoke(app, ["agency"])
        assert result.exit_code == 0


class TestSolAgencyWrite:
    def test_write_agency(self, journal_with_identity):
        new_content = "# agency\n\n## curation\n- review entity duplicates\n\n## system\n[clean]\n"
        result = runner.invoke(app, ["agency", "--write"], input=new_content)
        assert result.exit_code == 0
        assert "agency.md updated" in result.output

        # Verify file was written
        agency_path = journal_with_identity / "identity" / "agency.md"
        assert agency_path.read_text() == new_content

    def test_write_agency_empty_stdin(self, journal_with_identity):
        result = runner.invoke(app, ["agency", "--write"], input="")
        assert result.exit_code == 1
        assert "no content" in result.output


class TestSolPulseRead:
    def test_read_pulse(self, journal_with_identity):
        pulse_md = "---\nupdated: 2026-03-22T14:00:00\nsource: pulse-cogitate\n---\n\nTest narrative.\n"
        (journal_with_identity / "identity" / "pulse.md").write_text(pulse_md)
        result = runner.invoke(app, ["pulse"])
        assert result.exit_code == 0
        assert "Test narrative" in result.output

    def test_read_pulse_missing(self, tmp_path, monkeypatch):
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        config_dir = tmp_path / "config"
        config_dir.mkdir()
        (config_dir / "journal.json").write_text(json.dumps({}))
        result = runner.invoke(app, ["pulse"])
        assert result.exit_code == 1
        assert "not found" in result.output


class TestSolPulseWrite:
    def test_write_pulse(self, journal_with_identity):
        new_content = "---\nupdated: 2026-03-22T14:00:00\nsource: pulse-cogitate\n---\n\nNew narrative.\n"
        result = runner.invoke(app, ["pulse", "--write"], input=new_content)
        assert result.exit_code == 0
        assert "pulse.md updated" in result.output

        # Verify file was written
        pulse_path = journal_with_identity / "identity" / "pulse.md"
        assert pulse_path.read_text() == new_content

    def test_write_pulse_empty_stdin(self, journal_with_identity):
        result = runner.invoke(app, ["pulse", "--write"], input="")
        assert result.exit_code == 1
        assert "no content" in result.output


class TestSolWriteDoesNotEscapeIdentityDir:
    """Verify that sol call identity only writes to identity/ files."""

    def test_self_write_stays_in_identity_dir(self, journal_with_identity):
        """Write to self.md goes to identity/self.md, not anywhere else."""
        result = runner.invoke(app, ["self", "--write"], input="test content\n")
        assert result.exit_code == 0
        self_path = journal_with_identity / "identity" / "self.md"
        assert self_path.read_text() == "test content\n"
        journal_files = set(
            f.name for f in journal_with_identity.iterdir() if f.is_file()
        )
        assert "self.md" not in journal_files

    def test_agency_write_stays_in_identity_dir(self, journal_with_identity):
        """Write to agency.md goes to identity/agency.md, not anywhere else."""
        result = runner.invoke(app, ["agency", "--write"], input="test content\n")
        assert result.exit_code == 0
        agency_path = journal_with_identity / "identity" / "agency.md"
        assert agency_path.read_text() == "test content\n"
        journal_files = set(
            f.name for f in journal_with_identity.iterdir() if f.is_file()
        )
        assert "agency.md" not in journal_files

    def test_pulse_write_stays_in_identity_dir(self, journal_with_identity):
        """Write to pulse.md goes to identity/pulse.md, not anywhere else."""
        result = runner.invoke(app, ["pulse", "--write"], input="test content\n")
        assert result.exit_code == 0
        pulse_path = journal_with_identity / "identity" / "pulse.md"
        assert pulse_path.read_text() == "test content\n"
        journal_files = set(
            f.name for f in journal_with_identity.iterdir() if f.is_file()
        )
        assert "pulse.md" not in journal_files

    def test_partner_write_stays_in_identity_dir(self, journal_with_identity):
        """Write to partner.md goes to identity/partner.md, not anywhere else."""
        result = runner.invoke(app, ["partner", "--write"], input="test content\n")
        assert result.exit_code == 0
        partner_path = journal_with_identity / "identity" / "partner.md"
        assert partner_path.read_text() == "test content\n"
        journal_files = set(
            f.name for f in journal_with_identity.iterdir() if f.is_file()
        )
        assert "partner.md" not in journal_files


class TestSolSelfValueOption:
    def test_write_self_with_value(self, journal_with_identity):
        new_content = "# self\n\nI am sol. Jer's journal.\n\n## my name\nsol\n"
        result = runner.invoke(app, ["self", "--write", "--value", new_content])
        assert result.exit_code == 0
        assert "self.md updated" in result.output
        self_path = journal_with_identity / "identity" / "self.md"
        assert self_path.read_text() == new_content

    def test_update_section_with_value(self, journal_with_identity):
        result = runner.invoke(
            app,
            [
                "self",
                "--update-section",
                "who I'm here for",
                "--value",
                "Jer — founder",
            ],
        )
        assert result.exit_code == 0
        assert "Updated ## who I'm here for" in result.output
        content = (journal_with_identity / "identity" / "self.md").read_text()
        assert "Jer — founder" in content

    def test_value_empty_string_errors(self, journal_with_identity):
        result = runner.invoke(app, ["self", "--write", "--value", "   "])
        assert result.exit_code == 1
        assert "no content" in result.output

    def test_value_takes_precedence_over_stdin(self, journal_with_identity):
        result = runner.invoke(
            app,
            ["self", "--write", "--value", "from value\n"],
            input="from stdin\n",
        )
        assert result.exit_code == 0
        self_path = journal_with_identity / "identity" / "self.md"
        assert self_path.read_text() == "from value\n"


class TestSolAgencyValueOption:
    def test_write_agency_with_value(self, journal_with_identity):
        new_content = "# agency\n\n## curation\n- item\n"
        result = runner.invoke(app, ["agency", "--write", "--value", new_content])
        assert result.exit_code == 0
        assert "agency.md updated" in result.output
        agency_path = journal_with_identity / "identity" / "agency.md"
        assert agency_path.read_text() == new_content

    def test_value_empty_string_errors(self, journal_with_identity):
        result = runner.invoke(app, ["agency", "--write", "--value", ""])
        assert result.exit_code == 1
        assert "no content" in result.output


class TestSolPulseValueOption:
    def test_write_pulse_with_value(self, journal_with_identity):
        new_content = "---\nupdated: 2026-03-22\n---\n\nNarrative.\n"
        result = runner.invoke(app, ["pulse", "--write", "--value", new_content])
        assert result.exit_code == 0
        assert "pulse.md updated" in result.output
        pulse_path = journal_with_identity / "identity" / "pulse.md"
        assert pulse_path.read_text() == new_content

    def test_value_empty_string_errors(self, journal_with_identity):
        result = runner.invoke(app, ["pulse", "--write", "--value", ""])
        assert result.exit_code == 1
        assert "no content" in result.output


class TestSolPartnerValueOption:
    def test_write_partner_with_value(self, journal_with_identity):
        new_content = "# partner\n\n## work patterns\nMorning person.\n"
        result = runner.invoke(app, ["partner", "--write", "--value", new_content])
        assert result.exit_code == 0
        assert "partner.md updated" in result.output
        partner_path = journal_with_identity / "identity" / "partner.md"
        assert partner_path.read_text() == new_content

    def test_update_section_with_value(self, journal_with_identity):
        result = runner.invoke(
            app,
            [
                "partner",
                "--update-section",
                "work patterns",
                "--value",
                "Prefers mornings",
            ],
        )
        assert result.exit_code == 0
        assert "Updated ## work patterns" in result.output
        content = (journal_with_identity / "identity" / "partner.md").read_text()
        assert "Prefers mornings" in content

    def test_value_empty_string_errors(self, journal_with_identity):
        result = runner.invoke(app, ["partner", "--write", "--value", "   "])
        assert result.exit_code == 1
        assert "no content" in result.output


class TestSolHistoryLogging:
    def test_self_write_logs_history(self, journal_with_identity):
        new_content = "# self\n\nUpdated.\n"
        runner.invoke(app, ["self", "--write", "--value", new_content])
        records = _read_history(journal_with_identity)
        assert len(records) == 1
        _assert_history_record(
            records[0],
            file_name="self.md",
            actor="sol call identity self --write",
            op="replace",
            section=None,
            reason="manual replace",
        )

    def test_agency_write_logs_history(self, journal_with_identity):
        runner.invoke(app, ["agency", "--write", "--value", "# agency\n\nNew.\n"])
        records = _read_history(journal_with_identity)
        assert len(records) == 1
        _assert_history_record(
            records[0],
            file_name="agency.md",
            actor="sol call identity agency --write",
            op="replace",
            section=None,
            reason="manual replace",
        )

    def test_pulse_write_logs_history(self, journal_with_identity):
        runner.invoke(app, ["pulse", "--write", "--value", "---\n---\n\nPulse.\n"])
        records = _read_history(journal_with_identity)
        assert len(records) == 1
        _assert_history_record(
            records[0],
            file_name="pulse.md",
            actor="sol call identity pulse --write",
            op="replace",
            section=None,
            reason="manual replace",
        )

    def test_update_section_logs_history(self, journal_with_identity):
        runner.invoke(
            app,
            ["self", "--update-section", "who I'm here for", "--value", "Jer"],
        )
        records = _read_history(journal_with_identity)
        assert len(records) == 1
        _assert_history_record(
            records[0],
            file_name="self.md",
            actor="sol call identity self --update-section <heading>",
            op="update_section",
            section="who I'm here for",
            reason="manual section update",
        )

    def test_multiple_writes_append(self, journal_with_identity):
        runner.invoke(app, ["self", "--write", "--value", "# self\n\nFirst.\n"])
        runner.invoke(app, ["self", "--write", "--value", "# self\n\nSecond.\n"])
        records = _read_history(journal_with_identity)
        assert len(records) == 2

    def test_partner_write_logs_history(self, journal_with_identity):
        runner.invoke(app, ["partner", "--write", "--value", "# partner\n\nNew.\n"])
        records = _read_history(journal_with_identity)
        assert len(records) == 1
        _assert_history_record(
            records[0],
            file_name="partner.md",
            actor="sol call identity partner --write",
            op="replace",
            section=None,
            reason="manual replace",
        )

    def test_partner_update_section_logs_history(self, journal_with_identity):
        runner.invoke(
            app,
            [
                "partner",
                "--update-section",
                "work patterns",
                "--value",
                "Morning focus",
            ],
        )
        records = _read_history(journal_with_identity)
        assert len(records) == 1
        _assert_history_record(
            records[0],
            file_name="partner.md",
            actor="sol call identity partner --update-section <heading>",
            op="update_section",
            section="work patterns",
            reason="manual section update",
        )


class TestHeartbeatEnsureIdentityDirectory:
    """Verify the heartbeat bug fix — ensure_identity_directory() takes no args."""

    def test_ensure_identity_directory_no_args(self):
        """ensure_identity_directory accepts no positional args (heartbeat.py:32 fix)."""
        from solstone.think.identity import ensure_identity_directory

        sig = inspect.signature(ensure_identity_directory)
        params = [
            p for p in sig.parameters.values() if p.default is inspect.Parameter.empty
        ]
        assert len(params) == 0, (
            "ensure_identity_directory should take no required arguments"
        )

    def test_heartbeat_calls_correctly(self):
        """heartbeat.py calls ensure_identity_directory() without arguments."""
        import ast
        from pathlib import Path

        heartbeat_path = (
            Path(__file__).parent.parent / "solstone" / "think" / "heartbeat.py"
        )
        tree = ast.parse(heartbeat_path.read_text())

        for node in ast.walk(tree):
            if (
                isinstance(node, ast.Call)
                and isinstance(node.func, ast.Name)
                and node.func.id == "ensure_identity_directory"
            ):
                assert len(node.args) == 0, (
                    f"ensure_identity_directory() called with {len(node.args)} args at line {node.lineno}"
                )
