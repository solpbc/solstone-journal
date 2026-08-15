# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for facet-scoped entity utilities."""

import json

import pytest

from solstone.think.entities import (
    DEFAULT_ACTIVITY_TS,
    AkaConflictError,
    EntityBlockedError,
    EntityExistsError,
    EntityNotFoundError,
    add_entity_aka,
    add_observation,
    attach_or_reactivate_entity,
    block_journal_entity,
    count_observations,
    delete_journal_entity,
    detach_facet_entity,
    detected_entities_path,
    ensure_entity_memory,
    entity_last_active_day,
    entity_last_active_ts,
    entity_memory_path,
    entity_slug,
    find_matching_entity,
    get_identity_names,
    iter_detected_entity_names_since,
    last_active_day_for_ts,
    load_all_attached_entities,
    load_all_facet_relationships,
    load_all_journal_entities,
    load_detected_entities_recent,
    load_entities,
    load_facet_relationship,
    load_journal_entity,
    load_observations,
    load_recent_entity_names,
    observations_file_path,
    rename_entity_memory,
    resolve_entity,
    save_detected_entity,
    save_entities,
    save_facet_relationship,
    save_journal_entity,
    save_observations,
    unblock_journal_entity,
    update_detected_entity,
    update_facet_entity_description,
    update_facet_entity_identity,
    validate_aka_uniqueness,
)


@pytest.fixture
def fixture_journal(monkeypatch):
    """Set SOLSTONE_JOURNAL to tests/fixtures/journal for testing."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    yield
    # No cleanup needed - just testing reads


# ============================================================================
# entity_last_active_ts tests
# ============================================================================


def test_entity_last_active_ts_priority_last_seen():
    """Test that last_seen takes priority over other timestamps."""
    entity = {
        "last_seen": "20260115",  # Jan 15, 2026
        "updated_at": 1700000000000,  # Nov 2023
        "attached_at": 1600000000000,  # Sep 2020
    }
    ts = entity_last_active_ts(entity)
    # Should use last_seen, which is Jan 15 2026 local midnight
    from datetime import datetime

    expected = int(datetime(2026, 1, 15).timestamp() * 1000)
    assert ts == expected


def test_entity_last_active_ts_priority_updated_at():
    """Test that updated_at is used when last_seen is missing."""
    entity = {
        "updated_at": 1700000000000,
        "attached_at": 1600000000000,
    }
    ts = entity_last_active_ts(entity)
    assert ts == 1700000000000


def test_entity_last_active_ts_priority_attached_at():
    """Test that attached_at is used when last_seen and updated_at are missing."""
    entity = {
        "attached_at": 1600000000000,
    }
    ts = entity_last_active_ts(entity)
    assert ts == 1600000000000


def test_entity_last_active_ts_default():
    """Test that DEFAULT_ACTIVITY_TS is returned when no timestamps present."""
    entity = {"name": "Test Entity"}
    ts = entity_last_active_ts(entity)
    assert ts == DEFAULT_ACTIVITY_TS


def test_entity_last_active_ts_empty_entity():
    """Test with completely empty entity."""
    ts = entity_last_active_ts({})
    assert ts == DEFAULT_ACTIVITY_TS


def test_entity_last_active_ts_malformed_last_seen():
    """Test that malformed last_seen falls through to next priority."""
    entity = {
        "last_seen": "invalid",
        "updated_at": 1700000000000,
    }
    ts = entity_last_active_ts(entity)
    assert ts == 1700000000000


def test_entity_last_active_ts_short_last_seen():
    """Test that short last_seen string falls through."""
    entity = {
        "last_seen": "2026",  # Too short
        "updated_at": 1700000000000,
    }
    ts = entity_last_active_ts(entity)
    assert ts == 1700000000000


def test_entity_last_active_ts_zero_timestamps():
    """Test that zero timestamps are treated as missing."""
    entity = {
        "updated_at": 0,
        "attached_at": 0,
    }
    ts = entity_last_active_ts(entity)
    assert ts == DEFAULT_ACTIVITY_TS


@pytest.fixture
def tz_americas_evening(monkeypatch):
    """Pin process tz to US Eastern so a UTC-next-day instant is the same local day.

    Journal days bucket on local time, so the derived last-active day must follow
    local time — this is the evening-Americas case that produced "Tomorrow".
    """
    import os
    import time

    original_tz = os.environ.get("TZ")
    monkeypatch.setenv("TZ", "America/New_York")
    time.tzset()
    yield
    if original_tz is None:
        os.environ.pop("TZ", None)
    else:
        os.environ["TZ"] = original_tz
    time.tzset()


def test_last_active_day_for_ts_uses_local_journal_day(tz_americas_evening):
    """Evening-Americas instant renders the local journal day, not the next UTC day."""
    from datetime import datetime, timezone

    # 2026-01-15 23:30 America/New_York == 2026-01-16 04:30 UTC.
    instant = datetime(2026, 1, 16, 4, 30, tzinfo=timezone.utc)
    ts_ms = int(instant.timestamp() * 1000)

    assert last_active_day_for_ts(ts_ms) == "20260115"  # local journal day (the fix)
    assert last_active_day_for_ts(ts_ms) != "20260116"  # NOT the next UTC day (the bug)


def test_entity_last_active_day_from_updated_at_local_basis(tz_americas_evening):
    """entity_last_active_day derives the local journal day from the updated_at epoch."""
    from datetime import datetime, timezone

    instant = datetime(2026, 1, 16, 4, 30, tzinfo=timezone.utc)
    ts_ms = int(instant.timestamp() * 1000)

    assert entity_last_active_day({"updated_at": ts_ms}) == "20260115"


def test_entity_last_active_day_returns_last_seen_verbatim():
    """A valid last_seen journal-day string is returned without an epoch round-trip."""
    entity = {"last_seen": "20260115", "updated_at": 1700000000000}
    assert entity_last_active_day(entity) == "20260115"


def test_entity_last_active_day_malformed_last_seen_falls_through(
    tz_americas_evening,
):
    """Malformed last_seen falls through to the epoch-derived local day."""
    from datetime import datetime, timezone

    instant = datetime(2026, 1, 16, 4, 30, tzinfo=timezone.utc)
    ts_ms = int(instant.timestamp() * 1000)
    entity = {"last_seen": "invalid", "updated_at": ts_ms}
    assert entity_last_active_day(entity) == "20260115"


def test_entity_last_active_ts_negative_timestamps():
    """Test that negative timestamps are treated as missing."""
    entity = {
        "updated_at": -1,
        "attached_at": 1600000000000,
    }
    ts = entity_last_active_ts(entity)
    assert ts == 1600000000000


def test_detected_entities_path(fixture_journal):
    """Test path generation for detected entities."""
    path = detected_entities_path("personal", "20250101")
    assert str(path).endswith(
        "tests/fixtures/journal/facets/personal/entities/20250101.jsonl"
    )
    assert path.name == "20250101.jsonl"


def test_load_entities_attached(fixture_journal):
    """Test loading attached entities from fixtures."""
    entities = load_entities("personal")
    assert len(entities) == 3

    # Check entities are dicts with expected fields
    alice = next(e for e in entities if e.get("name") == "Alice Johnson")
    assert alice["type"] == "Person"
    assert alice["description"] == "Close friend from college"
    # Check extended fields are preserved
    assert alice.get("tags") == ["friend"]
    assert alice.get("contact") == "alice@example.com"

    bob = next(e for e in entities if e.get("name") == "Bob Smith")
    assert bob["type"] == "Person"
    assert bob["description"] == "Neighbor"

    acme = next(e for e in entities if e.get("name") == "Acme Corp")
    assert acme["type"] == "Company"
    assert acme["description"] == "Local tech startup"


def test_load_entities_detected(fixture_journal):
    """Test loading detected entities from fixtures."""
    entities = load_entities("personal", "20250101")
    assert len(entities) == 2

    charlie = next(e for e in entities if e.get("name") == "Charlie Brown")
    assert charlie["type"] == "Person"
    assert charlie["description"] == "Met at coffee shop"

    project = next(e for e in entities if e.get("name") == "Home Renovation")
    assert project["type"] == "Project"
    assert project["description"] == "Kitchen remodel project"


def test_load_entities_missing_file(fixture_journal):
    """Test loading from non-existent file returns empty list."""
    entities = load_entities("personal", "20991231")
    assert entities == []


def test_load_entities_missing_facet(fixture_journal):
    """Test loading from non-existent facet returns empty list."""
    entities = load_entities("nonexistent")
    assert entities == []


def test_save_and_load_entities(fixture_journal, tmp_path, monkeypatch):
    """Test saving and loading entities with real files."""
    # Create a temporary facet structure
    facet_path = tmp_path / "facets" / "test_facet"
    entities_dir = facet_path / "entities"
    entities_dir.mkdir(parents=True)

    # Update SOLSTONE_JOURNAL to temp directory
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Save some entities (dicts with extended fields)
    test_entities = [
        {
            "type": "Person",
            "name": "Test Person",
            "description": "Test description",
            "role": "tester",
        },
        {"type": "Company", "name": "Test Co", "description": "Test company"},
    ]
    save_entities("test_facet", test_entities, "20250101")

    # Load them back
    loaded = load_entities("test_facet", "20250101")
    assert len(loaded) == 2

    person = next(e for e in loaded if e.get("name") == "Test Person")
    assert person["type"] == "Person"
    assert person["description"] == "Test description"
    assert person.get("role") == "tester"  # Extended field preserved

    company = next(e for e in loaded if e.get("name") == "Test Co")
    assert company["type"] == "Company"
    assert company["description"] == "Test company"

    # Verify file exists and has correct JSONL format
    entity_file = entities_dir / "20250101.jsonl"
    assert entity_file.exists()
    content = entity_file.read_text()
    # Should be valid JSONL
    lines = [line for line in content.strip().split("\n") if line]
    assert len(lines) == 2
    import json

    for line in lines:
        assert json.loads(line)  # Should not raise


def test_save_entities_sorting(fixture_journal, tmp_path, monkeypatch):
    """Test that entities can be saved and loaded back correctly."""
    facet_path = tmp_path / "facets" / "test_facet"
    facet_path.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Save unsorted entities
    unsorted = [
        {
            "type": "Project",
            "name": "Zebra Project",
            "description": "Last alphabetically",
        },
        {"type": "Company", "name": "Acme", "description": "Company name"},
        {"type": "Person", "name": "Alice", "description": "Person name"},
        {"type": "Company", "name": "Beta Corp", "description": "Another company"},
    ]
    save_entities("test_facet", unsorted)

    # Verify entities are saved to new structure and can be loaded
    loaded = load_entities("test_facet")

    # All entities should be present
    assert len(loaded) == 4

    # Find each entity by name
    names = {e["name"] for e in loaded}
    assert "Zebra Project" in names
    assert "Acme" in names
    assert "Alice" in names
    assert "Beta Corp" in names

    # Verify journal-level entities were created
    from solstone.think.entities import scan_journal_entities

    journal_ids = scan_journal_entities()
    assert "zebra_project" in journal_ids
    assert "acme" in journal_ids
    assert "alice" in journal_ids
    assert "beta_corp" in journal_ids


def test_save_entities_detected_reflects_fresh_read(
    fixture_journal, tmp_path, monkeypatch
):
    """Detected entity reads reflect same-process saves."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "facets" / "test_facet" / "entities").mkdir(parents=True)

    save_entities(
        "test_facet",
        [{"type": "Person", "name": "Alice", "description": "First"}],
        "20250101",
    )

    loaded_first = load_entities("test_facet", "20250101")
    assert [e["name"] for e in loaded_first] == ["Alice"]

    save_entities(
        "test_facet",
        [
            {"type": "Person", "name": "Alice", "description": "First"},
            {"type": "Person", "name": "Bob", "description": "Second"},
        ],
        "20250101",
    )

    loaded_second = load_entities("test_facet", "20250101")
    assert {e["name"] for e in loaded_second} == {"Alice", "Bob"}


