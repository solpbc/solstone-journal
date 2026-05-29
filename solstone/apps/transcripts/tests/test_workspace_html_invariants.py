# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from pathlib import Path


def test_workspace_html_single_purge_notice_emission():
    workspace_html = Path(__file__).resolve().parents[1] / "workspace.html"

    text = workspace_html.read_text()

    assert text.count('<div class="tr-purge-notice"') == 1, (
        "expected exactly one retention-banner emit site in workspace.html"
    )


def test_workspace_html_renders_awaiting_thinking_state():
    workspace_html = Path(__file__).resolve().parents[1] / "workspace.html"

    text = workspace_html.read_text()

    assert text.count("awaiting thinking") >= 1
    assert "tr-seg-awaiting" in text
    assert "tr-zoom-pill-awaiting" in text
    assert "seg.think" in text
    assert "rg.think" in text
