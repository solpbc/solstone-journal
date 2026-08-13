#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Conformance oracle for the timeline, news, curation and awareness surfaces.

These four app surfaces plus the six activity-record routes that back the native
``sol activities`` grammar are being rebuilt in Rust. Every acceptance criterion
the rebuild could write for itself would restate what the Rust does, never
evidence that it matches what the reference served. This module drives the
reference Flask application over a fixed probe list and records what came back.

What it pins, deliberately:

  * **admission** — each probe records the status the reference returned, so a
    case also proves whether the session gate admitted it. The read probe list
    runs against four journal states, because the gate has three outcomes and
    a read surface over an empty journal proves close to nothing:

      ``unestablished``       no config at all; the gate's first-run branch
      ``corrupt``             config exists and cannot be parsed; the gate's
                              THIRD outcome, which an implementation written
                              ``unwrap_or(false)`` collapses into the first and
                              thereby tells an owner with a real journal that
                              they never set one up
      ``established_empty``   config only; every list is empty and every
                              coverage window is absent
      ``populated``           chronicle segments across two streams, a master
                              rollup, facet newsletters, facet review
                              candidates, activity definitions and records, and
                              awareness state

  * **shape** — the full canonical JSON body per case. ⛔ Not a route/method
    coverage table: a coverage oracle records that a route ANSWERED and cannot
    see WHAT it answered, so a map returned where the reference returns a list
    reads as covered. Every field the reference publishes is in here.

  * **bytes** — the sha256 and length of the body, recorded even where the body
    was normalized, so a structural match that is not a byte match is still
    detectable.

  * **writes** — a fifth ``mutations`` phase drives the write routes in a fixed
    order against their own journal and records both the response and the state
    the next read returns. A read-only corpus over a surface with writers pins
    half a contract.

⚠ This corpus has a clock. Regenerating it requires a runnable reference tree
and this lane deletes that tree. It is a frozen record, not a live comparison.
That is why it is captured before the rebuild rather than after it, and why an
unreproducible value is NAMED rather than dropped.

🔴 **``SOLSTONE_DISABLE_CONVEY_SIDE_RUNTIMES=1`` is set here, as in every convey
corpus generator, and it means this corpus pins the side-runtime-ABSENT branch.**
A field a live runtime populates is therefore NOT graded by this file and needs a
derivation test over injected inputs instead. The four surfaces here are
filesystem projections and none of them reads a side runtime — checked, not
assumed — so the exposure is limited to the shell chrome they do not serve. ⛔ Do
not widen that claim to a surface added later without re-checking it.

⛔ The journal this drives is built by the generator in a temporary directory and
contains no owner data. Never point it at a real journal: the probes read app
state, and a recorded body would carry it into a tracked, PUBLIC file.

⛔ **Normalization is a PATH ALLOWLIST, never a shape test.** A ``^\\d{8}$`` rule
would eat every ``day`` value, both coverage bounds and every segment key the
moment the journal had content — and an implementation returning the WRONG
coverage window would still match, because both sides normalize to the same
placeholder. Each allowlist entry names one probe path and one dotted field.

Determinism: ``TZ`` is pinned to UTC before any solstone import, because the
reference derives ``today`` from the process timezone.

Usage:
    python scripts/convey_facets_corpus.py            # write the corpus
    python scripts/convey_facets_corpus.py --check    # fail if it would change
