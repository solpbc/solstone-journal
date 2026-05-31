# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for segment orchestration in think."""

import importlib
import json
from pathlib import Path

import pytest


@pytest.fixture
def segment_dir(tmp_path, monkeypatch):
    """Create a temporary journal with a segment directory."""
    journal = tmp_path / "journal"
    day_dir = journal / "chronicle" / "20240115"
    segment_path = day_dir / "default" / "120000_300"
    segment_path.mkdir(parents=True)
    (segment_path / "talents").mkdir(parents=True)

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    return segment_path


def _segment_configs(*names: str) -> dict[str, dict]:
    configs = {
        "sense": {
            "priority": 10,
            "type": "generate",
            "output": "json",
            "schedule": "segment",
        },
        "entities": {
            "priority": 20,
            "type": "cogitate",
            "schedule": "segment",
        },
        "screen": {
            "priority": 20,
            "type": "generate",
            "output": "md",
            "schedule": "segment",
        },
        "speaker_attribution": {
            "priority": 20,
            "type": "cogitate",
            "schedule": "segment",
        },
        "pulse": {
            "priority": 30,
            "type": "cogitate",
            "schedule": "segment",
        },
    }
    return {name: dict(configs[name]) for name in names}


def _write_sense_output(segment_dir: Path, sense_json: dict) -> None:
    (segment_dir / "talents" / "sense.json").write_text(
        json.dumps(sense_json),
        encoding="utf-8",
    )


class TestLoadSegmentFacets:
    """Tests for load_segment_facets helper function."""

    def test_missing_file_returns_empty(self, segment_dir):
        from solstone.think.facets import load_segment_facets

        assert load_segment_facets("20240115", "120000_300") == []

    def test_empty_file_returns_empty(self, segment_dir):
        from solstone.think.facets import load_segment_facets

        (segment_dir / "talents" / "facets.json").write_text("")
        assert load_segment_facets("20240115", "120000_300") == []

    def test_empty_array_returns_empty(self, segment_dir):
        from solstone.think.facets import load_segment_facets

        (segment_dir / "talents" / "facets.json").write_text("[]")
        assert load_segment_facets("20240115", "120000_300") == []

    def test_valid_facets_extracted(self, segment_dir):
        from solstone.think.facets import load_segment_facets

        facets_data = [
            {"facet": "work", "activity": "Code review", "level": "high"},
            {"facet": "personal", "activity": "Email check", "level": "low"},
        ]
        (segment_dir / "talents" / "facets.json").write_text(json.dumps(facets_data))

        assert load_segment_facets("20240115", "120000_300") == ["work", "personal"]

    def test_malformed_json_returns_empty(self, segment_dir, caplog):
        from solstone.think.facets import load_segment_facets

        (segment_dir / "talents" / "facets.json").write_text("{ invalid json")
        assert load_segment_facets("20240115", "120000_300") == []
        assert "Failed to parse facets.json" in caplog.text

    def test_non_array_returns_empty(self, segment_dir, caplog):
        from solstone.think.facets import load_segment_facets

        (segment_dir / "talents" / "facets.json").write_text('{"facet": "work"}')
        assert load_segment_facets("20240115", "120000_300") == []
        assert "not an array" in caplog.text

    def test_missing_facet_field_skipped(self, segment_dir):
        from solstone.think.facets import load_segment_facets

        facets_data = [
            {"facet": "work", "activity": "Coding"},
            {"activity": "Unknown"},
            {"facet": "personal", "activity": "Email"},
        ]
        (segment_dir / "talents" / "facets.json").write_text(json.dumps(facets_data))

        assert load_segment_facets("20240115", "120000_300") == ["work", "personal"]


