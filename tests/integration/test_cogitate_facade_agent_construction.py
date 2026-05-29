# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Integration test: real openhands-sdk Agent construction with the façade's tools wiring.

5/23 regression `req_y2pt6j52`: the façade passed `ToolDefinition` instances to
`Agent(...)`, but `openhands-sdk==1.23.0` types `Agent.tools` as `list[Tool]` (a
registry spec). All four-backend smoke runs hit a pydantic ValidationError before
any LLM call. The 62 façade unit tests passed because `tests/openhands_fakes.py`
stubs `Agent` with a permissive FakeModel that does not enforce the real schema.

This test exists so the next refactor cannot regress the same way: it constructs
a real `Agent(...)` with the façade's actual register-and-spec wiring against the
installed openhands-sdk. No LLM call is made — pydantic schema validation alone
catches the regression.
"""

from __future__ import annotations

import pytest

from solstone.think.cogitate_policy import CogitatePolicy
from solstone.think.providers import emit_final_tool
from solstone.think.providers import openhands as facade
from solstone.think.providers.shared import JSONEventCallback

pytestmark = [pytest.mark.integration]


def test_real_openhands_agent_accepts_facade_tools_wiring(tmp_path):
    """Façade's register_tool + Tool(name=) wiring must satisfy real Agent schema."""
    sdk = pytest.importorskip(
        "openhands.sdk",
        reason="openhands-sdk baseline dependency is not installed",
    )
    registry = pytest.importorskip("openhands.sdk.tool.registry")
    spec = pytest.importorskip("openhands.sdk.tool.spec")

    events: list[dict] = []
    policy = CogitatePolicy(write=False, allowed_roots=[tmp_path])
    sol_tools, _executor = facade._build_sol_tools(
        policy=policy,
        callback=JSONEventCallback(events.append),
        write=False,
        read_call_budget=10,
    )
    assert len(sol_tools) == 1
    assert sol_tools[0].name == "sol"

    registry.register_tool("sol", sol_tools[0])

    llm = sdk.LLM(model="openai/gpt-5", api_key="EMPTY", native_tool_calling=True)
    # Pydantic validates tools at Agent.__init__; this is the line that the
    # 5/23 smoke caught as ValidationError under the wrong (instance) wiring.
    agent = sdk.Agent(
        llm=llm,
        tools=[spec.Tool(name="sol")],
        include_default_tools=["FinishTool"],
        system_prompt="probe",
    )

    assert [t.name for t in agent.tools] == ["sol"]
    assert "FinishTool" in agent.include_default_tools


def test_emit_final_branch_agent_construction(tmp_path):
    """Emit-final branch must satisfy real Agent schema without FinishTool."""
    sdk = pytest.importorskip(
        "openhands.sdk",
        reason="openhands-sdk baseline dependency is not installed",
    )
    registry = pytest.importorskip("openhands.sdk.tool.registry")
    spec = pytest.importorskip("openhands.sdk.tool.spec")

    events: list[dict] = []
    policy = CogitatePolicy(write=False, allowed_roots=[tmp_path])
    sol_tools, _executor = facade._build_sol_tools(
        policy=policy,
        callback=JSONEventCallback(events.append),
        write=False,
        read_call_budget=10,
    )
    emit_final_tools = emit_final_tool.build_emit_final_tools()

    registry.register_tool("sol", sol_tools[0])
    registry.register_tool("emit_final", emit_final_tools[0])

    llm = sdk.LLM(model="openai/gpt-5", api_key="EMPTY", native_tool_calling=True)
    agent = sdk.Agent(
        llm=llm,
        tools=[spec.Tool(name="sol"), spec.Tool(name="emit_final")],
        include_default_tools=[],
        system_prompt="probe",
    )

    assert [t.name for t in agent.tools] == ["sol", "emit_final"]
    assert agent.include_default_tools == []
