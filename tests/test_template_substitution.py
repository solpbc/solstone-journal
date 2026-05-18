# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for template variable substitution in load_prompt."""

import json

import pytest

from solstone.think.prompts import _flatten_identity_to_template_vars, load_prompt


@pytest.fixture
def mock_journal_with_config(tmp_path, monkeypatch):
    """Create a temporary journal with config."""
    # Create config directory and journal.json
    config_dir = tmp_path / "config"
    config_dir.mkdir()

    config = {
        "identity": {
            "name": "Test User",
            "preferred": "Testy",
            "bio": "a curious software engineer interested in AI",
            "pronouns": {
                "subject": "they",
                "object": "them",
                "possessive": "their",
                "reflexive": "themselves",
            },
            "aliases": ["test", "tester"],
            "email_addresses": ["test@example.com"],
            "timezone": "America/Los_Angeles",
        },
        "agent": {
            "name": "sol",
            "name_status": "default",
            "named_date": None,
            "proposal_count": 0,
        },
    }

    with open(config_dir / "journal.json", "w") as f:
        json.dump(config, f)

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    yield tmp_path


@pytest.fixture
def mock_prompt_dir(tmp_path):
    """Create a temporary directory with test prompt files."""
    prompts_dir = tmp_path / "prompts"
    prompts_dir.mkdir()

    # Create a prompt with template variables
    template_prompt = """Hello $name, also known as $preferred!

You use $pronouns_subject/$pronouns_object/$pronouns_possessive/$pronouns_reflexive pronouns.

Capitalized: $Pronouns_subject will do it $Pronouns_reflexive.

Bio: $bio

Timezone: $timezone
"""
    (prompts_dir / "test_template.md").write_text(template_prompt)

    # Create a prompt without template variables
    plain_prompt = "This is a plain prompt without any variables."
    (prompts_dir / "plain.md").write_text(plain_prompt)

    return prompts_dir


def test_flatten_identity_basic_fields():
    """Test flattening of basic identity fields."""
    identity = {"name": "Alice Smith", "preferred": "Alice", "timezone": "UTC"}

    result = _flatten_identity_to_template_vars(identity)

    assert result["name"] == "Alice Smith"
    assert result["Name"] == "Alice smith"  # Capitalized
    assert result["preferred"] == "Alice"
    assert result["Preferred"] == "Alice"
    assert result["timezone"] == "UTC"


def test_flatten_identity_nested_pronouns():
    """Test flattening of nested pronoun fields."""
    identity = {
        "pronouns": {
            "subject": "she",
            "object": "her",
            "possessive": "her",
            "reflexive": "herself",
        }
    }

    result = _flatten_identity_to_template_vars(identity)

    assert result["pronouns_subject"] == "she"
    assert result["Pronouns_subject"] == "She"
    assert result["pronouns_object"] == "her"
    assert result["pronouns_possessive"] == "her"
    assert result["pronouns_reflexive"] == "herself"
    assert result["Pronouns_reflexive"] == "Herself"


def test_flatten_identity_with_bio(mock_journal_with_config):
    """Test bio field extraction."""
    from solstone.think.utils import get_config

    config = get_config()
    identity = config["identity"]

    result = _flatten_identity_to_template_vars(identity)

    assert result["bio"] == "a curious software engineer interested in AI"
    assert result["Bio"] == "A curious software engineer interested in ai"


def test_load_prompt_with_substitution(mock_journal_with_config, mock_prompt_dir):
    """Test that load_prompt performs template substitution."""
    result = load_prompt("test_template", base_dir=mock_prompt_dir)

    # Check that variables were substituted
    assert "Test User" in result.text
    assert "Testy" in result.text
    assert "they/them/their/themselves" in result.text
    assert "They will do it Themselves" in result.text
    assert "a curious software engineer interested in AI" in result.text
    assert "America/Los_Angeles" in result.text

    # Ensure no template variables remain
    assert "$name" not in result.text
    assert "$pronouns_subject" not in result.text
    assert "$bio" not in result.text


def test_load_prompt_without_substitution(mock_journal_with_config, mock_prompt_dir):
    """Test that prompts without variables work normally."""
    result = load_prompt("plain", base_dir=mock_prompt_dir)

    assert result.text == "This is a plain prompt without any variables."