"""

from __future__ import annotations

import argparse
import datetime as _datetime
import hashlib
import json
import os
import re
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

# Pinned before any solstone import so module-level datetime work sees it too.
os.environ["TZ"] = "UTC"
if hasattr(time, "tzset"):
    time.tzset()


# ---------------------------------------------------------------------------
# 🔴 THE CLOCK IS INJECTED, NOT READ.
#
# Three fields of this surface are derived from the wall clock, and one of them
# changes the SHAPE of a response rather than a value: `/api/overview` spans its
# `months` array from the earliest month with data to `max(latest_month,
# CURRENT month)`, so the array grows by one entry every calendar month. A
# corpus captured on the ambient clock therefore freezes an array length that
# the replay cannot reproduce in any later month -- and the replay would go red
# on the calendar with nothing having changed, which is how a check earns being
# ignored.
#
# The fix is not to normalize those fields away; normalizing an array's
# MEMBERSHIP is not possible and normalizing `today` would delete the only pin
# on the branch. The fix is to hand the reference a clock and record which one,
# so the replay establishes the same condition rather than inheriting the host's.
#
# ⚠ `time.monotonic` is deliberately NOT frozen: `think/journal_io/locking.py`
# and `lease.py` compute their deadlines from it, and a frozen monotonic clock
# turns every lock timeout into an infinite wait.
# ---------------------------------------------------------------------------

CAPTURE_CLOCK_ISO = "2026-05-15T12:00:00+00:00"
_CAPTURE_INSTANT = _datetime.datetime.fromisoformat(CAPTURE_CLOCK_ISO)
_CAPTURE_EPOCH = _CAPTURE_INSTANT.timestamp()

_REAL_DATE = _datetime.date
_REAL_DATETIME = _datetime.datetime


class _InjectedDate(_REAL_DATE):
    @classmethod
    def today(cls) -> "_InjectedDate":
        return cls(_CAPTURE_INSTANT.year, _CAPTURE_INSTANT.month, _CAPTURE_INSTANT.day)


class _InjectedDateTime(_REAL_DATETIME):
    @classmethod
    def now(cls, tz: Any = None) -> "_InjectedDateTime":
        return cls.fromtimestamp(_CAPTURE_EPOCH, tz)

    @classmethod
    def utcnow(cls) -> "_InjectedDateTime":
        return cls.fromtimestamp(_CAPTURE_EPOCH, _datetime.timezone.utc).replace(
            tzinfo=None
        )

    @classmethod
    def today(cls) -> "_InjectedDateTime":
        return cls.now()


def _install_injected_clock() -> None:
    """Install the pinned clock before any solstone, flask or weasyprint import.

    ⛔ Must run at module import time. A module that did ``from datetime import
    date`` before this point keeps the real class, and a half-frozen capture is
    worse than an unfrozen one because part of it looks reproducible.
    """
    _datetime.date = _InjectedDate
    _datetime.datetime = _InjectedDateTime
    time.time = lambda: _CAPTURE_EPOCH  # type: ignore[assignment]
    time.time_ns = lambda: int(_CAPTURE_EPOCH * 1_000_000_000)  # type: ignore[assignment]


_install_injected_clock()


# ---------------------------------------------------------------------------
# 🔴 EGRESS IS FORBIDDEN, AND THAT IS PROVEN HERE RATHER THAN ASSUMED.
#
# "Driving the reference to capture a corpus is read-only" is an ASSUMPTION, not
# a property of the reference. A sibling lane probed one app route on a
# throwaway journal and the handler generated a keypair, signed a live document
# and POSTed a registration to a PRODUCTION service.
#
# ⚠ **The unit of analysis is the BLUEPRINT, not the route.** A drain registered
# on both `before_request` and `after_request` runs on refusals too, so auditing
# the routes you intend to probe does not bound what probing them reaches.
# Audited here first, and this surface is filesystem-only: the support drain is
# `@support_bp.before_request`/`@support_bp.after_request`, which are
# BLUEPRINT-scoped and cannot fire for a `/app/timeline`, `/app/news`,
# `/app/curation`, `/app/awareness` or `/app/activities` probe; the only
# app-wide hooks are `convey/root.py`'s access and loopback-origin guards and
# the request-id filter, none of which egress.
#
# ⛔ None of that audit is why this block exists, and saying so matters: a right
# practice held for the wrong reason does not survive the day someone decides
# the reason no longer applies. It exists because the check has to be
# STRUCTURAL -- a route added later, or a lazily-imported client, silently
# invalidates every sentence above, and the failure mode is a real request to a
# real service.
#
# 🔴 And a byte-identical capture is NOT sufficient proof: a route can attempt
# an outbound call, SWALLOW the exception and answer unchanged, so the diff
# passes while the UNGUARDED run egressed. The decisive instrument is the
# attempt LOG asserted empty, which is why this uses the shared module rather
# than a local guard that only raises.
# ---------------------------------------------------------------------------

sys.path.insert(0, str(Path(__file__).resolve().parent))

from corpus_scrub import (  # noqa: E402
    assert_egress_guard_can_see,
    assert_no_egress_attempted,
    assert_publishable,
    forbid_non_loopback_egress,
)

# The two destinations the guard's own positive control provokes. Anything else
# in the attempt log is a reference route reaching out.
CONTROL_DESTINATIONS = ("example.invalid", "198.51.100.7")

forbid_non_loopback_egress()
assert_egress_guard_can_see(__file__)

# Positive control on the injection itself. ⛔ Never assume the patch took: a
# clock that failed to install produces a corpus that looks completely normal
# and is reproducible on exactly one day.
assert _datetime.date.today().isoformat() == "2026-05-15", (
    "the injected clock did not install; every clock-derived field in this "
    "capture would be an artifact of the capture host's calendar"
)
assert int(time.time()) == int(_CAPTURE_EPOCH), "time.time was not injected"

REPO_ROOT = Path(__file__).resolve().parent.parent
CORPUS_PATH = REPO_ROOT / "core" / "fixtures" / "convey_facets_corpus.json"

SCHEMA = "solstone-convey-facets-corpus-v1"

# A fixed instant so `setup.completed_at` is reproducible across regenerations.
# 2026-01-01T00:00:00Z -- before any journal this corpus will ever describe.
PINNED_COMPLETED_AT = 1767225600

# The seeded content day and month. Fixed, in the past, and never "today", so a
# probe against a real coverage window can never accidentally match the clock.
SEED_DAY = "20260510"
SEED_MONTH = "202605"
SEED_PREV_MONTH = "202604"
SEED_FACET = "work"
SEED_OTHER_FACET = "personal"
# The second seeded newsletter day, used to prove multi-day ordering.
SEED_DAY_2 = "20260503"

PLACEHOLDER_DAY = "<TODAY>"
PLACEHOLDER_NOW = "<NOW>"
PLACEHOLDER_ROOT = "<JOURNAL_ROOT>"
PLACEHOLDER_CLOCK = "<CAPTURE_CLOCK>"
PLACEHOLDER_ACTIVITY_ID = "<ACTIVITY_ID>"

DAY_PATTERN = re.compile(r"^\d{8}$")
ISO_PATTERN = re.compile(r"^\d{4}-\d{2}-\d{2}T")
COMPACT_ISO_PATTERN = re.compile(r"^\d{8}T\d{2}:\d{2}:\d{2}$")


# ---------------------------------------------------------------------------
# Probes
#
# (method, path, why this probe is in the corpus)
#
# ⛔ The routes this lane DELETES are deliberately absent. `/app/activities/`,
# `/app/activities/<day>`, `/app/activities/api/index`,
# `/app/activities/api/stats/<month>`, `/app/activities/api/day/<day>/activities`
# and `/app/activities/api/activity_output/<path>` are the activities WEB UI,
# which is dropped by ruling, and every `/app/reflections/*` route is dropped
# entirely. Recording them here would freeze an expectation for a surface that
# is not coming back, and a fixture row reads as authoritative. They are
# enumerated in the lane's close-out instead.
# ---------------------------------------------------------------------------

READ_PROBES: list[tuple[str, str, str]] = [
    # -- timeline ---------------------------------------------------------
    ("GET", "/app/timeline/", "the app index: a redirect whose target is today"),
    ("GET", "/app/timeline/workspace", "the app fragment bytes"),
    (
        "GET",
        "/app/timeline/background",
        "timeline is the only app in this plate with a background badge",
    ),
    ("GET", "/app/timeline/year", "the year view serves the shell verbatim"),
    ("GET", f"/app/timeline/{SEED_DAY}", "a day view: shell, not a payload"),
    (
        "GET",
        f"/app/timeline/{SEED_MONTH}",
        "a month view: same shell, different route arm",
    ),
    (
        "GET",
        "/app/timeline/notaday",
        "⚠ an EMPTY-BODY 404, not the shared HTML 404 page",
    ),
    ("GET", "/app/timeline/static/timeline.js", "per-app static"),
    ("GET", "/app/timeline/static/timeline.css", "per-app static"),
    ("GET", "/app/timeline/static/timeline_provenance.js", "per-app static"),
    (
        "GET",
        "/app/timeline/api/overview",
        "the index payload: months meta plus the rollup watermark",
    ),
    ("GET", "/app/timeline/api/grid", "the day-grid framework contract"),
    ("GET", "/app/timeline/api/index", "the date-nav framework contract"),
    ("GET", f"/app/timeline/api/stats/{SEED_MONTH}", "per-month segment counts"),
    (
        "GET",
        f"/app/timeline/api/stats/{SEED_PREV_MONTH}",
        "a month with no data: the empty answer, not a 404",
    ),
    (
        "GET",
        "/app/timeline/api/stats/20260",
        "a malformed month: invalid_month at its reference status",
    ),
    (
        "GET",
        f"/app/timeline/api/month/{SEED_MONTH}",
        "a month present in the master rollup",
    ),
    (
        "GET",
        "/app/timeline/api/month/209912",
        "a month absent from the rollup: 404 timeline_month_not_found",
    ),
    ("GET", "/app/timeline/api/month/bad", "a malformed month on the month route: 400"),
    (
        "GET",
        f"/app/timeline/api/day/{SEED_DAY}",
        "the day payload: rollup fields plus the availability grid",
    ),
    (
        "GET",
        "/app/timeline/api/day/20260511",
        "a day with no segments and no rollup entry",
    ),
    ("GET", "/app/timeline/api/day/bad", "a malformed day: 400"),
    (
        "GET",
        f"/app/timeline/api/segment/{SEED_DAY}/100000_300",
        "a default-stream segment carrying audio and screen",
    ),
    (
        "GET",
        f"/app/timeline/api/segment/{SEED_DAY}/workstation.browser/140000_300",
        "a named-stream segment carrying a browser file",
    ),
    (
        "GET",
        f"/app/timeline/api/segment/{SEED_DAY}/999999_300",
        "a segment directory that does not exist: 200 with an error field",
    ),
    (
        "GET",
        "/app/timeline/api/segment/bad/100000_300",
        "a malformed day on the segment route: 400",
    ),
    (
        "GET",
        f"/app/timeline/api/segment/{SEED_DAY}/notakey",
        "a malformed segment key: 400",
    ),
    # -- news -------------------------------------------------------------
    ("GET", "/app/news/", "the app index serves the shell"),
    ("GET", "/app/news/workspace", "the app fragment bytes"),
    ("GET", "/app/news/background", "news declares no background: 404"),
    (
        "GET",
        "/app/news/api/state",
        "the initial-state payload, including every copy string",
    ),
    ("GET", "/app/news/api/index", "the date-nav framework contract"),
    ("GET", "/app/news/api/grid", "the day-grid framework contract"),
    ("GET", f"/app/news/api/stats/{SEED_MONTH}", "per-month newsletter counts"),
    ("GET", "/app/news/api/stats/bad", "a malformed month: invalid_month"),
    ("GET", f"/app/news/api/day/{SEED_DAY}", "a day carrying two facets' newsletters"),
    ("GET", "/app/news/api/day/20260511", "a day with no newsletter: the empty flag"),
    ("GET", "/app/news/api/day/bad", "a malformed day: 404 invalid_day"),
    (
        "GET",
        f"/app/news/api/facet/{SEED_FACET}",
        "the CLI-facing facet feed, default paging",
    ),
    (
        "GET",
        f"/app/news/api/facet/{SEED_FACET}?limit=1",
        "the feed's cursor emission at a page boundary",
    ),
    (
        "GET",
        f"/app/news/api/facet/{SEED_FACET}?day={SEED_DAY}",
        "the feed pinned to one day",
    ),
    ("GET", "/app/news/api/facet/nope", "a facet with no news directory"),
    (
        "GET",
        "/app/news/api/facet/bad!facet",
        "a malformed facet: invalid_request_value",
    ),
    (
        "GET",
        f"/app/news/api/facet/{SEED_FACET}?limit=0",
        "limit below the accepted range",
    ),
    ("GET", f"/app/news/api/facet/{SEED_FACET}?limit=nope", "a non-integer limit"),
    ("GET", "/app/news/sample", "the sample view serves the shell"),
    ("GET", "/app/news/api/sample", "the built-in sample newsletter payload"),
    ("GET", "/app/news/sample/raw", "the sample as text/markdown"),
    ("GET", f"/app/news/{SEED_DAY}", "a day view serves the shell"),
    ("GET", f"/app/news/{SEED_FACET}/{SEED_DAY}", "a detail view serves the shell"),
    (
        "GET",
        f"/app/news/api/{SEED_FACET}/{SEED_DAY}",
        "the detail payload including its pdf_url",
    ),
    (
        "GET",
        f"/app/news/api/{SEED_FACET}/20991231",
        "a missing newsletter: the EMPTY payload, not a 404",
    ),
    (
        "GET",
        "/app/news/api/bad!facet/20260510",
        "a malformed facet on detail: file_not_found",
    ),
    (
        "GET",
        f"/app/news/{SEED_FACET}/{SEED_DAY}/raw",
        "the raw markdown, frontmatter included",
    ),
    (
        "GET",
        f"/app/news/{SEED_FACET}/20991231/raw",
        "a missing newsletter raw: plain-text 404",
    ),
    (
        "GET",
        f"/app/news/{SEED_FACET}/{SEED_DAY}/pdf",
        "the PDF export: headers and disposition are the contract",
    ),
    (
        "GET",
        f"/app/news/{SEED_FACET}/20991231/pdf",
        "a missing newsletter PDF: plain-text 404",
    ),
    # -- curation ---------------------------------------------------------
    ("GET", "/app/curation/", "the app index serves the shell"),
    ("GET", "/app/curation/workspace", "the app fragment bytes"),
    ("GET", "/app/curation/static/curation_evidence.js", "per-app static"),
    (
        "GET",
        "/app/curation/api/state",
        "every open curation item across all five kinds",
    ),
    (
        "GET",
        "/app/curation/api/facet/candidates",
        "the CLI-facing facet candidate list",
    ),
    # -- awareness (API-only: no workspace, no menu entry, index is a 404) --
    (
        "GET",
        "/app/awareness/",
        "⚠ deliberately a 404: awareness has no owner-visible page",
    ),
    ("GET", "/app/awareness/api/state", "the whole awareness state"),
    ("GET", "/app/awareness/api/state?section=imports", "a section projection"),
    (
        "GET",
        "/app/awareness/api/state?section=nope",
        "an unknown section: awareness_section_not_found",
    ),
    ("GET", "/app/awareness/api/imports", "import tracking, defaults included"),
    ("GET", "/app/awareness/api/log", "the daily log, default paging"),
    ("GET", f"/app/awareness/api/log?day={SEED_DAY}", "a named day's log"),
    ("GET", f"/app/awareness/api/log?day={SEED_DAY}&kind=state", "the kind filter"),
    (
        "GET",
        f"/app/awareness/api/log?day={SEED_DAY}&limit=1&offset=1",
        "paging inside a day",
    ),
    ("GET", "/app/awareness/api/log?day=20991231", "a day with no log file"),
    # -- activities: the SIX routes that back the native `sol activities` --
    (
        "GET",
        f"/app/activities/api/day/{SEED_DAY}/records?facet={SEED_FACET}",
        "`sol activities list`, one facet",
    ),
    (
        "GET",
        f"/app/activities/api/day/{SEED_DAY}/records",
        "`sol activities list` with no facet: every facet",
    ),
    (
        "GET",
        f"/app/activities/api/day/{SEED_DAY}/records?facet={SEED_FACET}&include_hidden=1",
        "muted records are excluded by default",
    ),
    (
        "GET",
        f"/app/activities/api/day/20260511/records?facet={SEED_FACET}",
        "a day with no records",
    ),
    (
        "GET",
        f"/app/activities/api/day/{SEED_DAY}/record/nosuchid?facet={SEED_FACET}",
        "`sol activities get` on a missing id: activity_not_found",
    ),
]

# Read probes whose target only exists once content is seeded. Recorded in every
# phase anyway -- a route that answers differently on an empty journal is exactly
# what the empty phase is for.
POPULATED_ONLY_PROBES: list[tuple[str, str, str]] = []


# The mutation replay. Ordered, cumulative, against its own populated journal.
# Each entry is (method, path, json body or None, why).
MUTATION_PROBES: list[tuple[str, str, Any, str]] = [
    (
        "POST",
        "/app/awareness/api/imports",
        {"record": "ics"},
        "record an import: writes current.json AND appends a log entry",
    ),
    (
        "GET",
        "/app/awareness/api/imports",
        None,
        "the state the previous write left behind",
    ),
    (
        "POST",
        "/app/awareness/api/imports",
        {"declined": True},
        "decline an import offer",
    ),
    (
        "POST",
        "/app/awareness/api/imports",
        {"nudge": True},
        "record a nudge",
    ),
    (
        "POST",
        "/app/awareness/api/imports",
        {},
        "no action selected: invalid_request_value naming all three",
    ),
    (
        "POST",
        "/app/awareness/api/imports",
        {"record": "ics", "nudge": True},
        "two actions selected: the refusal NAMES which ones",
    ),
    (
        "POST",
        "/app/awareness/api/log",
        {"kind": "observation", "message": "corpus entry", "key": "corpus.one"},
        "append a log entry: 201 created",
    ),
    (
        "POST",
        "/app/awareness/api/log",
        {"message": "no kind"},
        "a log entry with no kind: missing_required_field",
    ),
    (
        "GET",
        "/app/awareness/api/state",
        None,
        "the whole state after every awareness write",
    ),
    (
        "POST",
        f"/app/activities/api/day/{SEED_DAY}/records?facet={SEED_FACET}",
        {"title": "Corpus activity", "activity": "meeting", "source": "user"},
        "`sol activities create`, argv shape",
    ),
    (
        "POST",
        f"/app/activities/api/day/{SEED_DAY}/records?facet={SEED_FACET}",
        {"title": "", "activity": "meeting"},
        "an empty title: activity_invalid",
    ),
    (
        "POST",
        f"/app/activities/api/day/{SEED_DAY}/records?facet={SEED_FACET}",
        {"title": "Bad activity", "activity": "nosuchactivity"},
        "an unknown activity type: activity_not_found",
    ),
    (
        "POST",
        f"/app/activities/api/day/{SEED_DAY}/records?facet={SEED_FACET}",
        {"title": "Bad source", "activity": "meeting", "source": "robot"},
        "a source outside {user, cogitate}: activity_invalid",
    ),
    (
        "POST",
        f"/app/activities/api/day/{SEED_DAY}/record/{{created}}/update?facet={SEED_FACET}",
        {"patch": {"title": "Corpus activity, renamed"}, "note": "corpus update"},
        "`sol activities update`: the edit trail grows",
    ),
    (
        "POST",
        f"/app/activities/api/day/{SEED_DAY}/record/{{created}}/mute?facet={SEED_FACET}",
        {"reason": "corpus mute"},
        "`sol activities mute`",
    ),
    (
        "GET",
        f"/app/activities/api/day/{SEED_DAY}/records?facet={SEED_FACET}",
        None,
        "a muted record is absent from the default list",
    ),
    (
        "GET",
        f"/app/activities/api/day/{SEED_DAY}/records?facet={SEED_FACET}&include_hidden=1",
        None,
        "and present with include_hidden=1",
    ),
    (
        "POST",
        f"/app/activities/api/day/{SEED_DAY}/record/{{created}}/unmute?facet={SEED_FACET}",
        {"reason": "corpus unmute"},
        "`sol activities unmute`",
    ),
    (
        "POST",
        f"/app/activities/api/day/{SEED_DAY}/record/nosuchid/mute?facet={SEED_FACET}",
        {},
        "mute on a missing record: activity_not_found",
    ),
    (
        "GET",
        f"/app/activities/api/day/{SEED_DAY}/record/{{created}}?facet={SEED_FACET}",
        None,
        "`sol activities get` on the record this replay built",
    ),
    (
        "POST",
        "/app/curation/api/facet/accept",
        {"name_key": "atlas"},
        "`sol facets accept`: promote a facet candidate",
    ),
    (
        "POST",
        "/app/curation/api/facet/dismiss",
        {"name_key": "ledger"},
        "`sol facets dismiss`",
    ),
    (
        "POST",
        "/app/curation/api/facet/accept",
        {},
        "a missing name_key: missing_required_field",
    ),
    (
        "POST",
        "/app/curation/api/facet/accept",
        {"name_key": "nosuchcandidate"},
        "accepting a candidate that is not there",
    ),
    (
        "GET",
        "/app/curation/api/facet/candidates",
        None,
        "the candidate list after both decisions",
    ),
    (
        "GET",
        "/app/curation/api/state",
        None,
        "the whole curation surface after both decisions",
    ),
]


# 🔴 PATH ALLOWLIST — and it is EMPTY, deliberately.
#
# Every field on this surface that a first draft wanted to normalize was
# clock-derived: `/api/overview`'s `now` and `today` and the SPAN of its `months`
# array, `/api/grid`'s `coverage.end`, and every `ts`, `created_at`,
# `last_nudge`, `offer_declined` and `last_completed` the reference stamps at
# write time. Injecting the clock (see the block at the top of this file) makes
# all of them reproducible, so they are recorded VERBATIM and the replay
# establishes the same clock rather than being handed a placeholder.
#
# ⛔ That is the whole point: a placeholder pins nothing, and `<TODAY>` in place
# of a real coverage bound is exactly how an implementation returning the WRONG
# window still matches. Adding an entry here means conceding a field cannot be
# established at the call site -- name why, in the entry, when that happens.
#
# The one value that genuinely cannot be established is the journal root: it is
# a temporary directory, and it is substituted before hashing rather than after,
# so the recorded hash is over what the corpus actually asserts.
NORMALIZED_FIELDS: dict[str, set[str]] = {}


def _normalize(
    value: Any,
    found: set[str],
    allowed: set[str],
    path: str = "",
) -> Any:
    """Replace allowlisted non-reproducible scalars, recording each one."""
    if isinstance(value, dict):
        return {
            key: _normalize(item, found, allowed, f"{path}.{key}" if path else key)
            for key, item in value.items()
        }
    if isinstance(value, list):
        return [_normalize(item, found, allowed, f"{path}[]") for item in value]
    if path not in allowed:
        return value
    if isinstance(value, str):
        if DAY_PATTERN.match(value):
            found.add(path)
            return PLACEHOLDER_DAY
        if ISO_PATTERN.match(value):
            found.add(path)
            return PLACEHOLDER_NOW
        if COMPACT_ISO_PATTERN.match(value):
            found.add(path)
            return PLACEHOLDER_CLOCK
        found.add(path)
        return PLACEHOLDER_CLOCK
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        found.add(path)
        return PLACEHOLDER_CLOCK
    return value


# ---------------------------------------------------------------------------
# Seeding
#
# 🔴 Where a record is self-validating or carries derived identity, the
# REFERENCE'S OWN WRITER seeds it. A hand-assembled record pins the
# invalid-record branch of every route while looking fully populated, and the
# fixture then looks green because something was broken.
# ---------------------------------------------------------------------------

MASTER_ROLLUP: dict[str, Any] = {
    "generated_at": 1770000000,
    "model": "corpus-model",
    "top_n": 4,
    "year_top": [
        {
            "month": SEED_MONTH,
            "title": "Timeline port",
            "description": "The month the corpus describes.",
            "origin": f"{SEED_DAY}/100000_300",
        }
    ],
    "months": {
        SEED_PREV_MONTH: {
            "month_top": [],
            "month_rationale": "",
            "day_count": 0,
            "days_with_data": [],
            "days": {},
        },
        SEED_MONTH: {
            "month_top": [
                {
                    "title": "Timeline port",
                    "description": "The month the corpus describes.",
                    "origin": f"{SEED_DAY}/100000_300",
                }
            ],
            "month_rationale": "One seeded day with two streams.",
            "day_count": 1,
            "days_with_data": [SEED_DAY],
            "days": {
                SEED_DAY: {
                    "generated_at": 1770000100,
                    "model": "corpus-day-model",
                    "day_top": [
                        {
                            "title": "Both streams",
                            "description": "A default-stream segment with audio and screen.",
                            "origin": f"{SEED_DAY}/100000_300",
                        }
                    ],
                    "day_rationale": "One seeded day.",
                    "hours": {
                        "10": {
                            "picks": [
                                {
                                    "title": "Both streams",
                                    "description": "Audio and screen together.",
                                    "origin": f"{SEED_DAY}/100000_300",
                                }
                            ],
                            "rationale": "The only populated hour with both.",
                        },
                        "14": {
                            "picks": [
                                {
                                    "title": "Browsing",
                                    "description": "A named browser stream.",
                                    "origin": f"{SEED_DAY}/workstation.browser/140000_300",
                                }
                            ],
                            "rationale": "The browser hour.",
                        },
                    },
                }
            },
        },
    },
}

NEWSLETTER_WORK = """---
title: Work, week of May 10
facet: work
generated_at: 1770000200
---

