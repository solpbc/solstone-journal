# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the activity_state pre/post hook module."""

import json
import tempfile
from pathlib import Path


class TestExtractFacetFromOutputPath:
    """Tests for _extract_facet_from_output_path."""

    def test_extracts_facet_from_valid_path(self):
        from solstone.talent.activity_state import _extract_facet_from_output_path

        path = "/journal/20260130/143000_300/talents/work/activity_state.json"
        assert _extract_facet_from_output_path(path) == "work"

    def test_extracts_facet_with_hyphen(self):
        from solstone.talent.activity_state import _extract_facet_from_output_path

        path = "/journal/20260130/143000_300/talents/my-project/activity_state.json"
        assert _extract_facet_from_output_path(path) == "my-project"

    def test_returns_none_for_empty_path(self):
        from solstone.talent.activity_state import _extract_facet_from_output_path

        assert _extract_facet_from_output_path("") is None
        assert _extract_facet_from_output_path(None) is None

    def test_returns_none_for_non_matching_path(self):
        from solstone.talent.activity_state import _extract_facet_from_output_path

        # Different generator name
        assert _extract_facet_from_output_path("/path/to/facets.json") is None
        # No facet directory
        assert (
            _extract_facet_from_output_path("/path/to/talents/activity_state.json")
            is None
        )


class TestFindPreviousSegment:
    """Tests for find_previous_segment."""

    def test_finds_previous_segment(self, monkeypatch):
        from solstone.talent.activity_state import find_previous_segment

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            # Create day directory with segments
            day_dir = Path(tmpdir) / "chronicle" / "20260130"
            day_dir.mkdir(parents=True)
            (day_dir / "default" / "100000_300").mkdir(parents=True)
            (day_dir / "default" / "110000_300").mkdir(parents=True)
            (day_dir / "default" / "120000_300").mkdir(parents=True)

            # Test finding previous
            assert find_previous_segment("20260130", "120000_300") == "110000_300"
            assert find_previous_segment("20260130", "110000_300") == "100000_300"
            assert find_previous_segment("20260130", "100000_300") is None

    def test_returns_none_for_nonexistent_day(self, monkeypatch):
        from solstone.talent.activity_state import find_previous_segment

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            assert find_previous_segment("20260130", "100000_300") is None

    def test_handles_segments_with_suffix(self, monkeypatch):
        from solstone.talent.activity_state import find_previous_segment

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            day_dir = Path(tmpdir) / "chronicle" / "20260130"
            day_dir.mkdir(parents=True)
            (day_dir / "default" / "100000_300_audio").mkdir(parents=True)
            (day_dir / "default" / "110000_300").mkdir(parents=True)

            # Should still find previous
            assert find_previous_segment("20260130", "110000_300") == "100000_300_audio"


class TestCheckTimeout:
    """Tests for check_timeout."""

    def test_no_timeout_within_threshold(self):
        from solstone.talent.activity_state import check_timeout

        # 5 minute gap (300 seconds)
        assert check_timeout("100500_300", "100000_300", timeout_seconds=3600) is False

    def test_timeout_exceeds_threshold(self):
        from solstone.talent.activity_state import check_timeout

        # 2 hour gap
        assert check_timeout("120000_300", "100000_300", timeout_seconds=3600) is True

    def test_uses_segment_end_time(self):
        from solstone.talent.activity_state import check_timeout

        # Previous segment: 10:00:00 - 10:05:00 (300 seconds)
        # Current segment: 10:10:00
        # Gap should be 5 minutes (10:10:00 - 10:05:00)
        assert check_timeout("101000_300", "100000_300", timeout_seconds=600) is False


class TestLoadPreviousState:
    """Tests for load_previous_state."""

    def test_loads_valid_state(self, monkeypatch):
        from solstone.talent.activity_state import load_previous_state

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            # Create state file (new flat format)
            segment_dir = (
                Path(tmpdir) / "chronicle" / "20260130" / "default" / "100000_300"
            )
            segment_dir.mkdir(parents=True)
            (segment_dir / "talents" / "work").mkdir(parents=True)

            state = [
                {
                    "activity": "meeting",
                    "state": "active",
                    "since": "100000_300",
                    "description": "Standup",
                    "level": "high",
                }
            ]
            (segment_dir / "talents/work/activity_state.json").write_text(
                json.dumps(state)
            )

            loaded, segment = load_previous_state(
                "20260130", "100000_300", "work", stream="default"
            )
            assert segment == "100000_300"
            assert isinstance(loaded, list)
            assert loaded[0]["activity"] == "meeting"

    def test_returns_none_for_missing_file(self, monkeypatch):
        from solstone.talent.activity_state import load_previous_state

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            segment_dir = (
                Path(tmpdir) / "chronicle" / "20260130" / "default" / "100000_300"
            )
            segment_dir.mkdir(parents=True)
            (segment_dir / "talents" / "work").mkdir(parents=True)

            loaded, segment = load_previous_state(
                "20260130", "100000_300", "work", stream="default"
            )
            assert loaded is None
            assert segment is None

    def test_rejects_non_array(self, monkeypatch):
        from solstone.talent.activity_state import load_previous_state

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            segment_dir = (
                Path(tmpdir) / "chronicle" / "20260130" / "default" / "100000_300"
            )
            segment_dir.mkdir(parents=True)
            (segment_dir / "talents" / "work").mkdir(parents=True)

            # Write a dict (old format) — should be rejected
            (segment_dir / "talents/work/activity_state.json").write_text(
                '{"active": [], "ended": []}'
            )

            loaded, segment = load_previous_state(
                "20260130", "100000_300", "work", stream="default"
            )
            assert loaded is None
            assert segment == "100000_300"