class TestRunSegmentSense:
    def test_sense_runs_first(self, segment_dir, monkeypatch):
        from solstone.think import thinking as think

        spawned = []
        _write_sense_output(
            segment_dir,
            {"density": "active", "recommend": {}, "facets": []},
        )

        monkeypatch.setattr(
            think,
            "get_talent_configs",
            lambda schedule=None, **kwargs: _segment_configs("sense", "entities"),
        )
        monkeypatch.setattr(
            think,
            "cortex_request",
            lambda prompt, name, config=None: spawned.append(name) or f"agent-{name}",
        )
        monkeypatch.setattr(
            think,
            "wait_for_uses",
            lambda agent_ids, timeout=600: ({aid: "finish" for aid in agent_ids}, []),
        )
        monkeypatch.setattr(think, "_callosum", None)

        success, failed, failed_names = think.run_segment_sense(
            "20240115",
            "120000_300",
            refresh=False,
            verbose=False,
            stream="default",
        )

        assert spawned == ["sense", "entities"]
        assert success == 2
        assert failed == 0
        assert failed_names == []

    def test_idle_segment_returns_early(self, segment_dir, monkeypatch):
        from solstone.think import thinking as think

        spawned = []
        updates = []

        class StubStateMachine:
            def __init__(self):
                self.state = {}
                self.last_segment_key = None
                self.last_segment_day = None
                self.journal_root = segment_dir.parents[3]

            def update(self, sense_output, segment, day):
                updates.append((sense_output, segment, day))
                self.last_segment_key = segment
                self.last_segment_day = day
                return []

            def get_completed_activities(self):
                return []

        _write_sense_output(
            segment_dir,
            {"density": "idle", "recommend": {"screen_record": True}, "facets": []},
        )

        monkeypatch.setattr(
            think,
            "get_talent_configs",
            lambda schedule=None, **kwargs: _segment_configs(
                "sense", "entities", "screen"
            ),
        )
        monkeypatch.setattr(
            think,
            "cortex_request",
            lambda prompt, name, config=None: spawned.append(name) or f"agent-{name}",
        )
        monkeypatch.setattr(
            think,
            "wait_for_uses",
            lambda agent_ids, timeout=600: ({aid: "finish" for aid in agent_ids}, []),
        )
        monkeypatch.setattr(think, "_callosum", None)

        success, failed, _ = think.run_segment_sense(
            "20240115",
            "120000_300",
            refresh=False,
            verbose=False,
            stream="default",
            state_machine=StubStateMachine(),
        )

        assert spawned == ["sense"]
        assert success == 1
        assert failed == 0
        assert updates == [
            (
                {"density": "idle", "recommend": {"screen_record": True}, "facets": []},
                "120000_300",
                "20240115",
            )
        ]
        density = json.loads((segment_dir / "talents" / "density.json").read_text())
        assert density["classification"] == "idle"

        # Verify activity state persisted even on idle path
        activity_state_path = (
            segment_dir.parents[3] / "awareness" / "activity_state.json"
        )
        assert activity_state_path.exists()
        state_data = json.loads(activity_state_path.read_text())
        assert state_data == {
            "last_segment_key": "120000_300",
            "last_segment_day": "20240115",
            "active": {},
        }

    def test_conditional_screen_dispatch(self, segment_dir, monkeypatch):
        from solstone.think import thinking as think

        spawned = []
        _write_sense_output(
            segment_dir,
            {"density": "active", "recommend": {"screen_record": True}, "facets": []},
        )

        monkeypatch.setattr(
            think,
            "get_talent_configs",
            lambda schedule=None, **kwargs: _segment_configs(
                "sense", "entities", "screen"
            ),
        )
        monkeypatch.setattr(
            think,
            "cortex_request",
            lambda prompt, name, config=None: spawned.append(name) or f"agent-{name}",
        )
        monkeypatch.setattr(
            think,
            "wait_for_uses",
            lambda agent_ids, timeout=600: ({aid: "finish" for aid in agent_ids}, []),
        )
        monkeypatch.setattr(think, "_callosum", None)

        think.run_segment_sense(
            "20240115",
            "120000_300",
            refresh=False,
            verbose=False,
            stream="default",
        )

        assert spawned == ["sense", "entities", "screen"]

    @pytest.mark.parametrize(
        ("has_embeddings", "expected"),
        [
            (False, ["sense", "entities"]),
            (True, ["sense", "entities", "speaker_attribution"]),
        ],
    )
    def test_conditional_speaker_attribution(
        self,
        segment_dir,
        monkeypatch,
        has_embeddings,
        expected,
    ):
        from solstone.think import thinking as think

        spawned = []
        if has_embeddings:
            (segment_dir / "audio.npz").write_bytes(b"npz")

        _write_sense_output(
            segment_dir,
            {
                "density": "active",
                "recommend": {"speaker_attribution": True},
                "facets": [],
            },
        )

        monkeypatch.setattr(
            think,
            "get_talent_configs",
            lambda schedule=None, **kwargs: _segment_configs(
                "sense",
                "entities",
                "speaker_attribution",
            ),
        )
        monkeypatch.setattr(
            think,
            "cortex_request",
            lambda prompt, name, config=None: spawned.append(name) or f"agent-{name}",
        )
        monkeypatch.setattr(
            think,
            "wait_for_uses",
            lambda agent_ids, timeout=600: ({aid: "finish" for aid in agent_ids}, []),
        )
        monkeypatch.setattr(think, "_callosum", None)

        think.run_segment_sense(
            "20240115",
            "120000_300",
            refresh=False,
            verbose=False,
            stream="default",
        )

        assert spawned == expected

    def test_refresh_bypasses_idle(self, segment_dir, monkeypatch):
        from solstone.think import thinking as think

        spawned = []
        _write_sense_output(
            segment_dir,
            {"density": "idle", "recommend": {}, "facets": []},
        )

        monkeypatch.setattr(
            think,
            "get_talent_configs",
            lambda schedule=None, **kwargs: _segment_configs("sense", "entities"),
        )
        monkeypatch.setattr(
            think,
            "cortex_request",
            lambda prompt, name, config=None: spawned.append(name) or f"agent-{name}",
        )
        monkeypatch.setattr(
            think,
            "wait_for_uses",
            lambda agent_ids, timeout=600: ({aid: "finish" for aid in agent_ids}, []),
        )
        monkeypatch.setattr(think, "_callosum", None)

        success, failed, failed_names = think.run_segment_sense(
            "20240115",
            "120000_300",
            refresh=True,
            verbose=False,
            stream="default",
        )

        assert spawned == ["sense", "entities"]
        assert success == 2
        assert failed == 0
        assert failed_names == []

    def test_entities_always_runs(self, segment_dir, monkeypatch):
        from solstone.think import thinking as think

        spawned = []
        _write_sense_output(
            segment_dir,
            {"density": "active", "recommend": {"screen_record": False}, "facets": []},
        )

        monkeypatch.setattr(
            think,
            "get_talent_configs",
            lambda schedule=None, **kwargs: _segment_configs(
                "sense", "entities", "screen"
            ),
        )
        monkeypatch.setattr(
            think,
            "cortex_request",
            lambda prompt, name, config=None: spawned.append(name) or f"agent-{name}",
        )
        monkeypatch.setattr(
            think,
            "wait_for_uses",
            lambda agent_ids, timeout=600: ({aid: "finish" for aid in agent_ids}, []),
        )
        monkeypatch.setattr(think, "_callosum", None)

        think.run_segment_sense(
            "20240115",
            "120000_300",
            refresh=False,
            verbose=False,
            stream="default",
        )

        assert "entities" in spawned
        assert "screen" not in spawned

    def test_pulse_dispatch(self, segment_dir, monkeypatch):
        from solstone.think import thinking as think

        spawned = []
        _write_sense_output(
            segment_dir,
            {"density": "active", "recommend": {"pulse_update": True}, "facets": []},
        )

        monkeypatch.setattr(
            think,
            "get_talent_configs",
            lambda schedule=None, **kwargs: _segment_configs(
                "sense", "entities", "pulse"
            ),
        )
        monkeypatch.setattr(
            think,
            "cortex_request",
            lambda prompt, name, config=None: spawned.append(name) or f"agent-{name}",
        )
        monkeypatch.setattr(
            think,
            "wait_for_uses",
            lambda agent_ids, timeout=600: ({aid: "finish" for aid in agent_ids}, []),
        )
        monkeypatch.setattr(think, "_callosum", None)

        think.run_segment_sense(
            "20240115",
            "120000_300",
            refresh=False,
            verbose=False,
            stream="default",
        )

        assert spawned == ["sense", "entities", "pulse"]

    def test_sense_failure_stops_orchestrator(self, segment_dir, monkeypatch):
        from solstone.think import thinking as think

        spawned = []
        _write_sense_output(
            segment_dir,
            {"density": "active", "recommend": {}, "facets": []},
        )

        monkeypatch.setattr(
            think,
            "get_talent_configs",
            lambda schedule=None, **kwargs: _segment_configs("sense", "entities"),
        )
        monkeypatch.setattr(
            think,
            "cortex_request",
            lambda prompt, name, config=None: spawned.append(name) or f"agent-{name}",
        )

        def mock_wait_for_agents(agent_ids, timeout=600):
            return ({agent_ids[0]: "error"}, [])

        monkeypatch.setattr(think, "wait_for_uses", mock_wait_for_agents)
        monkeypatch.setattr(think, "_callosum", None)

        success, failed, failed_names = think.run_segment_sense(
            "20240115",
            "120000_300",
            refresh=False,
            verbose=False,
            stream="default",
        )

        assert spawned == ["sense"]
        assert success == 0
        assert failed == 1
        assert failed_names == ["sense (error)"]

    def test_activity_state_machine_updated(self, segment_dir, monkeypatch):
        from solstone.think import thinking as think

        updates = []
        activity_calls = []

        class StubStateMachine:
            def __init__(self):
                self.state = {}
                self.last_segment_key = None
                self.last_segment_day = None
                self.journal_root = segment_dir.parents[3]

            def update(self, sense_output, segment, day):
                updates.append((sense_output, segment, day))
                self.last_segment_key = segment
                self.last_segment_day = day
                self.state = {
                    "work": {
                        "facet": "work",
                        "state": "active",
                        "id": "coding_120000_300",
                    }
                }
                return [{"state": "ended", "id": "coding_120000_300", "facet": "work"}]

            def get_completed_activities(self):
                return [
                    {
                        "id": "coding_120000_300",
                        "activity": "coding",
                        "segments": ["120000_300"],
                        "level_avg": 0.5,
                        "description": "coding",
                        "active_entities": [],
                        "created_at": 1713200000000,
                    }
                ]

        _write_sense_output(
            segment_dir,
            {"density": "active", "recommend": {}, "facets": []},
        )

        monkeypatch.setattr(
            think,
            "get_talent_configs",
            lambda schedule=None, **kwargs: _segment_configs("sense", "entities"),
        )
        monkeypatch.setattr(
            think,
            "cortex_request",
            lambda prompt, name, config=None: f"agent-{name}",
        )
        monkeypatch.setattr(
            think,
            "wait_for_uses",
            lambda agent_ids, timeout=600: ({aid: "finish" for aid in agent_ids}, []),
        )
        monkeypatch.setattr(
            think,
            "run_activity_prompts",
            lambda **kwargs: activity_calls.append(kwargs) or True,
        )
        monkeypatch.setattr(think, "_callosum", None)

        think.run_segment_sense(
            "20240115",
            "120000_300",
            refresh=False,
            verbose=False,
            stream="default",
            state_machine=StubStateMachine(),
        )

        assert updates == [
            (
                {"density": "active", "recommend": {}, "facets": []},
                "120000_300",
                "20240115",
            )
        ]
        assert activity_calls == [
            {
                "day": "20240115",
                "activity_id": "coding_120000_300",
                "facet": "work",
                "refresh": False,
                "verbose": False,
                "max_concurrency": 2,
            }
        ]
        activity_state_path = (
            segment_dir.parents[3] / "awareness" / "activity_state.json"
        )
        assert activity_state_path.exists()
        state_data = json.loads(activity_state_path.read_text())
        assert state_data == {
            "last_segment_key": "120000_300",
            "last_segment_day": "20240115",
            "active": {
                "work": {"facet": "work", "state": "active", "id": "coding_120000_300"}
            },
        }

    def test_generator_triggers_incremental_indexing(self, segment_dir, monkeypatch):
        from solstone.think import thinking as think

        indexer_calls = []
        _write_sense_output(
            segment_dir,
            {"density": "active", "recommend": {}, "facets": []},
        )
        (segment_dir / "talents" / "entities.md").write_text(
            "entities", encoding="utf-8"
        )

        monkeypatch.setattr(
            think,
            "get_talent_configs",
            lambda schedule=None, **kwargs: {
                **_segment_configs("sense"),
                "entities": {
                    "priority": 20,
                    "type": "generate",
                    "output": "md",
                    "schedule": "segment",
                },
            },
        )
        monkeypatch.setattr(
            think,
            "cortex_request",
            lambda prompt, name, config=None: f"agent-{name}",
        )
        monkeypatch.setattr(
            think,
            "wait_for_uses",
            lambda agent_ids, timeout=600: ({aid: "finish" for aid in agent_ids}, []),
        )
        monkeypatch.setattr(
            think,
            "run_queued_command",
            lambda cmd, day, timeout=60: indexer_calls.append(cmd) or True,
        )
        monkeypatch.setattr(think, "_callosum", None)

        think.run_segment_sense(
            "20240115",
            "120000_300",
            refresh=False,
            verbose=False,
            stream="default",
        )

        assert len(indexer_calls) == 1
        assert indexer_calls[0][:2] == ["journal", "indexer"]
        assert "--rescan-file" in indexer_calls[0]

    def test_send_failure_counted(self, segment_dir, monkeypatch):
        from solstone.think import thinking as think

        calls = []
        _write_sense_output(
            segment_dir,
            {"density": "active", "recommend": {}, "facets": []},
        )

        def mock_cortex_request(prompt, name, config=None):
            calls.append(name)
            if name == "sense":
                return "agent-sense"
            return None

        monkeypatch.setattr(
            think,
            "get_talent_configs",
            lambda schedule=None, **kwargs: _segment_configs("sense", "entities"),
        )
        monkeypatch.setattr(think, "cortex_request", mock_cortex_request)
        monkeypatch.setattr(think, "_SEND_RETRY_DELAYS", (0.0, 0.0))
        monkeypatch.setattr(
            think,
            "wait_for_uses",
            lambda agent_ids, timeout=600: ({aid: "finish" for aid in agent_ids}, []),
        )
        monkeypatch.setattr(think, "_callosum", None)

        success, failed, failed_names = think.run_segment_sense(
            "20240115",
            "120000_300",
            refresh=False,
            verbose=False,
            stream="default",
        )

        assert calls[0] == "sense"
        assert calls[1:] == ["entities", "entities", "entities"]
        assert success == 1
        assert failed == 1
        assert failed_names == ["entities (send)"]


