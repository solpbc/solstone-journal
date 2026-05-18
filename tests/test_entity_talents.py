# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for entity detection and review agent configurations."""

import pytest

from solstone.think.talent import get_talent


@pytest.fixture
def fixture_journal(monkeypatch):
    """Set SOLSTONE_JOURNAL to tests/fixtures/journal for testing."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    yield
    # No cleanup needed - just testing reads


def test_entities_agent_config(fixture_journal):
    """Test detection agent configuration loads correctly."""
    # Entity agents are in apps/entities/talent/ so use app-qualified name
    config = get_talent("entities:entities")

    # Verify required fields
    assert config["name"] == "entities:entities"
    assert "user_instruction" in config
    assert len(config["user_instruction"]) > 0

    # Verify JSON metadata fields from entities.json
    assert config.get("title") == "Entity Detector"
    assert config.get("schedule") == "daily"
    assert config.get("priority") == 55
    assert config.get("multi_facet") is True


def test_entities_review_agent_config(fixture_journal):
    """Test review agent configuration loads correctly."""
    # Entity agents are in apps/entities/talent/ so use app-qualified name
    config = get_talent("entities:entities_review")

    # Verify required fields
    assert config["name"] == "entities:entities_review"
    assert "user_instruction" in config
    assert len(config["user_instruction"]) > 0

    # Verify JSON metadata fields from entities_review.json
    assert config.get("title") == "Entity Reviewer"
    assert config.get("schedule") == "daily"
    assert config.get("priority") == 56
    assert config.get("multi_facet") is True


def test_entities_agent_instruction_content(fixture_journal):
    """Test detection agent instruction contains expected sections."""
    config = get_talent("entities:entities")
    prompt = config["user_instruction"]

    # Check for key sections in the agent prompt
    assert "Core Mission" in prompt
    assert "sol call entities detect" in prompt
    assert "sol call entities list" in prompt
    assert "Knowledge Graphs" in prompt or "knowledge_graph" in prompt
    assert "day-specific context" in prompt.lower()


def test_entities_review_agent_instruction_content(fixture_journal):
    """Test review agent instruction contains expected sections."""
    config = get_talent("entities:entities_review")
    prompt = config["user_instruction"]

    # Check for key sections in the agent prompt
    assert "Core Mission" in prompt
    assert "sol call entities attach" in prompt
    assert "sol call entities list" in prompt
    assert "3+" in prompt or "promotion" in prompt.lower()
    assert "description" in prompt.lower()


def test_agent_context_includes_entities_by_facet(fixture_journal):
    """Test that agent context includes entities grouped by facet."""
    config = get_talent("entities:entities")

    prompt = config["user_instruction"]
    assert "Available Facets" in prompt

    # Should include facet names in backtick format
    assert "`test-facet`" in prompt or "`full-featured`" in prompt

    # Should include entities from fixture facets
    # tests/fixtures/journal/facets/ contains various entities
    assert "Entities" in prompt

    # Check for some known entities from the fixtures
    assert "John Smith" in prompt or "Jane Doe" in prompt or "Acme Corp" in prompt


def test_agent_context_with_facet_focus(fixture_journal):
    """Test that get_talent with facet parameter uses focused single-facet context."""
    config = get_talent("chat", facet="full-featured")

    prompt = config["user_instruction"]

    # Should have Facet Focus section instead of Available Facets
    assert "## Facet Focus" in prompt
    assert "Available Facets" not in prompt

    # Should include the focused facet's details
    assert "Full Featured Facet" in prompt
    assert "A facet for testing all features" in prompt

    # Should include entity details from the focused facet (detailed format)
    assert "## Entities" in prompt
    assert "Entity 1" in prompt or "First test entity" in prompt


def test_agent_priority_ordering(fixture_journal):
    """Test that entity agents have correct priority ordering."""
    detection_config = get_talent("entities:entities")
    review_config = get_talent("entities:entities_review")

    detection_priority = detection_config["priority"]
    review_priority = review_config["priority"]

    # Review should run after detection
    assert review_priority > detection_priority
    assert detection_priority == 55
    assert review_priority == 56
