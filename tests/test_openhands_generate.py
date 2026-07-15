# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import asyncio
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock

import pytest
from PIL import Image

from solstone.think.providers import openhands
from solstone.think.providers.cli import ProviderKeyMissingError
from tests.openhands_fakes import install_fake_openhands


@pytest.fixture
def fake_openhands(monkeypatch):
    return install_fake_openhands(monkeypatch)


@pytest.mark.parametrize(
    ("provider", "model"),
    [
        ("google", "gemini-flash-latest"),
        ("anthropic", "claude-sonnet-4-6"),
        ("openai", "gpt-5.5"),
    ],
)
def test_build_generate_llm_missing_env_raises_provider_key_missing(
    fake_openhands,
    monkeypatch,
    provider,
    model,
):
    monkeypatch.delenv(openhands._API_KEY_ENV[provider], raising=False)

    with pytest.raises(ProviderKeyMissingError) as raised:
        openhands._build_generate_llm(
            provider,
            model,
            max_output_tokens=256,
            thinking_budget=None,
            timeout_s=30,
        )

    assert raised.value.provider == provider
    assert raised.value.env_key == openhands._API_KEY_ENV[provider]
    assert raised.value.reason_code == "provider_key_missing"
    assert "Thinking" in str(raised.value)
    assert fake_openhands.LLM.instances == []


@pytest.mark.parametrize(
    ("provider", "model"),
    [
        ("google", "gemini-flash-latest"),
        ("anthropic", "claude-sonnet-4-6"),
        ("openai", "gpt-5.5"),
    ],
)
def test_build_generate_llm_explicit_key_bypasses_env(
    fake_openhands,
    monkeypatch,
    provider,
    model,
):
    monkeypatch.delenv(openhands._API_KEY_ENV[provider], raising=False)

    llm, _api_model = openhands._build_generate_llm(
        provider,
        model,
        max_output_tokens=256,
        thinking_budget=None,
        timeout_s=30,
        api_key="explicit-key",
    )

    assert llm is fake_openhands.LLM.instances[-1]
    assert llm.api_key == "explicit-key"


@pytest.mark.parametrize(
    ("provider", "model"),
    [
        ("google", "gemini-flash-latest"),
        ("anthropic", "claude-sonnet-4-6"),
        ("openai", "gpt-5.5"),
    ],
)
def test_build_generate_llm_blank_env_is_missing(
    fake_openhands,
    monkeypatch,
    provider,
    model,
):
    monkeypatch.setenv(openhands._API_KEY_ENV[provider], "   ")

    with pytest.raises(ProviderKeyMissingError):
        openhands._build_generate_llm(
            provider,
            model,
            max_output_tokens=256,
            thinking_budget=None,
            timeout_s=30,
        )

    assert fake_openhands.LLM.instances == []


def _response(
    text: str = "hello",
    *,
    model: str = "provider-model",
    finish_reason: str = "stop",
    reasoning: str | None = None,
):
    usage = SimpleNamespace(
        prompt_tokens=11,
        completion_tokens=7,
        cache_read_tokens=3,
        cache_write_tokens=0,
        reasoning_tokens=2,
    )
    message = SimpleNamespace(
        content=[SimpleNamespace(text=text)],
        thinking_blocks=[],
        reasoning_content=reasoning,
        responses_reasoning_item=None,
    )
    return SimpleNamespace(
        message=message,
        metrics=SimpleNamespace(accumulated_token_usage=usage),
        raw_response={
            "model": model,
            "choices": [{"finish_reason": finish_reason}],
        },
    )


