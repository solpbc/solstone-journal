# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import pytest

from solstone.think.cogitate_policy import CogitatePolicy
from solstone.think.providers import openhands
from solstone.think.providers.shared import JSONEventCallback
from tests.openhands_fakes import install_fake_openhands


@pytest.fixture
def fake_openhands(monkeypatch):
    return install_fake_openhands(monkeypatch)


@pytest.fixture
def fixed_time(monkeypatch):
    monkeypatch.setattr(openhands, "now_ms", lambda: 123456)


def _sol_tool_and_executor(
    *,
    tmp_path,
    events: list[dict],
    write: bool,
    read_call_budget: int = 200,
):
    policy = CogitatePolicy(write=write, allowed_roots=[tmp_path])
    tools, executor = openhands._build_sol_tools(
        policy=policy,
        callback=JSONEventCallback(events.append),
        write=write,
        read_call_budget=read_call_budget,
    )
    assert len(tools) == 1
    assert tools[0].name == "sol"
    return tools[0], executor


def test_read_only_allowed_sol_call_returns_non_error_observation(
    fake_openhands,
    fixed_time,
    tmp_path,
    monkeypatch,
):
    events: list[dict] = []
    tool, executor = _sol_tool_and_executor(
        tmp_path=tmp_path,
        events=events,
        write=False,
    )
    monkeypatch.setattr(
        openhands,
        "_run_shell_command",
        lambda command: {"text": f"ran: {command}", "is_error": False},
    )

    observation = tool(
        tool.action_from_arguments({"command": "sol call journal search x"})
    )

    assert observation.text == "ran: sol call journal search x"
    assert observation.is_error is False
    assert executor.read_call_count == 1
    assert events == []


def test_read_only_policy_deny_is_recoverable_observation(
    fake_openhands,
    fixed_time,
    tmp_path,
    monkeypatch,
):
    events: list[dict] = []
    tool, executor = _sol_tool_and_executor(
        tmp_path=tmp_path,
        events=events,
        write=False,
    )
    monkeypatch.setattr(
        openhands,
        "_run_shell_command",
        lambda _command: pytest.fail("denied commands must not run"),
    )

    observation = tool(tool.action_from_arguments({"command": "rm -rf journal"}))

    assert observation.is_error is True
    assert observation.text.startswith("policy_deny:")
    assert executor.read_call_count == 0
    assert events == []


def test_write_mode_allows_non_sol_shell_command(
    fake_openhands,
    fixed_time,
    tmp_path,
    monkeypatch,
):
    events: list[dict] = []
    tool, executor = _sol_tool_and_executor(
        tmp_path=tmp_path,
        events=events,
        write=True,
    )
    monkeypatch.setattr(
        openhands,
        "_run_shell_command",
        lambda command: {"text": f"write ran: {command}", "is_error": False},
    )

    observation = tool(tool.action_from_arguments({"command": "python -V"}))

    assert observation.text == "write ran: python -V"
    assert observation.is_error is False
    assert executor.read_call_count == 0
    assert events == []


def test_read_call_budget_overflow_emits_once_and_denies_recoverably(
    fake_openhands,
    fixed_time,
    tmp_path,
    monkeypatch,
):
    events: list[dict] = []
    tool, executor = _sol_tool_and_executor(
        tmp_path=tmp_path,
        events=events,
        write=False,
        read_call_budget=1,
    )
    monkeypatch.setattr(
        openhands,
        "_run_shell_command",
        lambda command: {"text": f"ran: {command}", "is_error": False},
    )
    action = tool.action_from_arguments({"command": "sol call journal search x"})

    first = tool(action)
    second = tool(action)
    third = tool(action)

    assert first.is_error is False
    assert first.text == "ran: sol call journal search x"
    assert second.is_error is True
    assert second.text.startswith("tool_budget_exhausted:")
    assert third.is_error is True
    assert third.text.startswith("tool_budget_exhausted:")
    assert executor.read_call_count == 3
    assert events == [
        {
            "event": "tool_budget_exhausted",
            "tool": "sol",
            "budget": 1,
            "count": 2,
            "ts": 123456,
        }
    ]