# What happened

A **short** newsletter body with a list:

- one item
- two item

> and a blockquote, because the PDF stylesheet has a rule for it.
"""

NEWSLETTER_HOME = """---
title: Personal, week of May 10
facet: personal
generated_at: 1770000201
---

The personal facet newsletter, one paragraph, no headings.
"""

NEWSLETTER_WORK_EARLIER = """---
title: Work, week of May 3
facet: work
generated_at: 1770000100
---

An earlier work newsletter so the feed has a second page.
"""


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def _build_config(root: Path, *, corrupt: bool = False) -> None:
    """Create the journal config the session gate reads.

    ``corrupt`` writes a config that exists and cannot be parsed, which is a
    third gate outcome rather than a variant of the second.
    """
    (root / "config").mkdir(parents=True, exist_ok=True)
    target = root / "config" / "journal.json"
    if corrupt:
        target.write_text('{"setup": {"completed_at": 17672256')
        return
    target.write_text(
        json.dumps({"setup": {"completed_at": PINNED_COMPLETED_AT}}, indent=2) + "\n"
    )


def _seed_chronicle(root: Path) -> None:
    """Two streams on one day: default (audio+screen) and a named browser stream."""
    day = root / "chronicle" / SEED_DAY

    seg = day / "100000_300"
    _write(
        seg / "audio.jsonl",
        json.dumps({"t": "header", "stream": "_default", "start": "10:00:00"})
        + "\n"
        + json.dumps({"t": "line", "ts": 1, "speaker": "S1", "text": "first line"})
        + "\n"
        + json.dumps({"t": "line", "ts": 2, "speaker": "S2", "text": "second line"})
        + "\n",
    )
    _write(
        seg / "desktop.screen.jsonl",
        json.dumps({"t": "header", "device": "desktop"})
        + "\n"
        + json.dumps({"t": "frame", "ts": 1, "summary": "an editor"})
        + "\n",
    )

    audio_only = day / "103000_300"
    _write(
        audio_only / "audio.jsonl",
        json.dumps({"t": "header", "stream": "_default", "start": "10:30:00"})
        + "\n"
        + json.dumps({"t": "line", "ts": 1, "speaker": "S1", "text": "audio only"})
        + "\n",
    )

    browser_seg = day / "workstation.browser" / "140000_300"
    _write(
        browser_seg / "stream.json",
        json.dumps({"stream": "workstation.browser"}) + "\n",
    )
    _write(
        browser_seg / "browser_docs-example-com.jsonl",
        json.dumps(
            {
                "t": "segment_start",
                "ts": 1770000300,
                "site": "docs.example.com",
                "title": "Example docs",
                "adapter": "generic",
                "text": "The opening snapshot of the page.",
            }
        )
        + "\n"
        + json.dumps(
            {
                "t": "change",
                "ts": 1770000360,
                "text": "A second paragraph appeared.",
            }
        )
        + "\n",
    )

    _write(root / "timeline.json", json.dumps(MASTER_ROLLUP, indent=2) + "\n")


def _seed_facets(root: Path) -> None:
    for facet, title in ((SEED_FACET, "Work"), (SEED_OTHER_FACET, "Personal")):
        _write(
            root / "facets" / facet / "facet.json",
            json.dumps({"title": title, "description": f"The {facet} facet."}) + "\n",
        )


def _seed_news(root: Path) -> None:
    _write(root / "facets" / SEED_FACET / "news" / f"{SEED_DAY}.md", NEWSLETTER_WORK)
    _write(
        root / "facets" / SEED_OTHER_FACET / "news" / f"{SEED_DAY}.md", NEWSLETTER_HOME
    )
    _write(
        root / "facets" / SEED_FACET / "news" / f"{SEED_DAY_2}.md",
        NEWSLETTER_WORK_EARLIER,
    )


def _seed_activities(root: Path) -> None:
    """Activity definitions and records, through the reference's own writers."""
    from solstone.think import activities as reference_activities

    reference_activities.save_facet_activities(
        SEED_FACET,
        [
            {
                "id": "meeting",
                "name": "Meeting",
                "emoji": "\U0001f5e3",
                "instructions": "Record meetings held during this span.",
            },
            {
                "id": "focus",
                "name": "Focus",
                "emoji": "\U0001f9e0",
                "instructions": "Record focused work during this span.",
            },
        ],
    )

    visible = {
        "id": reference_activities.make_activity_id("meeting", "100000_300"),
        "activity": "meeting",
        "title": "Seeded meeting",
        "description": "A meeting the corpus seeded.",
        "details": "",
        "segments": ["100000_300"],
        "active_entities": [],
        "created_at": 1770000400000,
        "source": "user",
        "hidden": False,
        "edits": [],
    }
    visible = reference_activities.append_edit(
        visible, actor="corpus:seed", fields=["activity", "title"], note="created"
    )
    reference_activities.append_activity_record(SEED_FACET, SEED_DAY, visible)

    hidden = {
        "id": reference_activities.make_activity_id("focus", "103000_300"),
        "activity": "focus",
        "title": "Seeded muted focus block",
        "description": "A muted record, so include_hidden has something to reveal.",
        "details": "",
        "segments": ["103000_300"],
        "active_entities": [],
        "created_at": 1770000500000,
        "source": "user",
        "hidden": True,
        "edits": [],
    }
    hidden = reference_activities.append_edit(
        hidden, actor="corpus:seed", fields=["activity", "title"], note="created"
    )
    reference_activities.append_activity_record(SEED_FACET, SEED_DAY, hidden)


