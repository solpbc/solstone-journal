#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Freeze the generate-seam oracles whose Python reference the conversion deletes.

Two cross-language differentials drive a Python function that lives inside the
half of its module the generate conversion removes:

    endpoint_overflow_differential -> providers/local._endpoint_overflow_decision
    schema_validation_differential -> models._validate_schema_with_annotations

When that half goes, the test and the thing it tested disappear together and
nothing reds. This script runs each reference over a corpus and records its
answers, so the check survives as a vector test.

⛔ The expected answers are never written by hand. They are OBSERVED by calling
the reference, which is why this script can only be run while the reference
still exists -- and why it must be run before the cut, not after.

Usage:  python scripts/generate_seam_oracles.py [--check]

        --check regenerates in memory and fails if the committed fixtures differ,
        which is how the gate notices the reference moved underneath them.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO))

OVERFLOW_FIXTURE = REPO / "core" / "fixtures" / "endpoint_overflow_oracle.json"
SCHEMA_FIXTURE = REPO / "core" / "fixtures" / "schema_validation_oracle.json"

# The reference sources, cited so a reader can check any recorded answer against
# the code that produced it -- and so `revision()` pins the right commit.
OVERFLOW_REFERENCE = {
    "decision": "solstone/think/providers/local.py:711",
    "patterns": "solstone/think/providers/shared.py:164",
}
SCHEMA_REFERENCE = {
    "validator": "solstone/think/models.py",
    "truncate_key": "solstone/think/schema_prep.py",
}

# --------------------------------------------------------------------------
# Corpora
# --------------------------------------------------------------------------
#
# ⚠ Every case the live differential covered appears here under a stable name,
# and the names beginning `live-` are exactly those cases. The rest were added
# while the reference was still runnable, because after the cut they cannot be.

_ANCHORED = (
    "maximum context length of 1000 tokens: "
    "600 tokens from the input messages and 400 tokens for the completion"
)

OVERFLOW_CASES: list[tuple[str, str, int | None, int]] = [
    # -- the seven the live differential already covered --
    ("live-retry-first-attempt", _ANCHORED, None, 0),
    ("live-context-second-attempt", _ANCHORED, None, 1),
    (
        "live-budget-input-dominates",
        "maximum context length of 1000 tokens: "
        "800 tokens from the input messages and 400 tokens for the completion",
        None,
        0,
    ),
    (
        "live-served-window-supplies-limit",
        "600 tokens from the input messages and 400 tokens for the completion",
        1000,
        0,
    ),
    # Same body, no configured window: the reference cannot compute a clamp,
    # falls through to the context-pattern check, misses that too, and
    # classifies it `contract` rather than a context error.
    (
        "live-no-limit-available",
        "600 tokens from the input messages and 400 tokens for the completion",
        None,
        0,
    ),
    ("live-context-pattern", "request exceeds the context window", None, 0),
    ("live-unrecognised", "unexpected endpoint response", None, 0),
    # -- boundaries around the reclamp, which the live corpus never pinned --
    (
        "reclamp-exactly-at-minimum",
        "maximum context length of 1000 tokens: "
        "728 tokens from the input messages and 400 tokens for the completion",
        None,
        0,
    ),
    (
        "reclamp-one-below-minimum",
        "maximum context length of 1000 tokens: "
        "729 tokens from the input messages and 400 tokens for the completion",
        None,
        0,
    ),
    (
        "reclamp-negative-headroom",
        "maximum context length of 1000 tokens: "
        "1200 tokens from the input messages and 400 tokens for the completion",
        None,
        0,
    ),
    ("attempt-beyond-first", _ANCHORED, None, 2),
    # -- parsing shapes --
    ("uppercase-body", _ANCHORED.upper(), None, 0),
    (
        "limit-phrase-outranks-served-window",
        _ANCHORED,
        99999,
        0,
    ),
    (
        "singular-input-message",
        "maximum context length of 1000 tokens: "
        "600 tokens from the input message and 400 tokens for the completion",
        None,
        0,
    ),
    (
        "anchor-without-input-count",
        "maximum context length of 1000 tokens for the completion",
        None,
        0,
    ),
    ("served-window-zero-is-a-limit", _ANCHORED.split(": ", 1)[1], 0, 0),
    # -- the remaining context patterns, none of which the live corpus reached --
    ("pattern-context-length-exceeded", "context length exceeded", None, 0),
    (
        "pattern-available-context-size",
        "this request exceeds the available context size",
        None,
        0,
    ),
    (
        "pattern-model-context-length",
        "the prompt is longer than the model's context length",
        None,
        0,
    ),
    ("pattern-context-size-exceeded", "context size has been exceeded", None, 0),
    ("empty-body", "", None, 0),
]

