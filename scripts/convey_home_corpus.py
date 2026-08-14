#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Conformance oracle for the home Pulse surface's derivation and formatting layer.

The home page is an aggregator: almost none of its behaviour is a route contract and
almost all of it is *derivation* -- a phase computed from a clock hour and a segment
count, a duration rendered one way below an hour and another above it, a health
verdict folded from four independent signals, a needs list deduplicated across two
surfaces that can name the same underlying item. A route-level capture records that
those functions were reached. It cannot record what they answered.

So this drives the reference's derivation functions directly, over authored inputs,
and records what came back. The native port asserts against captured reference output
rather than against a re-reading of the reference's source.

⚠ Why a capture and not a hand-written table: a hand-written table encodes the
author's model. Several boundaries here are off-by-one from the obvious reading --
`_compute_briefing_phase` returns `eod` at hour 20 but `morning` at hour 19 with a
briefing and no segments; `_format_duration` switches units at 60 minutes but rounds
to one decimal only above it; `_format_hour_label` drops the meridiem from the start
of a range only when both ends share it. A table written from the prose gets those
wrong and the tests then defend the mistake.

⛔ No value here comes from any real journal, any host, or any clock. Every input is
authored at the call site, and the two functions that read a clock are given a pinned
instant recorded in the fixture. Run it twice and diff before committing.

⚠ Regenerating requires a runnable reference tree, and the conversion removes that
tree. This is a frozen record.

Usage:
    python scripts/convey_home_corpus.py            # write the corpus
    python scripts/convey_home_corpus.py --check    # fail if it would change
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

os.environ["TZ"] = "UTC"
import time  # noqa: E402

if hasattr(time, "tzset"):
    time.tzset()

from datetime import datetime, timezone  # noqa: E402

REPO_ROOT = Path(__file__).resolve().parent.parent
CORPUS_PATH = REPO_ROOT / "core" / "fixtures" / "convey_home_corpus.json"
sys.path.insert(0, str(REPO_ROOT))

from solstone.apps.home import health_glance as hg  # noqa: E402
from solstone.apps.home import needs_you as ny  # noqa: E402
from solstone.apps.home import routes as home  # noqa: E402
from solstone.convey.backlog_source import BacklogSource  # noqa: E402
from solstone.convey.utils import format_date, relative_time  # noqa: E402
from solstone.think.briefing import (  # noqa: E402
    briefing_meeting_count,
    briefing_needs_items,
    render_briefing_sections,
)

# A pinned instant for the two derivations that read a clock. Recorded in the
# fixture so a reader never has to guess which "now" a case was captured against.
PINNED_NOW_UTC = datetime(2026, 5, 14, 15, 30, 0, tzinfo=timezone.utc)
PINNED_NOW_LOCAL = datetime(2026, 5, 14, 15, 30, 0)


def _pin_health_glance_clock() -> None:
    hg._now_utc = lambda: PINNED_NOW_UTC  # noqa: SLF001


def briefing_phase_cases() -> list[dict]:
    """Every hour against both sides of the two other inputs. 96 cases, exhaustive.

    The phase drives which briefing card an owner sees all day, its three thresholds
    interact, and the whole input space is small enough that sampling it would be a
    choice with no upside.
    """
    cases = []
    for segment_count in (0, 1, 7):
        for hour in range(24):
            for exists in (False, True):
                cases.append(
                    {
                        "input": {
                            "segment_count": segment_count,
                            "hour": hour,
                            "briefing_exists": exists,
                        },
                        "output": home._compute_briefing_phase(
                            segment_count, hour, exists
                        ),
                    }
                )
    return cases


def briefing_lateness_cases() -> list[dict]:
    cases = []
    for hour in range(24):
        for phase in ("pending", "morning", "active", "eod"):
            now = PINNED_NOW_LOCAL.replace(hour=hour, minute=17)
            cases.append(
                {
                    "input": {"now_hour": hour, "now_minute": 17, "phase": phase},
                    "output": home._briefing_lateness_state(now, phase),
                }
            )
    return cases


