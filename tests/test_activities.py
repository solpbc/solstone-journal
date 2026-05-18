# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the activities module and activities agent hooks."""

import json
import tempfile
from pathlib import Path

import pytest


def test_get_default_activities():
    """Test that default activities are returned correctly."""
    from solstone.think.activities import get_default_activities

    defaults = get_default_activities()

    # Should return a list
    assert isinstance(defaults, list)
    assert len(defaults) > 0

    # Each activity should have required fields
    for activity in defaults:
        assert "id" in activity
        assert "name" in activity
        assert "description" in activity

    # Check some known activities exist
    ids = [a["id"] for a in defaults]
    assert "meeting" in ids
    assert "coding" in ids
    assert "browsing" in ids

    # All defaults should have instructions
    for activity in defaults:
        assert "instructions" in activity, f"{activity['id']} missing instructions"
        assert isinstance(activity["instructions"], str)
        assert len(activity["instructions"]) > 0


def test_get_default_activities_returns_copy():
    """Test that get_default_activities returns a copy, not the original."""
    from solstone.think.activities import get_default_activities

    defaults1 = get_default_activities()
    defaults2 = get_default_activities()

    # Should be equal but not the same object
    assert defaults1 == defaults2
    assert defaults1 is not defaults2

    # Modifying one should not affect the other
    defaults1[0]["id"] = "modified"
    assert defaults2[0]["id"] != "modified"


def test_always_on_activities(monkeypatch):
    """Test that always-on activities are auto-included for all facets."""
    from solstone.think.activities import DEFAULT_ACTIVITIES, get_facet_activities

    always_on = [a for a in DEFAULT_ACTIVITIES if a.get("always_on")]
    assert len(always_on) >= 2  # messaging and email

    always_on_ids = {a["id"] for a in always_on}
    assert "messaging" in always_on_ids
    assert "email" in always_on_ids

    with tempfile.TemporaryDirectory() as tmpdir:
        monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

        facet_path = Path(tmpdir) / "facets" / "test_facet"
        facet_path.mkdir(parents=True)

        # Empty facet should still have always-on activities
        activities = get_facet_activities("test_facet")
        activity_ids = {a["id"] for a in activities}
        assert always_on_ids <= activity_ids

        # Explicitly attaching one should not duplicate it
        from solstone.think.activities import add_activity_to_facet

        add_activity_to_facet("test_facet", "messaging")
        activities = get_facet_activities("test_facet")
        messaging_count = sum(1 for a in activities if a["id"] == "messaging")
        assert messaging_count == 1


def test_generate_activity_id():
    """Test activity ID generation from names."""
    from solstone.think.activities import generate_activity_id

    assert generate_activity_id("My Activity") == "my_activity"
    assert generate_activity_id("Research & Development") == "research_development"
    assert generate_activity_id("  Spaces  ") == "spaces"
    assert generate_activity_id("123-Numbers!") == "123_numbers"
    assert generate_activity_id("") == "activity"


def test_facet_activities_empty():
    """Test loading activities from a facet with no activities file.

    With no activities.jsonl, all defaults are returned as the vocabulary.
    """
    from solstone.think.activities import DEFAULT_ACTIVITIES, get_facet_activities

    activities = get_facet_activities("personal")
    assert isinstance(activities, list)

    # Should contain all defaults (full vocabulary for unconfigured facets)
    all_default_ids = {a["id"] for a in DEFAULT_ACTIVITIES}
    assert {a["id"] for a in activities} == all_default_ids


def test_meeting_is_always_on():
    """Test that meeting is marked always_on in DEFAULT_ACTIVITIES."""
    from solstone.think.activities import DEFAULT_ACTIVITIES

    always_on_ids = {a["id"] for a in DEFAULT_ACTIVITIES if a.get("always_on")}
    assert "meeting" in always_on_ids
    assert "email" in always_on_ids
    assert "messaging" in always_on_ids


def test_unconfigured_facet_returns_all_defaults(monkeypatch):
    """Test that a facet with no activities.jsonl gets all 16 defaults."""
    from solstone.think.activities import DEFAULT_ACTIVITIES, get_facet_activities

    with tempfile.TemporaryDirectory() as tmpdir:
        monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

        facet_path = Path(tmpdir) / "facets" / "new_facet"
        facet_path.mkdir(parents=True)

        activities = get_facet_activities("new_facet")
        assert len(activities) == len(DEFAULT_ACTIVITIES)

        activity_ids = {a["id"] for a in activities}
        default_ids = {a["id"] for a in DEFAULT_ACTIVITIES}
        assert activity_ids == default_ids

        # All should be marked as not custom
        for activity in activities:
            assert activity.get("custom") is False


def test_configured_facet_includes_meeting_always_on(monkeypatch):
    """Test that a facet with explicit activities auto-includes meeting."""
    from solstone.think.activities import get_facet_activities, save_facet_activities

    with tempfile.TemporaryDirectory() as tmpdir:
        monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

        facet_path = Path(tmpdir) / "facets" / "work"
        facet_path.mkdir(parents=True)

        # Save only coding — meeting, email, messaging should auto-include
        save_facet_activities("work", [{"id": "coding"}])

        activities = get_facet_activities("work")
        activity_ids = {a["id"] for a in activities}

        assert "coding" in activity_ids
        assert "meeting" in activity_ids
        assert "email" in activity_ids
        assert "messaging" in activity_ids


def test_facet_activities_roundtrip(monkeypatch):
    """Test saving and loading activities."""
    from solstone.think.activities import (
        DEFAULT_ACTIVITIES,
        _get_activities_path,
        get_facet_activities,
        save_facet_activities,
    )

    # Create a temp journal
    with tempfile.TemporaryDirectory() as tmpdir:
        # Temporarily override SOLSTONE_JOURNAL
        monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

        # Create facet directory
        facet_path = Path(tmpdir) / "facets" / "test_facet"
        facet_path.mkdir(parents=True)

        # Save some activities
        activities = [
            {"id": "meeting", "priority": "high"},
            {"id": "coding", "description": "Custom coding description"},
            {
                "id": "browsing",
                "instructions": "Custom browsing instructions for this facet",
            },
            {
                "id": "custom_activity",
                "name": "Custom",
                "description": "A custom activity",
                "instructions": "Custom activity detection hints",
                "custom": True,
            },
        ]
        save_facet_activities("test_facet", activities)

        # Verify file was created
        path = _get_activities_path("test_facet")
        assert path.exists()

        # Load and verify (4 saved + always-on defaults not already saved)
        loaded = get_facet_activities("test_facet")
        loaded_ids = {a["id"] for a in loaded}
        saved_ids = {a["id"] for a in activities}
        always_on_ids = {a["id"] for a in DEFAULT_ACTIVITIES if a.get("always_on")}
        assert loaded_ids == saved_ids | always_on_ids

        # Check meeting (predefined with priority override)
        meeting = next(a for a in loaded if a["id"] == "meeting")
        assert meeting["priority"] == "high"
        assert meeting["custom"] is False
        assert "name" in meeting  # Should have default name
        # Should have default instructions (no override)
        assert "instructions" in meeting
        assert "Levels:" in meeting["instructions"]

        # Check coding (predefined with description override)
        coding = next(a for a in loaded if a["id"] == "coding")
        assert coding["description"] == "Custom coding description"
        # Should keep default instructions (only description overridden)
        assert "instructions" in coding

        # Check browsing (predefined with instructions override)
        browsing = next(a for a in loaded if a["id"] == "browsing")
        assert browsing["instructions"] == "Custom browsing instructions for this facet"

        # Check custom activity with instructions
        custom = next(a for a in loaded if a["id"] == "custom_activity")
        assert custom["custom"] is True
        assert custom["name"] == "Custom"
        assert custom["instructions"] == "Custom activity detection hints"


