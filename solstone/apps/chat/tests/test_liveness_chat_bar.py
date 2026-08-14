# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from pathlib import Path

import pytest

CHAT_CHROME = Path("solstone/convey/static/chat_chrome.js")
SHELL = Path("solstone/convey/static/shell.html")
CHAT_WORKSPACE = Path(
    "core/crates/solstone-core-records-web/assets/chat/workspace.html"
)


def _read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def _js_function_block(source: str, name: str) -> str:
    start = source.index(f"function {name}")
    brace_start = source.index("{", start)
    depth = 0
    for index in range(brace_start, len(source)):
        char = source[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return source[start : index + 1]
    raise AssertionError(f"function {name} block not found")


@pytest.fixture
def chat_html():
    return (
        SHELL.read_text(encoding="utf-8")
        + CHAT_WORKSPACE.read_text(encoding="utf-8")
        + CHAT_CHROME.read_text(encoding="utf-8")
    )


def test_chat_bar_sets_phase_one_from_owner_message(chat_html):
    assert "const chatBarPendingPlaceholders = [];" in chat_html
    assert (
        "if (!solRequestState) {\n        chatBarPendingPlaceholders.push({"
    ) in chat_html
    assert "window.solChatCopy.CHAT_LIVENESS_THINKING" in chat_html
    assert (
        "setStatus(window.solChatCopy.CHAT_LIVENESS_THINKING, "
        "window.solChatCopy.CHAT_LIVENESS_THINKING);"
    ) in chat_html
    assert "statusWrap.classList.add('chat-bar-status--thinking');" in chat_html
    assert "statusWrap.classList.remove('chat-bar-status--error');" in chat_html


def test_chat_bar_sets_phase_two_without_blocking_talent_tray(chat_html):
    assert "upsertTalent({" in chat_html
    assert "if (!solRequestState && chatBarPendingPlaceholders.length > 0)" in chat_html
    assert "String(msg.task || '').trim()" in chat_html
    assert (
        "window.solChatCopy.talentLabel(String(msg.name || ''), 'running')" in chat_html
    )
    assert "window.solChatCopy.CHAT_LIVENESS_TASK_FORMAT" in chat_html
    assert "setStatus(composed, composed);" in chat_html


def test_chat_bar_enter_submits(chat_html):
    assert "function handleComposerKeydown(event)" in chat_html
    assert "event.isComposing === true || event.keyCode === 229" in chat_html
    assert "event.key === 'Enter' && event.shiftKey" in chat_html
    assert "event.key === 'Enter'" in chat_html
    assert "form.requestSubmit()" in chat_html
    assert "input.addEventListener('keydown', handleComposerKeydown);" in chat_html
    assert chat_html.count("input.addEventListener('keydown'") == 1


def test_chat_bar_terminal_overwrites_liveness_without_retry_button(chat_html):
    assert "function clearPendingLivenessStatus()" in chat_html
    assert (
        "if (chatBarPendingPlaceholders.length > 0) chatBarPendingPlaceholders.shift();"
        in chat_html
    )
    assert "clearPendingLivenessStatus();" in chat_html
    assert "setStatus(msg.text || '', statusTitleFor(msg));" in chat_html
    assert "function statusTitleFor(msg)" in chat_html
    status_title_block = chat_html.split("function statusTitleFor(msg)", 1)[1].split(
        "function renderJobsIndicator()", 1
    )[0]
    assert "window.solChatCopy.CHAT_DISPATCH_ORIGIN_PREFIX" in status_title_block
    assert "msg.notes || msg.text || ''" in status_title_block
    assert (
        "setStatus(renderedReason.message, renderedReason.message, renderedReason.action);"
        in chat_html
    )
    assert "statusWrap.classList.remove('chat-bar-status--thinking');" in chat_html
    assert "statusWrap.classList.add('chat-bar-status--error');" in chat_html
    assert "statusErrorActive = true;" in chat_html
    assert "window.location.href = '/app/chat/';" in chat_html

    app_template = Path("solstone/convey/static/chat_chrome.js").read_text(
        encoding="utf-8"
    )
    retry_class = "-".join(("chat", "error", "retry"))
    assert retry_class not in app_template


def test_support_draft_card_structure_and_result_helper(chat_html):
    assert (
        '<div id="chatBarResult" class="chat-bar-result" aria-live="polite" hidden>'
        in chat_html
    )
    assert 'class="chat-bar-draft-card" role="group" aria-label=""' in chat_html
    assert (
        "draftCardEl.setAttribute('aria-label', 'support draft for review')"
        in chat_html
    )
    assert "chat-bar-draft-route-from" in chat_html
    assert "chat-bar-draft-route-to" in chat_html
    assert "window.solChatCopy.CHAT_CAPACITY_SUPPORT_ROUTE_FROM" in chat_html
    assert "window.solChatCopy.CHAT_CAPACITY_SUPPORT_ROUTE_TO" in chat_html
    assert "chat-bar-draft-lead" not in chat_html
    assert "function showSupportDraft(draft)" in chat_html
    assert "showSupportDraft(msg.draft);" in chat_html

    assert "function renderSupportOutcome(msg)" in chat_html
    assert '"support draft submitted"' in chat_html
    assert '"support draft failed"' in chat_html
    assert '"support draft ambiguous"' in chat_html
    assert '"support draft in_progress"' in chat_html
    assert '"support draft re_consent_required"' in chat_html
    assert '"support draft cancelled"' in chat_html
    assert "if (!renderSupportOutcome(msg))" in chat_html
    assert "function hideSupportResult()" in chat_html
    assert "function reenableSupportDraft()" in chat_html
    assert (
        "postChatMessage(window.solChatCopy.CHAT_RESULT_TRY_AGAIN_MESSAGE)"
        not in chat_html
    )
    assert chat_html.count("setStatus(msg.text || '', statusTitleFor(msg));") == 2


def test_chat_bar_talent_terminal_clears_liveness(chat_html):
    assert "if (eventName === 'talent_finished')" in chat_html
    assert "if (eventName === 'talent_errored')" in chat_html
    assert (
        "if (!solRequestState && chatBarPendingPlaceholders.length > 0) {\n"
        "        clearPendingLivenessStatus();\n"
        "        setStatus('', '');\n"
        "      }"
    ) in chat_html


def test_app_bar_jobs_indicator_and_composer_state_are_source_wired():
    source = _read(CHAT_CHROME)

    pending_block = _js_function_block(source, "setPendingState")
    assert "pendingSend = !!active;" in pending_block
    assert "input.disabled" not in pending_block
    assert "sendBtn.disabled" not in pending_block

    disable_block = _js_function_block(source, "disableComposer")
    assert "pendingSend = true;" in disable_block
    assert "input.disabled = true;" in disable_block
    assert "sendBtn.disabled = true;" in disable_block
    assert "disableComposer();" in source

    assert "setPendingState(true);" in source
    assert "setPendingState(false);" in source
    assert "if (!input || !sendBtn || pendingSend) return;" in source
    assert "if (pendingSend) return;" in source

    assert "const queuedJobs = new Map();" in source
    assert "function renderJobsIndicator()" in source
    assert "runningCount + queuedJobs.size" in source
    assert "window.solChatCopy.CHAT_JOBS_INDICATOR_SINGULAR" in source
    assert "window.solChatCopy.CHAT_JOBS_INDICATOR_PLURAL_FORMAT" in source
    assert "eventName === 'talent_queued'" in source
    assert "queuedJobs.set(queuedUseId" in source
    assert source.count("queuedJobs.delete(String(msg.use_id || ''));") == 3
    assert "data.queued_talents" in source

    assert "function setQueueDepth" not in source
    assert "eventName === 'chat_queue_depth'" not in source


def test_app_bar_talent_tray_reflects_in_flight_only():
    source = _read(CHAT_CHROME)

    # Removal helper exists and re-renders, mirroring upsertTalent.
    assert "function removeTalent(useId)" in source
    assert "talentState.delete(useId);" in source

    # Load path seeds the tray from in-flight talents only.
    assert "data.active_talents" in source
    assert "data.completed_talents" not in source

    # No terminal status is ever written into the tray state.
    assert "status: 'finished'" not in source
    assert "status: 'errored'" not in source

    # Both terminal handlers clear the dot instead of keeping a finished/errored chip.
    assert source.count("removeTalent(String(msg.use_id || ''))") == 2

    # Running lifecycle is intact: spawn + active-seed still mark running.
    assert "status: 'running'" in source

    # Queued path untouched (no tray dots for queued talents).
    assert "data.queued_talents" in source
    assert source.count("queuedJobs.delete(String(msg.use_id || ''));") == 3
