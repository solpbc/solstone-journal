#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Conformance oracle for the journal search index's query compiler.

The query path turns raw text from an owner or an agent into an FTS5 `MATCH`
expression plus a date range. That translation is a contract: nine production
readers depend on it, and a term the compiler mangles is a term the index cannot
find even though the content is stored correctly.

This module runs the reference implementation over a synthetic corpus and
records what it produced. It pins three things at once:

  * **temporal extraction** — which phrase was lifted out of the text, what
    remained, and the `(day_from, day_to)` pair it resolved to;
  * **term compilation** — the `MATCH` expression, including the reference's
    character handling; and
  * **the shapes the reference gets wrong** — accented and non-Latin input,
    which the reference deletes before FTS5 ever sees it. Those cases are
    recorded deliberately, so that a replacement changing them has to say so
    rather than merely differ.

⚠ Regenerating this file requires a runnable reference tree. It is a frozen
record, not a live comparison: once the reference can no longer be executed the
recorded values are the only remaining statement of what the query path meant.

⛔ Every case is synthetic. No text here comes from any journal. The classes are
chosen to cover the query shapes the surveyed traffic exhibits — short proper
nouns, small OR-sets, quoted phrases, punctuation-bearing atoms and temporal
phrases — without carrying a single recorded query.

Determinism: temporal resolution reads a reference date. The generator pins both
`TZ=UTC` and an explicit reference instant, and records them, so a regeneration
on another machine reproduces the same bytes.
"""

from __future__ import annotations

import json
import os
import time
from typing import Any

# Pinned before any solstone import so module-level datetime work sees it too.
os.environ["TZ"] = "UTC"
if hasattr(time, "tzset"):
    time.tzset()

from datetime import datetime  # noqa: E402

from solstone.think.indexer.journal import (  # noqa: E402
    extract_temporal_references,
    sanitize_fts_query,
)

# A Thursday, chosen so that "last monday", "this week", "last week" and
# "over the weekend" each resolve to a distinct, non-degenerate range.
REFERENCE_INSTANT = "2026-08-06T12:00:00"

# (case id, input). The id names the shape being pinned, not the text.
CASES: list[tuple[str, str]] = [
    # --- empty and whitespace ---
    ("empty", ""),
    ("whitespace_only", "   "),
    ("interior_whitespace_run", "alpha     beta"),
    ("leading_trailing_whitespace", "  alpha beta  "),
    # --- plain terms ---
    ("single_term", "standup"),
    ("two_terms", "weekly meeting"),
    ("four_terms", "quarterly planning review notes"),
    ("mixed_case_terms", "Weekly Meeting"),
    # --- explicit operators ---
    ("operator_or", "standup OR retro"),
    ("operator_and", "standup AND retro"),
    ("operator_not", "standup NOT retro"),
    ("operator_or_three", "alpha OR beta OR gamma"),
    ("lowercase_and_is_a_word", "salt and pepper"),
    ("lowercase_or_is_a_word", "this or that"),
    # --- quoting ---
    ("balanced_quoted_phrase", '"release train"'),
    ("quoted_phrase_plus_term", '"release train" schedule'),
    ("unterminated_quote", '"release train'),
    ("quote_inside_terms", 'say "hi" now'),
    ("two_quoted_phrases", '"alpha beta" "gamma delta"'),
    # --- apostrophes ---
    ("apostrophe_proper_noun", "O'Brien"),
    ("apostrophe_contraction", "it's"),
    ("apostrophe_with_wildcard", "O'Bri*"),
    # --- wildcards ---
    ("trailing_wildcard", "plan*"),
    ("short_trailing_wildcard", "a*"),
    ("wildcard_inside_quotes", '"plan*"'),
    ("wildcard_midword", "pl*an"),
    # --- non-ASCII: the shapes the reference destroys ---
    ("accented_latin_jose", "José"),
    ("accented_latin_cafe", "café"),
    ("accented_latin_phrase", "José café meeting"),
    ("umlaut", "Müller"),
    ("cedilla", "façade"),
    ("greek", "Αθήνα"),
    ("cyrillic", "Москва"),
    ("cjk_han", "会議"),
    ("cjk_kana", "ミーティング"),
    ("hangul", "회의"),
    ("arabic", "اجتماع"),
    ("hebrew", "פגישה"),
    ("devanagari", "बैठक"),
    ("emoji", "meeting 📅"),
    ("mixed_script", "meeting 会議 réunion"),
    # --- Unicode normalization: the same string, composed and decomposed.
    # The two literals below render identically and are different byte
    # sequences on purpose: U+00E9 versus "e" + U+0301. ⛔ Do not "deduplicate"
    # them -- a compiler that folds them together is what the pair proves. ---
    ("nfc_composed", "café"),
    ("nfd_decomposed", "café"),
    # --- punctuation-bearing atoms ---
    ("email_address", "someone@example.com"),
    ("domain", "example.com"),
    ("posix_path", "notes/2026/plan.md"),
    ("windows_path", "notes\\2026\\plan.md"),
    ("hyphenated_compound", "follow-up"),
    ("underscored_identifier", "weekly_reflection"),
    ("dotted_version", "version 1.0.22"),
    ("url", "https://example.com/notes"),
    ("parenthesised", "planning (draft)"),
    ("colon_separated", "note:draft"),
    # --- control characters ---
    ("tab_separated", "alpha\tbeta"),
    ("newline_separated", "alpha\nbeta"),
    ("null_byte", "alpha\x00beta"),
    ("bell_character", "alpha\x07beta"),
    # --- temporal phrases, bare ---
    ("temporal_yesterday", "yesterday"),
    ("temporal_today", "today"),
    ("temporal_last_week", "last week"),
    ("temporal_this_week", "this week"),
    ("temporal_last_month", "last month"),
    ("temporal_this_month", "this month"),
    ("temporal_weekend_over", "over the weekend"),
    ("temporal_weekend_on", "on the weekend"),
    ("temporal_last_monday", "last monday"),
    ("temporal_last_sunday", "last sunday"),
    ("temporal_last_friday_mixed_case", "last Friday"),
    # --- temporal phrases in context ---
    ("temporal_with_terms", "standup notes from yesterday"),
    ("temporal_leading", "yesterday standup notes"),
    ("temporal_quoted_is_literal", '"yesterday" standup'),
    ("temporal_two_phrases", "yesterday and last week"),
    ("temporal_inside_longer_phrase", "notes from last monday about planning"),
    ("temporal_with_quoted_phrase", '"release train" last week'),
    # --- shapes that stress the operator/quote interaction ---
    ("operator_after_quote", '"release train" OR standup'),
    ("or_binds_looser_than_implicit_and", "alpha OR beta gamma"),
    ("stopword_heavy", "what did i do about the meeting"),
    ("single_stopword", "the"),
]


def _run(case_id: str, text: str, reference: datetime) -> dict[str, Any]:
    expression, sanitize_day_from, sanitize_day_to = sanitize_fts_query(text, reference)
    remaining, temporal_day_from, temporal_day_to = extract_temporal_references(
        text, reference
    )
    return {
        "case": case_id,
        "input": text,
        "temporal": {
            "remaining_text": remaining,
            "day_from": temporal_day_from,
            "day_to": temporal_day_to,
        },
        "compiled": {
            "expression": expression,
            "day_from": sanitize_day_from,
            "day_to": sanitize_day_to,
        },
    }


def build() -> dict[str, Any]:
    reference = datetime.fromisoformat(REFERENCE_INSTANT)
    seen: set[str] = set()
    cases = []
    for case_id, text in CASES:
        if case_id in seen:
            raise SystemExit(f"duplicate case id: {case_id}")
        seen.add(case_id)
        cases.append(_run(case_id, text, reference))
    return {
        "tz": "UTC",
        "reference_instant": REFERENCE_INSTANT,
        "reference_weekday": reference.strftime("%A"),
        "cases": cases,
    }


def main() -> None:
    corpus = build()
    if not corpus["cases"]:
        raise SystemExit("refusing to write an empty corpus")
    print(json.dumps(corpus, ensure_ascii=False, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
