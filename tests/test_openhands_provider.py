# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from types import SimpleNamespace

import pytest

from solstone.think.cogitate_policy import MAX_TURNS
from solstone.think.providers import openhands
from solstone.think.providers.shared import USAGE_KEYS, JSONEventCallback
from tests.openhands_fakes import install_fake_openhands


@pytest.fixture
def fake_openhands(monkeypatch):
    return install_fake_openhands(monkeypatch)


@pytest.fixture
def fixed_time(monkeypatch):
    monkeypatch.setattr(openhands, "now_ms", lambda: 123456)


def _translator(fake_openhands, events: list[dict]) -> openhands._OpenHandsTranslator:
    return openhands._OpenHandsTranslator(
        callback=JSONEventCallback(events.append),
        provider="openai",
        model="openai/gpt-5",
    )


def test_translator_maps_thinking_sources(fake_openhands, fixed_time):
    events: list[dict] = []
    translator = _translator(fake_openhands, events)

    translator.on_event(
        fake_openhands.ActionEvent(
            reasoning_content="reasoning summary",
            thinking_blocks=[],
            responses_reasoning_item=None,
            tool_name="",
        )
    )
    translator.on_event(
        fake_openhands.ActionEvent(
            reasoning_content=None,
            thinking_blocks=[
                SimpleNamespace(thinking="signed thinking", signature="sig-1")
            ],
            responses_reasoning_item=None,
            tool_name="",
        )
    )
    translator.on_event(
        fake_openhands.ActionEvent(
            reasoning_content=None,
            thinking_blocks=[],
            responses_reasoning_item=SimpleNamespace(
                summary=[SimpleNamespace(text="responses reasoning")],
                encrypted_content="encrypted",
            ),
            tool_name="",
        )
    )

    assert [{key: event[key] for key in event if key != "raw"} for event in events] == [
        {
            "event": "thinking",
            "summary": "reasoning summary",
            "model": "openai/gpt-5",
            "signature": None,
            "redacted_data": None,
            "ts": 123456,
        },
        {
            "event": "thinking",
            "summary": "signed thinking",
            "model": "openai/gpt-5",
            "signature": "sig-1",
            "redacted_data": None,
            "ts": 123456,
        },
        {
            "event": "thinking",
            "summary": "responses reasoning",
            "model": "openai/gpt-5",
            "signature": None,
            "redacted_data": "encrypted",
            "ts": 123456,
        },
    ]
    assert events[0]["raw"][0]["reasoning_content"] == "reasoning summary"


def test_translator_maps_tool_start_and_paired_tool_end(fake_openhands, fixed_time):
    events: list[dict] = []
    translator = _translator(fake_openhands, events)
    command = "sol call journal search x"

    translator.on_event(
        fake_openhands.ActionEvent(
            reasoning_content=None,
            thinking_blocks=[],
            responses_reasoning_item=None,
            tool_name="sol",
            tool_call=SimpleNamespace(arguments=f'{{"command":"{command}"}}'),
            tool_call_id="c1",
            action=None,
        )
    )
    translator.on_event(
        fake_openhands.ObservationEvent(
            tool_name="wrong-if-unpaired",
            tool_call_id="c1",
            observation=fake_openhands.Observation.from_text("tool output"),
        )
    )

    assert events[0]["event"] == "tool_start"
    assert events[0]["tool"] == "sol"
    assert events[0]["args"] == {"command": command}
    assert events[0]["call_id"] == "c1"
    assert events[0]["ts"] == 123456
    assert events[0]["raw"][0]["tool_name"] == "sol"

    assert events[1] == {
        "event": "tool_end",
        "tool": "sol",
        "args": {"command": command},
        "result": "tool output",
        "call_id": "c1",
        "raw": events[1]["raw"],
        "ts": 123456,
    }


def test_translator_records_finish_action_without_tool_start(
    fake_openhands,
    fixed_time,
):
    events: list[dict] = []
    translator = _translator(fake_openhands, events)

    translator.on_event(
        fake_openhands.ActionEvent(
            reasoning_content=None,
            thinking_blocks=[],
            responses_reasoning_item=None,
            tool_name="finish",
            tool_call=SimpleNamespace(arguments='{"message":"done"}'),
            tool_call_id="finish-1",
            action=SimpleNamespace(message="done"),
        )
    )

    assert events == []
    assert translator.finish_message == "done"
    assert translator.result() == "done"


def test_translator_maps_text_delta_tokens(fake_openhands, fixed_time):
    events: list[dict] = []
    translator = _translator(fake_openhands, events)

    translator.on_token(
        SimpleNamespace(choices=[SimpleNamespace(delta=SimpleNamespace(content="hi"))])
    )
    translator.on_token(
        SimpleNamespace(choices=[SimpleNamespace(delta=SimpleNamespace(content=None))])
    )

    assert events == [
        {
            "event": "text_delta",
            "delta": "hi",
            "model": "openai/gpt-5",
            "ts": 123456,
        }
    ]


def test_translator_result_prefers_finish_message(fake_openhands, fixed_time):
    events: list[dict] = []
    translator = _translator(fake_openhands, events)

    translator.on_event(
        fake_openhands.MessageEvent(
            source="agent",
            llm_message=SimpleNamespace(
                content=[SimpleNamespace(text="message result")]
            ),
        )
    )
    assert translator.final_message == "message result"
    assert translator.result() == "message result"

    translator.on_event(
        fake_openhands.ActionEvent(
            reasoning_content=None,
            thinking_blocks=[],
            responses_reasoning_item=None,
            tool_name="finish",
            tool_call=SimpleNamespace(arguments='{"message":"finish result"}'),
            tool_call_id="finish-1",
            action=SimpleNamespace(message="finish result"),
        )
    )
    assert translator.result() == "finish result"


def test_translator_maps_max_turns_once(fake_openhands, fixed_time):
    events: list[dict] = []
    translator = _translator(fake_openhands, events)

    translator.on_event(
        fake_openhands.ConversationErrorEvent(
            code="MaxIterationsReached",
            detail="limit",
        )
    )
    translator.on_event(
        fake_openhands.ConversationErrorEvent(
            code="MaxIterationsReached",
            detail="limit",
        )
    )
    translator.on_event(
        fake_openhands.ConversationErrorEvent(code="Other", detail="ignored")
    )

    assert translator.max_turns_exhausted is True
    assert events == [
        {
            "event": "max_turns_exhausted",
            "max_turns": MAX_TURNS,
            "ts": 123456,
        }
    ]


def test_usage_delta_is_normalized_delta():
    class Usage:
        prompt_tokens = 10
        completion_tokens = 20
        cache_read_tokens = 3
        cache_write_tokens = 4
        reasoning_tokens = 5

    class Metrics:
        accumulated_token_usage = Usage()
        token_usages = [object()]

    llm = SimpleNamespace(metrics=Metrics())
    start = openhands._usage_snapshot(llm)

    Usage.prompt_tokens = 15
    Usage.completion_tokens = 29
    Usage.cache_read_tokens = 8
    Usage.cache_write_tokens = 10
    Usage.reasoning_tokens = 12
    Metrics.token_usages = [object(), object(), object()]

    usage = openhands._usage_delta(start, llm)

    assert set(usage) == USAGE_KEYS
    assert usage == {
        "input_tokens": 5,
        "output_tokens": 9,
        "cached_tokens": 5,
        "cache_creation_tokens": 6,
        "reasoning_tokens": 7,
        "requests": 2,
        "total_tokens": 14,
    }
