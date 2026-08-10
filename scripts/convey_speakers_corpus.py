#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Conformance oracle for the speakers READ surface, against a populated journal.

`convey_shell_corpus.py` captures the convey shell against an empty journal.
That is sufficient for the shell, the app registry and the session gate, whose
routes do not depend on journal content -- and it proves almost nothing for a
read surface, where an implementation returning empty objects for everything
passes every case. `/api/grid` answers 55 bytes on an empty journal.

This drives the reference over the read routes against the populated journal
seeded by `entity_corpus.seed_populated_speakers_journal`, and records what came
back -- including query-parameter variants and ranged media requests, neither of
which the shell corpus can express.

It is a SEPARATE file from the shell corpus on purpose. The shipped
`solstone-core-convey-shell` corpus test iterates every phase in that file and
asserts an exact probe count, so adding phases or probes there breaks a gate
that is already green.

⚠ Same clock as its sibling: regenerating requires a runnable reference tree and
the conversion deletes that tree. It is a frozen record.

⛔ The journal is built in a temporary directory by a seeder that accepts no path
to an existing journal. No value here is copied from any real journal.

Usage:
    python scripts/convey_speakers_corpus.py            # write the corpus
    python scripts/convey_speakers_corpus.py --check    # fail if it would change
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
import tempfile
from pathlib import Path
from typing import Any

os.environ["TZ"] = "UTC"
import time  # noqa: E402

if hasattr(time, "tzset"):
    time.tzset()

REPO_ROOT = Path(__file__).resolve().parent.parent
CORPUS_PATH = REPO_ROOT / "core" / "fixtures" / "convey_speakers_corpus.json"
sys.path.insert(0, str(REPO_ROOT / "scripts"))

DAY_PATTERN = re.compile(r"^\d{8}$")
PLACEHOLDER_DAY = "<TODAY>"
PLACEHOLDER_ROOT = "<JOURNAL_ROOT>"

# 🔴 Normalization is a PATH ALLOWLIST, never a shape test -- the sibling
# generator's shape test would have eaten every `day` value and both coverage
# bounds on exactly this journal, so a port returning the wrong coverage window
# would still have matched.
NORMALIZED_FIELDS: dict[str, set[str]] = {
    "/app/speakers/api/state": {"today"},
}

# Response headers worth pinning. `send_file(conditional=True)` buys range
# support, and a port that drops it passes a naive 200-and-right-bytes test while
# silently breaking audio scrubbing in the owner's browser.
RECORDED_HEADERS = (
    "Content-Type",
    "Content-Range",
    "Accept-Ranges",
    "Content-Length",
    "Content-Disposition",
    "Cache-Control",
    "Location",
)

DAY_FULL = "20260731"
DAY_NO_NPZ = "20260730"
DAY_EMPTY_LABELS = "20260729"
DAY_CORRUPT_LABELS = "20260728"