class TestFormatActivitiesContext:
    """Tests for format_activities_context."""

    def test_formats_activities_list(self, monkeypatch):
        from solstone.talent.activity_state import format_activities_context

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            # Create facet with activities
            facet_dir = Path(tmpdir) / "facets" / "work" / "activities"
            facet_dir.mkdir(parents=True)

            activities = [
                {"id": "meeting"},
                {"id": "coding", "priority": "high"},
            ]
            (facet_dir / "activities.jsonl").write_text(
                "\n".join(json.dumps(a) for a in activities)
            )

            result = format_activities_context("work")
            assert "## Facet Activities" in result
            assert "meeting" in result
            assert "coding" in result
            assert "[high priority]" in result

    def test_handles_empty_activities(self, monkeypatch):
        """Facet with no activities.jsonl still gets always-on defaults."""
        from solstone.talent.activity_state import format_activities_context

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            # Create facet without activities
            facet_dir = Path(tmpdir) / "facets" / "work"
            facet_dir.mkdir(parents=True)

            result = format_activities_context("work")
            # Always-on activities (messaging, email) are auto-included
            assert "Facet Activities" in result
            assert "messaging" in result
            assert "email" in result


class TestFormatPreviousState:
    """Tests for format_previous_state."""

    def test_formats_active_activities(self):
        from solstone.talent.activity_state import format_previous_state

        state = [
            {
                "activity": "meeting",
                "state": "active",
                "since": "100000_300",
                "description": "Team standup",
                "level": "high",
            }
        ]

        result = format_previous_state(
            state, "100000_300", "100500_300", timed_out=False
        )
        assert "Previous State" in result
        assert "meeting" in result
        assert "Team standup" in result
        # since should NOT appear in context shown to LLM
        assert "since" not in result

    def test_formats_ended_activities(self):
        from solstone.talent.activity_state import format_previous_state

        state = [
            {
                "activity": "email",
                "state": "ended",
                "since": "093000_300",
                "description": "Replied to boss",
            }
        ]

        result = format_previous_state(
            state, "100000_300", "100500_300", timed_out=False
        )
        assert "Recently ended" in result
        assert "email" in result

    def test_handles_timeout(self):
        from solstone.talent.activity_state import format_previous_state

        state = [{"activity": "meeting", "state": "active"}]
        result = format_previous_state(
            state, "100000_300", "120000_300", timed_out=True
        )
        assert "Starting fresh" in result
        assert "meeting" not in result

    def test_handles_no_previous_state(self):
        from solstone.talent.activity_state import format_previous_state

        result = format_previous_state(None, None, "100000_300", timed_out=False)
        assert "No previous segment state" in result

    def test_handles_empty_list(self):
        from solstone.talent.activity_state import format_previous_state

        result = format_previous_state([], "100000_300", "100500_300", timed_out=False)
        assert "No activities were detected" in result


