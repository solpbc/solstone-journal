# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json
from pathlib import Path

import pytest

from solstone.apps.chat import copy as chat_copy


def _extract_object_literal(text: str, marker: str) -> dict:
    start = text.index(marker) + len(marker)
    depth = 0
    in_string = False
    escaped = False
    object_start = None

    for index in range(start, len(text)):
        char = text[index]
        if in_string:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                in_string = False
            continue
        if char == '"':
            in_string = True
            continue
        if char == "{":
            if object_start is None:
                object_start = index
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0 and object_start is not None:
                return json.loads(text[object_start : index + 1])

    raise AssertionError(f"Could not extract object after marker {marker!r}")


def test_talent_label_for_all_known_combinations():
    expected = {
        ("read", "running"): chat_copy.TALENT_LABEL_READ_RUNNING,
        ("read", "finished"): chat_copy.TALENT_LABEL_READ_FINISHED,
        ("read", "errored"): chat_copy.TALENT_LABEL_READ_ERRORED,
        ("exec", "running"): chat_copy.TALENT_LABEL_EXEC_RUNNING,
        ("exec", "finished"): chat_copy.TALENT_LABEL_EXEC_FINISHED,
        ("exec", "errored"): chat_copy.TALENT_LABEL_EXEC_ERRORED,
        ("support", "running"): chat_copy.TALENT_LABEL_SUPPORT_RUNNING,
        ("support", "finished"): chat_copy.TALENT_LABEL_SUPPORT_FINISHED,
        ("support", "errored"): chat_copy.TALENT_LABEL_SUPPORT_ERRORED,
    }

    for (target, status), label in expected.items():
        assert chat_copy.talent_label_for(target, status) == label


def test_talent_label_for_unknown_values_raise():
    with pytest.raises(ValueError, match="no chat talent label"):
        chat_copy.talent_label_for("search", "running")

    with pytest.raises(ValueError, match="no chat talent label"):
        chat_copy.talent_label_for("exec", "queued")


def test_liveness_and_error_detail_copy_bytes():
    assert chat_copy.CHAT_LIVENESS_THINKING == "sol is thinking…"
    assert chat_copy.CHAT_LIVENESS_TASK_FORMAT == "{label} {task}"
    assert chat_copy.CHAT_ERROR_DETAIL_EXPANDER_LABEL == "show details"
    assert chat_copy.CHAT_ERROR_DETAIL_COLLAPSER_LABEL == "hide details"


def test_support_lifecycle_copy_is_owner_facing():
    assert "open list" in chat_copy.CHAT_SUPPORT_CLOSE_SUBMITTED
    assert "minimal closed record" in chat_copy.CHAT_SUPPORT_RESOLVED_SUBMITTED
    assert "proposed close was cancelled" in chat_copy.CHAT_SUPPORT_STILL_NEED_HELP_SUBMITTED
    assert "try again" in chat_copy.CHAT_SUPPORT_IN_PROGRESS
    assert "terms changed" in chat_copy.CHAT_SUPPORT_RECONSENT_NEEDED


def test_jobs_and_dispatch_origin_copy_bytes():
    assert chat_copy.CHAT_TALENT_QUEUED_LABEL == "waiting to start…"
    assert "…" in chat_copy.CHAT_TALENT_QUEUED_LABEL
    assert chat_copy.CHAT_DISPATCH_ORIGIN_PREFIX == "in reply to:"
    assert chat_copy.CHAT_JOBS_INDICATOR_SINGULAR == "sol is running 1 job"
    assert chat_copy.CHAT_JOBS_INDICATOR_PLURAL_FORMAT == "sol is running {count} jobs"


