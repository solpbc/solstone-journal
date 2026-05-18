# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import logging
from datetime import datetime

import pytest
from flask import Flask

from solstone.convey.chat_stream import append_chat_event, read_chat_events


def _reset_chat_state(chat_module) -> None:
    chat_module.stop_all_chat_runtime()
    with chat_module._state_lock:
        chat_module._current_chat_use_id = None
        chat_module._current_chat_state = None
        chat_module._queued_trigger = None
        chat_module._active_talents.clear()
        chat_module._reserved_use_ids.clear()
        for timer in chat_module._watchdog_timers.values():
            timer.cancel()
        chat_module._watchdog_timers.clear()
        chat_module._last_use_id = 0


def _setup_journal(tmp_path, monkeypatch):
    journal = tmp_path / "journal"
    journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    return journal


def _ms(year: int, month: int, day: int, hour: int, minute: int, second: int) -> int:
    return int(datetime(year, month, day, hour, minute, second).timestamp() * 1000)


def _install_fake_timers(monkeypatch):
    timers: list[FakeTimer] = []

    class FakeTimer:
        def __init__(self, interval, function, args=None, kwargs=None):
            self.interval = interval
            self.function = function
            self.args = tuple(args or ())
            self.kwargs = dict(kwargs or {})
            self.started = False
            self.cancelled = False
            self.daemon = False
            timers.append(self)

        def start(self) -> None:
            self.started = True

        def cancel(self) -> None:
            self.cancelled = True

        def fire(self) -> None:
            if self.cancelled:
                return
            self.function(*self.args, **self.kwargs)

    monkeypatch.setattr("solstone.convey.chat.threading.Timer", FakeTimer)
    return timers


def _append_recoverable_talent_events(
    chat_use_id: str,
    talent_use_id: str,
    *,
    target: str = "exec",
    task: str = "research it",
) -> None:
    now = datetime.now()
    start = _ms(now.year, now.month, now.day, 12, 0, 0)
    append_chat_event(
        "owner_message",
        ts=start,
        text="Help me with this",
        app="home",
        path="/app/home",
        facet="work",
    )
    append_chat_event(
        "sol_message",
        ts=start + 1,
        use_id=chat_use_id,
        text="I am looking into that.",
        notes="need exec",
        requested_target=target,
        requested_task=task,
    )
    append_chat_event(
        "talent_spawned",
        ts=start + 1_000,
        use_id=talent_use_id,
        name=target,
        task=task,
        started_at=start + 1_000,
    )


def test_chat_result_with_two_active_talents_retriggers_with_max_active_reason(
    tmp_path, monkeypatch
):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)

    append_chat_event(
        "talent_spawned",
        use_id="1713620000001",
        name="exec",
        task="first task",
        started_at=1713620000001,
    )
    append_chat_event(
        "talent_spawned",
        use_id="1713620000002",
        name="exec",
        task="second task",
        started_at=1713620000002,
    )

    actions: list[dict] = []
    monkeypatch.setattr(
        "solstone.convey.chat._run_next_action", lambda action: actions.append(action)
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_finish", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_error", lambda *args, **kwargs: None
    )

    with chat._state_lock:
        chat._current_chat_use_id = "1713620000100"
        chat._current_chat_state = {
            "raw_use_id": "1713620000101",
            "raw_use_ids_seen": {"1713620000101"},
            "trigger": {"type": "owner_message", "message": "help"},
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "retry_count": 0,
        }

    chat._on_cortex_finish(
        {
            "use_id": "1713620000101",
            "result": json.dumps(
                {
                    "message": "I am looking into that.",
                    "notes": "need exec",
                    "talent_request": {
                        "target": "exec",
                        "task": "research it",
                        "context": json.dumps({"k": "v"}),
                    },
                }
            ),
        }
    )

    assert actions
    assert actions[-1]["kind"] == "chat"
    assert actions[-1]["trigger"] == {
        "type": "synthetic-max-active",
        "reason": "max active — waiting for one to finish",
    }

    sol_messages = [
        e for e in read_chat_events(chat._today_day()) if e["kind"] == "sol_message"
    ]
    assert sol_messages[-1]["requested_target"] == "exec"
    assert sol_messages[-1]["requested_task"] == "research it"


def test_exec_retrigger_loop_stops_after_three_without_owner_reset(
    tmp_path, monkeypatch
):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)

    append_chat_event(
        "owner_message",
        text="dig deeper",
        app="sol",
        path="/app/sol",
        facet="work",
    )
    for index in range(3):
        append_chat_event(
            "talent_finished",
            use_id=f"171362100000{index}",
            name="exec",
            summary=f"summary {index}",
        )
        if index < 2:
            append_chat_event(
                "sol_message",
                use_id="1713621999999",
                text=f"follow up {index}",
                notes="retrying",
                requested_target="exec",
                requested_task=f"task {index}",
            )

    emitted_errors: list[tuple[str, str]] = []
    actions: list[dict | None] = []
    monkeypatch.setattr(
        "solstone.convey.chat._run_next_action", lambda action: actions.append(action)
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_finish", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_error",
        lambda use_id, reason: emitted_errors.append((use_id, reason)),
    )

    with chat._state_lock:
        chat._current_chat_use_id = "1713621999999"
        chat._current_chat_state = {
            "raw_use_id": "1713622000000",
            "raw_use_ids_seen": {"1713622000000"},
            "trigger": {"type": "talent_finished", "summary": "summary 2"},
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "retry_count": 0,
        }

    chat._on_cortex_finish(
        {
            "use_id": "1713622000000",
            "result": json.dumps(
                {
                    "message": "Still digging.",
                    "notes": "loop",
                    "talent_request": {
                        "target": "exec",
                        "task": "one more pass",
                        "context": json.dumps({}),
                    },
                }
            ),
        }
    )

    assert emitted_errors == [("1713621999999", "provider_response_invalid")]
    assert actions == [None]
    errors = [
        e for e in read_chat_events(chat._today_day()) if e["kind"] == "chat_error"
    ]
    assert errors[-1]["reason"] == "provider_response_invalid"


