#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Conformance oracle for the journal content-formatting families.

The native content formatters in `core/crates/solstone-core-indexer/src/content/`
are checked against hand-written expectations. Only the markdown chunker has an
oracle generated from the reference implementation, so for every other family the
Rust assertions are a restatement of what the Rust does, not evidence that it
matches what the journal's content actually meant.

This module renders each family through the reference implementation and records
the result. It pins two things at once, deliberately:

  * **dispatch** — the formatter is resolved through the registry by the case's
    journal-relative path, exactly as a reader resolves it, so a case also proves
    which family a path belongs to; and
  * **render** — the emitted markdown, the indexer agent, and the header/error
    the reference produced.

⚠ Regenerating this file requires a runnable reference tree. It is a frozen
record, not a live comparison: once the reference can no longer be executed the
recorded values are the only remaining statement of intent, which is why the
corpus is captured before the surrounding rework rather than after it.

Determinism: several formatters build epoch-millisecond values through
``datetime.timestamp()`` on naive datetimes, which reads the process timezone.
The generator pins ``TZ=UTC`` and records it, so a regeneration on another
machine reproduces the same bytes. `timestamp_utc_ms` is recorded per chunk for
the families that emit one; where a family emits none the key is absent.
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

from solstone.think.formatters import get_formatter  # noqa: E402

FIXTURE_VERSION = 1
GENERATOR_TZ = "UTC"


def _on_disk_text(rel: str, entries: list[dict[str, Any]]) -> str:
    """Render the case's records as the bytes a reader would actually load.

    Recorded alongside the parsed input so a consumer of this corpus compares
    against file content rather than reconstructing a serialization and
    inheriting a mismatch that has nothing to do with formatting.
    """
    if rel.endswith(".jsonl"):
        if not entries:
            return ""
        return "".join(f"{json.dumps(entry)}\n" for entry in entries)
    if rel.endswith(".json"):
        return json.dumps(entries[0]) if entries else ""
    raise AssertionError(f"no on-disk shape for {rel!r}")


def _case(
    case_id: str,
    rel: str,
    family: str,
    entries: list[dict[str, Any]] | str,
    *,
    note: str | None = None,
    context_extra: dict[str, Any] | None = None,
    expect_raises: str | None = None,
) -> dict[str, Any]:
    """Render one case through the registry and record what came back."""
    formatter = get_formatter(rel)
    if formatter is None:
        raise AssertionError(f"{case_id}: no formatter dispatches for {rel!r}")

    context: dict[str, Any] = {"file_path": rel}
    if context_extra:
        context.update(context_extra)

    if expect_raises is not None:
        try:
            formatter(entries, context)
        except Exception as exc:  # noqa: BLE001 — the raise is the recorded behaviour
            if type(exc).__name__ != expect_raises:
                raise AssertionError(
                    f"{case_id}: expected {expect_raises}, got {type(exc).__name__}"
                ) from exc
            out: dict[str, Any] = {
                "id": case_id,
                "rel": rel,
                "family": family,
                "formatter": formatter.__name__,
                "input": entries,
                "input_text": _on_disk_text(rel, entries),
                "raises": expect_raises,
                "raise_message": str(exc),
            }
            if context_extra:
                out["context"] = context_extra
            if note:
                out["note"] = note
            return out
        raise AssertionError(f"{case_id}: expected {expect_raises}, nothing raised")

    chunks, meta = formatter(entries, context)

    recorded: list[dict[str, Any]] = []
    for chunk in chunks:
        row: dict[str, Any] = {"markdown": chunk.get("markdown", "")}
        if "timestamp" in chunk and chunk["timestamp"] is not None:
            row["timestamp_utc_ms"] = chunk["timestamp"]
        # The originating record. Only some formatters attach one, and it is the
        # field the owner-facing surface depends on — speaker attribution, screen
        # frame geometry, browser snapshot-vs-delta. Recorded so a consumer of
        # this corpus can check it rather than taking it on trust.
        if "source" in chunk:
            row["source"] = chunk["source"]
        recorded.append(row)

    indexer = meta.get("indexer", {}) or {}
    out: dict[str, Any] = {
        "id": case_id,
        "rel": rel,
        "family": family,
        "formatter": formatter.__name__,
        "input": entries,
        "input_text": _on_disk_text(rel, entries),
        "chunks": recorded,
        "agent": indexer.get("agent"),
    }
    if context_extra:
        out["context"] = context_extra
    if indexer.get("day") is not None:
        out["indexer_day"] = indexer["day"]
    if indexer.get("facet") is not None:
        out["indexer_facet"] = indexer["facet"]
    if meta.get("header"):
        out["header"] = meta["header"]
    if meta.get("error"):
        out["error"] = meta["error"]
    if note:
        out["note"] = note
    return out