class TestPreProcess:
    """Tests for the pre_process hook function."""

    def test_builds_enriched_context(self, monkeypatch):
        from solstone.talent.activity_state import pre_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            # Create day and segments
            day_dir = Path(tmpdir) / "chronicle" / "20260130"
            day_dir.mkdir(parents=True)
            (day_dir / "default" / "100000_300").mkdir(parents=True)
            (day_dir / "default" / "100000_300" / "talents" / "work").mkdir(
                parents=True
            )
            segment_dir = day_dir / "default" / "110000_300"
            segment_dir.mkdir(parents=True)

            # Create facet with activities
            facet_dir = Path(tmpdir) / "facets" / "work" / "activities"
            facet_dir.mkdir(parents=True)
            (facet_dir / "activities.jsonl").write_text(
                '{"id": "meeting"}\n{"id": "coding"}'
            )

            # Create previous state (new flat format)
            prev_state = [
                {
                    "activity": "meeting",
                    "state": "active",
                    "since": "100000_300",
                    "description": "Standup",
                    "level": "high",
                }
            ]
            (
                day_dir / "default" / "100000_300" / "talents/work/activity_state.json"
            ).write_text(json.dumps(prev_state))

            context = {
                "day": "20260130",
                "segment": "110000_300",
                "stream": "default",
                "output_path": "/journal/20260130/110000_300/talents/work/activity_state.json",
                "transcript": "User is typing code...",
                "meta": {},
            }

            result = pre_process(context)
            assert result is not None
            assert "transcript" in result

            transcript = result["transcript"]
            assert "## Facet Activities" in transcript
            assert "meeting" in transcript
            assert "## Previous State" in transcript
            assert "Standup" in transcript
            assert "## Current Segment Content" in transcript
            assert "User is typing code" in transcript

    def test_returns_none_without_day(self):
        from solstone.talent.activity_state import pre_process

        context = {
            "segment": "100000_300",
            "output_path": "/path/to/talents/work/activity_state.json",
        }
        assert pre_process(context) is None

    def test_returns_none_without_segment(self):
        from solstone.talent.activity_state import pre_process

        context = {
            "day": "20260130",
            "output_path": "/path/to/talents/work/activity_state.json",
        }
        assert pre_process(context) is None

    def test_returns_none_without_facet_in_path(self):
        from solstone.talent.activity_state import pre_process

        context = {
            "day": "20260130",
            "segment": "100000_300",
            "output_path": "/path/to/something_else.json",
        }
        assert pre_process(context) is None