class TestCortexRequestRetry:
    """Tests for _cortex_request_with_retry."""

    def test_succeeds_on_first_try(self, monkeypatch):
        from solstone.think import thinking as think

        calls = []

        def mock_cortex_request(**kwargs):
            calls.append(kwargs)
            return "agent-1"

        monkeypatch.setattr(think, "cortex_request", mock_cortex_request)

        result = think._cortex_request_with_retry(prompt="hi", name="test")

        assert result == "agent-1"
        assert len(calls) == 1

    def test_succeeds_on_retry(self, monkeypatch):
        from solstone.think import thinking as think

        calls = []

        def mock_cortex_request(**kwargs):
            calls.append(kwargs)
            return None if len(calls) <= 1 else "agent-2"

        monkeypatch.setattr(think, "cortex_request", mock_cortex_request)
        monkeypatch.setattr(think, "_SEND_RETRY_DELAYS", (0.0, 0.0))

        result = think._cortex_request_with_retry(prompt="hi", name="test")

        assert result == "agent-2"
        assert len(calls) == 2

    def test_returns_none_after_all_retries(self, monkeypatch):
        from solstone.think import thinking as think

        calls = []

        def mock_cortex_request(**kwargs):
            calls.append(kwargs)
            return None

        monkeypatch.setattr(think, "cortex_request", mock_cortex_request)
        monkeypatch.setattr(think, "_SEND_RETRY_DELAYS", (0.0, 0.0))

        result = think._cortex_request_with_retry(prompt="hi", name="test")

        assert result is None
        assert len(calls) == 3