def test_talent_loop_count_skips_chat_error_between_retry_hops(tmp_path, monkeypatch):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)

    append_chat_event(
        "owner_message",
        text="dig deeper",
        app="sol",
        path="/app/sol",
        facet="work",
    )
    append_chat_event(
        "sol_message",
        use_id="1713622100000",
        text="follow up 0",
        notes="retrying",
        requested_target="exec",
        requested_task="task 0",
    )
    append_chat_event(
        "talent_finished",
        use_id="1713622100001",
        name="exec",
        summary="summary 0",
    )
    append_chat_event(
        "chat_error",
        use_id="1713622100000",
        reason="transient trouble",
    )
    append_chat_event(
        "sol_message",
        use_id="1713622100000",
        text="follow up 1",
        notes="retrying",
        requested_target="exec",
        requested_task="task 1",
    )
    append_chat_event(
        "talent_finished",
        use_id="1713622100002",
        name="exec",
        summary="summary 1",
    )
    append_chat_event(
        "sol_message",
        use_id="1713622100000",
        text="follow up 2",
        notes="retrying",
        requested_target="exec",
        requested_task="task 2",
    )

    with chat._state_lock:
        assert chat._talent_loop_count_locked() == 2


def test_talent_loop_count_counts_through_talent_errored_and_reflection_ready(
    tmp_path, monkeypatch
):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)

    append_chat_event(
        "owner_message",
        text="dig deeper",
        app="sol",
        path="/app/sol",
        facet="work",
    )
    append_chat_event(
        "sol_message",
        use_id="1713622200000",
        text="follow up 0",
        notes="retrying",
        requested_target="exec",
        requested_task="task 0",
    )
    append_chat_event(
        "talent_errored",
        use_id="1713622200001",
        name="exec",
        reason="needs clarification",
    )
    append_chat_event(
        "reflection_ready",
        day=chat._today_day(),
        url="/app/chat/today",
    )
    append_chat_event(
        "sol_message",
        use_id="1713622200000",
        text="follow up 1",
        notes="retrying",
        requested_target="exec",
        requested_task="task 1",
    )
    append_chat_event(
        "talent_finished",
        use_id="1713622200002",
        name="exec",
        summary="summary 1",
    )
    append_chat_event(
        "reflection_ready",
        day=chat._today_day(),
        url="/app/chat/today#latest",
    )
    append_chat_event(
        "sol_message",
        use_id="1713622200000",
        text="follow up 2",
        notes="retrying",
        requested_target="exec",
        requested_task="task 2",
    )

    with chat._state_lock:
        assert chat._talent_loop_count_locked() == 2


def test_cortex_finish_and_error_append_exec_terminal_events_by_use_id(
    tmp_path, monkeypatch
):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)

    actions: list[dict] = []
    monkeypatch.setattr(
        "solstone.convey.chat._run_next_action", lambda action: actions.append(action)
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_finish", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_error", lambda *args, **kwargs: None
    )

    with chat._state_lock:
        chat._current_chat_use_id = "1713623000000"
        chat._current_chat_state = {
            "raw_use_id": None,
            "raw_use_ids_seen": set(),
            "trigger": {"type": "owner_message", "message": "help"},
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "retry_count": 0,
        }
        chat._active_talents["1713623000001"] = {
            "chat_use_id": "1713623000000",
            "target": "exec",
            "task": "summarize",
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
        }

    chat._on_cortex_finish({"use_id": "1713623000001", "result": "done"})
    finished_events = [
        e for e in read_chat_events(chat._today_day()) if e["kind"] == "talent_finished"
    ]
    assert finished_events[-1]["use_id"] == "1713623000001"
    assert actions[-1]["trigger"]["type"] == "talent_finished"

    _reset_chat_state(chat)
    actions.clear()
    with chat._state_lock:
        chat._current_chat_use_id = "1713624000000"
        chat._current_chat_state = {
            "raw_use_id": None,
            "raw_use_ids_seen": set(),
            "trigger": {"type": "owner_message", "message": "help"},
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "retry_count": 0,
        }
        chat._active_talents["1713624000001"] = {
            "chat_use_id": "1713624000000",
            "target": "exec",
            "task": "summarize",
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
        }

    chat._on_cortex_error({"use_id": "1713624000001", "error": "boom"})
    errored_events = [
        e for e in read_chat_events(chat._today_day()) if e["kind"] == "talent_errored"
    ]
    assert errored_events[-1]["use_id"] == "1713624000001"
    assert actions[-1]["trigger"]["type"] == "talent_errored"
    assert actions[-1]["trigger"]["reason"] == "boom"


