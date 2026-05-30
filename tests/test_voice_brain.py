# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import asyncio
import time

from flask import Flask

from solstone.think.voice import brain
from solstone.think.voice.runtime import start_voice_runtime, stop_voice_runtime


def test_extract_instruction():
    text = "before<voice_instruction>Hello there</voice_instruction>after"
    assert brain.extract_instruction(text) == "Hello there"
    assert brain.extract_instruction("no tags here") is None


def test_start_brain_persists_session(monkeypatch, journal_copy):
    async def fake_run_claude(message, extra_args, *, timeout):
        assert "voice-session instruction" in message
        assert extra_args == ["-n", "voice-brain"]
        assert timeout == 300
        return "<voice_instruction>Speak clearly</voice_instruction>", "session-1"

    monkeypatch.setattr(brain, "_run_claude", fake_run_claude)

    session_id, instruction = asyncio.run(brain.start_brain())

    assert session_id == "session-1"
    assert instruction == "Speak clearly"
    assert (journal_copy / "health" / "voice-brain-session").read_text(
        encoding="utf-8"
    ) == "session-1"


def test_refresh_brain_touches_session_file(monkeypatch, journal_copy):
    session_file = journal_copy / "health" / "voice-brain-session"
    session_file.parent.mkdir(parents=True, exist_ok=True)
    session_file.write_text("session-1", encoding="utf-8")

    async def fake_run_claude(message, extra_args, *, timeout):
        assert extra_args == ["--resume", "session-1"]
        assert timeout == 300
        return "<voice_instruction>Fresh voice</voice_instruction>", "session-1"

    monkeypatch.setattr(brain, "_run_claude", fake_run_claude)
    before = session_file.stat().st_mtime

    instruction = asyncio.run(brain.refresh_brain("session-1"))

    assert instruction == "Fresh voice"
    assert session_file.stat().st_mtime >= before


def test_ask_brain_uses_resume(monkeypatch):
    async def fake_run_claude(message, extra_args, *, timeout):
        assert message == "What changed?"
        assert extra_args == ["--resume", "session-1"]
        assert timeout == 120
        return "Short answer", "session-1"

    monkeypatch.setattr(brain, "_run_claude", fake_run_claude)

    assert asyncio.run(brain.ask_brain("session-1", "What changed?")) == "Short answer"


def test_schedule_start_and_wait_until_ready(monkeypatch, journal_copy):
    brain.clear_brain_state()
    app = Flask(__name__)

    async def fake_run_claude(message, extra_args, *, timeout):
        return "<voice_instruction>Ready voice</voice_instruction>", "session-2"

    monkeypatch.setattr(brain, "_run_claude", fake_run_claude)

    start_voice_runtime(app)
    try:
        assert brain.wait_until_ready(app, 1.0) is True
        assert app.voice_brain_session == "session-2"
        assert app.voice_brain_instruction == "Ready voice"
        assert isinstance(brain.brain_age_seconds(app), int)
    finally:
        stop_voice_runtime(app)
        brain.clear_brain_state()


def test_schedule_refresh_updates_instruction(monkeypatch, journal_copy):
    brain.clear_brain_state()
    app = Flask(__name__)
    app.voice_brain_session = "session-3"
    app.voice_brain_instruction = "Old voice"
    app.voice_brain_refreshed_at = None
    (journal_copy / "health").mkdir(parents=True, exist_ok=True)
    (journal_copy / "health" / "voice-brain-session").write_text(
        "session-3", encoding="utf-8"
    )

    async def fake_run_claude(message, extra_args, *, timeout):
        return "<voice_instruction>New voice</voice_instruction>", "session-3"

    monkeypatch.setattr(brain, "_run_claude", fake_run_claude)

    start_voice_runtime(app)
    try:
        future = brain.schedule_refresh(app, force=True)
        assert future.result(timeout=1.0) == ("session-3", "New voice")
        # The app-state update runs in the future's done-callback. A
        # concurrent.futures.Future notifies result() waiters *before* it
        # invokes done-callbacks, so the callback may not have applied the
        # instruction yet when result() returns -- poll for the side effect
        # rather than racing it (this was an xdist-only flake under load).
        deadline = time.monotonic() + 1.0
        while (
            app.voice_brain_instruction != "New voice" and time.monotonic() < deadline
        ):
            time.sleep(0.01)
        assert app.voice_brain_instruction == "New voice"
    finally:
        stop_voice_runtime(app)
        brain.clear_brain_state()


def test_brain_session_file_stays_on_bound_journal(monkeypatch, tmp_path):
    brain.clear_brain_state()
    initial_journal = tmp_path / "initial-journal"
    later_journal = tmp_path / "later-journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(initial_journal))
    app = Flask(__name__)

    async def fake_run_claude(message, extra_args, *, timeout):
        await asyncio.sleep(0.01)
        return "<voice_instruction>Ready voice</voice_instruction>", "session-4"

    monkeypatch.setattr(brain, "_run_claude", fake_run_claude)

    start_voice_runtime(app)
    try:
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(later_journal))
        assert brain.wait_until_ready(app, 1.0) is True
        assert (initial_journal / "health" / "voice-brain-session").read_text(
            encoding="utf-8"
        ) == "session-4"
        assert not (later_journal / "health" / "voice-brain-session").exists()
    finally:
        stop_voice_runtime(app)
        brain.clear_brain_state()