class TestStreamAutoResolution:
    """Tests for stream resolution in segment mode."""

    def test_auto_resolves_stream_from_filesystem(self, segment_dir, monkeypatch):
        mod = importlib.import_module("solstone.think.thinking")
        calls: list[dict] = []

        class MockCallosumConnection:
            def __init__(self, *args, **kwargs):
                pass

            def start(self, callback=None):
                return None

            def emit(self, *args, **kwargs):
                return None

            def stop(self):
                return None

        def mock_run_segment_sense(day, segment, refresh, verbose, **kwargs):
            calls.append(
                {
                    "day": day,
                    "segment": segment,
                    "refresh": refresh,
                    "verbose": verbose,
                    **kwargs,
                }
            )
            return (1, 0, [])

        monkeypatch.setattr(
            mod,
            "iter_segments",
            lambda day: [("mystream", "120000_300", Path("/tmp/segment"))],
        )
        monkeypatch.setattr(mod, "run_segment_sense", mock_run_segment_sense)
        monkeypatch.setattr(mod, "check_callosum_available", lambda: True)
        monkeypatch.setattr(mod, "run_command", lambda cmd, day: True)
        monkeypatch.setattr(
            mod, "run_queued_command", lambda cmd, day, timeout=600: True
        )
        monkeypatch.setattr(mod, "CallosumConnection", MockCallosumConnection)
        monkeypatch.setattr(
            "sys.argv",
            ["sol think", "--day", "20240115", "--segment", "120000_300"],
        )

        mod.main()

        assert len(calls) == 1
        assert calls[0]["stream"] == "mystream"

    def test_segment_not_found_exits(self, segment_dir, monkeypatch):
        mod = importlib.import_module("solstone.think.thinking")

        class MockCallosumConnection:
            def __init__(self, *args, **kwargs):
                pass

            def start(self, callback=None):
                return None

            def emit(self, *args, **kwargs):
                return None

            def stop(self):
                return None

        monkeypatch.setattr(mod, "iter_segments", lambda day: [])
        monkeypatch.setattr(
            mod, "run_segment_sense", lambda *args, **kwargs: (1, 0, [])
        )
        monkeypatch.setattr(mod, "check_callosum_available", lambda: True)
        monkeypatch.setattr(mod, "run_command", lambda cmd, day: True)
        monkeypatch.setattr(mod, "CallosumConnection", MockCallosumConnection)
        monkeypatch.setattr(
            "sys.argv",
            ["sol think", "--day", "20240115", "--segment", "999999_300"],
        )

        with pytest.raises(SystemExit) as excinfo:
            mod.main()

        assert excinfo.value.code != 0

    def test_explicit_stream_skips_filesystem_lookup(self, segment_dir, monkeypatch):
        mod = importlib.import_module("solstone.think.thinking")
        iter_calls = 0
        calls: list[dict] = []

        class MockCallosumConnection:
            def __init__(self, *args, **kwargs):
                pass

            def start(self, callback=None):
                return None

            def emit(self, *args, **kwargs):
                return None

            def stop(self):
                return None

        def mock_iter_segments(day):
            nonlocal iter_calls
            iter_calls += 1
            return [("mystream", "120000_300", Path("/tmp/segment"))]

        def mock_run_segment_sense(day, segment, refresh, verbose, **kwargs):
            calls.append(kwargs)
            return (1, 0, [])

        monkeypatch.setattr(mod, "iter_segments", mock_iter_segments)
        monkeypatch.setattr(mod, "run_segment_sense", mock_run_segment_sense)
        monkeypatch.setattr(mod, "check_callosum_available", lambda: True)
        monkeypatch.setattr(mod, "run_command", lambda cmd, day: True)
        monkeypatch.setattr(
            mod, "run_queued_command", lambda cmd, day, timeout=600: True
        )
        monkeypatch.setattr(mod, "CallosumConnection", MockCallosumConnection)
        monkeypatch.setattr(
            "sys.argv",
            [
                "sol think",
                "--day",
                "20240115",
                "--segment",
                "120000_300",
                "--stream",
                "explicit_stream",
            ],
        )

        mod.main()

        assert iter_calls == 0
        assert len(calls) == 1
        assert calls[0]["stream"] == "explicit_stream"


