# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for per-unit daily think idempotency."""

from __future__ import annotations

import importlib
import json
from pathlib import Path

import pytest

DAY = "20990301"


@pytest.fixture
def daily_journal(tmp_path, monkeypatch):
    journal = tmp_path / "journal"
    (journal / "chronicle" / DAY / "health").mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    return journal


def _write_health(journal: Path, day: str, filename: str, events: list[dict]) -> Path:
    path = journal / "chronicle" / day / "health" / filename
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        for event in events:
            handle.write(json.dumps(event) + "\n")
    return path


def _read_jsonl(path: Path) -> list[dict]:
    with path.open(encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


def _complete(name: str, ts: int = 1, facet: str | None = None) -> dict:
    event = {"event": "talent.complete", "ts": ts, "mode": "daily", "name": name}
    if facet:
        event["facet"] = facet
    return event


def _fail(name: str, ts: int = 1, facet: str | None = None) -> dict:
    event = {"event": "talent.fail", "ts": ts, "mode": "daily", "name": name}
    if facet:
        event["facet"] = facet
    return event


def _single_configs(*names: str) -> dict[str, dict]:
    return {name: {"type": "cogitate", "priority": 10} for name in names}


def _install_daily_mocks(
    monkeypatch,
    mod,
    configs: dict[str, dict],
    dispatched: list[tuple[str, dict]],
    *,
    enabled_facets: dict[str, dict] | None = None,
    active_facets: set[str] | None = None,
) -> None:
    monkeypatch.setattr(mod, "get_talent_configs", lambda schedule: configs)
    monkeypatch.setattr(mod, "day_input_summary", lambda day: "No recordings")
    monkeypatch.setattr(mod, "get_enabled_facets", lambda: enabled_facets or {})
    monkeypatch.setattr(mod, "get_active_facets", lambda day: active_facets or set())

    def mock_cortex_request(**kwargs):
        dispatched.append((kwargs["name"], dict(kwargs["config"])))
        return f"use-{len(dispatched)}"

    def mock_drain(spawned, *_args, **_kwargs):
        return (len(spawned), 0, [])

    monkeypatch.setattr(mod, "_cortex_request_with_retry", mock_cortex_request)
    monkeypatch.setattr(mod, "_drain_priority_batch", mock_drain)


def _run_daily_with_writer(mod, journal: Path, day: str, filename: str):
    path = journal / "chronicle" / day / "health" / filename
    old_writer = mod._jsonl
    writer = mod.ThinkingJSONLWriter(str(path))
    mod._jsonl = writer
    try:
        return mod.run_daily_prompts(
            day=day,
            verbose=False,
            max_concurrency=0,
        )
    finally:
        writer.close()
        mod._jsonl = old_writer


def test_check_daily_skip_predicate():
    mod = importlib.import_module("solstone.think.thinking")
    completed = {("daily", "alpha", None), ("daily", "pulse", None)}

    assert mod._check_daily_skip(
        "alpha",
        None,
        mode="daily",
        completed=completed,
        never_skip=mod.NEVER_SKIP_DAILY,
    ) == (True, "already_complete")
    assert mod._check_daily_skip(
        "beta",
        None,
        mode="daily",
        completed=completed,
        never_skip=mod.NEVER_SKIP_DAILY,
    ) == (False, None)
    assert mod._check_daily_skip(
        "alpha",
        None,
        mode="segment",
        completed=completed,
        never_skip=mod.NEVER_SKIP_DAILY,
    ) == (False, None)
    assert mod._check_daily_skip(
        "pulse",
        None,
        mode="daily",
        completed=completed,
        never_skip=mod.NEVER_SKIP_DAILY,
    ) == (False, None)
    assert mod._check_daily_skip(
        "awareness_tender",
        None,
        mode="daily",
        completed={("daily", "awareness_tender", None)},
        never_skip=mod.NEVER_SKIP_DAILY,
    ) == (False, None)


def test_check_daily_skip_has_no_freshness_inputs():
    mod = importlib.import_module("solstone.think.thinking")

    names = mod._check_daily_skip.__code__.co_varnames[
        : mod._check_daily_skip.__code__.co_argcount
        + mod._check_daily_skip.__code__.co_kwonlyargcount
    ]

    assert "stream" not in names
    assert "mtime" not in names
    assert "freshness" not in names


def test_run_daily_prompts_skips_all_completed_units(daily_journal, monkeypatch):
    mod = importlib.import_module("solstone.think.thinking")
    _write_health(
        daily_journal,
        DAY,
        "001_daily.jsonl",
        [_complete("alpha"), _complete("beta")],
    )
    dispatched: list[tuple[str, dict]] = []
    _install_daily_mocks(monkeypatch, mod, _single_configs("alpha", "beta"), dispatched)

    result = _run_daily_with_writer(mod, daily_journal, DAY, "002_daily.jsonl")

    assert dispatched == []
    assert result[3] == {("alpha", None), ("beta", None)}
    skips = [
        event
        for event in _read_jsonl(
            daily_journal / "chronicle" / DAY / "health" / "002_daily.jsonl"
        )
        if event["event"] == "talent.skip"
    ]
    assert {event["name"] for event in skips} == {"alpha", "beta"}
    assert {event["reason"] for event in skips} == {"already_complete"}


def test_run_daily_prompts_repeated_skips_ignore_prior_skips(
    daily_journal, monkeypatch
):
    mod = importlib.import_module("solstone.think.thinking")
    _write_health(
        daily_journal,
        DAY,
        "001_daily.jsonl",
        [_complete("alpha"), _complete("beta")],
    )
    dispatched: list[tuple[str, dict]] = []
    _install_daily_mocks(monkeypatch, mod, _single_configs("alpha", "beta"), dispatched)

    _run_daily_with_writer(mod, daily_journal, DAY, "002_daily.jsonl")
    _run_daily_with_writer(mod, daily_journal, DAY, "003_daily.jsonl")

    assert dispatched == []
    for filename in ("002_daily.jsonl", "003_daily.jsonl"):
        skips = [
            event
            for event in _read_jsonl(
                daily_journal / "chronicle" / DAY / "health" / filename
            )
            if event["event"] == "talent.skip"
        ]
        assert {event["name"] for event in skips} == {"alpha", "beta"}


def test_run_daily_prompts_only_reruns_latest_failures(daily_journal, monkeypatch):
    mod = importlib.import_module("solstone.think.thinking")
    _write_health(
        daily_journal,
        DAY,
        "001_daily.jsonl",
        [_complete("alpha"), _fail("beta")],
    )
    dispatched: list[tuple[str, dict]] = []
    _install_daily_mocks(monkeypatch, mod, _single_configs("alpha", "beta"), dispatched)

    _run_daily_with_writer(mod, daily_journal, DAY, "002_daily.jsonl")

    assert [name for name, _config in dispatched] == ["beta"]
    skips = [
        event
        for event in _read_jsonl(
            daily_journal / "chronicle" / DAY / "health" / "002_daily.jsonl"
        )
        if event["event"] == "talent.skip"
    ]
    assert [event["name"] for event in skips] == ["alpha"]


def test_run_daily_prompts_keys_multi_facet_units_by_facet(daily_journal, monkeypatch):
    mod = importlib.import_module("solstone.think.thinking")
    _write_health(
        daily_journal,
        DAY,
        "001_daily.jsonl",
        [
            _complete("facet_newsletter", facet="work"),
            _fail("facet_newsletter", facet="personal"),
        ],
    )
    configs = {
        "facet_newsletter": {
            "type": "cogitate",
            "priority": 10,
            "multi_facet": True,
        }
    }
    dispatched: list[tuple[str, dict]] = []
    _install_daily_mocks(
        monkeypatch,
        mod,
        configs,
        dispatched,
        enabled_facets={"work": {}, "personal": {}},
        active_facets={"work", "personal"},
    )

    result = _run_daily_with_writer(mod, daily_journal, DAY, "002_daily.jsonl")

    assert [(name, config["facet"]) for name, config in dispatched] == [
        ("facet_newsletter", "personal")
    ]
    assert result[3] == {
        ("facet_newsletter", "work"),
        ("facet_newsletter", "personal"),
    }
    skips = [
        event
        for event in _read_jsonl(
            daily_journal / "chronicle" / DAY / "health" / "002_daily.jsonl"
        )
        if event["event"] == "talent.skip"
    ]
    assert [(event["name"], event["facet"]) for event in skips] == [
        ("facet_newsletter", "work")
    ]


def test_run_daily_prompts_respects_same_run_complete_and_fail_order(
    daily_journal, monkeypatch
):
    mod = importlib.import_module("solstone.think.thinking")
    _write_health(
        daily_journal,
        DAY,
        "001_daily.jsonl",
        [_complete("alpha"), _fail("beta")],
    )
    dispatched: list[tuple[str, dict]] = []
    _install_daily_mocks(monkeypatch, mod, _single_configs("alpha", "beta"), dispatched)

    _run_daily_with_writer(mod, daily_journal, DAY, "002_daily.jsonl")

    assert [name for name, _config in dispatched] == ["beta"]


def test_run_daily_prompts_reruns_dispatch_without_terminal(daily_journal, monkeypatch):
    mod = importlib.import_module("solstone.think.thinking")
    _write_health(
        daily_journal,
        DAY,
        "001_daily.jsonl",
        [{"event": "talent.dispatch", "ts": 1, "mode": "daily", "name": "alpha"}],
    )
    dispatched: list[tuple[str, dict]] = []
    _install_daily_mocks(monkeypatch, mod, _single_configs("alpha"), dispatched)

    _run_daily_with_writer(mod, daily_journal, DAY, "002_daily.jsonl")

    assert [name for name, _config in dispatched] == ["alpha"]


def test_run_daily_prompts_ignores_stream_freshness_for_completed_units(
    daily_journal, monkeypatch
):
    mod = importlib.import_module("solstone.think.thinking")
    _write_health(daily_journal, DAY, "001_daily.jsonl", [_complete("alpha")])
    (daily_journal / "chronicle" / DAY / "health" / "stream.updated").touch()
    dispatched: list[tuple[str, dict]] = []
    _install_daily_mocks(monkeypatch, mod, _single_configs("alpha"), dispatched)

    _run_daily_with_writer(mod, daily_journal, DAY, "002_daily.jsonl")

    assert dispatched == []


def test_run_daily_prompts_refreshes_dispatched_generators(daily_journal, monkeypatch):
    mod = importlib.import_module("solstone.think.thinking")
    configs = {"alpha": {"type": "generate", "priority": 10, "output": "md"}}
    dispatched: list[tuple[str, dict]] = []
    _install_daily_mocks(monkeypatch, mod, configs, dispatched)

    _run_daily_with_writer(mod, daily_journal, DAY, "001_daily.jsonl")

    assert dispatched == [
        (
            "alpha",
            {
                "day": DAY,
                "output": "md",
                "refresh": True,
                "env": {"SOL_DAY": DAY},
                "schedule": "daily",
            },
        )
    ]


def test_run_daily_prompts_refreshes_dispatched_cogitate_with_output(
    daily_journal, monkeypatch
):
    mod = importlib.import_module("solstone.think.thinking")
    configs = {"alpha": {"type": "cogitate", "priority": 10, "output": "md"}}
    _write_health(
        daily_journal,
        DAY,
        "001_daily.jsonl",
        [{"event": "talent.dispatch", "ts": 1, "mode": "daily", "name": "alpha"}],
    )
    output_path = daily_journal / "chronicle" / DAY / "talents" / "alpha.md"
    output_path.parent.mkdir(parents=True)
    output_path.touch()
    dispatched: list[tuple[str, dict]] = []
    _install_daily_mocks(monkeypatch, mod, configs, dispatched)

    _run_daily_with_writer(mod, daily_journal, DAY, "002_daily.jsonl")

    assert dispatched == [
        (
            "alpha",
            {
                "day": DAY,
                "output": "md",
                "refresh": True,
                "env": {"SOL_DAY": DAY},
                "schedule": "daily",
            },
        )
    ]


def test_run_daily_prompts_refreshes_multifacet_cogitate_with_output(
    daily_journal, monkeypatch
):
    mod = importlib.import_module("solstone.think.thinking")
    configs = {
        "alpha": {
            "type": "cogitate",
            "priority": 10,
            "output": "md",
            "multi_facet": True,
        }
    }
    dispatched: list[tuple[str, dict]] = []
    _install_daily_mocks(
        monkeypatch,
        mod,
        configs,
        dispatched,
        enabled_facets={"work": {}},
        active_facets={"work"},
    )

    _run_daily_with_writer(mod, daily_journal, DAY, "001_daily.jsonl")

    assert len(dispatched) == 1
    name, config = dispatched[0]
    assert (name, config["facet"]) == ("alpha", "work")
    assert config["output"] == "md"
    assert config["refresh"] is True
    assert config["env"] == {"SOL_DAY": DAY, "SOL_FACET": "work"}


def _prepare_main_day(journal: Path, day: str) -> Path:
    health = journal / "chronicle" / day / "health"
    health.mkdir(parents=True)
    return health


def _patch_main(monkeypatch, mod, applicable_units):
    calls = []

    def mock_run_command(cmd, day):
        return True

    def mock_run_queued_command(cmd, day, timeout=600):
        return True

    def mock_run_daily_prompts(**kwargs):
        calls.append(kwargs)
        return (len(applicable_units), 0, [], applicable_units)

    monkeypatch.setattr(mod, "run_command", mock_run_command)
    monkeypatch.setattr(mod, "run_queued_command", mock_run_queued_command)
    monkeypatch.setattr(mod, "run_daily_prompts", mock_run_daily_prompts)
    return calls


def test_main_writes_daily_marker_when_all_applicable_complete(
    journal_copy, monkeypatch
):
    mod = importlib.import_module("solstone.think.thinking")
    day = "20990310"
    health = _prepare_main_day(journal_copy, day)
    _write_health(journal_copy, day, "001_daily.jsonl", [_complete("alpha")])
    _patch_main(monkeypatch, mod, {("alpha", None)})
    monkeypatch.setattr("sys.argv", ["sol think", "--day", day])

    mod.main()

    assert (health / "daily.updated").exists()


def test_main_withholds_daily_marker_when_applicable_unit_incomplete(
    journal_copy, monkeypatch
):
    mod = importlib.import_module("solstone.think.thinking")
    day = "20990311"
    health = _prepare_main_day(journal_copy, day)
    _write_health(journal_copy, day, "001_daily.jsonl", [_complete("alpha")])
    _patch_main(monkeypatch, mod, {("alpha", None), ("beta", None)})
    monkeypatch.setattr("sys.argv", ["sol think", "--day", day])

    mod.main()

    assert not (health / "daily.updated").exists()


def test_main_ignores_not_applicable_incomplete_units(journal_copy, monkeypatch):
    mod = importlib.import_module("solstone.think.thinking")
    day = "20990312"
    health = _prepare_main_day(journal_copy, day)
    _write_health(
        journal_copy,
        day,
        "001_daily.jsonl",
        [_complete("alpha"), _fail("beta")],
    )
    _patch_main(monkeypatch, mod, {("alpha", None)})
    monkeypatch.setattr("sys.argv", ["sol think", "--day", day])

    mod.main()

    assert (health / "daily.updated").exists()


def test_main_does_not_force_refresh_from_stream_marker(journal_copy, monkeypatch):
    mod = importlib.import_module("solstone.think.thinking")
    day = "20990313"
    health = _prepare_main_day(journal_copy, day)
    _write_health(journal_copy, day, "001_daily.jsonl", [_complete("alpha")])
    (health / "daily.updated").touch()
    (health / "stream.updated").touch()
    calls = _patch_main(monkeypatch, mod, {("alpha", None)})
    monkeypatch.setattr("sys.argv", ["sol think", "--day", day])

    mod.main()

    assert calls == [
        {"day": day, "verbose": False, "max_concurrency": 2, "stream": None}
    ]