def _seed_curation(root: Path) -> None:
    """All FIVE curation kinds, through the reference's own writers.

    🔴 Seeding only facet candidates was the first draft, and it made four of the
    five kinds record their EMPTY branch while the case still read as covered --
    the exact tell this corpus exists to defeat. Curation's whole contract is the
    five kinds and their composite ordering; a fixture that exercises one of them
    grades a fifth of the surface and reports a full green.
    """
    from solstone.think import (
        facet_review_candidates,
        speaker_candidate_pair_review_candidates,
        speaker_review_candidates,
    )
    from solstone.think.entities import review_candidates as entity_review_candidates
    from solstone.think.entities.ambiguities import record_ambiguity_observation
    from solstone.think.entities.journal import create_journal_entity
    from solstone.think.entities.matching import (
        MatchTier,
        ResolutionOrigin,
        ResolutionScope,
    )

    # -- entities the merge and ambiguity rows refer to ---------------------
    # ⚠ Through the reference's creator: entity identity is written beside the
    # data (rule 4a) and a hand-written entity dict forks the namespace.
    for entity_id, name in (
        ("jordan-vance", "Jordan Vance"),
        ("jordan-vancey", "Jordan Vancey"),
        ("morgan-reyes", "Morgan Reyes"),
    ):
        create_journal_entity(entity_id, name, "Person")

    # -- KIND_ENTITY_MERGE --------------------------------------------------
    entity_review_candidates.record_merge_candidate(
        facet=SEED_FACET,
        day=SEED_DAY,
        source="Jordan Vancey",
        source_slug="jordan-vancey",
        target="Jordan Vance",
        target_slug="jordan-vance",
        evidence="the two names appear in the same segment",
        detections=3,
        needs=2,
    )

    # -- KIND_ENTITY_AMBIGUITY ---------------------------------------------
    record_ambiguity_observation(
        scope=ResolutionScope.facet_scope(SEED_FACET),
        query="Jordan",
        normalized_query="jordan",
        # ⚠ Only tiers 5-8 are low-confidence, and `ambiguities.jsonl` VALIDATES
        # that on write (`entities/ambiguities.py:185-191`). A first draft used
        # tier 2 and the store refused the row -- which is exactly why this seed
        # goes through the reference's writer: a hand-written row would have
        # produced a fixture pinning a state the reference cannot create.
        observed_tier=int(MatchTier.FIRST_WORD),
        # ⚠ Each candidate needs `id`, `name`, a `tier` EQUAL to observed_tier,
        # and a numeric `score` -- all four validated on write. The store is
        # self-validating with zero docs, so a subtly-wrong row bricks it rather
        # than degrading, and this is where that gets discovered rather than in
        # a port.
        ranked_candidates=[
            {
                "id": "jordan-vance",
                "name": "Jordan Vance",
                "tier": int(MatchTier.FIRST_WORD),
                "score": 0.61,
            },
            {
                "id": "jordan-vancey",
                "name": "Jordan Vancey",
                "tier": int(MatchTier.FIRST_WORD),
                "score": 0.58,
            },
        ],
        origin=ResolutionOrigin(
            lane="corpus.seed",
            facet=SEED_FACET,
            day=SEED_DAY,
            record_id="100000_300",
            field="participation.name",
        ),
    )

    # -- KIND_SPEAKER_NAME_VARIANT -----------------------------------------
    speaker_review_candidates.record_name_variant_candidate(
        source_id="spk_jordan_v",
        source_label="Jordan V",
        target_id="spk_jordan_vance",
        target_label="Jordan Vance",
        similarity=0.86,
    )

    # -- KIND_SPEAKER_CANDIDATE_PAIR ---------------------------------------
    speaker_candidate_pair_review_candidates.record_candidate_pair(
        source_anchor="anchor_a",
        target_anchor="anchor_b",
        source_anchors={"anchor_a"},
        target_anchors={"anchor_b"},
        similarity=0.74,
        source_intervals=4,
        target_intervals=3,
        source_samples=[{"day": SEED_DAY, "segment": "100000_300"}],
        target_samples=[{"day": SEED_DAY, "segment": "103000_300"}],
    )

    # -- KIND_FACET_CANDIDATE ----------------------------------------------
    for name_key, display, count in (("atlas", "Atlas", 4), ("ledger", "Ledger", 2)):
        facet_review_candidates.record_facet_candidate(
            name=display,
            name_key=name_key,
            count=count,
            window_days=7,
            samples=[
                {
                    "day": SEED_DAY,
                    "segment": "100000_300",
                    "quote": f"we should look at {display} again",
                }
            ],
            day=SEED_DAY,
        )