# --------------------------------------------------------------------------
# Cases. Each family carries a nominal row, the skip/degenerate rows that decide
# whether a record is emitted at all, and the shapes the native renderer already
# claims to handle.
# --------------------------------------------------------------------------

FACET_EVENTS = "facets/work/events/20240101.jsonl"
FACET_ACTIVITIES = "facets/work/activities/20240101.jsonl"
FACET_LOGS = "facets/work/logs/20240101.jsonl"
CONFIG_ACTIONS = "config/actions/20240101.jsonl"
FACET_ENTITIES = "facets/work/entities/20260304.jsonl"
FACET_ENTITIES_SLUG = "facets/work/entities/some-slug.jsonl"
OBSERVATIONS = "facets/work/entities/alice/observations.jsonl"
STRUCTURED_IMPORT = "20260101/import.ics/imported.jsonl"
AI_CHAT = "20260101/import.claude/thread_a/conversation_transcript.jsonl"
AI_CHAT_LEGACY = "20260101/import.chatgpt/conv_b/imported_audio.jsonl"
AI_CHAT_UNPINNED = "20260101/misc/thread_a/conversation_transcript.jsonl"
CHAT = "20260508/chat/120000_300/chat.jsonl"
BROWSER = "20260703/suze.browser/000141_317/browser_mail-google-com.jsonl"
DAY_ACCUMULATOR = "20260304/talents/pulse.jsonl"
SENSE = "20260304/default/090000_300/talents/sense.json"
DOCUMENTS = "20260304/default/090000_300/talents/documents.json"
SCREEN_RECORD = "20260304/default/090000_300/talents/screen.json"
MORNING_BRIEFING = "20260304/talents/morning_briefing.json"


def _event_cases() -> list[dict[str, Any]]:
    return [
        _case(
            "event_nominal",
            FACET_EVENTS,
            "Event",
            [
                {
                    "title": "Standup",
                    "type": "meeting",
                    "participants": ["Alice", "Bob"],
                    "summary": "Daily sync",
                }
            ],
        ),
        _case(
            "event_skip_is_title_only",
            FACET_EVENTS,
            "Event",
            [
                {"type": "meeting"},
                {"title": "", "type": "meeting"},
                {"title": "Kept", "type": "task"},
            ],
            note="a row without a truthy title emits nothing; type is not part of the skip test",
        ),
        _case("event_empty", FACET_EVENTS, "Event", []),
    ]


def _activity_cases() -> list[dict[str, Any]]:
    return [
        _case(
            "activity_nominal",
            FACET_ACTIVITIES,
            "Activity",
            [
                {
                    "title": "Launch sync",
                    "activity": "meeting",
                    "facet": "work",
                    "day": "20260418",
                    "segments": ["090000_300"],
                    "level_avg": 0.5,
                    "description": "Team sync",
                    "details": "Assigned owners",
                    "participation": [{"name": "Mina"}],
                    "story": {
                        "body": "Aligned on launch.",
                        "topics": ["launch", "owners"],
                    },
                    "hidden": True,
                }
            ],
        ),
        _case(
            "activity_degenerate_rows_still_emit",
            FACET_ACTIVITIES,
            "Activity",
            [{}, {"id": "x"}],
            note="unlike events, an activity object always produces a record",
        ),
    ]


