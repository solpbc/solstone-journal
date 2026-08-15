# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for think.facets module."""

import json
from datetime import datetime, timedelta
from pathlib import Path

import pytest
from slugify import slugify

from solstone.think.facets import (
    FACET_CANDIDATE_WINDOW_DAYS,
    _format_principal_role,
    _get_principal_display_name,
    _rank_entities_by_signal,
    aggregate_speculative_facets,
    create_facet,
    ensure_facet,
    facet_summaries,
    facet_summary,
    find_orphan_facets,
    get_active_facets,
    get_facets,
    update_facet,
)

# Use the permanent fixtures in tests/fixtures/journal/facets/
FIXTURES_PATH = Path(__file__).parent / "fixtures" / "journal"


def setup_entities_new_structure(
    journal_path: Path,
    facet: str,
    entities: list[dict],
):
    """Helper to set up entities using the new structure for tests.

    Creates both journal-level entity files and facet relationship files.

    Args:
        journal_path: Path to journal root
        facet: Facet name (e.g., "work")
        entities: List of entity dicts with type, name, description, etc.
    """
    for entity in entities:
        etype = entity.get("type", "")
        name = entity.get("name", "")
        desc = entity.get("description", "")
        is_principal = entity.get("is_principal", False)

        entity_id = slugify(name, separator="_")
        if not entity_id:
            continue

        # Create journal-level entity
        journal_entity_dir = journal_path / "entities" / entity_id
        journal_entity_dir.mkdir(parents=True, exist_ok=True)
        journal_entity = {"id": entity_id, "name": name, "type": etype}
        if is_principal:
            journal_entity["is_principal"] = True
        with open(journal_entity_dir / "entity.json", "w", encoding="utf-8") as f:
            json.dump(journal_entity, f)

        # Create facet relationship
        facet_entity_dir = journal_path / "facets" / facet / "entities" / entity_id
        facet_entity_dir.mkdir(parents=True, exist_ok=True)
        relationship = {"entity_id": entity_id, "description": desc}
        with open(facet_entity_dir / "entity.json", "w", encoding="utf-8") as f:
            json.dump(relationship, f)


def setup_facet(
    journal_path: Path,
    facet: str,
    *,
    title: str | None = None,
    description: str = "",
) -> Path:
    """Create a facet directory with minimal metadata for tests."""
    facet_dir = journal_path / "facets" / facet
    facet_dir.mkdir(parents=True, exist_ok=True)
    facet_data = {"title": title or facet.replace("-", " ").title()}
    if description:
        facet_data["description"] = description
    (facet_dir / "facet.json").write_text(json.dumps(facet_data), encoding="utf-8")
    return facet_dir


def _facet_json_bytes(payload: dict[str, object]) -> str:
    return json.dumps(payload, indent=2, ensure_ascii=False) + "\n"


