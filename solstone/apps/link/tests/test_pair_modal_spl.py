# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Pair modal posture rendering regressions."""

from __future__ import annotations

from solstone.apps.link import copy

SPL_TEST_TOTP_SECRET = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"


def _link_pair_script(body: str) -> str:
    start = body.index("const LINK_POSTURE")
    end = body.index("function getUnpairFocusable", start)
    return body[start:end]


def test_spl_pair_modal_is_qr_only_with_rotation_affordance(link_env) -> None:
    env = link_env(posture="spl", totp_secret=SPL_TEST_TOTP_SECRET)

    response = env.client.get("/app/link/")

    assert response.status_code == 200
    body = response.get_data(as_text=True)
    assert 'id="link-pair-manual-code"' not in body
    assert 'id="link-pair-rotation-ring"' in body
    assert copy.PAIR_ROTATE_NOTE in body
    assert copy.WINDOW_CLOSED_BUTTON in body
    assert copy.EXPIRED_BUTTON not in body
    assert "this code expired" not in body
    assert "countdown-number" not in body
    assert 'const LINK_POSTURE = "spl";' in body
    pair_script = _link_pair_script(body)
    assert "Number(data.expires_in) || 300" in pair_script
    assert "rotationTimer = setTimeout" in pair_script
    assert "expiresIn * 1000" in pair_script
    assert pair_script.count("5 * 60 * 1000") == 1


def test_direct_pair_modal_keeps_manual_code_and_expired_copy(link_env) -> None:
    env = link_env()

    response = env.client.get("/app/link/")

    assert response.status_code == 200
    body = response.get_data(as_text=True)
    assert 'id="link-pair-manual-code"' in body
    assert copy.EXPIRED_BUTTON in body
    assert copy.PAIR_ROTATE_NOTE not in body
    assert 'const LINK_POSTURE = "direct";' in body