def test_add_activity_to_facet(monkeypatch):
    """Test adding an activity to a facet."""
    from solstone.think.activities import (
        add_activity_to_facet,
        get_facet_activities,
        remove_activity_from_facet,
    )

    with tempfile.TemporaryDirectory() as tmpdir:
        monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

        facet_path = Path(tmpdir) / "facets" / "test_facet"
        facet_path.mkdir(parents=True)

        # Add a predefined activity
        result = add_activity_to_facet("test_facet", "meeting", priority="high")
        assert result["id"] == "meeting"

        # Verify it was added (+ always-on defaults)
        activities = get_facet_activities("test_facet")
        activity_ids = {a["id"] for a in activities}
        assert "meeting" in activity_ids

        # Adding same activity again should not duplicate
        prev_count = len(activities)
        add_activity_to_facet("test_facet", "meeting")
        activities = get_facet_activities("test_facet")
        assert len(activities) == prev_count

        # Add a predefined activity with custom instructions
        add_activity_to_facet(
            "test_facet",
            "coding",
            instructions="Focus on Python and Rust only",
        )
        coding = next(
            a for a in get_facet_activities("test_facet") if a["id"] == "coding"
        )
        assert coding["instructions"] == "Focus on Python and Rust only"

        # Add a custom activity with instructions
        add_activity_to_facet(
            "test_facet",
            "3d_modeling",
            name="3D Modeling",
            description="Blender and CAD work",
            instructions="Detect via: Blender, FreeCAD, OpenSCAD windows",
        )
        modeling = next(
            a for a in get_facet_activities("test_facet") if a["id"] == "3d_modeling"
        )
        assert (
            modeling["instructions"] == "Detect via: Blender, FreeCAD, OpenSCAD windows"
        )

        # Remove it
        removed = remove_activity_from_facet("test_facet", "meeting")
        assert removed is True

        # Removing non-existent should return False
        removed = remove_activity_from_facet("test_facet", "meeting")
        assert removed is False


def test_update_activity_in_facet(monkeypatch):
    """Test updating an activity in a facet."""
    from solstone.think.activities import (
        add_activity_to_facet,
        get_activity_by_id,
        update_activity_in_facet,
    )

    with tempfile.TemporaryDirectory() as tmpdir:
        monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

        facet_path = Path(tmpdir) / "facets" / "test_facet"
        facet_path.mkdir(parents=True)

        # Add an activity
        add_activity_to_facet("test_facet", "meeting")

        # Update it
        updated = update_activity_in_facet(
            "test_facet", "meeting", priority="low", description="Updated desc"
        )
        assert updated is not None
        assert updated["priority"] == "low"
        assert updated["description"] == "Updated desc"

        # Update instructions
        updated = update_activity_in_facet(
            "test_facet",
            "meeting",
            instructions="Only detect scheduled meetings, not ad-hoc calls",
        )
        assert updated is not None
        assert (
            updated["instructions"]
            == "Only detect scheduled meetings, not ad-hoc calls"
        )
        # Other fields should be preserved
        assert updated["priority"] == "low"

        # Verify via lookup
        activity = get_activity_by_id("test_facet", "meeting")
        assert activity["priority"] == "low"
        assert (
            activity["instructions"]
            == "Only detect scheduled meetings, not ad-hoc calls"
        )

        # Reset instructions to default via empty string
        from solstone.think.activities import DEFAULT_ACTIVITIES

        default_instructions = next(
            a["instructions"] for a in DEFAULT_ACTIVITIES if a["id"] == "meeting"
        )
        updated = update_activity_in_facet("test_facet", "meeting", instructions="")
        assert updated is not None
        assert updated["instructions"] == default_instructions

        # Reset description to default via empty string
        default_desc = next(
            a["description"] for a in DEFAULT_ACTIVITIES if a["id"] == "meeting"
        )
        updated = update_activity_in_facet("test_facet", "meeting", description="")
        assert updated is not None
        assert updated["description"] == default_desc

        # Reset priority to default via "normal"
        updated = update_activity_in_facet("test_facet", "meeting", priority="normal")
        assert updated is not None
        assert updated["priority"] == "normal"

        # Update non-existent should return None
        result = update_activity_in_facet("test_facet", "nonexistent", priority="high")
        assert result is None


def test_format_activities_context_includes_instructions(monkeypatch):
    """Test that format_activities_context renders instructions inline."""
    from solstone.think.activities import save_facet_activities

    with tempfile.TemporaryDirectory() as tmpdir:
        monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

        facet_path = Path(tmpdir) / "facets" / "test_facet"
        facet_path.mkdir(parents=True)

        save_facet_activities(
            "test_facet",
            [
                {"id": "coding"},
                {
                    "id": "custom_task",
                    "name": "Custom",
                    "description": "A custom activity",
                    "instructions": "Detect via: specific app UI",
                    "custom": True,
                },
            ],
        )

        from solstone.talent.activity_state import format_activities_context

        output = format_activities_context("test_facet")

        # Predefined coding should include its default instructions
        assert "**coding**" in output
        assert "IDE or editor open" in output  # from default instructions

        # Custom activity should include its custom instructions
        assert "**custom_task**" in output
        assert "Detect via: specific app UI" in output


# ---------------------------------------------------------------------------
# Activity Records (think/activities.py)
# ---------------------------------------------------------------------------


class TestLevelAvg:
    """Tests for level_avg computation."""

    def test_all_high(self):
        from solstone.think.activities import level_avg

        assert level_avg(["high", "high", "high"]) == 1.0

    def test_all_medium(self):
        from solstone.think.activities import level_avg

        assert level_avg(["medium", "medium"]) == 0.5

    def test_all_low(self):
        from solstone.think.activities import level_avg

        assert level_avg(["low", "low"]) == 0.25

    def test_mixed(self):
        from solstone.think.activities import level_avg

        assert level_avg(["high", "medium"]) == 0.75

    def test_empty_defaults_to_medium(self):
        from solstone.think.activities import level_avg

        assert level_avg([]) == 0.5

    def test_unknown_defaults_to_medium(self):
        from solstone.think.activities import level_avg

        assert level_avg(["unknown", "high"]) == 0.75


