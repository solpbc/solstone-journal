# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for speakers owner-facing copy discipline."""

from __future__ import annotations

import re
from pathlib import Path

from solstone.apps.speakers.copy import speaker_copy_payload, speaker_copy_values


def test_no_literal_copy_in_templates():
    """Templates reference copy constants; Python protocol keys are out of scope.

    Several short copy values are also API keys or status values in Python
    sources. This check intentionally covers the render surfaces where copy is
    owner-visible and avoids treating those protocol tokens as copy literals.
    """

    root = Path("solstone/apps/speakers")

    hits: list[tuple[Path, str]] = []
    for path in root.rglob("*.html"):
        text = path.read_text(encoding="utf-8")
        for value in speaker_copy_values():
            literal_patterns = (
                re.compile(rf">\s*{re.escape(value)}\s*<"),
                re.compile(rf"(?<!=)['\"`]{re.escape(value)}['\"`]"),
            )
            if any(pattern.search(text) for pattern in literal_patterns):
                hits.append((path, value))

    assert hits == []


def test_all_copy_constants_referenced_by_render_surface():
    html = "\n".join(
        path.read_text(encoding="utf-8")
        for path in Path("solstone/apps/speakers").rglob("*.html")
    )

    missing = [name for name in speaker_copy_payload() if name not in html]

    assert missing == []
