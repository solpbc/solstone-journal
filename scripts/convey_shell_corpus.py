#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Conformance oracle for the convey shell and the speakers app surface.

The convey web service is being rebuilt in Rust. Every acceptance criterion the
rebuild could write for itself would be a restatement of what the Rust does, not
evidence that it matches what the reference actually served. This module drives
the reference Flask application over a fixed list of probe requests and records
what came back.

It pins three things at once, deliberately:

  * **admission** — each probe records the status the reference returned, so a
    case also proves whether the session gate admitted it. The same probe list is
    driven three times, against an unestablished, an established, and a
    *corrupt-config* journal, because the gate has three outcomes and not two.
    ``journal_is_active`` returns ``False`` when the config is absent, ``True``
    when setup completed, and **raises** ``CorruptConfigError`` when the file
    exists and cannot be read or parsed. A port that collapses the third case
    into the first sends an owner whose config went bad into the **first-run
    wizard** -- telling them their journal was never set up, over an existing
    journal. That branch is invisible in any single-phase capture;
  * **shape** — the JSON body, normalized only where a value is genuinely
    non-reproducible, with every normalization named in ``normalized_fields``
    rather than silently applied; and
  * **bytes** — the sha256 and length of the raw body, recorded even where the
    body was normalized, so a structural match that is not a byte match is still
    detectable.

⚠ This corpus has a clock. Regenerating it requires a runnable reference tree,
and the conversion deletes that tree. It is a frozen record, not a live
comparison: once the reference can no longer be executed these values are the
only remaining statement of what convey served. That is why it is captured
before the rebuild rather than after it, and why an unreproducible field is
named rather than dropped.

⛔ The journal this drives is built by the generator in a temporary directory and
contains no owner data. Never point it at a real journal: the probe list reads
app state, and a recorded body would carry it into a tracked file.

Determinism: ``TZ`` is pinned to UTC before any solstone import, because the
reference derives ``today`` from the process timezone. Values that cannot be
made reproducible -- the wall-clock day, the installed package version, the
onboarding timestamp -- are replaced with named placeholders and listed per
case in ``normalized_fields``.

Usage:
    python scripts/convey_shell_corpus.py            # write the corpus
    python scripts/convey_shell_corpus.py --check    # fail if it would change
"""

from __future__ import annotations

import argparse
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

REPO_ROOT = Path(__file__).resolve().parent.parent
CORPUS_PATH = REPO_ROOT / "core" / "fixtures" / "convey_shell_corpus.json"

# A fixed instant so `setup.completed_at` is reproducible across regenerations.
# 2026-01-01T00:00:00Z -- before any journal this corpus will ever describe.
PINNED_COMPLETED_AT = 1767225600

DAY_PATTERN = re.compile(r"^\d{8}$")

PLACEHOLDER_DAY = "<TODAY>"
PLACEHOLDER_VERSION = "<VERSION>"

# (method, path, why this probe is in the corpus)
PROBES: list[tuple[str, str, str]] = [
    ("GET", "/", "the root gate: its redirect target is the whole first-run branch"),
    ("GET", "/favicon.ico", "gate-exempt static, served before any session exists"),
    ("GET", "/static/shell.html", "the SPA shell every app serves verbatim"),
    ("GET", "/api/shell", "the one boot payload shell_boot.js fetches"),
    ("GET", "/api/system/status", "status_pane.js"),
    ("GET", "/app/speakers/", "an app index: must be the shell, byte-identical"),
    ("GET", "/app/speakers/workspace", "the app fragment bytes"),
    ("GET", "/app/speakers/static/who_is_this.js", "per-app static"),
    ("GET", "/app/speakers/api/state", "the app's initial-state endpoint"),
    ("GET", "/app/speakers/api/grid", "first data call from workspace.html"),
    ("GET", "/app/speakers/api/quality", "overview quality panel"),
    ("GET", "/app/speakers/api/speakers/known", "known-voices list"),
    ("GET", "/app/speakers/api/owner/status", "owner voiceprint status"),
    ("GET", "/app/speakers/api/discovery/cache", "discovery cache read"),
    ("GET", "/app/speakers/api/stats/202601", "the date_nav framework contract"),
    ("GET", "/app/speakers/api/segments/20260101", "a day with no segments"),
    ("GET", "/app/speakers/api/index", "reached only by its own tests -- recorded so a deletion is a decision"),
    ("GET", "/app/nonexistent-app/", "the not-found fallback shape"),
]


# 🔴 Normalization is a PATH ALLOWLIST, never a shape test.
#
# An earlier version replaced any string matching ^\d{8}$ with the day
# placeholder. On an empty journal that touched one field. On a populated one it
# would eat /api/grid's `coverage.start` and `coverage.end`, /api/index's
# coverage, and every `day` value in every segment payload -- and a port
# returning the WRONG coverage window would still match, because both sides
# normalize to the same placeholder. An oracle that erases the values it exists
# to pin is worse than no oracle, because it reports green.
#
# Each entry is (probe path, dotted field path within the JSON body).
NORMALIZED_FIELDS: dict[str, set[str]] = {
    "/api/shell": {"version"},
    "/app/speakers/api/state": {"today"},
}


def _normalize(
    value: Any,
    found: set[str],
    allowed: set[str],
    path: str = "",
) -> Any:
    """Replace allowlisted non-reproducible scalars, recording each one.

    ⛔ A field absent from `allowed` is returned verbatim however volatile it
    looks. Widening this is a decision, not a convenience.
    """
    if isinstance(value, dict):
        return {
            key: _normalize(item, found, allowed, f"{path}.{key}" if path else key)
            for key, item in value.items()
        }
    if isinstance(value, list):
        return [_normalize(item, found, allowed, f"{path}[]") for item in value]
    if isinstance(value, str) and path in allowed:
        if DAY_PATTERN.match(value):
            found.add(path)
            return PLACEHOLDER_DAY
        if re.match(r"^\d+\.\d+\.\d+", value):
            found.add(path)
            return PLACEHOLDER_VERSION
    return value


PLACEHOLDER_ROOT = "<JOURNAL_ROOT>"


def _build_journal(root: Path, *, corrupt: bool = False) -> None:
    """Create the journal the probes run against.

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