def duration_cases() -> list[dict]:
    minutes = [
        0,
        0.4,
        0.5,
        1,
        59,
        59.4,
        59.5,
        59.6,
        60,
        61,
        89,
        90,
        119,
        120,
        125,
        1439,
        1440,
    ]
    return [
        {"input": {"total_minutes": value}, "output": home._format_duration(value)}
        for value in minutes
    ]


def hour_label_cases() -> list[dict]:
    pairs = [
        (0, 1),
        (0, 12),
        (9, 11),
        (11, 12),
        (11, 13),
        (12, 13),
        (13, 15),
        (23, 24),
        (22, 25),
    ]
    return [
        {
            "input": {"start_hour": start, "end_hour": end},
            "output": home._format_hour_label(start, end),
        }
        for start, end in pairs
    ]


def join_phrase_cases() -> list[dict]:
    inputs = [[], ["a"], ["a", "b"], ["a", "b", "c"], ["a", "b", "c", "d"]]
    return [
        {"input": {"parts": parts}, "output": home._join_phrases(list(parts))}
        for parts in inputs
    ]


def newsletter_summary_cases() -> list[dict]:
    pairs = [(0, 0), (0, 1), (0, 2), (1, 1), (1, 2), (2, 2), (3, 2)]
    return [
        {
            "input": {"successful": successful, "attempted": attempted},
            "output": home._format_newsletter_summary(successful, attempted),
        }
        for successful, attempted in pairs
    ]


def processing_summary_cases() -> list[dict]:
    cases = []
    for mode in ("healthy", "degraded", "sparse"):
        for successful, attempted in ((0, 0), (0, 1), (1, 1), (1, 3), (2, 2)):
            for valid in (False, True):
                briefing = {"exists": True, "valid": valid, "generated_label": None}
                cases.append(
                    {
                        "input": {
                            "mode": mode,
                            "successful_newsletters": successful,
                            "attempted_newsletters": attempted,
                            "briefing_valid": valid,
                        },
                        "output": home._format_processing_summary(
                            mode, successful, attempted, briefing
                        ),
                    }
                )
    return cases


def heatmap_cases() -> list[dict]:
    inputs = [
        {},
        {"heatmap_data": {}},
        {"heatmap_data": {"hours": {}}},
        {"heatmap_data": {"hours": {"9": 0}}},
        {"heatmap_data": {"hours": {"9": 45}}},
        {"heatmap_data": {"hours": {"9": 45, "10": 30, "11": 20}}},
        {"heatmap_data": {"hours": {"9": 45, "14": 30, "20": 20}}},
        {"heatmap_data": {"hours": {"9": 45, "10": 45, "11": 45, "12": 45}}},
        {"heatmap_data": {"hours": {"23": 60, "0": 50, "1": 40}}},
        {"heatmap_data": {"hours": {"bad": 10, "9": "x", "10": 5}}},
    ]
    return [
        {
            "input": {"stats_data": value},
            "top_hours": home._top_heatmap_hours(value),
            "summary": home._format_heatmap_summary(value),
        }
        for value in inputs
    ]


def activity_title_cases() -> list[dict]:
    inputs = [
        {},
        {"description": "  "},
        {"description": "wrote the spec"},
        {"activity": "deep_work"},
        {"activity": "  "},
        {"description": "", "activity": "pair_review"},
    ]
    return [
        {
            "input": {"record": value},
            "output": home._normalize_activity_title(value),
        }
        for value in inputs
    ]


def activity_label_cases() -> list[dict]:
    inputs = [
        {},
        {"title": "wrote the spec", "duration_minutes": 0, "facet": "work"},
        {"title": "wrote the spec", "duration_minutes": 45, "facet": "work"},
        {"title": "wrote the spec", "duration_minutes": 90, "facet": "work"},
        {"title": "wrote the spec", "duration_minutes": 125, "facet": "life"},
    ]
    return [
        {"input": {"activity": value}, "output": home._format_activity_label(value)}
        for value in inputs
    ]


def _briefing(your_day=None, needs=None, yesterday=None, reading=None, forward=None):
    return {
        "metadata": {"generated": "2026-05-14T07:02:00+00:00"},
        "your_day": your_day or [],
        "yesterday": yesterday or [],
        "needs_attention": needs or [],
        "forward_look": forward or [],
        "reading": reading or [],
    }


