# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import base64
import json
import logging
import threading
import time
from datetime import date, datetime, timedelta
from pathlib import Path

import httpx
import pytest
from flask import Flask

from solstone.apps.chat.copy import (
    CHAT_CLOSER_SUPPORT_SEND_FAILED,
    CHAT_OFFER_SUPPORT_DECLINE,
    CHAT_OFFER_SUPPORT_PROMPT,
    CHAT_SUPPORT_ATTACH_FILED_FORMAT,
    CHAT_SUPPORT_CLOSE_SUBMITTED,
    CHAT_SUPPORT_DRAFT_CANCELLED,
    CHAT_SUPPORT_DRAFT_READY,
    CHAT_SUPPORT_IN_PROGRESS,
    CHAT_SUPPORT_RECONSENT_NEEDED,
    CHAT_SUPPORT_RESOLVED_SUBMITTED,
    CHAT_SUPPORT_STILL_NEED_HELP_SUBMITTED,
    CHAT_SUPPORT_SUBMIT_AMBIGUOUS,
    CHAT_SUPPORT_SUBMIT_FAILED,
    CHAT_SUPPORT_SUBMIT_FILED_FORMAT,
    talent_label_for,
)
from solstone.convey.chat import ChatSpawnResult, chat_bp
from solstone.convey.chat_stream import (
    append_chat_event,
    read_chat_events,
    reduce_chat_state,
)
from solstone.think.cortex_client import CortexSpawnUnavailable


def _setup_journal(tmp_path, monkeypatch):
    journal = tmp_path / "journal"
    journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    return journal


def _reset_chat_state(chat_module) -> None:
    chat_module.stop_all_chat_runtime()
    with chat_module._state_lock:
        chat_module._current_chat_use_id = None
        chat_module._current_chat_state = None
        chat_module._queued_triggers.clear()
        chat_module._active_talents.clear()
        chat_module._reserved_use_ids.clear()
        chat_module._thinking_buffers.clear()
        chat_module._thinking_providers.clear()
        for timer in chat_module._watchdog_timers.values():
            timer.cancel()
        chat_module._watchdog_timers.clear()
        chat_module._last_use_id = 0


def _ms(year: int, month: int, day: int, hour: int, minute: int, second: int) -> int:
    return int(datetime(year, month, day, hour, minute, second).timestamp() * 1000)


def _write_talent_log(
    journal, talent_name: str, filename: str, events: list[dict]
) -> None:
    talent_dir = journal / "talents" / talent_name
    talent_dir.mkdir(parents=True, exist_ok=True)
    log_path = talent_dir / filename
    log_path.write_text(
        "\n".join(json.dumps(event) for event in events) + "\n",
        encoding="utf-8",
    )


def _post_chat_message(client, message: str):
    return client.post(
        "/api/chat",
        json={
            "message": message,
            "app": "sol",
            "path": "/app/sol",
            "facet": "work",
        },
    )


def _set_current_chat(chat_module, logical_use_id: str, raw_use_id: str | None) -> None:
    with chat_module._state_lock:
        chat_module._current_chat_use_id = logical_use_id
        chat_module._current_chat_state = {
            "raw_use_id": raw_use_id,
            "raw_use_ids_seen": {raw_use_id} if raw_use_id else set(),
            "trigger": {"type": "owner_message", "message": "help"},
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "retry_count": 0,
        }


def _set_current_chat_trigger(chat_module, trigger: dict) -> None:
    _set_current_chat(chat_module, "logical-chat", "raw-chat")
    with chat_module._state_lock:
        chat_module._current_chat_state["trigger"] = trigger


def _talent_route_result(target: str, task: str = "file a ticket") -> dict:
    return {
        "message": "let me file that",
        "notes": target,
        "talent_request": {"target": target, "task": task},
    }


def _support_create_payload(diagnostics: dict) -> dict:
    return {
        "subject": "Bug report",
        "description": "Something broke",
        "product": "solstone",
        "severity": "medium",
        "category": "bug",
        "user_context": diagnostics,
        "auto_context": False,
        "anonymous": False,
    }


def _support_attach_payload(
    data: bytes = b"attachment bytes",
    *,
    ticket_id: int = 77,
    filename: str = "evidence.txt",
) -> dict:
    return {
        "ticket_id": ticket_id,
        "filename": filename,
        "content_type": "text/plain",
        "byte_size": len(data),
        "content_b64": base64.b64encode(data).decode("ascii"),
    }


def _append_support_draft(
    draft_id: str,
    *,
    verb: str = "create",
    payload: dict | None = None,
    diagnostics_snapshot: dict | None = None,
    captured_day: str | None = None,
    ts: int | None = None,
) -> None:
    append_chat_event(
        "support_draft",
        **({"ts": ts} if ts is not None else {}),
        draft_id=draft_id,
        captured_day=captured_day or date.today().strftime("%Y%m%d"),
        verb=verb,
        payload=payload or _support_create_payload(diagnostics_snapshot or {}),
        diagnostics_snapshot=diagnostics_snapshot,
    )


def _events_of_kind(day: str, kind: str) -> list[dict]:
    return [event for event in read_chat_events(day) if event["kind"] == kind]


@pytest.fixture
def chat_client(tmp_path, monkeypatch):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)

    app = Flask(__name__)
    app.config["TESTING"] = True
    app.register_blueprint(chat_bp)
    return app.test_client()


def test_cortex_thinking_reaches_sol_message(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )
    _set_current_chat(chat, "logical-chat", "raw-chat")

    chat._handle_callosum_message(
        {
            "tract": "cortex",
            "event": "start",
            "use_id": "raw-chat",
            "provider": "openai",
        }
    )
    for summary in ("first thought", "second thought"):
        chat._handle_callosum_message(
            {
                "tract": "cortex",
                "event": "thinking",
                "use_id": "raw-chat",
                "summary": summary,
            }
        )
    chat._on_cortex_finish(
        {
            "use_id": "raw-chat",
            "model": "gpt-reasoning",
            "usage": {"reasoning_tokens": 100},
            "result": json.dumps(
                {
                    "message": "done",
                    "notes": "ok",
                    "talent_request": None,
                }
            ),
        }
    )

    sol_message = next(
        event
        for event in read_chat_events(date.today().strftime("%Y%m%d"))
        if event["kind"] == "sol_message"
    )
    assert sol_message["thinking"] == {
        "content": "first thought\n\nsecond thought",
        "provider": "openai",
        "model": "gpt-reasoning",
        "tokens": 100,
    }
    assert "raw-chat" not in chat._thinking_buffers
    assert "raw-chat" not in chat._thinking_providers


def test_cortex_thinking_reaches_talent_finished(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )
    monkeypatch.setattr("solstone.convey.chat._run_next_action", lambda _action: None)
    monkeypatch.setattr(
        "solstone.convey.chat._arm_watchdog_locked", lambda *_args, **_kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._cancel_watchdog_locked", lambda *_args, **_kwargs: None
    )
    _set_current_chat(chat, "logical-chat", None)
    with chat._state_lock:
        chat._active_talents["talent-raw"] = {
            "chat_use_id": "logical-chat",
            "target": "exec",
            "task": "research",
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "ask": "help",
        }

    chat._handle_callosum_message(
        {
            "tract": "cortex",
            "event": "start",
            "use_id": "talent-raw",
            "provider": "anthropic",
        }
    )
    chat._handle_callosum_message(
        {
            "tract": "cortex",
            "event": "thinking",
            "use_id": "talent-raw",
            "summary": "talent thought",
        }
    )
    chat._on_cortex_finish(
        {
            "use_id": "talent-raw",
            "model": "claude-reasoning",
            "usage": {"reasoning_tokens": 7},
            "result": "summary",
        }
    )

    finished = next(
        event
        for event in read_chat_events(date.today().strftime("%Y%m%d"))
        if event["kind"] == "talent_finished"
    )
    assert finished["thinking"] == {
        "content": "talent thought",
        "provider": "anthropic",
        "model": "claude-reasoning",
        "tokens": 7,
    }


def test_first_support_route_intercepts_with_offer(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )
    _set_current_chat(chat, "logical-chat", "raw-chat")

    chat._on_cortex_finish(
        {
            "use_id": "raw-chat",
            "result": _talent_route_result("support"),
        }
    )

    events = read_chat_events(date.today().strftime("%Y%m%d"))
    sol_message = next(event for event in events if event["kind"] == "sol_message")
    assert sol_message["offer"] == {"kind": "support"}
    assert sol_message["text"] == CHAT_OFFER_SUPPORT_PROMPT
    assert [event for event in events if event["kind"] == "talent_spawned"] == []


def test_support_route_with_pending_offer_allows_spawn(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )
    monkeypatch.setattr("solstone.convey.chat._run_next_action", lambda _action: None)
    append_chat_event(
        "owner_message",
        text="x",
        app="sol",
        path="/app/sol",
        facet="work",
    )
    append_chat_event(
        "sol_message",
        use_id="seed-offer",
        text=CHAT_OFFER_SUPPORT_PROMPT,
        notes="offer",
        requested_target=None,
        requested_task=None,
        offer={"kind": "support"},
    )
    _set_current_chat(chat, "logical-chat", "raw-chat")

    chat._on_cortex_finish(
        {
            "use_id": "raw-chat",
            "result": _talent_route_result("support"),
        }
    )

    events = read_chat_events(date.today().strftime("%Y%m%d"))
    assert any(
        event["kind"] == "talent_spawned" and event["name"] == "support"
        for event in events
    )
    sol_message = next(
        event
        for event in events
        if event["kind"] == "sol_message" and event["use_id"] == "logical-chat"
    )
    assert "offer" not in sol_message


def test_support_route_after_support_spawn_allows_spawn(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )
    monkeypatch.setattr("solstone.convey.chat._run_next_action", lambda _action: None)
    append_chat_event(
        "talent_spawned",
        use_id="prior",
        name="support",
        task="t",
        started_at=1,
    )
    _set_current_chat(chat, "logical-chat", "raw-chat")

    chat._on_cortex_finish(
        {
            "use_id": "raw-chat",
            "result": _talent_route_result("support"),
        }
    )

    events = read_chat_events(date.today().strftime("%Y%m%d"))
    support_spawns = [
        event
        for event in events
        if event["kind"] == "talent_spawned" and event["name"] == "support"
    ]
    assert len(support_spawns) == 2
    sol_message = next(
        event
        for event in events
        if event["kind"] == "sol_message" and event["use_id"] == "logical-chat"
    )
    assert "offer" not in sol_message


@pytest.mark.parametrize("target", ("read", "exec"))
def test_non_outbound_routes_are_not_gated(chat_client, monkeypatch, target):
    import solstone.convey.chat as chat

    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )
    monkeypatch.setattr("solstone.convey.chat._run_next_action", lambda _action: None)
    _set_current_chat(chat, "logical-chat", "raw-chat")

    chat._on_cortex_finish(
        {
            "use_id": "raw-chat",
            "result": _talent_route_result(target, "do the work"),
        }
    )

    events = read_chat_events(date.today().strftime("%Y%m%d"))
    assert any(
        event["kind"] == "talent_spawned" and event["name"] == target
        for event in events
    )
    sol_message = next(
        event
        for event in events
        if event["kind"] == "sol_message" and event["use_id"] == "logical-chat"
    )
    assert "offer" not in sol_message