@pytest.mark.parametrize(
    ("terminal_kind", "result_field_name", "result_field_label", "result_value"),
    [
        ("talent_finished", "summary", "result", "Found the answer."),
        ("talent_errored", "reason", "reason", "The lookup failed."),
    ],
)
def test_terminal_talent_reports_back_without_redispatch(
    tmp_path,
    monkeypatch,
    terminal_kind,
    result_field_name,
    result_field_label,
    result_value,
):
    import solstone.convey.chat as chat
    from solstone.talent import chat_context

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)
    _install_fake_timers(monkeypatch)

    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *args, **kwargs: None
    )
    monkeypatch.setattr("solstone.think.routines.get_routine_state", lambda: [])
    monkeypatch.setattr(
        "solstone.think.routines.get_config",
        lambda: {"_meta": {"suggestions_enabled": False, "suggestions": {}}},
    )
    monkeypatch.setattr("solstone.think.routines.save_config", lambda config: None)

    spawns: list[dict] = []

    def fake_spawn_agent(prompt, name, provider, config, use_id):
        spawns.append(
            {
                "prompt": prompt,
                "name": name,
                "provider": provider,
                "config": dict(config),
                "use_id": str(use_id),
            }
        )
        return use_id

    monkeypatch.setattr("solstone.convey.utils.spawn_agent", fake_spawn_agent)

    app = Flask(__name__)
    app.register_blueprint(chat.chat_bp)
    app.testing = True
    client = app.test_client()

    response = client.post(
        "/api/chat",
        json={
            "message": "Can you check this?",
            "app": "sol",
            "path": "/app/sol",
            "facet": "work",
        },
    )
    assert response.status_code == 200
    assert response.get_json()["queued"] is False

    first_chat_spawn = spawns[-1]
    assert first_chat_spawn["name"] == "chat"
    chat._on_cortex_finish(
        {
            "use_id": first_chat_spawn["use_id"],
            "result": json.dumps(
                {
                    "message": "I am checking.",
                    "notes": "dispatch exec",
                    "talent_request": {
                        "target": "exec",
                        "task": "Check the thing",
                        "context": json.dumps({"facet": "work"}),
                    },
                }
            ),
        }
    )

    talent_spawn = spawns[-1]
    assert talent_spawn["name"] == "exec"
    if terminal_kind == "talent_finished":
        chat._on_cortex_finish(
            {"use_id": talent_spawn["use_id"], "result": result_value}
        )
    else:
        chat._on_cortex_error({"use_id": talent_spawn["use_id"], "error": result_value})

    report_back_spawn = spawns[-1]
    assert report_back_spawn["name"] == "chat"
    assert report_back_spawn["config"]["trigger"]["type"] == terminal_kind
    assert report_back_spawn["config"]["trigger"][result_field_name] == result_value

    context_result = chat_context.pre_process(report_back_spawn["config"])
    followup = context_result["messages"][-1]["content"]
    trigger_context = context_result["template_vars"]["trigger_context"]
    stop_and_report = (
        "stop-and-report turn, not a dispatch turn. Do not retry this task "
        "or request another talent for it. Stop here and report to the owner "
        "directly using the"
    )
    assert stop_and_report in followup
    assert stop_and_report in trigger_context
    assert f"{result_field_label.capitalize()}: {result_value}" in followup

    raw_report_use_id = report_back_spawn["use_id"]
    chat._on_cortex_finish(
        {
            "use_id": raw_report_use_id,
            "result": json.dumps(
                {
                    "message": "Here is the summary.",
                    "notes": "reported terminal talent result",
                    "talent_request": None,
                }
            ),
        }
    )

    events = read_chat_events(chat._today_day())
    sol_messages = [event for event in events if event["kind"] == "sol_message"]
    assert sol_messages[-1]["text"] == "Here is the summary."
    assert sol_messages[-1]["requested_target"] is None
    talent_spawns = [event for event in events if event["kind"] == "talent_spawned"]
    assert len(talent_spawns) == 1
    assert [spawn["name"] for spawn in spawns] == ["chat", "exec", "chat"]


def test_start_chat_runtime_recovers_exactly_one_unresponded_trigger(
    tmp_path, monkeypatch
):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)

    append_chat_event(
        "owner_message",
        text="recover me",
        app="sol",
        path="/app/sol",
        facet="work",
    )

    starts: list[dict] = []
    monkeypatch.setattr(
        "solstone.convey.chat.CallosumConnection.start",
        lambda self, callback=None: None,
    )
    monkeypatch.setattr(
        "solstone.convey.chat.CallosumConnection.stop", lambda self: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._spawn_chat_generate",
        lambda action: starts.append(action) or True,
    )

    app = Flask(__name__)
    chat.start_chat_runtime(app)
    chat.start_chat_runtime(app)

    assert len(starts) == 1


def test_start_chat_runtime_skips_debug_reloader_parent(tmp_path, monkeypatch, caplog):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)

    starts: list[object] = []
    monkeypatch.setattr(
        "solstone.convey.chat.CallosumConnection.start",
        lambda self, callback=None: starts.append(callback),
    )
    monkeypatch.setattr(
        "solstone.convey.chat.CallosumConnection.stop", lambda self: None
    )

    app = Flask(__name__)
    app.debug = True

    monkeypatch.delenv("WERKZEUG_RUN_MAIN", raising=False)
    with caplog.at_level("INFO"):
        chat.start_chat_runtime(app)

    assert chat._runtime is None
    assert starts == []
    assert app.chat_runtime_started is False
    assert "skipping chat runtime startup in Werkzeug reloader parent" in caplog.text

    monkeypatch.setenv("WERKZEUG_RUN_MAIN", "true")
    chat.start_chat_runtime(app)

    assert chat._runtime is not None
    assert len(starts) == 1
    assert app.chat_runtime_started is True


def test_recover_active_talents_repopulates_from_chat_stream(tmp_path, monkeypatch):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)
    timers = _install_fake_timers(monkeypatch)
    day = datetime.now().strftime("%Y%m%d")
    monkeypatch.setattr("solstone.convey.chat._today_day", lambda: day)

    chat_use_id = "1713624500000"
    talent_use_id = "1713624500001"
    _append_recoverable_talent_events(chat_use_id, talent_use_id)

    chat._recover_chat_if_needed()

    with chat._state_lock:
        assert chat._active_talents[talent_use_id] == {
            "chat_use_id": chat_use_id,
            "target": "exec",
            "task": "research it",
            "location": {"app": "home", "path": "/app/home", "facet": "work"},
        }
        assert talent_use_id in chat._watchdog_timers
    assert len(timers) == 1


def test_late_talent_finish_after_recovery_routes_to_chat_continuation(
    tmp_path, monkeypatch, caplog
):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)
    _install_fake_timers(monkeypatch)
    day = datetime.now().strftime("%Y%m%d")
    monkeypatch.setattr("solstone.convey.chat._today_day", lambda: day)

    chat_use_id = "1713624600000"
    talent_use_id = "1713624600001"
    _append_recoverable_talent_events(chat_use_id, talent_use_id)
    chat._recover_chat_if_needed()

    actions: list[dict | None] = []
    monkeypatch.setattr(
        "solstone.convey.chat._run_next_action", lambda action: actions.append(action)
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_finish", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_error", lambda *args, **kwargs: None
    )

    with chat._state_lock:
        chat._current_chat_use_id = chat_use_id
        chat._current_chat_state = {
            "raw_use_id": None,
            "raw_use_ids_seen": set(),
            "trigger": {"type": "owner_message", "message": "help"},
            "location": {"app": "home", "path": "/app/home", "facet": "work"},
            "retry_count": 0,
        }

    with caplog.at_level("WARNING"):
        chat._on_cortex_finish({"use_id": talent_use_id, "result": "done"})

    assert "unrouteable cortex event" not in caplog.text
    finished_events = [
        e for e in read_chat_events(chat._today_day()) if e["kind"] == "talent_finished"
    ]
    assert finished_events[-1]["use_id"] == talent_use_id
    assert actions[-1]["logical_use_id"] == chat_use_id
    assert actions[-1]["trigger"]["type"] == "talent_finished"