def test_create_facet_with_icon_persists_icon(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    slug = create_facet("Treasury", icon="coins")

    payload = json.loads(
        (tmp_path / "facets" / slug / "facet.json").read_text(encoding="utf-8")
    )
    assert payload["icon"] == "coins"


def test_create_facet_without_icon_omits_icon_key(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    slug = create_facet("No Icon")

    expected = {
        "title": "No Icon",
        "description": "",
        "color": "#667eea",
        "emoji": "📦",
    }
    assert (tmp_path / "facets" / slug / "facet.json").read_text(
        encoding="utf-8"
    ) == _facet_json_bytes(expected)


def test_update_facet_icon_set_persists(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    slug = create_facet("Thinking")

    changed = update_facet(slug, icon="brain")

    payload = json.loads(
        (tmp_path / "facets" / slug / "facet.json").read_text(encoding="utf-8")
    )
    assert changed["icon"] == {"old": None, "new": "brain"}
    assert payload["icon"] == "brain"


def test_update_facet_invalid_icon_writes_nothing(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    slug = create_facet("Stable")
    facet_path = tmp_path / "facets" / slug / "facet.json"
    before = facet_path.read_text(encoding="utf-8")

    with pytest.raises(ValueError, match="open the picker or see lucide.dev/icons"):
        update_facet(slug, title="Changed", icon="not-a-real-icon")

    assert facet_path.read_text(encoding="utf-8") == before
    assert "icon" not in json.loads(before)


def test_update_facet_icon_clear_removes_key(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    slug = create_facet("Clearable", icon="brain")

    changed = update_facet(slug, icon="")

    payload = json.loads(
        (tmp_path / "facets" / slug / "facet.json").read_text(encoding="utf-8")
    )
    assert changed["icon"] == {"old": "brain", "new": None}
    assert "icon" not in payload


def test_update_facet_icon_clear_when_absent_is_observable_noop(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    slug = create_facet("Already Emoji")
    facet_path = tmp_path / "facets" / slug / "facet.json"
    before = facet_path.read_text(encoding="utf-8")
    today = datetime.now().strftime("%Y%m%d")
    log_path = tmp_path / "facets" / slug / "logs" / f"{today}.jsonl"
    before_log = log_path.read_text(encoding="utf-8")

    changed = update_facet(slug, icon="")

    assert changed == {}
    assert facet_path.read_text(encoding="utf-8") == before
    assert "icon" not in json.loads(before)
    assert log_path.read_text(encoding="utf-8") == before_log


def write_identity_config(
    journal_path: Path,
    *,
    name: str = "Test User",
    preferred: str = "Tester",
) -> None:
    """Write a minimal journal identity config for principal tests."""
    config_dir = journal_path / "config"
    config_dir.mkdir(parents=True, exist_ok=True)
    config = {"identity": {"name": name, "preferred": preferred}}
    (config_dir / "journal.json").write_text(json.dumps(config), encoding="utf-8")


def write_observations(
    journal_path: Path,
    facet: str,
    entity_name: str,
    observed_at_values: list[object],
) -> None:
    """Write observations.jsonl records for a test entity."""
    entity_id = slugify(entity_name, separator="_")
    observations_path = (
        journal_path / "facets" / facet / "entities" / entity_id / "observations.jsonl"
    )
    observations_path.parent.mkdir(parents=True, exist_ok=True)
    lines = [
        json.dumps(
            {
                "content": f"Observation {index}",
                "observed_at": observed_at,
                "source_day": "20260420",
            }
        )
        for index, observed_at in enumerate(observed_at_values, 1)
    ]
    observations_path.write_text(
        "\n".join(lines) + ("\n" if lines else ""),
        encoding="utf-8",
    )


def test_facet_summary_full(monkeypatch):
    """Test facet_summary with full metadata."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(FIXTURES_PATH))

    summary = facet_summary("full-featured")

    # Check title without emoji
    assert "# Full Featured Facet" in summary

    # Check description
    assert "**Description:** A facet for testing all features" in summary

    # Check color badge
    assert "![Color](#28a745)" in summary

    # Check entities section
    assert "## Entities" in summary
    assert "**Entity 1**: First test entity" in summary
    assert "**Entity 2**: Second test entity" in summary
    assert "**Entity 3**: Third test entity with description" in summary

    # Check activities section
    assert "## Activities" in summary
    assert "**Meetings** (high)" in summary
    assert "**Coding**" in summary
    assert "**Custom Activity**:" in summary
    assert "A custom test activity" in summary


def test_facet_summary_short_mode(monkeypatch):
    """Test facet_summary with detailed=False shows names only."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(FIXTURES_PATH))

    summary = facet_summary("full-featured", detailed=False)

    # Check title and description still present
    assert "# Full Featured Facet" in summary
    assert "**Description:** A facet for testing all features" in summary

    # Should NOT have detailed entities section
    assert "## Entities" not in summary
    # Should have inline entities list
    assert "**Entities**:" in summary

    # Should NOT have detailed activities section
    assert "## Activities" not in summary
    # Should have inline activities list
    assert "**Activities**:" in summary

    # Should NOT have activity descriptions
    assert "A custom test activity" not in summary


def test_facet_summary_minimal(monkeypatch):
    """Test facet_summary with minimal metadata."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(FIXTURES_PATH))

    summary = facet_summary("minimal-facet")

    # Check title without emoji
    assert "# Minimal Facet" in summary

    # Should not have description, color, or entities
    assert "**Description:**" not in summary
    assert "![Color]" not in summary
    assert "## Entities" not in summary


def test_facet_summary_test_facet(monkeypatch):
    """Test facet_summary with the existing test-facet fixture."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(FIXTURES_PATH))

    summary = facet_summary("test-facet")

    # Check title without emoji
    assert "# Test Facet" in summary

    # Check description
    assert "**Description:** A test facet for validating functionality" in summary

    # Check color badge
    assert "![Color](#007bff)" in summary


def test_facet_summary_nonexistent(monkeypatch):
    """Test facet_summary with nonexistent facet."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(FIXTURES_PATH))

    with pytest.raises(FileNotFoundError, match="Facet 'nonexistent' not found"):
        facet_summary("nonexistent")


def test_facet_summary_empty_journal(tmp_path, monkeypatch):
    """Test facet_summary raises FileNotFoundError with empty journal."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    with pytest.raises(FileNotFoundError, match="not found"):
        facet_summary("any-facet")


def test_facet_summary_missing_facet_json(monkeypatch):
    """Test facet_summary with missing facet.json."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(FIXTURES_PATH))

    with pytest.raises(FileNotFoundError, match="facet.json not found"):
        facet_summary("broken-facet")


def test_facet_summary_empty_entities(monkeypatch):
    """Test facet_summary with empty entities file."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(FIXTURES_PATH))

    summary = facet_summary("empty-entities")

    # Should not include entities section if file is empty
    assert "## Entities" not in summary


def test_get_facets_with_entities(monkeypatch):
    """Test that get_facets() returns metadata and load_entity_names() works with facets."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(FIXTURES_PATH))

    facets = get_facets()

    # Check test-facet exists
    assert "test-facet" in facets
    test_facet = facets["test-facet"]

    # Check basic metadata
    assert test_facet["title"] == "Test Facet"
    assert test_facet["emoji"] == "🧪"

    # Verify entities are NOT included in get_facets() anymore
    assert "entities" not in test_facet

    # Instead, verify entities can be loaded via load_entity_names()
    from solstone.think.entities import load_entity_names

    entity_names = load_entity_names(facet="test-facet")
    assert entity_names is not None

    # Check that specific entities are in the semicolon-delimited string
    assert "John Smith" in entity_names
    assert "Jane Doe" in entity_names
    assert "Bob Wilson" in entity_names
    assert "Acme Corp" in entity_names
    assert "Tech Solutions Inc" in entity_names
    assert "API Optimization" in entity_names
    assert "Dashboard Redesign" in entity_names
    assert "Visual Studio Code" in entity_names
    assert "Docker" in entity_names
    assert "PostgreSQL" in entity_names


def test_get_facets_empty_entities(monkeypatch):
    """Test get_facets() with facet that has no entities."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(FIXTURES_PATH))

    facets = get_facets()

    # Check minimal-facet (should have no entities file)
    if "minimal-facet" in facets:
        minimal_facet = facets["minimal-facet"]
        # Entities are no longer included in get_facets()
        assert "entities" not in minimal_facet

        # Verify load_entity_names returns None for facets without entities
        from solstone.think.entities import load_entity_names

        entity_names = load_entity_names(facet="minimal-facet")
        assert entity_names is None


def test_find_orphan_facets_detects_content_bearing_dirs_only(tmp_path, monkeypatch):
    """Orphan detection promotes only real content-bearing facet dirs."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    facets_dir = tmp_path / "facets"

    orphan_a = facets_dir / "orphan-a" / "todos"
    orphan_a.mkdir(parents=True)
    (orphan_a / "20260101.jsonl").write_text(
        json.dumps({"text": "Review"}) + "\n", encoding="utf-8"
    )

    orphan_b = facets_dir / "orphan-b" / "entities" / "alice"
    orphan_b.mkdir(parents=True)
    (orphan_b / "observations.jsonl").write_text(
        json.dumps({"content": "Met Alice"}) + "\n", encoding="utf-8"
    )

    junk = facets_dir / "junk"
    junk.mkdir(parents=True)
    (junk / ".gitkeep").write_text("", encoding="utf-8")

    lock_only = facets_dir / "lock-only" / "entities" / "x"
    lock_only.mkdir(parents=True)
    (lock_only / "observations.jsonl.lock").write_text("", encoding="utf-8")

    has_json = facets_dir / "has-json"
    (has_json / "news").mkdir(parents=True)
    (has_json / "facet.json").write_text(
        json.dumps({"title": "Has Json"}), encoding="utf-8"
    )
    (has_json / "news" / "20260101.md").write_text("news\n", encoding="utf-8")

    assert find_orphan_facets() == ["orphan-a", "orphan-b"]


def test_find_orphan_facets_fixture_excludes_broken_facet(monkeypatch):
    """The .gitkeep-only broken fixture is not promoted as an orphan."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(FIXTURES_PATH))

    assert "broken-facet" not in find_orphan_facets()


def test_ensure_facet_repairs_orphan_visible_in_get_facets(tmp_path, monkeypatch):
    """ensure_facet writes default metadata once and makes the facet visible."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    entities_dir = tmp_path / "facets" / "orphan" / "entities"
    entities_dir.mkdir(parents=True)
    (entities_dir / "20260101.jsonl").write_text(
        json.dumps({"type": "Person", "name": "Alice"}) + "\n", encoding="utf-8"
    )

    assert "orphan" not in get_facets()
    assert ensure_facet("orphan") is True

    facets = get_facets()
    assert "orphan" in facets
    assert facets["orphan"]["title"] == "Orphan"
    assert facets["orphan"]["color"] == "#667eea"
    assert facets["orphan"]["emoji"] == "📦"
    assert ensure_facet("orphan") is False

    today = datetime.now().strftime("%Y%m%d")
    log_path = tmp_path / "facets" / "orphan" / "logs" / f"{today}.jsonl"
    entries = [
        json.loads(line)
        for line in log_path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    assert [entry["action"] for entry in entries] == ["facet_heal"]


def test_facet_summaries(monkeypatch):
    """Test facet_summaries() generates correct agent prompt format."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(FIXTURES_PATH))

    summary = facet_summaries()

    # Check header
    assert "## Available Facets" in summary

    # Check test-facet is included with backtick format
    assert "**Test Facet** (`test-facet`)" in summary
    assert "A test facet for validating functionality" in summary

    # Check entities are included with title prefix
    assert "  - **Test Facet Entities**:" in summary
    # Verify some specific entities are present
    assert "John Smith" in summary
    assert "Jane Doe" in summary
    assert "Acme Corp" in summary
    assert "API Optimization" in summary

    # Check other facets are included
    assert "(`full-featured`)" in summary
    assert "(`minimal-facet`)" in summary

    # Check activities are included (short mode - names only)
    assert (
        "**Full Featured Facet Activities**: Meetings; Coding; Custom Activity"
        in summary
    )


def test_facet_summaries_excludes_muted(monkeypatch, tmp_path):
    """Test facet_summaries() excludes muted facets."""
    facets_dir = tmp_path / "facets"
    active_dir = facets_dir / "active"
    muted_dir = facets_dir / "muted_one"
    active_dir.mkdir(parents=True)
    muted_dir.mkdir(parents=True)

    (active_dir / "facet.json").write_text(
        json.dumps({"name": "active", "title": "Active Facet"}),
        encoding="utf-8",
    )
    (muted_dir / "facet.json").write_text(
        json.dumps({"name": "muted_one", "title": "Muted Facet", "muted": True}),
        encoding="utf-8",
    )

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    summary = facet_summaries()

    assert "(`active`)" in summary
    assert "(`muted_one`)" not in summary


def test_facet_summaries_no_facets(monkeypatch, tmp_path):
    """Test facet_summaries() when no facets exist."""
    empty_journal = tmp_path / "empty_journal"
    empty_journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(empty_journal))

    summary = facet_summaries()
    assert summary == "No facets found."


def test_facet_summaries_empty_journal(tmp_path, monkeypatch):
    """Test facet_summaries() returns 'No facets found' with empty journal."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    summary = facet_summaries()
    assert summary == "No facets found."


def test_facet_summaries_mixed_entities(monkeypatch):
    """Test facet_summaries() with facets having different entity configurations."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(FIXTURES_PATH))

    summary = facet_summaries()

    # Test facet should have entities (semicolon-delimited, not grouped by type)
    assert "**Test Facet** (`test-facet`)" in summary
    assert "  - **Test Facet Entities**:" in summary

    # Minimal facet should not have entity lists
    assert "**Minimal Facet** (`minimal-facet`)" in summary
    # Check that there's no entity list immediately after minimal-facet
    lines = summary.split("\n")
    for i, line in enumerate(lines):
        if "**Minimal Facet** (`minimal-facet`)" in line:
            # Next non-empty line should not be an entity list
            j = i + 1
            while j < len(lines) and lines[j].strip():
                # Should not have Entities line for minimal-facet
                if lines[j].strip().startswith("- **"):
                    # This means we've reached the next facet
                    break
                # If we're still in minimal-facet section, shouldn't have entities
                assert not lines[j].strip().startswith("- **Entities**:")
                j += 1
            break


def test_get_active_facets_from_segment_facets(monkeypatch, tmp_path):
    """Test get_active_facets() returns facets from segment facets.json files."""
    journal = tmp_path / "journal"
    day_dir = journal / "chronicle" / "20240115"

    # Create segment with facets.json containing two facets (stream layout)
    seg1 = day_dir / "archon" / "100000_300" / "talents"
    seg1.mkdir(parents=True)
    (seg1 / "facets.json").write_text(
        json.dumps(
            [
                {"facet": "work", "activity": "Code review", "level": "high"},
                {"facet": "personal", "activity": "Email check", "level": "low"},
            ]
        )
    )

    # Create another segment with overlapping + new facet
    seg2 = day_dir / "archon" / "110000_300" / "talents"
    seg2.mkdir(parents=True)
    (seg2 / "facets.json").write_text(
        json.dumps(
            [
                {"facet": "work", "activity": "Meeting", "level": "high"},
                {"facet": "sunstone", "activity": "Dev work", "level": "medium"},
            ]
        )
    )

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    active = get_active_facets("20240115")

    assert active == {"work", "personal", "sunstone"}


def test_get_active_facets_empty_segments(monkeypatch, tmp_path):
    """Test get_active_facets() with segments that have empty facets.json."""
    journal = tmp_path / "journal"
    day_dir = journal / "chronicle" / "20240115"

    # Segment with empty facets array (stream layout)
    seg1 = day_dir / "archon" / "100000_300" / "talents"
    seg1.mkdir(parents=True)
    (seg1 / "facets.json").write_text("[]")

    # Segment with empty file
    seg2 = day_dir / "archon" / "110000_300" / "talents"
    seg2.mkdir(parents=True)
    (seg2 / "facets.json").write_text("")

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    active = get_active_facets("20240115")

    assert active == set()


def test_get_active_facets_no_segments(monkeypatch, tmp_path):
    """Test get_active_facets() when day directory has no segments."""
    journal = tmp_path / "journal"
    (journal / "chronicle" / "20240115").mkdir(parents=True)

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    active = get_active_facets("20240115")

    assert active == set()


def test_get_active_facets_no_day_dir(monkeypatch, tmp_path):
    """Test get_active_facets() when day directory doesn't exist."""
    journal = tmp_path / "journal"
    journal.mkdir()

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    active = get_active_facets("20240115")

    assert active == set()


def test_get_active_facets_malformed_json(monkeypatch, tmp_path):
    """Test get_active_facets() skips malformed facets.json gracefully."""
    journal = tmp_path / "journal"
    day_dir = journal / "chronicle" / "20240115"

    # Malformed JSON segment (stream layout)
    seg1 = day_dir / "archon" / "100000_300" / "talents"
    seg1.mkdir(parents=True)
    (seg1 / "facets.json").write_text("{ invalid json")

    # Valid segment
    seg2 = day_dir / "archon" / "110000_300" / "talents"
    seg2.mkdir(parents=True)
    (seg2 / "facets.json").write_text(
        json.dumps(
            [
                {"facet": "work", "activity": "Coding", "level": "high"},
            ]
        )
    )

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    active = get_active_facets("20240115")

    assert active == {"work"}


_MISSING = object()


def _write_segment_sense(
    journal: Path,
    day: str,
    segment: str,
    speculative_facet: object = _MISSING,
    *,
    stream: str = "archon",
) -> None:
    talents_dir = journal / "chronicle" / day / stream / segment / "talents"
    talents_dir.mkdir(parents=True, exist_ok=True)
    payload = {}
    if speculative_facet is not _MISSING:
        payload["speculative_facet"] = speculative_facet
    (talents_dir / "sense.json").write_text(json.dumps(payload), encoding="utf-8")


def test_aggregate_speculative_facets_surfaces_above_threshold(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day = "20260602"
    for segment in ("090000_300", "093000_300", "100000_300"):
        _write_segment_sense(tmp_path, day, segment, "Home Reno")

    result = aggregate_speculative_facets(days=[day])

    assert result == [
        {
            "name": "Home Reno",
            "name_key": "home reno",
            "count": 3,
            "window_days": FACET_CANDIDATE_WINDOW_DAYS,
            "samples": [
                {"day": day, "stream": "archon", "segment": "090000_300"},
                {"day": day, "stream": "archon", "segment": "093000_300"},
                {"day": day, "stream": "archon", "segment": "100000_300"},
            ],
        }
    ]


def test_aggregate_speculative_facets_skips_one_off(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day = "20260602"
    _write_segment_sense(tmp_path, day, "090000_300", "Home Reno")
    _write_segment_sense(tmp_path, day, "093000_300", None)
    _write_segment_sense(tmp_path, day, "100000_300", None)

    assert aggregate_speculative_facets(days=[day]) == []


def test_aggregate_speculative_facets_skips_invalid_values(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day = "20260602"
    _write_segment_sense(tmp_path, day, "090000_300", None)
    _write_segment_sense(tmp_path, day, "093000_300")
    _write_segment_sense(tmp_path, day, "100000_300", 123)
    _write_segment_sense(tmp_path, day, "103000_300", "")
    _write_segment_sense(tmp_path, day, "110000_300", "Home Reno")

    result = aggregate_speculative_facets(days=[day], min_count=1)

    assert len(result) == 1
    assert result[0]["name_key"] == "home reno"
    assert result[0]["count"] == 1


def test_aggregate_speculative_facets_uses_rolling_window(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    now = datetime.now()
    today = now.strftime("%Y%m%d")
    two_days_ago = (now - timedelta(days=2)).strftime("%Y%m%d")
    old_day = (now - timedelta(days=FACET_CANDIDATE_WINDOW_DAYS + 5)).strftime("%Y%m%d")
    _write_segment_sense(tmp_path, today, "090000_300", "Home Reno")
    _write_segment_sense(tmp_path, today, "093000_300", "Home Reno")
    _write_segment_sense(tmp_path, two_days_ago, "100000_300", "Home Reno")
    _write_segment_sense(tmp_path, old_day, "090000_300", "Home Reno")
    _write_segment_sense(tmp_path, old_day, "093000_300", "Home Reno")

    result = aggregate_speculative_facets()

    assert len(result) == 1
    assert result[0]["name_key"] == "home reno"
    assert result[0]["count"] == 3


def test_aggregate_speculative_facets_groups_case_and_whitespace(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day = "20260602"
    _write_segment_sense(tmp_path, day, "090000_300", "Home Reno")
    _write_segment_sense(tmp_path, day, "093000_300", "  home   reno ")
    _write_segment_sense(tmp_path, day, "100000_300", "HOME RENO")

    result = aggregate_speculative_facets(days=[day])

    assert len(result) == 1
    assert result[0]["name"] == "Home Reno"
    assert result[0]["name_key"] == "home reno"
    assert result[0]["count"] == 3


# ============================================================================
# Principal role in facet summaries tests
# ============================================================================


def test_get_principal_display_name_preferred(tmp_path, monkeypatch):
    """Test _get_principal_display_name returns preferred name."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    config_dir = tmp_path / "config"
    config_dir.mkdir()
    config = {"identity": {"name": "Raylyn Miller", "preferred": "Rae"}}
    (config_dir / "journal.json").write_text(json.dumps(config))

    assert _get_principal_display_name() == "Rae"


def test_get_principal_display_name_fallback_to_name(tmp_path, monkeypatch):
    """Test _get_principal_display_name falls back to name when no preferred."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    config_dir = tmp_path / "config"
    config_dir.mkdir()
    config = {"identity": {"name": "Raylyn Miller", "preferred": ""}}
    (config_dir / "journal.json").write_text(json.dumps(config))

    assert _get_principal_display_name() == "Raylyn Miller"


def test_get_principal_display_name_none_when_empty(tmp_path, monkeypatch):
    """Test _get_principal_display_name returns None when identity empty."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    # No config file

    assert _get_principal_display_name() is None


def test_format_principal_role_with_principal(tmp_path, monkeypatch):
    """Test _format_principal_role extracts and formats principal."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    config_dir = tmp_path / "config"
    config_dir.mkdir()
    config = {"identity": {"name": "Raylyn", "preferred": "Rae"}}
    (config_dir / "journal.json").write_text(json.dumps(config))

    entities = [
        {"name": "Raylyn", "description": "Software engineer", "is_principal": True},
        {"name": "Bob", "description": "Friend"},
    ]

    role_line, filtered = _format_principal_role(entities)

    assert role_line == "**Rae's Role**: Software engineer"
    assert len(filtered) == 1
    assert filtered[0]["name"] == "Bob"


def test_format_principal_role_no_principal(tmp_path, monkeypatch):
    """Test _format_principal_role returns None when no principal."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    entities = [
        {"name": "Alice", "description": "Friend"},
        {"name": "Bob", "description": "Colleague"},
    ]

    role_line, filtered = _format_principal_role(entities)

    assert role_line is None
    assert filtered == entities


def test_format_principal_role_no_description(tmp_path, monkeypatch):
    """Test _format_principal_role returns None when principal has no description."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    config_dir = tmp_path / "config"
    config_dir.mkdir()
    config = {"identity": {"name": "Raylyn", "preferred": "Rae"}}
    (config_dir / "journal.json").write_text(json.dumps(config))

    entities = [
        {"name": "Raylyn", "description": "", "is_principal": True},
        {"name": "Bob", "description": "Friend"},
    ]

    role_line, filtered = _format_principal_role(entities)

    # No role line because description is empty
    assert role_line is None
    # But principal is still filtered out
    assert filtered == entities


def test_format_principal_role_no_identity(tmp_path, monkeypatch):
    """Test _format_principal_role returns None when no identity configured."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    # No config file

    entities = [
        {"name": "Raylyn", "description": "Engineer", "is_principal": True},
        {"name": "Bob", "description": "Friend"},
    ]

    role_line, filtered = _format_principal_role(entities)

    # No role line because no identity config
    assert role_line is None
    # Entities unchanged
    assert filtered == entities


def test_facet_summary_with_principal(tmp_path, monkeypatch):
    """Test facet_summary shows principal role and excludes from entities list."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create identity config
    config_dir = tmp_path / "config"
    config_dir.mkdir()
    config = {"identity": {"name": "Test User", "preferred": "Tester"}}
    (config_dir / "journal.json").write_text(json.dumps(config))

    # Create facet with principal entity using new structure
    facet_dir = tmp_path / "facets" / "work"
    facet_dir.mkdir(parents=True)
    (facet_dir / "facet.json").write_text(
        json.dumps({"title": "Work", "description": "Work stuff"})
    )
    setup_entities_new_structure(
        tmp_path,
        "work",
        [
            {
                "type": "Person",
                "name": "Test User",
                "description": "Lead developer",
                "is_principal": True,
            },
            {"type": "Person", "name": "Alice", "description": "Colleague"},
        ],
    )

    summary = facet_summary("work")

    # Should have principal role line
    assert "**Tester's Role**: Lead developer" in summary
    # Should have entities section with Alice but not Test User
    assert "## Entities" in summary
    assert "Alice" in summary
    assert "Colleague" in summary
    # Principal should not appear in entities list
    assert "- **Person**: Test User" not in summary


def test_facet_summary_principal_only_entity(tmp_path, monkeypatch):
    """Test facet_summary when principal is the only entity."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create identity config
    config_dir = tmp_path / "config"
    config_dir.mkdir()
    config = {"identity": {"name": "Test User", "preferred": "Tester"}}
    (config_dir / "journal.json").write_text(json.dumps(config))

    # Create facet with only principal entity using new structure
    facet_dir = tmp_path / "facets" / "solo"
    facet_dir.mkdir(parents=True)
    (facet_dir / "facet.json").write_text(json.dumps({"title": "Solo"}))
    setup_entities_new_structure(
        tmp_path,
        "solo",
        [
            {
                "type": "Person",
                "name": "Test User",
                "description": "Just me",
                "is_principal": True,
            },
        ],
    )

    summary = facet_summary("solo")

    # Should have principal role line
    assert "**Tester's Role**: Just me" in summary
    # Should NOT have entities section (no other entities)
    assert "## Entities" not in summary


def test_facet_summaries_detailed_with_principal(tmp_path, monkeypatch):
    """Test facet_summaries detailed mode shows principal role."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create identity config
    config_dir = tmp_path / "config"
    config_dir.mkdir()
    config = {"identity": {"name": "Test User", "preferred": "Tester"}}
    (config_dir / "journal.json").write_text(json.dumps(config))

    # Create facet with principal using new structure
    facet_dir = tmp_path / "facets" / "project"
    facet_dir.mkdir(parents=True)
    (facet_dir / "facet.json").write_text(
        json.dumps({"title": "Project X", "description": "Secret project"})
    )
    setup_entities_new_structure(
        tmp_path,
        "project",
        [
            {
                "type": "Person",
                "name": "Test User",
                "description": "Project lead",
                "is_principal": True,
            },
            {"type": "Person", "name": "Bob", "description": "Team member"},
        ],
    )

    summary = facet_summaries(detailed=True)

    # Should have principal role
    assert "**Tester's Role**: Project lead" in summary
    # Should have Bob in entities
    assert "Bob: Team member" in summary
    # Principal should not be in entities list
    assert "Test User: Project lead" not in summary


def test_facet_summaries_simple_mode_with_principal(tmp_path, monkeypatch):
    """Test facet_summaries simple mode also filters principal consistently."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create identity config
    config_dir = tmp_path / "config"
    config_dir.mkdir()
    config = {"identity": {"name": "Test User", "preferred": "Tester"}}
    (config_dir / "journal.json").write_text(json.dumps(config))

    # Create facet with principal using new structure
    facet_dir = tmp_path / "facets" / "simple"
    facet_dir.mkdir(parents=True)
    (facet_dir / "facet.json").write_text(json.dumps({"title": "Simple"}))
    setup_entities_new_structure(
        tmp_path,
        "simple",
        [
            {
                "type": "Person",
                "name": "Test User",
                "description": "Me",
                "is_principal": True,
            },
            {"type": "Person", "name": "Bob", "description": "Friend"},
        ],
    )

    summary = facet_summaries(detailed=False)

    # Simple mode now shows principal role (consistent with detailed mode)
    assert "**Tester's Role**: Me" in summary
    # Principal should not appear in entity names
    assert "Test User" not in summary
    # Other entities should appear
    assert "Bob" in summary


def test_facet_summaries_detailed_with_activities(monkeypatch):
    """Test facet_summaries detailed mode includes activity details."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(FIXTURES_PATH))

    summary = facet_summaries(detailed=True)

    # Check activities are included with details
    assert "**Full Featured Facet Activities**:" in summary
    assert "Meetings (high):" in summary
    assert "Video calls, in-person meetings, and conferences" in summary
    assert "Coding:" in summary
    assert "Custom Activity:" in summary
    assert "A custom test activity" in summary


def test_rank_entities_by_signal_orders_by_count_then_last_observed(
    tmp_path,
    monkeypatch,
):
    """Rank entities by observation count, then recency, then name."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    setup_facet(tmp_path, "signals", title="Signals")
    entities = [
        {"type": "Person", "name": "Alpha", "description": "A"},
        {"type": "Person", "name": "Beta", "description": "B"},
        {"type": "Person", "name": "Gamma", "description": "C"},
        {"type": "Person", "name": "Delta", "description": "D"},
    ]
    setup_entities_new_structure(tmp_path, "signals", entities)
    write_observations(
        tmp_path,
        "signals",
        "Alpha",
        ["2026-04-01T10:00:00Z", "2026-04-01T11:00:00Z", "2026-04-01T12:00:00Z"],
    )
    write_observations(
        tmp_path,
        "signals",
        "Beta",
        ["2026-04-15T10:00:00Z", "2026-04-15T11:00:00Z", "2026-04-15T12:00:00Z"],
    )
    write_observations(tmp_path, "signals", "Gamma", ["2026-04-10T09:00:00Z"])

    ranked = _rank_entities_by_signal("signals", entities)

    assert [entity["name"] for entity in ranked] == ["Beta", "Alpha", "Gamma", "Delta"]


def test_rank_entities_by_signal_uses_casefold_name_tiebreaker(tmp_path, monkeypatch):
    """Identical signals fall back to case-insensitive name ordering."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    setup_facet(tmp_path, "signals", title="Signals")
    entities = [
        {"type": "Person", "name": "bravo", "description": "B"},
        {"type": "Person", "name": "Alpha", "description": "A"},
    ]
    setup_entities_new_structure(tmp_path, "signals", entities)
    observed = ["2026-04-15T12:00:00Z", "2026-04-15T13:00:00Z"]
    write_observations(tmp_path, "signals", "bravo", observed)
    write_observations(tmp_path, "signals", "Alpha", observed)

    ranked = _rank_entities_by_signal("signals", entities)

    assert [entity["name"] for entity in ranked] == ["Alpha", "bravo"]


def test_facet_summaries_detailed_entity_cap_appends_trailing_bullet(
    tmp_path,
    monkeypatch,
):
    """Detailed mode caps entities and appends the trailing bullet."""
    from solstone.think.activities import save_facet_activities

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    setup_facet(
        tmp_path,
        "entity-cap",
        title="Entity Cap",
        description="Detailed entity cap",
    )
    setup_entities_new_structure(
        tmp_path,
        "entity-cap",
        [
            {
                "type": "Person",
                "name": f"Entity {index:02d}",
                "description": f"Description {index:02d}",
            }
            for index in range(1, 26)
        ],
    )
    save_facet_activities("entity-cap", [{"id": "meeting"}, {"id": "coding"}])

    summary = facet_summaries(detailed=True)

    assert "    - Entity 01: Description 01" in summary
    assert "    - Entity 20: Description 20" in summary
    assert "    - _and 5 more entities_" in summary
    assert "Entity 21: Description 21" not in summary
    assert "_and 1 more activities_" not in summary


def test_facet_summaries_detailed_activity_cap_appends_trailing_bullet(
    tmp_path,
    monkeypatch,
):
    """Detailed mode caps activities and appends the trailing bullet."""
    from solstone.think.activities import DEFAULT_ACTIVITIES, save_facet_activities

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    setup_facet(
        tmp_path,
        "activity-cap",
        title="Activity Cap",
        description="Detailed activity cap",
    )
    setup_entities_new_structure(
        tmp_path,
        "activity-cap",
        [
            {"type": "Person", "name": "Alice", "description": "Lead"},
            {"type": "Person", "name": "Bob", "description": "Partner"},
        ],
    )
    save_facet_activities(
        "activity-cap",
        [{"id": activity["id"]} for activity in DEFAULT_ACTIVITIES],
    )

    summary = facet_summaries(detailed=True)

    assert "    - Meetings" in summary
    assert "    - doctor appointment:" in summary
    assert "    - _and 10 more activities_" in summary
    assert "    - Writing:" not in summary
    assert "_and 1 more entities_" not in summary


def test_facet_summaries_simple_cap_trips_switches_capped_sections_to_bullets(
    tmp_path,
    monkeypatch,
):
    """Simple mode only switches capped sections to bullet lists."""
    from solstone.think.activities import save_facet_activities

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    setup_facet(
        tmp_path,
        "simple-cap",
        title="Simple Cap",
        description="Simple cap formatting",
    )
    setup_entities_new_structure(
        tmp_path,
        "simple-cap",
        [
            {
                "type": "Person",
                "name": f"Entity {index:02d}",
                "description": f"Description {index:02d}",
            }
            for index in range(1, 26)
        ],
    )
    save_facet_activities("simple-cap", [{"id": "meeting"}, {"id": "coding"}])

    summary = facet_summaries(detailed=False)

    assert "  - **Simple Cap Entities**:\n    - Entity 01" in summary
    assert "    - _and 5 more entities_" in summary
    assert "  - **Simple Cap Activities**: Meetings; Coding" in summary
    assert "  - **Simple Cap Entities**: Entity 01; Entity 02" not in summary
    assert "    - _and 1 more activities_" not in summary


def test_facet_summaries_exactly_at_caps_has_no_trailing_bullets(
    tmp_path,
    monkeypatch,
):
    """Exactly-at-cap output stays uncapped and keeps simple one-line formatting."""
    from solstone.think.activities import DEFAULT_ACTIVITIES, save_facet_activities

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    setup_facet(
        tmp_path,
        "exact-cap",
        title="Exact Cap",
        description="Exactly at cap",
    )
    setup_entities_new_structure(
        tmp_path,
        "exact-cap",
        [
            {
                "type": "Person",
                "name": f"Entity {index:02d}",
                "description": f"Description {index:02d}",
            }
            for index in range(1, 21)
        ],
    )
    save_facet_activities(
        "exact-cap",
        [{"id": activity["id"]} for activity in DEFAULT_ACTIVITIES[:15]],
    )

    detailed_summary = facet_summaries(detailed=True)
    simple_summary = facet_summaries(detailed=False)

    assert "_and 0 more entities_" not in detailed_summary
    assert "_and 0 more activities_" not in detailed_summary
    assert "_and 0 more entities_" not in simple_summary
    assert "_and 0 more activities_" not in simple_summary
    assert "  - **Exact Cap Entities**: Entity 01; Entity 02" in simple_summary
    assert "  - **Exact Cap Activities**: Meetings; call" in simple_summary


def test_facet_summaries_none_entity_cap_is_unbounded(tmp_path, monkeypatch):
    """None entity cap restores the full entity list."""
    from solstone.think.activities import save_facet_activities

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    setup_facet(
        tmp_path,
        "entity-unbounded",
        title="Entity Unbounded",
        description="No entity cap",
    )
    setup_entities_new_structure(
        tmp_path,
        "entity-unbounded",
        [
            {
                "type": "Person",
                "name": f"Entity {index:02d}",
                "description": f"Description {index:02d}",
            }
            for index in range(1, 26)
        ],
    )
    save_facet_activities("entity-unbounded", [{"id": "meeting"}])

    summary = facet_summaries(detailed=True, max_entities_per_facet=None)

    assert "Entity 25: Description 25" in summary
    assert "_and 5 more entities_" not in summary


def test_facet_summaries_none_activity_cap_is_unbounded(tmp_path, monkeypatch):
    """None activity cap restores the full activity list."""
    from solstone.think.activities import DEFAULT_ACTIVITIES, save_facet_activities

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    setup_facet(
        tmp_path,
        "activity-unbounded",
        title="Activity Unbounded",
        description="No activity cap",
    )
    setup_entities_new_structure(
        tmp_path,
        "activity-unbounded",
        [{"type": "Person", "name": "Alice", "description": "Lead"}],
    )
    save_facet_activities(
        "activity-unbounded",
        [{"id": activity["id"]} for activity in DEFAULT_ACTIVITIES],
    )

    summary = facet_summaries(detailed=True, max_activities_per_facet=None)

    assert "Music:" in summary
    assert "_and 1 more activities_" not in summary


def test_facet_summaries_principal_is_excluded_from_entity_budget(
    tmp_path,
    monkeypatch,
):
    """Principal role line does not count against the entity cap."""
    from solstone.think.activities import save_facet_activities

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    write_identity_config(tmp_path)
    setup_facet(
        tmp_path,
        "principal-budget",
        title="Principal Budget",
        description="Principal excluded from cap",
    )
    setup_entities_new_structure(
        tmp_path,
        "principal-budget",
        [
            {
                "type": "Person",
                "name": "Test User",
                "description": "Principal role",
                "is_principal": True,
            }
        ]
        + [
            {
                "type": "Person",
                "name": f"Entity {index:02d}",
                "description": f"Description {index:02d}",
            }
            for index in range(1, 21)
        ],
    )
    save_facet_activities("principal-budget", [{"id": "meeting"}])

    summary = facet_summaries(detailed=True)

    assert "**Tester's Role**: Principal role" in summary
    assert "    - Entity 20: Description 20" in summary
    assert "Test User: Principal role" not in summary
    assert "_and 1 more entities_" not in summary
