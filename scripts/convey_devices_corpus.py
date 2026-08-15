#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Conformance oracle for the devices READ surface's classification and serialization.

The device store itself already has seven differential tests against the reference
(`solstone-core-observer/tests/*_differential.rs`). What none of them cover is the
**presentation** layer the web surface adds on top: how a stored record becomes a row
in the owner's device list -- its freshness state, its group, its label, its elapsed
milliseconds, and the order the rows arrive in.

That layer is a pile of threshold comparisons, and every one of them has two sides.
This drives the reference over both sides of every threshold and records what came
back, so the native port's tests assert against captured reference output rather than
against a re-reading of the reference's source.

⚠ Why a capture and not a hand-written table: a hand-written table encodes the
author's model of the code. Three boundaries here are off-by-one from the obvious
reading -- `elapsed == ACTIVE_THRESHOLD_MS` classifies as **stale**, not connected;
`elapsed == STALE_THRESHOLD_MS` classifies as **disconnected**; and a future
`last_seen` at exactly the drift tolerance is **not** clock skew. A table written
from the prose would have got all three wrong and the tests would have defended it.

⛔ CORRECTION, and it is the reason `list_order` alone is not a specification.
An earlier reading of this capture concluded "a revoked device sorts above an
offline one." **That is not a rule, and encoding it produces a wrong
implementation.** `revoked` and `disconnected` share `group == "inactive"`; in the
captured order the revoked row preceded the offline one purely because its
`last_seen` was more recent. The comparator is
`(group_order[group], last_seen is None, -(last_seen or 0), prefix)` and nothing
in it mentions revocation. **Implement the comparator, not the observed order.**
The first version of this corpus held a single revoked row, so it could not have
caught that mistake; it now carries a second revoked row whose `last_seen` is
older than the offline row's, which fails any "revoked first" implementation.

⚠ Same clock as its siblings: regenerating requires a runnable reference tree, and
the conversion removes that tree. It is a frozen record.

⛔ No value here comes from any real journal. Every record is synthetic.

Usage:
    python scripts/convey_devices_corpus.py            # write the corpus
    python scripts/convey_devices_corpus.py --check    # fail if it would change
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

REPO_ROOT = Path(__file__).resolve().parent.parent
CORPUS_PATH = REPO_ROOT / "core" / "fixtures" / "convey_devices_corpus.json"

from solstone.apps.observer.presentation import (  # noqa: E402
    ACTIVE_THRESHOLD_MS,
    FUTURE_CLOCK_DRIFT_TOLERANCE_MS,
    OBSERVER_STATE_LABELS,
    STALE_THRESHOLD_MS,
    classify_observer_freshness,
    serialize_observer,
)

# A fixed epoch so the corpus is byte-reproducible. Nothing here reads the clock.
NOW = 1_760_000_000_000

# Both sides of every threshold, plus the two clock-skew branches and never-seen.
ELAPSED_CASES: list[tuple[str, int | None]] = [
    ("now", 0),
    ("active_minus_1", ACTIVE_THRESHOLD_MS - 1),
    ("active_exact", ACTIVE_THRESHOLD_MS),
    ("active_plus_1", ACTIVE_THRESHOLD_MS + 1),
    ("stale_minus_1", STALE_THRESHOLD_MS - 1),
    ("stale_exact", STALE_THRESHOLD_MS),
    ("stale_plus_1", STALE_THRESHOLD_MS + 1),
    ("future_1s", -1_000),
    ("future_tolerance_minus_1", -(FUTURE_CLOCK_DRIFT_TOLERANCE_MS - 1)),
    ("future_tolerance_exact", -FUTURE_CLOCK_DRIFT_TOLERANCE_MS),
    ("future_tolerance_plus_1", -(FUTURE_CLOCK_DRIFT_TOLERANCE_MS + 1)),
    ("never_seen", None),
]


def freshness_cases() -> list[dict]:
    cases = []
    for label, elapsed in ELAPSED_CASES:
        last_seen = None if elapsed is None else NOW - elapsed
        for revoked in (False, True):
            output = classify_observer_freshness(last_seen, revoked, NOW)
            cases.append(
                {
                    "case": f"{label}_revoked" if revoked else label,
                    "input": {
                        "last_seen": last_seen,
                        "revoked": revoked,
                        "now_ms": NOW,
                    },
                    "output": dict(output),
                    "label": OBSERVER_STATE_LABELS[str(output["state"])],
                }
            )
    return cases


def record(key: str, name: str, elapsed: int | None, **overrides) -> dict:
    base = {
        "key": key,
        "name": name,
        "created_at": NOW - 3_600_000,
        "last_seen": None if elapsed is None else NOW - elapsed,
        "last_segment": None,
        "enabled": True,
        "revoked": False,
        "revoked_at": None,
        "stats": {"segments_received": 2, "bytes_received": 1024},
    }
    base.update(overrides)
    return base