class TestActivityRecordIO:
    """Tests for append/load/update of activity records."""

    def test_append_and_load(self, monkeypatch):
        from solstone.think.activities import (
            append_activity_record,
            load_activity_records,
        )

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            record = {
                "id": "coding_100000_300",
                "activity": "coding",
                "segments": ["100000_300", "100500_300"],
                "level_avg": 0.75,
                "description": "Test coding session",
                "active_entities": ["VS Code"],
                "created_at": 1234567890000,
            }

            assert append_activity_record("work", "20260209", record) is True
            records = load_activity_records("work", "20260209")
            assert len(records) == 1
            assert records[0]["id"] == "coding_100000_300"
            assert records[0]["segments"] == ["100000_300", "100500_300"]
            assert records[0]["title"] == "Test coding session"
            assert records[0]["details"] == ""
            assert records[0]["hidden"] is False
            assert records[0]["edits"] == []

    def test_append_idempotent(self, monkeypatch):
        from solstone.think.activities import (
            append_activity_record,
            load_activity_records,
        )

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            record = {
                "id": "coding_100000_300",
                "activity": "coding",
                "segments": ["100000_300"],
                "created_at": 1234567890000,
            }

            assert append_activity_record("work", "20260209", record) is True
            assert append_activity_record("work", "20260209", record) is False

            records = load_activity_records("work", "20260209")
            assert len(records) == 1

    def test_load_nonexistent_returns_empty(self, monkeypatch):
        from solstone.think.activities import load_activity_records

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)
            assert load_activity_records("work", "20260209") == []

    def test_update_description(self, monkeypatch):
        from solstone.think.activities import (
            append_activity_record,
            load_activity_records,
            update_record_description,
        )

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            record = {
                "id": "coding_100000_300",
                "activity": "coding",
                "description": "Original description",
                "segments": ["100000_300"],
                "created_at": 1234567890000,
            }

            append_activity_record("work", "20260209", record)
            result = update_record_description(
                "work", "20260209", "coding_100000_300", "Updated description"
            )
            assert result is True

            records = load_activity_records("work", "20260209")
            assert records[0]["description"] == "Updated description"
            assert records[0]["title"] == "Updated description"
            assert records[0]["details"] == ""

    def test_update_description_with_title_and_details(self, monkeypatch):
        from solstone.think.activities import (
            append_activity_record,
            load_activity_records,
            update_record_description,
        )

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            record = {
                "id": "coding_100000_300",
                "activity": "coding",
                "description": "Original description",
                "segments": ["100000_300"],
                "created_at": 1234567890000,
            }

            append_activity_record("work", "20260209", record)
            result = update_record_description(
                "work",
                "20260209",
                "coding_100000_300",
                "Updated description",
                title="Focused coding",
                details="Pairing with Alex on tests.",
            )

            assert result is True
            records = load_activity_records("work", "20260209")
            assert records[0]["description"] == "Updated description"
            assert records[0]["title"] == "Focused coding"
            assert records[0]["details"] == "Pairing with Alex on tests."

    def test_update_description_none_title_and_details_only_updates_description(
        self, monkeypatch
    ):
        from solstone.think.activities import (
            append_activity_record,
            load_activity_records,
            update_record_description,
        )

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            append_activity_record(
                "work",
                "20260209",
                {
                    "id": "coding_100000_300",
                    "activity": "coding",
                    "title": "Existing title",
                    "details": "Existing details",
                    "description": "Original description",
                    "segments": ["100000_300"],
                    "created_at": 1234567890000,
                },
            )

            assert (
                update_record_description(
                    "work",
                    "20260209",
                    "coding_100000_300",
                    "Updated description",
                    title=None,
                    details=None,
                )
                is True
            )

            records = load_activity_records("work", "20260209")
            assert records[0]["description"] == "Updated description"
            assert records[0]["title"] == "Existing title"
            assert records[0]["details"] == "Existing details"

    def test_update_nonexistent_returns_false(self, monkeypatch):
        from solstone.think.activities import update_record_description

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)
            assert (
                update_record_description("work", "20260209", "nonexistent", "desc")
                is False
            )

    def test_update_preserves_other_records(self, monkeypatch):
        from solstone.think.activities import (
            append_activity_record,
            load_activity_records,
            update_record_description,
        )

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            r1 = {
                "id": "coding_100000_300",
                "activity": "coding",
                "description": "First",
                "segments": ["100000_300"],
                "created_at": 1,
            }
            r2 = {
                "id": "meeting_110000_300",
                "activity": "meeting",
                "description": "Second",
                "segments": ["110000_300"],
                "created_at": 2,
            }

            append_activity_record("work", "20260209", r1)
            append_activity_record("work", "20260209", r2)

            update_record_description(
                "work", "20260209", "coding_100000_300", "Updated first"
            )

            records = load_activity_records("work", "20260209")
            assert len(records) == 2
            assert records[0]["description"] == "Updated first"
            assert records[1]["description"] == "Second"

    def test_update_activity_record_appends_edit(self, monkeypatch):
        from solstone.think.activities import (
            append_activity_record,
            load_activity_records,
            update_activity_record,
        )

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            append_activity_record(
                "work",
                "20260209",
                {
                    "id": "coding_100000_300",
                    "activity": "coding",
                    "description": "Original description",
                    "segments": ["100000_300"],
                    "created_at": 1234567890000,
                },
            )

            updated = update_activity_record(
                "work",
                "20260209",
                "coding_100000_300",
                {"title": "Focused coding", "details": "Updated details"},
                actor="cli:update",
                note="updated fields: details, title",
            )

            assert updated is not None
            assert updated["title"] == "Focused coding"
            assert updated["details"] == "Updated details"
            assert updated["edits"][-1]["actor"] == "cli:update"
            assert updated["edits"][-1]["fields"] == ["title", "details"]

            records = load_activity_records("work", "20260209")
            assert records[0]["edits"][-1]["note"] == "updated fields: details, title"

    def test_update_activity_record_validates_patch(self, monkeypatch):
        from solstone.think.activities import update_activity_record

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            with pytest.raises(ValueError, match="patch cannot be empty"):
                update_activity_record(
                    "work",
                    "20260209",
                    "coding_100000_300",
                    {},
                    actor="cli:update",
                    note="no-op",
                )

            with pytest.raises(ValueError, match="disallowed fields"):
                update_activity_record(
                    "work",
                    "20260209",
                    "coding_100000_300",
                    {"activity": "meeting"},
                    actor="cli:update",
                    note="bad field",
                )

    def test_format_activities_renders_story(self):
        from solstone.think.activities import format_activities

        chunks, _meta = format_activities(
            [
                {
                    "id": "meeting_090000_300",
                    "activity": "meeting",
                    "description": "Team sync",
                    "segments": ["090000_300"],
                    "created_at": 1,
                    "participation": [{"name": "Mina"}],
                    "story": {
                        "body": "Aligned on the launch plan and assigned owners.",
                        "topics": ["launch", "owners"],
                        "confidence": 0.9,
                    },
                },
                {
                    "id": "coding_100000_300",
                    "activity": "coding",
                    "description": "Implementation block",
                    "segments": ["100000_300"],
                    "created_at": 2,
                },
            ]
        )

        assert (
            "Aligned on the launch plan and assigned owners." in chunks[0]["markdown"]
        )
        assert "Topics: launch, owners" in chunks[0]["markdown"]
        assert "Topics:" not in chunks[1]["markdown"]
        assert (
            "Aligned on the launch plan and assigned owners."
            not in chunks[1]["markdown"]
        )

    def test_hidden_records_filtered_by_default(self, monkeypatch):
        from solstone.think.activities import (
            append_activity_record,
            load_activity_records,
            mute_activity_record,
            unmute_activity_record,
        )

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            append_activity_record(
                "work",
                "20260209",
                {
                    "id": "coding_100000_300",
                    "activity": "coding",
                    "description": "Original description",
                    "segments": ["100000_300"],
                    "created_at": 1234567890000,
                },
            )

            muted = mute_activity_record(
                "work",
                "20260209",
                "coding_100000_300",
                actor="cli:mute",
                reason="too noisy",
            )
            assert muted is not None
            assert muted["hidden"] is True
            assert muted["edits"][-1]["note"] == "too noisy"

            assert load_activity_records("work", "20260209") == []
            hidden_records = load_activity_records(
                "work", "20260209", include_hidden=True
            )
            assert len(hidden_records) == 1
            assert hidden_records[0]["hidden"] is True

            hidden_count = len(hidden_records[0]["edits"])
            muted_again = mute_activity_record(
                "work",
                "20260209",
                "coding_100000_300",
                actor="cli:mute",
                reason="still noisy",
            )
            assert muted_again is not None
            assert len(muted_again["edits"]) == hidden_count

            unmuted = unmute_activity_record(
                "work",
                "20260209",
                "coding_100000_300",
                actor="cli:unmute",
                reason=None,
            )
            assert unmuted is not None
            assert unmuted["hidden"] is False
            assert unmuted["edits"][-1]["note"] == "unmuted"
            assert len(load_activity_records("work", "20260209")) == 1


