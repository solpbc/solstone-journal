# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from pathlib import Path

import pytest

CHAT_WORKSPACE = Path(
    "core/crates/solstone-core-records-web/assets/chat/workspace.html"
)


@pytest.fixture
def chat_html():
    return CHAT_WORKSPACE.read_text(encoding="utf-8")


def test_live_script_creates_phase_one_placeholder(chat_html):
    assert "const pendingPlaceholders = [];" in chat_html
    assert "window.solChatCopy.CHAT_LIVENESS_THINKING" in chat_html
    assert "chat-event--placeholder" in chat_html
    assert "placeholder.dataset.kind = 'sol_placeholder';" in chat_html
    assert "chat-bubble--placeholder" in chat_html


def test_placeholder_is_excluded_from_event_bookkeeping(chat_html):
    assert chat_html.count('.chat-event:not([data-kind="sol_placeholder"])') >= 3


def test_live_script_updates_placeholder_on_talent_spawned(chat_html):
    assert "String(msg.task || '').trim()" in chat_html
    assert (
        "window.solChatCopy.talentLabel(String(msg.name || ''), 'running')" in chat_html
    )
    assert "window.solChatCopy.CHAT_LIVENESS_TASK_FORMAT" in chat_html
    assert "catch (_err)" in chat_html
    assert "unknown talent target" in chat_html


def test_live_script_removes_placeholder_on_terminal_events(chat_html):
    assert "kind === 'sol_message' || kind === 'chat_error'" in chat_html
    assert "pendingPlaceholders.shift()" in chat_html
    assert (
        "placeholder.element.parentNode.removeChild(placeholder.element)" in chat_html
    )
    assert "detail: msg.detail || ''" in chat_html


def test_chat_thinking_live_js_handler_is_wired(chat_html):
    renderer = Path("solstone/convey/static/chat_render.js").read_text(encoding="utf-8")

    assert "button.chat-thinking-expander" in chat_html
    assert "toggleThinkingSurface(thinkingExpander)" in chat_html
    assert "button.dataset.thinkingId" in chat_html
    assert "content.textContent = contentText" in renderer
    assert "innerHTML = contentText" not in renderer


def test_chat_error_detail_live_js_handler_is_wired(chat_html):
    renderer = Path("solstone/convey/static/chat_render.js").read_text(encoding="utf-8")

    assert "button.chat-error-detail-expander" in chat_html
    assert "toggleErrorDetailSurface(errorDetailExpander)" in chat_html
    assert "button.dataset.errorDetailId" in chat_html
    assert "detail: msg.detail || ''" in chat_html
    assert "provider: msg.provider || ''" in chat_html
    assert "code.textContent = detailText" in renderer
    assert "innerHTML = detailText" not in renderer
    assert "button.dataset.errorDetailId" in renderer


def test_empty_state_copy_is_distinct_from_error_state(chat_html):
    history_block = chat_html.split("function renderHistory", 1)[1].split(
        "function emptyEventItem", 1
    )[0]
    error_block = chat_html.split("function renderErrorState(error)", 1)[1].split(
        "function renderHistory", 1
    )[0]

    assert "const CHAT_EMPTY_COPY = 'no chat yet on this day';" in chat_html
    assert "item.className = 'chat-empty';" in history_block
    assert "item.textContent = CHAT_EMPTY_COPY;" in history_block
    assert "item.className = 'chat-state';" not in history_block
    assert "item.className = 'chat-state';" in error_block
    assert "window.SurfaceState.error" in error_block
