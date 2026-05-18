# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for journal configuration utilities."""

import json

import pytest

from solstone.think.utils import get_config, journal_is_active


@pytest.fixture
def config_journal(tmp_path):
    """Create a temporary journal with config."""
    config_dir = tmp_path / "config"
    config_dir.mkdir()

    config_data = {
        "identity": {
            "name": "Test User",
            "preferred": "Tester",
            "bio": "a software engineer and tester",
            "pronouns": {
                "subject": "they",
                "object": "them",
                "possessive": "their",
                "reflexive": "themselves",
            },
            "aliases": ["test", "tester"],
            "email_addresses": ["test@example.com"],
            "timezone": "America/New_York",
        }
    }

    config_file = config_dir / "journal.json"
    with open(config_file, "w") as f:
        json.dump(config_data, f, indent=2)
        f.write("\n")

    return tmp_path


def test_get_config_default_structure(tmp_path, monkeypatch):
    """Test get_config returns default structure when file doesn't exist."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    config = get_config()

    assert "identity" in config
    assert config["identity"]["name"] == ""
    assert config["identity"]["preferred"] == ""
    assert config["identity"]["pronouns"] == {
        "subject": "",
        "object": "",
        "possessive": "",
        "reflexive": "",
    }
    assert config["identity"]["aliases"] == []
    assert config["identity"]["email_addresses"] == []
    assert config["identity"]["timezone"] == ""
    assert config["identity"]["bio"] == ""

    # Describe defaults
    assert "describe" in config
    assert isinstance(config["describe"]["redact"], list)
    assert len(config["describe"]["redact"]) > 0


def test_get_config_default_is_deep_copy(tmp_path, monkeypatch):
    """Test that modifying returned defaults doesn't affect future calls."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    config1 = get_config()
    config1["identity"]["name"] = "Modified"
    config1["describe"]["redact"].append("extra rule")

    config2 = get_config()
    assert config2["identity"]["name"] == ""
    assert "extra rule" not in config2["describe"]["redact"]


def test_get_config_loads_existing(config_journal, monkeypatch):
    """Test get_config loads existing configuration."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(config_journal))

    config = get_config()

    assert config["identity"]["name"] == "Test User"
    assert config["identity"]["preferred"] == "Tester"
    assert config["identity"]["pronouns"] == {
        "subject": "they",
        "object": "them",
        "possessive": "their",
        "reflexive": "themselves",
    }
    assert config["identity"]["aliases"] == ["test", "tester"]
    assert config["identity"]["email_addresses"] == ["test@example.com"]
    assert config["identity"]["timezone"] == "America/New_York"
    assert config["identity"]["bio"] == "a software engineer and tester"


def test_get_config_existing_is_master(tmp_path, monkeypatch):
    """Test that existing journal.json is returned as-is without merging defaults."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create config with only a name - no other identity fields, no describe
    config_dir = tmp_path / "config"
    config_dir.mkdir()

    partial_config = {
        "identity": {
            "name": "Partial User",
        }
    }

    config_file = config_dir / "journal.json"
    with open(config_file, "w") as f:
        json.dump(partial_config, f)

    config = get_config()

    # User's value is preserved
    assert config["identity"]["name"] == "Partial User"
    # Missing fields are NOT filled from defaults - journal.json is master
    assert "preferred" not in config["identity"]
    assert "describe" not in config


def test_get_config_empty_journal(tmp_path, monkeypatch):
    """Test get_config returns defaults with an empty journal directory."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    config = get_config()
    assert "identity" in config
    assert config["identity"]["name"] == ""


def test_get_config_handles_invalid_json(tmp_path, monkeypatch):
    """Test get_config returns defaults when JSON is invalid."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create config with invalid JSON
    config_dir = tmp_path / "config"
    config_dir.mkdir()

    config_file = config_dir / "journal.json"
    with open(config_file, "w") as f:
        f.write("{ invalid json }")

    # Should return default structure and log warning
    config = get_config()

    assert "identity" in config
    assert config["identity"]["name"] == ""
    assert config["identity"]["pronouns"] == {
        "subject": "",
        "object": "",
        "possessive": "",
        "reflexive": "",
    }
    assert config["identity"]["bio"] == ""
    assert "describe" in config


def test_get_config_with_fixtures(monkeypatch):
    """Test get_config with tests/fixtures/journal path."""
    # Set SOLSTONE_JOURNAL to fixtures
    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")

    config = get_config()

    # Fixtures has journal.json - returned as-is
    assert "identity" in config
    assert isinstance(config["identity"]["name"], str)
    assert isinstance(config["identity"]["preferred"], str)
    assert isinstance(config["identity"]["pronouns"], dict)
    assert "subject" in config["identity"]["pronouns"]
    assert "object" in config["identity"]["pronouns"]
    assert "possessive" in config["identity"]["pronouns"]
    assert "reflexive" in config["identity"]["pronouns"]
    assert isinstance(config["identity"]["aliases"], list)
    assert isinstance(config["identity"]["email_addresses"], list)
    assert isinstance(config["identity"]["timezone"], str)
    assert isinstance(config["identity"]["bio"], str)


def _write_journal_config(journal_path, config):
    config_dir = journal_path / "config"
    config_dir.mkdir(parents=True, exist_ok=True)
    (config_dir / "journal.json").write_text(json.dumps(config), encoding="utf-8")


@pytest.mark.parametrize(
    ("config", "expected"),
    [
        ({"setup": {"completed_at": 1}}, True),
        ({"setup": {"completed_at": 1.5}}, True),
        ({"setup": {"completed_at": 0}}, False),
        ({"setup": {"completed_at": None}}, False),
        ({}, False),
        ({"setup": {"completed_at": "foo"}}, False),
        ({"identity": {"name": "Active User"}}, False),
        ({"identity": {"name": ""}, "setup": {"completed_at": 1}}, True),
    ],
)
def test_journal_is_active_from_config(tmp_path, config, expected):
    _write_journal_config(tmp_path, config)
    assert journal_is_active(tmp_path) is expected


def test_journal_is_active_with_fixtures(monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    assert journal_is_active("tests/fixtures/journal") is True


def test_journal_is_active_false_for_empty_dir(tmp_path):
    assert journal_is_active(tmp_path) is False


def test_journal_is_active_false_without_config(tmp_path):
    (tmp_path / "config").mkdir()
    assert journal_is_active(tmp_path) is False


def test_journal_is_active_false_for_malformed_json(tmp_path):
    config_dir = tmp_path / "config"
    config_dir.mkdir()
    (config_dir / "journal.json").write_text("{bad json", encoding="utf-8")
    assert journal_is_active(tmp_path) is False


def test_journal_is_active_false_for_path_that_is_not_directory(tmp_path):
    file_path = tmp_path / "journal.json"
    file_path.write_text("{}", encoding="utf-8")
    assert journal_is_active(file_path) is False


def test_journal_is_active_false_for_absent_path(tmp_path):
    assert journal_is_active(tmp_path / "missing") is False