# ---------------------------------------------------------------------------
# Activities Agent Hooks (talent/activities.py)
# ---------------------------------------------------------------------------


def _setup_segment(tmpdir, day, segment, facet, state):
    """Helper to create an activity_state.json file in a segment."""
    talents_dir = (
        Path(tmpdir) / "chronicle" / day / "default" / segment / "talents" / facet
    )
    talents_dir.mkdir(parents=True, exist_ok=True)
    state_file = talents_dir / "activity_state.json"
    state_file.write_text(json.dumps(state))


class TestMakeActivityId:
    def test_basic(self):
        from solstone.think.activities import make_activity_id

        assert make_activity_id("coding", "095809_303") == "coding_095809_303"

    def test_with_custom_type(self):
        from solstone.think.activities import make_activity_id

        assert (
            make_activity_id("video_editing", "120000_300")
            == "video_editing_120000_300"
        )


class TestListFacetsWithActivityState:
    def test_finds_facets(self, monkeypatch):
        from solstone.talent.activities import _list_facets_with_activity_state

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            _setup_segment(tmpdir, "20260209", "100000_300", "personal", [])
            _setup_segment(tmpdir, "20260209", "100000_300", "work", [])

            facets = _list_facets_with_activity_state(
                "20260209", "100000_300", stream="default"
            )
            assert facets == ["personal", "work"]

    def test_returns_empty_for_nonexistent(self, monkeypatch):
        from solstone.talent.activities import _list_facets_with_activity_state

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)
            assert (
                _list_facets_with_activity_state(
                    "20260209", "100000_300", stream="default"
                )
                == []
            )


class TestDetectEndedActivities:
    def test_explicit_ended(self):
        from solstone.talent.activities import _detect_ended_activities

        prev = [
            {"activity": "coding", "state": "active", "since": "100000_300"},
        ]
        curr = [
            {"activity": "coding", "state": "ended", "since": "100000_300"},
        ]
        ended = _detect_ended_activities(prev, curr, timed_out=False)
        assert len(ended) == 1
        assert ended[0]["activity"] == "coding"

    def test_implicit_ended(self):
        from solstone.talent.activities import _detect_ended_activities

        prev = [
            {"activity": "coding", "state": "active", "since": "100000_300"},
            {"activity": "meeting", "state": "active", "since": "100000_300"},
        ]
        curr = [
            {"activity": "meeting", "state": "active", "since": "100000_300"},
        ]
        ended = _detect_ended_activities(prev, curr, timed_out=False)
        assert len(ended) == 1
        assert ended[0]["activity"] == "coding"

    def test_timeout_ends_all(self):
        from solstone.talent.activities import _detect_ended_activities

        prev = [
            {"activity": "coding", "state": "active", "since": "100000_300"},
            {"activity": "meeting", "state": "active", "since": "100000_300"},
        ]
        ended = _detect_ended_activities(prev, [], timed_out=True)
        assert len(ended) == 2

    def test_continuing_not_ended(self):
        from solstone.talent.activities import _detect_ended_activities

        prev = [
            {"activity": "coding", "state": "active", "since": "100000_300"},
        ]
        curr = [
            {"activity": "coding", "state": "active", "since": "100000_300"},
        ]
        ended = _detect_ended_activities(prev, curr, timed_out=False)
        assert len(ended) == 0

    def test_ignores_previously_ended(self):
        from solstone.talent.activities import _detect_ended_activities

        prev = [
            {"activity": "coding", "state": "ended", "since": "090000_300"},
            {"activity": "meeting", "state": "active", "since": "100000_300"},
        ]
        curr = []
        ended = _detect_ended_activities(prev, curr, timed_out=False)
        assert len(ended) == 1
        assert ended[0]["activity"] == "meeting"

    def test_new_activity_same_type(self):
        """A new activity of same type with different since is not the same."""
        from solstone.talent.activities import _detect_ended_activities

        prev = [
            {"activity": "coding", "state": "active", "since": "100000_300"},
        ]
        curr = [
            {"activity": "coding", "state": "active", "since": "110000_300"},
        ]
        ended = _detect_ended_activities(prev, curr, timed_out=False)
        assert len(ended) == 1
        assert ended[0]["since"] == "100000_300"


class TestWalkActivitySegments:
    def test_walks_segments(self, monkeypatch):
        from solstone.talent.activities import _walk_activity_segments

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            _setup_segment(
                tmpdir,
                "20260209",
                "100000_300",
                "work",
                [
                    {
                        "activity": "coding",
                        "state": "active",
                        "since": "100000_300",
                        "description": "Starting work",
                        "level": "high",
                        "active_entities": ["VS Code"],
                    }
                ],
            )
            _setup_segment(
                tmpdir,
                "20260209",
                "100500_300",
                "work",
                [
                    {
                        "activity": "coding",
                        "state": "active",
                        "since": "100000_300",
                        "description": "Continuing work",
                        "level": "medium",
                        "active_entities": ["VS Code", "Claude Code"],
                    }
                ],
            )

            result = _walk_activity_segments(
                "20260209", "work", "coding", "100000_300", "100500_300"
            )

            assert result["segments"] == ["100000_300", "100500_300"]
            assert len(result["descriptions"]) == 2
            assert result["levels"] == ["high", "medium"]
            assert result["active_entities"] == ["VS Code", "Claude Code"]

    def test_deduplicates_entities(self, monkeypatch):
        from solstone.talent.activities import _walk_activity_segments

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            _setup_segment(
                tmpdir,
                "20260209",
                "100000_300",
                "work",
                [
                    {
                        "activity": "coding",
                        "state": "active",
                        "since": "100000_300",
                        "level": "high",
                        "active_entities": ["VS Code", "Git"],
                    }
                ],
            )
            _setup_segment(
                tmpdir,
                "20260209",
                "100500_300",
                "work",
                [
                    {
                        "activity": "coding",
                        "state": "active",
                        "since": "100000_300",
                        "level": "high",
                        "active_entities": ["VS Code", "Claude Code"],
                    }
                ],
            )

            result = _walk_activity_segments(
                "20260209", "work", "coding", "100000_300", "100500_300"
            )

            assert result["active_entities"] == ["VS Code", "Git", "Claude Code"]

    def test_empty_when_no_match(self, monkeypatch):
        from solstone.talent.activities import _walk_activity_segments

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)
            (Path(tmpdir) / "chronicle" / "20260209").mkdir(parents=True)

            result = _walk_activity_segments(
                "20260209", "work", "coding", "100000_300", "100500_300"
            )
            assert result["segments"] == []