def test_load_prompt_missing_config_graceful(tmp_path, mock_prompt_dir, monkeypatch):
    """Test that load_prompt works even without config (safe_substitute)."""
    # Point to a journal without config
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    result = load_prompt("test_template", base_dir=mock_prompt_dir)

    # When config exists but has empty values, safe_substitute replaces them with empty strings
    # Variables that are not set will remain empty in the output (replaced with "")
    # The prompt should still load without errors
    assert result.path.exists()


def test_load_prompt_with_custom_context(mock_journal_with_config, mock_prompt_dir):
    """Test that custom context variables are substituted."""
    # Create a prompt with context variables
    context_prompt = """Day: $day
Segment: $segment
Custom value: $custom_value"""
    (mock_prompt_dir / "context_test.md").write_text(context_prompt)

    result = load_prompt(
        "context_test",
        base_dir=mock_prompt_dir,
        context={"day": "20250110", "segment": "143022_300", "custom_value": "hello"},
    )

    assert "Day: 20250110" in result.text
    assert "Segment: 143022_300" in result.text
    assert "Custom value: hello" in result.text


def test_load_prompt_context_uppercase_versions(
    mock_journal_with_config, mock_prompt_dir
):
    """Test that uppercase-first versions are created for context variables."""
    context_prompt = """lowercase: $agent
Uppercase: $Agent"""
    (mock_prompt_dir / "uppercase_test.md").write_text(context_prompt)

    result = load_prompt(
        "uppercase_test",
        base_dir=mock_prompt_dir,
        context={"agent": "meetings"},
    )

    assert "lowercase: meetings" in result.text
    assert "Uppercase: Meetings" in result.text


def test_load_prompt_context_overrides_identity(
    mock_journal_with_config, mock_prompt_dir
):
    """Test that context variables override identity variables."""
    override_prompt = "Name: $name"
    (mock_prompt_dir / "override_test.md").write_text(override_prompt)

    # Without context, should use identity name
    result_default = load_prompt("override_test", base_dir=mock_prompt_dir)
    assert "Name: Test User" in result_default.text

    # With context, should override
    result_override = load_prompt(
        "override_test",
        base_dir=mock_prompt_dir,
        context={"name": "Custom Name"},
    )
    assert "Name: Custom Name" in result_override.text


def test_load_prompt_context_stringifies_values(
    mock_journal_with_config, mock_prompt_dir
):
    """Test that non-string context values are converted to strings."""
    stringify_prompt = "Number: $count, Bool: $flag"
    (mock_prompt_dir / "stringify_test.md").write_text(stringify_prompt)

    result = load_prompt(
        "stringify_test",
        base_dir=mock_prompt_dir,
        context={"count": 42, "flag": True},
    )

    assert "Number: 42" in result.text
    assert "Bool: True" in result.text


def test_load_prompt_empty_context(mock_journal_with_config, mock_prompt_dir):
    """Test that empty context dict behaves same as None.

    Note: mock_journal_with_config needed for get_config() call in load_prompt.
    """
    result_none = load_prompt("plain", base_dir=mock_prompt_dir, context=None)
    result_empty = load_prompt("plain", base_dir=mock_prompt_dir, context={})

    assert result_none.text == result_empty.text


def test_load_prompt_identity_vars_follow_journal_override(monkeypatch, tmp_path):
    """Journal identity/ content should not leak across journal overrides."""

    def write_journal(journal_dir, awareness_text):
        config_dir = journal_dir / "config"
        config_dir.mkdir(parents=True)
        (config_dir / "journal.json").write_text(
            json.dumps(
                {
                    "identity": {"name": "Test User"},
                    "agent": {"name": "sol", "name_status": "default"},
                }
            )
        )
        identity_dir = journal_dir / "identity"
        identity_dir.mkdir()
        (identity_dir / "awareness.md").write_text(awareness_text)

    prompt_dir = tmp_path / "prompts"
    prompt_dir.mkdir()
    (prompt_dir / "identity_vars.md").write_text("Awareness:\n$identity_awareness\n")

    journal_one = tmp_path / "journal-one"
    journal_two = tmp_path / "journal-two"
    write_journal(journal_one, "first awareness")
    write_journal(journal_two, "second awareness")

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal_one))
    first = load_prompt("identity_vars", base_dir=prompt_dir)
    assert "first awareness" in first.text

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal_two))
    second = load_prompt("identity_vars", base_dir=prompt_dir)
    assert "second awareness" in second.text
    assert "first awareness" not in second.text