def _seed_awareness(root: Path) -> None:
    """Awareness state and a day log, through the reference's own writers."""
    from solstone.think import awareness

    awareness.update_state(
        "capture",
        {
            "first_segment_day": SEED_DAY,
            "streams_seen": ["_default", "workstation.browser"],
        },
    )
    # 🔴 Without this, `GET /api/imports` and `?section=imports` both answer the
    # DEFAULTS in the populated phase -- byte-identical to the empty phase. Two
    # probes would have read as covered while pinning the absence of the thing
    # they exist to pin, and `?section=imports` would have recorded
    # `awareness_section_not_found` as though that were the projection contract.
    awareness.record_import("obsidian", source_display="notes", entries_written=12)
    awareness.append_log(
        "state", key="capture.first_segment", message="first segment seen", day=SEED_DAY
    )
    awareness.append_log(
        "observation", message="the owner works mornings", day=SEED_DAY
    )
    awareness.append_log("nudge", key="imports.nudge_sent", day=SEED_DAY)
    # ⚠ `GET /app/awareness/api/log` with no `day` reads TODAY's log, and with
    # nothing seeded there the probe would record an empty envelope -- pinning
    # the ABSENCE of a log rather than the default-day BRANCH, while reading as
    # a covered route. The injected clock makes "today" an authored value, so
    # this entry lands at a known day and the probe pins real behaviour.
    awareness.append_log(
        "observation", message="seeded on the injected clock's today", day=None
    )


