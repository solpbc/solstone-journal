# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Context-window fitting for bundled local llama-server requests."""

from __future__ import annotations

import math
import re
from collections.abc import Callable
from typing import Any

from solstone.think.providers.local import ContextBudgetExceeded

_SAFETY_MARGIN_TOKENS = 256
# Conservative per-image upper estimate; images inform endpoint completion
# clamps but are never dropped or truncated by the fitting helper.
_ESTIMATED_IMAGE_TOKENS = 2500
_OUTPUT_RESERVE_DIVISOR = 4
_CHARS_PER_TOKEN = 3
_ENTRY_HEADER_RE = re.compile(r"^#{2,3}\s")
TRUNCATION_MARKER = "[earlier input truncated to fit the on-device model's context]"


def estimate_tokens(text: str) -> int:
    return math.ceil(len(text) / _CHARS_PER_TOKEN)


def compute_input_budget(max_output_tokens: int, window: int) -> int:
    reserve = min(max_output_tokens, window // _OUTPUT_RESERVE_DIVISOR)
    return window - reserve - _SAFETY_MARGIN_TOKENS


def split_entries(block: str) -> list[str]:
    entries: list[str] = []
    current: list[str] = []

    for line in block.splitlines(keepends=True):
        if _ENTRY_HEADER_RE.match(line) and current:
            entries.append("".join(current))
            current = []
        current.append(line)

    if current:
        entries.append("".join(current))
    return entries


def _dedup_adjacent(entries: list[str]) -> list[str]:
    deduped: list[str] = []
    for entry in entries:
        if deduped and deduped[-1] == entry:
            continue
        deduped.append(entry)
    return deduped


def fit_contents(
    contents: Any,
    system_instruction: str | None,
    max_output_tokens: int,
    *,
    count: Callable[[str], int],
    window: int,
) -> tuple[Any, dict | None]:
    budget = compute_input_budget(max_output_tokens, window)

    if isinstance(contents, str):
        block = contents
        preserved = [system_instruction or ""]

        def rebuild(fitted: str) -> str:
            return fitted

    elif isinstance(contents, list) and contents and isinstance(contents[0], str):
        block = contents[0]
        preserved = [system_instruction or "", *[str(c) for c in contents[1:]]]

        def rebuild(fitted: str) -> list[Any]:
            return [fitted, *contents[1:]]

    else:
        return contents, None

    preserved_tokens = sum(count(text) for text in preserved if text)
    if preserved_tokens >= budget:
        raise ContextBudgetExceeded(
            "Local request system instruction and preserved prompt content exceed "
            "the model context window."
        )

    entries = _dedup_adjacent(split_entries(block))
    fitted_block = "".join(entries)
    if preserved_tokens + count(fitted_block) <= budget:
        return rebuild(fitted_block), None

    marker_text = TRUNCATION_MARKER + "\n\n"
    available = budget - preserved_tokens - count(marker_text)
    running = 0
    kept_reversed: list[str] = []
    for entry in reversed(entries):
        entry_tokens = count(entry)
        if running + entry_tokens > available:
            break
        kept_reversed.append(entry)
        running += entry_tokens

    kept = list(reversed(kept_reversed))
    dropped = entries[: len(entries) - len(kept)]
    dropped_chars = sum(len(entry) for entry in dropped)
    new_block = marker_text + "".join(kept)
    input_budget = {
        "clipped": True,
        "dropped_chars": dropped_chars,
        "dropped_entries": len(dropped),
        "budget_tokens": budget,
    }
    return rebuild(new_block), input_budget


__all__ = [
    "TRUNCATION_MARKER",
    "compute_input_budget",
    "estimate_tokens",
    "fit_contents",
    "split_entries",
]