def _action_log_cases() -> list[dict[str, Any]]:
    return [
        _case(
            "action_log_nominal",
            CONFIG_ACTIONS,
            "ActionLog",
            [
                {
                    "action": "identity_update",
                    "actor": "settings",
                    "source": "app",
                    "timestamp": "2025-12-16T07:33:05.135587+00:00",
                    "use_id": "123",
                    "params": {"name": "Alice"},
                }
            ],
        ),
        _case(
            "action_log_skip_is_action_only",
            CONFIG_ACTIONS,
            "ActionLog",
            [{"actor": "settings"}, {"action": "", "actor": "settings"}],
        ),
        _case(
            "action_log_facet_scoped_path",
            FACET_LOGS,
            "ActionLog",
            [{"action": "facet_note", "actor": "sol"}],
            note="facets/*/logs/*.jsonl shares the formatter with config/actions",
        ),
    ]


def _facet_entity_cases() -> list[dict[str, Any]]:
    return [
        _case(
            "facet_entity_nominal",
            FACET_ENTITIES,
            "FacetEntity",
            [
                {
                    "id": "alice",
                    "type": "Person",
                    "name": "Alice",
                    "description": "Friend from work",
                    "tags": ["tech", "mentor"],
                    "aka": ["A", "Al"],
                    "contact": "alice@example.com",
                    "roles": ["lead", "reviewer"],
                    "empty_note": "",
                    "last_seen": "20260304",
                    "detached": True,
                }
            ],
        ),
        _case(
            "facet_entity_missing_fields",
            FACET_ENTITIES,
            "FacetEntity",
            [
                {"type": "Project", "name": "No Description", "description": ""},
                {"description": "Only description"},
            ],
        ),
        _case(
            "facet_entity_agent_from_slug_stem",
            FACET_ENTITIES_SLUG,
            "FacetEntity",
            [{"name": "Slugged", "type": "Person"}],
            note="a non-digit stem selects the attached agent rather than detected",
        ),
    ]


def _observation_cases() -> list[dict[str, Any]]:
    return [
        _case(
            "observation_nominal",
            OBSERVATIONS,
            "Observation",
            [
                {"content": "Prefers morning meetings", "source_day": "20250113"},
                {"content": "Expert in distributed systems"},
                {"source_day": "20250114"},
                {"content": "", "source_day": ""},
            ],
            note="every row emits, including the empty one",
        ),
    ]


def _structured_import_cases() -> list[dict[str, Any]]:
    return [
        _case(
            "structured_import_nominal",
            STRUCTURED_IMPORT,
            "StructuredImport",
            [
                {"import": {"source": "ics"}},
                {
                    "type": "calendar_event",
                    "title": "Quarterly Planning",
                    "ts": "2026-01-01T09:30:00-07:00",
                },
            ],
        ),
        _case(
            "structured_import_header_and_generic_are_skipped",
            STRUCTURED_IMPORT,
            "StructuredImport",
            [{"import": {"source": "ics"}, "title": "Header"}, {"type": "generic"}],
        ),
        _case(
            "structured_import_source_case_preserved",
            STRUCTURED_IMPORT,
            "StructuredImport",
            [{"import": {"source": "ICS"}}],
            note="the agent carries the source verbatim; lowercasing happens at merge",
        ),
        _case(
            "structured_import_missing_source",
            STRUCTURED_IMPORT,
            "StructuredImport",
            [{"entry_count": 1}],
        ),
        _case("structured_import_empty", STRUCTURED_IMPORT, "StructuredImport", []),
    ]


