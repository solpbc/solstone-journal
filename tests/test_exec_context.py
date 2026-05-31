# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import importlib.util
import json
from pathlib import Path

TEMPLATE_VAR_KEYS = frozenset({"active_routines", "routine_suggestion"})


def _load_exec_context_module():
    path = (
        Path(__file__).resolve().parents[1] / "solstone" / "talent" / "exec_context.py"
    )
    spec = importlib.util.spec_from_file_location("test_exec_context_local", path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _load_chat_context_module():
    path = (
        Path(__file__).resolve().parents[1] / "solstone" / "talent" / "chat_context.py"
    )
    spec = importlib.util.spec_from_file_location("test_chat_context_local", path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _assert_template_vars_result(result):
    assert set(result.keys()) == {"template_vars"}
    assert set(result["template_vars"].keys()) == TEMPLATE_VAR_KEYS
    return result["template_vars"]


def _write_journal_config(journal: Path, data: dict) -> None:
    config_dir = journal / "config"
    config_dir.mkdir(parents=True, exist_ok=True)
    (config_dir / "journal.json").write_text(
        json.dumps(data, indent=2),
        encoding="utf-8",
    )


def test_exec_pre_process_populated_state(monkeypatch, tmp_path):
    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    _write_journal_config(
        journal, {"agent": {"name": "Sol-agent", "name_status": "custom"}}
    )

    monkeypatch.setattr(
        "solstone.think.routines.get_routine_state",
        lambda: [
            {
                "name": "Morning Briefing",
                "cadence": "0 9 * * *",
                "last_run": None,
                "enabled": True,
                "paused_until": None,
                "output_summary": "Shared the top priorities.",
            }
        ],
    )
    monkeypatch.setattr(
        "solstone.think.routines.get_config",
        lambda: {
            "_meta": {
                "suggestions_enabled": True,
                "suggestions": {
                    "meeting-prep": {
                        "trigger_count": 3,
                        "first_trigger": "2026-04-01",
                        "last_trigger": "2026-04-19",
                        "trigger_data": {},
                        "response": None,
                        "suggested": False,
                    }
                },
            }
        },
    )
    monkeypatch.setattr("solstone.think.routines.save_config", lambda config: None)

    result = _load_exec_context_module().pre_process({"day": "20260420"})

    template_vars = _assert_template_vars_result(result)
    assert "## Active Routines" in template_vars["active_routines"]
    assert "Morning Briefing" in template_vars["active_routines"]
    assert "## Routine Suggestion Eligible" in template_vars["routine_suggestion"]
    assert "meeting-prep" in template_vars["routine_suggestion"]
    assert (
        "journal routines suggest-respond meeting-prep --accepted"
        in template_vars["routine_suggestion"]
    )


def test_exec_pre_process_empty_state(monkeypatch, tmp_path):
    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    _write_journal_config(
        journal, {"agent": {"name": "Sol-agent", "name_status": "custom"}}
    )

    monkeypatch.setattr("solstone.think.routines.get_routine_state", lambda: [])
    monkeypatch.setattr(
        "solstone.think.routines.get_config",
        lambda: {"_meta": {"suggestions_enabled": True, "suggestions": {}}},
    )
    monkeypatch.setattr("solstone.think.routines.save_config", lambda config: None)

    result = _load_exec_context_module().pre_process({"day": "20260420"})

    template_vars = _assert_template_vars_result(result)
    assert template_vars["active_routines"] == ""
    assert template_vars["routine_suggestion"] == ""


def test_exec_pre_process_errors_swallowed(monkeypatch, tmp_path):
    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    _write_journal_config(
        journal, {"agent": {"name": "Sol-agent", "name_status": "custom"}}
    )

    module = _load_exec_context_module()

    def _boom(*_args, **_kwargs):
        raise RuntimeError("boom")

    monkeypatch.setattr("solstone.think.routines.get_routine_state", _boom)
    monkeypatch.setattr(
        "solstone.think.routines.get_config",
        lambda: {"_meta": {"suggestions_enabled": True, "suggestions": {}}},
    )
    monkeypatch.setattr("solstone.think.routines.save_config", _boom)

    result = module.pre_process({"day": "20260420"})
    template_vars = _assert_template_vars_result(result)
    assert template_vars["active_routines"] == ""

    monkeypatch.setattr("solstone.think.routines.get_routine_state", lambda: [])
    monkeypatch.setattr("solstone.think.routines.get_config", _boom)

    result = module.pre_process({"day": "20260420"})
    template_vars = _assert_template_vars_result(result)
    assert template_vars["routine_suggestion"] == ""


def test_exec_pre_process_returned_dict_shape(monkeypatch, tmp_path):
    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    monkeypatch.setattr("solstone.think.routines.get_routine_state", lambda: [])
    monkeypatch.setattr(
        "solstone.think.routines.get_config",
        lambda: {"_meta": {"suggestions_enabled": False, "suggestions": {}}},
    )
    monkeypatch.setattr("solstone.think.routines.save_config", lambda config: None)

    result = _load_exec_context_module().pre_process({"day": "20260420"})

    assert set(result.keys()) == {"template_vars"}
    assert set(result["template_vars"].keys()) == TEMPLATE_VAR_KEYS


def test_exec_pre_process_never_calls_save_config(monkeypatch, tmp_path):
    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    _write_journal_config(
        journal, {"agent": {"name": "Sol-agent", "name_status": "custom"}}
    )

    def _fail_save(*_args, **_kwargs):
        raise AssertionError("save_config should not be called")

    monkeypatch.setattr("solstone.think.routines.get_routine_state", lambda: [])
    monkeypatch.setattr(
        "solstone.think.routines.get_config",
        lambda: {"_meta": {"suggestions_enabled": True, "suggestions": {}}},
    )
    monkeypatch.setattr("solstone.think.routines.save_config", _fail_save)

    module = _load_exec_context_module()
    owner_result = module.pre_process(
        {
            "prompt": "What is on my calendar today?",
            "trigger_kind": "owner_message",
            "trigger_payload": {"text": "What is on my calendar today?"},
        }
    )
    talent_result = module.pre_process(
        {
            "trigger_kind": "talent_finished",
            "trigger_payload": {"name": "exec", "summary": "Done."},
        }
    )

    _assert_template_vars_result(owner_result)
    _assert_template_vars_result(talent_result)


def test_exec_and_chat_render_identical_routine_vars(monkeypatch, tmp_path):
    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    _write_journal_config(
        journal, {"agent": {"name": "Sol-agent", "name_status": "custom"}}
    )

    routine_state = [
        {
            "name": "Morning Briefing",
            "cadence": "0 9 * * *",
            "last_run": None,
            "enabled": True,
            "paused_until": None,
            "output_summary": "Shared the top priorities.",
        }
    ]
    routines_config = {
        "_meta": {
            "suggestions_enabled": True,
            "suggestions": {
                "meeting-prep": {
                    "trigger_count": 3,
                    "first_trigger": "2026-04-01",
                    "last_trigger": "2026-04-19",
                    "trigger_data": {},
                    "response": None,
                    "suggested": False,
                }
            },
        }
    }

    def _fail_save(*_args, **_kwargs):
        raise AssertionError("save_config should not be called")

    monkeypatch.setattr(
        "solstone.think.routines.get_routine_state", lambda: routine_state
    )
    monkeypatch.setattr("solstone.think.routines.get_config", lambda: routines_config)
    monkeypatch.setattr("solstone.think.routines.save_config", _fail_save)

    context = {"day": "20260420"}
    chat_result = _load_chat_context_module().pre_process(context)
    exec_result = _load_exec_context_module().pre_process(context)

    assert (
        chat_result["template_vars"]["active_routines"]
        == exec_result["template_vars"]["active_routines"]
    )
    assert (
        chat_result["template_vars"]["routine_suggestion"]
        == exec_result["template_vars"]["routine_suggestion"]
    )