def _seed_populated(root: Path) -> None:
    _build_config(root)
    _seed_chronicle(root)
    _seed_facets(root)
    _seed_news(root)


def _seed_reference_writers(root: Path) -> None:
    """Second seeding pass: everything that must go through reference writers.

    ⚠ Separated because these import `solstone.think`, which resolves the
    journal from the environment, so they can only run once `SOLSTONE_JOURNAL`
    points at this root.
    """
    _seed_activities(root)
    _seed_curation(root)
    _seed_awareness(root)


# ---------------------------------------------------------------------------
# Recording
# ---------------------------------------------------------------------------


def _pdf_text(raw: bytes) -> list[str]:
    """Extract the rendered text, page by page, as the port's real contract.

    ⚠ Extraction runs through `pypdfium2`, which the journal host already ships
    for the `pdf-import` extra, so this adds no dependency. If it is unavailable
    the case says so explicitly rather than recording an empty list -- an empty
    `pdf_text` would read as "this document has no words".
    """
    try:
        import pypdfium2
    except ImportError:  # pragma: no cover - only on a host without the extra
        return ["<PDF TEXT UNAVAILABLE: pypdfium2 is not installed>"]
    document = pypdfium2.PdfDocument(raw)
    try:
        return [
            page.get_textpage().get_text_bounded().replace("\r\n", "\n")
            for page in document
        ]
    finally:
        document.close()


