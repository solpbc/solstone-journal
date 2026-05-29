# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for think --skip-talents behavior."""

import json
from pathlib import Path

import pytest

DAY = "20240115"
SEGMENT = "120000_300"
STREAM = "default"
FACET = "work"
ACTIVITY_ID = "coding_120000_300"


@pytest.fixture
def segment_dir(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """Create a temporary journal with a segment directory."""
    journal = tmp_path / "journal"
    segment_path = journal / "chronicle" / DAY / STREAM / SEGMENT
    (segment_path / "talents").mkdir(parents=True)

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setenv("SOL_SKIP_SUPERVISOR_CHECK", "1")
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
        "documents": {
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
        "awareness_tender": {
            "priority": 30,
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


def _all_segment_configs() -> dict[str, dict]:
    return _segment_configs(
        "sense",
        "entities",
        "documents",
        "screen",
        "speaker_attribution",
        "awareness_tender",
        "pulse",
    )


def _new_only_segment_configs() -> dict[str, dict]:
    configs = _all_segment_configs()
    for name in ("awareness_tender", "pulse"):
        configs[name] = {**configs[name], "new_only": True}
    return configs


def _active_sense_output() -> dict:
    return {
        "density": "active",
        "recommend": {
            "screen_record": True,
            "speaker_attribution": True,
            "pulse_update": True,
        },
        "facets": [],
    }


def _write_sense_output(segment_dir: Path, sense_json: dict) -> None:
    (segment_dir / "talents" / "sense.json").write_text(
        json.dumps(sense_json),
        encoding="utf-8",
    )


def _read_events(path: Path) -> list[dict]:
    return [
        json.loads(line)
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]


def _skip_events(events: list[dict], reason: str = "skip_talents_flag") -> list[dict]:
    return [
        event
        for event in events
        if event["event"] == "talent.skip" and event.get("reason") == reason
    ]


def _patch_segment_dependencies(
    monkeypatch: pytest.MonkeyPatch,
    spawned: list[str],
    append_calls: list[tuple] | None = None,
    activity_calls: list[dict] | None = None,
    configs: dict | None = None,
) -> None:
    from solstone.think import thinking as think

    monkeypatch.setattr(
        think,
        "get_talent_configs",
        lambda schedule=None, **kwargs: (
            configs if configs is not None else _all_segment_configs()
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
    monkeypatch.setattr(
        think,
        "append_activity_record",
        lambda *args: append_calls.append(args) if append_calls is not None else None,
    )
    monkeypatch.setattr(
        think,
        "run_activity_prompts",
        lambda **kwargs: (
            activity_calls.append(kwargs) or True
            if activity_calls is not None
            else True
        ),
    )
    monkeypatch.setattr(think, "_callosum", None)


class EndedActivityStateMachine:
    def __init__(self, journal_root: Path) -> None:
        self.state: dict = {}
        self.last_segment_key: str | None = None
        self.last_segment_day: str | None = None
        self.journal_root = journal_root

    def update(self, sense_output: dict, segment: str, day: str) -> list[dict]:
        self.last_segment_key = segment
        self.last_segment_day = day
        self.state = {
            FACET: {
                "facet": FACET,
                "state": "active",
                "id": ACTIVITY_ID,
            }
        }
        return [{"state": "ended", "id": ACTIVITY_ID, "facet": FACET}]

    def get_completed_activities(self) -> list[dict]:
        return [
            {
                "id": ACTIVITY_ID,
                "activity": "coding",
                "segments": [SEGMENT],
                "level_avg": 0.5,
                "description": "coding",
                "active_entities": [],
                "created_at": 1713200000000,
            }
        ]


class MockCallosumConnection:
    def __init__(self, *args, **kwargs) -> None:
        pass

    def start(self, callback=None) -> None:
        return None

    def emit(self, *args, **kwargs) -> None:
        return None

    def stop(self) -> None:
        return None


def _patch_main_dependencies(
    monkeypatch: pytest.MonkeyPatch,
    segment_dir: Path,
    calls: list[dict],
) -> None:
    from solstone.think import thinking as think

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
        think,
        "iter_segments",
        lambda day: [(STREAM, SEGMENT, segment_dir)],
    )
    monkeypatch.setattr(
        think,
        "cluster_segments",
        lambda day: [
            {
                "key": SEGMENT,
                "stream": STREAM,
                "start": "12:00:00",
                "end": "12:05:00",
            }
        ],
    )
    monkeypatch.setattr(think, "run_segment_sense", mock_run_segment_sense)
    monkeypatch.setattr(think, "check_callosum_available", lambda: True)
    monkeypatch.setattr(think, "CallosumConnection", MockCallosumConnection)


def test_parser_forwards_skip_talents(
    segment_dir: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.think import thinking as think

    calls: list[dict] = []
    _patch_main_dependencies(monkeypatch, segment_dir, calls)
    monkeypatch.setattr(
        "sys.argv",
        [
            "sol think",
            "--day",
            DAY,
            "--segment",
            SEGMENT,
            "--skip-talents",
            "awareness_tender,pulse",
        ],
    )

    think.main()

    assert len(calls) == 1
    assert calls[0]["skip_talents"] == frozenset({"awareness_tender", "pulse"})


def test_empty_flag_forwards_empty_set(
    segment_dir: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.think import thinking as think

    calls: list[dict] = []
    _patch_main_dependencies(monkeypatch, segment_dir, calls)
    monkeypatch.setattr(
        "sys.argv",
        [
            "sol think",
            "--day",
            DAY,
            "--segment",
            SEGMENT,
            "--skip-talents",
            "",
        ],
    )

    think.main()

    assert len(calls) == 1
    assert calls[0]["skip_talents"] == frozenset()


def test_parser_forwards_live_true_for_segment(
    segment_dir: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.think import thinking as think

    calls: list[dict] = []
    _patch_main_dependencies(monkeypatch, segment_dir, calls)
    monkeypatch.setattr(
        "sys.argv",
        [
            "sol think",
            "--day",
            DAY,
            "--segment",
            SEGMENT,
            "--live",
        ],
    )

    think.main()

    assert len(calls) == 1
    assert calls[0]["live"] is True


def test_parser_forwards_live_false_by_default(
    segment_dir: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.think import thinking as think

    calls: list[dict] = []
    _patch_main_dependencies(monkeypatch, segment_dir, calls)
    monkeypatch.setattr(
        "sys.argv",
        [
            "sol think",
            "--day",
            DAY,
            "--segment",
            SEGMENT,
        ],
    )

    think.main()

    assert len(calls) == 1
    assert calls[0]["live"] is False


def test_segments_batch_forwards_live_false(
    segment_dir: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.think import thinking as think

    calls: list[dict] = []
    _patch_main_dependencies(monkeypatch, segment_dir, calls)
    monkeypatch.setattr(
        "sys.argv",
        [
            "sol think",
            "--day",
            DAY,
            "--segments",
        ],
    )

    with pytest.raises(SystemExit) as excinfo:
        think.main()

    assert excinfo.value.code == 0
    assert len(calls) == 1
    assert calls[0]["live"] is False


def test_segment_batch_skip_does_not_dispatch_or_fail(
    segment_dir: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.think import thinking as think
    from solstone.think.thinking import ThinkingJSONLWriter

    spawned: list[str] = []
    jsonl_path = segment_dir.parent.parent / "health" / "test_skip_entities.jsonl"
    writer = ThinkingJSONLWriter(str(jsonl_path))
    _write_sense_output(segment_dir, _active_sense_output())
    (segment_dir / "audio.npz").touch()
    _patch_segment_dependencies(monkeypatch, spawned)
    monkeypatch.setattr(think, "_jsonl", writer)

    success, failed, failed_names = think.run_segment_sense(
        DAY,
        SEGMENT,
        refresh=False,
        verbose=False,
        stream=STREAM,
        skip_talents=frozenset({"entities"}),
    )
    writer.close()
    monkeypatch.setattr(think, "_jsonl", None)

    assert spawned == [
        "sense",
        "documents",
        "screen",
        "speaker_attribution",
        "awareness_tender",
        "pulse",
    ]
    assert success == 6
    assert failed == 0
    assert failed_names == []

    skip_events = _skip_events(_read_events(jsonl_path))
    assert len(skip_events) == 1
    assert skip_events[0]["name"] == "entities"


def test_tail_talent_skip_does_not_dispatch_or_fail(
    segment_dir: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.think import thinking as think
    from solstone.think.thinking import ThinkingJSONLWriter

    spawned: list[str] = []
    jsonl_path = segment_dir.parent.parent / "health" / "test_skip_pulse.jsonl"
    writer = ThinkingJSONLWriter(str(jsonl_path))
    _write_sense_output(segment_dir, _active_sense_output())
    (segment_dir / "audio.npz").touch()
    _patch_segment_dependencies(monkeypatch, spawned)
    monkeypatch.setattr(think, "_jsonl", writer)

    success, failed, failed_names = think.run_segment_sense(
        DAY,
        SEGMENT,
        refresh=False,
        verbose=False,
        stream=STREAM,
        skip_talents=frozenset({"pulse"}),
    )
    writer.close()
    monkeypatch.setattr(think, "_jsonl", None)

    assert "pulse" not in spawned
    assert spawned == [
        "sense",
        "entities",
        "documents",
        "screen",
        "speaker_attribution",
        "awareness_tender",
    ]
    assert success == 6
    assert failed == 0
    assert failed_names == []

    skip_events = _skip_events(_read_events(jsonl_path))
    assert len(skip_events) == 1
    assert skip_events[0]["name"] == "pulse"


def test_new_only_talents_skip_on_historical_segment_think(
    segment_dir: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.think import thinking as think
    from solstone.think.thinking import ThinkingJSONLWriter

    spawned: list[str] = []
    jsonl_path = segment_dir.parent.parent / "health" / "test_new_only_skip.jsonl"
    writer = ThinkingJSONLWriter(str(jsonl_path))
    _write_sense_output(segment_dir, _active_sense_output())
    (segment_dir / "audio.npz").touch()
    _patch_segment_dependencies(
        monkeypatch,
        spawned,
        configs=_new_only_segment_configs(),
    )
    monkeypatch.setattr(think, "_jsonl", writer)

    success, failed, failed_names = think.run_segment_sense(
        DAY,
        SEGMENT,
        refresh=False,
        verbose=False,
        stream=STREAM,
    )
    writer.close()
    monkeypatch.setattr(think, "_jsonl", None)

    assert "pulse" not in spawned
    assert "awareness_tender" not in spawned
    assert spawned == [
        "sense",
        "entities",
        "documents",
        "screen",
        "speaker_attribution",
    ]
    assert success == 5
    assert failed == 0
    assert failed_names == []

    events = _read_events(jsonl_path)
    new_only_events = _skip_events(events, reason="new_only_historical")
    assert len(new_only_events) == 2
    assert {event["name"] for event in new_only_events} == {
        "awareness_tender",
        "pulse",
    }
    pulse_skip = next(event for event in new_only_events if event["name"] == "pulse")
    assert pulse_skip["reason"] == "new_only_historical"
    assert not any(
        event["event"] == "talent.skip"
        and event.get("name") == "pulse"
        and event.get("reason") == "not_recommended"
        for event in events
    )


def test_new_only_talents_dispatch_on_live_segment_think(
    segment_dir: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.think import thinking as think
    from solstone.think.thinking import ThinkingJSONLWriter

    spawned: list[str] = []
    jsonl_path = segment_dir.parent.parent / "health" / "test_new_only_live.jsonl"
    writer = ThinkingJSONLWriter(str(jsonl_path))
    _write_sense_output(segment_dir, _active_sense_output())
    (segment_dir / "audio.npz").touch()
    _patch_segment_dependencies(
        monkeypatch,
        spawned,
        configs=_new_only_segment_configs(),
    )
    monkeypatch.setattr(think, "_jsonl", writer)

    success, failed, failed_names = think.run_segment_sense(
        DAY,
        SEGMENT,
        refresh=False,
        verbose=False,
        stream=STREAM,
        live=True,
    )
    writer.close()
    monkeypatch.setattr(think, "_jsonl", None)

    assert spawned == [
        "sense",
        "entities",
        "documents",
        "screen",
        "speaker_attribution",
        "awareness_tender",
        "pulse",
    ]
    assert success == 7
    assert failed == 0
    assert failed_names == []
    assert _skip_events(_read_events(jsonl_path), reason="new_only_historical") == []


def test_new_only_composes_with_skip_talents_on_live_segment_think(
    segment_dir: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.think import thinking as think
    from solstone.think.thinking import ThinkingJSONLWriter

    spawned: list[str] = []
    jsonl_path = segment_dir.parent.parent / "health" / "test_new_only_compose.jsonl"
    writer = ThinkingJSONLWriter(str(jsonl_path))
    _write_sense_output(segment_dir, _active_sense_output())
    (segment_dir / "audio.npz").touch()
    _patch_segment_dependencies(
        monkeypatch,
        spawned,
        configs=_new_only_segment_configs(),
    )
    monkeypatch.setattr(think, "_jsonl", writer)

    success, failed, failed_names = think.run_segment_sense(
        DAY,
        SEGMENT,
        refresh=False,
        verbose=False,
        stream=STREAM,
        skip_talents=frozenset({"pulse"}),
        live=True,
    )
    writer.close()
    monkeypatch.setattr(think, "_jsonl", None)

    assert "pulse" not in spawned
    assert "awareness_tender" in spawned
    assert success == 6
    assert failed == 0
    assert failed_names == []

    events = _read_events(jsonl_path)
    skip_events = _skip_events(events)
    assert len(skip_events) == 1
    assert skip_events[0]["name"] == "pulse"
    assert _skip_events(events, reason="new_only_historical") == []


def test_sense_skip_uses_cached_output_for_downstream(
    segment_dir: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.think import thinking as think
    from solstone.think.thinking import ThinkingJSONLWriter

    spawned: list[str] = []
    jsonl_path = segment_dir.parent.parent / "health" / "test_skip_sense.jsonl"
    writer = ThinkingJSONLWriter(str(jsonl_path))
    _write_sense_output(segment_dir, _active_sense_output())
    (segment_dir / "audio.npz").touch()
    _patch_segment_dependencies(monkeypatch, spawned)
    monkeypatch.setattr(think, "_jsonl", writer)

    success, failed, failed_names = think.run_segment_sense(
        DAY,
        SEGMENT,
        refresh=False,
        verbose=False,
        stream=STREAM,
        skip_talents=frozenset({"sense"}),
    )
    writer.close()
    monkeypatch.setattr(think, "_jsonl", None)

    assert "sense" not in spawned
    assert spawned == [
        "entities",
        "documents",
        "screen",
        "speaker_attribution",
        "awareness_tender",
        "pulse",
    ]
    assert success == 6
    assert failed == 0
    assert failed_names == []

    skip_events = _skip_events(_read_events(jsonl_path))
    assert len(skip_events) == 1
    assert skip_events[0]["name"] == "sense"


def test_skip_talents_composes_with_no_activity_prompts(
    segment_dir: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.think import thinking as think
    from solstone.think.thinking import ThinkingJSONLWriter

    spawned: list[str] = []
    append_calls: list[tuple] = []
    activity_calls: list[dict] = []
    jsonl_path = segment_dir.parent.parent / "health" / "test_composed.jsonl"
    writer = ThinkingJSONLWriter(str(jsonl_path))
    _write_sense_output(segment_dir, _active_sense_output())
    (segment_dir / "audio.npz").touch()
    _patch_segment_dependencies(monkeypatch, spawned, append_calls, activity_calls)
    monkeypatch.setattr(think, "_jsonl", writer)

    success, failed, failed_names = think.run_segment_sense(
        DAY,
        SEGMENT,
        refresh=False,
        verbose=False,
        stream=STREAM,
        state_machine=EndedActivityStateMachine(segment_dir.parents[3]),
        skip_activity_prompts=True,
        skip_talents=frozenset({"entities"}),
    )
    writer.close()
    monkeypatch.setattr(think, "_jsonl", None)

    assert "entities" not in spawned
    assert len(append_calls) >= 1
    assert activity_calls == []
    assert success == 6
    assert failed == 0
    assert failed_names == []

    events = _read_events(jsonl_path)
    assert [event["name"] for event in _skip_events(events)] == ["entities"]
    assert any(
        event["event"] == "activity.prompts_skipped"
        and event["activity"] == ACTIVITY_ID
        and event["facet"] == FACET
        for event in events
    )


def test_unknown_name_is_silent_noop(
    segment_dir: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.think import thinking as think
    from solstone.think.thinking import ThinkingJSONLWriter

    spawned: list[str] = []
    jsonl_path = segment_dir.parent.parent / "health" / "test_unknown_noop.jsonl"
    writer = ThinkingJSONLWriter(str(jsonl_path))
    _write_sense_output(segment_dir, _active_sense_output())
    (segment_dir / "audio.npz").touch()
    _patch_segment_dependencies(monkeypatch, spawned)
    monkeypatch.setattr(think, "_jsonl", writer)

    success, failed, failed_names = think.run_segment_sense(
        DAY,
        SEGMENT,
        refresh=False,
        verbose=False,
        stream=STREAM,
        skip_talents=frozenset({"bogus_name"}),
    )
    writer.close()
    monkeypatch.setattr(think, "_jsonl", None)

    assert spawned == [
        "sense",
        "entities",
        "documents",
        "screen",
        "speaker_attribution",
        "awareness_tender",
        "pulse",
    ]
    assert success == 7
    assert failed == 0
    assert failed_names == []
    assert _skip_events(_read_events(jsonl_path)) == []