class TestPreProcess:
    """Tests for the activities pre_process hook."""

    def test_skips_when_no_previous_segment(self, monkeypatch):
        from solstone.talent.activities import pre_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            day_dir = Path(tmpdir) / "chronicle" / "20260209"
            day_dir.mkdir(parents=True)
            (day_dir / "default" / "100000_300").mkdir(parents=True)

            result = pre_process(
                {"day": "20260209", "segment": "100000_300", "stream": "default"}
            )
            assert result is not None
            assert "skip_reason" in result

    def test_skips_when_no_ended_activities(self, monkeypatch):
        from solstone.talent.activities import pre_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            _setup_segment(
                tmpdir,
                "20260209",
                "100000_300",
                "work",
                [
                    {
                        "activity": "coding",
                        "state": "active",
                        "since": "100000_300",
                        "level": "high",
                    }
                ],
            )
            _setup_segment(
                tmpdir,
                "20260209",
                "100500_300",
                "work",
                [
                    {
                        "activity": "coding",
                        "state": "active",
                        "since": "100000_300",
                        "level": "high",
                    }
                ],
            )

            result = pre_process(
                {"day": "20260209", "segment": "100500_300", "stream": "default"}
            )
            assert result is not None
            assert result.get("skip_reason") == "no_ended_activities"

    def test_detects_ended_and_writes_record(self, monkeypatch):
        from solstone.talent.activities import pre_process
        from solstone.think.activities import load_activity_records

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            _setup_segment(
                tmpdir,
                "20260209",
                "100000_300",
                "work",
                [
                    {
                        "activity": "coding",
                        "state": "active",
                        "since": "100000_300",
                        "description": "Starting work",
                        "level": "high",
                        "active_entities": ["VS Code"],
                    }
                ],
            )
            _setup_segment(tmpdir, "20260209", "100500_300", "work", [])

            result = pre_process(
                {"day": "20260209", "segment": "100500_300", "stream": "default"}
            )

            assert "skip_reason" not in result
            assert "transcript" in result
            assert "coding_100000_300" in result["transcript"]

            records = load_activity_records("work", "20260209")
            assert len(records) == 1
            assert records[0]["id"] == "coding_100000_300"
            assert records[0]["segments"] == ["100000_300"]

    def test_idempotent_on_rerun(self, monkeypatch):
        from solstone.talent.activities import pre_process
        from solstone.think.activities import load_activity_records

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            _setup_segment(
                tmpdir,
                "20260209",
                "100000_300",
                "work",
                [
                    {
                        "activity": "coding",
                        "state": "active",
                        "since": "100000_300",
                        "description": "Coding",
                        "level": "high",
                    }
                ],
            )
            _setup_segment(tmpdir, "20260209", "100500_300", "work", [])

            context = {"day": "20260209", "segment": "100500_300", "stream": "default"}
            pre_process(context)
            pre_process(context)

            records = load_activity_records("work", "20260209")
            assert len(records) == 1

    def test_multi_facet_detection(self, monkeypatch):
        from solstone.talent.activities import pre_process
        from solstone.think.activities import load_activity_records

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            _setup_segment(
                tmpdir,
                "20260209",
                "100000_300",
                "work",
                [
                    {
                        "activity": "coding",
                        "state": "active",
                        "since": "100000_300",
                        "description": "Work coding",
                        "level": "high",
                    }
                ],
            )
            _setup_segment(
                tmpdir,
                "20260209",
                "100000_300",
                "personal",
                [
                    {
                        "activity": "meeting",
                        "state": "active",
                        "since": "100000_300",
                        "description": "Team standup",
                        "level": "medium",
                    }
                ],
            )

            _setup_segment(tmpdir, "20260209", "100500_300", "work", [])
            _setup_segment(tmpdir, "20260209", "100500_300", "personal", [])

            result = pre_process(
                {"day": "20260209", "segment": "100500_300", "stream": "default"}
            )

            assert "skip_reason" not in result
            assert "#work" in result["transcript"]
            assert "#personal" in result["transcript"]

            work_records = load_activity_records("work", "20260209")
            personal_records = load_activity_records("personal", "20260209")
            assert len(work_records) == 1
            assert len(personal_records) == 1

    def test_multi_segment_span(self, monkeypatch):
        """Activity spanning multiple segments should collect all segments."""
        from solstone.talent.activities import pre_process
        from solstone.think.activities import load_activity_records

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            _setup_segment(
                tmpdir,
                "20260209",
                "100000_300",
                "work",
                [
                    {
                        "activity": "coding",
                        "state": "active",
                        "since": "100000_300",
                        "description": "Starting",
                        "level": "high",
                        "active_entities": ["VS Code"],
                    }
                ],
            )
            _setup_segment(
                tmpdir,
                "20260209",
                "100500_300",
                "work",
                [
                    {
                        "activity": "coding",
                        "state": "active",
                        "since": "100000_300",
                        "description": "Continuing",
                        "level": "medium",
                        "active_entities": ["VS Code", "Git"],
                    }
                ],
            )
            _setup_segment(
                tmpdir,
                "20260209",
                "101000_300",
                "work",
                [
                    {
                        "activity": "coding",
                        "state": "active",
                        "since": "100000_300",
                        "description": "Finishing",
                        "level": "high",
                        "active_entities": ["Claude Code"],
                    }
                ],
            )
            # Coding ends
            _setup_segment(tmpdir, "20260209", "101500_300", "work", [])

            pre_process(
                {"day": "20260209", "segment": "101500_300", "stream": "default"}
            )

            records = load_activity_records("work", "20260209")
            assert len(records) == 1
            r = records[0]
            assert r["segments"] == ["100000_300", "100500_300", "101000_300"]
            assert r["active_entities"] == ["VS Code", "Git", "Claude Code"]
            assert r["level_avg"] == 0.83  # (1.0 + 0.5 + 1.0) / 3


class TestPostProcess:
    """Tests for the activities post_process hook."""

    def test_updates_descriptions(self, monkeypatch):
        from solstone.talent.activities import post_process
        from solstone.think.activities import (
            append_activity_record,
            load_activity_records,
        )

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            record = {
                "id": "coding_100000_300",
                "activity": "coding",
                "description": "Preliminary description",
                "segments": ["100000_300"],
                "created_at": 1,
            }
            append_activity_record("work", "20260209", record)

            llm_result = json.dumps(
                {
                    "work": [
                        {
                            "id": "coding_100000_300",
                            "title": "Coding summary",
                            "details": "Worked through test failures and cleanup.",
                            "description": "Synthesized full description of coding session",
                        }
                    ]
                }
            )

            post_process(llm_result, {"day": "20260209"})

            records = load_activity_records("work", "20260209")
            assert (
                records[0]["description"]
                == "Synthesized full description of coding session"
            )
            assert records[0]["title"] == "Coding summary"
            assert records[0]["details"] == "Worked through test failures and cleanup."

    def test_updates_descriptions_without_optional_fields(self, monkeypatch):
        from solstone.talent.activities import post_process
        from solstone.think.activities import (
            append_activity_record,
            load_activity_records,
        )

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            append_activity_record(
                "work",
                "20260209",
                {
                    "id": "coding_100000_300",
                    "activity": "coding",
                    "title": "Existing title",
                    "details": "Existing details",
                    "description": "Preliminary description",
                    "segments": ["100000_300"],
                    "created_at": 1,
                },
            )

            llm_result = json.dumps(
                {
                    "work": [
                        {
                            "id": "coding_100000_300",
                            "description": "Only description changed",
                        }
                    ]
                }
            )

            post_process(llm_result, {"day": "20260209"})

            records = load_activity_records("work", "20260209")
            assert records[0]["description"] == "Only description changed"
            assert records[0]["title"] == "Existing title"
            assert records[0]["details"] == "Existing details"

    def test_handles_invalid_json(self):
        from solstone.talent.activities import post_process

        result = post_process("not json", {"day": "20260209"})
        assert result is None

    def test_handles_non_object(self):
        from solstone.talent.activities import post_process

        result = post_process("[]", {"day": "20260209"})
        assert result is None

    def test_returns_none(self, monkeypatch):
        from solstone.talent.activities import post_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            result = post_process("{}", {"day": "20260209"})
            assert result is None


class TestEstimateDurationMinutes:
    def test_single_segment(self):
        from solstone.think.activities import estimate_duration_minutes

        assert estimate_duration_minutes(["100000_300"]) == 5

    def test_multiple_segments(self):
        from solstone.think.activities import estimate_duration_minutes

        assert estimate_duration_minutes(["100000_300", "100500_300"]) == 10

    def test_empty_returns_1(self):
        from solstone.think.activities import estimate_duration_minutes

        assert estimate_duration_minutes([]) == 1


