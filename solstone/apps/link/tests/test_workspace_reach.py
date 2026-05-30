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
        # no unconditional relay claim in the header — false in direct posture
        "sol pbc carries the connection — but can never see inside it",
    ):
        assert gone not in body_text

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
    assert "pair a device" in body_text

    for forbidden in (
        "'/posture'",
        '"/posture"',
        "posture-set",
        "'/config'",
        '"/config"',
    ):
        assert forbidden not in body


def test_workspace_renders_spl_reach_card_and_states(link_env) -> None:
    env = link_env()
    response = env.client.get("/app/link/")

    assert response.status_code == 200
    body = response.get_data(as_text=True)
    body_text = _normalized_body(body)

    assert 'id="link-spl-reach-card"' in body
    assert re.search(r'<section id="link-spl-reach-card"[^>]{0,200}\bhidden\b', body)
    for value in (
        copy.POSTURE_SPL_TITLE,
        copy.REACH_SPL_ACTIVE_BODY,
        copy.REACH_SPL_TRUST_LINE,
        copy.REACH_SPL_MANAGE_LABEL,
    ):
        assert value in body_text
    spl_start = body_text.index('<section id="link-spl-reach-card"')
    spl_end = body_text.index("</section>", spl_start)
    spl_card = body_text[spl_start:spl_end]
    assert re.search(
        r'<a href="https://services\.solstone\.app/" '
        r'target="_blank" rel="noopener noreferrer">'
        + re.escape(copy.REACH_SPL_MANAGE_LABEL)
        + r"</a>",
        spl_card,
    )

    assert 'id="link-spl-connecting-note"' in body
    assert copy.REACH_SPL_CONNECTING_NOTE in body_text
    assert 'id="link-spl-check-again"' in body
    assert f"[ {copy.CHECK_AGAIN_LABEL} ]" in body_text
    assert "splCheckAgain.addEventListener('click', refreshStatus)" in body


def test_workspace_keeps_spl_trust_line_out_of_header_and_direct_card(
    link_env,
) -> None:
    env = link_env()
    response = env.client.get("/app/link/")

    assert response.status_code == 200
    body_text = _normalized_body(response.get_data(as_text=True))

    header = body_text[body_text.index("<header") : body_text.index("</header>")]
    direct_start = body_text.index('<section id="link-reach-card"')
    direct_end = body_text.index('<section id="link-spl-reach-card"', direct_start)
    direct_card = body_text[direct_start:direct_end]

    assert copy.REACH_SPL_TRUST_LINE not in header
    assert copy.REACH_SPL_TRUST_LINE not in direct_card


def test_workspace_maps_spl_status_without_red_offline_dot(link_env) -> None:
    env = link_env()
    response = env.client.get("/app/link/")

    assert response.status_code == 200
    body = response.get_data(as_text=True)

    select_start = body.index("function selectStatusSentenceKey")
    select_end = body.index("function setStatusSentence", select_start)
    select_body = body[select_start:select_end]
    assert (
        "if (reachability === 'lan-unreachable') return 'lan_unreachable';"
        in select_body
    )
    assert "if (posture === 'spl')" in select_body
    assert "if (reachability === 'offline') return 'spl_offline';" in select_body
    assert select_body.index("if (posture === 'spl')") < select_body.index(
        "if (reachability === 'offline') return 'offline';"
    )

    status_start = body.index("function setStatusSentence")
    status_end = body.index("function renderVpnCandidates", status_start)
    status_body = body[status_start:status_end]
    assert "['direct_online', 'direct_online_vpn', 'spl_online']" in status_body
    assert "['offline', 'lan_unreachable']" in status_body
    assert "spl_offline" not in status_body