SCHEMA_CASES: list[tuple[str, str, dict[str, Any]]] = [
    # -- the thirteen the live differential already covered --
    (
        "live-valid",
        '{"field":"ok"}',
        {
            "type": "object",
            "properties": {"field": {"type": "string"}},
            "required": ["field"],
        },
    ),
    (
        "live-type",
        '{"field":"bad"}',
        {"type": "object", "properties": {"field": {"type": "integer"}}},
    ),
    ("live-required", "{}", {"type": "object", "required": ["field"]}),
    (
        "live-nested",
        '{"outer":{"inner":"bad"}}',
        {
            "type": "object",
            "properties": {
                "outer": {
                    "type": "object",
                    "properties": {"inner": {"type": "integer"}},
                }
            },
        },
    ),
    (
        "live-array-element",
        '{"items":[1,"bad"]}',
        {
            "type": "object",
            "properties": {
                "items": {"type": "array", "items": {"type": "integer"}}
            },
        },
    ),
    ("live-unparseable", "{", {"type": "object"}),
    ("live-uncompilable-schema", '{"field":"ok"}', {"type": "not-a-real-type"}),
    (
        "live-truncation",
        '{"word":"four"}',
        {
            "type": "object",
            "properties": {
                "word": {"type": "string", "maxLength": 3, "x-truncate": True}
            },
        },
    ),
    (
        "live-unicode-truncation",
        '{"word":"éééé"}',
        {
            "type": "object",
            "properties": {
                "word": {"type": "string", "maxLength": 3, "x-truncate": True}
            },
        },
    ),
    (
        "live-reference-hidden",
        '{"word":"four"}',
        {
            "$defs": {"word": {"type": "string", "maxLength": 3, "x-truncate": True}},
            "properties": {"word": {"$ref": "#/$defs/word"}},
        },
    ),
    (
        "live-all-of-hidden",
        '{"word":"four"}',
        {
            "type": "object",
            "properties": {
                "word": {
                    "allOf": [{"type": "string", "maxLength": 3, "x-truncate": True}]
                }
            },
        },
    ),
    (
        "live-pattern-properties-hidden",
        '{"word":"four"}',
        {
            "type": "object",
            "patternProperties": {
                "^word$": {"type": "string", "maxLength": 3, "x-truncate": True}
            },
        },
    ),
    (
        "live-prefix-items-hidden",
        '["four"]',
        {
            "type": "array",
            "prefixItems": [
                {"type": "string", "maxLength": 3, "x-truncate": True}
            ],
        },
    ),
    # -- added while the reference is still runnable --
    (
        "multiple-errors",
        '{"a":"bad","b":"bad"}',
        {
            "type": "object",
            "properties": {"a": {"type": "integer"}, "b": {"type": "integer"}},
        },
    ),
    (
        "enum",
        '{"field":"c"}',
        {"type": "object", "properties": {"field": {"enum": ["a", "b"]}}},
    ),
    (
        "numeric-minimum",
        '{"field":1}',
        {"type": "object", "properties": {"field": {"type": "integer", "minimum": 5}}},
    ),
    (
        "additional-properties-false",
        '{"field":"ok","extra":1}',
        {
            "type": "object",
            "properties": {"field": {"type": "string"}},
            "additionalProperties": False,
        },
    ),
    (
        "deeply-nested",
        '{"a":{"b":{"c":"bad"}}}',
        {
            "type": "object",
            "properties": {
                "a": {
                    "type": "object",
                    "properties": {
                        "b": {
                            "type": "object",
                            "properties": {"c": {"type": "integer"}},
                        }
                    },
                }
            },
        },
    ),
    (
        "truncation-exactly-at-limit",
        '{"word":"abc"}',
        {
            "type": "object",
            "properties": {
                "word": {"type": "string", "maxLength": 3, "x-truncate": True}
            },
        },
    ),
    ("empty-schema", '{"anything":true}', {}),
    (
        "array-of-objects",
        '{"rows":[{"n":1},{"n":"bad"}]}',
        {
            "type": "object",
            "properties": {
                "rows": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {"n": {"type": "integer"}},
                    },
                }
            },
        },
    ),
    (
        "truncate-without-max-length",
        '{"word":"four"}',
        {
            "type": "object",
            "properties": {"word": {"type": "string", "x-truncate": True}},
        },
    ),
    (
        "any-of-hidden",
        '{"word":"four"}',
        {
            "type": "object",
            "properties": {
                "word": {
                    "anyOf": [{"type": "string", "maxLength": 3, "x-truncate": True}]
                }
            },
        },
    ),
    (
        "one-of-hidden",
        '{"word":"four"}',
        {
            "type": "object",
            "properties": {
                "word": {
                    "oneOf": [{"type": "string", "maxLength": 3, "x-truncate": True}]
                }
            },
        },
    ),
    (
        "null-where-string-required",
        '{"field":null}',
        {"type": "object", "properties": {"field": {"type": "string"}}},
    ),
    (
        "root-is-not-an-object",
        '"a bare string"',
        {"type": "object"},
    ),
    (
        "emoji-truncation",
        '{"word":"👍👍👍👍"}',
        {
            "type": "object",
            "properties": {
                "word": {"type": "string", "maxLength": 3, "x-truncate": True}
            },
        },
    ),
]


