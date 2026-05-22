# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
from datetime import datetime

from typer.testing import CliRunner

from solstone.think.call import call_app

runner = CliRunner()


def test_list_outputs_activity_records(activities_env):
    activities_env(
        [
            {
                "id": "coding_090000_300",
                "activity": "coding",
                "title": "Focused coding",
                "description": "Implementing the CLI",
                "segments": ["090000_300"],
                "created_at": 1,
            }
        ]
    )

    result = runner.invoke(call_app, ["activities", "list", "--facet", "work"])

    assert result.exit_code == 0
    assert "Focused coding" in result.output
    assert "Activity: coding" in result.output


def test_list_defaults_to_today_without_day_or_env(activities_env, monkeypatch):
    today = datetime.now().strftime("%Y%m%d")
    activities_env(
        [
            {
                "id": "coding_090000_300",
                "activity": "coding",
                "title": "Today coding",
                "description": "Using today's default",
                "segments": ["090000_300"],
                "created_at": 1,
            }
        ],
        day=today,
    )
    monkeypatch.delenv("SOL_DAY", raising=False)

    result = runner.invoke(call_app, ["activities", "list", "--facet", "work"])

    assert result.exit_code == 0
    assert "Today coding" in result.output


def test_list_filters_hidden_by_default_and_allows_all(activities_env):
    activities_env(
        [
            {
                "id": "coding_090000_300",
                "activity": "coding",
                "description": "Visible",
                "segments": ["090000_300"],
                "created_at": 1,
            },
            {
                "id": "meeting_100000_300",
                "activity": "meeting",
                "description": "Muted",
                "segments": ["100000_300"],
                "created_at": 2,
                "hidden": True,
            },
        ]
    )

    hidden_default = runner.invoke(call_app, ["activities", "list", "--facet", "work"])
    hidden_all = runner.invoke(
        call_app, ["activities", "list", "--facet", "work", "--all", "--json"]
    )

    assert hidden_default.exit_code == 0
    assert "Visible" in hidden_default.output
    assert "Muted" not in hidden_default.output

    assert hidden_all.exit_code == 0
    payload = json.loads(hidden_all.output)
    assert len(payload) == 2
    assert any(item["hidden"] for item in payload)


def test_list_filters_by_entity(activities_env):
    activities_env(
        [
            {
                "id": "coding_090000_300",
                "activity": "coding",
                "description": "Entity match",
                "segments": ["090000_300"],
                "created_at": 1,
                "active_entities": ["Ada Lovelace"],
            },
            {
                "id": "meeting_100000_300",
                "activity": "meeting",
                "description": "Different entity",
                "segments": ["100000_300"],
                "created_at": 2,
                "active_entities": ["Grace Hopper"],
            },
        ]
    )

    result = runner.invoke(
        call_app,
        [
            "activities",
            "list",
            "--facet",
            "work",
            "--entity",
            "lovel",
            "--json",
        ],
    )

    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert [item["id"] for item in payload] == ["coding_090000_300"]


def test_list_filters_by_source(activities_env):
    activities_env(
        [
            {
                "id": "anticipated_call_103000_0421",
                "activity": "call",
                "title": "Mari intro",
                "description": "Planned",
                "target_date": "2026-04-21",
                "source": "anticipated",
                "created_at": 1,
            },
            {
                "id": "coding_090000_300",
                "activity": "coding",
                "title": "Focused coding",
                "description": "User created",
                "source": "user",
                "created_at": 2,
            },
            {
                "id": "meeting_100000_300",
                "activity": "meeting",
                "title": "Synthesized meeting",
                "description": "Cogitate created",
                "source": "cogitate",
                "created_at": 3,
            },
        ]
    )

    result = runner.invoke(
        call_app,
        ["activities", "list", "--facet", "work", "--source", "anticipated", "--json"],
    )

    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert [item["id"] for item in payload] == ["anticipated_call_103000_0421"]


def test_list_rejects_unknown_source(activities_env):
    activities_env([])

    result = runner.invoke(
        call_app,
        ["activities", "list", "--facet", "work", "--source", "calendar"],
    )

    assert result.exit_code == 1
    assert "--source must be 'anticipated', 'cogitate', or 'user'" in result.output


def test_get_returns_hidden_record_with_json_output(activities_env):
    activities_env(
        [
            {
                "id": "meeting_100000_300",
                "activity": "meeting",
                "description": "Muted",
                "segments": ["100000_300"],
                "created_at": 2,
                "hidden": True,
            }
        ]
    )

    result = runner.invoke(
        call_app,
        [
            "activities",
            "get",
            "meeting_100000_300",
            "--facet",
            "work",
            "--json",
        ],
    )

    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert payload["id"] == "meeting_100000_300"
    assert payload["hidden"] is True


def test_get_missing_exits_1(activities_env):
    activities_env([])

    result = runner.invoke(
        call_app,
        ["activities", "get", "missing_090000_300", "--facet", "work"],
    )

    assert result.exit_code == 1
    assert "activity not found: missing_090000_300" in result.output


def test_create_reads_json_from_stdin(activities_env):
    activities_env([])

    result = runner.invoke(
        call_app,
        ["activities", "create", "--facet", "work", "--json"],
        input=json.dumps({"title": "CLI created", "activity": "coding"}),
    )

    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert payload["title"] == "CLI created"
    assert payload["activity"] == "coding"
    assert payload["source"] == "user"
    assert payload["segments"] == []
    assert payload["id"].startswith("coding_user_")


def test_create_with_since_segment_and_cogitate_source(activities_env):
    activities_env([])

    result = runner.invoke(
        call_app,
        [
            "activities",
            "create",
            "--facet",
            "work",
            "--since-segment",
            "090000_300",
            "--source",
            "cogitate",
            "--json",
        ],
        input=json.dumps({"title": "LLM seeded", "activity": "coding"}),
    )

    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert payload["id"] == "coding_090000_300"
    assert payload["segments"] == ["090000_300"]
    assert payload["source"] == "cogitate"
    assert payload["edits"][-1]["actor"] == "cogitate:activities"


def test_update_applies_patch_and_default_note(activities_env):
    activities_env(
        [
            {
                "id": "coding_090000_300",
                "activity": "coding",
                "description": "Old description",
                "segments": ["090000_300"],
                "created_at": 1,
            }
        ]
    )

    result = runner.invoke(
        call_app,
        ["activities", "update", "coding_090000_300", "--facet", "work", "--json"],
        input=json.dumps({"details": "New details", "title": "Focused coding"}),
    )

    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert payload["title"] == "Focused coding"
    assert payload["details"] == "New details"
    assert payload["edits"][-1]["note"] == "updated fields: details, title"


def test_mute_and_unmute_toggle_hidden_state(activities_env):
    activities_env(
        [
            {
                "id": "coding_090000_300",
                "activity": "coding",
                "description": "Old description",
                "segments": ["090000_300"],
                "created_at": 1,
            }
        ]
    )

    muted = runner.invoke(
        call_app,
        [
            "activities",
            "mute",
            "coding_090000_300",
            "--facet",
            "work",
            "--reason",
            "noise",
            "--json",
        ],
    )
    unmuted = runner.invoke(
        call_app,
        [
            "activities",
            "unmute",
            "coding_090000_300",
            "--facet",
            "work",
            "--json",
        ],
    )

    assert muted.exit_code == 0
    assert json.loads(muted.output)["hidden"] is True

    assert unmuted.exit_code == 0
    unmuted_payload = json.loads(unmuted.output)
    assert unmuted_payload["hidden"] is False
    assert unmuted_payload["edits"][-1]["note"] == "unmuted"