# The sort the owner's list arrives in:
#   (group_order[group], last_seen is None, -(last_seen or 0), prefix)
# with group_order = active < stale < inactive. Seeded deliberately OUT of order,
# including a same-group/same-last_seen pair whose only tiebreak is the prefix.
SORT_RECORDS = [
    record("dddddddd444", "disconnected-old", STALE_THRESHOLD_MS + 60_000),
    record("bbbbbbbb222", "stale-one", ACTIVE_THRESHOLD_MS + 5_000),
    record("ffffffff666", "never-seen", None),
    record("aaaaaaaa111", "connected-one", 1_000),
    record("cccccccc333", "connected-two-same-last-seen", 1_000),
    record(
        "eeeeeeee555",
        "revoked-recent",
        5_000,
        revoked=True,
        revoked_at=NOW - 60_000,
    ),
    # The discriminating row: revoked, but seen LONGER ago than "disconnected-old"
    # above. It shares group "inactive" with that row, so the comparator must place
    # it BELOW -- which any "revoked sorts first" implementation gets backwards.
    record(
        "99999999777",
        "revoked-stale",
        STALE_THRESHOLD_MS + 120_000,
        revoked=True,
        revoked_at=NOW - 30_000,
    ),
]

GROUP_ORDER = {"active": 0, "stale": 1, "inactive": 2}


def serialized_cases() -> list[dict]:
    return [
        {"input": rec, "output": serialize_observer(dict(rec), NOW)}
        for rec in SORT_RECORDS
    ]


def sorted_prefixes(rows: list[dict]) -> list[str]:
    ordered = sorted(
        rows,
        key=lambda observer: (
            GROUP_ORDER[observer.get("group", "inactive")],
            1 if observer.get("last_seen") is None else 0,
            -(observer.get("last_seen") or 0),
            observer.get("prefix", ""),
        ),
    )
    return [row["prefix"] for row in ordered]


def build() -> dict:
    serialized = serialized_cases()
    return {
        "captured_from": [
            "solstone.apps.observer.presentation.classify_observer_freshness",
            "solstone.apps.observer.presentation.serialize_observer",
        ],
        "reference_rev": "0a0632ad73816efe7e4d0fc38b92e732e4f8ceb4",
        "now_ms": NOW,
        "constants": {
            "ACTIVE_THRESHOLD_MS": ACTIVE_THRESHOLD_MS,
            "STALE_THRESHOLD_MS": STALE_THRESHOLD_MS,
            "FUTURE_CLOCK_DRIFT_TOLERANCE_MS": FUTURE_CLOCK_DRIFT_TOLERANCE_MS,
            "OBSERVER_STATE_LABELS": dict(OBSERVER_STATE_LABELS),
        },
        # 🔴 Not every recorded field is a contract. Two describe the ENVIRONMENT
        # this capture ran in, and a port that copies them through asserts
        # something false.
        #
        # `live` and `last_chat_request_at` are read from the reference's SSE
        # bridge (`convey_bridge.subscription_count`, `.last_chat_request_at`).
        # The captured `false` / `null` mean "a bridge answered and nothing was
        # subscribed" -- NOT "there is no bridge". A native surface with no bridge
        # at all is in a third state the reference cannot express, and `false`
        # renders in the page identically to a device that is genuinely not live.
        # A class alone is not enough to test against: it says a field may
        # differ, not what it should BE. `environment_native` records the value a
        # native surface must emit, so a port asserts a value instead of
        # inferring one from a set difference.
        #
        # `live`: null, because a native surface has no bridge -- an "unknown"
        # the reference cannot express, and distinct from its `false`.
        # `last_chat_request_at`: null, which coincidentally equals the capture;
        # recorded explicitly so nobody has to notice the coincidence.
        "field_classes": {
            "contract": [
                "clock_skew", "created_at", "elapsed_ms", "enabled", "failing",
                "group", "label", "last_seen", "last_segment", "name", "prefix",
                "revoked", "revoked_at", "state", "stats",
            ],
            "environment": ["live", "last_chat_request_at"],
        },
        "environment_native": {"live": None, "last_chat_request_at": None},
        "comparator": {
            "note": "Implement THIS, not the observed list_order.",
            "key": "(group_order[group], last_seen is None, -(last_seen or 0), prefix)",
            "group_order": GROUP_ORDER,
        },
        "freshness": freshness_cases(),
        "serialized": serialized,
        "list_order": sorted_prefixes([case["output"] for case in serialized]),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail if the corpus on disk differs from a fresh capture",
    )
    args = parser.parse_args()

    rendered = json.dumps(build(), indent=2, sort_keys=True) + "\n"
    if args.check:
        if not CORPUS_PATH.exists():
            print(f"corpus missing: {CORPUS_PATH}", file=sys.stderr)
            return 1
        if CORPUS_PATH.read_text() != rendered:
            print(
                f"corpus differs from a fresh capture: {CORPUS_PATH}",
                file=sys.stderr,
            )
            return 1
        print(f"corpus matches the reference: {CORPUS_PATH}")
        return 0

    CORPUS_PATH.parent.mkdir(parents=True, exist_ok=True)
    CORPUS_PATH.write_text(rendered)
    print(f"wrote {CORPUS_PATH}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