class TestThinkJSONLWriter:
    """Tests for ThinkingJSONLWriter."""

    def test_noop_when_no_path(self):
        from solstone.think.thinking import ThinkingJSONLWriter

        writer = ThinkingJSONLWriter(None)
        writer.log("test.event", foo="bar")
        writer.close()

        assert writer.skip_count == 0

    def test_writes_jsonl_to_file(self, tmp_path):
        from solstone.think.thinking import ThinkingJSONLWriter

        path = tmp_path / "test.jsonl"
        writer = ThinkingJSONLWriter(str(path))
        writer.log("run.start", mode="segment", day="20240115")
        writer.log(
            "talent.skip", name="screen", reason="not_recommended", detail="test"
        )
        writer.close()

        lines = path.read_text().strip().split("\n")
        assert len(lines) == 2

        first = json.loads(lines[0])
        assert first["event"] == "run.start"
        assert "ts" in first
        assert isinstance(first["ts"], int)
        assert first["mode"] == "segment"

        second = json.loads(lines[1])
        assert second["event"] == "talent.skip"
        assert writer.skip_count == 1

    def test_creates_parent_dirs(self, tmp_path):
        from solstone.think.thinking import ThinkingJSONLWriter

        path = tmp_path / "nested" / "dir" / "test.jsonl"
        writer = ThinkingJSONLWriter(str(path))
        writer.log("test.event")
        writer.close()

        assert path.exists()