def briefing_summary_cases() -> list[dict]:
    long_line = "a line that is definitely longer than fifty eight characters in total"
    inputs = [
        (_briefing(), {}, 0),
        (_briefing(your_day=[{"time": "09:00", "text": "standup"}]), {}, 0),
        (_briefing(your_day=[{"time": "09:00", "text": "standup"}]), {}, 1),
        (_briefing(your_day=[{"text": "no time"}]), {}, 2),
        (_briefing(), {"yesterday": "- short line"}, 0),
        (_briefing(), {"yesterday": f"- {long_line}"}, 0),
        (_briefing(), {"yesterday": "\n\n  \n- indented"}, 0),
        (None, {}, 0),
    ]
    return [
        {
            "input": {
                "briefing": briefing,
                "sections": sections,
                "needs_count": needs_count,
            },
            "output": home._briefing_summary(briefing, sections, needs_count),
        }
        for briefing, sections, needs_count in inputs
    ]


def briefing_render_cases() -> list[dict]:
    inputs = [
        _briefing(),
        _briefing(
            your_day=[
                {"time": "09:00", "text": "standup"},
                {"text": "no time"},
                {"time": " ", "text": " "},
                "not a dict",
            ],
            yesterday=["shipped the thing", "  ", 7],
            needs=[{"text": "reply to sam"}, {"text": ""}],
            forward=["draft the memo"],
            reading=[
                {"facet": "work", "summary": "three items"},
                {"facet": "life"},
                {"summary": "orphan summary"},
                {},
            ],
        ),
        {"your_day": "not a list", "needs_attention": None, "reading": 3},
    ]
    return [
        {
            "input": {"briefing": value},
            "sections": render_briefing_sections(value),
            "needs_items": briefing_needs_items(value),
            "meeting_count": briefing_meeting_count(value),
        }
        for value in inputs
    ]


def gap_link_cases() -> list[dict]:
    def summary(anomalies, talents=None):
        return {
            "status": "warning",
            "anomalies": anomalies,
            "talents": talents or {},
        }

    inputs = [
        (summary([]), True),
        (summary([]), False),
        (summary([{"kind": "daily_agents_missing"}]), True),
        (summary([{"kind": "activity_agents_missing"}]), True),
        (
            summary(
                [{"kind": "talent_failure", "name": "facet_newsletter", "use_id": "u1"}]
            ),
            True,
        ),
        (
            summary(
                [
                    {
                        "kind": "talent_failure",
                        "name": "facet_newsletter",
                        "state": "request_lost",
                        "use_id": "u1",
                    }
                ]
            ),
            True,
        ),
        (
            summary(
                [
                    {"kind": "talent_failure", "name": "morning_briefing", "use_id": "a"},
                    {"kind": "talent_failure", "name": "morning_briefing", "use_id": "b"},
                ]
            ),
            True,
        ),
        (
            summary(
                [{"kind": "talent_failure", "name": "x", "use_id": "u"}],
                {"failed_list_truncated": True, "outstanding_failed": 5},
            ),
            True,
        ),
        (summary([{"kind": "talent_failure", "name": "  "}]), True),
    ]
    return [
        {
            "input": {"pipeline_summary": value, "briefing_valid": valid},
            "output": home._format_gap_links(
                value, {"valid": valid}, "20260513", "20260514"
            ),
        }
        for value, valid in inputs
    ]


def needs_you_cases() -> list[dict]:
    attentions = [
        None,
        {"placeholder_text": "the invoice from sam"},
        {"placeholder_text": "  "},
    ]
    pulse_needs_sets = [
        [],
        ["a plain string need"],
        [{"text": "chat need", "kind": "chat", "payload": {"prompt": "dig in"}}],
        [{"text": "chat need", "kind": "chat", "payload": {}}],
        [{"text": "confirm need", "kind": "confirm", "payload": {}}],
        [{"text": "route need", "kind": "route", "payload": {"href": "/app/health"}}],
        [{"text": "bad route", "kind": "route", "payload": {"href": "//evil"}}],
        [{"text": "bad route", "kind": "route", "payload": {"href": "http://evil"}}],
        [{"text": "unknown", "kind": "nope", "payload": {}}],
        [{"kind": "chat", "payload": {}}],
    ]
    cases = []
    for attention in attentions:
        for needs in pulse_needs_sets:
            items = ny.classify_needs_you(attention, list(needs))
            cases.append(
                {
                    "input": {"attention": attention, "pulse_needs": needs},
                    "output": [item.to_dict() for item in items],
                }
            )
    return cases


