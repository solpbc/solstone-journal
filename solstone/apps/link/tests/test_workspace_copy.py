# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Regression tests for rendered link pair-flow copy."""

from __future__ import annotations

import html
import re

from solstone.apps.link import copy

MODAL_COPY_VALUES = [
    copy.MODAL_TITLE,
    copy.STEP_1,
    copy.STEP_2,
    copy.STEP_3,
    copy.MANUAL_CODE_LABEL,
    copy.PAIR_NETWORK_LINE,
    copy.DEVICE_LABEL_FIELD_LABEL,
    copy.DETAILS_DISCLOSURE,
    copy.CA_FP_LABEL,
    copy.CA_FP_NOTE,
    copy.DEVICE_LABEL_PLACEHOLDER,
    copy.DEVICE_LABEL_DEFAULT_FORMAT,
    copy.EXPIRED_BUTTON,
    copy.PAIR_ERROR_BODY,
    copy.SUCCESS_HEADING,
    copy.SUCCESS_SUBHEAD,
    copy.SUCCESS_DONE,
    copy.SUCCESS_VERIFY_NOTE,
    copy.SUCCESS_REMOVE_LABEL,
]


def test_workspace_renders_pair_flow_copy_and_qr_script(link_env) -> None:
    env = link_env()
    response = env.client.get("/app/link/")

    assert response.status_code == 200
    body = response.get_data(as_text=True)
    body_text = (
        html.unescape(body)
        .replace('\\"', '"')
        .replace("\\u00b7", "·")
        .replace("\\u2014", "—")
        .replace("\\u2192", "→")
    )

    for value in MODAL_COPY_VALUES:
        assert value in body_text

    assert "QR rendering lib not bundled yet" not in body
    assert "link-pair-generate" not in body
    assert "Waiting for phone" not in body
    assert "data.pair_url" not in body
    assert "pair_url" not in body
    assert "pairing-qr.js" in body
    assert re.search(r'<div id="link-pair-success"[^>]{0,200}\bhidden\b', body)