def test_save_entities_attached_reflects_fresh_read(
    fixture_journal, tmp_path, monkeypatch
):
    """Attached entity reads reflect same-process saves."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "facets" / "test_facet").mkdir(parents=True)

    save_entities(
        "test_facet",
        [{"type": "Person", "name": "Alice", "description": "First"}],
    )

    loaded_first = load_entities("test_facet")
    assert [e["name"] for e in loaded_first] == ["Alice"]

    save_entities(
        "test_facet",
        [{"type": "Person", "name": "Bob", "description": "Second"}],
    )

    loaded_second = load_entities("test_facet")
    assert {e["name"] for e in loaded_second} == {"Alice", "Bob"}


def test_concurrent_description_updates_preserve_both_entities(
    fixture_journal, tmp_path, monkeypatch
):
    """Concurrent attached-entity description writes do not lose sibling updates."""
    import threading

    facet = "test_facet"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "facets" / facet).mkdir(parents=True)

    save_entities(
        facet,
        [
            {"type": "Person", "name": "Alice", "description": "Initial Alice"},
            {"type": "Person", "name": "Bob", "description": "Initial Bob"},
        ],
    )

    barrier = threading.Barrier(2)
    errors: list[BaseException] = []

    def update_description(name: str, description: str) -> None:
        try:
            entity_id = entity_slug(name)
            load_entities(facet, include_detached=True)
            barrier.wait(timeout=5)
            update_facet_entity_description(facet, entity_id, description)
        except BaseException as exc:
            errors.append(exc)

    threads = [
        threading.Thread(
            target=update_description,
            args=("Alice", "Updated Alice"),
        ),
        threading.Thread(
            target=update_description,
            args=("Bob", "Updated Bob"),
        ),
    ]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=10)

    assert not errors
    assert load_facet_relationship(facet, "alice")["description"] == "Updated Alice"
    assert load_facet_relationship(facet, "bob")["description"] == "Updated Bob"


def test_attach_or_reactivate_entity_fresh_key_order(
    fixture_journal, tmp_path, monkeypatch
):
    """Fresh targeted attach writes the same ordered schema as bulk save."""
    facet = "test_facet"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    relationship, reattached = attach_or_reactivate_entity(
        facet,
        entity_type="Person",
        name="Alice Johnson",
        description="Friend",
    )

    assert reattached is False
    assert list(load_journal_entity("alice_johnson")) == [
        "id",
        "name",
        "type",
        "created_at",
    ]
    assert list(relationship) == [
        "entity_id",
        "description",
        "attached_at",
        "updated_at",
    ]
    assert list(load_facet_relationship(facet, "alice_johnson")) == [
        "entity_id",
        "description",
        "attached_at",
        "updated_at",
    ]


def test_detach_facet_entity_key_order(fixture_journal, tmp_path, monkeypatch):
    """Targeted detach preserves the existing relationship field order."""
    facet = "test_facet"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    attach_or_reactivate_entity(
        facet,
        entity_type="Person",
        name="Alice",
        description="Friend",
    )

    relationship = detach_facet_entity(facet, "alice")

    assert relationship["detached"] is True
    assert list(load_facet_relationship(facet, "alice")) == [
        "entity_id",
        "description",
        "attached_at",
        "updated_at",
        "detached",
    ]


def test_update_facet_entity_description_key_order(
    fixture_journal, tmp_path, monkeypatch
):
    """Targeted description updates keep the relationship schema order stable."""
    facet = "test_facet"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    attach_or_reactivate_entity(
        facet,
        entity_type="Person",
        name="Alice",
        description="Initial",
    )

    update_facet_entity_description(facet, "alice", "Updated")

    relationship = load_facet_relationship(facet, "alice")
    assert relationship["description"] == "Updated"
    assert list(relationship) == [
        "entity_id",
        "description",
        "attached_at",
        "updated_at",
    ]


def test_attached_owner_methods_raise_typed_errors(
    fixture_journal, tmp_path, monkeypatch
):
    """Targeted attached writes raise owner exceptions instead of value errors."""
    facet = "test_facet"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    save_entities(
        facet,
        [
            {"type": "Person", "name": "Alice", "description": "Friend"},
            {"type": "Person", "name": "Bob", "description": "Neighbor"},
        ],
    )

    with pytest.raises(EntityExistsError):
        attach_or_reactivate_entity(
            facet,
            entity_type="Person",
            name="Alice",
            description="Duplicate",
        )

    with pytest.raises(EntityNotFoundError):
        detach_facet_entity(facet, "missing")

    with pytest.raises(EntityNotFoundError):
        update_facet_entity_description(facet, "missing", "Updated")

    with pytest.raises(EntityExistsError):
        update_facet_entity_identity(
            facet,
            old_name="Alice",
            new_name="Bob",
            entity_type="Person",
            aka_list=[],
        )

    with pytest.raises(AkaConflictError):
        update_facet_entity_identity(
            facet,
            old_name="Alice",
            new_name="Alice",
            entity_type="Person",
            aka_list=["Bob"],
        )

    with pytest.raises(AkaConflictError):
        add_entity_aka(facet, "alice", "Bob")

    with pytest.raises(EntityNotFoundError):
        add_entity_aka(facet, "missing", "Missing Alias")


def test_attach_or_reactivate_entity_blocked_before_detached(
    fixture_journal, tmp_path, monkeypatch
):
    """Attach reports blocked for entities that are both blocked and detached."""
    facet = "test_facet"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    save_entities(
        facet,
        [{"type": "Person", "name": "Alice", "description": "Friend"}],
    )
    block_journal_entity("alice")

    with pytest.raises(EntityBlockedError):
        attach_or_reactivate_entity(
            facet,
            entity_type="Person",
            name="Alice",
            description="Friend",
        )


def test_save_journal_entity_reflects_fresh_reads(tmp_path, monkeypatch):
    """Journal entity readers reflect same-process saves."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    save_journal_entity({"id": "alice", "name": "Alice", "type": "Person"})

    assert load_journal_entity("alice")["name"] == "Alice"
    assert load_all_journal_entities()["alice"]["name"] == "Alice"

    save_journal_entity({"id": "alice", "name": "Alice Updated", "type": "Person"})

    assert load_journal_entity("alice")["name"] == "Alice Updated"
    assert load_all_journal_entities()["alice"]["name"] == "Alice Updated"


def test_save_facet_relationship_reflects_fresh_reads(tmp_path, monkeypatch):
    """Facet relationship readers reflect same-process saves."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    save_facet_relationship("work", "alice", {"description": "First"})

    assert load_facet_relationship("work", "alice")["description"] == "First"
    assert load_all_facet_relationships("work")["alice"]["description"] == "First"

    save_facet_relationship("work", "alice", {"description": "Second"})

    assert load_facet_relationship("work", "alice")["description"] == "Second"
    assert load_all_facet_relationships("work")["alice"]["description"] == "Second"


def test_save_detected_entity_basic(fixture_journal, tmp_path, monkeypatch):
    """Test save_detected_entity adds an entity with locking."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "facets" / "test_facet" / "entities").mkdir(parents=True)

    result = save_detected_entity("test_facet", "20250101", "Person", "Alice", "Friend")
    assert result["name"] == "Alice"
    assert result["type"] == "Person"

    loaded = load_entities("test_facet", "20250101")
    assert len(loaded) == 1
    assert loaded[0]["name"] == "Alice"


def test_save_detected_entity_duplicate(fixture_journal, tmp_path, monkeypatch):
    """Test save_detected_entity raises on duplicate."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "facets" / "test_facet" / "entities").mkdir(parents=True)

    save_detected_entity("test_facet", "20250101", "Person", "Alice", "Friend")

    import pytest as _pytest

    with _pytest.raises(ValueError, match="already detected"):
        save_detected_entity("test_facet", "20250101", "Person", "Alice", "Different")


def test_save_detected_entity_concurrent(fixture_journal, tmp_path, monkeypatch):
    """Test concurrent save_detected_entity calls don't lose data."""
    import threading

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "facets" / "test_facet" / "entities").mkdir(parents=True)

    errors = []
    count = 10

    def detect_entity(i):
        try:
            save_detected_entity(
                "test_facet", "20250101", "Person", f"Entity{i}", f"Description {i}"
            )
        except Exception as exc:
            errors.append(exc)

    threads = [threading.Thread(target=detect_entity, args=(i,)) for i in range(count)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()

    assert not errors, f"Unexpected errors: {errors}"
    loaded = load_entities("test_facet", "20250101")
    assert len(loaded) == count, f"Expected {count} entities, got {len(loaded)}"

    names = {e["name"] for e in loaded}
    for i in range(count):
        assert f"Entity{i}" in names, f"Entity{i} missing from saved entities"


def test_save_detected_entity_retry_on_error(fixture_journal, tmp_path, monkeypatch):
    """Test that save_detected_entity retries on transient OSError."""
    from unittest.mock import patch

    from solstone.think.journal_io import atomic_replace as _real_atomic_replace

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "facets" / "test_facet" / "entities").mkdir(parents=True)

    call_count = 0

    def flaky_atomic_replace(path, data, *, mode=None):
        nonlocal call_count
        call_count += 1
        if call_count == 1:
            raise PermissionError("Simulated transient error")
        return _real_atomic_replace(path, data, mode=mode)

    with patch(
        "solstone.think.entities.saving.atomic_replace",
        side_effect=flaky_atomic_replace,
    ):
        save_detected_entity("test_facet", "20250101", "Person", "Alice", "Friend")

    assert call_count == 2
    loaded = load_entities("test_facet", "20250101")
    assert len(loaded) == 1
    assert loaded[0]["name"] == "Alice"