def test_generate_google_uses_openhands_chat_and_normalizes_result(monkeypatch):
    llm = MagicMock()
    llm.completion.return_value = _response(reasoning="brief reasoning")
    monkeypatch.setattr(
        openhands,
        "_build_generate_llm",
        lambda *args, **kwargs: (llm, "gemini-flash-latest"),
    )

    result = openhands.run_generate(
        "hello",
        "gemini-flash-latest",
        provider="google",
        thinking_budget=128,
        json_schema={"title": "Reply", "type": "object"},
        max_output_tokens=256,
    )

    llm.completion.assert_called_once()
    assert not llm.responses.called
    messages = llm.completion.call_args.args[0]
    assert messages[0].role == "user"
    assert messages[0].content[0].text == "hello"
    kwargs = llm.completion.call_args.kwargs
    assert "timeout" not in kwargs
    assert kwargs["thinking"] == {"type": "enabled", "budget_tokens": 128}
    assert kwargs["response_format"]["json_schema"]["name"] == "Reply"
    assert result == {
        "text": "hello",
        "model": "provider-model",
        "usage": {
            "input_tokens": 11,
            "output_tokens": 7,
            "total_tokens": 18,
            "cached_tokens": 3,
            "reasoning_tokens": 2,
            "model_version": "provider-model",
        },
        "finish_reason": "stop",
        "thinking": [{"summary": "brief reasoning"}],
    }


def test_generate_restores_process_logging_after_openhands_import(monkeypatch):
    llm = MagicMock()
    llm.completion.return_value = _response()
    baseline = object()
    restored = []
    monkeypatch.delenv("OPENHANDS_SUPPRESS_BANNER", raising=False)
    monkeypatch.setattr(openhands, "snapshot_root_logging", lambda: baseline)
    monkeypatch.setattr(
        openhands,
        "apply_http_logging_policy",
        lambda value: restored.append(value),
    )
    monkeypatch.setattr(
        openhands,
        "_build_generate_llm",
        lambda *args, **kwargs: (llm, "gemini-flash-latest"),
    )

    openhands.run_generate("hello", "gemini-flash-latest", provider="google")

    assert restored == [baseline]
    assert openhands.os.environ["OPENHANDS_SUPPRESS_BANNER"] == "1"


def test_generate_openai_uses_responses_json_schema(monkeypatch):
    llm = MagicMock()
    llm.responses.return_value = _response(text='{"ok":true}', model="gpt-5.5")
    monkeypatch.setattr(
        openhands,
        "_build_generate_llm",
        lambda *args, **kwargs: (llm, "gpt-5.5"),
    )

    result = openhands.run_generate(
        "hello",
        "gpt-5.5-high",
        provider="openai",
        json_schema={"type": "object"},
    )

    llm.responses.assert_called_once()
    assert not llm.completion.called
    kwargs = llm.responses.call_args.kwargs
    assert "timeout" not in kwargs
    assert kwargs["text"]["format"] == {
        "type": "json_schema",
        "name": "response",
        "schema": {"type": "object"},
        "strict": True,
    }
    assert result["text"] == '{"ok":true}'


def test_generate_messages_preserve_roles_and_multimodal_content():
    messages = openhands._generate_messages(
        [
            {"role": "model", "content": "earlier"},
            {"role": "user", "content": ["look", Image.new("RGB", (2, 2))]},
        ],
        "system",
    )

    assert [message.role for message in messages] == ["system", "assistant", "user"]
    assert messages[2].content[0].text == "look"
    assert messages[2].content[1].image_urls[0].startswith("data:image/png;base64,")


def test_generate_provider_thinking_and_token_budget_mapping(caplog):
    assert openhands._generate_token_budget("google", 100, 25) == 125
    assert openhands._generate_token_budget("anthropic", 100, 200) == 1201
    assert openhands._generate_token_budget("openai", 100, 200) == 100
    assert openhands._parse_openai_effort("gpt-5.5-xhigh") == ("gpt-5.5", "xhigh")
    google_kwargs = openhands._generate_call_kwargs(
        "google",
        "gemini-flash-latest",
        temperature=None,
        json_output=False,
        json_schema=None,
        thinking_budget=0,
        responses_api=False,
    )
    assert google_kwargs["thinking"] == {"type": "disabled", "budget_tokens": 0}
    assert "timeout" not in google_kwargs

    assert "temperature" not in openhands._generate_call_kwargs(
        "openai",
        "gpt-5.5",
        temperature=0.3,
        json_output=False,
        json_schema=None,
        thinking_budget=None,
        responses_api=True,
    )
    assert "temperature" not in openhands._generate_call_kwargs(
        "anthropic",
        "claude-sonnet-4-6",
        temperature=0.3,
        json_output=False,
        json_schema=None,
        thinking_budget=1_024,
        responses_api=False,
    )

    assert openhands._generate_token_budget("google", 65_000, 1_000) == 65_535
    assert "Clamping Gemini token budget" in caplog.text


