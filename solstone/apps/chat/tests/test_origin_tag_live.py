# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from pathlib import Path

from solstone.convey.sol_initiated.copy import (
    KIND_OWNER_CHAT_DISMISSED,
    KIND_OWNER_CHAT_OPEN,
    KIND_SOL_CHAT_REQUEST,
    KIND_SOL_CHAT_REQUEST_SUPERSEDED,
)


CHAT_WORKSPACE = Path(
    "core/crates/solstone-core-records-web/assets/chat/workspace.html"
)


def test_live_append_origin_tag_script_handles_sol_initiated_events():
    fragment = CHAT_WORKSPACE.read_text(encoding="utf-8")
    renderer = Path("solstone/convey/static/chat_render.js").read_text(encoding="utf-8")

    assert "let pendingSolChatRequest = null;" in fragment
    assert "origin: origin" in fragment
    assert "const supersededRequestId = msg.request_id || '';" in fragment
    assert KIND_SOL_CHAT_REQUEST in fragment
    assert KIND_SOL_CHAT_REQUEST_SUPERSEDED in fragment
    assert KIND_OWNER_CHAT_OPEN in fragment
    assert KIND_OWNER_CHAT_DISMISSED in fragment

    assert "renderOriginTag" in renderer
    assert "item.dataset.requestId = event.origin.request_id;" in renderer


def test_workspace_scopes_talent_events_column_rule():
    fragment = CHAT_WORKSPACE.read_text(encoding="utf-8")
    css = Path("solstone/convey/static/app.css").read_text(encoding="utf-8")
    rule = """  .chat-transcript .chat-event--talent {
    align-items: center;
    flex-direction: column;
  }"""

    assert rule in fragment
    assert ".chat-transcript .chat-event--talent" not in css


def test_workspace_scopes_hidden_origin_provenance_rule():
    fragment = CHAT_WORKSPACE.read_text(encoding="utf-8")
    css = Path("solstone/convey/static/app.css").read_text(encoding="utf-8")
    rule = """  .chat-transcript .chat-origin-provenance[hidden] {
    display: none;
  }"""

    assert rule in fragment
    assert ".chat-transcript .chat-origin-provenance[hidden]" not in css
