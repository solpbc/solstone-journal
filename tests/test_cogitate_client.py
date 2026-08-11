# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import asyncio
import json
import subprocess
from pathlib import Path

import pytest

from solstone.think import cogitate_client
from solstone.think.core_handshake import CoreHandshakeResult


def _cogitate_config() -> dict[str, str]:
    return {
        "provider": "google",
        "model": "gemini-test",
        "prompt": "Do the task.",
    }


def _assert_terminal_error(events: list[dict], reason_code: str) -> None:
    assert events[-1]["event"] == "error"
    assert events[-1]["terminal"] is True
    assert events[-1]["reason_code"] == reason_code


def test_load_talent_contract_uses_native_contract_once(monkeypatch) -> None:
    calls: list[tuple[list[str], dict[str, object]]] = []

    def native_binary() -> Path:
        return Path("/native/solstone-core")

    def run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        calls.append((command, kwargs))
        return subprocess.CompletedProcess(
            command,
            0,
            json.dumps(
                {
                    "tiers": [
                        {
                            "name": "normal",
                            "talent_facing": True,
                        }
                    ]
                }
            ),
            "",
        )

    monkeypatch.setattr(cogitate_client, "_native_binary", native_binary)
    monkeypatch.setattr(cogitate_client.subprocess, "run", run)
    cogitate_client.load_talent_contract.cache_clear()

    expected = {"tiers": [{"name": "normal", "talent_facing": True}]}
    assert cogitate_client.load_talent_contract() == expected
    assert cogitate_client.load_talent_contract() == expected
    assert calls == [
        (
            ["/native/solstone-core", "cogitate", "--talent-contract"],
            {
                "input": None,
                "text": True,
                "capture_output": True,
                "check": False,
            },
        )
    ]


def test_render_dry_run_details_extracts_native_prompt_and_finalization(monkeypatch) -> None:
    captured: dict[str, object] = {}

    def native_binary() -> Path:
        return Path("/native/solstone-core")

    def run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        captured["command"] = command
        captured.update(kwargs)
        return subprocess.CompletedProcess(
            command,
            0,
            json.dumps(
                {
                    "event": "dry_run",
                    "terminal": True,
                    "rendered_prompt": {
                        "initial_prompt": "Summarize today.",
                        "system_instruction": "Use the native tools.",
                    },
                    "expects_emit_final": True,
                }
            )
            + "\n",
            "",
        )

    monkeypatch.setattr(cogitate_client, "_native_binary", native_binary)
    monkeypatch.setattr(cogitate_client.subprocess, "run", run)

    assert cogitate_client.render_dry_run_details(
        {
            "model": "gpt-5",
            "prompt": "Summarize today.",
            "session_id": "dry-run-test",
        }
    ) == {
        "initial_prompt": "Summarize today.",
        "system_instruction": "Use the native tools.",
        "expects_emit_final": True,
    }
    assert captured["command"] == [
        "/native/solstone-core",
        "cogitate",
        "--one-shot",
    ]
    request = json.loads(str(captured["input"]))
    assert request["dry_run"] is True
    assert request["initial_prompt"] == "Summarize today."


def test_run_cogitate_handshake_failure_emits_terminal_error(monkeypatch) -> None:
    cogitate_client._native_binary.cache_clear()
    monkeypatch.setattr(
        cogitate_client.core_handshake,
        "check_solstone_core_handshake",
        lambda: CoreHandshakeResult("fail", "handshake failed"),
    )
    events: list[dict] = []

    with pytest.raises(RuntimeError, match="handshake failed"):
        asyncio.run(cogitate_client.run_cogitate(_cogitate_config(), events.append))

    _assert_terminal_error(events, "native_runtime_unavailable")


def test_run_cogitate_missing_binary_emits_terminal_error(
    tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.setattr(cogitate_client, "_native_binary", lambda: tmp_path / "missing")
    events: list[dict] = []

    with pytest.raises(RuntimeError, match="could not start"):
        asyncio.run(cogitate_client.run_cogitate(_cogitate_config(), events.append))

    _assert_terminal_error(events, "native_runtime_unavailable")


def test_run_cogitate_spawn_failure_emits_terminal_error(monkeypatch) -> None:
    async def fail_spawn(*_args, **_kwargs):
        raise OSError("spawn refused")

    monkeypatch.setattr(cogitate_client, "_native_binary", lambda: Path("/native/core"))
    monkeypatch.setattr(cogitate_client.asyncio, "create_subprocess_exec", fail_spawn)
    events: list[dict] = []

    with pytest.raises(RuntimeError, match="could not start"):
        asyncio.run(cogitate_client.run_cogitate(_cogitate_config(), events.append))

    _assert_terminal_error(events, "native_runtime_unavailable")


def test_run_cogitate_write_failure_emits_terminal_error(monkeypatch) -> None:
    class BrokenStdin:
        def write(self, _data: bytes) -> None:
            raise BrokenPipeError("stdin closed")

        async def drain(self) -> None:
            raise AssertionError("write failure must skip drain")

        def close(self) -> None:
            raise AssertionError("write failure must skip close")

    class BrokenProcess:
        stdin = BrokenStdin()
        stdout = None
        stderr = None
        returncode = 0

    async def spawn(*_args, **_kwargs):
        return BrokenProcess()

    monkeypatch.setattr(cogitate_client, "_native_binary", lambda: Path("/native/core"))
    monkeypatch.setattr(cogitate_client.asyncio, "create_subprocess_exec", spawn)
    events: list[dict] = []

    with pytest.raises(BrokenPipeError, match="stdin closed"):
        asyncio.run(cogitate_client.run_cogitate(_cogitate_config(), events.append))

    _assert_terminal_error(events, "native_runtime_incomplete")


def test_run_cogitate_midstream_nonzero_exit_emits_terminal_error(
    tmp_path: Path, monkeypatch
) -> None:
    binary = tmp_path / "fake-solstone-core"
    binary.write_text(
        "#!/usr/bin/env python3\n"
        "import json\n"
        "import sys\n"
        "sys.stdin.read()\n"
        "print(json.dumps({'event': 'delta', 'delta': 'partial'}), flush=True)\n"
        "sys.exit(17)\n",
        encoding="utf-8",
    )
    binary.chmod(0o700)
    monkeypatch.setattr(cogitate_client, "_native_binary", lambda: binary)
    events: list[dict] = []

    with pytest.raises(RuntimeError, match="without a terminal event"):
        asyncio.run(cogitate_client.run_cogitate(_cogitate_config(), events.append))

    assert events[0] == {"event": "delta", "delta": "partial"}
    _assert_terminal_error(events, "native_runtime_incomplete")
