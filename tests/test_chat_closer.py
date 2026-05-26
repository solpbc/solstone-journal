# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from solstone.convey import chat

FIXTURE_OPENERS = (
    "Let me look up",
    "Let me check",
    "Let me see",
    "I'll check",
    "I'll look up",
    "I'll take a look",
)
FIXTURE_TRAILING = ("one moment while I check",)


def test_closer_strip_patterns_locked_bytes():
    assert chat.CLOSER_STRIP_PATTERNS["openers"] == FIXTURE_OPENERS
    assert chat.CLOSER_STRIP_PATTERNS["trailing"] == FIXTURE_TRAILING


def test_strip_closer_patterns_casefolds_openers_and_preserves_out_of_set():
    assert (
        chat._strip_closer_patterns("let me look up emails. Found one.") == "Found one."
    )
    assert chat._strip_closer_patterns("Let me check. Found one.") == "Found one."
    assert chat._strip_closer_patterns("I'LL TAKE A LOOK. Found one.") == "Found one."
    assert (
        chat._strip_closer_patterns("Looking into this. Found one.")
        == "Looking into this. Found one."
    )


def test_strip_closer_patterns_removes_trailing_span_only():
    assert (
        chat._strip_closer_patterns("Here is one moment while I check the answer.")
        == "Here is the answer."
    )


def test_loop_exhausted_substantive_text_surfaces_verbatim():
    message = (
        "Adrian sent three updates about the launch plan, the budget review, and "
        "the Friday timeline, with the timeline note asking for confirmation today please."
    )

    assert chat._compose_terminal_closer("loop_exhausted", message) == message


def test_loop_exhausted_fragmentary_text_frames_with_suffix():
    assert (
        chat._compose_terminal_closer("loop_exhausted", "Found three relevant notes.")
        == "Here's what I have so far: Found three relevant notes. "
        "Want me to try a different angle?"
    )


def test_loop_exhausted_token_threshold_boundary():
    fourteen_tokens = (
        "one two three four five six seven eight nine ten eleven twelve thirteen "
        "fourteen"
    )
    fifteen_tokens = f"{fourteen_tokens} fifteen"

    assert (
        chat._compose_terminal_closer("loop_exhausted", fourteen_tokens)
        == f"Here's what I have so far: {fourteen_tokens} "
        "Want me to try a different angle?"
    )
    assert (
        chat._compose_terminal_closer("loop_exhausted", fifteen_tokens)
        == fifteen_tokens
    )


def test_talent_errored_reason_framing():
    assert (
        chat._compose_terminal_closer(
            "talent_errored",
            "I'll check.",
            talent_errored_reason="talent timed out waiting for provider response",
        )
        == "I couldn't finish that lookup — talent timed out waiting for provider response. "
        "Want to try a different angle, or rephrase the question?"
    )
    assert (
        chat._compose_terminal_closer(
            "talent_errored",
            "",
            talent_errored_reason="Traceback (most recent call last)",
        )
        == "I couldn't finish that lookup. Want to try a different angle, or rephrase the question?"
    )
    assert (
        chat._compose_terminal_closer("talent_errored", "", talent_errored_reason="")
        == "I couldn't finish that lookup. Want to try a different angle, or rephrase the question?"
    )
    assert (
        chat._compose_terminal_closer(
            "talent_errored",
            "",
            talent_errored_reason="/tmp/provider.py failed",
        )
        == "I couldn't finish that lookup. Want to try a different angle, or rephrase the question?"
    )


def test_loop_exhausted_empty_empty_fallback():
    assert (
        chat._compose_terminal_closer(
            "loop_exhausted",
            "",
            talent_finished_summary="",
        )
        == "Here's what I have so far: Want me to try a different angle?"
    )


def test_keep_form_survives_strip_helper_verbatim():
    keep_form = "Useful result — let me know if you want me to dig deeper"

    assert chat._strip_closer_patterns(keep_form) == keep_form


def test_multi_sentence_post_strip_frames_remaining_body():
    assert (
        chat._compose_terminal_closer(
            "loop_exhausted",
            "Let me look up emails. There are 3 from Adrian.",
        )
        == "Here's what I have so far: There are 3 from Adrian. "
        "Want me to try a different angle?"
    )