def test_recovery_is_idempotent_for_active_talents(tmp_path, monkeypatch):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)
    timers = _install_fake_timers(monkeypatch)
    day = datetime.now().strftime("%Y%m%d")
    monkeypatch.setattr("solstone.convey.chat._today_day", lambda: day)

    chat_use_id = "1713624700000"
    talent_use_id = "1713624700001"
    _append_recoverable_talent_events(chat_use_id, talent_use_id)

    chat._recover_chat_if_needed()
    chat._recover_chat_if_needed()

    with chat._state_lock:
        assert list(chat._active_talents) == [talent_use_id]
        assert talent_use_id in chat._watchdog_timers
    assert len(timers) == 1


def test_chat_generate_schema_violation_retries_once_then_chat_errors(
    tmp_path, monkeypatch
):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)

    actions: list[dict | None] = []
    emitted_errors: list[tuple[str, str]] = []
    monkeypatch.setattr(
        "solstone.convey.chat._run_next_action", lambda action: actions.append(action)
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_finish", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_error",
        lambda use_id, reason: emitted_errors.append((use_id, reason)),
    )

    with chat._state_lock:
        chat._current_chat_use_id = "1713625000000"
        chat._current_chat_state = {
            "raw_use_id": "1713625000001",
            "raw_use_ids_seen": {"1713625000001"},
            "trigger": {"type": "owner_message", "message": "help"},
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "retry_count": 0,
        }

    chat._on_cortex_finish({"use_id": "1713625000001", "result": "not json"})

    assert actions and actions[-1]["kind"] == "chat"
    assert actions[-1]["logical_use_id"] == "1713625000000"
    assert emitted_errors == []

    with chat._state_lock:
        retry_use_id = chat._current_chat_state["raw_use_id"]

    chat._on_cortex_finish({"use_id": retry_use_id, "result": "still not json"})

    assert emitted_errors == [("1713625000000", "provider_response_invalid")]
    errors = [
        e for e in read_chat_events(chat._today_day()) if e["kind"] == "chat_error"
    ]
    assert errors[-1]["use_id"] == "1713625000000"


def test_chat_generate_invalid_context_retries_once_then_chat_errors(
    tmp_path, monkeypatch
):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)

    actions: list[dict | None] = []
    emitted_errors: list[tuple[str, str]] = []
    monkeypatch.setattr(
        "solstone.convey.chat._run_next_action", lambda action: actions.append(action)
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_finish", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_error",
        lambda use_id, reason: emitted_errors.append((use_id, reason)),
    )

    with chat._state_lock:
        chat._current_chat_use_id = "1713625050000"
        chat._current_chat_state = {
            "raw_use_id": "1713625050001",
            "raw_use_ids_seen": {"1713625050001"},
            "trigger": {"type": "owner_message", "message": "help"},
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "retry_count": 0,
        }

    invalid_context_result = json.dumps(
        {
            "message": "I am looking into that.",
            "notes": "need exec",
            "talent_request": {
                "target": "exec",
                "task": "research it",
                "context": "not json{",
            },
        }
    )
    chat._on_cortex_finish(
        {"use_id": "1713625050001", "result": invalid_context_result}
    )

    assert actions and actions[-1]["kind"] == "chat"
    assert actions[-1]["logical_use_id"] == "1713625050000"
    assert emitted_errors == []

    with chat._state_lock:
        retry_use_id = chat._current_chat_state["raw_use_id"]

    chat._on_cortex_finish({"use_id": retry_use_id, "result": invalid_context_result})

    assert emitted_errors == [("1713625050000", "provider_response_invalid")]
    errors = [
        e for e in read_chat_events(chat._today_day()) if e["kind"] == "chat_error"
    ]
    assert errors[-1]["use_id"] == "1713625050000"
    assert errors[-1]["reason"] == "provider_response_invalid"


def test_superseded_raw_finish_after_retry_is_dropped_without_warning(
    tmp_path, monkeypatch, caplog
):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)

    actions: list[dict | None] = []
    monkeypatch.setattr(
        "solstone.convey.chat._run_next_action", lambda action: actions.append(action)
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_finish", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_error", lambda *args, **kwargs: None
    )

    raw_use_id = "1713625100001"
    with chat._state_lock:
        chat._current_chat_use_id = "1713625100000"
        chat._current_chat_state = {
            "raw_use_id": raw_use_id,
            "raw_use_ids_seen": {raw_use_id},
            "trigger": {"type": "owner_message", "message": "help"},
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "retry_count": 0,
        }

    chat._on_cortex_finish({"use_id": raw_use_id, "result": "not json"})

    with chat._state_lock:
        retry_use_id = str(chat._current_chat_state["raw_use_id"])

    events_before = list(read_chat_events(chat._today_day()))
    with caplog.at_level("DEBUG"):
        chat._on_cortex_finish({"use_id": raw_use_id, "result": "still not json"})

    assert (
        "superseded raw cortex event use_id=1713625100001 event=finish reason=raw rotated"
        in caplog.text
    )
    assert "unrouteable cortex event" not in caplog.text
    assert read_chat_events(chat._today_day()) == events_before

    chat._on_cortex_finish(
        {
            "use_id": retry_use_id,
            "result": '{"message":"done","notes":"ok","talent_request":null}',
        }
    )

    sol_messages = [
        event
        for event in read_chat_events(chat._today_day())
        if event["kind"] == "sol_message"
    ]
    assert sol_messages[-1]["text"] == "done"


