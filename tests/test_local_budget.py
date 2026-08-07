# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import math

import pytest

from solstone.think.providers import local_budget
from solstone.think.providers.local import ContextBudgetExceeded


def _count_chars(text: str) -> int:
    return len(text)


def test_compute_input_budget_reserves_output_and_clamps():
    assert local_budget.compute_input_budget(123, window=2000) == 2000 - 123 - 256
    assert local_budget.compute_input_budget(900, window=2000) == 2000 - 500 - 256


def test_estimate_tokens_overcounts_four_char_baseline():
    text = "x" * 12

    assert local_budget.estimate_tokens(text) == 4
    assert local_budget.estimate_tokens(text) > math.ceil(len(text) / 4)


def test_split_entries_round_trips_realistic_markdown():
    block = (
        "preamble before the first header\n"
        "## 2026-06-23 09:00:00 - 09:05:00\n"
        "### Transcript\n"
        "hello\n"
        "### Screen Activity\n"
        "browser changed\n"
        "## 2026-06-23 09:05:00 - 09:10:00\n"
        "### screen summary\n"
        "later\n"
    )

    entries = local_budget.split_entries(block)

    assert "".join(entries) == block
    assert entries[0] == "preamble before the first header\n"
    assert entries[1].startswith("## 2026-06-23")
    assert entries[2].startswith("### Transcript")
    assert entries[3].startswith("### Screen Activity")


def test_dedup_runs_before_clip_and_prevents_marker():
    entry = "### Screen Activity\n" + ("same screen\n" * 30)
    block = entry * 3

    fitted, input_budget = local_budget.fit_contents(
        block,
        None,
        50,
        count=_count_chars,
        window=1000,
    )

    assert fitted == entry
    assert input_budget is None
    assert local_budget.TRUNCATION_MARKER not in fitted


def test_fit_contents_clips_oldest_entries_to_tail():
    chunks = [
        "## 2026-06-23 09:00:00 - 09:05:00\n",
        "### Transcript\noldest " + ("o" * 4300) + "\n",
        "### Screen Activity\nmiddle " + ("m" * 4300) + "\n",
        "## 2026-06-23 09:05:00 - 09:10:00\n",
        "### Transcript\nrecent " + ("r" * 4300) + "\n",
        "### Screen Activity\nlatest " + ("l" * 4300) + "\n",
    ]
    block = "".join(chunks)
    talent_prompt = "talent prompt"
    system_instruction = "system"

    fitted_contents, input_budget = local_budget.fit_contents(
        [block, talent_prompt],
        system_instruction,
        8192 * 6,
        count=_count_chars,
        window=16384,
    )

    assert isinstance(fitted_contents, list)
    fitted_block = fitted_contents[0]
    assert fitted_contents[1] is talent_prompt
    assert fitted_block.startswith(local_budget.TRUNCATION_MARKER + "\n\n")
    assert "oldest " not in fitted_block
    assert "middle " not in fitted_block
    assert "recent " in fitted_block
    assert "latest " in fitted_block
    assert input_budget == {
        "clipped": True,
        "dropped_chars": sum(len(chunk) for chunk in chunks[:3]),
        "dropped_entries": 3,
        "budget_tokens": 12032,
    }
    assert (
        _count_chars(system_instruction)
        + _count_chars(talent_prompt)
        + _count_chars(fitted_block)
        <= input_budget["budget_tokens"]
    )


def test_fit_contents_leaves_non_overflow_unmarked():
    contents = ["## Segment\n### Transcript\nshort\n", "prompt"]

    fitted, input_budget = local_budget.fit_contents(
        contents,
        "system",
        50,
        count=_count_chars,
        window=1000,
    )

    assert fitted == contents
    assert input_budget is None
    assert local_budget.TRUNCATION_MARKER not in fitted[0]


def test_fit_contents_raises_when_preserved_content_exceeds_budget():

    with pytest.raises(ContextBudgetExceeded) as exc:
        local_budget.fit_contents(
            "## Segment\n### Transcript\nshort\n",
            "s" * 200,
            10,
            count=_count_chars,
            window=400,
        )

    assert exc.value.reason_code == "context_budget_exceeded"


def test_fit_contents_noops_role_dicts_and_empty_lists():
    role_messages = [{"role": "user", "content": "hello"}]
    empty: list[str] = []

    fitted_messages, message_budget = local_budget.fit_contents(
        role_messages,
        None,
        10,
        count=_count_chars,
        window=16384,
    )
    fitted_empty, empty_budget = local_budget.fit_contents(
        empty,
        None,
        10,
        count=_count_chars,
        window=16384,
    )

    assert fitted_messages is role_messages
    assert message_budget is None
    assert fitted_empty is empty
    assert empty_budget is None