def test_thinking_copy_bytes():
    expected = """CHAT_THINKING_EXPANDER_LABEL = "show thinking"
CHAT_THINKING_COLLAPSER_LABEL = "hide thinking"
CHAT_THINKING_SETTING_LABEL = "thinking surfaces"
CHAT_THINKING_OPT_ON_TAP = "show on tap"
CHAT_THINKING_OPT_ALWAYS = "always show"
CHAT_THINKING_OPT_NEVER = "never show"
CHAT_THINKING_SETTING_HELP = "sol does some thinking before replying. choose how much you want to see."
"""
    actual = "\n".join(
        [
            f'CHAT_THINKING_EXPANDER_LABEL = "{chat_copy.CHAT_THINKING_EXPANDER_LABEL}"',
            f'CHAT_THINKING_COLLAPSER_LABEL = "{chat_copy.CHAT_THINKING_COLLAPSER_LABEL}"',
            f'CHAT_THINKING_SETTING_LABEL = "{chat_copy.CHAT_THINKING_SETTING_LABEL}"',
            f'CHAT_THINKING_OPT_ON_TAP = "{chat_copy.CHAT_THINKING_OPT_ON_TAP}"',
            f'CHAT_THINKING_OPT_ALWAYS = "{chat_copy.CHAT_THINKING_OPT_ALWAYS}"',
            f'CHAT_THINKING_OPT_NEVER = "{chat_copy.CHAT_THINKING_OPT_NEVER}"',
            f'CHAT_THINKING_SETTING_HELP = "{chat_copy.CHAT_THINKING_SETTING_HELP}"',
            "",
        ]
    )

    assert actual == expected


def test_js_parity():
    js_path = Path("solstone/convey/static/chat_copy.js")
    text = js_path.read_text(encoding="utf-8")
    js_labels = _extract_object_literal(text, "const TALENT_LABELS = ")

    assert js_labels == {
        "read": {
            "running": chat_copy.TALENT_LABEL_READ_RUNNING,
            "finished": chat_copy.TALENT_LABEL_READ_FINISHED,
            "errored": chat_copy.TALENT_LABEL_READ_ERRORED,
        },
        "exec": {
            "running": chat_copy.TALENT_LABEL_EXEC_RUNNING,
            "finished": chat_copy.TALENT_LABEL_EXEC_FINISHED,
            "errored": chat_copy.TALENT_LABEL_EXEC_ERRORED,
        },
        "support": {
            "running": chat_copy.TALENT_LABEL_SUPPORT_RUNNING,
            "finished": chat_copy.TALENT_LABEL_SUPPORT_FINISHED,
            "errored": chat_copy.TALENT_LABEL_SUPPORT_ERRORED,
        },
    }
    assert (
        f'CHAT_JOBS_INDICATOR_SINGULAR: "{chat_copy.CHAT_JOBS_INDICATOR_SINGULAR}"'
        in text
    )
    assert (
        "CHAT_JOBS_INDICATOR_PLURAL_FORMAT: "
        f'"{chat_copy.CHAT_JOBS_INDICATOR_PLURAL_FORMAT}"'
    ) in text
    for jobs_copy in (
        chat_copy.CHAT_JOBS_INDICATOR_SINGULAR,
        chat_copy.CHAT_JOBS_INDICATOR_PLURAL_FORMAT,
    ):
        assert "sol is running" in jobs_copy
        assert "Sol" not in jobs_copy
        assert "sol pbc" not in jobs_copy
    assert (
        f'CHAT_QUEUE_DEPTH_CAP_MESSAGE: "{chat_copy.CHAT_QUEUE_DEPTH_CAP_MESSAGE}"'
        in text
    )
    assert f'CHAT_TALENT_QUEUED_LABEL: "{chat_copy.CHAT_TALENT_QUEUED_LABEL}"' in text
    assert (
        f'CHAT_DISPATCH_ORIGIN_PREFIX: "{chat_copy.CHAT_DISPATCH_ORIGIN_PREFIX}"'
        in text
    )
    assert "Sol" not in chat_copy.CHAT_DISPATCH_ORIGIN_PREFIX
    assert "sol pbc" not in chat_copy.CHAT_DISPATCH_ORIGIN_PREFIX
    assert f'CHAT_LIVENESS_THINKING: "{chat_copy.CHAT_LIVENESS_THINKING}"' in text
    assert f'CHAT_LIVENESS_TASK_FORMAT: "{chat_copy.CHAT_LIVENESS_TASK_FORMAT}"' in text
    assert (
        "CHAT_ERROR_DETAIL_EXPANDER_LABEL: "
        f'"{chat_copy.CHAT_ERROR_DETAIL_EXPANDER_LABEL}"'
    ) in text
    assert (
        "CHAT_ERROR_DETAIL_COLLAPSER_LABEL: "
        f'"{chat_copy.CHAT_ERROR_DETAIL_COLLAPSER_LABEL}"'
    ) in text
    expected_js_thinking = """CHAT_THINKING_EXPANDER_LABEL: "show thinking",
CHAT_THINKING_COLLAPSER_LABEL: "hide thinking",
CHAT_THINKING_SETTING_LABEL: "thinking surfaces",
CHAT_THINKING_OPT_ON_TAP: "show on tap",
CHAT_THINKING_OPT_ALWAYS: "always show",
CHAT_THINKING_OPT_NEVER: "never show",
CHAT_THINKING_SETTING_HELP: "sol does some thinking before replying. choose how much you want to see.",
"""
    for expected_line in expected_js_thinking.splitlines():
        assert expected_line in text


