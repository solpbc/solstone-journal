# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import asyncio
from types import SimpleNamespace

import httpx
import openai
import pytest

from solstone.think.cogitate_policy import MaxTurnsExhausted
from solstone.think.providers import openhands
from solstone.think.providers.cli import QuotaExhaustedError
from tests.openhands_fakes import install_fake_openhands


@pytest.fixture
def fake_openhands(monkeypatch):
    return install_fake_openhands(monkeypatch)


@pytest.fixture
def run_env(monkeypatch, tmp_path):
    monkeypatch.setattr(openhands, "get_journal", lambda: tmp_path)
    monkeypatch.setattr(openhands, "get_project_root", lambda: tmp_path)
    monkeypatch.setattr(openhands, "now_ms", lambda: 123456)
    monkeypatch.setenv("OPENAI_API_KEY", "test-key")
    return {
        "provider": "openai",
        "model": "gpt-5",
        "prompt": "Do the work.",
        "session_id": "11111111-1111-1111-1111-111111111111",
        "day": "20260522",
    }


def _openai_response(status_code: int, headers: dict[str, str] | None = None):
    request = httpx.Request("POST", "https://api.openai.com/v1/responses")
    return httpx.Response(status_code, request=request, headers=headers or {})


def test_retry_delay_ms_reads_retry_after_seconds_header():
    exc = SimpleNamespace(
        response=SimpleNamespace(headers={"retry-after": "30"}),
    )

    assert openhands._retry_delay_ms(exc) == 30000


def test_retry_delay_ms_returns_none_without_header():
    exc = SimpleNamespace(response=SimpleNamespace(headers={}))

    assert openhands._retry_delay_ms(exc) is None


def test_unwrap_provider_exception_prefers_cause():
    provider_exc = RuntimeError("provider")
    wrapper = RuntimeError("wrapper")
    wrapper.__cause__ = provider_exc

    assert openhands._unwrap_provider_exception(wrapper) is provider_exc


def test_unwrap_provider_exception_uses_context_without_cause():
    provider_exc = RuntimeError("provider")
    wrapper = RuntimeError("wrapper")
    wrapper.__context__ = provider_exc

    assert openhands._unwrap_provider_exception(wrapper) is provider_exc


def test_run_cogitate_quota_path_raises_quota_without_error_event(
    fake_openhands,
    run_env,
):
    quota_exc = openai.RateLimitError(
        "rate limited",
        response=_openai_response(429, {"retry-after": "30"}),
        body={},
    )

    async def fail(_conversation):
        raise quota_exc

    fake_openhands.Conversation.arun_impl = fail
    events: list[dict] = []

    with pytest.raises(QuotaExhaustedError) as raised:
        asyncio.run(openhands.run_cogitate(run_env, events.append))

    assert raised.value.retry_delay_ms == 30000
    assert events == []


def test_run_cogitate_generic_error_emits_event_and_marks_evented(
    fake_openhands,
    run_env,
):
    generic_exc = RuntimeError("boom")

    async def fail(_conversation):
        raise generic_exc

    fake_openhands.Conversation.arun_impl = fail
    events: list[dict] = []

    with pytest.raises(RuntimeError) as raised:
        asyncio.run(openhands.run_cogitate(run_env, events.append))

    assert raised.value is generic_exc
    assert getattr(generic_exc, "_evented") is True
    assert len(events) == 1
    assert events[0]["event"] == "error"
    assert events[0]["error"] == "boom"
    assert events[0]["reason_code"] == "unknown"
    assert events[0]["provider"] == "openai"
    assert "RuntimeError: boom" in events[0]["trace"]
    assert events[0]["ts"] == 123456


def test_run_cogitate_propagates_max_turns_unwrapped(
    fake_openhands,
    run_env,
):
    exhausted = MaxTurnsExhausted("max turns")

    async def fail(_conversation):
        raise exhausted

    fake_openhands.Conversation.arun_impl = fail
    events: list[dict] = []

    with pytest.raises(MaxTurnsExhausted) as raised:
        asyncio.run(openhands.run_cogitate(run_env, events.append))

    assert raised.value is exhausted
    assert events == []


def test_run_cogitate_propagates_quota_unwrapped(fake_openhands, run_env):
    quota = QuotaExhaustedError("quota", retry_delay_ms=111)

    async def fail(_conversation):
        raise quota

    fake_openhands.Conversation.arun_impl = fail
    events: list[dict] = []

    with pytest.raises(QuotaExhaustedError) as raised:
        asyncio.run(openhands.run_cogitate(run_env, events.append))

    assert raised.value is quota
    assert raised.value.retry_delay_ms == 111
    assert events == []
