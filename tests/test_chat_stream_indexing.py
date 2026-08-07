# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc


def test_append_chat_event_indexes_without_rescan(tmp_path, monkeypatch):
    from solstone.convey.chat_stream import append_chat_event
    from solstone.think.indexer.journal import search_journal

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    from solstone.think.indexer.journal import get_journal_index
    import solstone.think.utils as think_utils

    think_utils._journal_path_cache = None
    conn, _ = get_journal_index(str(tmp_path))
    conn.close()

    total, results = search_journal("nebula phrase X42", stream="chat")
    assert total == 0
    assert results == []

    append_chat_event(
        "owner_message",
        text="unique nebula phrase X42",
        app="sol",
        path="/app/sol",
        facet="work",
    )

    total, results = search_journal("nebula phrase X42", stream="chat")
    assert total == 1
    assert any("nebula phrase x42" in result["text"].lower() for result in results)
    assert {result["metadata"]["stream"] for result in results} == {"chat"}

    append_chat_event(
        "owner_message",
        text="second unique aurora signal Y99",
        app="sol",
        path="/app/sol",
        facet="work",
    )

    total, results = search_journal("aurora signal Y99", stream="chat")
    assert total == 1
    assert any("aurora signal y99" in result["text"].lower() for result in results)
    assert {result["metadata"]["stream"] for result in results} == {"chat"}

    total, results = search_journal("nebula phrase X42", stream="chat")
    assert total == 1
    assert any("nebula phrase x42" in result["text"].lower() for result in results)


def test_chat_formatter_handles_sol_initiated_events():
    from solstone.convey.sol_initiated.copy import (
        CATEGORIES,
        KIND_OWNER_CHAT_DISMISSED,
        KIND_OWNER_CHAT_OPEN,
        KIND_SOL_CHAT_REQUEST,
        KIND_SOL_CHAT_REQUEST_SUPERSEDED,
    )
    from solstone.think.chat_formatter import format_chat

    chunks, _ = format_chat(
        [
            {
                "kind": KIND_SOL_CHAT_REQUEST,
                "ts": 1,
                "request_id": "req",
                "summary": "unique solar cue Z77",
                "message": "extended amber detail",
                "category": CATEGORIES[0],
                "dedupe": "dedupe-z77",
                "dedupe_window": "24h",
                "since_ts": 1,
                "trigger_talent": "reflection",
            }
        ]
    )
    assert chunks[0]["markdown"] == "[sol] unique solar cue Z77\nextended amber detail"

    chunks, _ = format_chat(
        [
            {
                "kind": KIND_OWNER_CHAT_OPEN,
                "request_id": "req",
                "surface": "surface marker should not index",
            },
            {
                "kind": KIND_OWNER_CHAT_DISMISSED,
                "request_id": "req",
                "surface": "surface",
                "reason": "dismiss marker should not index",
            },
            {
                "kind": KIND_SOL_CHAT_REQUEST_SUPERSEDED,
                "request_id": "req",
                "replaced_by": "replacement marker should not index",
            },
        ]
    )
    assert chunks == []