def test_closer_constants_byte_parity():
    js_path = Path("solstone/convey/static/chat_copy.js")
    text = js_path.read_text(encoding="utf-8")
    expected = {
        "CHAT_CLOSER_LOOP_EXHAUSTED_PREFIX": "Here's what I have so far:",
        "CHAT_CLOSER_DIFFERENT_ANGLE_SUFFIX": "Want me to try a different angle?",
        "CHAT_CLOSER_TALENT_ERRORED_FORMAT": "I couldn't finish that lookup — {reason}. Want to try a different angle, or rephrase the question?",
        "CHAT_CLOSER_TALENT_ERRORED_GENERIC": "I couldn't finish that lookup. Want to try a different angle, or rephrase the question?",
        "CHAT_CLOSER_SUPPORT_SEND_FAILED": "I couldn't finish reaching solstone support, so nothing was sent. Want me to try again?",
    }

    for name, literal in expected.items():
        assert getattr(chat_copy, name) == literal
        assert literal in text

    assert "\u2014" in chat_copy.CHAT_CLOSER_TALENT_ERRORED_FORMAT
    assert "solstone support" in chat_copy.CHAT_CLOSER_SUPPORT_SEND_FAILED
    assert "sol pbc" not in chat_copy.CHAT_CLOSER_SUPPORT_SEND_FAILED
    assert "live chat" not in chat_copy.CHAT_CLOSER_SUPPORT_SEND_FAILED
    assert "lookup" not in chat_copy.CHAT_CLOSER_SUPPORT_SEND_FAILED
    assert "try again" in chat_copy.CHAT_CLOSER_SUPPORT_SEND_FAILED.lower()


def test_support_draft_ready_copy_bytes():
    assert chat_copy.CHAT_SUPPORT_DRAFT_READY == (
        "Here's the support request I put together — look it over before anything "
        "goes to solstone support."
    )
    assert "solstone support" in chat_copy.CHAT_SUPPORT_DRAFT_READY
    assert "sol pbc" not in chat_copy.CHAT_SUPPORT_DRAFT_READY


def test_support_attach_success_copy_bytes():
    assert chat_copy.CHAT_SUPPORT_ATTACH_FILED_FORMAT == (
        "I added that to solstone support ticket #{ticket_id}."
    )
    assert "solstone support" in chat_copy.CHAT_SUPPORT_ATTACH_FILED_FORMAT
    assert "sol pbc" not in chat_copy.CHAT_SUPPORT_ATTACH_FILED_FORMAT
    assert "sent" not in chat_copy.CHAT_SUPPORT_ATTACH_FILED_FORMAT.lower()
    assert not hasattr(chat_copy, "CHAT_SUPPORT_ATTACH_UNSUPPORTED")