def test_dispatch_clears_turn_and_records_origin_ask(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    actions: list[dict] = []
    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._run_next_action",
        lambda action: actions.append(action) if action is not None else None,
    )
    _set_current_chat(chat, "logical-chat", "raw-chat")

    chat._on_cortex_finish(
        {
            "use_id": "raw-chat",
            "result": _talent_route_result("exec", "do the work"),
        }
    )

    with chat._state_lock:
        assert chat._current_chat_state is None
        assert chat._current_chat_use_id is None
        talent_state = next(iter(chat._active_talents.values()))
    assert talent_state["ask"] == "help"
    assert actions[-1]["kind"] == "talent"
    events = read_chat_events(date.today().strftime("%Y%m%d"))
    sol_message = next(event for event in events if event["kind"] == "sol_message")
    assert sol_message["requested_target"] == "exec"
    assert sol_message["requested_task"] == "do the work"
    assert "origin" not in sol_message


def test_second_owner_message_starts_after_dispatch_not_queued(
    chat_client, monkeypatch
):
    import solstone.convey.chat as chat

    starts: list[dict] = []
    monkeypatch.setattr(
        "solstone.think.identity.ensure_identity_directory", lambda: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._spawn_chat_generate",
        lambda action: starts.append(action) or ChatSpawnResult(ok=True),
    )
    monkeypatch.setattr("solstone.convey.chat._spawn_talent", lambda _action: True)
    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._arm_watchdog_locked", lambda *_args, **_kwargs: None
    )

    first = _post_chat_message(chat_client, "first")
    assert first.status_code == 200
    assert first.get_json()["queued"] is False
    chat._on_cortex_finish(
        {
            "use_id": starts[0]["raw_use_id"],
            "result": _talent_route_result("exec", "do the work"),
        }
    )

    second = _post_chat_message(chat_client, "second")

    assert second.status_code == 200
    assert second.get_json()["queued"] is False
    assert [start["trigger"]["message"] for start in starts] == ["first", "second"]


def test_empty_dispatch_ack_uses_nonraising_liveness_backstop(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )
    monkeypatch.setattr("solstone.convey.chat._run_next_action", lambda _action: None)
    _set_current_chat(chat, "logical-chat", "raw-chat")

    chat._on_cortex_finish(
        {
            "use_id": "raw-chat",
            "result": {
                "message": "",
                "notes": "ok",
                "talent_request": {"target": "exec", "task": "research"},
            },
        }
    )

    sol_message = next(
        event
        for event in read_chat_events(date.today().strftime("%Y%m%d"))
        if event["kind"] == "sol_message"
    )
    assert sol_message["text"] == "making that change… research"
    assert chat._dispatch_ack_text("unknown", None, "") == "unknown"


def test_talent_finish_folds_fresh_turn_with_origin(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    actions: list[dict] = []
    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._run_next_action",
        lambda action: actions.append(action) if action is not None else None,
    )
    with chat._state_lock:
        chat._active_talents["talent-raw"] = {
            "chat_use_id": "dispatch-chat",
            "target": "exec",
            "task": "research",
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "ask": "what happened?",
        }

    chat._on_cortex_finish({"use_id": "talent-raw", "result": "summary"})

    assert actions and actions[-1]["kind"] == "chat"
    assert actions[-1]["logical_use_id"] != "dispatch-chat"
    assert actions[-1]["trigger"]["origin"] == {
        "logical_use_id": "dispatch-chat",
        "ask": "what happened?",
    }
    chat._on_cortex_finish(
        {
            "use_id": actions[-1]["raw_use_id"],
            "result": {
                "message": "Here is the answer with enough detail to stand alone.",
                "notes": "ok",
                "talent_request": None,
            },
        }
    )

    sol_messages = [
        event
        for event in read_chat_events(date.today().strftime("%Y%m%d"))
        if event["kind"] == "sol_message"
    ]
    assert sol_messages[-1]["origin"] == {
        "logical_use_id": "dispatch-chat",
        "ask": "what happened?",
    }
    assert sol_messages[-1]["requested_target"] is None