def _record(
    client: Any,
    method: str,
    path: str,
    why: str,
    root: Path,
    body: Any = None,
) -> dict[str, Any]:
    if body is None:
        response = client.open(path, method=method)
    else:
        response = client.open(path, method=method, json=body)
    raw = response.get_data()
    content_type = response.headers.get("Content-Type", "")
    # ⚠ Both spellings. `/tmp` is a symlink on some hosts and the reference
    # resolves the root on some paths and not others, so replacing only the
    # unresolved form leaves the resolved one in a PUBLIC fixture.
    normalized_body = raw
    for spelling in {str(root), str(root.resolve())}:
        normalized_body = normalized_body.replace(
            spelling.encode(), PLACEHOLDER_ROOT.encode()
        )

    case: dict[str, Any] = {
        "method": method,
        "path": path,
        "why": why,
        "status": response.status_code,
        "content_type": content_type,
        "body_sha256": hashlib.sha256(normalized_body).hexdigest(),
        "body_sha256_basis": "raw-body",
        "body_bytes": len(normalized_body),
    }
    if body is not None:
        case["request_json"] = body
    if normalized_body != raw:
        # The journal root is a temporary directory; it cannot be reproduced, so
        # it is replaced before hashing rather than left to make the case unstable.
        case["body_normalized"] = [PLACEHOLDER_ROOT]

    location = response.headers.get("Location")
    if location:
        # ⛔ Recorded verbatim. `/app/timeline/` redirects to TODAY, and with the
        # clock injected that target is authored, not observed -- so pinning the
        # real value is strictly stronger than a `<TODAY>` placeholder, which
        # would match a port that redirected to the wrong day.
        case["location"] = location

    disposition = response.headers.get("Content-Disposition")
    if disposition:
        case["content_disposition"] = disposition

    if "application/pdf" in content_type:
        # 🔴 TWO pins with DIFFERENT standing, and conflating them would make one
        # of them unsatisfiable.
        #
        # ⛔ CORRECTION, mine, in this file: a first draft recorded no hash at
        # all and gave the reason "weasyprint stamps a creation instant, so the
        # bytes are not reproducible". Measured after the clock injection landed
        # a few lines up: two separate captures of this route are BYTE-IDENTICAL
        # (sha 94fc5e0d…, 8823 bytes). The stated reason had stopped being true
        # because of a later change in the same file, and a reason that is no
        # longer the one holding a practice up is how the practice gets dropped
        # for the wrong cause.
        #
        # `reference_body_sha256` therefore records the reference's exact bytes,
        # and is a RECORD, ⛔ never a port criterion: a Rust renderer will never
        # emit weasyprint's bytes, so asserting it would be an unsatisfiable pair
        # with "port this route".
        #
        # ✅ `pdf_text` is the port's actual contract -- the words the owner
        # receives -- and it is implementation-independent.
        case["body_sha256_basis"] = "reference-bytes-record-only"
        case["reference_body_sha256"] = case.pop("body_sha256")
        case["reference_body_bytes"] = case.pop("body_bytes")
        case["reference_renderer"] = "weasyprint"
        case["pdf_magic"] = raw[:5].decode("ascii", errors="replace")
        case["pdf_text"] = _pdf_text(raw)
        case["pin"] = (
            "status, content_type, content_disposition and pdf_text are the "
            "contract; reference_body_sha256 is a record of what weasyprint "
            "emitted and must NOT be asserted by a port"
        )
        return case

    if "json" in content_type:
        found: set[str] = set()
        allowed = set(NORMALIZED_FIELDS.get(path, set()))
        parsed = json.loads(normalized_body)
        case["json"] = _normalize(parsed, found, allowed)
        case["normalized_fields"] = sorted(found)
        # 🔴 A raw-body hash is NOT reproducible for a case carrying a
        # normalized field. Hash what the corpus actually asserts -- the
        # canonical normalized JSON.
        case["body_sha256"] = hashlib.sha256(
            json.dumps(case["json"], sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
        case["body_sha256_basis"] = "normalized-json"
        return case

    if content_type.startswith("text/") and len(normalized_body) <= 8192:
        # Short text bodies are recorded verbatim: the plain-text 404s and the
        # markdown raw routes are exactly the half a port gets wrong.
        case["body_text"] = normalized_body.decode("utf-8", errors="replace")
    return case


def _phase_cases(phase: str, seed: bool) -> list[dict[str, Any]]:
    from solstone.convey import create_app

    with tempfile.TemporaryDirectory(prefix=f"convey-facets-{phase}-") as tmp:
        root = Path(tmp)
        if phase == "corrupt":
            _build_config(root, corrupt=True)
        elif phase != "unestablished":
            _build_config(root)
        if seed:
            _seed_populated(root)

        os.environ["SOLSTONE_JOURNAL"] = str(root)
        os.environ["SOLSTONE_DISABLE_CONVEY_SIDE_RUNTIMES"] = "1"
        if seed:
            _seed_reference_writers(root)

        _reset_reference_caches()
        app = create_app(str(root))
        client = app.test_client()
        return [_record(client, *probe, root) for probe in READ_PROBES]


def _reset_reference_caches() -> None:
    """Drop the reference's process-global caches between phases.

    ⚠ `apps.timeline.routes` memoizes the master rollup and per-segment reads on
    module globals keyed partly by journal root, and `_stats_for_month` is an
    `lru_cache`. Two phases in one process would otherwise let phase N's answers
    leak into phase N+1 -- a corpus that pins the PREVIOUS journal's content and
    still looks fully populated.
    """
    from solstone.apps.timeline import routes as timeline_routes

    timeline_routes._master_cache = None
    timeline_routes._master_key = None
    timeline_routes._seg_cache.clear()
    timeline_routes._stats_for_month.cache_clear()


def _mutation_cases() -> list[dict[str, Any]]:
    from solstone.convey import create_app

    with tempfile.TemporaryDirectory(prefix="convey-facets-mutations-") as tmp:
        root = Path(tmp)
        _seed_populated(root)
        os.environ["SOLSTONE_JOURNAL"] = str(root)
        os.environ["SOLSTONE_DISABLE_CONVEY_SIDE_RUNTIMES"] = "1"
        _seed_reference_writers(root)
        _reset_reference_caches()

        app = create_app(str(root))
        client = app.test_client()

        cases: list[dict[str, Any]] = []
        created_id: str | None = None
        for index, (method, path, body, why) in enumerate(MUTATION_PROBES):
            if "{created}" in path:
                if created_id is None:
                    raise RuntimeError(
                        "a mutation probe referenced {created} before any record "
                        "was created; the replay order is wrong"
                    )
                resolved = path.replace("{created}", created_id)
            else:
                resolved = path
            case = _record(client, method, resolved, why, root, body)
            case["sequence"] = index
            if "{created}" in path:
                case["path"] = path
                case["path_resolved_from"] = PLACEHOLDER_ACTIVITY_ID
            if (
                created_id is None
                and method == "POST"
                and resolved.endswith("/records?facet=" + SEED_FACET)
                and case["status"] == 200
            ):
                created_id = case["json"]["record"]["id"]
            cases.append(case)
        if created_id is None:
            raise RuntimeError(
                "the activities create probe never produced a record id; the "
                "replay recorded a refusal where it expected a creation"
            )
        return cases


def build_corpus() -> dict[str, Any]:
    phases = {
        "unestablished": _phase_cases("unestablished", seed=False),
        "corrupt": _phase_cases("corrupt", seed=False),
        "established_empty": _phase_cases("established_empty", seed=False),
        "populated": _phase_cases("populated", seed=True),
    }
    return {
        "schema": SCHEMA,
        "generator": "scripts/convey_facets_corpus.py",
        "tz": "UTC",
        "pinned_completed_at": PINNED_COMPLETED_AT,
        "seed": {
            "day": SEED_DAY,
            "earlier_day": SEED_DAY_2,
            "month": SEED_MONTH,
            "empty_month": SEED_PREV_MONTH,
            "facet": SEED_FACET,
            "other_facet": SEED_OTHER_FACET,
        },
        "placeholders": {
            "day": PLACEHOLDER_DAY,
            "now": PLACEHOLDER_NOW,
            "clock": PLACEHOLDER_CLOCK,
            "journal_root": PLACEHOLDER_ROOT,
            "activity_id": PLACEHOLDER_ACTIVITY_ID,
        },
        "side_runtimes_disabled": True,
        "phases": phases,
        "mutations": _mutation_cases(),
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Capture the facets-plate conformance oracle"
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit non-zero if the corpus on disk differs from a fresh capture",
    )
    args = parser.parse_args()

    corpus = build_corpus()
    rendered = json.dumps(corpus, indent=2, sort_keys=True) + "\n"
    # 🔴 Both, in this order, and neither is sufficient alone. `assert_publishable`
    # says nothing left the machine's identity IN the fixture;
    # `assert_no_egress_attempted` says nothing tried to leave the machine while
    # producing it -- and a swallowed attempt is invisible to the first check and
    # to a byte-identical diff.
    assert_publishable(rendered, label="convey facets corpus")
    assert_no_egress_attempted("convey facets corpus", ignore=CONTROL_DESTINATIONS)

    if args.check:
        if not CORPUS_PATH.exists():
            print(f"missing corpus: {CORPUS_PATH}", file=sys.stderr)
            return 1
        if CORPUS_PATH.read_text() != rendered:
            print(
                f"facets corpus is stale: {CORPUS_PATH}\n"
                "regenerate with: python scripts/convey_facets_corpus.py",
                file=sys.stderr,
            )
            return 1
        print(f"facets corpus is current: {CORPUS_PATH}")
        return 0

    CORPUS_PATH.parent.mkdir(parents=True, exist_ok=True)
    CORPUS_PATH.write_text(rendered)
    total = sum(len(cases) for cases in corpus["phases"].values())
    print(
        f"wrote {CORPUS_PATH} "
        f"({total} read cases across {len(corpus['phases'])} phases, "
        f"{len(corpus['mutations'])} mutation cases)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