def _record(client: Any, method: str, path: str, why: str, root: Path) -> dict[str, Any]:
    response = client.open(path, method=method)
    body = response.get_data()
    content_type = response.headers.get("Content-Type", "")
    normalized_body = body.replace(str(root).encode(), PLACEHOLDER_ROOT.encode())
    case: dict[str, Any] = {
        "method": method,
        "path": path,
        "why": why,
        "status": response.status_code,
        "content_type": content_type,
        # Overwritten below for JSON cases -- see body_sha256_basis.
        "body_sha256": hashlib.sha256(normalized_body).hexdigest(),
        "body_sha256_basis": "raw-body",
        "body_bytes": len(normalized_body),
    }
    if normalized_body != body:
        # The journal root is a temporary directory; it cannot be reproduced, so
        # it is replaced before hashing rather than left to make the case unstable.
        case["body_normalized"] = [PLACEHOLDER_ROOT]
    location = response.headers.get("Location")
    if location:
        case["location"] = location
    if "json" in content_type:
        found: set[str] = set()
        allowed = NORMALIZED_FIELDS.get(path, set())
        case["json"] = _normalize(json.loads(normalized_body), found, allowed)
        case["normalized_fields"] = sorted(found)
        # 🔴 A raw-body hash is NOT reproducible for a case carrying a normalized
        # field: /app/speakers/api/state embeds `today`, so the raw hash rolls at
        # UTC midnight while the recorded JSON does not. Hash what the corpus
        # actually asserts -- the canonical normalized JSON.
        case["body_sha256"] = hashlib.sha256(
            json.dumps(case["json"], sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
        case["body_sha256_basis"] = "normalized-json"
    elif response.status_code >= 400:
        # Error bodies are short and are the whole point of the corrupt phase --
        # record them verbatim so a port can be checked against the real text.
        case["body_text"] = normalized_body.decode("utf-8", errors="replace")
    return case


def build_corpus() -> dict[str, Any]:
    from solstone.convey import create_app

    cases: dict[str, list[dict[str, Any]]] = {}

    for phase in ("unestablished", "established", "corrupt"):
        with tempfile.TemporaryDirectory(prefix=f"convey-corpus-{phase}-") as tmp:
            root = Path(tmp)
            root.mkdir(parents=True, exist_ok=True)
            if phase == "established":
                _build_journal(root)
            elif phase == "corrupt":
                _build_journal(root, corrupt=True)
            os.environ["SOLSTONE_JOURNAL"] = str(root)
            os.environ["SOLSTONE_DISABLE_CONVEY_SIDE_RUNTIMES"] = "1"
            app = create_app(str(root))
            client = app.test_client()
            cases[phase] = [_record(client, *probe, root) for probe in PROBES]

    return {
        "schema": "solstone-convey-shell-corpus-v1",
        "generator": "scripts/convey_shell_corpus.py",
        "tz": "UTC",
        "pinned_completed_at": PINNED_COMPLETED_AT,
        "placeholders": {
            "day": PLACEHOLDER_DAY,
            "version": PLACEHOLDER_VERSION,
            "journal_root": PLACEHOLDER_ROOT,
        },
        "phases": cases,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit non-zero if the corpus on disk differs from a fresh capture",
    )
    args = parser.parse_args()

    corpus = build_corpus()
    rendered = json.dumps(corpus, indent=2, sort_keys=True) + "\n"

    if args.check:
        if not CORPUS_PATH.exists():
            print(f"missing corpus: {CORPUS_PATH}", file=sys.stderr)
            return 1
        current = CORPUS_PATH.read_text()
        if current != rendered:
            print(
                f"convey shell corpus is stale: {CORPUS_PATH}\n"
                "regenerate with: python scripts/convey_shell_corpus.py",
                file=sys.stderr,
            )
            return 1
        print(f"convey shell corpus is current: {CORPUS_PATH}")
        return 0

    CORPUS_PATH.parent.mkdir(parents=True, exist_ok=True)
    CORPUS_PATH.write_text(rendered)
    total = sum(len(phase) for phase in corpus["phases"].values())
    print(f"wrote {CORPUS_PATH} ({total} cases across {len(corpus['phases'])} phases)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