def test_superseded_raw_error_after_followup_rotation_is_dropped_without_warning(
    tmp_path, monkeypatch, caplog
):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)

    actions: list[dict | None] = []
    monkeypatch.setattr(
        "solstone.convey.chat._run_next_action", lambda action: actions.append(action)
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_finish", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_error", lambda *args, **kwargs: None
    )

    stale_raw_use_id = "1713625200001"
    with chat._state_lock:
        chat._current_chat_use_id = "1713625200000"
        chat._current_chat_state = {
            "raw_use_id": None,
            "raw_use_ids_seen": {stale_raw_use_id},
            "trigger": {"type": "owner_message", "message": "help"},
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "retry_count": 0,
        }
        chat._active_talents["1713625200002"] = {
            "chat_use_id": "1713625200000",
            "target": "exec",
            "task": "summarize",
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
        }

    chat._on_cortex_finish({"use_id": "1713625200002", "result": "summary"})

    with chat._state_lock:
        followup_use_id = str(chat._current_chat_state["raw_use_id"])

    events_before = list(read_chat_events(chat._today_day()))
    with caplog.at_level("DEBUG"):
        chat._on_cortex_error({"use_id": stale_raw_use_id, "error": "boom"})

    assert (
        "superseded raw cortex event use_id=1713625200001 event=error reason=raw rotated"
        in caplog.text
    )
    assert "unrouteable cortex event" not in caplog.text
    assert read_chat_events(chat._today_day()) == events_before
    assert actions[0]["trigger"]["type"] == "talent_finished"

    chat._on_cortex_finish(
        {
            "use_id": followup_use_id,
            "result": '{"message":"wrapped up","notes":"ok","talent_request":null}',
        }
    )

    sol_messages = [
        event
        for event in read_chat_events(chat._today_day())
        if event["kind"] == "sol_message"
    ]
    assert sol_messages[-1]["text"] == "wrapped up"


def test_reserved_unknown_raw_use_id_still_warns(tmp_path, monkeypatch, caplog):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)

    use_id = "1713625300009"
    with chat._state_lock:
        chat._current_chat_use_id = "1713625300000"
        chat._current_chat_state = {
            "raw_use_id": "1713625300002",
            "raw_use_ids_seen": {"1713625300001", "1713625300002"},
            "trigger": {"type": "owner_message", "message": "help"},
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "retry_count": 0,
        }
        chat._reserved_use_ids[use_id] = None

    caplog.set_level(logging.WARNING, logger="solstone.convey.chat")
    chat._on_cortex_finish({"use_id": use_id, "result": "done"})

    chat_records = [
        record
        for record in caplog.records
        if record.name == "solstone.convey.chat" and record.levelno == logging.WARNING
    ]
    assert len(chat_records) == 1
    assert (
        "unrouteable cortex event use_id=1713625300009 event=finish "
        "reason=no matching active chat-generate or talent"
    ) == chat_records[0].getMessage()


def test_exec_dispatch_appends_sol_message_and_spawns_talent_real_path(
    tmp_path, monkeypatch
):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)
    timers = _install_fake_timers(monkeypatch)

    spawn_calls: list[dict[str, object]] = []
    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *args, **kwargs: None
    )

    def fake_spawn_agent(prompt, name, provider=None, config=None, use_id=None):
        spawn_calls.append(
            {
                "prompt": prompt,
                "name": name,
                "provider": provider,
                "config": config,
                "use_id": use_id,
            }
        )
        return use_id

    monkeypatch.setattr("solstone.convey.utils.spawn_agent", fake_spawn_agent)

    with chat._state_lock:
        start_info = chat._activate_current_locked(
            "1713625500000",
            {"type": "owner_message", "message": "help"},
            {"app": "sol", "path": "/app/sol", "facet": "work"},
        )

    raw_use_id = start_info["raw_use_id"]
    chat._on_cortex_finish(
        {
            "use_id": raw_use_id,
            "result": json.dumps(
                {
                    "message": "I am looking into that.",
                    "notes": "need exec",
                    "talent_request": {
                        "target": "exec",
                        "task": "research it",
                        "context": json.dumps({"k": "v"}),
                    },
                }
            ),
        }
    )

    events = read_chat_events(chat._today_day())
    sol_messages = [event for event in events if event["kind"] == "sol_message"]
    spawned_events = [event for event in events if event["kind"] == "talent_spawned"]

    assert sol_messages[-1]["text"] == "I am looking into that."
    assert sol_messages[-1]["requested_target"] == "exec"
    assert sol_messages[-1]["requested_task"] == "research it"
    assert spawned_events[-1]["name"] == "exec"
    assert spawned_events[-1]["task"] == "research it"
    assert len(spawn_calls) == 1
    spawn_call = spawn_calls[0]
    assert spawn_call["name"] == "exec"
    assert spawn_call["use_id"] == spawned_events[-1]["use_id"]
    assert spawn_call["config"] == {
        "app": "sol",
        "path": "/app/sol",
        "facet": "work",
        "chat_parent_use_id": "1713625500000",
    }
    assert "research it" in str(spawn_call["prompt"])
    assert "Context hints:\n{'k': 'v'}" in str(spawn_call["prompt"])
    assert len(timers) == 2
    assert timers[0].cancelled is True
    with chat._state_lock:
        assert spawned_events[-1]["use_id"] in chat._active_talents


def test_watchdog_refreshed_by_progress_event(tmp_path, monkeypatch):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)
    timers = _install_fake_timers(monkeypatch)

    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *args, **kwargs: None
    )

    with chat._state_lock:
        start_info = chat._activate_current_locked(
            "1713627800000",
            {"type": "owner_message", "message": "help"},
            {"app": "sol", "path": "/app/sol", "facet": "work"},
        )

    raw_use_id = start_info["raw_use_id"]
    assert len(timers) == 1

    chat._proxy_progress(
        {
            "tract": "cortex",
            "event": "thinking",
            "use_id": raw_use_id,
        }
    )

    assert len(timers) == 2
    assert timers[0].cancelled is True
    assert timers[-1].interval == chat._CHAT_WATCHDOG_SECONDS