def test_update_detected_entity(fixture_journal, tmp_path, monkeypatch):
    """Test update_detected_entity with locking."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "facets" / "test_facet" / "entities").mkdir(parents=True)

    save_detected_entity("test_facet", "20250101", "Person", "Alice", "Friend")
    result = update_detected_entity("test_facet", "20250101", "Alice", "Best friend")
    assert result["description"] == "Best friend"

    loaded = load_entities("test_facet", "20250101")
    assert loaded[0]["description"] == "Best friend"


def test_update_detected_entity_not_found(fixture_journal, tmp_path, monkeypatch):
    """Test update_detected_entity raises when entity missing."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "facets" / "test_facet" / "entities").mkdir(parents=True)

    import pytest as _pytest

    with _pytest.raises(ValueError, match="not found"):
        update_detected_entity("test_facet", "20250101", "Nobody", "Desc")


def test_load_all_attached_entities(fixture_journal):
    """Test loading all attached entities from all facets."""
    all_entities = load_all_attached_entities()

    # Should have entities from both personal and full-featured facets
    assert len(all_entities) >= 3  # At least the personal facet entities

    # Check personal facet entities are present
    entity_names = [e.get("name") for e in all_entities]
    assert "Alice Johnson" in entity_names
    assert "Bob Smith" in entity_names
    assert "Acme Corp" in entity_names


def test_load_all_attached_entities_deduplication(
    fixture_journal, tmp_path, monkeypatch
):
    """Test that load_all_attached_entities deduplicates by name."""
    # Create two facets with overlapping entity names
    facet1_path = tmp_path / "facets" / "facet1"
    facet2_path = tmp_path / "facets" / "facet2"
    facet1_path.mkdir(parents=True)
    facet2_path.mkdir(parents=True)

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Save same entity name in both facets with different descriptions
    entities1 = [
        {
            "type": "Person",
            "name": "John Smith",
            "description": "Description from facet1",
        }
    ]
    entities2 = [
        {
            "type": "Person",
            "name": "John Smith",
            "description": "Description from facet2",
        }
    ]

    save_entities("facet1", entities1)
    save_entities("facet2", entities2)

    # Load all entities
    all_entities = load_all_attached_entities()

    # Should only have one "John Smith" (from first facet alphabetically)
    john_smiths = [e for e in all_entities if e.get("name") == "John Smith"]
    assert len(john_smiths) == 1
    # Should be from facet1 (alphabetically first)
    assert john_smiths[0]["description"] == "Description from facet1"


def test_load_all_attached_entities_sort_by_last_seen(
    fixture_journal, tmp_path, monkeypatch
):
    """Test sorting entities by last_seen."""
    facet_path = tmp_path / "facets" / "test_facet"
    facet_path.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create entities with varying last_seen values
    entities = [
        {"type": "Person", "name": "Old Entity", "description": "No last_seen"},
        {
            "type": "Person",
            "name": "Recent Entity",
            "description": "Most recent",
            "last_seen": "20260108",
        },
        {
            "type": "Person",
            "name": "Middle Entity",
            "description": "Middle",
            "last_seen": "20260105",
        },
    ]
    save_entities("test_facet", entities)

    # Load with sorting
    result = load_all_attached_entities(sort_by="last_seen")

    # Most recent should be first, no last_seen should be last
    assert result[0]["name"] == "Recent Entity"
    assert result[1]["name"] == "Middle Entity"
    assert result[2]["name"] == "Old Entity"


def test_load_all_attached_entities_limit(fixture_journal, tmp_path, monkeypatch):
    """Test limiting number of entities returned."""
    facet_path = tmp_path / "facets" / "test_facet"
    facet_path.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create 5 entities
    entities = [
        {"type": "Person", "name": f"Entity {i}", "description": f"Desc {i}"}
        for i in range(5)
    ]
    save_entities("test_facet", entities)

    # Load with limit
    result = load_all_attached_entities(limit=3)
    assert len(result) == 3


def test_load_all_attached_entities_sort_and_limit(
    fixture_journal, tmp_path, monkeypatch
):
    """Test sorting and limiting together."""
    facet_path = tmp_path / "facets" / "test_facet"
    facet_path.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create entities with last_seen
    entities = [
        {"type": "Person", "name": "A", "last_seen": "20260101"},
        {"type": "Person", "name": "B", "last_seen": "20260108"},
        {"type": "Person", "name": "C", "last_seen": "20260105"},
        {"type": "Person", "name": "D", "last_seen": "20260103"},
    ]
    save_entities("test_facet", entities)

    # Get top 2 most recent
    result = load_all_attached_entities(sort_by="last_seen", limit=2)
    assert len(result) == 2
    assert result[0]["name"] == "B"  # 20260108
    assert result[1]["name"] == "C"  # 20260105


# Tests for load_recent_entity_names


def test_load_recent_entity_names_basic(fixture_journal, tmp_path, monkeypatch):
    """Test basic functionality of load_recent_entity_names."""
    facet_path = tmp_path / "facets" / "test_facet"
    facet_path.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create entities with last_seen
    entities = [
        {"type": "Person", "name": "Alice Johnson", "last_seen": "20260108"},
        {"type": "Company", "name": "Acme Corp", "last_seen": "20260107"},
    ]
    save_entities("test_facet", entities)

    result = load_recent_entity_names()

    # Should return list of spoken forms
    assert result is not None
    assert isinstance(result, list)
    assert "Alice" in result
    assert "Acme" in result


def test_load_recent_entity_names_returns_list(fixture_journal, tmp_path, monkeypatch):
    """Test that result is a list of names."""
    facet_path = tmp_path / "facets" / "test_facet"
    facet_path.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create 10 entities with speakable names (no digits)
    names = [
        "Alice",
        "Bob",
        "Carol",
        "Dan",
        "Eve",
        "Frank",
        "Grace",
        "Hank",
        "Ivy",
        "Jack",
    ]
    entities = [
        {"type": "Person", "name": name, "last_seen": f"202601{i:02d}"}
        for i, name in enumerate(names, start=1)
    ]
    save_entities("test_facet", entities)

    result = load_recent_entity_names(limit=10)

    assert result is not None
    assert isinstance(result, list)
    assert len(result) == 10


def test_load_recent_entity_names_empty(fixture_journal, tmp_path, monkeypatch):
    """Test with no entities returns None."""
    facet_path = tmp_path / "facets" / "test_facet"
    facet_path.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    result = load_recent_entity_names()
    assert result is None


def test_load_recent_entity_names_with_aka(fixture_journal, tmp_path, monkeypatch):
    """Test that aka values are included in spoken names."""
    facet_path = tmp_path / "facets" / "test_facet"
    facet_path.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    entities = [
        {
            "type": "Person",
            "name": "Robert Johnson",
            "aka": ["Bob", "Bobby"],
            "last_seen": "20260108",
        },
    ]
    save_entities("test_facet", entities)

    result = load_recent_entity_names()

    assert result is not None
    assert isinstance(result, list)
    assert "Robert" in result
    assert "Bob" in result
    assert "Bobby" in result


def test_load_recent_entity_names_respects_limit(
    fixture_journal, tmp_path, monkeypatch
):
    """Test that limit parameter is respected."""
    facet_entities_path = tmp_path / "facets" / "test_facet" / "entities"
    journal_entities_path = tmp_path / "entities"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    names = [
        "Alice",
        "Bob",
        "Carol",
        "Dan",
        "Eve",
        "Frank",
        "Grace",
        "Hank",
        "Ivy",
        "Jack",
        "Kate",
        "Leo",
        "Mia",
        "Nick",
        "Olive",
        "Paul",
        "Quinn",
        "Rose",
        "Sam",
        "Tina",
        "Uma",
        "Vic",
        "Wendy",
        "Xander",
        "Yara",
        "Zane",
        "Abel",
        "Beth",
        "Cody",
        "Dawn",
    ]
    for index, name in enumerate(names, start=1):
        entity_id = entity_slug(name)
        relationship_dir = facet_entities_path / entity_id
        relationship_dir.mkdir(parents=True, exist_ok=True)
        (relationship_dir / "entity.json").write_text(
            json.dumps({"last_seen": f"202601{index:02d}"}), encoding="utf-8"
        )
        entity_dir = journal_entities_path / entity_id
        entity_dir.mkdir(parents=True, exist_ok=True)
        (entity_dir / "entity.json").write_text(
            json.dumps({"id": entity_id, "type": "Person", "name": name}),
            encoding="utf-8",
        )

    result = load_recent_entity_names(limit=5)

    assert result is not None
    assert isinstance(result, list)
    assert "Dawn" in result
    assert "Zane" in result
    assert "Alice" not in result


def test_load_recent_entity_names_filters_unspeakable(
    fixture_journal, tmp_path, monkeypatch
):
    """Test that names with underscores or no letters are filtered out."""
    facet_path = tmp_path / "facets" / "test_facet"
    facet_path.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    entities = [
        # Speakable - should be included (letters required, digits OK)
        {"type": "Person", "name": "Alice", "last_seen": "20260110"},
        {"type": "Company", "name": "Acme Corp", "last_seen": "20260109"},
        {"type": "Person", "name": "Bob O'Brien", "last_seen": "20260108"},
        {"type": "Project", "name": "Project-X", "last_seen": "20260107"},
        {"type": "Tool", "name": "send2trash", "last_seen": "20260106"},  # has letters
        {
            "type": "Person",
            "name": "Ryan (R2)",
            "last_seen": "20260105",
        },  # R2 has letter
        # Unspeakable - should be filtered (underscores or no letters)
        {
            "type": "Tool",
            "name": "entity_registry",
            "last_seen": "20260104",
        },  # underscore
        {
            "type": "Tool",
            "name": "whisper_ctranslate2",
            "last_seen": "20260103",
        },  # underscore
        {
            "type": "Code",
            "name": "12345",
            "last_seen": "20260102",
        },  # no letters
    ]
    save_entities("test_facet", entities)

    result = load_recent_entity_names()

    assert result is not None
    # Speakable names included (digits OK if has letters)
    assert "Alice" in result
    assert "Acme" in result
    assert "Bob" in result
    assert "Project-X" in result
    assert "send2trash" in result  # has letters, digits OK
    assert "Ryan" in result
    assert "R2" in result  # has letter, digit OK
    # Unspeakable names filtered out
    assert "entity_registry" not in result  # underscore
    assert "whisper_ctranslate2" not in result  # underscore
    assert "12345" not in result  # no letters


