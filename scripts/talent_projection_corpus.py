#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Conformance oracle for the talent-output text projection layer.

The formatting boundary serves two consumers. The search indexer reads chunks;
the owner-facing surface reads a *projection* — one text blob per talent-output
key under a segment's `talents/` directory, which becomes one tab per key in the
segment view and one entry in the day context a talent is given.

That layer has no record anywhere but the reference implementation, and it
carries several decisions that a rewrite would otherwise re-derive by guesswork:

  * a key is the path stem relative to the talents directory, so a nested
    directory produces a slash-bearing key — and keys are **owner-visible**,
    because each becomes a tab label;
  * `.json` is preferred over a same-key `.md`, but only when the registry
    actually dispatches a formatter for it; when it dispatches and renders
    nothing, the key is **dropped entirely** rather than falling back;
  * a filter runs on the stem **before** rendering, so a map-shaped rewrite
    would move that cost onto every caller;
  * empty rendered text is skipped, for both the json and markdown paths.

⚠ Same clock as the content-family corpus: regenerating requires a runnable
reference tree. Captured before the layer is rewritten, not after.
"""

from __future__ import annotations

import json
import os
import tempfile
import time
from pathlib import Path
from typing import Any

os.environ["TZ"] = "UTC"
if hasattr(time, "tzset"):
    time.tzset()

FIXTURE_VERSION = 1

# The talents directory this corpus builds, relative to a journal root. It sits
# at segment depth because that is where the owner-facing surface reads it.
SEGMENT_REL = "chronicle/20260304/workstation/090000_300"

# One entry per file the corpus writes under that talents directory.
FILES: list[tuple[str, str]] = [
    # a talent output the registry dispatches and renders
    ("sense.json", json.dumps({"entities": [{"name": "Ada", "type": "Person"}]})),
    # same key in both spellings — json wins
    ("screen.json", json.dumps({"summary": "Editor open", "applications": ["nvim"]})),
    ("screen.md", "# Screen\n\nthis markdown must lose to the json\n"),
    # markdown only, no json sibling
    ("activity.md", "# Activity\n\nwalked to the office\n"),
    # nested directory — the key keeps the slash
    ("work/notes.md", "# Notes\n\nnested talent output\n"),
    # a json the registry does not dispatch, with a markdown sibling that must win
    ("unknown.json", json.dumps({"whatever": 1})),
    ("unknown.md", "# Unknown\n\nmarkdown fallback for an undispatched json\n"),
    # empty markdown — skipped entirely
    ("blank.md", "   \n"),
]


def _build(root: Path) -> Path:
    talents = root / SEGMENT_REL / "talents"
    for rel, text in FILES:
        path = talents / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    return talents


def build_talent_projection_fixture() -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="talent-projection-corpus-") as tmp:
        root = Path(tmp)
        talents = _build(root)
        os.environ["SOLSTONE_JOURNAL"] = str(root)

        from solstone.think.talent_outputs import (  # noqa: PLC0415 — after SOLSTONE_JOURNAL
            iter_talent_text_projections,
            talent_projection_map,
        )

        projections = [
            {
                "key": projection.key,
                "stem": projection.stem,
                "relative_path": projection.relative_path,
                "text": projection.text,
                "source_suffix": Path(projection.source_path).suffix,
            }
            for projection in iter_talent_text_projections(talents)
        ]
        assert projections, "MEASURED NOTHING — the walker yielded no projections"

        mapped = talent_projection_map(talents)

        # The stem filter runs before rendering, so a rewrite that filters a
        # rendered map instead of the walk changes what work is done, not just
        # what is returned.
        filtered = [
            projection.key
            for projection in iter_talent_text_projections(
                talents, stem_filter=lambda stem: stem.startswith("s")
            )
        ]

        return {
            "fixture": "solstone-talent-projections",
            "fixture_version": FIXTURE_VERSION,
            "generated_by": "make core-fixtures",
            "generator_timezone": "UTC",
            "talents_dir_rel": f"{SEGMENT_REL}/talents",
            "files": [{"rel": rel, "text": text} for rel, text in FILES],
            "projections": projections,
            "map_keys": sorted(mapped),
            "stem_filter_s_keys": filtered,
            "notes": [
                "a key is the stem relative to the talents dir; nested keys keep the slash",
                "json wins over a same-key md only when the registry dispatches it",
                "an undispatched json falls through to its md sibling",
                "empty rendered text is skipped on both paths",
                "the stem filter runs before rendering, not after",
            ],
        }