def test_watchdog_refresh_is_no_op_when_no_timer_registered(tmp_path, monkeypatch):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)
    timers = _install_fake_timers(monkeypatch)

    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *args, **kwargs: None
    )

    with chat._state_lock:
        chat._current_chat_use_id = "1713627850000"
        chat._current_chat_state = {
            "raw_use_id": "1713627850001",
            "raw_use_ids_seen": {"1713627850001"},
            "trigger": {"type": "owner_message", "message": "help"},
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "retry_count": 0,
        }

    chat._proxy_progress(
        {
            "tract": "cortex",
            "event": "thinking",
            "use_id": "1713627850002",
        }
    )

    assert len(timers) == 0


def test_watchdog_refreshed_by_talent_progress(tmp_path, monkeypatch):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)
    timers = _install_fake_timers(monkeypatch)

    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *args, **kwargs: None
    )

    with chat._state_lock:
        chat._current_chat_use_id = "1713627900000"
        chat._current_chat_state = {
            "raw_use_id": None,
            "raw_use_ids_seen": set(),
            "trigger": {"type": "owner_message", "message": "help"},
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "retry_count": 0,
        }
        chat._active_talents["1713627900001"] = {
            "chat_use_id": "1713627900000",
            "target": "exec",
            "task": "summarize",
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
        }
        chat._arm_watchdog_locked("1713627900001", "talent", "1713627900000")

    assert len(timers) == 1

    chat._proxy_progress(
        {
            "tract": "cortex",
            "event": "thinking",
            "use_id": "1713627900001",
        }
    )

    assert len(timers) == 2
    assert timers[0].cancelled is True
    assert timers[-1].interval == chat._CHAT_WATCHDOG_SECONDS


def test_stalled_run_still_times_out_after_inactivity(tmp_path, monkeypatch):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)
    timers = _install_fake_timers(monkeypatch)

    emitted_errors: list[tuple[str, str]] = []
    run_actions: list[dict | None] = []
    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_error",
        lambda use_id, reason: emitted_errors.append((use_id, reason)),
    )
    monkeypatch.setattr(
        "solstone.convey.chat._run_next_action",
        lambda action: run_actions.append(action),
    )

    with chat._state_lock:
        start_info = chat._activate_current_locked(
            "1713627950000",
            {"type": "owner_message", "message": "help"},
            {"app": "sol", "path": "/app/sol", "facet": "work"},
        )

    raw_use_id = start_info["raw_use_id"]
    assert len(timers) == 1

    for _ in range(3):
        chat._proxy_progress(
            {
                "tract": "cortex",
                "event": "thinking",
                "use_id": raw_use_id,
            }
        )

    assert len(timers) == 4
    timers[-1].fire()

    errors = [
        event
        for event in read_chat_events(chat._today_day())
        if event["kind"] == "chat_error"
    ]
    assert emitted_errors == [("1713627950000", "chat_timeout")]
    assert run_actions == [None]
    assert errors[-1]["use_id"] == "1713627950000"
    assert errors[-1]["reason"] == "chat_timeout"
    with chat._state_lock:
        assert chat._current_chat_use_id is None
        assert chat._current_chat_state is None
        assert raw_use_id not in chat._watchdog_timers


def test_chat_watchdog_times_out_current_chat_generate(tmp_path, monkeypatch):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)
    timers = _install_fake_timers(monkeypatch)

    emitted_errors: list[tuple[str, str]] = []
    run_actions: list[dict | None] = []
    monkeypatch.setattr(
        "solstone.convey.chat._emit_error",
        lambda use_id, reason: emitted_errors.append((use_id, reason)),
    )
    monkeypatch.setattr(
        "solstone.convey.chat._run_next_action",
        lambda action: run_actions.append(action),
    )

    with chat._state_lock:
        start_info = chat._activate_current_locked(
            "1713628000000",
            {"type": "owner_message", "message": "help"},
            {"app": "sol", "path": "/app/sol", "facet": "work"},
        )

    raw_use_id = start_info["raw_use_id"]
    assert raw_use_id in chat._watchdog_timers

    timers[-1].fire()

    errors = [
        event
        for event in read_chat_events(chat._today_day())
        if event["kind"] == "chat_error"
    ]
    assert emitted_errors == [("1713628000000", "chat_timeout")]
    assert run_actions == [None]
    assert errors[-1]["use_id"] == "1713628000000"
    assert errors[-1]["reason"] == "chat_timeout"
    with chat._state_lock:
        assert chat._current_chat_use_id is None
        assert chat._current_chat_state is None
        assert raw_use_id not in chat._watchdog_timers


def test_chat_watchdog_times_out_active_talent_and_clears_blocked_chat(
    tmp_path, monkeypatch
):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)
    timers = _install_fake_timers(monkeypatch)

    emitted_errors: list[tuple[str, str]] = []
    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_error",
        lambda use_id, reason: emitted_errors.append((use_id, reason)),
    )
    monkeypatch.setattr(
        "solstone.convey.utils.spawn_agent", lambda *args, **kwargs: kwargs["use_id"]
    )

    with chat._state_lock:
        chat._current_chat_use_id = "1713629000000"
        chat._current_chat_state = {
            "raw_use_id": None,
            "raw_use_ids_seen": set(),
            "trigger": {"type": "owner_message", "message": "help"},
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "retry_count": 0,
        }
        chat._active_talents["1713629000001"] = {
            "chat_use_id": "1713629000000",
            "target": "exec",
            "task": "summarize",
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
        }

    chat._run_next_action(
        {
            "kind": "talent",
            "logical_use_id": "1713629000000",
            "target": "exec",
            "use_id": "1713629000001",
            "task": "summarize",
            "context": {},
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
        }
    )

    assert "1713629000001" in chat._watchdog_timers
    timers[-1].fire()

    errors = [
        event
        for event in read_chat_events(chat._today_day())
        if event["kind"] == "chat_error"
    ]
    assert emitted_errors == [("1713629000000", "chat_timeout")]
    assert errors[-1]["use_id"] == "1713629000000"
    assert errors[-1]["reason"] == "chat_timeout"
    with chat._state_lock:
        assert "1713629000001" not in chat._active_talents
        assert chat._current_chat_use_id is None
        assert chat._current_chat_state is None
        assert "1713629000001" not in chat._watchdog_timers