def test_aka_field_preservation(fixture_journal, tmp_path, monkeypatch):
    """Test that aka field is preserved during save/load operations."""
    facet_path = tmp_path / "facets" / "test_facet"
    facet_path.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Save entities with aka fields
    test_entities = [
        {
            "type": "Person",
            "name": "Alice Johnson",
            "description": "Lead engineer",
            "aka": ["Ali", "AJ"],
        },
        {
            "type": "Company",
            "name": "PostgreSQL",
            "description": "Database system",
            "aka": ["Postgres", "PG"],
        },
    ]
    save_entities("test_facet", test_entities)

    # Load them back
    loaded = load_entities("test_facet")
    assert len(loaded) == 2

    alice = next(e for e in loaded if e.get("name") == "Alice Johnson")
    assert alice.get("aka") == ["Ali", "AJ"]

    postgres = next(e for e in loaded if e.get("name") == "PostgreSQL")
    assert postgres.get("aka") == ["Postgres", "PG"]


# Tests for load_detected_entities_recent


def test_load_detected_entities_recent_basic(fixture_journal):
    """Test loading detected entities with count and last_seen."""
    # Fixture has detected entities in 20250101 and 20250102
    # But these dates are old (> 30 days from now), so we need to use a large days value
    detected = load_detected_entities_recent("personal", days=36500)  # ~100 years

    # Should have 4 detected entities (Charlie Brown, Home Renovation, City Fitness, Diana Prince)
    # Note: excludes Alice Johnson, Bob Smith, Acme Corp which are attached
    assert len(detected) == 4

    # Check structure includes count and last_seen
    for entity in detected:
        assert "type" in entity
        assert "name" in entity
        assert "description" in entity
        assert "count" in entity
        assert "last_seen" in entity


def test_iter_detected_entity_names_since_fixture(fixture_journal):
    """Detected entity iterator yields names with facet and day."""
    detected = set(iter_detected_entity_names_since("20260310"))

    assert ("Romeo Montague", "montague", "20260310") in detected


def test_load_detected_entities_recent_excludes_attached(
    fixture_journal, tmp_path, monkeypatch
):
    """Test that attached entities and their akas are excluded from detected results."""
    facet_path = tmp_path / "facets" / "test_facet"
    entities_dir = facet_path / "entities"
    entities_dir.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create attached entity with aka
    attached = [
        {
            "type": "Person",
            "name": "Alice Johnson",
            "description": "Attached person",
            "aka": ["Ali", "AJ"],
        }
    ]
    save_entities("test_facet", attached)

    # Create detected entities including some that match attached/aka
    detected_entities = [
        {
            "type": "Person",
            "name": "Alice Johnson",
            "description": "Should be excluded",
        },
        {"type": "Person", "name": "Ali", "description": "Should be excluded (aka)"},
        {
            "type": "Person",
            "name": "Charlie Brown",
            "description": "Should be included",
        },
    ]
    save_entities("test_facet", detected_entities, "20250101")

    # Load detected - should only get Charlie Brown
    detected = load_detected_entities_recent("test_facet", days=36500)
    assert len(detected) == 1
    assert detected[0]["name"] == "Charlie Brown"


def test_load_detected_entities_recent_count_tracking(
    fixture_journal, tmp_path, monkeypatch
):
    """Test that count tracks occurrences across multiple days."""
    facet_path = tmp_path / "facets" / "test_facet"
    entities_dir = facet_path / "entities"
    entities_dir.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create same entity across multiple days
    save_entities(
        "test_facet",
        [{"type": "Person", "name": "Charlie", "description": "Day 1 desc"}],
        "20250101",
    )
    save_entities(
        "test_facet",
        [{"type": "Person", "name": "Charlie", "description": "Day 2 desc"}],
        "20250102",
    )
    save_entities(
        "test_facet",
        [{"type": "Person", "name": "Charlie", "description": "Day 3 desc"}],
        "20250103",
    )

    detected = load_detected_entities_recent("test_facet", days=36500)
    assert len(detected) == 1

    charlie = detected[0]
    assert charlie["name"] == "Charlie"
    assert charlie["count"] == 3


def test_load_detected_entities_recent_last_seen(
    fixture_journal, tmp_path, monkeypatch
):
    """Test that last_seen is the most recent day and description is from that day."""
    facet_path = tmp_path / "facets" / "test_facet"
    entities_dir = facet_path / "entities"
    entities_dir.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create entity across multiple days with different descriptions
    save_entities(
        "test_facet",
        [{"type": "Person", "name": "Charlie", "description": "Oldest description"}],
        "20250101",
    )
    save_entities(
        "test_facet",
        [
            {
                "type": "Person",
                "name": "Charlie",
                "description": "Most recent description",
            }
        ],
        "20250103",
    )
    save_entities(
        "test_facet",
        [{"type": "Person", "name": "Charlie", "description": "Middle description"}],
        "20250102",
    )

    detected = load_detected_entities_recent("test_facet", days=36500)
    assert len(detected) == 1

    charlie = detected[0]
    assert charlie["last_seen"] == "20250103"
    assert charlie["description"] == "Most recent description"


def test_load_detected_entities_recent_days_filter(
    fixture_journal, tmp_path, monkeypatch
):
    """Test that days parameter limits results to recent days."""
    facet_path = tmp_path / "facets" / "test_facet"
    entities_dir = facet_path / "entities"
    entities_dir.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    from datetime import datetime, timedelta

    # Create entities at various dates relative to today
    today = datetime.now()
    recent_day = (today - timedelta(days=5)).strftime("%Y%m%d")
    old_day = (today - timedelta(days=60)).strftime("%Y%m%d")

    save_entities(
        "test_facet",
        [{"type": "Person", "name": "Recent Person", "description": "Recent"}],
        recent_day,
    )
    save_entities(
        "test_facet",
        [{"type": "Person", "name": "Old Person", "description": "Old"}],
        old_day,
    )

    # With default 30 days, should only get recent person
    detected = load_detected_entities_recent("test_facet", days=30)
    assert len(detected) == 1
    assert detected[0]["name"] == "Recent Person"

    # With 90 days, should get both
    detected = load_detected_entities_recent("test_facet", days=90)
    assert len(detected) == 2


def test_load_detected_entities_recent_empty_facet(
    fixture_journal, tmp_path, monkeypatch
):
    """Test that empty or non-existent facet returns empty list."""
    facet_path = tmp_path / "facets" / "empty_facet"
    facet_path.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # No entities directory
    detected = load_detected_entities_recent("empty_facet")
    assert detected == []


def test_load_detected_entities_recent_type_name_key(
    fixture_journal, tmp_path, monkeypatch
):
    """Test that deduplication is by (type, name) tuple, not just name."""
    facet_path = tmp_path / "facets" / "test_facet"
    entities_dir = facet_path / "entities"
    entities_dir.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Same name, different types - should be treated as separate entities
    save_entities(
        "test_facet",
        [
            {"type": "Person", "name": "Mercury", "description": "Roman god"},
            {"type": "Project", "name": "Mercury", "description": "Space program"},
        ],
        "20250101",
    )

    detected = load_detected_entities_recent("test_facet", days=36500)
    assert len(detected) == 2

    names_and_types = {(e["type"], e["name"]) for e in detected}
    assert ("Person", "Mercury") in names_and_types
    assert ("Project", "Mercury") in names_and_types


def test_timestamp_preservation(fixture_journal, tmp_path, monkeypatch):
    """Test that attached_at and updated_at timestamps are preserved through save/load."""
    facet_path = tmp_path / "facets" / "test_facet"
    facet_path.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Save entities with timestamps
    test_entities = [
        {
            "type": "Person",
            "name": "Alice",
            "description": "Test person",
            "attached_at": 1700000000000,
            "updated_at": 1700000001000,
        },
        {
            "type": "Company",
            "name": "Acme",
            "description": "Test company",
            "attached_at": 1700000002000,
            "updated_at": 1700000002000,
        },
    ]
    save_entities("test_facet", test_entities)

    # Load them back
    loaded = load_entities("test_facet")
    assert len(loaded) == 2

    alice = next(e for e in loaded if e.get("name") == "Alice")
    assert alice["attached_at"] == 1700000000000
    assert alice["updated_at"] == 1700000001000

    acme = next(e for e in loaded if e.get("name") == "Acme")
    assert acme["attached_at"] == 1700000002000
    assert acme["updated_at"] == 1700000002000


# Tests for detached entity functionality


def test_load_entities_excludes_detached_by_default(
    fixture_journal, tmp_path, monkeypatch
):
    """Test that load_entities excludes detached entities by default."""
    facet_path = tmp_path / "facets" / "test_facet"
    facet_path.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Save entities with one detached
    test_entities = [
        {"type": "Person", "name": "Alice", "description": "Active person"},
        {
            "type": "Person",
            "name": "Bob",
            "description": "Detached person",
            "detached": True,
        },
        {"type": "Company", "name": "Acme", "description": "Active company"},
    ]
    save_entities("test_facet", test_entities)

    # Load without include_detached (default)
    loaded = load_entities("test_facet")
    assert len(loaded) == 2
    names = [e["name"] for e in loaded]
    assert "Alice" in names
    assert "Acme" in names
    assert "Bob" not in names


def test_load_entities_includes_detached_when_requested(
    fixture_journal, tmp_path, monkeypatch
):
    """Test that load_entities includes detached entities when include_detached=True."""
    facet_path = tmp_path / "facets" / "test_facet"
    facet_path.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Save entities with one detached
    test_entities = [
        {"type": "Person", "name": "Alice", "description": "Active person"},
        {
            "type": "Person",
            "name": "Bob",
            "description": "Detached person",
            "detached": True,
        },
    ]
    save_entities("test_facet", test_entities)

    # Load with include_detached=True
    loaded = load_entities("test_facet", include_detached=True)
    assert len(loaded) == 2
    names = [e["name"] for e in loaded]
    assert "Alice" in names
    assert "Bob" in names

    # Verify detached flag is preserved
    bob = next(e for e in loaded if e["name"] == "Bob")
    assert bob.get("detached") is True


def test_load_all_attached_entities_excludes_detached(
    fixture_journal, tmp_path, monkeypatch
):
    """Test that load_all_attached_entities excludes detached entities."""
    facet1_path = tmp_path / "facets" / "facet1"
    facet2_path = tmp_path / "facets" / "facet2"
    facet1_path.mkdir(parents=True)
    facet2_path.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Save entities - one active, one detached per facet
    save_entities(
        "facet1",
        [
            {"type": "Person", "name": "Alice", "description": "Active in facet1"},
            {
                "type": "Person",
                "name": "Bob",
                "description": "Detached in facet1",
                "detached": True,
            },
        ],
    )
    save_entities(
        "facet2",
        [
            {"type": "Person", "name": "Charlie", "description": "Active in facet2"},
        ],
    )

    all_entities = load_all_attached_entities()

    # Should only have active entities
    names = [e["name"] for e in all_entities]
    assert "Alice" in names
    assert "Charlie" in names
    assert "Bob" not in names


