# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import re
from pathlib import Path

INIT_HTML = Path(__file__).resolve().parents[1] / "templates" / "init.html"


def _init_text() -> str:
    return INIT_HTML.read_text(encoding="utf-8")


def test_gemini_key_input_is_masked_by_default():
    text = _init_text()

    match = re.search(r'<input[^>]*\bid="gemini-key"[^>]*>', text)

    assert match, "gemini-key input not found"
    tag = match.group(0)
    assert 'type="password"' in tag
    assert 'type="text"' not in tag


def test_gemini_key_wrapped_with_password_toggle():
    text = _init_text()

    assert re.search(
        r'<div class="password-wrap">\s*<input[^>]*\bid="gemini-key"',
        text,
    )
    assert re.search(
        r'<button type="button" class="password-toggle" '
        r'data-toggle="gemini-key" title="show key">\s*'
        r"<span>&#128065;</span>",
        text,
    )


def test_result_display_ms_constant_present():
    text = _init_text()

    assert text.count("RESULT_DISPLAY_MS = 1200") == 1


def test_validate_button_starts_with_validate_label():
    text = _init_text()

    assert re.search(
        r'<button type="button" id="gemini-validate" '
        r'class="settings-preset-btn">validate</button>',
        text,
    )


def test_wizard_self_contained():
    text = _init_text()

    assert text.count('<link rel="stylesheet"') == 0
    assert text.count("<script src=") == 2