class TestThinkJSONLEvents:
    """Tests for JSONL event emission during segment orchestration."""

    def test_density_idle_skip_event(self, segment_dir, monkeypatch):
        """JSONL emits talent.skip with reason=density_idle for idle segments."""
        from solstone.think import thinking as think
        from solstone.think.thinking import ThinkingJSONLWriter

        jsonl_path = segment_dir.parent.parent / "health" / "test_idle.jsonl"
        writer = ThinkingJSONLWriter(str(jsonl_path))

        _write_sense_output(
            segment_dir,
            {"density": "idle", "recommend": {}, "facets": []},
        )

        monkeypatch.setattr(
            think,
            "get_talent_configs",
            lambda schedule=None, **kwargs: _segment_configs("sense"),
        )
        monkeypatch.setattr(
            think,
            "cortex_request",
            lambda prompt, name, config=None: "agent-sense",
        )
        monkeypatch.setattr(
            think,
            "wait_for_uses",
            lambda agent_ids, timeout=600: ({aid: "finish" for aid in agent_ids}, []),
        )
        monkeypatch.setattr(think, "_callosum", None)
        monkeypatch.setattr(think, "_jsonl", writer)

        think.run_segment_sense(
            "20240115",
            "120000_300",
            refresh=False,
            verbose=False,
            stream="default",
        )
        writer.close()

        events = [
            json.loads(line)
            for line in jsonl_path.read_text(encoding="utf-8").strip().splitlines()
        ]
        skips = [event for event in events if event["event"] == "talent.skip"]

        assert any(skip["reason"] == "density_idle" for skip in skips)

    def test_sense_complete_and_skip_events(self, segment_dir, monkeypatch):
        from solstone.think import thinking as think
        from solstone.think.thinking import ThinkingJSONLWriter

        jsonl_path = segment_dir.parent.parent / "health" / "test_think.jsonl"
        writer = ThinkingJSONLWriter(str(jsonl_path))

        _write_sense_output(
            segment_dir,
            {
                "density": "active",
                "recommend": {
                    "screen_record": False,
                    "speaker_attribution": False,
                    "pulse_update": False,
                },
                "facets": [],
            },
        )

        monkeypatch.setattr(
            think,
            "get_talent_configs",
            lambda schedule=None, **kwargs: _segment_configs("sense", "entities"),
        )
        monkeypatch.setattr(
            think,
            "cortex_request",
            lambda prompt, name, config=None: f"agent-{name}",
        )
        monkeypatch.setattr(
            think,
            "wait_for_uses",
            lambda agent_ids, timeout=600: ({aid: "finish" for aid in agent_ids}, []),
        )
        monkeypatch.setattr(think, "_callosum", None)
        monkeypatch.setattr(think, "_jsonl", writer)

        think.run_segment_sense(
            "20240115",
            "120000_300",
            refresh=False,
            verbose=False,
            stream="default",
        )
        writer.close()

        events = [
            json.loads(line)
            for line in jsonl_path.read_text(encoding="utf-8").strip().splitlines()
        ]
        assert "sense.complete" in [event["event"] for event in events]

        skips = [event for event in events if event["event"] == "talent.skip"]
        skip_pairs = {(event["name"], event["reason"]) for event in skips}
        assert ("documents", "no_config") in skip_pairs
        assert ("screen", "not_recommended") in skip_pairs
        assert ("speaker_attribution", "not_recommended") in skip_pairs
        assert ("pulse", "not_recommended") in skip_pairs
