# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import asyncio
import json
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

import pytest

from solstone.think import talents
from solstone.think.providers import brain_state as brain_state_module
from solstone.think.providers.brain_state import (
    begin_brain_refresh,
    finish_brain_refresh,
    inspect_brain_state,
)
from solstone.think.providers.shared import QuotaExhaustedError

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


def _install_native_cogitate_event(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    event: dict[str, Any] | list[dict[str, Any]],
) -> None:
    """Point the thin client at a temporary native event executable."""
    binary = tmp_path / "fake-solstone-core-cogitate"
    events = event if isinstance(event, list) else [event]
    event_lines = "\n".join(f"print({json.dumps(item)!r}, flush=True)" for item in events)
    binary.write_text(
        "#!/usr/bin/env python3\n"
        "import sys\n"
        "sys.stdin.read()\n"
        f"{event_lines}\n",
        encoding="utf-8",
    )
    binary.chmod(0o700)
    monkeypatch.setattr(
        "solstone.think.cogitate_client._native_binary", lambda: binary
    )


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
        "solstone.think.cogitate_client.run_cogitate",
        run_cogitate,
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


def test_native_cogitate_finish_runs_post_hooks_and_persists_clean_output(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    output_path = tmp_path / "chronicle" / "20260810" / "talents" / "result.json"
    config = {
        **_cogitate_config(output_path),
        "day": "20260810",
        "schedule": "daily",
        "use_id": "use-1",
        "output": "json",
        "json_schema": {
            "type": "object",
            "properties": {"answer": {"type": "string"}},
            "required": ["answer"],
        },
        "degradation_check": True,
    }
    monkeypatch.setattr(
        talents,
        "load_post_hook",
        lambda _config: lambda _result, _context: '{"answer": "post-hook"}',
    )
    _install_native_cogitate_event(
        monkeypatch,
        tmp_path,
        [
            {"event": "start", "provider": "google"},
            {
                "event": "finish",
                "terminal": True,
                "result": '{"answer": "raw"}',
                "usage": {"output_tokens": talents.MIN_OUTPUT_TOKENS},
            },
        ],
    )
    events: list[dict[str, Any]] = []

    asyncio.run(talents._execute_with_tools(config, events.append))

    assert [event["event"] for event in events] == ["finish"]
    finish = events[0]
    assert finish["result"] == '{"answer": "post-hook"}'
    assert finish["cache_hit"] is False
    assert finish["output_changed"] is True
    assert isinstance(finish["completed_at_ms"], int)
    assert "degraded" not in finish
    assert output_path.read_text(encoding="utf-8") == '{"answer": "post-hook"}'
    assert talents.read_provenance(output_path) is not None


def test_native_cogitate_blank_expected_output_emits_no_output_error(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    recorded: list[tuple[str, str]] = []
    monkeypatch.setattr(
        talents,
        "_record_brain_runtime_failure",
        lambda reason_code, component, **_kwargs: recorded.append((reason_code, component)),
    )

    async def run_cogitate(config: dict[str, Any], on_event: Any) -> str:
        del config
        on_event({"event": "finish", "result": "   "})
        return "   "

    monkeypatch.setattr("solstone.think.cogitate_client.run_cogitate", run_cogitate)
    events: list[dict[str, Any]] = []

    asyncio.run(
        talents._execute_with_tools(
            _cogitate_config(tmp_path / "output.md"), events.append
        )
    )

    assert recorded == [("cogitate_terminal_error", "cogitate")]
    assert events[-1]["event"] == "error"
    assert events[-1]["reason_code"] == "no_output"
    assert events[-1]["terminal"] is True


def test_native_cogitate_nonblank_expected_output_does_not_emit_no_output_error(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    recorded: list[tuple[str, str]] = []
    monkeypatch.setattr(
        talents,
        "_record_brain_runtime_failure",
        lambda reason_code, component, **_kwargs: recorded.append((reason_code, component)),
    )

    async def run_cogitate(config: dict[str, Any], on_event: Any) -> str:
        del config
        on_event({"event": "finish", "result": "finished"})
        return "finished"

    monkeypatch.setattr("solstone.think.cogitate_client.run_cogitate", run_cogitate)
    events: list[dict[str, Any]] = []

    asyncio.run(
        talents._execute_with_tools(
            _cogitate_config(tmp_path / "output.md"), events.append
        )
    )

    assert recorded == []
    assert events == [
        {
            "event": "finish",
            "result": "finished",
            "cache_hit": False,
            "output_changed": True,
            "completed_at_ms": events[0]["completed_at_ms"],
        }
    ]


def test_local_cogitate_threads_admission_lease_to_native_client_and_releases_after_success(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.think.providers import local, local_admission, local_endpoint

    order: list[str] = []

    class Permit:
        slot_index = 0
        queue_wait_ms = 0.0

        def release(self) -> None:
            order.append("release")

    async def acquire(*_args: Any, **_kwargs: Any) -> Permit:
        order.append("acquire")
        return Permit()

    async def run_native(*_args: Any, context_window: int | None = None, **_kwargs: Any) -> str:
        assert context_window == 16_384
        order.append("native")
        return "done"

    monkeypatch.setattr(
        local,
        "resolve_local_endpoint",
        lambda: local_endpoint.LocalEndpoint(
            "http://endpoint.test", "local/test", None, False, parallel_slots=1
        ),
    )
    monkeypatch.setattr(local_admission, "acquire_local_slot_async", acquire)
    monkeypatch.setattr(
        local_endpoint, "resolve_endpoint_served_window", lambda _endpoint: 16_384
    )
    monkeypatch.setattr("solstone.think.cogitate_client.run_cogitate", run_native)

    assert asyncio.run(local.run_cogitate({"model": "local/test"})) == "done"
    assert order == ["acquire", "native", "release"]


def test_local_cogitate_threads_admission_lease_to_native_client_and_releases_after_failure(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.think.providers import local, local_admission, local_endpoint

    order: list[str] = []

    class Permit:
        slot_index = 0
        queue_wait_ms = 0.0

        def release(self) -> None:
            order.append("release")

    async def acquire(*_args: Any, **_kwargs: Any) -> Permit:
        order.append("acquire")
        return Permit()

    async def fail_native(*_args: Any, **_kwargs: Any) -> None:
        order.append("native")
        raise RuntimeError("native failed")

    monkeypatch.setattr(
        local,
        "resolve_local_endpoint",
        lambda: local_endpoint.LocalEndpoint(
            "http://endpoint.test", "local/test", None, False, parallel_slots=1
        ),
    )
    monkeypatch.setattr(local_admission, "acquire_local_slot_async", acquire)
    monkeypatch.setattr(
        local_endpoint, "resolve_endpoint_served_window", lambda _endpoint: None
    )
    monkeypatch.setattr("solstone.think.cogitate_client.run_cogitate", fail_native)

    with pytest.raises(RuntimeError, match="native failed"):
        asyncio.run(local.run_cogitate({"model": "local/test"}))
    assert order == ["acquire", "native", "release"]


def test_native_cogitate_quota_failure_reconstructs_quota_error_and_records_quota_failure(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    recorded: list[tuple[str, str]] = []
    monkeypatch.setattr(
        talents,
        "_record_brain_runtime_failure",
        lambda reason_code, component, **_kwargs: recorded.append((reason_code, component)),
    )
    _install_native_cogitate_event(
        monkeypatch,
        tmp_path,
        {
            "event": "error",
            "terminal": True,
            "error": "provider_quota_exceeded",
            "reason_code": "provider_quota_exceeded",
            "provider_failure": {
                "reason_code": "provider_quota_exceeded",
                "retryable": True,
                "blocking": False,
            },
            "usage": {"input_tokens": 1, "output_tokens": 0, "total_tokens": 1},
        },
    )
    events: list[dict[str, Any]] = []
    before = talents.now_ms()

    with pytest.raises(QuotaExhaustedError) as raised:
        asyncio.run(
            talents._execute_with_tools(
                _cogitate_config(tmp_path / "output.md"), events.append
            )
        )

    assert raised.value.retry_delay_ms is None
    assert recorded == [("provider_quota_exceeded", "cogitate")]
    assert events[-1]["reason_code"] == "provider_quota_exceeded"
    assert events[-1]["terminal"] is False
    assert events[-1]["reset_at_ms"] >= before


def test_native_cogitate_nonquota_provider_failure_does_not_reconstruct_or_record_quota_failure(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    recorded: list[tuple[str, str]] = []
    monkeypatch.setattr(
        talents,
        "_record_brain_runtime_failure",
        lambda reason_code, component, **_kwargs: recorded.append((reason_code, component)),
    )
    rejected_event = {
        "event": "error",
        "terminal": True,
        "error": "provider_request_rejected",
        "reason_code": "provider_request_rejected",
        "provider_failure": {
            "reason_code": "provider_request_rejected",
            "retryable": False,
            "blocking": False,
        },
        "usage": {"input_tokens": 1, "output_tokens": 0, "total_tokens": 1},
    }
    _install_native_cogitate_event(monkeypatch, tmp_path, rejected_event)
    events: list[dict[str, Any]] = []

    asyncio.run(
        talents._execute_with_tools(
            _cogitate_config(tmp_path / "output.md"), events.append
        )
    )

    assert recorded == []
    assert events == [rejected_event]


def test_native_cogitate_finish_preserves_usage_to_talent_stdout_ndjson(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    usage = {"input_tokens": 3, "output_tokens": 7, "model_version": "native"}

    async def run_cogitate(config: dict[str, Any], on_event: Any) -> str:
        del config
        on_event({"event": "finish", "result": "done", "usage": usage})
        return "done"

    monkeypatch.setattr("solstone.think.cogitate_client.run_cogitate", run_cogitate)
    writer = talents.JSONEventWriter()

    asyncio.run(
        talents._execute_with_tools(
            {**_cogitate_config(tmp_path / "output.md"), "output_path": None},
            writer.emit,
        )
    )

    event = json.loads(capsys.readouterr().out)
    assert event["event"] == "finish"
    assert event["usage"] == usage


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
