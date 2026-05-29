# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from pathlib import Path


def _function_body(text: str, name: str) -> str:
    start = text.index(f"function {name}(")
    nxt = text.index("\n  //", start + 1)
    return text[start:nxt]


def test_selected_pill_uses_status_inactive_fallback():
    fn = _function_body(
        Path("solstone/convey/static/app.js").read_text(encoding="utf-8"),
        "applyPillStyle",
    )

    assert "pill.style.background = facet.color || 'var(--status-inactive)';" in fn
    assert "pill.style.borderColor = facet.color || 'var(--status-inactive)';" in fn
    assert "pill.style.color = 'white';" in fn


def test_unselected_pill_reset_and_color_setters_unchanged():
    fn = _function_body(
        Path("solstone/convey/static/app.js").read_text(encoding="utf-8"),
        "applyPillStyle",
    )

    assert "pill.style.setProperty('--pill-color', facet.color);" in fn
    assert "pill.style.setProperty('--pill-bg', hexToRgba(facet.color, 0.2));" in fn
    assert (
        "pill.style.setProperty('--pill-bg-rest', hexToRgba(facet.color, 0.08));" in fn
    )
    assert "pill.style.background = '';" in fn
    assert "pill.style.color = '';" in fn
    assert "pill.style.borderColor = '';" in fn
