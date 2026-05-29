# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import html

from solstone.apps.link import copy


def test_render_devices_function_emits_device_sections(link_env) -> None:
    env = link_env()
    response = env.client.get("/app/link/")

    assert response.status_code == 200
    body = response.get_data(as_text=True)
    body_text = html.unescape(body).replace('\\"', '"')

    assert f'<h2 id="link-paired-h2">{copy.DEVICE_SECTION_TITLE}</h2>' in body
    assert 'id="link-devices-count"' in body
    assert copy.DEVICE_PAIR_CTA in body_text
    assert "const roleOrder = ['phone', 'observer', 'peer'];" not in body
    assert "roleLabels" not in body
    assert "No devices linked yet." not in body

    assert "document.createElement('details')" in body
    assert "link-device-group-details" in body
    assert "summary.textContent = `${label} (${devices.length})`;" in body
    assert copy.DEVICE_GROUP_LABELS["observers"] in body_text
    assert copy.DEVICE_GROUP_LABELS["peers"] in body_text

    assert "const ONLINE_THRESHOLD_SECONDS = 60;" in body
    assert "const RECENT_THRESHOLD_SECONDS = 86400;" in body
    assert "const GROUP_FILTER_THRESHOLD = 8;" in body
    assert "function deviceStatus(lastSeenIso)" in body
    assert "if (!lastSeenIso) return offline;" in body
    assert "if (Number.isNaN(then)) return offline;" in body
    for glyph in ("○", "●", "◐"):
        assert glyph in body

    assert 'class="link-recent-section"' in body
    assert 'id="link-recent-list"' in body
    assert copy.RECENT_SECTION_TITLE in body_text
    assert copy.RECENT_NETWORK_LABEL in body_text

    init_start = body.index("function initLink()")
    init_end = body.index("if (document.readyState", init_start)
    init_body = body[init_start:init_end]
    assert "window.appEvents.listen('link'" in init_body
    assert "pair_complete" in init_body

    assert copy.DEVICE_EMPTY_TITLE in body_text
    assert copy.DEVICE_EMPTY_BODY in body_text
    assert copy.REFRESH_FAIL_NOTICE in body_text
    assert copy.UNPAIR_BODY in body_text
