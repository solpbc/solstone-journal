# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for per-unit daily think idempotency."""

from __future__ import annotations

import importlib
import json
import os
from pathlib import Path

import pytest

from solstone.think.deterministic_failure_caps import DETERMINISTIC_FAILURE_CAPS
from solstone.think.pipeline_health import (
    DeterministicFailure,
    read_completed_units,
    read_daily_deterministic_failures,
)
from solstone.think.utils import updated_days

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


def _skip_events(journal: Path, day: str, filename: str) -> list[dict]:
    return [
        event
        for event in _read_jsonl(journal / "chronicle" / day / "health" / filename)
        if event["event"] == "talent.skip"
    ]


def _complete(name: str, ts: int = 1, facet: str | None = None) -> dict:
    event = {"event": "talent.complete", "ts": ts, "mode": "daily", "name": name}
    if facet:
        event["facet"] = facet
    return event


def _fail(
    name: str,
    ts: int = 1,
    facet: str | None = None,
    *,
    state: str | None = None,
    reason_code: str | None = None,
) -> dict:
    event = {"event": "talent.fail", "ts": ts, "mode": "daily", "name": name}
    if facet:
        event["facet"] = facet
    if state:
        event["state"] = state
    if reason_code:
        event["reason_code"] = reason_code
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

    monkeypatch.setattr(mod, "_dispatch_cortex_request", mock_cortex_request)
    monkeypatch.setattr(mod, "_drain_priority_batch", mock_drain)


def _run_daily_with_writer(mod, journal: Path, day: str, filename: str, **kwargs):
    path = journal / "chronicle" / day / "health" / filename
    old_writer = mod._jsonl
    writer = mod.ThinkingJSONLWriter(str(path))
    mod._jsonl = writer
    try:
        return mod.run_daily_prompts(
            day=day,
            verbose=False,
            max_concurrency=0,
            **kwargs,
        )
    finally:
        writer.close()
        mod._jsonl = old_writer