class TestPreProcessMeta:
    """Tests for pre-hook stashing record data in meta."""

    def test_meta_contains_activity_records(self, monkeypatch):
        from solstone.talent.activities import pre_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            _setup_segment(
                tmpdir,
                "20260209",
                "100000_300",
                "work",
                [
                    {
                        "activity": "coding",
                        "state": "active",
                        "since": "100000_300",
                        "level": "high",
                        "description": "Writing code",
                        "active_entities": ["VS Code"],
                    }
                ],
            )
            _setup_segment(tmpdir, "20260209", "100500_300", "work", [])

            result = pre_process(
                {
                    "day": "20260209",
                    "segment": "100500_300",
                    "stream": "default",
                    "meta": {},
                }
            )

            assert "meta" in result
            records = result["meta"]["activity_records"]
            assert "work" in records
            assert "coding_100000_300" in records["work"]
            rec = records["work"]["coding_100000_300"]
            assert rec["activity"] == "coding"
            assert rec["segments"] == ["100000_300"]
            assert rec["level_avg"] == 1.0
            assert rec["active_entities"] == ["VS Code"]
            assert rec["description"] == "Writing code"

    def test_meta_multiple_facets(self, monkeypatch):
        from solstone.talent.activities import pre_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            _setup_segment(
                tmpdir,
                "20260209",
                "100000_300",
                "work",
                [
                    {
                        "activity": "coding",
                        "state": "active",
                        "since": "100000_300",
                        "level": "high",
                    }
                ],
            )
            _setup_segment(
                tmpdir,
                "20260209",
                "100000_300",
                "personal",
                [
                    {
                        "activity": "browsing",
                        "state": "active",
                        "since": "100000_300",
                        "level": "low",
                    }
                ],
            )
            _setup_segment(tmpdir, "20260209", "100500_300", "work", [])
            _setup_segment(tmpdir, "20260209", "100500_300", "personal", [])

            result = pre_process(
                {"day": "20260209", "segment": "100500_300", "stream": "default"}
            )

            records = result["meta"]["activity_records"]
            assert "work" in records
            assert "personal" in records


class TestPostProcessEvents:
    """Tests for post-hook callosum event emission."""

    def test_emits_events_with_llm_description(self, monkeypatch):
        from unittest.mock import patch

        from solstone.talent.activities import post_process
        from solstone.think.activities import append_activity_record

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            record = {
                "id": "coding_100000_300",
                "activity": "coding",
                "description": "Preliminary",
                "segments": ["100000_300"],
                "created_at": 1,
            }
            append_activity_record("work", "20260209", record)

            llm_result = json.dumps(
                {
                    "work": [
                        {
                            "id": "coding_100000_300",
                            "description": "Full synthesized description",
                        }
                    ]
                }
            )

            meta = {
                "activity_records": {
                    "work": {
                        "coding_100000_300": {
                            "activity": "coding",
                            "segments": ["100000_300"],
                            "description": "Preliminary",
                            "level_avg": 1.0,
                            "active_entities": ["VS Code"],
                        }
                    }
                }
            }

            with patch("solstone.talent.activities.callosum_send") as mock_send:
                mock_send.return_value = True
                post_process(
                    llm_result,
                    {"day": "20260209", "segment": "100500_300", "meta": meta},
                )

                mock_send.assert_called_once_with(
                    "activity",
                    "recorded",
                    facet="work",
                    day="20260209",
                    segment="100500_300",
                    id="coding_100000_300",
                    activity="coding",
                    segments=["100000_300"],
                    level_avg=1.0,
                    description="Full synthesized description",
                    active_entities=["VS Code"],
                )

    def test_falls_back_to_prehook_description_with_warning(self, monkeypatch, caplog):
        from unittest.mock import patch

        from solstone.talent.activities import post_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            meta = {
                "activity_records": {
                    "work": {
                        "coding_100000_300": {
                            "activity": "coding",
                            "segments": ["100000_300"],
                            "description": "Pre-hook fallback desc",
                            "level_avg": 0.5,
                            "active_entities": [],
                        }
                    }
                }
            }

            # LLM returns empty — no descriptions to update
            with patch("solstone.talent.activities.callosum_send") as mock_send:
                mock_send.return_value = True
                import logging

                with caplog.at_level(
                    logging.WARNING, logger="solstone.talent.activities"
                ):
                    post_process(
                        "{}",
                        {"day": "20260209", "segment": "100500_300", "meta": meta},
                    )

                mock_send.assert_called_once()
                call_kwargs = mock_send.call_args[1]
                assert call_kwargs["description"] == "Pre-hook fallback desc"
                assert "No LLM description" in caplog.text

    def test_no_events_without_meta(self, monkeypatch):
        from unittest.mock import patch

        from solstone.talent.activities import post_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            with patch("solstone.talent.activities.callosum_send") as mock_send:
                post_process("{}", {"day": "20260209", "segment": "100500_300"})
                mock_send.assert_not_called()

    def test_event_emission_failure_does_not_raise(self, monkeypatch):
        from unittest.mock import patch

        from solstone.talent.activities import post_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            meta = {
                "activity_records": {
                    "work": {
                        "coding_100000_300": {
                            "activity": "coding",
                            "segments": ["100000_300"],
                            "description": "desc",
                            "level_avg": 0.5,
                            "active_entities": [],
                        }
                    }
                }
            }

            with patch("solstone.talent.activities.callosum_send") as mock_send:
                mock_send.side_effect = OSError("socket error")
                # Should not raise
                result = post_process(
                    "{}",
                    {"day": "20260209", "segment": "100500_300", "meta": meta},
                )
                assert result is None


class TestHandleActivityRecorded:
    """Tests for supervisor's _handle_activity_recorded handler."""

    def test_queues_think_task(self):
        from unittest.mock import MagicMock, patch

        from solstone.think.supervisor import _handle_activity_recorded

        mock_queue = MagicMock()
        with patch("solstone.think.supervisor._task_queue", mock_queue):
            _handle_activity_recorded(
                {
                    "tract": "activity",
                    "event": "recorded",
                    "id": "coding_100000_300",
                    "facet": "work",
                    "day": "20260209",
                }
            )

            mock_queue.submit.assert_called_once_with(
                [
                    "sol",
                    "think",
                    "--activity",
                    "coding_100000_300",
                    "--facet",
                    "work",
                    "--day",
                    "20260209",
                ],
                day="20260209",
            )

    def test_ignores_wrong_tract(self):
        from unittest.mock import MagicMock, patch

        from solstone.think.supervisor import _handle_activity_recorded

        mock_queue = MagicMock()
        with patch("solstone.think.supervisor._task_queue", mock_queue):
            _handle_activity_recorded(
                {
                    "tract": "think",
                    "event": "recorded",
                    "id": "x",
                    "facet": "w",
                    "day": "d",
                }
            )
            mock_queue.submit.assert_not_called()

    def test_ignores_wrong_event(self):
        from unittest.mock import MagicMock, patch

        from solstone.think.supervisor import _handle_activity_recorded

        mock_queue = MagicMock()
        with patch("solstone.think.supervisor._task_queue", mock_queue):
            _handle_activity_recorded(
                {
                    "tract": "activity",
                    "event": "other",
                    "id": "x",
                    "facet": "w",
                    "day": "d",
                }
            )
            mock_queue.submit.assert_not_called()

    def test_warns_on_missing_fields(self, caplog):
        from unittest.mock import MagicMock, patch

        from solstone.think.supervisor import _handle_activity_recorded

        mock_queue = MagicMock()
        import logging

        with patch("solstone.think.supervisor._task_queue", mock_queue):
            with caplog.at_level(logging.WARNING):
                _handle_activity_recorded(
                    {"tract": "activity", "event": "recorded", "id": "x"}
                )
            mock_queue.submit.assert_not_called()

    def test_warns_when_no_task_queue(self, caplog):
        import logging
        from unittest.mock import patch

        from solstone.think.supervisor import _handle_activity_recorded

        with patch("solstone.think.supervisor._task_queue", None):
            with caplog.at_level(logging.WARNING):
                _handle_activity_recorded(
                    {
                        "tract": "activity",
                        "event": "recorded",
                        "id": "coding_100000_300",
                        "facet": "work",
                        "day": "20260209",
                    }
                )
            assert "No task queue" in caplog.text