# --------------------------------------------------------------------------
# Observation
# --------------------------------------------------------------------------


def observe_overflow() -> list[dict[str, Any]]:
    from solstone.think.providers.local import _endpoint_overflow_decision

    rows = []
    for name, body, served_window, attempt in OVERFLOW_CASES:
        decision = _endpoint_overflow_decision(body, served_window, attempt)
        rows.append(
            {
                "name": name,
                "body": body,
                "served_window": served_window,
                "attempt": attempt,
                "kind": decision.kind,
                "max_tokens": decision.max_tokens,
            }
        )
    return rows


def observe_schema() -> list[dict[str, Any]]:
    from solstone.think.models import _validate_schema_with_annotations

    rows = []
    for name, text, schema in SCHEMA_CASES:
        observed_text, validation = _validate_schema_with_annotations(text, schema)
        rows.append(
            {
                "name": name,
                "text": text,
                "schema": schema,
                "observed_text": observed_text,
                "validation": validation,
            }
        )
    return rows


def revision(reference: dict[str, str]) -> str:
    """The revision of the reference these answers were read from.

    ⛔ Deliberately NOT ``rev-parse HEAD``, for the reason the retention oracle
    already recorded: pinning HEAD makes the fixture stale on every unrelated
    commit, so the gate reds for a reason that has nothing to do with the
    reference it guards. Pinning the last commit that touched the reference
    files goes stale exactly when these answers should be re-observed.

    ⚠ It is also what makes this generator's output byte-identical on a second
    run -- a wall-clock date would not be.
    """
    sources = sorted({citation.split(":", 1)[0] for citation in reference.values()})
    return subprocess.run(
        ["git", "log", "-1", "--format=%H", "--", *sources],
        cwd=REPO,
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()


def commit_date(commit: str) -> str:
    return subprocess.run(
        ["git", "show", "-s", "--format=%cI", commit],
        cwd=REPO,
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()


def document(
    *,
    oracle: str,
    reference: dict[str, str],
    method: str,
    rows: list[dict[str, Any]],
) -> dict[str, Any]:
    commit = revision(reference)
    return {
        "_provenance": {
            "generator": "scripts/generate_seam_oracles.py",
            "oracle": oracle,
            "method": method,
            "source_revision": commit,
            "source_revision_date": commit_date(commit),
            "reference": reference,
            "case_count": len(rows),
            "why": (
                "the generate conversion deletes the half of the reference module "
                "this oracle lives in. Once that lands these answers cannot be "
                "re-derived from the reference at any cost, because the reference "
                "is gone -- so they were observed while it still ran."
            ),
        },
        "cases": rows,
    }


def render(doc: dict[str, Any]) -> str:
    return json.dumps(doc, indent=2, sort_keys=True, ensure_ascii=False) + "\n"


def main() -> int:
    check = "--check" in sys.argv[1:]

    documents = [
        (
            OVERFLOW_FIXTURE,
            document(
                oracle="solstone.think.providers.local._endpoint_overflow_decision",
                reference=OVERFLOW_REFERENCE,
                method=(
                    "each row's kind and max_tokens are OBSERVED by calling the "
                    "reference with the row's own body, served_window and attempt"
                ),
                rows=observe_overflow(),
            ),
        ),
        (
            SCHEMA_FIXTURE,
            document(
                oracle="solstone.think.models._validate_schema_with_annotations",
                reference=SCHEMA_REFERENCE,
                method=(
                    "each row's observed_text and validation are OBSERVED by calling "
                    "the reference with the row's own text and schema. ⚠ Consumers "
                    "compare error path and constraint only -- messages differ "
                    "between the Python and Rust jsonschema implementations by "
                    "design, and observed_text differs by permitted JSON formatting"
                ),
                rows=observe_schema(),
            ),
        ),
    ]

    failures = []
    for path, doc in documents:
        rendered = render(doc)
        if check:
            existing = path.read_text(encoding="utf-8") if path.exists() else ""
            if existing != rendered:
                failures.append(path.relative_to(REPO))
        else:
            path.write_text(rendered, encoding="utf-8")
            print(f"wrote {path.relative_to(REPO)} ({doc['_provenance']['case_count']} cases)")

    if failures:
        for path in failures:
            print(f"stale: {path}", file=sys.stderr)
        print(
            "the frozen oracles no longer match the reference. Re-run "
            "`python scripts/generate_seam_oracles.py` WHILE THE REFERENCE STILL "
            "EXISTS and review the diff.",
            file=sys.stderr,
        )
        return 1

    if check:
        print("seam oracles match the reference")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