def dedup_key_cases() -> list[dict]:
    inputs = [
        "A Plain   String",
        {"text": "with source", "source_id": "ent:abc"},
        {"text": "blank source", "source_id": "   "},
        {"text": "with href", "payload": {"href": "/app/entities#x"}},
        {"text": "sol source [sol:day/20260514]"},
        {"placeholder_text": "from placeholder"},
        {},
    ]
    return [
        {"input": {"item": value}, "output": ny.needs_dedup_key(value)}
        for value in inputs
    ]


def degraded_capture_cases() -> list[dict]:
    inputs = [
        None,
        {},
        {"status": "active"},
        {"status": "degraded"},
        {"status": "degraded", "observers": [{"name": "laptop"}]},
    ]
    return [
        {
            "input": {"capture_health": value},
            "output": ny.format_degraded_capture_line(value),
        }
        for value in inputs
    ]


def health_glance_cases() -> list[dict]:
    stale_stamp = "2026-05-10T00:00:00+00:00"
    fresh_stamp = "2026-05-14T12:00:00+00:00"
    backlogs = [
        ("missing", BacklogSource(backlog=None, validity="missing", generated_at=None)),
        (
            "unparseable",
            BacklogSource(backlog=None, validity="unparseable", generated_at=None),
        ),
        (
            "valid_fresh_clear",
            BacklogSource(
                backlog={"stuck_days": 0, "degraded": False},
                validity="valid",
                generated_at=fresh_stamp,
            ),
        ),
        (
            "valid_stale_clear",
            BacklogSource(
                backlog={"stuck_days": 0},
                validity="valid",
                generated_at=stale_stamp,
            ),
        ),
        (
            "valid_no_stamp",
            BacklogSource(
                backlog={"stuck_days": 0}, validity="valid", generated_at=None
            ),
        ),
        (
            "valid_degraded",
            BacklogSource(
                backlog={"stuck_days": 0, "degraded": True},
                validity="valid",
                generated_at=fresh_stamp,
            ),
        ),
        (
            "valid_stuck",
            BacklogSource(
                backlog={
                    "stuck_days": 2,
                    "stuck_day_rows": [
                        {"day": "20260512", "reason": "waiting on the model"}
                    ],
                },
                validity="valid",
                generated_at=fresh_stamp,
            ),
        ),
    ]
    captures = [
        ("none", None),
        ("no_observers", {"status": "no_observers", "observers": []}),
        ("active", {"status": "active", "observers": [{"name": "laptop"}]}),
        ("stale", {"status": "stale", "observers": [{"name": "laptop"}]}),
        ("offline", {"status": "offline", "observers": [{"name": "laptop"}]}),
        ("degraded", {"status": "degraded", "observers": [{"name": "laptop"}]}),
        ("unknown", {"status": "unknown", "observers": []}),
    ]
    pipelines = [
        ("none", None),
        ("empty", {}),
        ("warning", {"status": "warning", "message": "processing is behind"}),
        (
            "headline",
            {"status": "warning", "headline": "three runs did not finish"},
        ),
        (
            "support",
            {
                "status": "warning",
                "headline": "three runs did not finish",
                "suggested_action": "open_support",
            },
        ),
    ]
    brains = [
        ("none", None),
        ("ready", {"state": "ready", "headline": "thinking is ready"}),
        ("checking", {"state": "checking", "headline": "checking thinking"}),
        (
            "blocked_progressing",
            {"state": "blocked", "headline": "installing", "progressing": True},
        ),
        ("blocked", {"state": "blocked", "headline": "no provider configured"}),
        ("unhealthy", {"state": "unhealthy", "headline": "the model failed"}),
        (
            "unknown_with_action",
            {
                "state": "unknown",
                "headline": "thinking status unavailable",
                "action": {"label": "check again", "href": "/app/thinking/"},
            },
        ),
    ]
    cases = []
    for backlog_name, backlog in backlogs:
        for capture_name, capture in captures:
            for pipeline_name, pipeline in pipelines:
                for brain_name, brain in brains:
                    cases.append(
                        {
                            "input": {
                                "backlog": backlog_name,
                                "capture": capture_name,
                                "pipeline": pipeline_name,
                                "brain": brain_name,
                                "last_observe_relative": "2 minutes ago",
                            },
                            "output": hg.build_health_glance(
                                capture,
                                pipeline,
                                "2 minutes ago",
                                backlog=backlog,
                                brain=brain,
                            ),
                        }
                    )
    return cases


