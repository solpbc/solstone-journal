# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Browser regression test for link pair QR sizing."""

from __future__ import annotations

import re
import threading

import pytest
from werkzeug.serving import make_server


def _parse_px_dimension(value: str | None) -> int:
    assert value
    match = re.fullmatch(r"(\d+)(?:px)?", value)
    assert match is not None
    return int(match.group(1))


@pytest.fixture
def live_server(link_env):
    env = link_env()
    server = make_server("127.0.0.1", 0, env.app, threaded=True)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}"
    finally:
        server.shutdown()
        thread.join(timeout=5)


def test_pair_modal_qr_renders_at_usable_size(live_server, page):
    page.goto(f"{live_server}/app/link/")
    page.locator("#link-pair-btn").click()
    page.wait_for_selector("#link-qr-container svg")

    svg = page.locator("#link-qr-container svg")
    width_attr = _parse_px_dimension(svg.get_attribute("width"))
    height_attr = _parse_px_dimension(svg.get_attribute("height"))
    module_count = int(svg.get_attribute("data-module-count") or "0")

    assert module_count <= 37
    assert width_attr >= 200
    assert height_attr >= 200

    box = svg.bounding_box()
    assert box is not None
    assert box["width"] >= 200
    assert box["height"] >= 200