# ---------------------------------------------------------------------------
# Flush Mode Tests
# ---------------------------------------------------------------------------


class TestPreProcessFlush:
    """Tests for the activities pre_process hook in flush mode."""

    def test_flush_ends_all_active_activities(self, monkeypatch):
        from solstone.talent.activities import pre_process
        from solstone.think.activities import load_activity_records

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            # Set up a segment with an active activity (no following segment)
            _setup_segment(
                tmpdir,
                "20260209",
                "100000_300",
                "work",
                [
                    {
                        "activity": "coding",
                        "state": "active",
                        "since": "100000_300",
                        "description": "Working on feature",
                        "level": "high",
                        "active_entities": ["VS Code"],
                    }
                ],
            )

            result = pre_process(
                {
                    "day": "20260209",
                    "segment": "100000_300",
                    "stream": "default",
                    "flush": True,
                }
            )

            # Should detect the active activity as ended
            assert "skip_reason" not in result
            assert "transcript" in result
            assert "coding_100000_300" in result["transcript"]

            # Record should be written
            records = load_activity_records("work", "20260209")
            assert len(records) == 1
            assert records[0]["id"] == "coding_100000_300"
            assert records[0]["segments"] == ["100000_300"]

    def test_flush_skips_when_no_active_activities(self, monkeypatch):
        from solstone.talent.activities import pre_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            # Set up a segment with only ended activities
            _setup_segment(
                tmpdir,
                "20260209",
                "100000_300",
                "work",
                [
                    {
                        "activity": "coding",
                        "state": "ended",
                        "since": "090000_300",
                    }
                ],
            )

            result = pre_process(
                {
                    "day": "20260209",
                    "segment": "100000_300",
                    "stream": "default",
                    "flush": True,
                }
            )
            assert result["skip_reason"] == "no_active_activities"

    def test_flush_skips_when_no_activity_state(self, monkeypatch):
        from solstone.talent.activities import pre_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            # Create segment dir but no activity_state files
            seg_dir = Path(tmpdir) / "chronicle" / "20260209" / "default" / "100000_300"
            seg_dir.mkdir(parents=True)

            result = pre_process(
                {
                    "day": "20260209",
                    "segment": "100000_300",
                    "stream": "default",
                    "flush": True,
                }
            )
            assert result["skip_reason"] == "no_activity_state"

    def test_flush_handles_multiple_facets(self, monkeypatch):
        from solstone.talent.activities import pre_process
        from solstone.think.activities import load_activity_records

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            _setup_segment(
                tmpdir,
                "20260209",
                "100000_300",
                "work",
                [
                    {
                        "activity": "coding",
                        "state": "active",
                        "since": "100000_300",
                        "description": "Work coding",
                        "level": "high",
                    }
                ],
            )
            _setup_segment(
                tmpdir,
                "20260209",
                "100000_300",
                "personal",
                [
                    {
                        "activity": "browsing",
                        "state": "active",
                        "since": "100000_300",
                        "description": "Personal browsing",
                        "level": "low",
                    }
                ],
            )

            result = pre_process(
                {
                    "day": "20260209",
                    "segment": "100000_300",
                    "stream": "default",
                    "flush": True,
                }
            )

            assert "skip_reason" not in result
            assert "transcript" in result

            work_records = load_activity_records("work", "20260209")
            personal_records = load_activity_records("personal", "20260209")
            assert len(work_records) == 1
            assert len(personal_records) == 1

    def test_flush_is_idempotent(self, monkeypatch):
        from solstone.talent.activities import pre_process
        from solstone.think.activities import load_activity_records

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            _setup_segment(
                tmpdir,
                "20260209",
                "100000_300",
                "work",
                [
                    {
                        "activity": "coding",
                        "state": "active",
                        "since": "100000_300",
                        "description": "Coding",
                        "level": "high",
                    }
                ],
            )

            context = {
                "day": "20260209",
                "segment": "100000_300",
                "stream": "default",
                "flush": True,
            }
            pre_process(context)
            pre_process(context)

            records = load_activity_records("work", "20260209")
            assert len(records) == 1

    def test_flush_stashes_meta_for_post_hook(self, monkeypatch):
        from solstone.talent.activities import pre_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            _setup_segment(
                tmpdir,
                "20260209",
                "100000_300",
                "work",
                [
                    {
                        "activity": "coding",
                        "state": "active",
                        "since": "100000_300",
                        "description": "Coding",
                        "level": "high",
                    }
                ],
            )

            result = pre_process(
                {
                    "day": "20260209",
                    "segment": "100000_300",
                    "stream": "default",
                    "flush": True,
                }
            )

            assert "meta" in result
            records = result["meta"]["activity_records"]
            assert "work" in records
            assert "coding_100000_300" in records["work"]