def test_load_detected_entities_recent_shows_detached_entity_names(
    fixture_journal, tmp_path, monkeypatch
):
    """Test that detached entities appear in detected list again (not excluded)."""
    facet_path = tmp_path / "facets" / "test_facet"
    entities_dir = facet_path / "entities"
    entities_dir.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create attached entity with detached=True
    attached = [
        {"type": "Person", "name": "Alice", "description": "Active person"},
        {
            "type": "Person",
            "name": "Bob",
            "description": "Detached person",
            "detached": True,
        },
    ]
    save_entities("test_facet", attached)

    # Create detected entities including the detached name
    detected_entities = [
        {
            "type": "Person",
            "name": "Alice",
            "description": "Should be excluded (active)",
        },
        {
            "type": "Person",
            "name": "Bob",
            "description": "Should be INCLUDED (detached)",
        },
        {
            "type": "Person",
            "name": "Charlie",
            "description": "Should be included (new)",
        },
    ]
    save_entities("test_facet", detected_entities, "20250101")

    # Load detected - Alice excluded (active), Bob included (detached), Charlie included (new)
    detected = load_detected_entities_recent("test_facet", days=36500)
    names = [e["name"] for e in detected]

    assert "Alice" not in names  # Excluded - still active
    assert "Bob" in names  # Included - detached, so shows up in detected
    assert "Charlie" in names  # Included - new entity


def test_detached_entity_preserves_all_fields(fixture_journal, tmp_path, monkeypatch):
    """Test that detached entities preserve all fields including custom ones."""
    facet_path = tmp_path / "facets" / "test_facet"
    facet_path.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Save entity with custom fields and detached flag
    test_entities = [
        {
            "type": "Person",
            "name": "Alice",
            "description": "Test person",
            "attached_at": 1700000000000,
            "updated_at": 1700000001000,
            "aka": ["Ali", "AJ"],
            "tags": ["friend", "colleague"],
            "custom_field": "custom_value",
            "detached": True,
        },
    ]
    save_entities("test_facet", test_entities)

    # Load with include_detached to verify all fields preserved
    loaded = load_entities("test_facet", include_detached=True)
    assert len(loaded) == 1

    alice = loaded[0]
    assert alice["name"] == "Alice"
    assert alice["description"] == "Test person"
    assert alice["attached_at"] == 1700000000000
    assert alice["updated_at"] == 1700000001000
    assert alice["aka"] == ["Ali", "AJ"]
    assert alice["tags"] == ["friend", "colleague"]
    assert alice["custom_field"] == "custom_value"
    assert alice["detached"] is True


def test_detached_flag_for_detected_entities_not_filtered(
    fixture_journal, tmp_path, monkeypatch
):
    """Test that include_detached only affects attached entities, not detected."""
    facet_path = tmp_path / "facets" / "test_facet"
    entities_dir = facet_path / "entities"
    entities_dir.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create detected entity for a specific day
    detected_entities = [
        {"type": "Person", "name": "Alice", "description": "Detected person"},
    ]
    save_entities("test_facet", detected_entities, "20250101")

    # Load detected entities - should always return all (no detached filtering for detected)
    loaded = load_entities("test_facet", "20250101")
    assert len(loaded) == 1

    # include_detached should have no effect on detected entities
    loaded_with_flag = load_entities("test_facet", "20250101", include_detached=True)
    assert len(loaded_with_flag) == 1


# Tests for entity memory utilities


def test_entity_slug_basic():
    """Test basic name slug generation."""
    assert entity_slug("Alice Johnson") == "alice_johnson"
    assert entity_slug("Acme Corp") == "acme_corp"
    assert entity_slug("PostgreSQL") == "postgresql"


def test_entity_slug_special_chars():
    """Test slug generation with special characters."""
    assert entity_slug("O'Brien") == "o_brien"
    assert entity_slug("AT&T") == "at_t"
    assert entity_slug("C++") == "c"


def test_entity_slug_unicode():
    """Test slug generation with unicode names."""
    assert entity_slug("José García") == "jose_garcia"
    assert entity_slug("Müller") == "muller"
    # Chinese characters are transliterated to pinyin by python-slugify
    assert entity_slug("北京") == "bei_jing"


def test_entity_slug_whitespace():
    """Test slug generation handles various whitespace."""
    assert entity_slug("  Spaced  Out  ") == "spaced_out"
    assert entity_slug("Tab\tSeparated") == "tab_separated"
    assert entity_slug("New\nLine") == "new_line"


def test_entity_slug_empty():
    """Test slug generation with empty/blank names."""
    assert entity_slug("") == ""
    assert entity_slug("   ") == ""
    assert entity_slug(None) == ""  # type: ignore


def test_entity_slug_long():
    """Test slug generation with very long names."""
    long_name = "A" * 300
    slug = entity_slug(long_name)
    # Should be truncated with hash suffix
    assert len(slug) <= 200
    assert "_" in slug[-9:]  # Hash suffix pattern


def test_entity_memory_path(fixture_journal, tmp_path, monkeypatch):
    """Test entity memory path generation."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    path = entity_memory_path("personal", "Alice Johnson")
    expected = tmp_path / "facets" / "personal" / "entities" / "alice_johnson"
    assert path == expected


def test_entity_memory_path_empty_name(fixture_journal, tmp_path, monkeypatch):
    """Test entity memory path with empty name raises ValueError."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    with pytest.raises(ValueError, match="slugifies to empty string"):
        entity_memory_path("personal", "")


def test_ensure_entity_memory(fixture_journal, tmp_path, monkeypatch):
    """Test entity memory folder creation."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    folder = ensure_entity_memory("personal", "Bob Smith")
    assert folder.exists()
    assert folder.is_dir()
    assert folder == tmp_path / "facets" / "personal" / "entities" / "bob_smith"


def test_ensure_entity_memory_idempotent(fixture_journal, tmp_path, monkeypatch):
    """Test that ensure_entity_memory is idempotent."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    folder1 = ensure_entity_memory("personal", "Charlie Brown")
    folder2 = ensure_entity_memory("personal", "Charlie Brown")
    assert folder1 == folder2
    assert folder1.exists()


def test_rename_entity_memory(fixture_journal, tmp_path, monkeypatch):
    """Test renaming entity memory folder."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create original folder
    old_folder = ensure_entity_memory("work", "Alice Johnson")
    assert old_folder.exists()

    # Create a file inside to verify contents are moved
    (old_folder / "notes.md").write_text("Test notes")

    # Rename
    result = rename_entity_memory("work", "Alice Johnson", "Alice Smith")
    assert result is True

    # Old folder should not exist
    assert not old_folder.exists()

    # New folder should exist with contents
    new_folder = tmp_path / "facets" / "work" / "entities" / "alice_smith"
    assert new_folder.exists()
    assert (new_folder / "notes.md").read_text() == "Test notes"


def test_rename_entity_memory_not_exists(fixture_journal, tmp_path, monkeypatch):
    """Test renaming non-existent folder returns False."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    result = rename_entity_memory("work", "NonExistent", "NewName")
    assert result is False


def test_rename_entity_memory_same_normalized(fixture_journal, tmp_path, monkeypatch):
    """Test renaming when normalized names are the same."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create folder
    ensure_entity_memory("work", "Alice Johnson")

    # Rename with different casing (normalizes to same)
    result = rename_entity_memory("work", "Alice Johnson", "alice johnson")
    assert result is False  # No rename needed


def test_rename_entity_memory_target_exists(fixture_journal, tmp_path, monkeypatch):
    """Test renaming when target folder already exists raises OSError."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create both folders
    ensure_entity_memory("work", "Alice")
    ensure_entity_memory("work", "Bob")

    # Try to rename Alice to Bob
    with pytest.raises(OSError, match="already exists"):
        rename_entity_memory("work", "Alice", "Bob")


# Tests for find_matching_entity


def test_find_matching_entity_exact_name():
    """Test exact name matching."""
    attached = [
        {"name": "Alice Johnson", "type": "Person"},
        {"name": "Bob Smith", "type": "Person"},
    ]
    result = find_matching_entity("Alice Johnson", attached)
    assert result is not None
    assert result["name"] == "Alice Johnson"


def test_find_matching_entity_exact_aka():
    """Test exact aka matching."""
    attached = [
        {"name": "Robert Johnson", "type": "Person", "aka": ["Bob", "Bobby"]},
    ]
    result = find_matching_entity("Bob", attached)
    assert result is not None
    assert result["name"] == "Robert Johnson"


def test_find_matching_entity_case_insensitive():
    """Test case-insensitive matching."""
    attached = [
        {"name": "Alice Johnson", "type": "Person"},
    ]
    result = find_matching_entity("alice johnson", attached)
    assert result is not None
    assert result["name"] == "Alice Johnson"


def test_find_matching_entity_case_insensitive_aka():
    """Test case-insensitive aka matching."""
    attached = [
        {"name": "Robert Johnson", "type": "Person", "aka": ["Bob"]},
    ]
    result = find_matching_entity("bob", attached)
    assert result is not None
    assert result["name"] == "Robert Johnson"


def test_find_matching_entity_normalized():
    """Test normalized (slugified) matching."""
    attached = [
        {"name": "José García", "type": "Person"},
    ]
    # "Jose Garcia" should match via normalization
    result = find_matching_entity("Jose Garcia", attached)
    assert result is not None
    assert result["name"] == "José García"


def test_find_matching_entity_first_word_unambiguous():
    """Test first-word matching when unambiguous."""
    attached = [
        {"name": "Sarah Chen", "type": "Person"},
        {"name": "Bob Smith", "type": "Person"},
    ]
    # "Sarah" should match "Sarah Chen" (only one Sarah)
    result = find_matching_entity("Sarah", attached)
    assert result is not None
    assert result["name"] == "Sarah Chen"


def test_find_matching_entity_first_word_ambiguous():
    """Test first-word matching skipped when ambiguous."""
    attached = [
        {"name": "John Smith", "type": "Person"},
        {"name": "John Doe", "type": "Person"},
    ]
    # "John" matches multiple entities - should not match
    result = find_matching_entity("John", attached)
    assert result is None


