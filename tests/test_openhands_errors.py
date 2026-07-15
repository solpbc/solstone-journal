# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import asyncio
import json
import traceback
from types import SimpleNamespace

import httpx
import openai
import pytest

from solstone.think.models import LOCAL_MODEL
from solstone.think.providers import openhands
from solstone.think.providers.cli import ProviderKeyMissingError, QuotaExhaustedError
from solstone.think.providers.local_endpoint import (
    LOCAL_ENDPOINT_CONTRACT_COPY,
    LOCAL_ENDPOINT_UNREACHABLE_COPY,
    LocalEndpoint,
)
from solstone.think.talents import TalentHookError
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

    async def fail(conversation):
        usage = conversation.agent.llm.metrics.accumulated_token_usage
        usage.prompt_tokens = 12
        usage.completion_tokens = 4
        conversation.agent.llm.metrics.token_usages = [object()]
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
    assert events[0]["usage"]["total_tokens"] > 0
    assert events[0]["ts"] == 123456


def test_run_cogitate_talent_hook_error_propagates_without_provider_event(
    fake_openhands,
    run_env,
):
    hook_exc = TalentHookError(
        "post",
        "broken_hook",
        "chat",
        RuntimeError("hook exploded"),
    )

    async def fail(_conversation):
        raise hook_exc

    fake_openhands.Conversation.arun_impl = fail
    events: list[dict] = []

    with pytest.raises(TalentHookError) as raised:
        asyncio.run(openhands.run_cogitate(run_env, events.append))

    assert raised.value is hook_exc
    assert events == []
    assert not getattr(hook_exc, "_evented", False)


def test_run_cogitate_error_before_usage_baseline_omits_usage(
    fake_openhands,
    run_env,
    monkeypatch,
):
    build_exc = RuntimeError("llm exploded")

    def fail_build(_provider, _model):
        raise build_exc

    monkeypatch.setattr(openhands, "_build_llm", fail_build)
    events: list[dict] = []

    with pytest.raises(RuntimeError) as raised:
        asyncio.run(openhands.run_cogitate(run_env, events.append))

    assert raised.value is build_exc
    assert getattr(build_exc, "_evented") is True
    assert len(events) == 1
    assert events[0]["event"] == "error"
    assert events[0]["error"] == "llm exploded"
    assert events[0]["reason_code"] == "unknown"
    assert events[0]["provider"] == "openai"
    assert "RuntimeError: llm exploded" in events[0]["trace"]
    assert "usage" not in events[0]
    assert events[0]["ts"] == 123456


def test_run_cogitate_missing_key_fails_pre_network(
    fake_openhands,
    run_env,
    monkeypatch,
    tmp_path,
):
    monkeypatch.delenv(openhands._API_KEY_ENV["openai"], raising=False)
    events: list[dict] = []

    with pytest.raises(ProviderKeyMissingError) as raised:
        asyncio.run(openhands.run_cogitate(run_env, events.append))

    assert raised.value.provider == "openai"
    assert raised.value.env_key == openhands._API_KEY_ENV["openai"]
    assert raised.value.reason_code == "provider_key_missing"
    assert len(events) == 1
    assert events[0]["event"] == "error"
    assert events[0]["reason_code"] == "provider_key_missing"
    assert events[0]["provider"] == "openai"
    assert fake_openhands.LLM.instances == []
    assert fake_openhands.Conversation.instances == []
    assert not (
        tmp_path / ".cache" / "cogitate-history" / run_env["session_id"]
    ).exists()


def test_run_cogitate_local_byo_error_event_uses_fixed_copy_and_redacts(
    fake_openhands,
    run_env,
    monkeypatch,
):
    token = "test-token-PLACEHOLDER"
    endpoint = LocalEndpoint(
        base_url="http://byo.example/openai",
        served_model_id="served-model",
        credential=token,
        is_bundled=False,
    )

    class BadRequestError(RuntimeError):
        status_code = 400

    async def fail(_conversation):
        raise BadRequestError(f"bad request with {token}")

    monkeypatch.setattr(
        "solstone.think.providers.local_endpoint.resolve_local_endpoint",
        lambda: endpoint,
    )
    fake_openhands.Conversation.arun_impl = fail
    events: list[dict] = []
    local_env = {**run_env, "provider": "local", "model": LOCAL_MODEL}

    with pytest.raises(BadRequestError):
        asyncio.run(openhands.run_cogitate(local_env, events.append))

    assert len(events) == 1
    assert events[0]["event"] == "error"
    assert events[0]["error"] == LOCAL_ENDPOINT_CONTRACT_COPY
    assert events[0]["reason_code"] == "local_endpoint_contract_failed"
    assert token not in events[0]["trace"]


@pytest.mark.parametrize("exc_type", [httpx.ConnectError, httpx.ConnectTimeout])
def test_run_cogitate_byo_connection_error_classifies_unreachable_no_wall_clock(
    fake_openhands,
    run_env,
    monkeypatch,
    exc_type,
):
    sentinel = "SENTINEL-BYO-CRED-9f3a2b"
    endpoint = LocalEndpoint(
        base_url="http://byo.example/openai",
        served_model_id="served-model",
        credential=sentinel,
        is_bundled=False,
    )

    async def fail(_conversation):
        raise exc_type(f"connection failed {sentinel}")

    monkeypatch.setattr(
        "solstone.think.providers.local_endpoint.resolve_local_endpoint",
        lambda: endpoint,
    )
    fake_openhands.Conversation.arun_impl = fail
    events: list[dict] = []
    local_env = {**run_env, "provider": "local", "model": LOCAL_MODEL}

    with pytest.raises(exc_type) as raised:
        asyncio.run(openhands.run_cogitate(local_env, events.append))

    assert len(events) == 1
    assert events[0]["event"] == "error"
    assert events[0]["reason_code"] == "local_endpoint_unreachable"
    assert events[0]["error"] == LOCAL_ENDPOINT_UNREACHABLE_COPY
    assert "wall_clock_exceeded" not in {event.get("reason_code") for event in events}
    assert sentinel not in json.dumps(events)
    assert all(sentinel not in event.get("trace", "") for event in events)
    assert sentinel not in str(raised.value)
    serialized = "".join(
        traceback.format_exception(
            type(raised.value),
            raised.value,
            raised.value.__traceback__,
        )
    )
    assert sentinel not in serialized


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
