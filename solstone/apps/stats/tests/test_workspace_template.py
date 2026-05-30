# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import re

import pytest

from solstone.convey import backlog_copy

BACKLOG_COPY_KEYS = [
    "BACKLOG_VERDICT_CAUGHT_UP",
    "BACKLOG_VERDICT_STUCK_ONLY_PLURAL",
    "BACKLOG_VERDICT_STUCK_ONLY_SINGULAR",
    "BACKLOG_VERDICT_PENDING_ONLY_PLURAL",
    "BACKLOG_VERDICT_PENDING_ONLY_SINGULAR",
    "BACKLOG_VERDICT_BOTH_PLURAL",
    "BACKLOG_VERDICT_CANT_TELL",
    "BACKLOG_BUCKET_HEADING",
    "BACKLOG_BUCKET_DESCRIPTION",
    "BACKLOG_DAY_BADGE",
    "BACKLOG_REASON_CORRUPT_RAW",
    "BACKLOG_REASON_FAILING_STEP",
    "BACKLOG_REASON_MISSING_CONFIG",
    "BACKLOG_REASON_PROVIDER_DOWN",
    "BACKLOG_WHY_NEVER_ATTEMPTED",
    "BACKLOG_WHY_FAILED",
    "BACKLOG_WHY_SENSED_NOT_THOUGHT",
    "BACKLOG_CATCHING_UP_DAY",
    "BACKLOG_CATCHING_UP_AGGREGATE",
    "BACKLOG_CATCHING_UP_TAIL",
]

BACKLOG_COPY_LITERALS = {
    "BACKLOG_VERDICT_CAUGHT_UP": "your journal's all caught up.",
    "BACKLOG_VERDICT_STUCK_ONLY_PLURAL": (
        "caught up except {stuck_n} days that need a hand."
    ),
    "BACKLOG_VERDICT_STUCK_ONLY_SINGULAR": (
        "caught up except 1 day that needs a hand."
    ),
    "BACKLOG_VERDICT_PENDING_ONLY_PLURAL": (
        "caught up — {pending_n} days still catching up."
    ),
    "BACKLOG_VERDICT_PENDING_ONLY_SINGULAR": ("caught up — 1 day still catching up."),
    "BACKLOG_VERDICT_BOTH_PLURAL": (
        "caught up except {stuck_n} days that need a hand — "
        "{pending_n} more still catching up."
    ),
    "BACKLOG_VERDICT_CANT_TELL": (
        "still checking — give me a moment to see where your journal stands."
    ),
    "BACKLOG_BUCKET_HEADING": "days that need a hand",
    "BACKLOG_BUCKET_DESCRIPTION": (
        "these days stopped on their own and can't pick back up without you — "
        "here's why, and what to try."
    ),
    "BACKLOG_DAY_BADGE": "stuck",
    "BACKLOG_REASON_CORRUPT_RAW": (
        "original recording is missing or damaged — re-import it"
    ),
    "BACKLOG_REASON_FAILING_STEP": "a processing step keeps failing — try again",
    "BACKLOG_REASON_MISSING_CONFIG": "a setting's missing — check solstone's setup",
    "BACKLOG_REASON_PROVIDER_DOWN": "the AI service was unreachable — try again",
    "BACKLOG_WHY_NEVER_ATTEMPTED": "not looked at yet",
    "BACKLOG_WHY_FAILED": "couldn't finish — will retry",
    "BACKLOG_WHY_SENSED_NOT_THOUGHT": "observed, not yet thought through",
    "BACKLOG_CATCHING_UP_DAY": "catching up",
    "BACKLOG_CATCHING_UP_AGGREGATE": "{pending_n} day(s) catching up",
    "BACKLOG_CATCHING_UP_TAIL": (
        "solstone's working through these on its own, freshest day first."
    ),
}


@pytest.fixture
def stats_env(tmp_path, monkeypatch):
    """Create a temporary journal for stats app testing."""

    def _create():
        journal = tmp_path / "journal"
        journal.mkdir(exist_ok=True)

        config_dir = journal / "config"
        config_dir.mkdir(parents=True, exist_ok=True)
        config_file = config_dir / "journal.json"
        config_file.write_text(
            json.dumps(
                {
                    "convey": {"trust_localhost": True},
                    "setup": {"completed_at": 1700000000000},
                },
                indent=2,
            ),
            encoding="utf-8",
        )

        monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

        from solstone.convey import create_app

        app = create_app(journal=str(journal))
        client = app.test_client()

        class Env:
            def __init__(self):
                self.journal = journal
                self.client = client
                self.app = app

        return Env()

    return _create


def _render_stats_workspace(stats_env) -> str:
    env = stats_env()
    response = env.client.get("/app/stats/")
    assert response.status_code == 200
    return response.get_data(as_text=True)


def test_backlog_copy_constants_are_literal():
    for key, value in BACKLOG_COPY_LITERALS.items():
        assert getattr(backlog_copy, key) == value


def test_backlog_copy_script_carries_all_keys(stats_env):
    rendered = _render_stats_workspace(stats_env)

    assert "window.BACKLOG_COPY" in rendered
    for key in BACKLOG_COPY_KEYS:
        js_key = key.removeprefix("BACKLOG_")
        assert f"{js_key}:" in rendered

    script_values = {}
    for key in BACKLOG_COPY_KEYS:
        js_key = key.removeprefix("BACKLOG_")
        match = re.search(rf"{js_key}:\s*(?P<value>\"(?:\\.|[^\"])*\")", rendered)
        assert match is not None, key
        script_values[key] = json.loads(match.group("value"))

    assert script_values == {
        key: getattr(backlog_copy, key) for key in BACKLOG_COPY_KEYS
    }


def test_backlog_both_plural_composes_from_locked_parts():
    assert backlog_copy.BACKLOG_VERDICT_BOTH_PLURAL == (
        backlog_copy.BACKLOG_VERDICT_STUCK_ONLY_PLURAL.rstrip(".")
        + " — "
        + backlog_copy.BACKLOG_VERDICT_BOTH_PLURAL.split(" — ")[1]
    )