class TestPostProcess:
    """Tests for the post_process hook function."""

    def test_new_activity_gets_current_segment(self):
        from solstone.talent.activity_state import post_process

        llm_output = json.dumps(
            [
                {
                    "activity": "coding",
                    "state": "new",
                    "description": "Writing tests",
                    "level": "high",
                }
            ]
        )

        result = post_process(llm_output, {"segment": "143000_300"})
        assert result is not None
        items = json.loads(result)
        assert len(items) == 1
        assert items[0]["state"] == "active"
        assert items[0]["since"] == "143000_300"
        assert items[0]["level"] == "high"

    def test_continuing_activity_copies_since(self, monkeypatch):
        from solstone.talent.activity_state import post_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            day_dir = Path(tmpdir) / "chronicle" / "20260130"
            day_dir.mkdir(parents=True)

            # Previous segment with active meeting
            prev_dir = day_dir / "default" / "100000_300"
            prev_dir.mkdir(parents=True)
            (prev_dir / "talents" / "work").mkdir(parents=True)
            prev_state = [
                {
                    "activity": "meeting",
                    "state": "active",
                    "since": "093000_300",
                    "description": "Sprint planning",
                    "level": "high",
                }
            ]
            (prev_dir / "talents/work/activity_state.json").write_text(
                json.dumps(prev_state)
            )

            # Current segment
            (day_dir / "default" / "100500_300").mkdir(parents=True)

            llm_output = json.dumps(
                [
                    {
                        "activity": "meeting",
                        "state": "continuing",
                        "description": "Sprint planning - discussing blockers",
                        "level": "high",
                    }
                ]
            )

            context = {
                "day": "20260130",
                "segment": "100500_300",
                "stream": "default",
                "output_path": f"{tmpdir}/20260130/100500_300/talents/work/activity_state.json",
            }

            result = post_process(llm_output, context)
            items = json.loads(result)
            assert items[0]["state"] == "active"
            assert items[0]["since"] == "093000_300"  # Copied from previous

    def test_ended_activity_copies_since(self, monkeypatch):
        from solstone.talent.activity_state import post_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            day_dir = Path(tmpdir) / "chronicle" / "20260130"
            day_dir.mkdir(parents=True)

            # Previous segment with active meeting
            prev_dir = day_dir / "default" / "100000_300"
            prev_dir.mkdir(parents=True)
            (prev_dir / "talents" / "work").mkdir(parents=True)
            prev_state = [
                {
                    "activity": "meeting",
                    "state": "active",
                    "since": "093000_300",
                    "description": "Sprint planning",
                    "level": "high",
                }
            ]
            (prev_dir / "talents/work/activity_state.json").write_text(
                json.dumps(prev_state)
            )

            (day_dir / "default" / "100500_300").mkdir(parents=True)

            llm_output = json.dumps(
                [
                    {
                        "activity": "meeting",
                        "state": "ended",
                        "description": "Sprint planning completed",
                    }
                ]
            )

            context = {
                "day": "20260130",
                "segment": "100500_300",
                "stream": "default",
                "output_path": f"{tmpdir}/20260130/100500_300/talents/work/activity_state.json",
            }

            result = post_process(llm_output, context)
            items = json.loads(result)
            assert items[0]["state"] == "ended"
            assert items[0]["since"] == "093000_300"
            assert "level" not in items[0]

    def test_no_previous_state_continuing_becomes_new(self):
        from solstone.talent.activity_state import post_process

        llm_output = json.dumps(
            [
                {
                    "activity": "coding",
                    "state": "continuing",
                    "description": "Writing code",
                    "level": "high",
                },
            ]
        )

        # No day/output_path — no previous state available
        result = post_process(llm_output, {"segment": "143000_300"})
        items = json.loads(result)

        # "continuing" with no match falls back to current segment
        assert items[0]["since"] == "143000_300"
        assert items[0]["state"] == "active"

    def test_unmatched_ended_with_novel_description_becomes_active(self):
        """Ended activity with no previous active match but novel description
        is treated as a new active activity (LLM mis-tagged)."""
        from solstone.talent.activity_state import post_process

        llm_output = json.dumps(
            [
                {
                    "activity": "meeting",
                    "state": "ended",
                    "description": "Quick sync about deployment",
                    "level": "medium",
                },
            ]
        )

        # No previous state — no active match, no ended match
        result = post_process(llm_output, {"segment": "143000_300"})
        items = json.loads(result)

        assert len(items) == 1
        assert items[0]["state"] == "active"
        assert items[0]["since"] == "143000_300"
        assert items[0]["level"] == "medium"

    def test_unmatched_ended_with_empty_description_dropped(self):
        """Ended activity with no previous active match and no description
        is dropped as redundant."""
        from solstone.talent.activity_state import post_process

        llm_output = json.dumps(
            [
                {
                    "activity": "email",
                    "state": "ended",
                    "description": "",
                },
            ]
        )

        result = post_process(llm_output, {"segment": "143000_300"})
        items = json.loads(result)
        assert len(items) == 0

    def test_unmatched_ended_matching_prev_ended_dropped(self, monkeypatch):
        """Ended activity that matches a previously ended activity is dropped."""
        from solstone.talent.activity_state import post_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            day_dir = Path(tmpdir) / "chronicle" / "20260130"
            day_dir.mkdir(parents=True)

            # Previous segment — email already ended
            prev_dir = day_dir / "default" / "100000_300"
            prev_dir.mkdir(parents=True)
            (prev_dir / "talents" / "work").mkdir(parents=True)
            prev_state = [
                {
                    "activity": "email",
                    "state": "ended",
                    "since": "090000_300",
                    "description": "Replied to boss",
                }
            ]
            (prev_dir / "talents/work/activity_state.json").write_text(
                json.dumps(prev_state)
            )

            (day_dir / "default" / "100500_300").mkdir(parents=True)

            llm_output = json.dumps(
                [
                    {
                        "activity": "email",
                        "state": "ended",
                        "description": "Replied to boss",
                    }
                ]
            )

            context = {
                "day": "20260130",
                "segment": "100500_300",
                "stream": "default",
                "output_path": f"{tmpdir}/20260130/100500_300/talents/work/activity_state.json",
            }

            result = post_process(llm_output, context)
            items = json.loads(result)
            assert len(items) == 0  # Dropped as redundant

    def test_unmatched_ended_novel_desc_with_prev_ended_becomes_active(
        self, monkeypatch
    ):
        """Ended activity with novel description (different from prev ended)
        is promoted to active."""
        from solstone.talent.activity_state import post_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            day_dir = Path(tmpdir) / "chronicle" / "20260130"
            day_dir.mkdir(parents=True)

            # Previous segment — email ended with different description
            prev_dir = day_dir / "default" / "100000_300"
            prev_dir.mkdir(parents=True)
            (prev_dir / "talents" / "work").mkdir(parents=True)
            prev_state = [
                {
                    "activity": "email",
                    "state": "ended",
                    "since": "090000_300",
                    "description": "Replied to boss",
                }
            ]
            (prev_dir / "talents/work/activity_state.json").write_text(
                json.dumps(prev_state)
            )

            (day_dir / "default" / "100500_300").mkdir(parents=True)

            llm_output = json.dumps(
                [
                    {
                        "activity": "email",
                        "state": "ended",
                        "description": "Composing proposal to new client",
                    }
                ]
            )

            context = {
                "day": "20260130",
                "segment": "100500_300",
                "stream": "default",
                "output_path": f"{tmpdir}/20260130/100500_300/talents/work/activity_state.json",
            }

            result = post_process(llm_output, context)
            items = json.loads(result)
            assert len(items) == 1
            assert items[0]["state"] == "active"
            assert items[0]["since"] == "100500_300"

    def test_empty_array_passthrough(self):
        from solstone.talent.activity_state import post_process

        result = post_process("[]", {"segment": "143000_300"})
        assert result is not None
        assert json.loads(result) == []

    def test_malformed_json_returns_none(self):
        from solstone.talent.activity_state import post_process

        result = post_process("not json", {"segment": "143000_300"})
        assert result is None

    def test_non_array_returns_none(self):
        from solstone.talent.activity_state import post_process

        result = post_process('{"active": []}', {"segment": "143000_300"})
        assert result is None

    def test_missing_segment_returns_none(self):
        from solstone.talent.activity_state import post_process

        result = post_process("[]", {})
        assert result is None

    def test_same_type_transition_end_and_new(self, monkeypatch):
        """One meeting ends, another starts — both get correct since."""
        from solstone.talent.activity_state import post_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            day_dir = Path(tmpdir) / "chronicle" / "20260130"
            day_dir.mkdir(parents=True)

            prev_dir = day_dir / "default" / "100000_300"
            prev_dir.mkdir(parents=True)
            (prev_dir / "talents" / "work").mkdir(parents=True)
            prev_state = [
                {
                    "activity": "meeting",
                    "state": "active",
                    "since": "093000_300",
                    "description": "Sprint planning",
                    "level": "high",
                }
            ]
            (prev_dir / "talents/work/activity_state.json").write_text(
                json.dumps(prev_state)
            )

            (day_dir / "default" / "100500_300").mkdir(parents=True)

            llm_output = json.dumps(
                [
                    {
                        "activity": "meeting",
                        "state": "ended",
                        "description": "Sprint planning completed",
                    },
                    {
                        "activity": "meeting",
                        "state": "new",
                        "description": "1:1 with manager",
                        "level": "high",
                    },
                ]
            )

            context = {
                "day": "20260130",
                "segment": "100500_300",
                "stream": "default",
                "output_path": f"{tmpdir}/20260130/100500_300/talents/work/activity_state.json",
            }

            result = post_process(llm_output, context)
            items = json.loads(result)

            ended = [i for i in items if i["state"] == "ended"]
            active = [i for i in items if i["state"] == "active"]

            assert len(ended) == 1
            assert ended[0]["since"] == "093000_300"  # From previous

            assert len(active) == 1
            assert active[0]["since"] == "100500_300"  # Current segment

    def test_default_level_for_new(self):
        """New activity without level gets default 'medium'."""
        from solstone.talent.activity_state import post_process

        llm_output = json.dumps(
            [{"activity": "coding", "state": "new", "description": "Writing code"}]
        )

        result = post_process(llm_output, {"segment": "143000_300"})
        items = json.loads(result)
        assert items[0]["level"] == "medium"

    def test_active_entities_passthrough_on_new(self):
        """active_entities array is passed through on new activities."""
        from solstone.talent.activity_state import post_process

        llm_output = json.dumps(
            [
                {
                    "activity": "meeting",
                    "state": "new",
                    "description": "Standup with team",
                    "level": "high",
                    "active_entities": ["Alice", "Bob"],
                }
            ]
        )

        result = post_process(llm_output, {"segment": "143000_300"})
        items = json.loads(result)
        assert items[0]["active_entities"] == ["Alice", "Bob"]

    def test_active_entities_omitted_when_empty(self):
        """active_entities is omitted from output when not provided or empty."""
        from solstone.talent.activity_state import post_process

        llm_output = json.dumps(
            [
                {
                    "activity": "coding",
                    "state": "new",
                    "description": "Writing code",
                    "level": "high",
                    "active_entities": [],
                }
            ]
        )

        result = post_process(llm_output, {"segment": "143000_300"})
        items = json.loads(result)
        assert "active_entities" not in items[0]

    def test_active_entities_omitted_on_ended(self, monkeypatch):
        """active_entities is not included on ended activities."""
        from solstone.talent.activity_state import post_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            day_dir = Path(tmpdir) / "chronicle" / "20260130"
            day_dir.mkdir(parents=True)

            prev_dir = day_dir / "default" / "100000_300"
            prev_dir.mkdir(parents=True)
            (prev_dir / "talents" / "work").mkdir(parents=True)
            prev_state = [
                {
                    "activity": "meeting",
                    "state": "active",
                    "since": "093000_300",
                    "description": "Sprint planning",
                    "level": "high",
                }
            ]
            (prev_dir / "talents/work/activity_state.json").write_text(
                json.dumps(prev_state)
            )

            (day_dir / "default" / "100500_300").mkdir(parents=True)

            llm_output = json.dumps(
                [
                    {
                        "activity": "meeting",
                        "state": "ended",
                        "description": "Sprint planning completed",
                        "active_entities": ["Alice"],
                    }
                ]
            )

            context = {
                "day": "20260130",
                "segment": "100500_300",
                "stream": "default",
                "output_path": f"{tmpdir}/20260130/100500_300/talents/work/activity_state.json",
            }

            result = post_process(llm_output, context)
            items = json.loads(result)
            assert items[0]["state"] == "ended"
            assert "active_entities" not in items[0]

    def test_fuzzy_match_disambiguates_same_type(self, monkeypatch):
        """Multiple same-type previous activities matched by description."""
        from solstone.talent.activity_state import post_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            day_dir = Path(tmpdir) / "chronicle" / "20260130"
            day_dir.mkdir(parents=True)

            prev_dir = day_dir / "default" / "100000_300"
            prev_dir.mkdir(parents=True)
            (prev_dir / "talents" / "work").mkdir(parents=True)
            prev_state = [
                {
                    "activity": "meeting",
                    "state": "active",
                    "since": "090000_300",
                    "description": "Sprint planning with engineering team",
                    "level": "high",
                },
                {
                    "activity": "meeting",
                    "state": "active",
                    "since": "093000_300",
                    "description": "Customer support standup",
                    "level": "medium",
                },
            ]
            (prev_dir / "talents/work/activity_state.json").write_text(
                json.dumps(prev_state)
            )

            (day_dir / "default" / "100500_300").mkdir(parents=True)

            llm_output = json.dumps(
                [
                    {
                        "activity": "meeting",
                        "state": "continuing",
                        "description": "Customer support standup - discussing tickets",
                        "level": "medium",
                    }
                ]
            )

            context = {
                "day": "20260130",
                "segment": "100500_300",
                "stream": "default",
                "output_path": f"{tmpdir}/20260130/100500_300/talents/work/activity_state.json",
            }

            result = post_process(llm_output, context)
            items = json.loads(result)
            # Should match the standup (093000_300), not sprint planning
            assert items[0]["since"] == "093000_300"