def convey_util_cases() -> list[dict]:
    days = ["20260101", "20260102", "20260103", "20260111", "20260112", "20260113",
            "20260121", "20260122", "20260123", "20260511", "20261231", "notaday"]
    seconds = [0, 1, 59, 60, 61, 3599, 3600, 3601, 86399, 86400, 86401, 604800]
    return [
        {
            "format_date": [{"day": day, "output": format_date(day)} for day in days],
            "relative_time": [
                {"seconds": value, "output": relative_time(value)} for value in seconds
            ],
        }
    ]


def build() -> dict:
    _pin_health_glance_clock()
    return {
        "generator": "scripts/convey_home_corpus.py",
        "pinned_now_utc": PINNED_NOW_UTC.isoformat(),
        "tz": "UTC",
        "note": (
            "Derivation and formatting oracle for the home Pulse surface. Every input "
            "is authored; nothing here comes from a journal, a host, or a clock. The "
            "two derivations that read a clock were driven against pinned_now_utc."
        ),
        "coverage_limits": {
            "not_covered": [
                "route status codes and the session gate, which the shell corpus covers",
                "every journal reader, which is filesystem state rather than derivation",
                "the payload assembly order in _build_pulse_context",
            ],
            "why": (
                "This oracle records what the derivation layer ANSWERS. A green replay "
                "is not evidence about any reader, any route, or the assembly."
            ),
        },
        "cases": {
            "briefing_phase": briefing_phase_cases(),
            "briefing_lateness": briefing_lateness_cases(),
            "duration": duration_cases(),
            "hour_label": hour_label_cases(),
            "join_phrases": join_phrase_cases(),
            "newsletter_summary": newsletter_summary_cases(),
            "processing_summary": processing_summary_cases(),
            "heatmap": heatmap_cases(),
            "activity_title": activity_title_cases(),
            "activity_label": activity_label_cases(),
            "briefing_summary": briefing_summary_cases(),
            "briefing_render": briefing_render_cases(),
            "gap_links": gap_link_cases(),
            "needs_you": needs_you_cases(),
            "dedup_key": dedup_key_cases(),
            "degraded_capture": degraded_capture_cases(),
            "health_glance": health_glance_cases(),
            "convey_utils": convey_util_cases(),
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()

    corpus = build()
    rendered = json.dumps(corpus, indent=2, sort_keys=True) + "\n"

    sys.path.insert(0, str(REPO_ROOT / "scripts"))
    import corpus_scrub

    corpus_scrub.assert_guard_can_see("convey_home_corpus")
    corpus_scrub.assert_publishable(rendered, label="convey_home_corpus")

    if args.check:
        if not CORPUS_PATH.exists():
            print(f"{CORPUS_PATH} is missing", file=sys.stderr)
            return 1
        if CORPUS_PATH.read_text(encoding="utf-8") != rendered:
            print(f"{CORPUS_PATH} would change", file=sys.stderr)
            return 1
        counts = {name: len(cases) for name, cases in corpus["cases"].items()}
        print(f"convey_home_corpus is current: {counts}")
        return 0

    CORPUS_PATH.parent.mkdir(parents=True, exist_ok=True)
    CORPUS_PATH.write_text(rendered, encoding="utf-8")
    counts = {name: len(cases) for name, cases in corpus["cases"].items()}
    print(f"wrote {CORPUS_PATH} {counts}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