def test_find_matching_entity_first_word_too_short():
    """Test first-word matching requires minimum 3 characters."""
    attached = [
        {"name": "Al Smith", "type": "Person"},
    ]
    # "Al" is too short (< 3 chars)
    result = find_matching_entity("Al", attached)
    assert result is None


def test_find_matching_entity_fuzzy():
    """Test fuzzy matching catches typos."""
    attached = [
        {"name": "Robert Johnson", "type": "Person"},
    ]
    # Typo: "Robet Johnson" should match "Robert Johnson"
    result = find_matching_entity("Robet Johnson", attached)
    assert result is not None
    assert result["name"] == "Robert Johnson"


def test_find_matching_entity_fuzzy_word_order():
    """Test fuzzy matching handles word order differences."""
    attached = [
        {"name": "Sarah Chen", "type": "Person"},
    ]
    # Different word order
    result = find_matching_entity("Chen Sarah", attached)
    assert result is not None
    assert result["name"] == "Sarah Chen"


def test_find_matching_entity_no_match():
    """Test no match returns None."""
    attached = [
        {"name": "Alice Johnson", "type": "Person"},
    ]
    result = find_matching_entity("Charlie Brown", attached)
    assert result is None


def test_find_matching_entity_empty_inputs():
    """Test empty inputs return None."""
    assert find_matching_entity("", []) is None
    assert find_matching_entity("Alice", []) is None
    assert find_matching_entity("", [{"name": "Alice"}]) is None


# Tests for validate_aka_uniqueness


def test_validate_aka_uniqueness_conflicts_with_name():
    """Test aka that matches another entity's name is rejected."""
    entities = [
        {"id": "ctt", "name": "CTT", "type": "Project"},
        {"id": "other_project", "name": "Other Project", "type": "Project"},
    ]
    # Adding "CTT" as aka to "Other Project" should conflict
    result = validate_aka_uniqueness("CTT", entities, exclude_entity_id="other_project")
    assert result == "CTT"


def test_validate_aka_uniqueness_conflicts_with_name_case_insensitive():
    """Test aka collision is case-insensitive."""
    entities = [
        {"id": "ctt", "name": "CTT", "type": "Project"},
        {"id": "other_project", "name": "Other Project", "type": "Project"},
    ]
    # "ctt" should also conflict with "CTT"
    result = validate_aka_uniqueness("ctt", entities, exclude_entity_id="other_project")
    assert result == "CTT"


def test_validate_aka_uniqueness_conflicts_with_aka():
    """Test aka that matches another entity's aka is rejected."""
    entities = [
        {
            "id": "robert_johnson",
            "name": "Robert Johnson",
            "type": "Person",
            "aka": ["Bob", "Bobby"],
        },
        {"id": "other_person", "name": "Other Person", "type": "Person"},
    ]
    # Adding "Bob" as aka to "Other Person" should conflict
    result = validate_aka_uniqueness("Bob", entities, exclude_entity_id="other_person")
    assert result == "Robert Johnson"


def test_validate_aka_uniqueness_own_name_ok():
    """Test adding aka that matches own name is allowed (edge case)."""
    entities = [
        {"id": "ctt", "name": "CTT", "type": "Project"},
        {"id": "other_project", "name": "Other Project", "type": "Project"},
    ]
    # Adding "CTT" as aka to "CTT" itself is ok (exclude_entity_id filters it)
    result = validate_aka_uniqueness("CTT", entities, exclude_entity_id="ctt")
    assert result is None


def test_validate_aka_uniqueness_parenthetical_own_variant_ok_cross_entity_conflicts():
    """Own parenthetical variants are ok, but real cross-entity aliases conflict."""
    entities = [
        {"id": "michael_bauer", "name": "Michael Bauer", "type": "person"},
    ]

    assert (
        validate_aka_uniqueness(
            "Michael Bauer (RadialNexus)",
            entities,
            exclude_entity_id="michael_bauer",
        )
        is None
    )

    entities.append({"id": "radialnexus", "name": "RadialNexus", "type": "company"})
    assert (
        validate_aka_uniqueness(
            "RadialNexus",
            entities,
            exclude_entity_id="michael_bauer",
        )
        == "RadialNexus"
    )


def test_validate_aka_uniqueness_no_conflict():
    """Test unique aka passes validation."""
    entities = [
        {"id": "ctt", "name": "CTT", "type": "Project"},
        {"id": "other_project", "name": "Other Project", "type": "Project"},
    ]
    # Adding "Foo" as aka should be fine
    result = validate_aka_uniqueness("Foo", entities, exclude_entity_id="other_project")
    assert result is None


def test_validate_aka_uniqueness_skips_detached():
    """Test detached entities are not considered for conflicts."""
    entities = [
        {"id": "ctt", "name": "CTT", "type": "Project", "detached": True},
        {"id": "other_project", "name": "Other Project", "type": "Project"},
    ]
    # "CTT" is detached, so adding it as aka should be ok
    result = validate_aka_uniqueness("CTT", entities, exclude_entity_id="other_project")
    assert result is None


# Tests for fuzzy exclusion in load_detected_entities_recent


def test_load_detected_entities_recent_fuzzy_exclusion(
    fixture_journal, tmp_path, monkeypatch
):
    """Test that fuzzy matching excludes detected entities matching attached."""
    facet_path = tmp_path / "facets" / "test_facet"
    entities_dir = facet_path / "entities"
    entities_dir.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create attached entity
    attached = [
        {
            "type": "Person",
            "name": "Robert Johnson",
            "description": "Attached",
            "aka": ["Bob"],
        },
    ]
    save_entities("test_facet", attached)

    # Create detected entities including variations
    detected_entities = [
        {"type": "Person", "name": "Robert Johnson", "description": "Exact match"},
        {"type": "Person", "name": "Bob", "description": "Aka match"},
        {"type": "Person", "name": "robert johnson", "description": "Case insensitive"},
        {
            "type": "Person",
            "name": "Charlie Brown",
            "description": "Should be included",
        },
    ]
    save_entities("test_facet", detected_entities, "20250101")

    # Load detected - only Charlie Brown should be included
    detected = load_detected_entities_recent("test_facet", days=36500)
    names = [e["name"] for e in detected]

    assert "Robert Johnson" not in names  # Exact match excluded
    assert "Bob" not in names  # Aka excluded
    assert "robert johnson" not in names  # Case insensitive excluded
    assert "Charlie Brown" in names  # Not matched, included


def test_load_detected_entities_recent_first_word_exclusion(
    fixture_journal, tmp_path, monkeypatch
):
    """Test that first-word matching excludes detected entities."""
    facet_path = tmp_path / "facets" / "test_facet"
    entities_dir = facet_path / "entities"
    entities_dir.mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create attached entity
    attached = [
        {"type": "Person", "name": "Sarah Chen", "description": "Attached"},
    ]
    save_entities("test_facet", attached)

    # Create detected entities
    detected_entities = [
        {"type": "Person", "name": "Sarah", "description": "First word match"},
        {
            "type": "Person",
            "name": "Charlie Brown",
            "description": "Should be included",
        },
    ]
    save_entities("test_facet", detected_entities, "20250101")

    # Load detected - Sarah should be excluded (first word of Sarah Chen)
    detected = load_detected_entities_recent("test_facet", days=36500)
    names = [e["name"] for e in detected]

    assert "Sarah" not in names  # First word excluded
    assert "Charlie Brown" in names  # Not matched, included


# Tests for entity observations


def test_observations_file_path(fixture_journal, tmp_path, monkeypatch):
    """Test observations file path generation."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    path = observations_file_path("personal", "Alice Johnson")
    expected = (
        tmp_path
        / "facets"
        / "personal"
        / "entities"
        / "alice_johnson"
        / "observations.jsonl"
    )
    assert path == expected


def test_load_observations_empty(fixture_journal, tmp_path, monkeypatch):
    """Test loading observations for entity with no observations."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # No file exists yet
    observations = load_observations("personal", "Alice Johnson")
    assert observations == []


def test_save_and_load_observations(fixture_journal, tmp_path, monkeypatch):
    """Observation readers reflect same-process saves."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Save observations
    test_observations = [
        {
            "content": "Prefers morning meetings",
            "observed_at": 1700000000000,
            "source_day": "20250113",
        },
        {"content": "Expert in Kubernetes", "observed_at": 1700000001000},
    ]
    save_observations("personal", "Alice Johnson", test_observations)

    # Load them back
    loaded = load_observations("personal", "Alice Johnson")
    assert len(loaded) == 2
    assert loaded[0]["content"] == "Prefers morning meetings"
    assert loaded[0]["observed_at"] == 1700000000000
    assert loaded[0]["source_day"] == "20250113"
    assert loaded[1]["content"] == "Expert in Kubernetes"
    assert count_observations("personal", "Alice Johnson") == 2

    updated_observations = [
        {"content": "Prefers afternoon meetings", "observed_at": 1700000002000},
    ]
    save_observations("personal", "Alice Johnson", updated_observations)

    loaded_updated = load_observations("personal", "Alice Johnson")
    assert [obs["content"] for obs in loaded_updated] == ["Prefers afternoon meetings"]
    assert count_observations("personal", "Alice Johnson") == 1


def test_add_observation_success(fixture_journal, tmp_path, monkeypatch):
    """Test adding observations sequentially."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    result = add_observation(
        "personal", "Alice", "Prefers async communication", "20250113"
    )
    assert result["count"] == 1
    assert len(result["observations"]) == 1
    assert result["observations"][0]["content"] == "Prefers async communication"
    assert result["observations"][0]["source_day"] == "20250113"
    assert "observed_at" in result["observations"][0]

    result = add_observation("personal", "Alice", "Works PST timezone")
    assert result["count"] == 2
    assert len(result["observations"]) == 2

    # Verify persistence
    loaded = load_observations("personal", "Alice")
    assert len(loaded) == 2


def test_add_observation_retry_on_error(fixture_journal, tmp_path, monkeypatch):
    """Test that add_observation retries on transient OSError."""
    from unittest.mock import patch

    from solstone.think.journal_io import atomic_replace as _real_atomic_replace

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    call_count = 0

    def flaky_atomic_replace(path, data, *, mode=None):
        nonlocal call_count
        call_count += 1
        if call_count == 1:
            raise PermissionError("Simulated transient error")
        return _real_atomic_replace(path, data, mode=mode)

    with patch(
        "solstone.think.entities.observations.atomic_replace",
        side_effect=flaky_atomic_replace,
    ):
        add_observation("personal", "Alice", "Prefers async communication", "20250113")

    assert call_count == 2
    loaded = load_observations("personal", "Alice")
    assert [obs["content"] for obs in loaded] == ["Prefers async communication"]