@pytest.mark.asyncio
async def test_agenerate_uses_async_openhands_transport(monkeypatch):
    llm = MagicMock()
    llm.acompletion = AsyncMock(return_value=_response())
    monkeypatch.setattr(
        openhands,
        "_build_generate_llm",
        lambda *args, **kwargs: (llm, "claude-sonnet-4-6"),
    )

    result = await openhands.run_agenerate(
        "hello",
        "claude-sonnet-4-6",
        provider="anthropic",
        thinking_budget=1_024,
    )

    llm.acompletion.assert_awaited_once()
    kwargs = llm.acompletion.call_args.kwargs
    assert "timeout" not in kwargs
    assert kwargs["thinking"] == {
        "type": "enabled",
        "budget_tokens": 1_024,
    }
    assert result["text"] == "hello"


def test_fake_openhands_completion_raises_on_duplicate_timeout(fake_openhands):
    llm = fake_openhands.LLM(model="google/gemini-flash-latest", timeout=10)

    with pytest.raises(
        TypeError,
        match="dict\\(\\) got multiple values for keyword argument 'timeout'",
    ):
        llm.completion([], timeout=1)


@pytest.mark.parametrize(
    ("provider", "model", "transport_attr"),
    [
        ("google", "gemini-flash-latest", "last_completion_kwargs"),
        ("anthropic", "claude-sonnet-4-6", "last_completion_kwargs"),
        ("openai", "gpt-5.5", "last_responses_kwargs"),
    ],
)
def test_run_generate_transport_kwargs_do_not_shadow_llm_timeout(
    fake_openhands,
    monkeypatch,
    provider,
    model,
    transport_attr,
):
    monkeypatch.setenv(openhands._API_KEY_ENV[provider], "test-key")

    result = openhands.run_generate("hello", model, provider=provider, timeout_s=7)

    llm = fake_openhands.LLM.instances[-1]
    assert llm.timeout == 7
    assert result["text"] == "fake response"
    assert "timeout" not in getattr(llm, transport_attr)


@pytest.mark.parametrize(
    ("provider", "model", "transport_attr"),
    [
        ("google", "gemini-flash-latest", "last_completion_kwargs"),
        ("anthropic", "claude-sonnet-4-6", "last_completion_kwargs"),
        ("openai", "gpt-5.5", "last_responses_kwargs"),
    ],
)
@pytest.mark.asyncio
async def test_run_agenerate_transport_kwargs_do_not_shadow_llm_timeout(
    fake_openhands,
    monkeypatch,
    provider,
    model,
    transport_attr,
):
    monkeypatch.setenv(openhands._API_KEY_ENV[provider], "test-key")

    result = await openhands.run_agenerate(
        "hello",
        model,
        provider=provider,
        timeout_s=7,
    )

    llm = fake_openhands.LLM.instances[-1]
    assert llm.timeout == 7
    assert result["text"] == "fake response"
    assert "timeout" not in getattr(llm, transport_attr)


def test_validation_uses_runtime_probe_and_classifies_results(monkeypatch):
    monkeypatch.setattr(openhands, "_probe", lambda *args: None)
    assert openhands.validate_key("google", "key") == {"valid": True}
    assert openhands.validate_model("openai", "gpt-5.5", "key") == {"valid": True}

    class NotFoundError(RuntimeError):
        status_code = 404

    def missing(*_args):
        raise NotFoundError("missing")

    monkeypatch.setattr(openhands, "_probe", missing)
    assert openhands.validate_key("google", "key") == {
        "valid": True,
        "probe_reason_code": "model_not_found",
    }
    assert openhands.validate_model("google", "missing", "key")["reason_code"] == (
        "model_not_found"
    )


def test_async_entry_point_rejects_transport_specific_options():
    with pytest.raises(TypeError, match="Unsupported generate options: client"):
        asyncio.run(
            openhands.run_agenerate(
                "hello",
                "gemini-flash-latest",
                provider="google",
                client=object(),
            )
        )