def test_chat_watchdog_marks_timed_out_talent_result_as_errored(tmp_path, monkeypatch):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)
    timers = _install_fake_timers(monkeypatch)

    monkeypatch.setattr(
        "solstone.convey.chat._emit_cortex_event", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_error", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.utils.spawn_agent", lambda *args, **kwargs: kwargs["use_id"]
    )

    with chat._state_lock:
        logical_use_id = chat._reserve_use_id_locked()
        talent_use_id = chat._reserve_use_id_locked()

    append_chat_event(
        "talent_spawned",
        use_id=talent_use_id,
        name="exec",
        task="summarize",
        started_at=int(talent_use_id),
    )

    with chat._state_lock:
        chat._current_chat_use_id = logical_use_id
        chat._current_chat_state = {
            "raw_use_id": None,
            "raw_use_ids_seen": set(),
            "trigger": {"type": "owner_message", "message": "help"},
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "retry_count": 0,
        }
        chat._active_talents[talent_use_id] = {
            "chat_use_id": logical_use_id,
            "target": "exec",
            "task": "summarize",
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
        }

    chat._run_next_action(
        {
            "kind": "talent",
            "logical_use_id": logical_use_id,
            "target": "exec",
            "use_id": talent_use_id,
            "task": "summarize",
            "context": {},
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
        }
    )

    timers[-1].fire()

    parent_errors = [
        event
        for event in read_chat_events(chat._today_day())
        if event["kind"] == "chat_error"
    ]
    talent_errors = [
        event
        for event in read_chat_events(chat._today_day())
        if event["kind"] == "talent_errored"
    ]
    assert parent_errors[-1]["use_id"] == logical_use_id
    assert parent_errors[-1]["reason"] == "chat_timeout"
    assert talent_errors[-1]["use_id"] == talent_use_id
    assert talent_errors[-1]["reason"] == "talent took too long"


def test_cortex_finish_logs_warning_for_unrouteable_use_id(
    tmp_path, monkeypatch, caplog
):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)

    with chat._state_lock:
        use_id = chat._reserve_use_id_locked()

    caplog.set_level(logging.WARNING, logger="solstone.convey.chat")
    chat._on_cortex_finish({"use_id": use_id, "result": "done"})

    chat_records = [
        record
        for record in caplog.records
        if record.name == "solstone.convey.chat" and record.levelno == logging.WARNING
    ]
    assert len(chat_records) == 1
    assert (
        f"unrouteable cortex event use_id={use_id} event=finish "
        "reason=no matching active chat-generate or talent"
    ) == chat_records[0].getMessage()


@pytest.mark.parametrize(
    ("error_text", "expected_detail"),
    [
        ("short msg", "short msg"),
        (
            "something went very long with newlines\n\nand whitespace   collapsed "
            + ("0123456789 " * 30),
            None,
        ),
    ],
)
def test_on_cortex_error_stores_normalized_detail(
    tmp_path, monkeypatch, error_text, expected_detail
):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)
    monkeypatch.setattr("solstone.convey.chat._run_next_action", lambda action: None)
    monkeypatch.setattr("solstone.convey.chat._emit_error", lambda *args: None)

    logical_use_id = "1713632500000"
    raw_use_id = "1713632500001"
    with chat._state_lock:
        chat._current_chat_use_id = logical_use_id
        chat._current_chat_state = {
            "raw_use_id": raw_use_id,
            "raw_use_ids_seen": {raw_use_id},
            "trigger": {"type": "owner_message", "message": "help"},
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "retry_count": 0,
        }

    chat._on_cortex_error(
        {
            "use_id": raw_use_id,
            "reason_code": "unknown",
            "provider": "google",
            "error": error_text,
        }
    )

    errors = [
        event
        for event in read_chat_events(chat._today_day())
        if event["kind"] == "chat_error"
    ]
    assert len(errors) == 1
    assert errors[0]["reason"] == "unknown"
    assert errors[0]["provider"] == "google"
    detail = errors[0]["detail"]
    if expected_detail is not None:
        assert detail == expected_detail
    else:
        assert "\n" not in detail
        assert "  " not in detail
        assert len(detail) <= 240
        assert detail[-1] == "…"


def test_cortex_error_logs_warning_for_unrouteable_use_id(
    tmp_path, monkeypatch, caplog
):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)

    with chat._state_lock:
        use_id = chat._reserve_use_id_locked()

    caplog.set_level(logging.WARNING, logger="solstone.convey.chat")
    chat._on_cortex_error({"use_id": use_id, "error": "boom"})

    chat_records = [
        record
        for record in caplog.records
        if record.name == "solstone.convey.chat" and record.levelno == logging.WARNING
    ]
    assert len(chat_records) == 1
    assert (
        f"unrouteable cortex event use_id={use_id} event=error "
        "reason=no matching active chat-generate or talent"
    ) == chat_records[0].getMessage()


def test_on_cortex_finish_unreserved_silent(tmp_path, monkeypatch, caplog):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)

    caplog.set_level(logging.WARNING, logger="solstone.convey.chat")
    chat._on_cortex_finish({"use_id": "1713632000000", "result": "done"})

    chat_records = [
        record
        for record in caplog.records
        if record.name == "solstone.convey.chat" and record.levelno == logging.WARNING
    ]
    assert chat_records == []
    with chat._state_lock:
        assert chat._current_chat_use_id is None
        assert chat._current_chat_state is None
        assert chat._active_talents == {}
        assert chat._reserved_use_ids == {}