def test_add_observation_empty_content(fixture_journal, tmp_path, monkeypatch):
    """Test adding observation with empty content fails."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    with pytest.raises(ValueError, match="cannot be empty"):
        add_observation("personal", "Alice", "")

    with pytest.raises(ValueError, match="cannot be empty"):
        add_observation("personal", "Alice", "   ")


def test_observations_with_entity_rename(fixture_journal, tmp_path, monkeypatch):
    """Test that observations are preserved when entity memory folder is renamed."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create entity memory folder and add observations
    ensure_entity_memory("work", "Alice Johnson")
    add_observation("work", "Alice Johnson", "Test observation")

    # Verify observation exists
    observations = load_observations("work", "Alice Johnson")
    assert len(observations) == 1

    # Rename entity memory folder
    result = rename_entity_memory("work", "Alice Johnson", "Alice Smith")
    assert result is True

    # Old name should have no observations (folder moved)
    old_observations = load_observations("work", "Alice Johnson")
    assert old_observations == []

    # New name should have observations
    new_observations = load_observations("work", "Alice Smith")
    assert len(new_observations) == 1
    assert new_observations[0]["content"] == "Test observation"


def test_observations_atomic_write(fixture_journal, tmp_path, monkeypatch):
    """Test that observations are written atomically."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Save observations
    test_observations = [
        {"content": "Test 1", "observed_at": 1700000000000},
        {"content": "Test 2", "observed_at": 1700000001000},
    ]
    save_observations("personal", "Bob", test_observations)

    # Verify file exists at expected location
    path = observations_file_path("personal", "Bob")
    assert path.exists()

    # Verify JSONL format
    import json

    lines = path.read_text().strip().split("\n")
    assert len(lines) == 2
    for line in lines:
        assert json.loads(line)  # Should not raise


# ============================================================================
# Principal entity tests
# ============================================================================


def test_get_identity_names_from_config(tmp_path, monkeypatch):
    """Test extracting identity names from journal config."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create config with identity
    config_dir = tmp_path / "config"
    config_dir.mkdir()
    config = {
        "identity": {
            "name": "Raylyn Miller",
            "preferred": "Rae",
            "aliases": ["JM", "Raylyn"],
        }
    }
    (config_dir / "journal.json").write_text(json.dumps(config))

    names = get_identity_names()
    # Preferred comes first (best for display), then full name, then aliases
    assert names == ["Rae", "Raylyn Miller", "JM", "Raylyn"]


def test_get_identity_names_no_config(tmp_path, monkeypatch):
    """Test that missing config returns empty list."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    # No config file

    names = get_identity_names()
    assert names == []


def test_get_identity_names_empty_identity(tmp_path, monkeypatch):
    """Test that empty identity config returns empty list."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    config_dir = tmp_path / "config"
    config_dir.mkdir()
    config = {"identity": {"name": "", "preferred": "", "aliases": []}}
    (config_dir / "journal.json").write_text(json.dumps(config))

    names = get_identity_names()
    assert names == []


def test_save_entities_flags_principal_on_name_match(tmp_path, monkeypatch):
    """Test that save_entities flags an entity as principal when it matches identity name."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create config with identity
    config_dir = tmp_path / "config"
    config_dir.mkdir()
    config = {
        "identity": {"name": "Alice Johnson", "preferred": "Alice", "aliases": []}
    }
    (config_dir / "journal.json").write_text(json.dumps(config))

    # Create facet directory
    facet_path = tmp_path / "facets" / "personal"
    facet_path.mkdir(parents=True)

    # Save entities including one matching identity
    entities = [
        {"type": "Person", "name": "Alice Johnson", "description": "Me"},
        {"type": "Person", "name": "Bob Smith", "description": "Friend"},
    ]
    save_entities("personal", entities)

    # Load and verify principal flag
    loaded = load_entities("personal")
    alice = next(e for e in loaded if e.get("name") == "Alice Johnson")
    bob = next(e for e in loaded if e.get("name") == "Bob Smith")

    assert alice.get("is_principal") is True
    assert bob.get("is_principal") is None


def test_save_entities_flags_principal_on_preferred_match(tmp_path, monkeypatch):
    """Test that save_entities flags principal when matching preferred name."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create config with identity - preferred name differs from entity name
    config_dir = tmp_path / "config"
    config_dir.mkdir()
    config = {"identity": {"name": "Raylyn Miller", "preferred": "Rae", "aliases": []}}
    (config_dir / "journal.json").write_text(json.dumps(config))

    # Create facet directory
    facet_path = tmp_path / "facets" / "work"
    facet_path.mkdir(parents=True)

    # Save entity matching preferred name
    entities = [
        {"type": "Person", "name": "Rae", "description": "Me at work"},
    ]
    save_entities("work", entities)

    loaded = load_entities("work")
    rae = loaded[0]
    assert rae.get("is_principal") is True


def test_save_entities_flags_principal_on_alias_match(tmp_path, monkeypatch):
    """Test that save_entities flags principal when matching an alias."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create config with alias
    config_dir = tmp_path / "config"
    config_dir.mkdir()
    config = {"identity": {"name": "Raylyn Miller", "preferred": "", "aliases": ["JM"]}}
    (config_dir / "journal.json").write_text(json.dumps(config))

    # Create facet directory
    facet_path = tmp_path / "facets" / "test"
    facet_path.mkdir(parents=True)

    # Save entity matching alias
    entities = [
        {"type": "Person", "name": "JM", "description": "Initials"},
    ]
    save_entities("test", entities)

    loaded = load_entities("test")
    assert loaded[0].get("is_principal") is True


def test_save_entities_flags_principal_via_entity_aka(tmp_path, monkeypatch):
    """Test that save_entities flags principal when entity aka matches identity."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create config
    config_dir = tmp_path / "config"
    config_dir.mkdir()
    config = {"identity": {"name": "Raylyn Miller", "preferred": "Rae", "aliases": []}}
    (config_dir / "journal.json").write_text(json.dumps(config))

    # Create facet directory
    facet_path = tmp_path / "facets" / "test"
    facet_path.mkdir(parents=True)

    # Save entity where aka matches identity name
    entities = [
        {
            "type": "Person",
            "name": "J. Miller",
            "description": "Me",
            "aka": ["Raylyn Miller", "JM"],
        },
    ]
    save_entities("test", entities)

    loaded = load_entities("test")
    assert loaded[0].get("is_principal") is True


def test_save_entities_preserves_existing_principal(tmp_path, monkeypatch):
    """Test that save_entities doesn't change principal if one already exists."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create config
    config_dir = tmp_path / "config"
    config_dir.mkdir()
    config = {"identity": {"name": "Alice", "preferred": "", "aliases": []}}
    (config_dir / "journal.json").write_text(json.dumps(config))

    # Create facet directory
    facet_path = tmp_path / "facets" / "test"
    facet_path.mkdir(parents=True)

    # Save entities with an existing principal that doesn't match identity
    entities = [
        {
            "type": "Person",
            "name": "Bob",
            "description": "Already principal",
            "is_principal": True,
        },
        {"type": "Person", "name": "Alice", "description": "Matches identity"},
    ]
    save_entities("test", entities)

    loaded = load_entities("test")
    bob = next(e for e in loaded if e.get("name") == "Bob")
    alice = next(e for e in loaded if e.get("name") == "Alice")

    # Bob should still be principal (was already set)
    assert bob.get("is_principal") is True
    # Alice should not be flagged (principal already exists)
    assert alice.get("is_principal") is None


def test_save_entities_no_principal_without_identity(tmp_path, monkeypatch):
    """Test that save_entities doesn't flag principal when no identity configured."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    # No config file

    # Create facet directory
    facet_path = tmp_path / "facets" / "test"
    facet_path.mkdir(parents=True)

    entities = [
        {"type": "Person", "name": "Alice", "description": "Someone"},
    ]
    save_entities("test", entities)

    loaded = load_entities("test")
    assert loaded[0].get("is_principal") is None


def test_save_entities_skips_detached_for_principal(tmp_path, monkeypatch):
    """Test that detached entities are not flagged as principal."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create config
    config_dir = tmp_path / "config"
    config_dir.mkdir()
    config = {"identity": {"name": "Alice", "preferred": "", "aliases": []}}
    (config_dir / "journal.json").write_text(json.dumps(config))

    # Create facet directory
    facet_path = tmp_path / "facets" / "test"
    facet_path.mkdir(parents=True)

    # Save entities with matching name but detached
    entities = [
        {
            "type": "Person",
            "name": "Alice",
            "description": "Detached",
            "detached": True,
        },
        {"type": "Person", "name": "Bob", "description": "Active"},
    ]
    save_entities("test", entities)

    loaded = load_entities("test", include_detached=True)
    alice = next(e for e in loaded if e.get("name") == "Alice")
    bob = next(e for e in loaded if e.get("name") == "Bob")

    # Alice is detached, should not be principal
    assert alice.get("is_principal") is None
    # Bob doesn't match identity, should not be principal
    assert bob.get("is_principal") is None


def test_save_entities_case_insensitive_principal_match(tmp_path, monkeypatch):
    """Test that principal matching is case-insensitive."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create config with lowercase name
    config_dir = tmp_path / "config"
    config_dir.mkdir()
    config = {"identity": {"name": "alice johnson", "preferred": "", "aliases": []}}
    (config_dir / "journal.json").write_text(json.dumps(config))

    # Create facet directory
    facet_path = tmp_path / "facets" / "test"
    facet_path.mkdir(parents=True)

    # Save entity with different case
    entities = [
        {"type": "Person", "name": "Alice Johnson", "description": "Me"},
    ]
    save_entities("test", entities)

    loaded = load_entities("test")
    assert loaded[0].get("is_principal") is True


def test_save_entities_detected_no_principal_flag(tmp_path, monkeypatch):
    """Test that save_entities with day (detected) doesn't flag principal."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create config
    config_dir = tmp_path / "config"
    config_dir.mkdir()
    config = {"identity": {"name": "Alice", "preferred": "", "aliases": []}}
    (config_dir / "journal.json").write_text(json.dumps(config))

    # Create facet entities directory
    entities_dir = tmp_path / "facets" / "test" / "entities"
    entities_dir.mkdir(parents=True)

    # Save detected entities (with day parameter)
    entities = [
        {"type": "Person", "name": "Alice", "description": "Detected"},
    ]
    save_entities("test", entities, day="20250101")

    loaded = load_entities("test", day="20250101")
    # Detected entities should not get is_principal flag
    assert loaded[0].get("is_principal") is None


# ============================================================================
# block_journal_entity tests
# ============================================================================


def test_block_journal_entity_success(tmp_path, monkeypatch):
    """Test blocking a journal entity sets blocked flag and detaches facets."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create journal entity
    entity_dir = tmp_path / "entities" / "alice"
    entity_dir.mkdir(parents=True)
    entity = {"id": "alice", "name": "Alice", "type": "Person"}
    (entity_dir / "entity.json").write_text(json.dumps(entity))

    # Create facet relationship
    facet_dir = tmp_path / "facets" / "work" / "entities" / "alice"
    facet_dir.mkdir(parents=True)
    relationship = {"entity_id": "alice", "description": "Coworker"}
    (facet_dir / "entity.json").write_text(json.dumps(relationship))

    # Block the entity
    result = block_journal_entity("alice")

    assert result["success"] is True
    assert "work" in result["facets_detached"]

    # Verify journal entity is blocked
    loaded = load_journal_entity("alice")
    assert loaded["blocked"] is True

    # Verify facet relationship is detached
    from solstone.think.entities import load_facet_relationship

    rel = load_facet_relationship("work", "alice")
    assert rel["detached"] is True


