# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Regression test for link pair QR expired-overlay markup."""

from __future__ import annotations

import html

from solstone.apps.link import copy


def test_workspace_qr_expired_overlay(link_env) -> None:
    env = link_env()
    response = env.client.get("/app/link/")

    assert response.status_code == 200
    body = response.get_data(as_text=True)

    assert 'id="link-qr-expired"' in body
    assert ".link-qr-container.is-expired" in body
    assert copy.EXPIRED_BUTTON in html.unescape(body)
    assert "qrContainer.classList.add('is-expired')" in body