# (method, path, headers, why)
PROBES: list[tuple[str, str, dict[str, str], str]] = [
    ("GET", "/app/speakers/api/state", {}, "initial state; the only normalized field"),
    ("GET", "/app/speakers/api/grid", {}, "overview grid over a populated journal"),
    ("GET", "/app/speakers/api/index", {}, "the date_nav contract -- coverage + months"),
    ("GET", "/app/speakers/api/quality", {}, "quality counters over a 31-day journal"),
    ("GET", "/app/speakers/api/speakers/known", {}, "known voices, default sort"),
    ("GET", "/app/speakers/api/speakers/known?sort=recent", {}, "explicit default sort"),
    ("GET", "/app/speakers/api/speakers/known?sort=most_samples", {}, "underscore form normalizes to a space"),
    ("GET", "/app/speakers/api/speakers/known?sort=alphabetical", {}, "third registered sort"),
    ("GET", "/app/speakers/api/speakers/known?sort=", {}, "empty sort falls back to recent"),
    ("GET", "/app/speakers/api/speakers/known?sort=bogus", {}, "unregistered sort REFUSES; it does not degrade"),
    ("GET", "/app/speakers/api/owner/status", {}, "owner voiceprint state machine"),
    ("GET", "/app/speakers/api/discovery/cache", {}, "cached clusters, no scan"),
    ("GET", "/app/speakers/api/discovery/cluster/1/presence", {}, "a cluster that exists"),
    ("GET", "/app/speakers/api/discovery/cluster/999/presence", {}, "absent cluster: JSON envelope with the id in detail"),
    ("GET", "/app/speakers/api/discovery/cluster/-1/presence", {}, "negative id: router 404, non-JSON body"),
    ("GET", "/app/speakers/api/people/search?q=", {}, "blank query is 200 with an empty list"),
    ("GET", "/app/speakers/api/people/search?q=a", {}, "broad query -- pins the invisible limit of 8"),
    ("GET", "/app/speakers/api/people/search?q=hopper", {}, "narrow query"),
    ("GET", f"/app/speakers/api/stats/{DAY_FULL[:6]}", {}, "month stats: bare {day: count}"),
    ("GET", "/app/speakers/api/stats/999999", {}, "\\d{6} accepts an impossible month"),
    ("GET", "/app/speakers/api/stats/nope", {}, "a non-month is 400"),
    ("GET", f"/app/speakers/api/segments/{DAY_FULL}", {}, "a day with segments in two streams"),
    ("GET", f"/app/speakers/api/segments/{DAY_FULL}?limit=1", {}, "paging: limit"),
    ("GET", f"/app/speakers/api/segments/{DAY_FULL}?limit=1&offset=1", {}, "paging: offset"),
    ("GET", f"/app/speakers/api/segments/{DAY_FULL}?limit=notanint", {}, "non-int limit is 400"),
    ("GET", f"/app/speakers/api/segments/{DAY_FULL}?speaker=%20", {}, "whitespace speaker is 400"),
    ("GET", f"/app/speakers/api/speakers/{DAY_FULL}/field/090000_300", {}, "per-segment speakers, labels present"),
    ("GET", f"/app/speakers/api/speakers/{DAY_EMPTY_LABELS}/field/173000_240", {}, "labels file is {}"),
    ("GET", f"/app/speakers/api/speakers/{DAY_CORRUPT_LABELS}/desk/080000_180", {}, "labels file is corrupt"),
    ("GET", f"/app/speakers/api/review/{DAY_FULL}/field/090000_300/mic_audio", {}, "the largest handler, fully populated"),
    ("GET", f"/app/speakers/api/review/{DAY_NO_NPZ}/field/101500_120/mic_audio", {}, "🔴 transcript present, .npz ABSENT -- blank screen, no signal"),
    ("GET", f"/app/speakers/api/review/{DAY_EMPTY_LABELS}/field/173000_240/mic_audio", {}, "labels {} -- has_labels true, needs_review false"),
    ("GET", f"/app/speakers/api/review/{DAY_FULL}/desk/140000_600/sys_audio", {}, "malformed transcript line -- sentence ids carry a GAP"),
    ("GET", f"/app/speakers/api/review/{DAY_FULL}/field/999999_999/mic_audio", {}, "missing transcript is a 404 refusal"),
    ("GET", f"/app/speakers/api/serve_audio/{DAY_FULL}/field/090000_300/mic_audio.flac", {}, "media, whole body"),
    ("GET", f"/app/speakers/api/serve_audio/{DAY_FULL}/field/090000_300/mic_audio.flac", {"Range": "bytes=0-15"}, "🔴 ranged: 206 + Content-Range"),
    ("GET", f"/app/speakers/api/serve_audio/{DAY_FULL}/field/090000_300/mic_audio.flac", {"Range": "bytes=9999-"}, "unsatisfiable range is 416"),
    ("GET", f"/app/speakers/api/serve_audio/{DAY_NO_NPZ}/field/101500_120/mic_audio.flac", {"Range": "bytes=0-5"}, "⚠ ranged against a ZERO-BYTE file is 200, not 416"),
    ("GET", f"/app/speakers/api/serve_audio/{DAY_CORRUPT_LABELS}/desk/080000_180/mic_audio.png", {}, "🔴 the allowlist is all media formats, not the audio subset"),
    ("GET", f"/app/speakers/api/serve_audio/{DAY_FULL}/../../../etc/passwd", {}, "traversal refusal"),
    ("GET", f"/app/speakers/api/serve_audio/{DAY_FULL}/field/090000_300/mic_audio.xyz", {}, "unregistered extension on a file that EXISTS -- reaches the raise branch"),
    ("GET", f"/app/speakers/api/serve_audio/{DAY_FULL}/field/090000_300/mic_audio.exe", {}, "absent file: is_file() is checked BEFORE the MIME lookup, so this is 404"),
    ("GET", "/app/speakers/api/serve_audio/notaday/field/x/y.flac", {}, "non-\\d{8} day"),
]