def test_check_daily_skip_predicate():
    mod = importlib.import_module("solstone.think.thinking")
    completed = {("daily", "alpha", None), ("daily", "pulse", None)}
    deterministic_failures = {}

    assert mod._check_daily_skip(
        "alpha",
        None,
        mode="daily",
        completed=completed,
        deterministic_failures=deterministic_failures,
    ) == (True, "already_complete")
    assert mod._check_daily_skip(
        "beta",
        None,
        mode="daily",
        completed=completed,
        deterministic_failures=deterministic_failures,
    ) == (False, None)
    assert mod._check_daily_skip(
        "alpha",
        None,
        mode="segment",
        completed=completed,
        deterministic_failures=deterministic_failures,
    ) == (False, None)
    assert mod._check_daily_skip(
        "pulse",
        None,
        mode="daily",
        completed=completed,
        deterministic_failures=deterministic_failures,
    ) == (True, "already_complete")
    assert mod._check_daily_skip(
        "alpha",
        None,
        mode="daily",
        completed=completed,
        deterministic_failures=deterministic_failures,
        from_scratch=True,
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
    assert "from_scratch" in names


@pytest.mark.parametrize("retry_on_deterministic_failure", [False, True])
@pytest.mark.parametrize("at_cap", [False, True])
@pytest.mark.parametrize(
    ("reason_code", "cap"), sorted(DETERMINISTIC_FAILURE_CAPS.items())
)
def test_daily_skip_and_completion_cap_predicates_match(
    retry_on_deterministic_failure,
    at_cap,
    reason_code,
    cap,
):
    mod = importlib.import_module("solstone.think.thinking")
    count = cap if at_cap else cap - 1
    deterministic_failures = {
        ("beta", None): DeterministicFailure(count=count, reason_code=reason_code)
    }

    skip, _reason = mod._check_daily_skip(
        "beta",
        None,
        mode="daily",
        completed=set(),
        deterministic_failures=deterministic_failures,
        retry_on_deterministic_failure=retry_on_deterministic_failure,
    )
    verdict = mod.evaluate_daily_completion(
        {("beta", None)},
        set(),
        deterministic_failures,
        [],
    )
    terminal_degraded = bool(verdict.capped_daily_units)

    if retry_on_deterministic_failure:
        assert skip is False
        assert terminal_degraded is at_cap
    else:
        assert skip is terminal_degraded
    assert not (skip and not terminal_degraded)


def test_evaluate_daily_completion_terminal_cases():
    mod = importlib.import_module("solstone.think.thinking")

    complete_and_capped = mod.evaluate_daily_completion(
        {("alpha", None), ("beta", None)},
        {("daily", "alpha", None)},
        {
            ("beta", None): DeterministicFailure(
                count=2, reason_code="context_window_exceeded"
            )
        },
        [],
    )
    assert complete_and_capped.complete is True
    assert complete_and_capped.daily_units_terminal is True
    assert complete_and_capped.capped_daily_units == (
        mod.CappedDailyUnit(
            name="beta",
            facet=None,
            reason_code="context_window_exceeded",
            count=2,
        ),
    )

    below_cap = mod.evaluate_daily_completion(
        {("beta", None)},
        set(),
        {
            ("beta", None): DeterministicFailure(
                count=1, reason_code="context_window_exceeded"
            )
        },
        [],
    )
    assert below_cap.complete is False
    assert below_cap.capped_daily_units == ()


def test_evaluate_daily_completion_transient_latest_stays_incomplete(journal_copy):
    mod = importlib.import_module("solstone.think.thinking")
    day = "20990319"
    _prepare_main_day(journal_copy, day)
    _write_health(
        journal_copy,
        day,
        "001_daily.jsonl",
        [
            _complete("alpha"),
            _fail("beta", ts=1, reason_code="context_window_exceeded"),
            _fail("beta", ts=2, reason_code="context_window_exceeded"),
            _fail("beta", ts=3, reason_code="schema_invalid"),
            _fail("beta", ts=4, reason_code="token_budget_exceeded"),
            _fail("beta", ts=5, reason_code="provider_transient"),
        ],
    )

    completed = read_completed_units(day)
    deterministic_failures = read_daily_deterministic_failures(day)

    assert ("beta", None) not in deterministic_failures
    transient_latest = mod.evaluate_daily_completion(
        {("alpha", None), ("beta", None)},
        completed,
        deterministic_failures,
        [],
    )
    assert transient_latest.complete is False
    assert transient_latest.capped_daily_units == ()


def test_evaluate_daily_completion_dispatch_without_terminal_stays_incomplete(
    journal_copy,
):
    mod = importlib.import_module("solstone.think.thinking")
    day = "20990320"
    _prepare_main_day(journal_copy, day)
    _write_health(
        journal_copy,
        day,
        "001_daily.jsonl",
        [
            _complete("alpha"),
            {"event": "talent.dispatch", "ts": 2, "mode": "daily", "name": "beta"},
        ],
    )

    completed = read_completed_units(day)
    deterministic_failures = read_daily_deterministic_failures(day)

    assert ("daily", "beta", None) not in completed
    assert ("beta", None) not in deterministic_failures
    dispatched_without_terminal = mod.evaluate_daily_completion(
        {("alpha", None), ("beta", None)},
        completed,
        deterministic_failures,
        [],
    )
    assert dispatched_without_terminal.complete is False
    assert dispatched_without_terminal.capped_daily_units == ()


def test_evaluate_daily_completion_withholds_with_segment_blockers():
    mod = importlib.import_module("solstone.think.thinking")

    verdict = mod.evaluate_daily_completion(
        {("beta", None)},
        set(),
        {
            ("beta", None): DeterministicFailure(
                count=2, reason_code="context_window_exceeded"
            )
        },
        [{"segment": "090000_300", "dimension": "not_thought", "detail": "floor"}],
    )

    assert verdict.complete is False
    assert verdict.daily_units_terminal is True
    assert verdict.capped_daily_units
    assert verdict.segment_blockers == (
        {"segment": "090000_300", "dimension": "not_thought", "detail": "floor"},
    )


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


def test_run_daily_prompts_from_scratch_reruns_completed_units(
    daily_journal, monkeypatch
):
    mod = importlib.import_module("solstone.think.thinking")
    _write_health(daily_journal, DAY, "001_daily.jsonl", [_complete("alpha")])
    dispatched: list[tuple[str, dict]] = []
    _install_daily_mocks(monkeypatch, mod, _single_configs("alpha"), dispatched)

    _run_daily_with_writer(
        mod,
        daily_journal,
        DAY,
        "002_daily.jsonl",
        from_scratch=True,
    )

    assert [name for name, _config in dispatched] == ["alpha"]


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


def test_run_daily_prompts_reruns_no_output_failures(daily_journal, monkeypatch):
    mod = importlib.import_module("solstone.think.thinking")
    _write_health(
        daily_journal,
        DAY,
        "001_daily.jsonl",
        [
            _complete("alpha"),
            _fail("beta", state="error", reason_code="no_output"),
        ],
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


def test_run_daily_prompts_skips_two_deterministic_failures(daily_journal, monkeypatch):
    mod = importlib.import_module("solstone.think.thinking")
    _write_health(
        daily_journal,
        DAY,
        "001_daily.jsonl",
        [
            _fail("alpha", ts=1, reason_code="context_window_exceeded"),
            _fail("alpha", ts=2, reason_code="context_window_exceeded"),
            _complete("beta"),
        ],
    )
    dispatched: list[tuple[str, dict]] = []
    _install_daily_mocks(monkeypatch, mod, _single_configs("alpha", "beta"), dispatched)

    _run_daily_with_writer(mod, daily_journal, DAY, "002_daily.jsonl")

    assert dispatched == []
    skips = _skip_events(daily_journal, DAY, "002_daily.jsonl")
    assert {event["name"] for event in skips} == {"alpha", "beta"}
    alpha_skip = next(event for event in skips if event["name"] == "alpha")
    assert alpha_skip["reason"] == "deterministic_failure_no_retry"
    assert alpha_skip["detail"] == (
        "2 same-day deterministic failures "
        "(context_window_exceeded); not re-dispatching"
    )


def test_run_daily_prompts_skips_one_model_not_found_failure(
    daily_journal, monkeypatch
):
    mod = importlib.import_module("solstone.think.thinking")
    _write_health(
        daily_journal,
        DAY,
        "001_daily.jsonl",
        [_fail("alpha", reason_code="model_not_found")],
    )
    dispatched: list[tuple[str, dict]] = []
    _install_daily_mocks(monkeypatch, mod, _single_configs("alpha"), dispatched)

    _run_daily_with_writer(mod, daily_journal, DAY, "002_daily.jsonl")

    assert dispatched == []
    skips = _skip_events(daily_journal, DAY, "002_daily.jsonl")
    assert [(event["name"], event["reason"]) for event in skips] == [
        ("alpha", "deterministic_failure_no_retry")
    ]
    assert skips[0]["detail"] == (
        "1 same-day deterministic failures (model_not_found); not re-dispatching"
    )


def test_run_daily_prompts_schema_invalid_retries_until_third_failure(
    daily_journal, monkeypatch
):
    mod = importlib.import_module("solstone.think.thinking")
    _write_health(
        daily_journal,
        DAY,
        "001_daily.jsonl",
        [
            _fail("alpha", ts=1, reason_code="schema_invalid"),
            _fail("alpha", ts=2, reason_code="schema_invalid"),
        ],
    )
    dispatched: list[tuple[str, dict]] = []
    _install_daily_mocks(monkeypatch, mod, _single_configs("alpha"), dispatched)

    _run_daily_with_writer(mod, daily_journal, DAY, "002_daily.jsonl")

    assert [name for name, _config in dispatched] == ["alpha"]
    dispatched.clear()
    _write_health(
        daily_journal,
        DAY,
        "003_daily.jsonl",
        [_fail("alpha", ts=3, reason_code="schema_invalid")],
    )

    _run_daily_with_writer(mod, daily_journal, DAY, "004_daily.jsonl")

    assert dispatched == []
    skips = _skip_events(daily_journal, DAY, "004_daily.jsonl")
    assert skips[0]["reason"] == "deterministic_failure_no_retry"
    assert skips[0]["detail"] == (
        "3 same-day deterministic failures (schema_invalid); not re-dispatching"
    )


def test_mixed_deterministic_reason_uses_latest_reason_cap(daily_journal, monkeypatch):
    mod = importlib.import_module("solstone.think.thinking")
    _write_health(
        daily_journal,
        DAY,
        "001_daily.jsonl",
        [
            _fail("alpha", ts=1, reason_code="context_window_exceeded"),
            _fail("alpha", ts=2, reason_code="schema_invalid"),
            _fail("beta", ts=1, reason_code="schema_invalid"),
            _fail("beta", ts=2, reason_code="context_window_exceeded"),
        ],
    )
    dispatched: list[tuple[str, dict]] = []
    _install_daily_mocks(monkeypatch, mod, _single_configs("alpha", "beta"), dispatched)

    _run_daily_with_writer(mod, daily_journal, DAY, "002_daily.jsonl")

    assert [name for name, _config in dispatched] == ["alpha"]
    skips = _skip_events(daily_journal, DAY, "002_daily.jsonl")
    assert [(event["name"], event["reason"]) for event in skips] == [
        ("beta", "deterministic_failure_no_retry")
    ]


def test_run_daily_prompts_reruns_one_deterministic_failure(daily_journal, monkeypatch):
    mod = importlib.import_module("solstone.think.thinking")
    _write_health(
        daily_journal,
        DAY,
        "001_daily.jsonl",
        [_fail("alpha", reason_code="context_window_exceeded")],
    )
    dispatched: list[tuple[str, dict]] = []
    _install_daily_mocks(monkeypatch, mod, _single_configs("alpha"), dispatched)

    _run_daily_with_writer(mod, daily_journal, DAY, "002_daily.jsonl")

    assert [name for name, _config in dispatched] == ["alpha"]


def test_run_daily_prompts_reruns_transient_failures(daily_journal, monkeypatch):
    mod = importlib.import_module("solstone.think.thinking")
    _write_health(
        daily_journal,
        DAY,
        "001_daily.jsonl",
        [
            _fail("alpha", ts=1, reason_code="provider_quota_exceeded"),
            _fail("alpha", ts=2, reason_code="provider_quota_exceeded"),
            _fail("alpha", ts=3, reason_code="provider_quota_exceeded"),
        ],
    )
    dispatched: list[tuple[str, dict]] = []
    _install_daily_mocks(monkeypatch, mod, _single_configs("alpha"), dispatched)

    _run_daily_with_writer(mod, daily_journal, DAY, "002_daily.jsonl")

    assert [name for name, _config in dispatched] == ["alpha"]


def test_run_daily_prompts_deterministic_failure_resets_per_day(
    daily_journal, monkeypatch
):
    mod = importlib.import_module("solstone.think.thinking")
    other_day = "20990302"
    _write_health(
        daily_journal,
        DAY,
        "001_daily.jsonl",
        [
            _fail("alpha", ts=1, reason_code="context_window_exceeded"),
            _fail("alpha", ts=2, reason_code="context_window_exceeded"),
        ],
    )
    dispatched: list[tuple[str, dict]] = []
    _install_daily_mocks(monkeypatch, mod, _single_configs("alpha"), dispatched)

    _run_daily_with_writer(mod, daily_journal, other_day, "001_daily.jsonl")

    assert [name for name, _config in dispatched] == ["alpha"]


def test_run_daily_prompts_retry_on_deterministic_failure_override(
    daily_journal, monkeypatch
):
    mod = importlib.import_module("solstone.think.thinking")
    _write_health(
        daily_journal,
        DAY,
        "001_daily.jsonl",
        [
            _fail("alpha", ts=1, reason_code="context_window_exceeded"),
            _fail("alpha", ts=2, reason_code="context_window_exceeded"),
        ],
    )
    configs = {
        "alpha": {
            "type": "cogitate",
            "priority": 10,
            "retry_on_deterministic_failure": True,
        }
    }
    dispatched: list[tuple[str, dict]] = []
    _install_daily_mocks(monkeypatch, mod, configs, dispatched)

    _run_daily_with_writer(mod, daily_journal, DAY, "002_daily.jsonl")

    assert [name for name, _config in dispatched] == ["alpha"]


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


def test_main_withholds_daily_marker_when_no_output_failure_is_incomplete(
    journal_copy, monkeypatch
):
    mod = importlib.import_module("solstone.think.thinking")
    day = "20990314"
    health = _prepare_main_day(journal_copy, day)
    _write_health(
        journal_copy,
        day,
        "001_daily.jsonl",
        [_fail("alpha", state="error", reason_code="no_output")],
    )
    _patch_main(monkeypatch, mod, {("alpha", None)})
    monkeypatch.setattr("sys.argv", ["sol think", "--day", day])

    mod.main()

    assert not (health / "daily.updated").exists()


def test_main_writes_daily_marker_when_capped_unit_terminal_and_payload_includes_capped_units(
    journal_copy, monkeypatch
):
    mod = importlib.import_module("solstone.think.thinking")
    day = "20990316"
    health = _prepare_main_day(journal_copy, day)
    _write_health(
        journal_copy,
        day,
        "001_daily.jsonl",
        [
            _complete("alpha"),
            _fail("beta", ts=1, reason_code="context_window_exceeded"),
            _fail("beta", ts=2, reason_code="context_window_exceeded"),
        ],
    )
    _patch_main(monkeypatch, mod, {("alpha", None), ("beta", None)})
    emitted: list[tuple[str, dict]] = []
    monkeypatch.setattr(
        mod,
        "emit",
        lambda event, **fields: emitted.append((event, fields)),
    )
    monkeypatch.setattr("sys.argv", ["sol think", "--day", day])

    mod.main()

    assert (health / "daily.updated").exists()
    daily_complete = next(
        fields for event, fields in emitted if event == "daily_complete"
    )
    assert daily_complete["capped_daily_units"] == [
        {
            "name": "beta",
            "facet": None,
            "reason_code": "context_window_exceeded",
            "count": 2,
        }
    ]


def test_capped_completion_clears_after_later_complete(journal_copy):
    mod = importlib.import_module("solstone.think.thinking")
    day = "20990317"
    _prepare_main_day(journal_copy, day)
    _write_health(
        journal_copy,
        day,
        "001_daily.jsonl",
        [
            _complete("alpha"),
            _fail("beta", ts=1, reason_code="context_window_exceeded"),
            _fail("beta", ts=2, reason_code="context_window_exceeded"),
        ],
    )

    capped = mod.evaluate_daily_completion(
        {("alpha", None), ("beta", None)},
        read_completed_units(day),
        read_daily_deterministic_failures(day),
        [],
    )

    assert capped.complete is True
    assert capped.capped_daily_units

    _write_health(journal_copy, day, "002_daily.jsonl", [_complete("beta", ts=3)])
    cleared = mod.evaluate_daily_completion(
        {("alpha", None), ("beta", None)},
        read_completed_units(day),
        read_daily_deterministic_failures(day),
        [],
    )

    assert cleared.complete is True
    assert cleared.capped_daily_units == ()


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
        {
            "day": day,
            "verbose": False,
            "max_concurrency": 2,
            "stream": None,
            "from_scratch": False,
        }
    ]


def test_capped_terminal_completion_clears_updated_days_until_stream_newer(
    journal_copy, monkeypatch
):
    mod = importlib.import_module("solstone.think.thinking")
    day = "20990318"
    health = _prepare_main_day(journal_copy, day)
    _write_health(
        journal_copy,
        day,
        "001_daily.jsonl",
        [
            _complete("alpha"),
            _fail("beta", ts=1, reason_code="context_window_exceeded"),
            _fail("beta", ts=2, reason_code="context_window_exceeded"),
        ],
    )
    (health / "stream.updated").touch()
    dispatched: list[tuple[str, dict]] = []
    _install_daily_mocks(monkeypatch, mod, _single_configs("alpha", "beta"), dispatched)
    monkeypatch.setattr(
        mod, "run_bounded_phase", lambda *_args, **_kwargs: (True, False)
    )
    monkeypatch.setattr(mod, "run_queued_command", lambda *_args, **_kwargs: True)
    monkeypatch.setattr("sys.argv", ["sol think", "--day", day])

    assert day in updated_days()
    mod.main()

    assert dispatched == []
    assert (health / "daily.updated").exists()
    assert day not in updated_days()

    os.utime(health / "daily.updated", (1000, 1000))
    os.utime(health / "stream.updated", (1010, 1010))
    assert day in updated_days()
    monkeypatch.setattr("sys.argv", ["sol think", "--day", day])

    mod.main()

    assert dispatched == []
    assert day not in updated_days()


def test_main_passes_from_scratch_to_daily_prompts(journal_copy, monkeypatch):
    mod = importlib.import_module("solstone.think.thinking")
    day = "20990315"
    _prepare_main_day(journal_copy, day)
    calls = _patch_main(monkeypatch, mod, set())
    monkeypatch.setattr("sys.argv", ["sol think", "--day", day, "--from-scratch"])

    mod.main()

    assert calls == [
        {
            "day": day,
            "verbose": False,
            "max_concurrency": 2,
            "stream": None,
            "from_scratch": True,
        }
    ]