def test_draft_card_copy_present():
    text = Path("solstone/convey/static/chat_copy.js").read_text(encoding="utf-8")
    expected = (
        'CHAT_DRAFT_SUBMIT: "send to solstone support"',
        'CHAT_DRAFT_CANCEL: "cancel"',
        'CHAT_DRAFT_HEADER: "review before this goes to solstone support"',
        'CHAT_DRAFT_KIND_CREATE: "new support request"',
        'CHAT_DRAFT_KIND_FEEDBACK: "send feedback"',
        'CHAT_DRAFT_KIND_REPLY: "reply"',
        'CHAT_DRAFT_KIND_ATTACH: "attach a file"',
        'CHAT_DRAFT_KIND_CLOSE: "close this ticket"',
        'CHAT_DRAFT_KIND_RESOLVED: "confirm this is resolved"',
        'CHAT_DRAFT_KIND_STILL_NEED_HELP: "still need help"',
        'CHAT_DRAFT_TICKET_FORMAT: "ticket #{ticket_id}"',
        'CHAT_DRAFT_DIAGNOSTICS_TITLE: "what\'s included with this request"',
        (
            'CHAT_DRAFT_DIAGNOSTICS_NOTE: "these exact values go to solstone '
            'support with your request. nothing else leaves this machine."'
        ),
        (
            'CHAT_DRAFT_ATTACH_NOTE: "the contents of this file go to solstone '
            'support. nothing else leaves this machine."'
        ),
        'CHAT_DRAFT_CLOSE_NOTE: "confirming closes this ticket.',
        'CHAT_DRAFT_RESOLVED_NOTE: "confirming accepts the proposed resolution',
        'CHAT_DRAFT_STILL_NEED_HELP_NOTE: "confirming tells solstone support',
        'CHAT_DRAFT_FLOOR: "nothing is sent until you choose"',
        'CHAT_DRAFT_NAME_ATTACHED_YES: "name attached: yes"',
        'CHAT_DRAFT_NAME_ATTACHED_NO: "name attached: no"',
        'CHAT_RESULT_VIEW_IN_SUPPORT: "view in support →"',
    )
    for needle in expected:
        assert needle in text
    assert "CHAT_DRAFT_DIAGNOSTICS_LABEL" not in text
    assert "sol pbc" not in "send to solstone support"


def test_support_attach_draft_card_branch_present():
    text = Path("solstone/convey/static/chat_chrome.js").read_text(encoding="utf-8")
    assert "function formatAttachmentSize(size)" in text
    assert "function renderAttachDraftBody(parent, payload)" in text
    assert "window.solChatCopy.CHAT_DRAFT_KIND_ATTACH" in text
    assert "formatAttachmentSize(payload.byte_size)" in text
    assert "appendDraftKind(parent, window.solChatCopy.CHAT_DRAFT_KIND_ATTACH" in text
    assert "appendDraftFieldIfPresent(parent, payload, 'filename')" in text
    assert "appendDraftMetaRow(parent, payload, ['content_type'])" in text
    assert "window.solChatCopy.CHAT_DRAFT_ATTACH_NOTE" in text
    assert "['ticket_id', 'ticket', payload.ticket_id]" not in text
    assert "['filename', 'file', payload.filename]" not in text
    assert "['byte_size', 'size', formatAttachmentSize(payload.byte_size)]" not in text


def test_support_lifecycle_draft_card_branches_present():
    text = Path("solstone/convey/static/chat_chrome.js").read_text(encoding="utf-8")

    for verb, kind, note in (
        ("close", "CHAT_DRAFT_KIND_CLOSE", "CHAT_DRAFT_CLOSE_NOTE"),
        ("resolved", "CHAT_DRAFT_KIND_RESOLVED", "CHAT_DRAFT_RESOLVED_NOTE"),
        (
            "still_need_help",
            "CHAT_DRAFT_KIND_STILL_NEED_HELP",
            "CHAT_DRAFT_STILL_NEED_HELP_NOTE",
        ),
    ):
        assert f"draft.verb === '{verb}'" in text
        assert f"window.solChatCopy.{kind}" in text
        assert f"window.solChatCopy.{note}" in text

    assert '"support draft in_progress"' in text
    assert '"support draft re_consent_required"' in text
    assert "function reenableSupportDraft()" in text
    assert "postChatMessage(window.solChatCopy.CHAT_RESULT_TRY_AGAIN_MESSAGE)" not in text


def test_chat_placeholder_css_present():
    css = Path("solstone/convey/static/app.css").read_text(encoding="utf-8")

    assert ".chat-bubble--placeholder" in css
    assert "opacity: 0.65" in css
    assert "font-style: italic" in css
