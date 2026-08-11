# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import subprocess
from pathlib import Path

from solstone.think import cogitate_client


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


def test_render_dry_run_prompt_extracts_native_rendered_prompt(monkeypatch) -> None:
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
                }
            )
            + "\n",
            "",
        )

    monkeypatch.setattr(cogitate_client, "_native_binary", native_binary)
    monkeypatch.setattr(cogitate_client.subprocess, "run", run)

    assert cogitate_client.render_dry_run_prompt(
        {
            "model": "gpt-5",
            "prompt": "Summarize today.",
            "session_id": "dry-run-test",
        }
    ) == {
        "initial_prompt": "Summarize today.",
        "system_instruction": "Use the native tools.",
    }
    assert captured["command"] == [
        "/native/solstone-core",
        "cogitate",
        "--one-shot",
    ]
    request = json.loads(str(captured["input"]))
    assert request["dry_run"] is True
    assert request["initial_prompt"] == "Summarize today."