class TestCheckSegmentFlush:
    """Tests for supervisor's _check_segment_flush."""

    def test_queues_flush_after_timeout(self):
        import time as time_mod
        from unittest.mock import MagicMock, patch

        from solstone.think.supervisor import _check_segment_flush, _flush_state

        # Set up state as if a segment arrived over an hour ago
        _flush_state["last_segment_ts"] = time_mod.time() - 4000
        _flush_state["day"] = "20260209"
        _flush_state["segment"] = "100000_300"
        _flush_state["flushed"] = False

        mock_queue = MagicMock()
        with (
            patch("solstone.think.supervisor._task_queue", mock_queue),
            patch("solstone.think.supervisor._is_remote_mode", False),
        ):
            _check_segment_flush()

        mock_queue.submit.assert_called_once_with(
            [
                "sol",
                "think",
                "-v",
                "--day",
                "20260209",
                "--segment",
                "100000_300",
                "--flush",
            ],
            day="20260209",
        )
        assert _flush_state["flushed"] is True

    def test_does_not_flush_before_timeout(self):
        import time as time_mod
        from unittest.mock import MagicMock, patch

        from solstone.think.supervisor import _check_segment_flush, _flush_state

        _flush_state["last_segment_ts"] = time_mod.time() - 100  # Only 100s ago
        _flush_state["day"] = "20260209"
        _flush_state["segment"] = "100000_300"
        _flush_state["flushed"] = False

        mock_queue = MagicMock()
        with (
            patch("solstone.think.supervisor._task_queue", mock_queue),
            patch("solstone.think.supervisor._is_remote_mode", False),
        ):
            _check_segment_flush()

        mock_queue.submit.assert_not_called()
        assert _flush_state["flushed"] is False

    def test_does_not_flush_twice(self):
        import time as time_mod
        from unittest.mock import MagicMock, patch

        from solstone.think.supervisor import _check_segment_flush, _flush_state

        _flush_state["last_segment_ts"] = time_mod.time() - 4000
        _flush_state["day"] = "20260209"
        _flush_state["segment"] = "100000_300"
        _flush_state["flushed"] = True  # Already flushed

        mock_queue = MagicMock()
        with (
            patch("solstone.think.supervisor._task_queue", mock_queue),
            patch("solstone.think.supervisor._is_remote_mode", False),
        ):
            _check_segment_flush()

        mock_queue.submit.assert_not_called()

    def test_skips_in_remote_mode(self):
        import time as time_mod
        from unittest.mock import MagicMock, patch

        from solstone.think.supervisor import _check_segment_flush, _flush_state

        _flush_state["last_segment_ts"] = time_mod.time() - 4000
        _flush_state["day"] = "20260209"
        _flush_state["segment"] = "100000_300"
        _flush_state["flushed"] = False

        mock_queue = MagicMock()
        with (
            patch("solstone.think.supervisor._task_queue", mock_queue),
            patch("solstone.think.supervisor._is_remote_mode", True),
        ):
            _check_segment_flush()

        mock_queue.submit.assert_not_called()

    def test_force_flushes_before_timeout(self):
        import time as time_mod
        from unittest.mock import MagicMock, patch

        from solstone.think.supervisor import _check_segment_flush, _flush_state

        # Only 100s ago — would NOT flush normally
        _flush_state["last_segment_ts"] = time_mod.time() - 100
        _flush_state["day"] = "20260209"
        _flush_state["segment"] = "100000_300"
        _flush_state["flushed"] = False

        mock_queue = MagicMock()
        with (
            patch("solstone.think.supervisor._task_queue", mock_queue),
            patch("solstone.think.supervisor._is_remote_mode", False),
        ):
            _check_segment_flush(force=True)

        mock_queue.submit.assert_called_once()
        assert _flush_state["flushed"] is True

    def test_segment_observed_resets_flush_state(self):
        from solstone.think.supervisor import _flush_state, _handle_segment_observed

        _flush_state["flushed"] = True
        _flush_state["last_segment_ts"] = 0

        # We need to mock the thread start to avoid side effects
        from unittest.mock import patch

        with patch("solstone.think.supervisor.threading"):
            _handle_segment_observed(
                {
                    "tract": "observe",
                    "event": "observed",
                    "day": "20260209",
                    "segment": "110000_300",
                }
            )

        assert _flush_state["flushed"] is False
        assert _flush_state["day"] == "20260209"
        assert _flush_state["segment"] == "110000_300"
        assert _flush_state["last_segment_ts"] > 0


def _seed_activity_records(
    tmpdir: str, facet: str, day: str, records: list[dict]
) -> None:
    from solstone.think.activities import append_activity_record

    for record in records:
        append_activity_record(facet, day, record)


def test_make_anticipation_id_builds_stable_id():
    from solstone.think.activities import make_anticipation_id

    assert make_anticipation_id("meeting", "16:30:00", "2026-04-20") == (
        "anticipated_meeting_163000_0420"
    )
    assert make_anticipation_id("deadline", None, "2026-05-05") == (
        "anticipated_deadline_000000_0505"
    )


@pytest.mark.parametrize(
    ("activity_type", "start", "target_date"),
    [
        ("meeting", "9:00", "2026-04-20"),
        ("meeting", "09:00:00", "2026/04/20"),
        ("", "09:00:00", "2026-04-20"),
    ],
)
def test_make_anticipation_id_rejects_malformed_inputs(
    activity_type,
    start,
    target_date,
):
    from solstone.think.activities import make_anticipation_id

    with pytest.raises(ValueError):
        make_anticipation_id(activity_type, start, target_date)


def test_dedup_anticipation_returns_empty_for_first_record(monkeypatch):
    from solstone.think.activities import dedup_anticipation

    with tempfile.TemporaryDirectory() as tmpdir:
        monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

        should_write, superseded_ids = dedup_anticipation(
            "work",
            "20260420",
            {"id": "anticipated_meeting_163000_0420", "title": "Yuri intro"},
        )

    assert should_write is True
    assert superseded_ids == []


def test_dedup_anticipation_rejects_exact_id_collision(monkeypatch):
    from solstone.think.activities import dedup_anticipation

    with tempfile.TemporaryDirectory() as tmpdir:
        monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)
        _seed_activity_records(
            tmpdir,
            "work",
            "20260420",
            [
                {
                    "id": "anticipated_meeting_163000_0420",
                    "activity": "meeting",
                    "title": "Yuri intro",
                    "description": "Original",
                    "source": "anticipated",
                }
            ],
        )

        should_write, superseded_ids = dedup_anticipation(
            "work",
            "20260420",
            {"id": "anticipated_meeting_163000_0420", "title": "Yuri intro"},
        )

    assert should_write is False
    assert superseded_ids == []


def test_dedup_anticipation_returns_fuzzy_supersede_matches(monkeypatch):
    from solstone.think.activities import dedup_anticipation

    with tempfile.TemporaryDirectory() as tmpdir:
        monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)
        _seed_activity_records(
            tmpdir,
            "work",
            "20260420",
            [
                {
                    "id": "anticipated_meeting_160000_0420",
                    "activity": "meeting",
                    "title": "Yuri Namikawa intro call",
                    "description": "Original",
                    "source": "anticipated",
                }
            ],
        )

        should_write, superseded_ids = dedup_anticipation(
            "work",
            "20260420",
            {
                "id": "anticipated_meeting_163000_0420",
                "title": "Yuri Namikawa intro call",
            },
        )

    assert should_write is True
    assert superseded_ids == ["anticipated_meeting_160000_0420"]


def test_dedup_anticipation_ignores_below_threshold_and_hidden_rows(monkeypatch):
    from solstone.think.activities import dedup_anticipation

    with tempfile.TemporaryDirectory() as tmpdir:
        monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)
        _seed_activity_records(
            tmpdir,
            "work",
            "20260420",
            [
                {
                    "id": "anticipated_meeting_090000_0420",
                    "activity": "meeting",
                    "title": "Quarterly planning summit",
                    "description": "Visible",
                    "source": "anticipated",
                },
                {
                    "id": "anticipated_meeting_100000_0420",
                    "activity": "meeting",
                    "title": "Yuri Namikawa intro call",
                    "description": "Hidden",
                    "source": "anticipated",
                    "hidden": True,
                },
            ],
        )

        should_write, superseded_ids = dedup_anticipation(
            "work",
            "20260420",
            {
                "id": "anticipated_meeting_163000_0420",
                "title": "Scott Ward standup",
            },
        )

    assert should_write is True
    assert superseded_ids == []


def test_dedup_anticipation_returns_all_matching_supersedes(monkeypatch):
    from solstone.think.activities import dedup_anticipation

    with tempfile.TemporaryDirectory() as tmpdir:
        monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)
        _seed_activity_records(
            tmpdir,
            "work",
            "20260420",
            [
                {
                    "id": "anticipated_call_090000_0420",
                    "activity": "call",
                    "title": "Mari Zumbro intro",
                    "description": "Old 1",
                    "source": "anticipated",
                },
                {
                    "id": "anticipated_call_093000_0420",
                    "activity": "call",
                    "title": "Mari Zumbro intro",
                    "description": "Old 2",
                    "source": "anticipated",
                },
                {
                    "id": "cogitate_call_100000_300",
                    "activity": "call",
                    "title": "Mari Zumbro intro",
                    "description": "Non-anticipated",
                    "source": "cogitate",
                },
            ],
        )

        should_write, superseded_ids = dedup_anticipation(
            "work",
            "20260420",
            {
                "id": "anticipated_call_103000_0420",
                "title": "Mari Zumbro intro",
            },
        )

    assert should_write is True
    assert superseded_ids == [
        "anticipated_call_090000_0420",
        "anticipated_call_093000_0420",
    ]