def test_on_cortex_error_unreserved_silent(tmp_path, monkeypatch, caplog):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)

    caplog.set_level(logging.WARNING, logger="solstone.convey.chat")
    chat._on_cortex_error({"use_id": "1713633000000", "error": "boom"})

    chat_records = [
        record
        for record in caplog.records
        if record.name == "solstone.convey.chat" and record.levelno == logging.WARNING
    ]
    assert chat_records == []
    with chat._state_lock:
        assert chat._current_chat_use_id is None
        assert chat._current_chat_state is None
        assert chat._active_talents == {}
        assert chat._reserved_use_ids == {}


def test_parse_chat_result_accepts_reflection_target():
    import solstone.convey.chat as chat

    parsed = chat._parse_chat_result(
        {
            "message": "Let me think about that.",
            "notes": "dispatch reflection",
            "talent_request": {
                "target": "reflection",
                "task": "Reflect on the last week",
                "context": {"facet": "work"},
            },
        }
    )

    assert parsed["talent_request"] == {
        "target": "reflection",
        "task": "Reflect on the last week",
        "context": {"facet": "work"},
    }
    # Exercises the scope-mandated defensive dict shim.
    assert parsed["talent_request"]["context"] == {"facet": "work"}


def test_parse_chat_result_decodes_json_string_context():
    import solstone.convey.chat as chat

    parsed = chat._parse_chat_result(
        {
            "message": "I am looking into that.",
            "notes": "need exec",
            "talent_request": {
                "target": "exec",
                "task": "Research the last two weeks",
                "context": '{"window":"14d"}',
            },
        }
    )

    assert parsed["talent_request"]["context"] == {"window": "14d"}


@pytest.mark.parametrize("raw_context", ['["a","b"]', "42"])
def test_parse_chat_result_rejects_non_object_context_string(raw_context):
    import solstone.convey.chat as chat

    with pytest.raises(ValueError) as excinfo:
        chat._parse_chat_result(
            {
                "message": "I am looking into that.",
                "notes": "need exec",
                "talent_request": {
                    "target": "exec",
                    "task": "Research it",
                    "context": raw_context,
                },
            }
        )

    assert type(excinfo.value) is ValueError


def test_parse_chat_result_rejects_non_json_context_string():
    import solstone.convey.chat as chat

    with pytest.raises(ValueError) as excinfo:
        chat._parse_chat_result(
            {
                "message": "I am looking into that.",
                "notes": "need exec",
                "talent_request": {
                    "target": "exec",
                    "task": "Research it",
                    "context": "not json{",
                },
            }
        )

    assert type(excinfo.value) is ValueError
    assert not isinstance(excinfo.value, json.JSONDecodeError)


def test_parse_chat_result_accepts_null_talent_request():
    import solstone.convey.chat as chat

    parsed = chat._parse_chat_result(
        {"message": "hi", "notes": "n", "talent_request": None}
    )

    assert parsed == {"message": "hi", "notes": "n", "talent_request": None}


def test_parse_chat_result_rejects_unknown_target():
    import solstone.convey.chat as chat

    with pytest.raises(ValueError, match="unknown talent target: foo"):
        chat._parse_chat_result(
            {
                "message": "Let me think about that.",
                "notes": "dispatch reflection",
                "talent_request": {"target": "foo", "task": "Reflect on the week"},
            }
        )


def test_parse_chat_result_rejects_missing_target():
    import solstone.convey.chat as chat

    with pytest.raises(ValueError, match="chat talent_request.target must be a string"):
        chat._parse_chat_result(
            {
                "message": "I am looking into that.",
                "notes": "need exec",
                "talent_request": {"task": "Research it", "context": {"k": "v"}},
            }
        )


def test_reflection_dispatch_spawns_reflection_talent(tmp_path, monkeypatch):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)

    actions: list[dict | None] = []
    monkeypatch.setattr(
        "solstone.convey.chat._run_next_action", lambda action: actions.append(action)
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_finish", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_error", lambda *args, **kwargs: None
    )

    with chat._state_lock:
        chat._current_chat_use_id = "1713626000000"
        chat._current_chat_state = {
            "raw_use_id": "1713626000001",
            "raw_use_ids_seen": {"1713626000001"},
            "trigger": {"type": "owner_message", "message": "help"},
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "retry_count": 0,
        }

    chat._on_cortex_finish(
        {
            "use_id": "1713626000001",
            "result": json.dumps(
                {
                    "message": "I want to sit with that.",
                    "notes": "need reflection",
                    "talent_request": {
                        "target": "reflection",
                        "task": "Reflect on the week",
                        "context": json.dumps({"facet": "work"}),
                    },
                }
            ),
        }
    )

    assert actions[-1]["kind"] == "talent"
    assert actions[-1]["target"] == "reflection"

    events = read_chat_events(chat._today_day())
    sol_message = next(event for event in events if event["kind"] == "sol_message")
    spawned = next(event for event in events if event["kind"] == "talent_spawned")
    assert sol_message["requested_target"] == "reflection"
    assert spawned["name"] == "reflection"


def test_reflection_finish_retriggers_chat_like_exec(tmp_path, monkeypatch):
    import solstone.convey.chat as chat

    _setup_journal(tmp_path, monkeypatch)
    _reset_chat_state(chat)

    actions: list[dict | None] = []
    monkeypatch.setattr(
        "solstone.convey.chat._run_next_action", lambda action: actions.append(action)
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_finish", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(
        "solstone.convey.chat._emit_error", lambda *args, **kwargs: None
    )

    with chat._state_lock:
        chat._current_chat_use_id = "1713627000000"
        chat._current_chat_state = {
            "raw_use_id": None,
            "raw_use_ids_seen": set(),
            "trigger": {"type": "owner_message", "message": "help"},
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "retry_count": 0,
        }
        chat._active_talents["1713627000001"] = {
            "chat_use_id": "1713627000000",
            "target": "reflection",
            "task": "Reflect on the week",
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
        }

    chat._on_cortex_finish({"use_id": "1713627000001", "result": "A reflective note"})

    finished_events = [
        e for e in read_chat_events(chat._today_day()) if e["kind"] == "talent_finished"
    ]
    assert finished_events[-1]["name"] == "reflection"
    assert actions[-1]["trigger"]["type"] == "talent_finished"
    assert actions[-1]["trigger"]["name"] == "reflection"