def test_block_journal_entity_not_found(tmp_path, monkeypatch):
    """Test blocking non-existent entity raises error."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    with pytest.raises(ValueError, match="not found"):
        block_journal_entity("nonexistent")


def test_block_journal_entity_principal_protected(tmp_path, monkeypatch):
    """Test blocking principal entity is rejected."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create principal entity
    entity_dir = tmp_path / "entities" / "myself"
    entity_dir.mkdir(parents=True)
    entity = {"id": "myself", "name": "Me", "type": "Person", "is_principal": True}
    (entity_dir / "entity.json").write_text(json.dumps(entity))

    with pytest.raises(ValueError, match="principal"):
        block_journal_entity("myself")


# ============================================================================
# unblock_journal_entity tests
# ============================================================================


def test_unblock_journal_entity_success(tmp_path, monkeypatch):
    """Test unblocking a blocked journal entity."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create blocked entity
    entity_dir = tmp_path / "entities" / "alice"
    entity_dir.mkdir(parents=True)
    entity = {"id": "alice", "name": "Alice", "type": "Person", "blocked": True}
    (entity_dir / "entity.json").write_text(json.dumps(entity))

    # Unblock
    result = unblock_journal_entity("alice")

    assert result["success"] is True

    # Verify blocked flag is removed
    loaded = load_journal_entity("alice")
    assert "blocked" not in loaded


def test_unblock_journal_entity_not_blocked(tmp_path, monkeypatch):
    """Test unblocking an entity that isn't blocked raises error."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    entity_dir = tmp_path / "entities" / "alice"
    entity_dir.mkdir(parents=True)
    entity = {"id": "alice", "name": "Alice", "type": "Person"}
    (entity_dir / "entity.json").write_text(json.dumps(entity))

    with pytest.raises(ValueError, match="not blocked"):
        unblock_journal_entity("alice")


# ============================================================================
# delete_journal_entity tests
# ============================================================================


def test_delete_journal_entity_success(tmp_path, monkeypatch):
    """Test deleting a journal entity removes all data."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create journal entity with observations
    entity_dir = tmp_path / "entities" / "alice"
    entity_dir.mkdir(parents=True)
    entity = {"id": "alice", "name": "Alice", "type": "Person"}
    (entity_dir / "entity.json").write_text(json.dumps(entity))

    # Create facet relationship with memory
    facet_dir = tmp_path / "facets" / "work" / "entities" / "alice"
    facet_dir.mkdir(parents=True)
    relationship = {"entity_id": "alice", "description": "Coworker"}
    (facet_dir / "entity.json").write_text(json.dumps(relationship))
    (facet_dir / "observations.jsonl").write_text('{"content": "Test"}\n')

    # Delete
    result = delete_journal_entity("alice")

    assert result["success"] is True
    assert "work" in result["facets_deleted"]

    # Verify everything is gone
    assert not entity_dir.exists()
    assert not facet_dir.exists()


def test_delete_journal_entity_principal_protected(tmp_path, monkeypatch):
    """Test deleting principal entity is rejected."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create principal entity
    entity_dir = tmp_path / "entities" / "myself"
    entity_dir.mkdir(parents=True)
    entity = {"id": "myself", "name": "Me", "type": "Person", "is_principal": True}
    (entity_dir / "entity.json").write_text(json.dumps(entity))

    with pytest.raises(ValueError, match="principal"):
        delete_journal_entity("myself")


# ============================================================================
# Blocked entity filtering tests
# ============================================================================


def test_load_entities_excludes_blocked_by_default(tmp_path, monkeypatch):
    """Test that load_entities excludes blocked entities by default."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create journal entities - one normal, one blocked
    normal_dir = tmp_path / "entities" / "alice"
    normal_dir.mkdir(parents=True)
    (normal_dir / "entity.json").write_text(
        json.dumps({"id": "alice", "name": "Alice", "type": "Person"})
    )

    blocked_dir = tmp_path / "entities" / "bob"
    blocked_dir.mkdir(parents=True)
    (blocked_dir / "entity.json").write_text(
        json.dumps({"id": "bob", "name": "Bob", "type": "Person", "blocked": True})
    )

    # Create facet relationships for both
    facet_dir = tmp_path / "facets" / "work"
    (facet_dir / "facet.json").parent.mkdir(parents=True, exist_ok=True)
    (facet_dir / "facet.json").write_text(json.dumps({"title": "Work"}))

    alice_rel = facet_dir / "entities" / "alice"
    alice_rel.mkdir(parents=True)
    (alice_rel / "entity.json").write_text(
        json.dumps({"entity_id": "alice", "description": "Colleague"})
    )

    bob_rel = facet_dir / "entities" / "bob"
    bob_rel.mkdir(parents=True)
    (bob_rel / "entity.json").write_text(
        json.dumps({"entity_id": "bob", "description": "Former colleague"})
    )

    # Load entities - should only get Alice
    entities = load_entities("work")
    assert len(entities) == 1
    assert entities[0]["name"] == "Alice"


def test_load_entities_includes_blocked_when_requested(tmp_path, monkeypatch):
    """Test that load_entities includes blocked entities when include_blocked=True."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create blocked journal entity
    blocked_dir = tmp_path / "entities" / "bob"
    blocked_dir.mkdir(parents=True)
    (blocked_dir / "entity.json").write_text(
        json.dumps({"id": "bob", "name": "Bob", "type": "Person", "blocked": True})
    )

    # Create facet and relationship
    facet_dir = tmp_path / "facets" / "work"
    (facet_dir / "facet.json").parent.mkdir(parents=True, exist_ok=True)
    (facet_dir / "facet.json").write_text(json.dumps({"title": "Work"}))

    bob_rel = facet_dir / "entities" / "bob"
    bob_rel.mkdir(parents=True)
    (bob_rel / "entity.json").write_text(
        json.dumps({"entity_id": "bob", "description": "Former colleague"})
    )

    # Load with include_blocked=True
    entities = load_entities("work", include_blocked=True)
    assert len(entities) == 1
    assert entities[0]["name"] == "Bob"
    assert entities[0].get("blocked") is True


def test_resolve_entity_excludes_blocked_by_default(tmp_path, monkeypatch):
    """Test that resolve_entity doesn't find blocked entities by default."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create blocked journal entity
    blocked_dir = tmp_path / "entities" / "bob"
    blocked_dir.mkdir(parents=True)
    (blocked_dir / "entity.json").write_text(
        json.dumps({"id": "bob", "name": "Bob", "type": "Person", "blocked": True})
    )

    # Create facet and relationship
    facet_dir = tmp_path / "facets" / "work"
    (facet_dir / "facet.json").parent.mkdir(parents=True, exist_ok=True)
    (facet_dir / "facet.json").write_text(json.dumps({"title": "Work"}))

    bob_rel = facet_dir / "entities" / "bob"
    bob_rel.mkdir(parents=True)
    (bob_rel / "entity.json").write_text(
        json.dumps({"entity_id": "bob", "description": "Former colleague"})
    )

    # Try to resolve - should not find Bob
    entity, candidates = resolve_entity("work", "Bob")
    assert entity is None


def test_resolve_entity_finds_blocked_when_requested(tmp_path, monkeypatch):
    """Test that resolve_entity finds blocked entities when include_blocked=True."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create blocked journal entity
    blocked_dir = tmp_path / "entities" / "bob"
    blocked_dir.mkdir(parents=True)
    (blocked_dir / "entity.json").write_text(
        json.dumps({"id": "bob", "name": "Bob", "type": "Person", "blocked": True})
    )

    # Create facet and relationship
    facet_dir = tmp_path / "facets" / "work"
    (facet_dir / "facet.json").parent.mkdir(parents=True, exist_ok=True)
    (facet_dir / "facet.json").write_text(json.dumps({"title": "Work"}))

    bob_rel = facet_dir / "entities" / "bob"
    bob_rel.mkdir(parents=True)
    (bob_rel / "entity.json").write_text(
        json.dumps({"entity_id": "bob", "description": "Former colleague"})
    )

    # Resolve with include_blocked=True
    entity, candidates = resolve_entity("work", "Bob", include_blocked=True)
    assert entity is not None
    assert entity["name"] == "Bob"
    assert entity.get("blocked") is True


def test_load_all_attached_entities_excludes_blocked(tmp_path, monkeypatch):
    """Test that load_all_attached_entities excludes blocked entities."""
    import json

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create journal entities - one normal, one blocked
    normal_dir = tmp_path / "entities" / "alice"
    normal_dir.mkdir(parents=True)
    (normal_dir / "entity.json").write_text(
        json.dumps({"id": "alice", "name": "Alice", "type": "Person"})
    )

    blocked_dir = tmp_path / "entities" / "bob"
    blocked_dir.mkdir(parents=True)
    (blocked_dir / "entity.json").write_text(
        json.dumps({"id": "bob", "name": "Bob", "type": "Person", "blocked": True})
    )

    # Create facet and relationships
    facet_dir = tmp_path / "facets" / "work"
    (facet_dir / "facet.json").parent.mkdir(parents=True, exist_ok=True)
    (facet_dir / "facet.json").write_text(json.dumps({"title": "Work"}))

    alice_rel = facet_dir / "entities" / "alice"
    alice_rel.mkdir(parents=True)
    (alice_rel / "entity.json").write_text(
        json.dumps({"entity_id": "alice", "description": "Colleague"})
    )

    bob_rel = facet_dir / "entities" / "bob"
    bob_rel.mkdir(parents=True)
    (bob_rel / "entity.json").write_text(
        json.dumps({"entity_id": "bob", "description": "Former colleague"})
    )

    # Load all attached entities - should only get Alice
    entities = load_all_attached_entities()
    assert len(entities) == 1
    assert entities[0]["name"] == "Alice"