@pytest.mark.parametrize("spawn_failure", (False, True))
def test_talent_failure_paths_fold_with_origin(
    chat_client,
    monkeypatch,
    spawn_failure,
):
    import solstone.convey.chat as chat

    actions: list[dict] = []
    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._run_next_action",
        lambda action: actions.append(action) if action is not None else None,
    )
    with chat._state_lock:
        chat._active_talents["talent-raw"] = {
            "chat_use_id": "dispatch-chat",
            "target": "exec",
            "task": "research",
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "ask": "what failed?",
        }

    if spawn_failure:
        chat._handle_talent_spawn_failure(
            {
                "kind": "talent",
                "logical_use_id": "dispatch-chat",
                "target": "exec",
                "use_id": "talent-raw",
                "task": "research",
                "context": {},
                "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            }
        )
    else:
        chat._on_cortex_error({"use_id": "talent-raw", "error": "boom"})

    assert actions and actions[-1]["kind"] == "chat"
    assert actions[-1]["trigger"]["origin"] == {
        "logical_use_id": "dispatch-chat",
        "ask": "what failed?",
    }
    assert actions[-1]["trigger"]["type"] == "talent_errored"
    chat._on_cortex_finish(
        {
            "use_id": actions[-1]["raw_use_id"],
            "result": {"message": "ignored", "notes": "ok", "talent_request": None},
        }
    )

    folded = [
        event
        for event in read_chat_events(date.today().strftime("%Y%m%d"))
        if event["kind"] == "sol_message"
    ][-1]
    assert folded["origin"] == {
        "logical_use_id": "dispatch-chat",
        "ask": "what failed?",
    }
    assert folded["answer_state"] == "failed"
    assert folded["text"].startswith("I couldn't finish that lookup")


def test_deferred_spawn_promotes_when_talent_slot_frees(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    actions: list[dict] = []
    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._run_next_action",
        lambda action: actions.append(action) if action is not None else None,
    )
    day = date.today().strftime("%Y%m%d")
    for use_id in ("active-1", "active-2"):
        append_chat_event(
            "talent_spawned",
            use_id=use_id,
            name="exec",
            task="existing",
            started_at=int(time.time() * 1000),
        )
    with chat._state_lock:
        for use_id in ("active-1", "active-2"):
            chat._active_talents[use_id] = {
                "chat_use_id": f"chat-{use_id}",
                "target": "exec",
                "task": "existing",
                "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
                "ask": "old ask",
            }
    _set_current_chat(chat, "logical-chat", "raw-chat")

    chat._on_cortex_finish(
        {
            "use_id": "raw-chat",
            "result": _talent_route_result("exec", "deferred"),
        }
    )

    queued = reduce_chat_state(day)["queued_talents"]
    assert len(queued) == 1
    queued_use_id = queued[0]["use_id"]
    chat._on_cortex_finish({"use_id": "active-1", "result": "done"})

    reduced = reduce_chat_state(day)
    assert reduced["queued_talents"] == []
    assert any(
        talent["use_id"] == queued_use_id for talent in reduced["active_talents"]
    )
    assert actions[-1]["kind"] == "talent"
    assert actions[-1]["use_id"] == queued_use_id


def test_clean_support_finish_with_pending_draft_emits_marker(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    monkeypatch.setattr("solstone.convey.chat._run_next_action", lambda _action: None)
    monkeypatch.setattr("solstone.convey.chat._emit_finish", lambda *_args: None)
    monkeypatch.setattr("solstone.convey.chat._emit_error", lambda *_args: None)
    draft_payload = {"subject": "Subj", "description": "Desc"}
    diagnostics = {"version": "9.9.9", "revision": "abc1234"}
    append_chat_event(
        "support_draft",
        draft_id="draft-1",
        captured_day=date.today().strftime("%Y%m%d"),
        verb="create",
        payload=draft_payload,
        diagnostics_snapshot=diagnostics,
    )
    _set_current_chat_trigger(
        chat,
        {"type": "talent_finished", "name": "support", "summary": "drafted"},
    )

    chat._on_cortex_finish(
        {
            "use_id": "raw-chat",
            "result": json.dumps(
                {
                    "message": "I drafted a request.",
                    "notes": "done",
                    "talent_request": None,
                }
            ),
        }
    )

    sol_message = next(
        event
        for event in read_chat_events(chat._today_day())
        if event["kind"] == "sol_message"
    )
    assert sol_message["text"] == CHAT_SUPPORT_DRAFT_READY
    assert sol_message["draft"] == {
        "draft_id": "draft-1",
        "verb": "create",
        "payload": draft_payload,
        "diagnostics_snapshot": diagnostics,
    }
    assert "offer" not in sol_message


def test_clean_support_finish_with_attach_draft_emits_slim_marker(
    chat_client, monkeypatch
):
    import solstone.convey.chat as chat

    monkeypatch.setattr("solstone.convey.chat._run_next_action", lambda _action: None)
    monkeypatch.setattr("solstone.convey.chat._emit_finish", lambda *_args: None)
    monkeypatch.setattr("solstone.convey.chat._emit_error", lambda *_args: None)
    full_payload = _support_attach_payload(b"private bytes", filename="shot.png")
    expected_payload = {
        "ticket_id": 77,
        "filename": "shot.png",
        "content_type": "text/plain",
        "byte_size": len(b"private bytes"),
    }
    append_chat_event(
        "support_draft",
        draft_id="draft-attach-marker",
        captured_day=date.today().strftime("%Y%m%d"),
        verb="attach",
        payload=full_payload,
        diagnostics_snapshot=None,
    )
    _set_current_chat_trigger(
        chat,
        {"type": "talent_finished", "name": "support", "summary": "drafted"},
    )

    chat._on_cortex_finish(
        {
            "use_id": "raw-chat",
            "result": json.dumps(
                {
                    "message": "I drafted an attachment.",
                    "notes": "done",
                    "talent_request": None,
                }
            ),
        }
    )

    day = chat._today_day()
    sol_message = next(
        event for event in read_chat_events(day) if event["kind"] == "sol_message"
    )
    expected_draft = {
        "draft_id": "draft-attach-marker",
        "verb": "attach",
        "payload": expected_payload,
        "diagnostics_snapshot": None,
    }
    assert sol_message["draft"] == expected_draft
    assert "content_b64" not in sol_message["draft"]["payload"]
    assert sol_message["sources"] == []
    assert sol_message["answer_state"] == "answered"
    assert reduce_chat_state(day)["latest_sol_message"]["draft"] == expected_draft
    session = chat_client.get("/api/chat/session")
    assert session.status_code == 200
    assert session.get_json()["latest_sol_message"]["draft"] == expected_draft


def test_errored_support_finish_with_pending_draft_keeps_send_failed_closer(
    chat_client, monkeypatch
):
    import solstone.convey.chat as chat

    monkeypatch.setattr("solstone.convey.chat._run_next_action", lambda _action: None)
    monkeypatch.setattr("solstone.convey.chat._emit_finish", lambda *_args: None)
    monkeypatch.setattr("solstone.convey.chat._emit_error", lambda *_args: None)
    append_chat_event(
        "support_draft",
        draft_id="draft-1",
        captured_day=date.today().strftime("%Y%m%d"),
        verb="create",
        payload={"subject": "Subj"},
        diagnostics_snapshot={"version": "9.9.9"},
    )
    _set_current_chat_trigger(
        chat,
        {
            "type": "talent_errored",
            "name": "support",
            "reason": "Traceback",
            "reason_code": "wall_clock_exceeded",
        },
    )

    chat._on_cortex_finish(
        {
            "use_id": "raw-chat",
            "result": json.dumps(
                {
                    "message": "I drafted a request.",
                    "notes": "blocked",
                    "talent_request": None,
                }
            ),
        }
    )

    sol_message = next(
        event
        for event in read_chat_events(chat._today_day())
        if event["kind"] == "sol_message"
    )
    assert sol_message["text"] == CHAT_CLOSER_SUPPORT_SEND_FAILED
    assert "draft" not in sol_message
    assert sol_message["sources"] == []
    assert sol_message["answer_state"] == "failed"


def test_clean_support_finish_without_pending_draft_does_not_emit_marker(
    chat_client, monkeypatch
):
    import solstone.convey.chat as chat

    monkeypatch.setattr("solstone.convey.chat._run_next_action", lambda _action: None)
    monkeypatch.setattr("solstone.convey.chat._emit_finish", lambda *_args: None)
    monkeypatch.setattr("solstone.convey.chat._emit_error", lambda *_args: None)
    _set_current_chat_trigger(
        chat,
        {"type": "talent_finished", "name": "support", "summary": "done"},
    )

    chat._on_cortex_finish(
        {
            "use_id": "raw-chat",
            "result": json.dumps(
                {
                    "message": "Done.",
                    "notes": "done",
                    "talent_request": None,
                }
            ),
        }
    )

    sol_message = next(
        event
        for event in read_chat_events(chat._today_day())
        if event["kind"] == "sol_message"
    )
    assert "draft" not in sol_message
    assert sol_message["text"] != CHAT_SUPPORT_DRAFT_READY
    assert sol_message["sources"] == []
    assert sol_message["answer_state"] == "partial"


def test_non_support_finish_with_pending_draft_does_not_emit_marker(
    chat_client, monkeypatch
):
    import solstone.convey.chat as chat

    monkeypatch.setattr("solstone.convey.chat._run_next_action", lambda _action: None)
    monkeypatch.setattr("solstone.convey.chat._emit_finish", lambda *_args: None)
    monkeypatch.setattr("solstone.convey.chat._emit_error", lambda *_args: None)
    append_chat_event(
        "support_draft",
        draft_id="draft-1",
        captured_day=date.today().strftime("%Y%m%d"),
        verb="create",
        payload={"subject": "Subj"},
        diagnostics_snapshot={"version": "9.9.9"},
    )
    _set_current_chat_trigger(
        chat,
        {"type": "talent_finished", "name": "read", "summary": "done"},
    )

    chat._on_cortex_finish(
        {
            "use_id": "raw-chat",
            "result": json.dumps(
                {
                    "message": "Done.",
                    "notes": "done",
                    "talent_request": None,
                }
            ),
        }
    )

    sol_message = next(
        event
        for event in read_chat_events(chat._today_day())
        if event["kind"] == "sol_message"
    )
    assert "draft" not in sol_message


def test_support_draft_state_result_seam_uses_latest_draft(monkeypatch):
    import solstone.convey.chat as chat

    monkeypatch.setattr(
        chat,
        "read_chat_events",
        lambda _day: [
            {"kind": "support_draft", "draft_id": "d1"},
            {"kind": "result", "draft_id": "d1"},
        ],
    )
    assert chat._support_draft_state("20260420") == "submitted"

    monkeypatch.setattr(
        chat,
        "read_chat_events",
        lambda _day: [
            {"kind": "support_draft", "draft_id": "d1"},
            {"kind": "support_draft", "draft_id": "d2"},
            {"kind": "result", "draft_id": "d1"},
        ],
    )
    assert chat._support_draft_state("20260420") == "pending"


def test_support_finish_draft_marker_does_not_emit_offer(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    monkeypatch.setattr("solstone.convey.chat._run_next_action", lambda _action: None)
    monkeypatch.setattr("solstone.convey.chat._emit_finish", lambda *_args: None)
    monkeypatch.setattr("solstone.convey.chat._emit_error", lambda *_args: None)
    append_chat_event(
        "support_draft",
        draft_id="draft-1",
        captured_day=date.today().strftime("%Y%m%d"),
        verb="create",
        payload={"subject": "Subj"},
        diagnostics_snapshot=None,
    )
    _set_current_chat_trigger(
        chat,
        {"type": "talent_finished", "name": "support", "summary": "drafted"},
    )

    chat._on_cortex_finish(
        {
            "use_id": "raw-chat",
            "result": json.dumps(
                {
                    "message": "I drafted a request.",
                    "notes": "done",
                    "talent_request": None,
                }
            ),
        }
    )

    sol_message = next(
        event
        for event in read_chat_events(chat._today_day())
        if event["kind"] == "sol_message"
    )
    assert "draft" in sol_message
    assert "offer" not in sol_message
    assert sol_message["sources"] == []
    assert sol_message["answer_state"] == "answered"


def test_decline_offer_endpoint_appends_local_sol_message(chat_client, monkeypatch):
    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )

    response = chat_client.post("/api/chat/offer/decline", json={})

    assert response.status_code == 200
    assert response.get_json() == {"ok": True}
    events = read_chat_events(date.today().strftime("%Y%m%d"))
    sol_message = next(event for event in events if event["kind"] == "sol_message")
    assert sol_message["text"] == CHAT_OFFER_SUPPORT_DECLINE
    assert "offer" not in sol_message
    assert sol_message["sources"] == []
    assert sol_message["answer_state"] == "answered"
    assert [event for event in events if event["kind"] == "talent_spawned"] == []


def test_support_draft_confirm_create_submits_captured_payload(
    chat_client, monkeypatch
):
    import solstone.convey.chat as chat

    diagnostics = {"version": "9.9.9", "revision": "abc1234"}
    payload = _support_create_payload(diagnostics)
    calls: list[dict] = []

    def record_support_create(**kwargs):
        calls.append(kwargs)
        return {"id": 123}

    def fail_collect_all():
        raise AssertionError("diagnostics should not be collected during confirm")

    monkeypatch.setattr(chat, "support_create", record_support_create)
    monkeypatch.setattr(
        "solstone.apps.support.diagnostics.collect_all",
        fail_collect_all,
    )
    _append_support_draft(
        "draft-create",
        payload=payload,
        diagnostics_snapshot=diagnostics,
    )

    response = chat_client.post(
        "/api/chat/support/draft/confirm",
        json={"draft_id": "draft-create"},
    )

    assert response.status_code == 200
    assert response.get_json() == {
        "ok": True,
        "outcome": "submitted",
        "ticket_id": 123,
    }
    assert calls == [{**payload, "action_id": "draft-create"}]
    assert calls[0]["auto_context"] is False
    assert calls[0]["user_context"] == diagnostics


def test_support_draft_confirm_feedback_uses_support_feedback_with_snapshot(
    chat_client, monkeypatch
):
    import solstone.convey.chat as chat

    diagnostics = {"version": "9.9.9", "revision": "abc1234"}
    calls: list[dict] = []

    def record_support_feedback(**kwargs):
        calls.append(kwargs)
        return {"id": 124}

    monkeypatch.setattr(chat, "support_feedback", record_support_feedback)
    _append_support_draft(
        "draft-feedback",
        verb="feedback",
        payload={"body": "I like this", "product": "solstone", "anonymous": True},
        diagnostics_snapshot=diagnostics,
    )

    response = chat_client.post(
        "/api/chat/support/draft/confirm",
        json={"draft_id": "draft-feedback"},
    )

    assert response.status_code == 200
    assert response.get_json() == {
        "ok": True,
        "outcome": "submitted",
        "ticket_id": 124,
    }
    assert calls == [
        {
            "body": "I like this",
            "product": "solstone",
            "anonymous": True,
            "user_context": diagnostics,
            "action_id": "draft-feedback",
        }
    ]


def test_support_draft_confirm_reply_uses_support_reply(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    replies: list[tuple[int, str, str]] = []

    def record_support_reply(ticket_id, content, *, action_id):
        replies.append((ticket_id, content, action_id))
        return {"id": "reply-1"}

    monkeypatch.setattr(
        chat,
        "support_create",
        lambda **_kwargs: pytest.fail("support_create should not be called"),
    )
    monkeypatch.setattr(chat, "support_reply", record_support_reply)
    _append_support_draft(
        "draft-reply",
        verb="reply",
        payload={"ticket_id": 77, "content": "More detail"},
        diagnostics_snapshot=None,
    )

    response = chat_client.post(
        "/api/chat/support/draft/confirm",
        json={"draft_id": "draft-reply"},
    )

    assert response.status_code == 200
    assert response.get_json() == {
        "ok": True,
        "outcome": "submitted",
        "ticket_id": 77,
    }
    assert replies == [(77, "More detail", "draft-reply")]


def test_support_draft_confirm_attach_uploads_captured_bytes(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    monkeypatch.setattr(
        chat,
        "support_create",
        lambda **_kwargs: pytest.fail("support_create should not be called"),
    )
    monkeypatch.setattr(
        chat,
        "support_reply",
        lambda *_args: pytest.fail("support_reply should not be called"),
    )
    captured = b"saved bytes"
    calls: list[tuple[int, str, str | None, bytes, str]] = []

    def record_support_attach(ticket_id, file_path, *, filename=None, action_id):
        path = Path(file_path)
        calls.append((ticket_id, file_path, filename, path.read_bytes(), action_id))
        assert path.exists()
        return {"id": "attachment-1"}

    monkeypatch.setattr(chat, "support_attach", record_support_attach)
    _append_support_draft(
        "draft-attach",
        verb="attach",
        payload=_support_attach_payload(captured, filename="capture.txt"),
        diagnostics_snapshot=None,
    )

    response = chat_client.post(
        "/api/chat/support/draft/confirm",
        json={"draft_id": "draft-attach"},
    )

    assert response.status_code == 200
    assert response.get_json() == {
        "ok": True,
        "outcome": "submitted",
        "ticket_id": 77,
    }
    assert len(calls) == 1
    ticket_id, tmp_path, filename, data, action_id = calls[0]
    assert (ticket_id, filename, data, action_id) == (
        77,
        "capture.txt",
        captured,
        "draft-attach",
    )
    assert not Path(tmp_path).exists()
    day = date.today().strftime("%Y%m%d")
    result = _events_of_kind(day, "result")[0]
    assert result == {
        "kind": "result",
        "ts": result["ts"],
        "draft_id": "draft-attach",
        "ok": True,
        "ticket_id": 77,
    }
    sol_message = _events_of_kind(day, "sol_message")[0]
    assert sol_message["text"] == CHAT_SUPPORT_ATTACH_FILED_FORMAT.format(ticket_id=77)
    assert "draft" not in sol_message


def test_support_draft_confirm_attach_uses_captured_bytes_after_source_deleted(
    chat_client, monkeypatch, tmp_path
):
    import solstone.convey.chat as chat
    from solstone.apps.support.routes import support_bp

    monkeypatch.setattr("solstone.apps.support.routes._enabled", lambda: True)
    source = tmp_path / "source.txt"
    original = b"original captured bytes"
    source.write_bytes(original)

    support_app = Flask(__name__)
    support_app.config["TESTING"] = True
    support_app.register_blueprint(support_bp)
    with source.open("rb") as handle:
        draft_response = support_app.test_client().post(
            "/app/support/api/draft",
            data={
                "verb": "attach",
                "ticket_id": "81",
                "file": (handle, "source.txt"),
            },
            content_type="multipart/form-data",
        )
    assert draft_response.status_code == 200
    draft_id = draft_response.get_json()["draft_id"]
    source.unlink()

    calls: list[bytes] = []

    def record_support_attach(_ticket_id, file_path, *, filename=None, action_id):
        calls.append(Path(file_path).read_bytes())
        return {"id": "attachment-1"}

    monkeypatch.setattr(chat, "support_attach", record_support_attach)

    response = chat_client.post(
        "/api/chat/support/draft/confirm",
        json={"draft_id": draft_id},
    )

    assert response.status_code == 200
    assert response.get_json() == {
        "ok": True,
        "outcome": "submitted",
        "ticket_id": 81,
    }
    assert calls == [original]


def test_support_draft_confirm_attach_cleans_temp_file_on_failure(
    chat_client, monkeypatch
):
    import solstone.convey.chat as chat

    temp_paths: list[str] = []

    def fail_support_attach(_ticket_id, file_path, *, filename=None, action_id):
        temp_paths.append(file_path)
        request = httpx.Request("POST", "http://x")
        raise httpx.ConnectError("x", request=request)

    monkeypatch.setattr(chat, "support_attach", fail_support_attach)
    _append_support_draft(
        "draft-attach-fail",
        verb="attach",
        payload=_support_attach_payload(),
        diagnostics_snapshot=None,
    )

    response = chat_client.post(
        "/api/chat/support/draft/confirm",
        json={"draft_id": "draft-attach-fail"},
    )

    assert response.status_code == 200
    assert response.get_json() == {"ok": False, "outcome": "failed"}
    assert len(temp_paths) == 1
    assert not Path(temp_paths[0]).exists()


def test_support_draft_confirm_attach_is_idempotent(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    calls: list[tuple[int, str | None]] = []

    def record_support_attach(ticket_id, _file_path, *, filename=None, action_id):
        calls.append((ticket_id, filename, action_id))
        return {"id": "attachment-1"}

    monkeypatch.setattr(chat, "support_attach", record_support_attach)
    _append_support_draft(
        "draft-attach-once",
        verb="attach",
        payload=_support_attach_payload(filename="once.txt"),
        diagnostics_snapshot=None,
    )

    first = chat_client.post(
        "/api/chat/support/draft/confirm",
        json={"draft_id": "draft-attach-once"},
    )
    second = chat_client.post(
        "/api/chat/support/draft/confirm",
        json={"draft_id": "draft-attach-once"},
    )

    assert first.status_code == 200
    assert first.get_json()["outcome"] == "submitted"
    assert second.status_code == 200
    assert second.get_json() == {"ok": False, "outcome": "already_submitted"}
    assert calls == [(77, "once.txt", "draft-attach-once")]


def test_support_draft_attach_superseded_terminal_and_cancel_noop(
    chat_client, monkeypatch
):
    import solstone.convey.chat as chat

    calls: list[tuple] = []
    monkeypatch.setattr(
        chat,
        "support_attach",
        lambda *args, **kwargs: calls.append((args, kwargs)) or {"id": 1},
    )
    _append_support_draft(
        "draft-attach-old",
        verb="attach",
        payload=_support_attach_payload(filename="old.txt"),
        diagnostics_snapshot=None,
    )
    _append_support_draft(
        "draft-attach-new",
        verb="attach",
        payload=_support_attach_payload(filename="new.txt"),
        diagnostics_snapshot=None,
    )

    superseded = chat_client.post(
        "/api/chat/support/draft/confirm",
        json={"draft_id": "draft-attach-old"},
    )
    cancelled = chat_client.post(
        "/api/chat/support/draft/cancel",
        json={"draft_id": "draft-attach-new"},
    )
    terminal = chat_client.post(
        "/api/chat/support/draft/confirm",
        json={"draft_id": "draft-attach-new"},
    )

    assert superseded.status_code == 200
    assert superseded.get_json() == {"ok": False, "outcome": "superseded"}
    assert cancelled.status_code == 200
    assert cancelled.get_json() == {"ok": True, "outcome": "cancelled"}
    assert terminal.status_code == 200
    assert terminal.get_json() == {"ok": False, "outcome": "already_submitted"}
    assert calls == []


@pytest.mark.parametrize(
    ("exc_factory", "outcome", "copy_text", "ambiguous"),
    [
        (
            lambda request: httpx.ConnectError("x", request=request),
            "failed",
            CHAT_SUPPORT_SUBMIT_FAILED,
            False,
        ),
        (
            lambda request: httpx.ReadTimeout("x", request=request),
            "ambiguous",
            CHAT_SUPPORT_SUBMIT_AMBIGUOUS,
            True,
        ),
        (
            lambda request: httpx.HTTPStatusError(
                "bad request",
                request=request,
                response=httpx.Response(400, request=request),
            ),
            "failed",
            CHAT_SUPPORT_SUBMIT_FAILED,
            False,
        ),
        (
            lambda request: httpx.HTTPStatusError(
                "unavailable",
                request=request,
                response=httpx.Response(503, request=request),
            ),
            "ambiguous",
            CHAT_SUPPORT_SUBMIT_AMBIGUOUS,
            True,
        ),
    ],
)
def test_support_draft_confirm_attach_failure_copy_is_honest(
    chat_client,
    monkeypatch,
    exc_factory,
    outcome,
    copy_text,
    ambiguous,
):
    import solstone.convey.chat as chat

    def fail_support_attach(_ticket_id, _file_path, *, filename=None, action_id):
        request = httpx.Request("POST", "http://x")
        raise exc_factory(request)

    monkeypatch.setattr(chat, "support_attach", fail_support_attach)
    _append_support_draft(
        f"draft-attach-{outcome}-{ambiguous}",
        verb="attach",
        payload=_support_attach_payload(),
        diagnostics_snapshot=None,
    )

    response = chat_client.post(
        "/api/chat/support/draft/confirm",
        json={"draft_id": f"draft-attach-{outcome}-{ambiguous}"},
    )

    assert response.status_code == 200
    assert response.get_json() == {"ok": False, "outcome": outcome}
    day = date.today().strftime("%Y%m%d")
    assert _events_of_kind(day, "result") == []
    sol_message = _events_of_kind(day, "sol_message")[0]
    assert sol_message["text"] == copy_text


def test_support_draft_confirm_claim_is_race_safe(chat_client, monkeypatch):
    import queue

    import solstone.convey.chat as chat

    app = chat_client.application
    submit_started = threading.Event()
    release_submit = threading.Event()
    submit_lock = threading.Lock()
    submit_count = 0

    def blocking_support_create(**_kwargs):
        nonlocal submit_count
        with submit_lock:
            submit_count += 1
            attempt = submit_count
        if attempt == 2:
            from solstone.apps.support.operations import OperationInProgressError

            raise OperationInProgressError()
        submit_started.set()
        assert release_submit.wait(timeout=5), "support_create release was not signaled"
        return {"id": 123}

    monkeypatch.setattr(chat, "support_create", blocking_support_create)

    diagnostics = {"iteration": "deterministic"}
    draft_id = "draft-race"
    _append_support_draft(
        draft_id,
        payload=_support_create_payload(diagnostics),
        diagnostics_snapshot=diagnostics,
    )
    responses: queue.Queue[tuple[int, dict]] = queue.Queue()
    errors: queue.Queue[BaseException] = queue.Queue()

    def post_confirm() -> None:
        try:
            with app.test_client() as client:
                response = client.post(
                    "/api/chat/support/draft/confirm",
                    json={"draft_id": draft_id},
                )
                responses.put((response.status_code, response.get_json()))
        except BaseException as exc:
            errors.put(exc)

    first = threading.Thread(target=post_confirm)
    first.start()
    assert submit_started.wait(timeout=3), "first confirm did not reach support_create"

    second = threading.Thread(target=post_confirm)
    in_progress_response = None
    try:
        second.start()
        second.join(timeout=3)
        second_finished_while_first_in_flight = not second.is_alive()
        if second_finished_while_first_in_flight and errors.empty():
            in_progress_response = responses.get(timeout=1)
    finally:
        release_submit.set()
    first.join(timeout=3)
    second.join(timeout=3)

    assert second_finished_while_first_in_flight
    assert not first.is_alive()
    assert not second.is_alive()
    if not errors.empty():
        raise errors.get()

    submitted_response = responses.get(timeout=1)
    assert in_progress_response == (
        200,
        {"ok": False, "outcome": "in_progress"},
    )
    assert submitted_response == (
        200,
        {"ok": True, "outcome": "submitted", "ticket_id": 123},
    )
    with submit_lock:
        assert submit_count == 2


def test_support_draft_confirm_transitions_state_and_suppresses_marker(
    chat_client, monkeypatch
):
    import solstone.convey.chat as chat
    import solstone.convey.chat_stream as chat_stream

    monkeypatch.setattr(chat, "support_create", lambda **_kwargs: {"id": 123})
    monkeypatch.setattr("solstone.convey.chat._run_next_action", lambda _action: None)
    monkeypatch.setattr("solstone.convey.chat._emit_finish", lambda *_args: None)
    monkeypatch.setattr("solstone.convey.chat._emit_error", lambda *_args: None)
    diagnostics = {"version": "9.9.9"}
    _append_support_draft(
        "draft-state",
        payload=_support_create_payload(diagnostics),
        diagnostics_snapshot=diagnostics,
    )

    response = chat_client.post(
        "/api/chat/support/draft/confirm",
        json={"draft_id": "draft-state"},
    )

    assert response.status_code == 200
    day = date.today().strftime("%Y%m%d")
    assert "result" in chat_stream._VALID_KINDS
    assert any(
        event["kind"] == "result" and event["draft_id"] == "draft-state"
        for event in read_chat_events(day)
    )
    assert chat._support_draft_state(day) == "submitted"

    _set_current_chat_trigger(
        chat,
        {"type": "talent_finished", "name": "support", "summary": "drafted"},
    )
    chat._on_cortex_finish(
        {
            "use_id": "raw-chat",
            "result": json.dumps(
                {
                    "message": "Support finished.",
                    "notes": "done",
                    "talent_request": None,
                }
            ),
        }
    )

    sol_message = _events_of_kind(day, "sol_message")[-1]
    assert sol_message["text"] != CHAT_SUPPORT_DRAFT_READY
    assert "draft" not in sol_message


@pytest.mark.parametrize(
    ("exc_factory", "outcome", "copy_text", "ambiguous"),
    [
        (
            lambda request: httpx.ConnectError("x", request=request),
            "failed",
            CHAT_SUPPORT_SUBMIT_FAILED,
            False,
        ),
        (
            lambda request: httpx.ReadTimeout("x", request=request),
            "ambiguous",
            CHAT_SUPPORT_SUBMIT_AMBIGUOUS,
            True,
        ),
        (
            lambda request: httpx.HTTPStatusError(
                "bad request",
                request=request,
                response=httpx.Response(400, request=request),
            ),
            "failed",
            CHAT_SUPPORT_SUBMIT_FAILED,
            False,
        ),
        (
            lambda request: httpx.HTTPStatusError(
                "unavailable",
                request=request,
                response=httpx.Response(503, request=request),
            ),
            "ambiguous",
            CHAT_SUPPORT_SUBMIT_AMBIGUOUS,
            True,
        ),
    ],
)
def test_support_draft_confirm_failure_copy_is_honest(
    chat_client,
    monkeypatch,
    exc_factory,
    outcome,
    copy_text,
    ambiguous,
):
    import solstone.convey.chat as chat

    def fail_support_create(**_kwargs):
        request = httpx.Request("POST", "http://x")
        raise exc_factory(request)

    monkeypatch.setattr(chat, "support_create", fail_support_create)
    diagnostics = {"version": "9.9.9"}
    _append_support_draft(
        f"draft-{outcome}-{ambiguous}",
        payload=_support_create_payload(diagnostics),
        diagnostics_snapshot=diagnostics,
    )

    response = chat_client.post(
        "/api/chat/support/draft/confirm",
        json={"draft_id": f"draft-{outcome}-{ambiguous}"},
    )

    assert response.status_code == 200
    assert response.get_json() == {"ok": False, "outcome": outcome}
    day = date.today().strftime("%Y%m%d")
    assert _events_of_kind(day, "result") == []
    sol_message = _events_of_kind(day, "sol_message")[0]
    assert sol_message["text"] == copy_text


def test_support_draft_confirm_sol_message_supersedes_draft(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    monkeypatch.setattr(chat, "support_create", lambda **_kwargs: {"id": 123})
    diagnostics = {"version": "9.9.9"}
    _append_support_draft(
        "draft-reduce",
        payload=_support_create_payload(diagnostics),
        diagnostics_snapshot=diagnostics,
    )

    response = chat_client.post(
        "/api/chat/support/draft/confirm",
        json={"draft_id": "draft-reduce"},
    )

    assert response.status_code == 200
    day = date.today().strftime("%Y%m%d")
    sol_message = _events_of_kind(day, "sol_message")[-1]
    assert sol_message["text"] == CHAT_SUPPORT_SUBMIT_FILED_FORMAT.format(ticket_id=123)
    assert "draft" not in sol_message
    assert sol_message["sources"] == []
    assert sol_message["answer_state"] == "answered"
    assert reduce_chat_state(day)["latest_sol_message"]["draft"] is None
    session = chat_client.get("/api/chat/session")
    assert session.status_code == 200
    assert session.get_json()["latest_sol_message"]["draft"] is None


def test_support_draft_confirm_superseded_and_not_found_noop(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    calls: list[dict] = []
    monkeypatch.setattr(
        chat,
        "support_create",
        lambda **kwargs: calls.append(kwargs) or {"id": 123},
    )
    diagnostics = {"version": "9.9.9"}
    _append_support_draft(
        "draft-a",
        payload=_support_create_payload(diagnostics),
        diagnostics_snapshot=diagnostics,
    )
    _append_support_draft(
        "draft-b",
        payload=_support_create_payload(diagnostics),
        diagnostics_snapshot=diagnostics,
    )
    day = date.today().strftime("%Y%m%d")
    before_superseded = read_chat_events(day)

    response = chat_client.post(
        "/api/chat/support/draft/confirm",
        json={"draft_id": "draft-a"},
    )

    assert response.status_code == 200
    assert response.get_json() == {"ok": False, "outcome": "superseded"}
    assert calls == []
    events = read_chat_events(day)
    assert events == before_superseded
    assert not any(
        event["kind"] == "result" and event["draft_id"] == "draft-a" for event in events
    )
    assert not any(
        event["kind"] == "sol_message"
        and event["text"] == CHAT_SUPPORT_SUBMIT_FILED_FORMAT.format(ticket_id=123)
        for event in events
    )

    before = read_chat_events(day)
    missing = chat_client.post(
        "/api/chat/support/draft/confirm",
        json={"draft_id": "bogus-draft"},
    )
    assert missing.status_code == 200
    assert missing.get_json() == {"ok": False, "outcome": "not_found"}
    assert read_chat_events(day) == before


def test_support_draft_cancel_records_terminal_result_without_submit(
    chat_client, monkeypatch
):
    import solstone.convey.chat as chat

    monkeypatch.setattr(
        chat,
        "support_create",
        lambda **_kwargs: pytest.fail("support_create should not be called"),
    )
    monkeypatch.setattr(
        chat,
        "support_reply",
        lambda *_args: pytest.fail("support_reply should not be called"),
    )
    diagnostics = {"version": "9.9.9"}
    _append_support_draft(
        "draft-cancel",
        payload=_support_create_payload(diagnostics),
        diagnostics_snapshot=diagnostics,
    )

    response = chat_client.post(
        "/api/chat/support/draft/cancel",
        json={"draft_id": "draft-cancel"},
    )

    assert response.status_code == 200
    assert response.get_json() == {"ok": True, "outcome": "cancelled"}
    day = date.today().strftime("%Y%m%d")
    result = _events_of_kind(day, "result")[0]
    assert result["draft_id"] == "draft-cancel"
    assert result["ok"] is False
    assert result["cancelled"] is True
    sol_message = _events_of_kind(day, "sol_message")[0]
    assert sol_message["text"] == CHAT_SUPPORT_DRAFT_CANCELLED
    assert "draft" not in sol_message
    assert sol_message["sources"] == []
    assert sol_message["answer_state"] == "answered"
    assert _events_of_kind(day, "talent_spawned") == []
    assert chat._support_draft_state(day) == "submitted"


def test_support_draft_confirm_resolves_yesterday_draft(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    now = datetime.now()
    today_day = now.strftime("%Y%m%d")
    yesterday_dt = now - timedelta(days=1)
    yesterday_day = yesterday_dt.strftime("%Y%m%d")
    monkeypatch.setattr(chat, "_today_day", lambda: today_day)
    monkeypatch.setattr(chat, "support_create", lambda **_kwargs: {"id": 123})
    diagnostics = {"version": "9.9.9"}
    _append_support_draft(
        "draft-yesterday-confirm",
        payload=_support_create_payload(diagnostics),
        diagnostics_snapshot=diagnostics,
        captured_day=yesterday_day,
        ts=int(
            yesterday_dt.replace(hour=12, minute=0, second=0, microsecond=0).timestamp()
            * 1000
        ),
    )

    response = chat_client.post(
        "/api/chat/support/draft/confirm",
        json={"draft_id": "draft-yesterday-confirm"},
    )

    assert response.status_code == 200
    assert response.get_json()["outcome"] == "submitted"
    yesterday_results = _events_of_kind(yesterday_day, "result")
    assert len(yesterday_results) == 1
    assert yesterday_results[0]["draft_id"] == "draft-yesterday-confirm"
    assert _events_of_kind(today_day, "result") == []


def test_support_draft_cancel_resolves_yesterday_draft(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    now = datetime.now()
    today_day = now.strftime("%Y%m%d")
    yesterday_dt = now - timedelta(days=1)
    yesterday_day = yesterday_dt.strftime("%Y%m%d")
    monkeypatch.setattr(chat, "_today_day", lambda: today_day)
    monkeypatch.setattr(
        chat,
        "support_create",
        lambda **_kwargs: pytest.fail("support_create should not be called"),
    )
    diagnostics = {"version": "9.9.9"}
    _append_support_draft(
        "draft-yesterday-cancel",
        payload=_support_create_payload(diagnostics),
        diagnostics_snapshot=diagnostics,
        captured_day=yesterday_day,
        ts=int(
            yesterday_dt.replace(hour=12, minute=0, second=0, microsecond=0).timestamp()
            * 1000
        ),
    )

    response = chat_client.post(
        "/api/chat/support/draft/cancel",
        json={"draft_id": "draft-yesterday-cancel"},
    )

    assert response.status_code == 200
    assert response.get_json() == {"ok": True, "outcome": "cancelled"}
    yesterday_results = _events_of_kind(yesterday_day, "result")
    assert len(yesterday_results) == 1
    assert yesterday_results[0]["draft_id"] == "draft-yesterday-cancel"
    assert _events_of_kind(today_day, "result") == []


def test_sol_message_omits_thinking_when_not_emitted(chat_client, monkeypatch, caplog):
    import solstone.convey.chat as chat

    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )
    _set_current_chat(chat, "logical-chat", "raw-chat")

    with caplog.at_level(logging.WARNING, logger="solstone.convey.chat"):
        chat._on_cortex_finish(
            {
                "use_id": "raw-chat",
                "result": {
                    "message": "done",
                    "notes": "ok",
                    "talent_request": None,
                },
            }
        )

    sol_message = next(
        event
        for event in read_chat_events(date.today().strftime("%Y%m%d"))
        if event["kind"] == "sol_message"
    )
    assert "thinking" not in sol_message
    assert caplog.records == []


def test_sol_message_omits_offer_when_not_emitted(chat_client, monkeypatch, caplog):
    import solstone.convey.chat as chat

    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )
    _set_current_chat(chat, "logical-chat", "raw-chat")

    with caplog.at_level(logging.WARNING, logger="solstone.convey.chat"):
        chat._on_cortex_finish(
            {
                "use_id": "raw-chat",
                "result": {
                    "message": "done",
                    "notes": "ok",
                    "talent_request": None,
                },
            }
        )

    sol_message = next(
        event
        for event in read_chat_events(date.today().strftime("%Y%m%d"))
        if event["kind"] == "sol_message"
    )
    assert "offer" not in sol_message
    assert caplog.records == []


def test_empty_thinking_summary_is_suppressed(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )
    _set_current_chat(chat, "logical-chat", "raw-chat")

    chat._handle_callosum_message(
        {
            "tract": "cortex",
            "event": "thinking",
            "use_id": "raw-chat",
            "summary": "   \n",
        }
    )
    chat._on_cortex_finish(
        {
            "use_id": "raw-chat",
            "result": {
                "message": "done",
                "notes": "ok",
                "talent_request": None,
            },
        }
    )

    sol_message = next(
        event
        for event in read_chat_events(date.today().strftime("%Y%m%d"))
        if event["kind"] == "sol_message"
    )
    assert "thinking" not in sol_message


def test_thinking_buffers_are_isolated_and_cleared(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )
    _set_current_chat(chat, "logical-chat", "raw-chat")
    with chat._state_lock:
        chat._active_talents["talent-raw"] = {
            "chat_use_id": "logical-chat",
            "target": "exec",
            "task": "research",
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
        }

    chat._handle_callosum_message(
        {
            "tract": "cortex",
            "event": "thinking",
            "use_id": "raw-chat",
            "summary": "chat thought",
        }
    )
    chat._handle_callosum_message(
        {
            "tract": "cortex",
            "event": "thinking",
            "use_id": "talent-raw",
            "summary": "talent thought",
        }
    )
    chat._on_cortex_finish(
        {
            "use_id": "raw-chat",
            "result": {
                "message": "done",
                "notes": "ok",
                "talent_request": None,
            },
        }
    )

    sol_message = next(
        event
        for event in read_chat_events(date.today().strftime("%Y%m%d"))
        if event["kind"] == "sol_message"
    )
    assert sol_message["thinking"]["content"] == "chat thought"
    assert "raw-chat" not in chat._thinking_buffers
    assert chat._thinking_buffers["talent-raw"] == ["talent thought"]


def test_thinking_buffer_evicted_on_retry_rotation(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )
    monkeypatch.setattr("solstone.convey.chat._run_next_action", lambda _action: None)
    monkeypatch.setattr(
        "solstone.convey.chat._arm_watchdog_locked", lambda *_args, **_kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._cancel_watchdog_locked", lambda *_args, **_kwargs: None
    )
    _set_current_chat(chat, "logical-chat", "raw-old")
    chat._handle_callosum_message(
        {
            "tract": "cortex",
            "event": "thinking",
            "use_id": "raw-old",
            "summary": "old thought",
        }
    )

    chat._on_cortex_finish({"use_id": "raw-old", "result": "not json"})

    with chat._state_lock:
        raw_new = str(chat._current_chat_state["raw_use_id"])
    assert "raw-old" not in chat._thinking_buffers
    chat._handle_callosum_message(
        {
            "tract": "cortex",
            "event": "thinking",
            "use_id": raw_new,
            "summary": "new thought",
        }
    )
    chat._on_cortex_finish(
        {
            "use_id": raw_new,
            "result": {
                "message": "done",
                "notes": "ok",
                "talent_request": None,
            },
        }
    )

    sol_message = next(
        event
        for event in read_chat_events(date.today().strftime("%Y%m%d"))
        if event["kind"] == "sol_message"
    )
    assert sol_message["thinking"]["content"] == "new thought"


def test_thinking_buffer_clear_across_at_cap_defer(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )
    monkeypatch.setattr("solstone.convey.chat._run_next_action", lambda _action: None)
    monkeypatch.setattr(
        "solstone.convey.chat._active_talent_count_for_today_locked",
        lambda: chat.MAX_ACTIVE_TALENTS,
    )
    monkeypatch.setattr(
        "solstone.convey.chat._arm_watchdog_locked", lambda *_args, **_kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._cancel_watchdog_locked", lambda *_args, **_kwargs: None
    )
    _set_current_chat(chat, "logical-chat", "raw-old")
    chat._handle_callosum_message(
        {
            "tract": "cortex",
            "event": "thinking",
            "use_id": "raw-old",
            "summary": "old thought",
        }
    )

    chat._on_cortex_finish(
        {
            "use_id": "raw-old",
            "result": {
                "message": "checking",
                "notes": "ok",
                "talent_request": {
                    "target": "exec",
                    "task": "research",
                    "context": "{}",
                },
            },
        }
    )

    with chat._state_lock:
        assert chat._current_chat_state is None
        assert chat._current_chat_use_id is None
    assert "raw-old" not in chat._thinking_buffers
    events = read_chat_events(date.today().strftime("%Y%m%d"))
    sol_message = next(event for event in events if event["kind"] == "sol_message")
    assert sol_message["text"] == "checking"
    assert sol_message["requested_target"] == "exec"
    assert sol_message["requested_task"] == "research"
    assert sol_message["thinking"]["content"] == "old thought"
    queued = next(event for event in events if event["kind"] == "talent_queued")
    assert queued["name"] == "exec"
    assert queued["task"] == "research"
    assert queued["chat_use_id"] == "logical-chat"
    assert queued["ask"] == "help"
    assert queued["context"] == {}
    assert reduce_chat_state(date.today().strftime("%Y%m%d"))["queued_talents"] == [
        {
            "use_id": queued["use_id"],
            "name": "exec",
            "task": "research",
            "queued_at": queued["queued_at"],
        }
    ]


def test_late_thinking_arrival_drops_without_mutating_events(
    chat_client, monkeypatch, caplog
):
    import solstone.convey.chat as chat

    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )
    events_before = list(read_chat_events(date.today().strftime("%Y%m%d")))

    with caplog.at_level(logging.DEBUG, logger="solstone.convey.chat"):
        chat._handle_callosum_message(
            {
                "tract": "cortex",
                "event": "thinking",
                "use_id": "late-raw",
                "summary": "too late",
            }
        )

    assert "dropping late thinking event use_id=late-raw" in caplog.text
    assert read_chat_events(date.today().strftime("%Y%m%d")) == events_before
    assert chat._thinking_buffers == {}


def test_talent_error_evicts_but_does_not_attach_thinking(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )
    monkeypatch.setattr("solstone.convey.chat._run_next_action", lambda _action: None)
    _set_current_chat(chat, "logical-chat", None)
    with chat._state_lock:
        chat._active_talents["talent-raw"] = {
            "chat_use_id": "logical-chat",
            "target": "exec",
            "task": "research",
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "ask": "help",
        }
    chat._handle_callosum_message(
        {
            "tract": "cortex",
            "event": "thinking",
            "use_id": "talent-raw",
            "summary": "do not attach",
        }
    )

    chat._on_cortex_error({"use_id": "talent-raw", "error": "boom"})

    errored = next(
        event
        for event in read_chat_events(date.today().strftime("%Y%m%d"))
        if event["kind"] == "talent_errored"
    )
    assert "thinking" not in errored
    assert "talent-raw" not in chat._thinking_buffers


def test_chat_watchdog_timeout_evicts_thinking_buffers(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )
    monkeypatch.setattr("solstone.convey.chat._run_next_action", lambda _action: None)
    _set_current_chat(chat, "logical-chat", "raw-timeout")

    chat._handle_callosum_message(
        {
            "tract": "cortex",
            "event": "start",
            "use_id": "raw-timeout",
            "provider": "openai",
        }
    )
    chat._handle_callosum_message(
        {
            "tract": "cortex",
            "event": "thinking",
            "use_id": "raw-timeout",
            "summary": "thinking before timeout",
        }
    )

    assert chat._thinking_buffers["raw-timeout"] == ["thinking before timeout"]
    assert chat._thinking_providers["raw-timeout"] == "openai"

    chat._on_watchdog_timeout("raw-timeout", "chat", "logical-chat")

    assert "raw-timeout" not in chat._thinking_buffers
    assert "raw-timeout" not in chat._thinking_providers


def test_post_chat_appends_owner_message_and_returns_reserved_use_id(
    chat_client, monkeypatch
):
    import solstone.convey.chat as chat

    starts: list[dict] = []
    approvals: list[str | None] = []
    monkeypatch.setattr(
        "solstone.think.identity.ensure_identity_directory", lambda: None
    )

    def fake_spawn(action):
        starts.append(action)
        with chat._state_lock:
            approvals.append(chat._current_chat_state.get("outbound_approval"))
        return ChatSpawnResult(ok=True)

    monkeypatch.setattr(
        "solstone.convey.chat._spawn_chat_generate",
        fake_spawn,
    )

    response = chat_client.post(
        "/api/chat",
        json={
            "message": "hello there",
            "app": "sol",
            "path": "/app/sol",
            "facet": "work",
        },
    )

    assert response.status_code == 200
    payload = response.get_json()
    assert payload["queued"] is False
    assert payload["use_id"].isdigit()
    assert starts and starts[-1]["logical_use_id"] == payload["use_id"]
    assert approvals == [None]


def test_post_chat_dispatches_queued_messages_fifo(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    starts: list[dict] = []
    approvals: list[str | None] = []
    monkeypatch.setattr(
        "solstone.think.identity.ensure_identity_directory", lambda: None
    )

    def fake_spawn(action):
        starts.append(action)
        with chat._state_lock:
            approvals.append(chat._current_chat_state.get("outbound_approval"))
        return ChatSpawnResult(ok=True)

    monkeypatch.setattr(
        "solstone.convey.chat._spawn_chat_generate",
        fake_spawn,
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )

    responses = [_post_chat_message(chat_client, f"msg {idx}") for idx in range(5)]

    assert [response.status_code for response in responses] == [200] * 5
    assert [response.get_json()["queued"] for response in responses] == [
        False,
        True,
        True,
        True,
        True,
    ]
    assert len(starts) == 1

    index = 0
    while index < len(starts):
        action = starts[index]
        message = action["trigger"]["message"]
        chat._on_cortex_finish(
            {
                "use_id": action["raw_use_id"],
                "result": json.dumps(
                    {
                        "message": f"reply {message}",
                        "notes": "ok",
                        "talent_request": None,
                    }
                ),
            }
        )
        index += 1

    assert [action["trigger"]["message"] for action in starts] == [
        "msg 0",
        "msg 1",
        "msg 2",
        "msg 3",
        "msg 4",
    ]
    assert len(approvals) == 5
    assert approvals == [None] * 5
    events = read_chat_events(date.today().strftime("%Y%m%d"))
    replies = [event["text"] for event in events if event["kind"] == "sol_message"]
    assert replies == [
        "reply msg 0",
        "reply msg 1",
        "reply msg 2",
        "reply msg 3",
        "reply msg 4",
    ]


def test_post_chat_rejects_when_queue_depth_cap_reached(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    monkeypatch.setattr(
        "solstone.think.identity.ensure_identity_directory", lambda: None
    )
    with chat._state_lock:
        chat._current_chat_use_id = "current"
        chat._current_chat_state = {
            "raw_use_id": "raw-current",
            "raw_use_ids_seen": {"raw-current"},
            "trigger": {"type": "owner_message", "message": "busy"},
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "retry_count": 0,
        }
        for index in range(10):
            chat._queued_triggers.append(
                {
                    "use_id": str(index + 1),
                    "trigger": {"type": "owner_message", "message": f"queued {index}"},
                    "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
                }
            )

    response = _post_chat_message(chat_client, "one too many")

    assert response.status_code == 429
    assert response.get_json() == {
        "error": "Chat queue full",
        "reason_code": "chat_queue_full",
        "detail": "",
        "queue_depth": 10,
    }
    events = read_chat_events(date.today().strftime("%Y%m%d"))
    assert [event for event in events if event["kind"] == "owner_message"] == []
    assert [event for event in events if event["kind"] == "chat_queue_depth"] == []


def test_chat_error_starts_next_queued_message(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    starts: list[dict] = []
    monkeypatch.setattr(
        "solstone.think.identity.ensure_identity_directory", lambda: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._spawn_chat_generate",
        lambda action: starts.append(action) or ChatSpawnResult(ok=True),
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )

    assert _post_chat_message(chat_client, "first").status_code == 200
    assert _post_chat_message(chat_client, "second").status_code == 200
    assert len(starts) == 1

    chat._handle_chat_failure(starts[0]["logical_use_id"], "unknown")

    assert [action["trigger"]["message"] for action in starts] == [
        "first",
        "second",
    ]
    errors = [
        event
        for event in read_chat_events(date.today().strftime("%Y%m%d"))
        if event["kind"] == "chat_error"
    ]
    assert errors[-1]["use_id"] == starts[0]["logical_use_id"]
    assert errors[-1]["reason"] == "unknown"


def test_queue_depth_events_emit_on_enqueue_and_dequeue(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    starts: list[dict] = []
    monkeypatch.setattr(
        "solstone.think.identity.ensure_identity_directory", lambda: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._spawn_chat_generate",
        lambda action: starts.append(action) or ChatSpawnResult(ok=True),
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *_args, **_kwargs: None
    )

    for message in ("first", "second", "third"):
        assert _post_chat_message(chat_client, message).status_code == 200

    for index in range(2):
        action = starts[index]
        chat._on_cortex_finish(
            {
                "use_id": action["raw_use_id"],
                "result": {
                    "message": f"reply {action['trigger']['message']}",
                    "notes": "ok",
                    "talent_request": None,
                },
            }
        )

    events = read_chat_events(date.today().strftime("%Y%m%d"))
    depths = [event["depth"] for event in events if event["kind"] == "chat_queue_depth"]
    assert depths == [1, 2, 1, 0]


def test_handle_chat_failure_threads_pipeline_unavailable(chat_client, monkeypatch):
    monkeypatch.setattr(
        "solstone.think.identity.ensure_identity_directory", lambda: None
    )

    def fail_spawn(*_args, **_kwargs):
        raise CortexSpawnUnavailable(detail="FileNotFoundError")

    monkeypatch.setattr("solstone.convey.utils.spawn_agent", fail_spawn)

    response = chat_client.post(
        "/api/chat",
        json={
            "message": "hello there",
            "app": "sol",
            "path": "/app/sol",
            "facet": "work",
        },
    )

    assert response.status_code != 200
    errors = [
        event
        for event in read_chat_events(date.today().strftime("%Y%m%d"))
        if event["kind"] == "chat_error"
    ]
    assert errors[-1]["reason"] == "chat_pipeline_unavailable"
    assert errors[-1]["detail"] == "FileNotFoundError"


def test_chat_event_error_persists_and_emits_detail(tmp_path, monkeypatch):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)
    emitted: list[tuple[str, dict]] = []
    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event",
        lambda event, **fields: emitted.append((event, fields)),
    )

    chat._handle_chat_failure(
        "1713626000000",
        "chat_pipeline_unavailable",
        detail=" FileNotFoundError \n",
    )

    errors = [
        event
        for event in read_chat_events(date.today().strftime("%Y%m%d"))
        if event["kind"] == "chat_error"
    ]
    assert errors[-1]["reason"] == "chat_pipeline_unavailable"
    assert errors[-1]["detail"] == "FileNotFoundError"
    assert emitted == [
        (
            "error",
            {
                "use_id": "1713626000000",
                "error": "chat_pipeline_unavailable",
                "provider": "",
                "detail": "FileNotFoundError",
                "chat_proxy": True,
            },
        )
    ]


def test_session_endpoint_reduces_from_chat_stream(chat_client, monkeypatch):
    day = "20260420"
    monkeypatch.setattr("solstone.convey.chat._today_day", lambda: day)
    started_at = _ms(2026, 4, 20, 12, 1, 0)
    finished_at = _ms(2026, 4, 20, 12, 2, 0)
    append_chat_event(
        "sol_message",
        ts=_ms(2026, 4, 20, 12, 0, 0),
        use_id="1713626000000",
        text="hello",
        notes="ready",
        requested_target=None,
        requested_task=None,
    )
    append_chat_event(
        "talent_spawned",
        ts=started_at,
        use_id="1713626000001",
        name="exec",
        task="research",
        started_at=started_at,
    )
    append_chat_event(
        "talent_finished",
        ts=finished_at,
        use_id="1713626000001",
        name="exec",
        summary="done",
    )

    response = chat_client.get("/api/chat/session")
    assert response.status_code == 200
    payload = response.get_json()
    assert payload["latest_sol_message"]["text"] == "hello"
    assert payload["latest_sol_message"]["sources"] == []
    assert payload["latest_sol_message"]["answer_state"] == "answered"
    assert payload["active_talents"] == []
    assert payload["completed_talents"] == [
        {
            "finished_at": finished_at,
            "label": talent_label_for("exec", "finished"),
            "name": "exec",
            "summary": "done",
            "task": "research",
            "use_id": "1713626000001",
        }
    ]
    assert payload["errored_talents"] == []
    assert payload["chat_error"] is None


def test_chat_session_retries_unresolved_trigger_when_idle(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    day = "20260420"
    monkeypatch.setattr("solstone.convey.chat._today_day", lambda: day)
    append_chat_event(
        "owner_message",
        ts=_ms(2026, 4, 20, 12, 0, 0),
        text="retry me",
        app="sol",
        path="/app/sol",
        facet="work",
    )

    starts: list[dict] = []

    approvals: list[str | None] = []

    def fake_spawn(action):
        starts.append(action)
        with chat._state_lock:
            approvals.append(chat._current_chat_state.get("outbound_approval"))
        return ChatSpawnResult(ok=True)

    monkeypatch.setattr(
        "solstone.convey.chat._spawn_chat_generate",
        fake_spawn,
    )

    response = chat_client.get("/api/chat/session")

    assert response.status_code == 200
    assert len(starts) == 1
    assert starts[0]["trigger"]["type"] == "owner_message"
    assert approvals == [None]


def test_chat_session_reconstructs_origin_for_unresponded_terminal(
    chat_client,
    monkeypatch,
):

    day = "20260420"
    monkeypatch.setattr("solstone.convey.chat._today_day", lambda: day)
    start = _ms(2026, 4, 20, 12, 0, 0)
    append_chat_event(
        "owner_message",
        ts=start,
        text="look this up",
        app="sol",
        path="/app/sol",
        facet="work",
    )
    append_chat_event(
        "sol_message",
        ts=start + 1_000,
        use_id="dispatch-chat",
        text="working",
        notes="",
        requested_target="exec",
        requested_task="research",
    )
    append_chat_event(
        "talent_spawned",
        ts=start + 2_000,
        use_id="talent-raw",
        name="exec",
        task="research",
        started_at=start + 2_000,
    )
    append_chat_event(
        "talent_finished",
        ts=start + 3_000,
        use_id="talent-raw",
        name="exec",
        summary="done",
    )

    starts: list[dict] = []
    monkeypatch.setattr(
        "solstone.convey.chat._spawn_chat_generate",
        lambda action: starts.append(action) or ChatSpawnResult(ok=True),
    )

    response = chat_client.get("/api/chat/session")

    assert response.status_code == 200
    assert len(starts) == 1
    assert starts[0]["trigger"]["type"] == "talent_finished"
    assert starts[0]["trigger"]["origin"] == {
        "logical_use_id": "dispatch-chat",
        "ask": "look this up",
    }


def test_chat_session_retries_again_when_spawn_fails_and_trigger_remains_unresolved(
    chat_client, monkeypatch
):
    day = "20260420"
    monkeypatch.setattr("solstone.convey.chat._today_day", lambda: day)
    append_chat_event(
        "owner_message",
        ts=_ms(2026, 4, 20, 12, 0, 0),
        text="retry me again",
        app="sol",
        path="/app/sol",
        facet="work",
    )

    starts: list[dict] = []

    def fake_spawn(action):
        starts.append(action)
        if len(starts) > 1:
            return ChatSpawnResult(ok=True)
        return ChatSpawnResult(ok=False, reason="unknown")

    monkeypatch.setattr("solstone.convey.chat._spawn_chat_generate", fake_spawn)
    monkeypatch.setattr(
        "solstone.convey.chat._emit_error", lambda *_args, **_kwargs: None
    )

    first = chat_client.get("/api/chat/session")
    second = chat_client.get("/api/chat/session")

    assert first.status_code == 200
    assert second.status_code == 200
    assert len(starts) == 2
    assert starts[0]["trigger"]["type"] == "owner_message"
    assert starts[1]["trigger"]["type"] == "owner_message"


def test_talent_log_endpoint_returns_completed_run(chat_client, tmp_path):
    use_id = "1700000000001"
    _write_talent_log(
        tmp_path / "journal",
        "default",
        f"{use_id}.jsonl",
        [
            {
                "event": "request",
                "ts": 1700000000001,
                "use_id": use_id,
                "prompt": "Search for meetings about project updates",
                "name": "default",
                "provider": "openai",
            },
            {
                "event": "start",
                "ts": 1700000000100,
                "use_id": use_id,
                "model": "gpt-4o",
                "provider": "openai",
            },
            {
                "event": "thinking",
                "ts": 1700000000300,
                "use_id": use_id,
                "content": "reasoning",
                "raw": {"provider": "openai"},
            },
            {
                "event": "finish",
                "ts": 1700000000600,
                "use_id": use_id,
                "result": "done",
            },
        ],
    )

    response = chat_client.get(f"/api/chat/talent-log/{use_id}")

    assert response.status_code == 200
    payload = response.get_json()
    assert payload["use_id"] == use_id
    assert payload["status"] == "completed"
    assert payload["task"] == "Search for meetings about project updates"
    assert payload["started_at"] == 1700000000100
    assert payload["finished_at"] == 1700000000600
    assert len(payload["events"]) == 3
    assert payload["events"][1]["event"] == "thinking"
    assert "raw" not in payload["events"][1]


def test_talent_log_endpoint_omits_non_responsive_raw_output(chat_client, tmp_path):
    use_id = "1700000000010"
    raw_output = "I cannot describe this screen."
    _write_talent_log(
        tmp_path / "journal",
        "default",
        f"{use_id}.jsonl",
        [
            {
                "event": "request",
                "ts": 1700000000010,
                "use_id": use_id,
                "prompt": "Describe the screen",
                "name": "default",
                "provider": "openai",
            },
            {
                "event": "error",
                "ts": 1700000000100,
                "use_id": use_id,
                "error": "The requested work was not completed.",
                "reason_code": "non_responsive",
                "terminal": True,
                "raw": [
                    {
                        "reason_code": "non_responsive",
                        "non_responsive_output": raw_output,
                    }
                ],
            },
        ],
    )

    response = chat_client.get(f"/api/chat/talent-log/{use_id}")

    assert response.status_code == 200
    serialized = json.dumps(response.get_json())
    assert raw_output not in serialized
    assert '"raw"' not in serialized


def test_talent_log_endpoint_returns_running_active_run(chat_client, tmp_path):
    use_id = "1700000000002"
    _write_talent_log(
        tmp_path / "journal",
        "default",
        f"{use_id}_active.jsonl",
        [
            {
                "event": "request",
                "ts": 1700000000002,
                "use_id": use_id,
                "task": "Analyze conversation flow",
            },
            {
                "event": "start",
                "ts": 1700000000102,
                "use_id": use_id,
                "model": "gpt-4o-mini",
            },
            {
                "event": "thinking",
                "ts": 1700000000202,
                "use_id": use_id,
                "content": "still working",
            },
        ],
    )

    response = chat_client.get(f"/api/chat/talent-log/{use_id}")

    assert response.status_code == 200
    payload = response.get_json()
    assert payload["status"] == "running"
    assert payload["task"] == "Analyze conversation flow"
    assert payload["finished_at"] is None
    assert payload["events"][-1]["event"] == "thinking"


def test_talent_log_endpoint_prefers_active_log(chat_client, tmp_path):
    use_id = "1700000000003"
    journal = tmp_path / "journal"
    _write_talent_log(
        journal,
        "default",
        f"{use_id}_active.jsonl",
        [
            {
                "event": "request",
                "ts": 1700000000003,
                "use_id": use_id,
                "prompt": "active prompt",
            },
            {
                "event": "thinking",
                "ts": 1700000000103,
                "use_id": use_id,
                "content": "active content",
            },
        ],
    )
    _write_talent_log(
        journal,
        "flow",
        f"{use_id}.jsonl",
        [
            {
                "event": "request",
                "ts": 1700000000003,
                "use_id": use_id,
                "prompt": "completed prompt",
            },
            {
                "event": "finish",
                "ts": 1700000000203,
                "use_id": use_id,
                "result": "completed result",
            },
        ],
    )

    response = chat_client.get(f"/api/chat/talent-log/{use_id}")

    assert response.status_code == 200
    payload = response.get_json()
    assert payload["status"] == "running"
    assert payload["task"] == "active prompt"
    assert payload["events"][0]["content"] == "active content"


def test_talent_log_endpoint_returns_errored_run(chat_client, tmp_path):
    use_id = "1700000000004"
    _write_talent_log(
        tmp_path / "journal",
        "flow",
        f"{use_id}.jsonl",
        [
            {
                "event": "request",
                "ts": 1700000000004,
                "use_id": use_id,
                "prompt": "Analyze flow",
            },
            {
                "event": "error",
                "ts": 1700000000204,
                "use_id": use_id,
                "error": "Rate limit exceeded",
            },
        ],
    )

    response = chat_client.get(f"/api/chat/talent-log/{use_id}")

    assert response.status_code == 200
    payload = response.get_json()
    assert payload["status"] == "errored"
    assert payload["finished_at"] == 1700000000204
    assert payload["events"][-1]["event"] == "error"


def test_talent_log_endpoint_returns_missing(chat_client):
    use_id = "1700000000999"

    response = chat_client.get(f"/api/chat/talent-log/{use_id}")

    assert response.status_code == 404
    payload = response.get_json()
    assert payload["error"] == "I couldn't find that talent run."
    assert payload["reason_code"] == "talent_not_found"
    assert payload["detail"] == f"Talent log not found for use_id {use_id}"


def test_talent_log_endpoint_task_falls_back_to_prompt(chat_client, tmp_path):
    use_id = "1700000000005"
    _write_talent_log(
        tmp_path / "journal",
        "default",
        f"{use_id}.jsonl",
        [
            {
                "event": "request",
                "ts": 1700000000005,
                "use_id": use_id,
                "prompt": "Fallback prompt",
            },
            {
                "event": "finish",
                "ts": 1700000000305,
                "use_id": use_id,
                "result": "done",
            },
        ],
    )

    response = chat_client.get(f"/api/chat/talent-log/{use_id}")

    assert response.status_code == 200
    assert response.get_json()["task"] == "Fallback prompt"


def test_support_draft_retries_after_transient_failure(chat_client, monkeypatch):
    import solstone.convey.chat as chat

    calls = 0

    def flaky_support_create(**_kwargs):
        nonlocal calls
        calls += 1
        if calls == 1:
            raise RuntimeError("temporary failure")
        return {"id": 123}

    monkeypatch.setattr(chat, "support_create", flaky_support_create)
    _append_support_draft("draft-retry", diagnostics_snapshot={})

    first = chat_client.post(
        "/api/chat/support/draft/confirm", json={"draft_id": "draft-retry"}
    )
    second = chat_client.post(
        "/api/chat/support/draft/confirm", json={"draft_id": "draft-retry"}
    )

    assert first.get_json() == {"ok": False, "outcome": "failed"}
    assert second.get_json() == {
        "ok": True,
        "outcome": "submitted",
        "ticket_id": 123,
    }
    day = date.today().strftime("%Y%m%d")
    claims = _events_of_kind(day, "support_submit_claim")
    assert [claim["generation"] for claim in claims] == [1, 2]
    assert len(_events_of_kind(day, "result")) == 1


def test_legacy_submit_claim_remains_terminal(chat_client):
    draft_id = "legacy-claim"
    _append_support_draft(draft_id, diagnostics_snapshot={})
    append_chat_event("support_submit_claim", draft_id=draft_id)

    confirm = chat_client.post(
        "/api/chat/support/draft/confirm", json={"draft_id": draft_id}
    )
    cancel = chat_client.post(
        "/api/chat/support/draft/cancel", json={"draft_id": draft_id}
    )

    assert confirm.get_json() == {"ok": False, "outcome": "already_submitted"}
    assert cancel.get_json() == {"ok": False, "outcome": "already_submitted"}


@pytest.mark.parametrize(
    ("verb", "tool_name", "copy_text"),
    [
        ("close", "support_close", CHAT_SUPPORT_CLOSE_SUBMITTED),
        ("resolved", "support_resolved", CHAT_SUPPORT_RESOLVED_SUBMITTED),
        (
            "still_need_help",
            "support_still_need_help",
            CHAT_SUPPORT_STILL_NEED_HELP_SUBMITTED,
        ),
    ],
)
def test_support_draft_lifecycle_verbs_submit(
    chat_client, monkeypatch, verb, tool_name, copy_text
):
    import solstone.convey.chat as chat

    calls: list[tuple[int, str]] = []

    def tool(ticket_id, *, action_id):
        calls.append((ticket_id, action_id))
        return {"ticket_id": ticket_id}

    monkeypatch.setattr(chat, tool_name, tool)
    draft_id = f"draft-{verb}"
    _append_support_draft(
        draft_id,
        verb=verb,
        payload={"ticket_id": 77},
        diagnostics_snapshot=None,
    )

    response = chat_client.post(
        "/api/chat/support/draft/confirm", json={"draft_id": draft_id}
    )

    assert response.get_json() == {
        "ok": True,
        "outcome": "submitted",
        "ticket_id": 77,
    }
    assert calls == [(77, draft_id)]
    assert _events_of_kind(date.today().strftime("%Y%m%d"), "sol_message")[-1][
        "text"
    ] == copy_text


@pytest.mark.parametrize(
    ("error_type", "outcome", "copy_text"),
    [
        (
            "OperationInProgressError",
            "in_progress",
            CHAT_SUPPORT_IN_PROGRESS,
        ),
        (
            "OperationTosChangedError",
            "re_consent_required",
            CHAT_SUPPORT_RECONSENT_NEEDED,
        ),
    ],
)
def test_retryable_operation_errors_leave_draft_open(
    chat_client, monkeypatch, error_type, outcome, copy_text
):
    import solstone.convey.chat as chat
    from solstone.apps.support import operations

    calls = 0

    def support_create_once(**_kwargs):
        nonlocal calls
        calls += 1
        if calls == 1:
            raise getattr(operations, error_type)()
        return {"id": 456}

    monkeypatch.setattr(chat, "support_create", support_create_once)
    draft_id = f"draft-{outcome}"
    _append_support_draft(draft_id, diagnostics_snapshot={})

    first = chat_client.post(
        "/api/chat/support/draft/confirm", json={"draft_id": draft_id}
    )
    second = chat_client.post(
        "/api/chat/support/draft/confirm", json={"draft_id": draft_id}
    )

    assert first.get_json() == {"ok": False, "outcome": outcome}
    assert second.get_json()["outcome"] == "submitted"
    day = date.today().strftime("%Y%m%d")
    assert _events_of_kind(day, "sol_message")[0]["text"] == copy_text
    assert len(_events_of_kind(day, "result")) == 1


def test_superseded_generation_returns_existing_result_without_new_terminal_events(
    chat_client, monkeypatch
):
    import solstone.convey.chat as chat
    from solstone.apps.support.operations import OperationSupersededError

    draft_id = "draft-superseded-generation"
    _append_support_draft(draft_id, diagnostics_snapshot={})

    def resolved_elsewhere(_draft_event, _draft_id):
        append_chat_event("result", draft_id=draft_id, ok=True, ticket_id=99)
        raise OperationSupersededError()

    monkeypatch.setattr(chat, "_submit_support_draft", resolved_elsewhere)
    response = chat_client.post(
        "/api/chat/support/draft/confirm", json={"draft_id": draft_id}
    )

    assert response.get_json() == {
        "ok": True,
        "outcome": "submitted",
        "ticket_id": 99,
    }
    day = date.today().strftime("%Y%m%d")
    assert len(_events_of_kind(day, "support_submit_claim")) == 1
    assert len(_events_of_kind(day, "result")) == 1
    assert _events_of_kind(day, "sol_message") == []


def test_support_draft_index_resolves_old_drafts_and_preserves_legacy_boundary(
    chat_client,
):
    import solstone.convey.chat as chat
    import solstone.convey.chat_stream as chat_stream

    old = datetime.now() - timedelta(days=3)
    old_day = old.strftime("%Y%m%d")
    indexed_id = "indexed-old-draft"
    _append_support_draft(
        indexed_id,
        captured_day=old_day,
        diagnostics_snapshot={},
        ts=int(old.replace(hour=12, minute=0, second=0, microsecond=0).timestamp() * 1000),
    )
    chat_stream.record_draft_captured(indexed_id, old_day)
    chat_stream.record_draft_captured(indexed_id, old_day)

    assert chat_stream.resolve_draft_day(indexed_id) == old_day
    assert chat._resolve_support_draft(indexed_id) is not None

    legacy_id = "unindexed-old-draft"
    _append_support_draft(
        legacy_id,
        captured_day=old_day,
        diagnostics_snapshot={},
        ts=int(old.replace(hour=13, minute=0, second=0, microsecond=0).timestamp() * 1000),
    )
    assert chat_stream.resolve_draft_day(legacy_id) is None
    assert chat._resolve_support_draft(legacy_id) is None
