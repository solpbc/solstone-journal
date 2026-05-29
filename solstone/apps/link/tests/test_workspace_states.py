# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Regression tests for U4 rendered link workspace states."""

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


def test_first_run_hero_present_and_zero_clients(link_env) -> None:
    env = link_env()
    response = env.client.get("/app/link/")

    assert response.status_code == 200
    body = response.get_data(as_text=True)
    body_text = _normalized_body(body)

    assert 'id="link-first-run-hero"' in body
    assert re.search(r'<section id="link-first-run-hero"[^>]{0,200}\bhidden\b', body)
    assert copy.HERO_TITLE in body_text
    assert copy.HERO_BODY in body_text
    assert copy.HERO_HOW_REACH_LABEL in body_text
    assert "isFirstRun" in body
    assert "applyFirstRunGate" in body
    assert "latestDevices.length === 0" in body
    assert "pairedSection.hidden = isFirstRun" in body

    devices_response = env.client.get("/app/link/api/devices")
    assert devices_response.status_code == 200
    assert devices_response.get_json() == {"devices": []}


def test_loading_skeletons_present(link_env) -> None:
    env = link_env()
    response = env.client.get("/app/link/")

    assert response.status_code == 200
    body = response.get_data(as_text=True)

    assert 'id="link-status-skeleton"' in body
    assert re.search(r'<span id="link-status-text"[^>]{0,80}\bhidden\b', body)
    assert body.count("link-skeleton-row") >= 2
    assert "@keyframes skeleton-pulse" in body
    assert ".skeleton-line" in body
    assert ".skeleton-block" in body
    assert 'class="surface-loading"' not in body


def test_lan_banner_replaces_nudge_and_blocks_pairing(link_env) -> None:
    env = link_env()
    response = env.client.get("/app/link/")

    assert response.status_code == 200
    body = response.get_data(as_text=True)
    body_text = _normalized_body(body)

    assert 'id="link-lan-banner"' in body
    assert re.search(r'<section id="link-lan-banner"[^>]{0,200}\bhidden\b', body)
    assert 'tabindex="-1"' in body
    old_nudge_id = "link-lan-" + "nudge"
    old_nudge_class = ".link-lan-" + "nudge"
    assert f'id="{old_nudge_id}"' not in body
    assert old_nudge_class not in body
    assert 'aria-live="polite"' in body
    assert 'id="link-lan-diy"' in body

    for value in (
        copy.LAN_BANNER_TITLE,
        copy.LAN_BANNER_BODY,
        copy.LAN_BANNER_ENABLE_CTA,
        copy.LAN_BANNER_PASSWORD_INTRO,
        copy.LAN_BANNER_DIY_LABEL,
        copy.LAN_BANNER_DIY_BODY,
    ):
        assert value in body_text
    assert "make dev PORT=0.0.0.0:5015" in body_text
    assert "convey.host" in body_text

    assert "function pairBlocked()" in body
    assert "latestStatus?.reachability === 'lan-unreachable'" in body
    assert "document.querySelectorAll('.link-pair-btn')" in body
    assert "button.disabled = pairBlocked()" in body
    assert "button.setAttribute('aria-disabled', String(pairBlocked()))" in body
    assert "function openPairModal()" in body
    assert "revealLanBanner();" in body
    assert "'/app/link/network-access'" in body
    assert "await refreshStatus()" in body
    assert "lanDiy.open = true" in body


def test_qr_expired_overlay_renders(link_env) -> None:
    env = link_env()
    response = env.client.get("/app/link/")

    assert response.status_code == 200
    body = response.get_data(as_text=True)
    body_text = _normalized_body(body)

    assert 'id="link-qr-expired"' in body
    assert re.search(r'<div id="link-qr-expired"[^>]{0,200}\bhidden\b', body)
    assert copy.EXPIRED_BUTTON in body_text
    assert ".is-expired" in body
    assert "qrContainer.classList.add('is-expired')" in body


def test_success_card_structure(link_env) -> None:
    env = link_env()
    response = env.client.get("/app/link/")

    assert response.status_code == 200
    body = response.get_data(as_text=True)
    body_text = _normalized_body(body)

    for element_id in (
        "link-pair-success",
        "link-pair-success-heading",
        "link-pair-success-subhead",
        "link-pair-remove",
        "link-pair-done",
    ):
        assert f'id="{element_id}"' in body
    assert copy.SUCCESS_VERIFY_NOTE in body_text
    assert copy.SUCCESS_REMOVE_LABEL in body_text
    assert copy.SUCCESS_DONE in body_text

    remove_start = body.index("pairRemove.addEventListener")
    remove_end = body.index("pairModal.addEventListener", remove_start)
    remove_body = body[remove_start:remove_end]
    assert "'/app/link/unpair'" in remove_body
    assert "lastPairedFingerprint" in remove_body
    assert "fingerprint:" in remove_body
    assert "openUnpair" not in remove_body

    assert (
        "RECENT_NETWORK_LABEL"
        in body[
            body.index("function handlePairComplete") : body.index(
                "function showPairError"
            )
        ]
    )


def test_pair_complete_single_refresh(link_env) -> None:
    env = link_env()
    response = env.client.get("/app/link/")

    assert response.status_code == 200
    body = response.get_data(as_text=True)

    pair_complete_start = body.index("function handlePairComplete")
    pair_complete_end = body.index("function showPairError", pair_complete_start)
    pair_complete_body = body[pair_complete_start:pair_complete_end]
    assert "refreshDevices" not in pair_complete_body

    init_start = body.index("function initLink()")
    init_end = body.index("if (document.readyState", init_start)
    init_body = body[init_start:init_end]
    assert "pair_complete" in init_body
    assert "refreshDevices" in init_body


def test_pair_modal_error_state_present(link_env) -> None:
    env = link_env()
    response = env.client.get("/app/link/")

    assert response.status_code == 200
    body = response.get_data(as_text=True)
    body_text = _normalized_body(body)

    assert 'id="link-pair-error"' in body
    assert re.search(r'<div id="link-pair-error"[^>]{0,200}\bhidden\b', body)
    assert copy.PAIR_ERROR_BODY in body_text
    assert "function showPairError" in body
    assert body.count("showPairError(") >= 2