def _ai_chat_cases() -> list[dict[str, Any]]:
    return [
        _case(
            "ai_chat_nominal",
            AI_CHAT,
            "AiChat",
            [
                {"model": "claude-3", "imported": {"facet": "work"}},
                {"start": "00:00:01", "speaker": "User", "text": "Hello"},
                {"start": "00:00:02", "speaker": "Assistant", "text": ""},
                {"start": "00:00:03", "speaker": "Assistant", "text": "Hi there"},
                {"speaker": "System", "text": "metadata-like"},
            ],
            note="a turn needs both a start and non-empty text",
        ),
        _case(
            "ai_chat_legacy_filename",
            AI_CHAT_LEGACY,
            "AiChat",
            [
                {"model": "gpt-4"},
                {"start": "00:00:01", "speaker": "User", "text": "Legacy path"},
            ],
        ),
        _case(
            "conversation_transcript_off_an_unpinned_stream_is_not_ai_chat",
            AI_CHAT_UNPINNED,
            None,
            [{"model": "claude-3"}],
            note=(
                "🔴 dispatch fact, not an AiChat case. A conversation_transcript.jsonl "
                "outside import.{chatgpt,claude,gemini} resolves to format_audio in the "
                "reference, and to nothing natively — */*/*/*_transcript.jsonl is one of "
                "the six unported families. Consequence: ai_chat.rs's 'ai_chat' agent "
                "fallback is unreachable through classify(), because AiChat is only ever "
                "produced for a path that already contains 'import.'. The native test that "
                "covers that fallback calls produce_chunks directly and bypasses dispatch."
            ),
        ),
    ]


def _chat_cases() -> list[dict[str, Any]]:
    # 🔴 Speaker labels are RESOLVED FROM JOURNAL CONFIG, not constants. The
    # reference reads identity.preferred / identity.name and agent.name, and only
    # falls back to "Owner"/"Sol" when both are unset. The native renderer
    # hardcodes "Owner"/"Sol", so on any journal whose owner has set a name the
    # two disagree on every owner and agent turn. Cases pin the labels through
    # context so the corpus states the contract: labels are an INPUT.
    labelled = {"owner_name": "Jamie", "agent_name": "Sol"}
    return [
        _case(
            "chat_turns_with_configured_labels",
            CHAT,
            "Chat",
            [
                {"kind": "owner_message", "ts": 1772000000000, "text": "What did I do today?"},
                {"kind": "sol_message", "ts": 1772000001000, "text": "You shipped the cable."},
            ],
            context_extra=labelled,
            note="the owner label is journal identity, never the literal 'Owner'",
        ),
        _case(
            "chat_turn_without_text_keeps_the_label",
            CHAT,
            "Chat",
            [{"kind": "owner_message", "ts": 1772000002000, "text": ""}],
            context_extra=labelled,
        ),
        _case(
            "chat_talent_and_error_events",
            CHAT,
            "Chat",
            [
                {"kind": "talent_spawned", "ts": 1, "name": "recall", "task": "find it"},
                {"kind": "talent_finished", "ts": 2, "name": "recall", "summary": "found"},
                {"kind": "talent_errored", "ts": 3, "name": "recall", "reason": "timeout"},
                {"kind": "chat_error", "ts": 4, "reason": "non_responsive"},
            ],
            context_extra=labelled,
        ),
        _case(
            "chat_backend_only_kinds_are_skipped",
            CHAT,
            "Chat",
            [
                {"kind": "chat_queue_depth", "ts": 1, "depth": 2},
                {"kind": "support_draft", "ts": 2},
                {"kind": "result", "ts": 3},
                {"kind": "owner_chat_open", "ts": 4},
            ],
            context_extra=labelled,
        ),
        _case(
            "chat_unknown_kind_raises_in_the_reference",
            CHAT,
            "Chat",
            [{"kind": "not_a_real_kind", "ts": 1}],
            context_extra=labelled,
            expect_raises="ValueError",
            note=(
                "recorded as a deliberate divergence: the reference raises and its caller "
                "swallows it, dropping the whole file; the native renderer skips the row "
                "and keeps the rest. The native behaviour is the one to keep — this case "
                "exists so the difference stays visible rather than being rediscovered."
            ),
        ),
    ]


