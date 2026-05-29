# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Regression tests for the rendered link reach shell."""

from __future__ import annotations

import html
import re

from solstone.apps.link import copy


def _normalized_body(body: str) -> str:
    return (
        html.unescape(body)
        .replace('\\"', '"')
        .replace("\\u0027", "'")
        .replace("\\u00b7", "·")
        .replace("\\u2014", "—")
        .replace("\\u2192", "→")
    )


def test_workspace_renders_reach_shell_copy_and_static_guards(link_env) -> None:
    env = link_env()
    response = env.client.get("/app/link/")

    assert response.status_code == 200
    body = response.get_data(as_text=True)
    body_text = _normalized_body(body)

    for gone in (
        "reach your solstone from anywhere",
        "blind by construction",
        "reachable from the internet",
        "typeof data.enrolled !== 'boolean'",
    ):
        assert gone not in body_text

    assert copy.HEADER_TRUST_LINE in body_text
    for value in copy.STATUS_SENTENCES.values():
        assert value in body_text
    assert copy.REACH_CARD_TITLE in body_text
    assert copy.REACH_DIRECT_LABEL in body_text
    assert copy.REACH_UPGRADE_LINK_LABEL in body_text

    assert re.search(
        r'<a href="https://services\.solstone\.app/" '
        r'target="_blank" rel="noopener noreferrer">[^<]+</a>',
        body,
    )
    assert 'id="link-posture-modal"' in body
    assert re.search(r'<div id="link-posture-modal"[^>]{0,200}\bhidden\b', body)
    for color in ("#1e7b42", "#b88400", "#a53a1f"):
        assert color in body
    assert "SurfaceState.replaceLoading('link-status-panel'" in body
    assert 'id="link-pair-btn"' in body
    assert "pair a phone" in body_text

    for forbidden in (
        "'/posture'",
        '"/posture"',
        "posture-set",
        "'/config'",
        '"/config"',
    ):
        assert forbidden not in body