class TestActivityId:
    """Tests for the id field added to resolved activity entries."""

    def test_new_activity_gets_id(self):
        from solstone.talent.activity_state import post_process

        llm_output = json.dumps(
            [
                {
                    "activity": "coding",
                    "state": "new",
                    "description": "Writing tests",
                    "level": "high",
                }
            ]
        )

        result = post_process(llm_output, {"segment": "143000_300"})
        items = json.loads(result)
        assert items[0]["id"] == "coding_143000_300"

    def test_continuing_activity_preserves_since_in_id(self, monkeypatch):
        from solstone.talent.activity_state import post_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            day_dir = Path(tmpdir) / "chronicle" / "20260130"
            day_dir.mkdir(parents=True)

            prev_dir = day_dir / "default" / "100000_300"
            prev_dir.mkdir(parents=True)
            (prev_dir / "talents" / "work").mkdir(parents=True)
            prev_state = [
                {
                    "activity": "meeting",
                    "state": "active",
                    "since": "093000_300",
                    "description": "Sprint planning",
                    "level": "high",
                }
            ]
            (prev_dir / "talents/work/activity_state.json").write_text(
                json.dumps(prev_state)
            )

            (day_dir / "default" / "100500_300").mkdir(parents=True)

            llm_output = json.dumps(
                [
                    {
                        "activity": "meeting",
                        "state": "continuing",
                        "description": "Sprint planning - blockers",
                        "level": "high",
                    }
                ]
            )

            context = {
                "day": "20260130",
                "segment": "100500_300",
                "stream": "default",
                "output_path": f"{tmpdir}/20260130/100500_300/talents/work/activity_state.json",
            }

            result = post_process(llm_output, context)
            items = json.loads(result)
            assert items[0]["id"] == "meeting_093000_300"

    def test_ended_activity_gets_id(self, monkeypatch):
        from solstone.talent.activity_state import post_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            day_dir = Path(tmpdir) / "chronicle" / "20260130"
            day_dir.mkdir(parents=True)

            prev_dir = day_dir / "default" / "100000_300"
            prev_dir.mkdir(parents=True)
            (prev_dir / "talents" / "work").mkdir(parents=True)
            prev_state = [
                {
                    "activity": "meeting",
                    "state": "active",
                    "since": "093000_300",
                    "description": "Sprint planning",
                    "level": "high",
                }
            ]
            (prev_dir / "talents/work/activity_state.json").write_text(
                json.dumps(prev_state)
            )

            (day_dir / "default" / "100500_300").mkdir(parents=True)

            llm_output = json.dumps(
                [
                    {
                        "activity": "meeting",
                        "state": "ended",
                        "description": "Sprint planning completed",
                    }
                ]
            )

            context = {
                "day": "20260130",
                "segment": "100500_300",
                "stream": "default",
                "output_path": f"{tmpdir}/20260130/100500_300/talents/work/activity_state.json",
            }

            result = post_process(llm_output, context)
            items = json.loads(result)
            assert items[0]["id"] == "meeting_093000_300"

    def test_promoted_ended_gets_new_id(self):
        """Ended activity promoted to active gets id with current segment."""
        from solstone.talent.activity_state import post_process

        llm_output = json.dumps(
            [
                {
                    "activity": "meeting",
                    "state": "ended",
                    "description": "Quick sync about deployment",
                    "level": "medium",
                }
            ]
        )

        result = post_process(llm_output, {"segment": "143000_300"})
        items = json.loads(result)
        assert items[0]["id"] == "meeting_143000_300"


