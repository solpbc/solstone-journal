# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import asyncio
import json
from datetime import datetime, timedelta, timezone
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from solstone.think import talents
from solstone.think.providers import brain_state as brain_state_module
from solstone.think.providers.brain_state import (
    begin_brain_refresh,
    finish_brain_refresh,
    inspect_brain_state,
)

NOW = datetime.now(timezone.utc)


def _install_native_brain_binary(monkeypatch: pytest.MonkeyPatch) -> None:
    binary = Path(__file__).resolve().parents[1] / "core/target/debug/solstone-core"
    assert binary.is_file()
    monkeypatch.setattr(brain_state_module, "_native_binary", lambda **_kwargs: binary)


def _write_brain_config(journal: Path) -> None:
    config_path = journal / "config" / "journal.json"
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text(
        json.dumps(
            {
                "providers": {
                    "active": {
                        "provider": "google",
                        "model": "gemini-3.5-flash",
                    }
                },
                "env": {"GOOGLE_API_KEY": "test-key"},
            }
        ),
        encoding="utf-8",
    )


def _ok_component() -> dict[str, str]:
    return {
        "status": "ok",
        "observed_at": NOW.isoformat(),
        "expires_at": (NOW + timedelta(days=1)).isoformat(),
    }


def _write_ready_brain(journal: Path) -> None:
    _write_brain_config(journal)
    permit = begin_brain_refresh(NOW, journal_path=journal)
    assert permit is not None
    finish_brain_refresh(
        permit,
        {
            "configuration": _ok_component(),
            "lane_prerequisites": _ok_component(),
            "generate": _ok_component(),
            "cogitate": _ok_component(),
        },
        NOW,
        journal_path=journal,
    )


def _cogitate_config(output_path: Path) -> dict[str, Any]:
    return {
        "provider": "google",
        "model": "gemini-3.5-flash",
        "type": "cogitate",
        "name": "test_cogitate",
        "prompt": "x",
        "output": "md",
        "output_path": str(output_path),
    }


def _generate_config(output_path: Path) -> dict[str, Any]:
    return {
        "provider": "google",
        "model": "gemini-3.5-flash",
        "name": "test_generate",
        "prompt": "x",
        "output": "md",
        "output_path": str(output_path),
    }


@pytest.mark.parametrize("event_kind", ["finish_blank", "agent_stuck"])
def test_cogitate_terminal_infrastructure_records_brain_failure(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    event_kind: str,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _install_native_brain_binary(monkeypatch)
    _write_ready_brain(tmp_path)
    original_record_failure = brain_state_module.record_brain_runtime_failure
    accepted: list[bool] = []

    def spy_record_brain_runtime_failure(*args: Any, **kwargs: Any) -> dict[str, Any]:
        result = original_record_failure(*args, **kwargs)
        accepted.append(result["accepted"])
        return result

    monkeypatch.setattr(
        brain_state_module,
        "record_brain_runtime_failure",
        spy_record_brain_runtime_failure,
    )

    async def run_cogitate(config: dict[str, Any], on_event: Any) -> str | None:
        del config
        if event_kind == "finish_blank":
            on_event({"event": "finish", "result": "   "})
            return "   "
        on_event(
            {
                "event": "error",
                "reason_code": "agent_stuck",
                "terminal": True,
                "result": "",
            }
        )
        return ""

    monkeypatch.setattr(
        "solstone.think.providers.get_provider_module",
        lambda _provider: SimpleNamespace(run_cogitate=run_cogitate),
    )
    events: list[dict[str, Any]] = []

    asyncio.run(
        talents._execute_with_tools(
            _cogitate_config(tmp_path / "out.md"), events.append
        )
    )

    assert accepted == [True]
    record = inspect_brain_state(NOW, journal_path=tmp_path)["record"]
    assert record is not None
    assert record["evidence"]["cogitate"]["reason_code"] == "cogitate_terminal_error"
    assert record["runtime_failure_marker"] is not None
    assert record["runtime_failure_marker"]["reason_code"] == "cogitate_terminal_error"
    assert events[-1]["event"] == "error"
    assert events[-1]["terminal"] is True


def test_generate_blank_after_post_hook_does_not_record_cogitate_terminal_error(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_brain_config(tmp_path)
    recorded: list[tuple[str, str]] = []
    monkeypatch.setattr(
        talents,
        "_record_brain_runtime_failure",
        lambda reason_code, component, **_kwargs: recorded.append(
            (reason_code, component)
        ),
    )

    def generate_with_result(*_args: Any, **_kwargs: Any) -> dict[str, Any]:
        return {
            "text": "   ",
            "model": "gemini-3.5-flash",
            "finish_reason": "stop",
            "usage": {"input_tokens": 1, "output_tokens": 0, "total_tokens": 1},
        }

    monkeypatch.setattr(
        "solstone.think.models.generate_with_result",
        generate_with_result,
    )
    events: list[dict[str, Any]] = []

    asyncio.run(
        talents._execute_generate(_generate_config(tmp_path / "out.md"), events.append)
    )

    assert recorded == []
    assert events[-1]["event"] == "error"
    assert events[-1]["reason_code"] == "no_output"
    assert events[-1]["terminal"] is True