def _normalize(value: Any, found: set[str], allowed: set[str], path: str = "") -> Any:
    if isinstance(value, dict):
        return {
            key: _normalize(item, found, allowed, f"{path}.{key}" if path else key)
            for key, item in value.items()
        }
    if isinstance(value, list):
        return [_normalize(item, found, allowed, f"{path}[]") for item in value]
    if isinstance(value, str) and path in allowed and DAY_PATTERN.match(value):
        found.add(path)
        return PLACEHOLDER_DAY
    return value


def _record(client: Any, method: str, path: str, headers: dict[str, str], why: str, root: Path) -> dict[str, Any]:
    response = client.open(path, method=method, headers=headers or None)
    body = response.get_data()
    normalized_body = body.replace(str(root).encode(), PLACEHOLDER_ROOT.encode())
    content_type = response.headers.get("Content-Type", "")
    probe_key = path.split("?")[0]
    case: dict[str, Any] = {
        "method": method,
        "path": path,
        "request_headers": headers,
        "why": why,
        "status": response.status_code,
        "headers": {
            name: response.headers.get(name)
            for name in RECORDED_HEADERS
            if response.headers.get(name) is not None
        },
        "body_bytes": len(normalized_body),
        "body_sha256": hashlib.sha256(normalized_body).hexdigest(),
        "body_sha256_basis": "raw-body",
    }
    if "json" in content_type:
        found: set[str] = set()
        allowed = NORMALIZED_FIELDS.get(probe_key, set())
        case["json"] = _normalize(json.loads(normalized_body), found, allowed)
        case["normalized_fields"] = sorted(found)
        case["body_sha256"] = hashlib.sha256(
            json.dumps(case["json"], sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
        case["body_sha256_basis"] = "normalized-json"
    return case


def build_corpus() -> dict[str, Any]:
    import entity_corpus

    from solstone.convey import create_app

    with tempfile.TemporaryDirectory(prefix="convey-speakers-corpus-") as tmp:
        root = Path(tmp)
        manifest = entity_corpus.seed_populated_speakers_journal(root)
        os.environ["SOLSTONE_JOURNAL"] = str(root)
        os.environ["SOLSTONE_DISABLE_CONVEY_SIDE_RUNTIMES"] = "1"
        app = create_app(str(root))
        client = app.test_client()
        cases = [_record(client, m, p, h, w, root) for m, p, h, w in PROBES]

    return {
        "schema": "solstone-convey-speakers-corpus-v1",
        "generator": "scripts/convey_speakers_corpus.py",
        "seeder": "entity_corpus.seed_populated_speakers_journal",
        "tz": "UTC",
        "placeholders": {"day": PLACEHOLDER_DAY, "journal_root": PLACEHOLDER_ROOT},
        "journal": {
            "days": len(manifest["days"]),
            "entities": len(manifest["entities"]),
            "segments": manifest["segments"],
            "notes": manifest["notes"],
        },
        "cases": cases,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="exit non-zero if the corpus would change")
    args = parser.parse_args()

    rendered = json.dumps(build_corpus(), indent=2, sort_keys=True) + "\n"

    if args.check:
        if not CORPUS_PATH.exists():
            print(f"missing corpus: {CORPUS_PATH}", file=sys.stderr)
            return 1
        if CORPUS_PATH.read_text() != rendered:
            print(
                f"speakers corpus is stale: {CORPUS_PATH}\n"
                "regenerate with: python scripts/convey_speakers_corpus.py",
                file=sys.stderr,
            )
            return 1
        print(f"speakers corpus is current: {CORPUS_PATH}")
        return 0

    CORPUS_PATH.parent.mkdir(parents=True, exist_ok=True)
    CORPUS_PATH.write_text(rendered)
    print(f"wrote {CORPUS_PATH} ({len(PROBES)} cases)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