class TestActivityLiveEvents:
    """Tests for activity.live callosum event emission."""

    def test_emits_live_for_new_activity(self):
        from unittest.mock import patch

        from solstone.talent.activity_state import post_process

        llm_output = json.dumps(
            [
                {
                    "activity": "coding",
                    "state": "new",
                    "description": "Writing tests",
                    "level": "high",
                    "active_entities": ["VS Code"],
                }
            ]
        )

        context = {
            "day": "20260130",
            "segment": "143000_300",
            "output_path": "/j/20260130/143000_300/talents/work/activity_state.json",
        }

        with patch("solstone.talent.activity_state.callosum_send") as mock_send:
            mock_send.return_value = True
            post_process(llm_output, context)

            mock_send.assert_called_once_with(
                "activity",
                "live",
                facet="work",
                day="20260130",
                segment="143000_300",
                id="coding_143000_300",
                activity="coding",
                since="143000_300",
                description="Writing tests",
                level="high",
                active_entities=["VS Code"],
            )

    def test_emits_live_for_continuing_activity(self, monkeypatch):
        from unittest.mock import patch

        from solstone.talent.activity_state import post_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            day_dir = Path(tmpdir) / "chronicle" / "20260130"
            day_dir.mkdir(parents=True)

            prev_dir = day_dir / "default" / "100000_300"
            prev_dir.mkdir(parents=True)
            (prev_dir / "talents" / "work").mkdir(parents=True)
            prev_state = [
                {
                    "activity": "coding",
                    "state": "active",
                    "since": "093000_300",
                    "description": "Writing code",
                    "level": "high",
                }
            ]
            (prev_dir / "talents/work/activity_state.json").write_text(
                json.dumps(prev_state)
            )

            (day_dir / "default" / "100500_300").mkdir(parents=True)

            llm_output = json.dumps(
                [
                    {
                        "activity": "coding",
                        "state": "continuing",
                        "description": "Still writing code",
                        "level": "medium",
                    }
                ]
            )

            context = {
                "day": "20260130",
                "segment": "100500_300",
                "stream": "default",
                "output_path": f"{tmpdir}/20260130/100500_300/talents/work/activity_state.json",
            }

            with patch("solstone.talent.activity_state.callosum_send") as mock_send:
                mock_send.return_value = True
                post_process(llm_output, context)

                mock_send.assert_called_once()
                call_kwargs = mock_send.call_args[1]
                assert call_kwargs["id"] == "coding_093000_300"
                assert call_kwargs["since"] == "093000_300"

    def test_no_live_event_for_ended_activity(self, monkeypatch):
        from unittest.mock import patch

        from solstone.talent.activity_state import post_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            day_dir = Path(tmpdir) / "chronicle" / "20260130"
            day_dir.mkdir(parents=True)

            prev_dir = day_dir / "default" / "100000_300"
            prev_dir.mkdir(parents=True)
            (prev_dir / "talents" / "work").mkdir(parents=True)
            prev_state = [
                {
                    "activity": "meeting",
                    "state": "active",
                    "since": "093000_300",
                    "description": "Sprint planning",
                    "level": "high",
                }
            ]
            (prev_dir / "talents/work/activity_state.json").write_text(
                json.dumps(prev_state)
            )

            (day_dir / "default" / "100500_300").mkdir(parents=True)

            llm_output = json.dumps(
                [
                    {
                        "activity": "meeting",
                        "state": "ended",
                        "description": "Sprint planning completed",
                    }
                ]
            )

            context = {
                "day": "20260130",
                "segment": "100500_300",
                "stream": "default",
                "output_path": f"{tmpdir}/20260130/100500_300/talents/work/activity_state.json",
            }

            with patch("solstone.talent.activity_state.callosum_send") as mock_send:
                post_process(llm_output, context)
                mock_send.assert_not_called()

    def test_no_live_events_without_day_or_facet(self):
        from unittest.mock import patch

        from solstone.talent.activity_state import post_process

        llm_output = json.dumps(
            [
                {
                    "activity": "coding",
                    "state": "new",
                    "description": "Writing",
                    "level": "high",
                }
            ]
        )

        # No day — events should not fire
        with patch("solstone.talent.activity_state.callosum_send") as mock_send:
            post_process(llm_output, {"segment": "143000_300"})
            mock_send.assert_not_called()

    def test_live_event_failure_does_not_break_posthook(self):
        from unittest.mock import patch

        from solstone.talent.activity_state import post_process

        llm_output = json.dumps(
            [
                {
                    "activity": "coding",
                    "state": "new",
                    "description": "Writing",
                    "level": "high",
                }
            ]
        )

        context = {
            "day": "20260130",
            "segment": "143000_300",
            "output_path": "/j/20260130/143000_300/talents/work/activity_state.json",
        }

        with patch("solstone.talent.activity_state.callosum_send") as mock_send:
            mock_send.side_effect = OSError("socket error")
            result = post_process(llm_output, context)
            # Should still return valid resolved output
            assert result is not None
            items = json.loads(result)
            assert len(items) == 1
            assert items[0]["id"] == "coding_143000_300"