def _browser_cases() -> list[dict[str, Any]]:
    return [
        _case(
            "browser_snapshot_then_delta",
            BROWSER,
            "Browser",
            [
                {
                    "t": "segment_start",
                    "ts": 1772000000000,
                    "url": "https://mail.google.com/",
                    "site": "mail.google.com",
                    "title": "Inbox",
                    "blocks": [{"text": "alpha block"}],
                },
                {
                    "t": "delta",
                    "ts": 1772000005000,
                    "op": "add",
                    "blocks": [{"text": "beta block"}],
                },
            ],
        ),
        _case(
            "browser_without_a_segment_start",
            BROWSER,
            "Browser",
            [
                {
                    "t": "delta",
                    "ts": 1772000005000,
                    "op": "add",
                    "blocks": [{"text": "orphan"}],
                }
            ],
            note=(
                "the reference reports this through meta error and both web consumers "
                "act on it; the native renderer hardcodes an empty warning list, so the "
                "condition is currently undetectable natively"
            ),
        ),
        _case(
            "browser_empty_stream",
            BROWSER,
            "Browser",
            [],
        ),
        _case(
            "browser_unknown_row_kind_is_skipped",
            BROWSER,
            "Browser",
            [
                {"t": "heartbeat", "ts": 1772000000000},
                {
                    "t": "segment_start",
                    "ts": 1772000001000,
                    "site": "example.com",
                    "title": "Example",
                    "blocks": [{"text": "kept"}],
                },
            ],
        ),
        _case(
            "browser_row_without_ts_raises_in_the_reference",
            BROWSER,
            "Browser",
            [{"t": "segment_start", "site": "example.com", "title": "No ts", "blocks": [{"text": "x"}]}],
            expect_raises="KeyError",
            note=(
                "an unguarded subscript on a row that rendered fine otherwise. The caller "
                "catches only ValueError/FileNotFoundError, so this escapes the indexer "
                "rather than skipping the file. The native side must not reproduce it."
            ),
        ),
    ]


def _day_accumulator_cases() -> list[dict[str, Any]]:
    return [
        _case(
            "day_accumulator_nominal",
            DAY_ACCUMULATOR,
            "DayAccumulator",
            [{"ts": 1772000000000, "summary": "steady morning"}],
            note="the agent comes from the file stem, which requires context file_path",
        ),
        _case("day_accumulator_empty", DAY_ACCUMULATOR, "DayAccumulator", []),
    ]


def _talent_json_cases() -> list[dict[str, Any]]:
    return [
        _case(
            "sense_nominal",
            SENSE,
            "Sense",
            [{"entities": [{"name": "Alice", "type": "Person"}]}],
        ),
        _case(
            "sense_non_object_first_entry",
            SENSE,
            "Sense",
            [],
            note="an empty or malformed talent JSON renders nothing and reports nothing",
        ),
        _case(
            "documents_nominal",
            DOCUMENTS,
            "Documents",
            [
                {
                    "documents": [
                        {
                            "title": "Contract",
                            "summary": "Signed",
                            "kind": "pdf",
                        }
                    ]
                }
            ],
        ),
        _case("documents_empty", DOCUMENTS, "Documents", []),
        _case(
            "screen_record_nominal",
            SCREEN_RECORD,
            "Screen",
            [{"summary": "Editor open", "applications": ["nvim"]}],
            note="this is the talents/screen.json record, not the raw screen stream",
        ),
        _case("screen_record_empty", SCREEN_RECORD, "Screen", []),
        _case(
            "morning_briefing_nominal",
            MORNING_BRIEFING,
            "MorningBriefing",
            [{"greeting": "Morning", "sections": [{"title": "Today", "items": ["Ship"]}]}],
        ),
        _case("morning_briefing_empty", MORNING_BRIEFING, "MorningBriefing", []),
    ]


def build_content_families_fixture() -> dict[str, Any]:
    cases: list[dict[str, Any]] = []
    cases.extend(_event_cases())
    cases.extend(_activity_cases())
    cases.extend(_action_log_cases())
    cases.extend(_facet_entity_cases())
    cases.extend(_observation_cases())
    cases.extend(_structured_import_cases())
    cases.extend(_ai_chat_cases())
    cases.extend(_chat_cases())
    cases.extend(_browser_cases())
    cases.extend(_day_accumulator_cases())
    cases.extend(_talent_json_cases())

    # A case whose family is null is a dispatch fact the native side has no family
    # for — it belongs in the corpus precisely because nothing else records it.
    families = sorted({case["family"] for case in cases if case["family"] is not None})
    return {
        "fixture": "solstone-content-families",
        "fixture_version": FIXTURE_VERSION,
        "generated_by": "make core-fixtures",
        "generator_timezone": GENERATOR_TZ,
        "families": families,
        "cases": cases,
    }
