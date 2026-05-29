# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for entities owner-facing copy discipline."""

from __future__ import annotations

import json
import re
from pathlib import Path

from solstone.apps.entities.copy import entities_copy_payload, entities_copy_values


def test_no_literal_copy_in_templates():
    """Templates reference ENT_COPY constants; prose values are never inlined."""

    root = Path("solstone/apps/entities")

    hits: list[tuple[Path, str]] = []
    for path in root.rglob("*.html"):
        text = path.read_text(encoding="utf-8")
        for value in entities_copy_values():
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
        for path in Path("solstone/apps/entities").rglob("*.html")
    )

    missing = [name for name in entities_copy_payload() if name not in html]

    assert missing == []


def test_entities_index_injects_copy(client):
    resp = client.get("/app/entities/")
    assert resp.status_code == 200
    html = resp.get_data(as_text=True)
    match = re.search(r"const ENT_COPY = (\{.*\});", html)
    assert match, "ENT_COPY assignment not found in rendered page"
    assert json.loads(match.group(1)) == entities_copy_payload()