class TestActivityIdValidation:
    """Tests for post-hook activity ID validation against configured vocabulary."""

    def test_drops_unrecognized_activity_ids(self, monkeypatch):
        """Post-hook drops LLM output entries with activity IDs not in config."""
        from unittest.mock import patch

        from solstone.talent.activity_state import post_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            # Create facet with only coding and meeting configured
            facet_dir = Path(tmpdir) / "facets" / "work" / "activities"
            facet_dir.mkdir(parents=True)
            (facet_dir / "activities.jsonl").write_text(
                '{"id": "coding"}\n{"id": "meeting"}'
            )

            llm_output = json.dumps(
                [
                    {
                        "activity": "coding",
                        "state": "new",
                        "description": "Writing code",
                        "level": "high",
                    },
                    {
                        "activity": "Technical Work",
                        "state": "new",
                        "description": "Hallucinated from facet name",
                        "level": "medium",
                    },
                ]
            )

            context = {
                "segment": "143000_300",
                "output_path": f"{tmpdir}/20260130/143000_300/talents/work/activity_state.json",
            }

            with patch("solstone.talent.activity_state.callosum_send"):
                result = post_process(llm_output, context)

            items = json.loads(result)
            assert len(items) == 1
            assert items[0]["activity"] == "coding"

    def test_logs_warning_on_dropped_activities(self, caplog, monkeypatch):
        """Post-hook logs a warning when dropping unrecognized activity IDs."""
        import logging
        from unittest.mock import patch

        from solstone.talent.activity_state import post_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            facet_dir = Path(tmpdir) / "facets" / "work" / "activities"
            facet_dir.mkdir(parents=True)
            (facet_dir / "activities.jsonl").write_text('{"id": "coding"}')

            llm_output = json.dumps(
                [
                    {
                        "activity": "meetings",
                        "state": "new",
                        "description": "Hallucinated",
                        "level": "medium",
                    },
                ]
            )

            context = {
                "segment": "143000_300",
                "output_path": f"{tmpdir}/20260130/143000_300/talents/work/activity_state.json",
            }

            with caplog.at_level(
                logging.WARNING, logger="solstone.talent.activity_state"
            ):
                with patch("solstone.talent.activity_state.callosum_send"):
                    post_process(llm_output, context)

            assert "Dropped 1 activity entries" in caplog.text
            assert "work" in caplog.text

    def test_valid_activity_ids_pass_through(self, monkeypatch):
        """Post-hook preserves entries with valid activity IDs."""
        from unittest.mock import patch

        from solstone.talent.activity_state import post_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            facet_dir = Path(tmpdir) / "facets" / "work" / "activities"
            facet_dir.mkdir(parents=True)
            (facet_dir / "activities.jsonl").write_text(
                '{"id": "coding"}\n{"id": "meeting"}'
            )

            llm_output = json.dumps(
                [
                    {
                        "activity": "coding",
                        "state": "new",
                        "description": "Writing code",
                        "level": "high",
                    },
                    {
                        "activity": "meeting",
                        "state": "new",
                        "description": "Standup",
                        "level": "medium",
                    },
                    {
                        "activity": "email",
                        "state": "new",
                        "description": "Checking inbox",
                        "level": "low",
                    },
                ]
            )

            context = {
                "segment": "143000_300",
                "output_path": f"{tmpdir}/20260130/143000_300/talents/work/activity_state.json",
            }

            with patch("solstone.talent.activity_state.callosum_send"):
                result = post_process(llm_output, context)

            items = json.loads(result)
            # All three are valid: coding + meeting explicit, email always_on
            assert len(items) == 3
            activity_ids = {item["activity"] for item in items}
            assert activity_ids == {"coding", "meeting", "email"}

    def test_unconfigured_facet_allows_all_defaults(self, monkeypatch):
        """Post-hook allows all default activity IDs for unconfigured facets."""
        from unittest.mock import patch

        from solstone.talent.activity_state import post_process

        with tempfile.TemporaryDirectory() as tmpdir:
            monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

            # Create facet dir but no activities.jsonl
            facet_dir = Path(tmpdir) / "facets" / "new_facet"
            facet_dir.mkdir(parents=True)

            llm_output = json.dumps(
                [
                    {
                        "activity": "meeting",
                        "state": "new",
                        "description": "Team sync",
                        "level": "high",
                    },
                    {
                        "activity": "coding",
                        "state": "new",
                        "description": "Writing code",
                        "level": "medium",
                    },
                ]
            )

            context = {
                "segment": "143000_300",
                "output_path": f"{tmpdir}/20260130/143000_300/talents/new_facet/activity_state.json",
            }

            with patch("solstone.talent.activity_state.callosum_send"):
                result = post_process(llm_output, context)

            items = json.loads(result)
            # Both are valid defaults
            assert len(items) == 2
