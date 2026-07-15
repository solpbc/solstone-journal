# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import asyncio
import base64
import copy
import importlib
import json
import logging
import sys
import traceback
import types
from pathlib import Path
from types import SimpleNamespace

import pytest

from solstone.think.models import (
    DEFAULT_MODEL_BY_PROVIDER,
    LOCAL_MODEL,
    get_model_provider,
)
from solstone.think.talents import TalentHookError


@pytest.fixture(autouse=True)
def _isolate_local_admission(monkeypatch, tmp_path):
    from solstone.think.providers import local_admission

    monkeypatch.setattr(
        local_admission,
        "_admission_dir",
        lambda: tmp_path / "local-inference-admission",
    )
    monkeypatch.setattr(local_admission, "record_local_inference", lambda _record: None)


def _provider():
    providers_pkg = importlib.import_module("solstone.think.providers")
    if hasattr(providers_pkg, "local_budget"):
        delattr(providers_pkg, "local_budget")
    sys.modules.pop("solstone.think.providers.local_budget", None)
    return importlib.reload(importlib.import_module("solstone.think.providers.local"))


def _schema_keyword_paths(schema, keywords):
    found = []

    def walk(node, path="$"):
        if isinstance(node, dict):
            for key, value in node.items():
                child_path = f"{path}/{key}"
                if key in keywords:
                    found.append(child_path)
                walk(value, child_path)
        elif isinstance(node, list):
            for index, item in enumerate(node):
                walk(item, f"{path}[{index}]")

    walk(schema)
    return found


def _local_response(finish_reason):
    return {
        "choices": [
            {
                "message": {"content": "ok"},
                "finish_reason": finish_reason,
            }
        ]
    }


class _ChatResponse:
    def __init__(self, text: str = "hello") -> None:
        self.text = text

    def raise_for_status(self):
        return None

    def json(self):
        return {
            "choices": [
                {
                    "message": {"content": self.text},
                    "finish_reason": "stop",
                }
            ],
        }


def test_local_model_prefix_maps_to_provider():
    assert get_model_provider(LOCAL_MODEL) == "local"


def test_local_model_specs():
    provider = _provider()

    assert set(provider.LOCAL_MODEL_SPECS) == {LOCAL_MODEL}
    spec = provider.LOCAL_MODEL_SPECS[LOCAL_MODEL]
    assert spec.repo == "unsloth/Qwen3.5-4B-GGUF"
    assert spec.filename == "Qwen3.5-4B-Q4_K_M.gguf"
    assert (
        spec.sha256
        == "00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4"
    )
    assert spec.size_bytes == 2740937888
    assert spec.min_ram_bytes == 8 * 1024**3
    assert spec.mmproj_filename == "mmproj-F16.gguf"
    assert (
        spec.mmproj_sha256
        == "cd88edcf8d031894960bb0c9c5b9b7e1fea6ebee02b9f7ce925a00d12891f864"
    )
    assert spec.mmproj_size_bytes == 672423616


def test_local_provider_defaults_and_registry():
    from solstone.think.providers import PROVIDER_METADATA, PROVIDER_REGISTRY

    assert DEFAULT_MODEL_BY_PROVIDER["local"] == LOCAL_MODEL
    assert PROVIDER_REGISTRY["local"] == "solstone.think.providers.local"
    assert PROVIDER_METADATA["local"] == {
        "label": "Local (on-device)",
        "env_key": "",
    }


def test_context_budget_exceeded_classifies_by_reason_code():
    provider = _provider()

    assert (
        provider.classify_provider_error(
            provider.ContextBudgetExceeded("too large"), "local"
        )
        == "context_budget_exceeded"
    )


@pytest.mark.parametrize(
    ("raw", "expected"),
    [
        ("stop", "stop"),
        ("length", "max_tokens"),
        ("max_tokens", "max_tokens"),
        ("content_filter", "content_filter"),
    ],
)
def test_parse_response_normalizes_known_finish_reasons(raw, expected):
    provider = _provider()

    result = provider._parse_response(_local_response(raw))

    assert result["finish_reason"] == expected


@pytest.mark.parametrize("raw", [None, "", "weird", "tool_calls", "function_call"])
def test_parse_response_fails_closed_on_bad_finish_reasons(raw):
    provider = _provider()

    with pytest.raises(provider.LocalProviderError) as exc_info:
        provider._parse_response(_local_response(raw))

    assert exc_info.value.reason_code == "provider_response_invalid"


def test_list_models_returns_specs():
    models = _provider().list_models("local")

    assert [model["model"] for model in models] == [LOCAL_MODEL]
    assert models[0]["min_ram_bytes"] == 8 * 1024**3


def test_validate_key_uses_tiny_generate(monkeypatch):
    provider = _provider()
    calls = []

    def fake_generate(*args, **kwargs):
        calls.append((args, kwargs))
        return {"text": "OK"}

    monkeypatch.setattr(provider, "run_generate", fake_generate)

    assert provider.validate_key("local", "") == {"valid": True}
    assert calls[0][0] == ("Say OK",)
    assert calls[0][1]["model"] == LOCAL_MODEL
    assert calls[0][1]["max_output_tokens"] == 8


def test_run_generate_posts_to_loopback(monkeypatch):
    provider = _provider()
    served_model_id = (
        "/Users/sol/.cache/huggingface/hub/"
        "models--mlx-community--Qwen3.5-9B/snapshots/abc123"
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: SimpleNamespace(
            port=4321,
            base_url="http://127.0.0.1:4321",
            served_model_id=served_model_id,
        ),
    )
    captured = {}

    class Response:
        def raise_for_status(self):
            return None

        def json(self):
            return {
                "model": served_model_id,
                "choices": [
                    {
                        "message": {"content": "hello"},
                        "finish_reason": "stop",
                    }
                ],
                "usage": {
                    "prompt_tokens": 3,
                    "completion_tokens": 2,
                    "total_tokens": 5,
                },
            }

    def fake_post(url, json, timeout):
        captured.update({"url": url, "json": json, "timeout": timeout})
        return Response()

    import httpx

    monkeypatch.setattr(httpx, "post", fake_post)

    result = provider.run_generate("hello", model=LOCAL_MODEL, max_output_tokens=16)

    assert captured["url"] == "http://127.0.0.1:4321/v1/chat/completions"
    assert captured["json"]["model"] == served_model_id
    assert captured["json"]["messages"] == [{"role": "user", "content": "hello"}]
    assert captured["json"]["max_tokens"] == 16
    assert captured["json"]["chat_template_kwargs"] == {"enable_thinking": False}
    assert captured["json"]["top_p"] == 0.8
    assert captured["json"]["top_k"] == 20
    assert captured["json"]["min_p"] == 0.0
    assert captured["json"]["presence_penalty"] == 1.5
    assert result["text"] == "hello"
    assert result["model"] == LOCAL_MODEL
    assert result["usage"] == {
        "input_tokens": 3,
        "output_tokens": 2,
        "total_tokens": 5,
    }


def test_run_generate_emits_chat_completions_image_url(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: SimpleNamespace(
            port=4321,
            base_url="http://127.0.0.1:4321",
            served_model_id=LOCAL_MODEL,
        ),
    )
    png = b"\x89PNG\r\n\x1a\npayload"
    captured = {}

    class Response:
        def raise_for_status(self):
            return None

        def json(self):
            return {
                "model": LOCAL_MODEL,
                "choices": [
                    {
                        "message": {"content": "ok"},
                        "finish_reason": "stop",
                    }
                ],
            }

    def fake_post(url, json, timeout):
        captured.update({"url": url, "json": json, "timeout": timeout})
        return Response()

    import httpx

    monkeypatch.setattr(httpx, "post", fake_post)

    provider.run_generate(["look", png], model=LOCAL_MODEL)

    assert captured["json"]["messages"] == [
        {
            "role": "user",
            "content": [
                {"type": "text", "text": "look"},
                {
                    "type": "image_url",
                    "image_url": {
                        "url": "data:image/png;base64,"
                        + base64.b64encode(png).decode("ascii")
                    },
                },
            ],
        }
    ]


def test_run_generate_bundled_clips_oversized_text_block(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: SimpleNamespace(
            port=4321,
            base_url="http://127.0.0.1:4321",
            served_model_id=LOCAL_MODEL,
        ),
    )
    from solstone.think.providers import local_budget

    monkeypatch.setattr(local_budget, "count_tokens", lambda text, _base_url: len(text))
    chunks = [
        "## 2026-06-23 09:00:00 - 09:05:00\n",
        "### Transcript\noldest " + ("o" * 5000) + "\n",
        "### Screen Activity\nmiddle " + ("m" * 5000) + "\n",
        "## 2026-06-23 09:05:00 - 09:10:00\n",
        "### Transcript\nrecent " + ("r" * 5000) + "\n",
        "### Screen Activity\nlatest " + ("l" * 5000) + "\n",
    ]
    big_block = "".join(chunks)
    captured = {}

    class Response:
        def raise_for_status(self):
            return None

        def json(self):
            return {
                "model": LOCAL_MODEL,
                "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}],
            }

    def fake_post(url, json, timeout):
        captured.update({"url": url, "json": json, "timeout": timeout})
        return Response()

    import httpx

    monkeypatch.setattr(httpx, "post", fake_post)

    schema = {"type": "object"}
    result = provider.run_generate(
        [big_block, "talent prompt"],
        model=LOCAL_MODEL,
        max_output_tokens=8192 * 6,
        system_instruction="system",
        json_schema=schema,
    )

    assert captured["json"]["messages"][0] == {"role": "system", "content": "system"}
    user_message = captured["json"]["messages"][1]["content"]
    assert local_budget.TRUNCATION_MARKER in user_message
    assert "oldest " not in user_message
    assert "latest " in user_message
    assert "talent prompt" in user_message
    assert len(user_message) < len(big_block)
    assert captured["json"]["response_format"]["json_schema"]["schema"] == schema
    assert result["input_budget"]["clipped"] is True


def test_run_generate_does_not_mutate_caller_schema(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: SimpleNamespace(
            port=4321,
            base_url="http://127.0.0.1:4321",
            served_model_id=LOCAL_MODEL,
        ),
    )
    captured = {}

    class Response:
        def raise_for_status(self):
            return None

        def json(self):
            return {
                "model": LOCAL_MODEL,
                "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}],
            }

    def fake_post(url, json, timeout):
        captured.update({"url": url, "json": json, "timeout": timeout})
        return Response()

    import httpx

    monkeypatch.setattr(httpx, "post", fake_post)

    schema = {
        "type": "object",
        "properties": {
            "timestamp": {
                "type": "string",
                "pattern": r"^\d{2}:\d{2}:\d{2}$",
                "minLength": 8,
                "maxLength": 8,
            },
            "slots": {"type": "array", "items": {"type": "string"}},
        },
    }
    original_schema = copy.deepcopy(schema)

    provider.run_generate("hello", model=LOCAL_MODEL, json_schema=schema)

    posted_schema = captured["json"]["response_format"]["json_schema"]["schema"]
    unsupported = {"pattern", "minLength", "maxLength"}
    assert _schema_keyword_paths(posted_schema, unsupported) == []
    assert posted_schema["properties"]["slots"]["maxItems"] == 192
    assert schema == original_schema
    assert schema["properties"]["timestamp"]["pattern"] == r"^\d{2}:\d{2}:\d{2}$"
    assert schema["properties"]["timestamp"]["minLength"] == 8
    assert schema["properties"]["timestamp"]["maxLength"] == 8
    assert sorted(_schema_keyword_paths(schema, unsupported)) == [
        "$/properties/timestamp/maxLength",
        "$/properties/timestamp/minLength",
        "$/properties/timestamp/pattern",
    ]
    assert "maxItems" not in schema["properties"]["slots"]


def test_prepare_local_schema_bounds_arrays_only_and_preserves_input():
    provider = _provider()
    schema = {
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "minItems": 1,
                "items": {
                    "type": "string",
                    "pattern": r"^\d+$",
                    "minLength": 1,
                    "maxLength": 5,
                },
            },
            "nullable_items": {
                "type": ["array", "null"],
                "items": {"type": "string"},
            },
            "prebounded": {
                "type": "array",
                "maxItems": 7,
                "items": {"type": "string"},
            },
            "status": {"type": "string", "enum": ["open", "closed"]},
            "empty": {"type": "null"},
            "name": {"type": "string"},
            "score": {"type": "number", "minimum": 0, "maximum": 10},
        },
    }
    original_schema = copy.deepcopy(schema)

    prepared = provider._prepare_local_schema(schema)

    assert not hasattr(provider, "_normalize_schema_patterns")
    assert schema == original_schema
    assert prepared["properties"]["items"]["maxItems"] == 192
    assert prepared["properties"]["items"]["minItems"] == 1
    assert prepared["properties"]["nullable_items"]["maxItems"] == 192
    assert prepared["properties"]["prebounded"]["maxItems"] == 7
    assert prepared["properties"]["items"]["items"] == {"type": "string"}
    assert prepared["properties"]["status"] == schema["properties"]["status"]
    assert prepared["properties"]["empty"] == schema["properties"]["empty"]
    assert prepared["properties"]["name"] == schema["properties"]["name"]
    assert prepared["properties"]["score"]["minimum"] == 0
    assert prepared["properties"]["score"]["maximum"] == 10

    assert _schema_keyword_paths(prepared, {"pattern", "minLength", "maxLength"}) == []


def test_prepare_local_schema_skips_json_literals_and_bounds_schema_nodes():
    provider = _provider()
    schema = {
        "type": "object",
        "properties": {
            "literal": {
                "enum": [
                    {"type": "array", "pattern": r"^\d+$", "maxLength": 12},
                ],
            },
            "fixed": {
                "const": {"type": "array", "pattern": r"^\d+$", "maxLength": 12},
            },
            "code": {
                "type": "string",
                "pattern": r"^\d+$",
                "minLength": 1,
                "maxLength": 12,
            },
            "type": {"type": "array", "items": {"type": "string"}},
        },
    }

    prepared = provider._prepare_local_schema(schema)

    assert prepared["properties"]["literal"]["enum"] == [
        {"type": "array", "pattern": r"^\d+$", "maxLength": 12},
    ]
    assert "maxItems" not in prepared["properties"]["literal"]["enum"][0]
    assert prepared["properties"]["fixed"]["const"] == {
        "type": "array",
        "pattern": r"^\d+$",
        "maxLength": 12,
    }
    assert "maxItems" not in prepared["properties"]["fixed"]["const"]
    assert prepared["properties"]["code"] == {"type": "string"}
    assert prepared["properties"]["type"]["maxItems"] == 192


def test_prepare_local_schema_bounds_array_schema_with_enum():
    provider = _provider()
    schema = {
        "type": "array",
        "enum": [["a"], ["b"]],
        "items": {"type": "string"},
    }

    prepared = provider._prepare_local_schema(schema)

    assert prepared["maxItems"] == 192
    assert prepared["enum"] == [["a"], ["b"]]


def test_run_generate_bundled_non_overflow_keeps_body_unmarked(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: SimpleNamespace(
            port=4321,
            base_url="http://127.0.0.1:4321",
            served_model_id=LOCAL_MODEL,
        ),
    )
    from solstone.think.providers import local_budget

    monkeypatch.setattr(local_budget, "count_tokens", lambda text, _base_url: len(text))
    small_block = "## Segment\n### Transcript\nsmall\n"
    captured = {}

    class Response:
        def raise_for_status(self):
            return None

        def json(self):
            return {
                "model": LOCAL_MODEL,
                "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}],
            }

    def fake_post(url, json, timeout):
        captured.update({"url": url, "json": json, "timeout": timeout})
        return Response()

    import httpx

    monkeypatch.setattr(httpx, "post", fake_post)

    result = provider.run_generate(
        [small_block, "talent prompt"],
        model=LOCAL_MODEL,
        max_output_tokens=1024,
        system_instruction="system",
    )

    assert captured["json"]["messages"][1]["content"] == (
        small_block + "\ntalent prompt"
    )
    assert (
        local_budget.TRUNCATION_MARKER not in captured["json"]["messages"][1]["content"]
    )
    assert "input_budget" not in result


def test_run_generate_bundled_preserved_exceeds_budget_skips_post(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: SimpleNamespace(
            port=4321,
            base_url="http://127.0.0.1:4321",
            served_model_id=LOCAL_MODEL,
        ),
    )
    from solstone.think.providers import local_budget

    monkeypatch.setattr(local_budget, "count_tokens", lambda text, _base_url: len(text))

    def fake_post(*_args, **_kwargs):
        raise AssertionError("httpx.post not expected")

    import httpx

    monkeypatch.setattr(httpx, "post", fake_post)

    with pytest.raises(provider.ContextBudgetExceeded) as exc:
        provider.run_generate(
            "## Segment\n### Transcript\nsmall\n",
            model=LOCAL_MODEL,
            max_output_tokens=8192 * 6,
            system_instruction="s" * 13000,
        )

    assert exc.value.reason_code == "context_budget_exceeded"


def test_run_generate_bundled_context_rejection_backstop(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: SimpleNamespace(
            port=4321,
            base_url="http://127.0.0.1:4321",
            served_model_id=LOCAL_MODEL,
        ),
    )
    from solstone.think.providers import local_budget

    monkeypatch.setattr(local_budget, "count_tokens", lambda text, _base_url: len(text))

    def fake_post(url, json, timeout):
        del json, timeout
        request = httpx.Request("POST", url)
        return httpx.Response(
            400,
            request=request,
            json={
                "error": {
                    "type": "exceed_context_size_error",
                    "message": (
                        "request (17 tokens) exceeds the available context size "
                        "(16 tokens), try increasing it"
                    ),
                    "n_prompt_tokens": 17,
                    "n_ctx": 16,
                }
            },
        )

    import httpx

    monkeypatch.setattr(httpx, "post", fake_post)

    with pytest.raises(provider.ContextBudgetExceeded) as exc:
        provider.run_generate("hello", model=LOCAL_MODEL, max_output_tokens=16)

    assert exc.value.reason_code == "context_budget_exceeded"


def test_run_generate_bundled_context_rejection_backstop_alt_phrasing(monkeypatch):
    # llama-server emits this after post-admission unified-KV exhaustion; the
    # fitted prompt is not proven too long, so this is transient capacity.
    provider = _provider()
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: SimpleNamespace(
            port=4321,
            base_url="http://127.0.0.1:4321",
            served_model_id=LOCAL_MODEL,
        ),
    )
    from solstone.think.providers import local_budget

    monkeypatch.setattr(local_budget, "count_tokens", lambda text, _base_url: len(text))

    def fake_post(url, json, timeout):
        del json, timeout
        request = httpx.Request("POST", url)
        return httpx.Response(
            500,
            request=request,
            json={
                "error": {
                    "type": "server_error",
                    "message": "Context size has been exceeded.",
                }
            },
        )

    import httpx

    monkeypatch.setattr(httpx, "post", fake_post)

    with pytest.raises(provider.LocalProviderError) as exc:
        provider.run_generate("hello", model=LOCAL_MODEL, max_output_tokens=16)

    assert type(exc.value).__name__ == "LocalCapacityExhausted"
    assert exc.value.reason_code == "local_capacity_exhausted"


def test_run_generate_bundled_context_rejection_missing_type_is_capacity(
    monkeypatch,
):
    provider = _provider()
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: SimpleNamespace(
            port=4321,
            base_url="http://127.0.0.1:4321",
            served_model_id=LOCAL_MODEL,
        ),
    )
    from solstone.think.providers import local_budget

    monkeypatch.setattr(local_budget, "count_tokens", lambda text, _base_url: len(text))

    def fake_post(url, json, timeout):
        del json, timeout
        request = httpx.Request("POST", url)
        return httpx.Response(
            500,
            request=request,
            json={"error": {"message": "Context size has been exceeded."}},
        )

    import httpx

    monkeypatch.setattr(httpx, "post", fake_post)

    with pytest.raises(provider.LocalProviderError) as exc:
        provider.run_generate("hello", model=LOCAL_MODEL, max_output_tokens=16)

    assert type(exc.value).__name__ == "LocalCapacityExhausted"
    assert exc.value.reason_code == "local_capacity_exhausted"


def test_run_agenerate_bundled_capacity_rejection_matches_sync(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(provider, "resolve_local_endpoint", _bundled_endpoint)
    _patch_bundled_server(monkeypatch)

    class Response:
        text = '{"error":{"type":"server_error","message":"Context size has been exceeded."}}'

        def raise_for_status(self):
            request = httpx.Request(
                "POST",
                "http://127.0.0.1:4321/v1/chat/completions",
            )
            response = httpx.Response(500, request=request, text=self.text)
            raise httpx.HTTPStatusError(
                "server error",
                request=request,
                response=response,
            )

    class AsyncClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *_args):
            return None

        async def post(self, *_args, **_kwargs):
            return Response()

    import httpx

    monkeypatch.setattr(httpx, "AsyncClient", AsyncClient)

    with pytest.raises(provider.LocalProviderError) as exc:
        asyncio.run(provider.run_agenerate("hello", model=LOCAL_MODEL))

    assert type(exc.value).__name__ == "LocalCapacityExhausted"
    assert exc.value.reason_code == "local_capacity_exhausted"


def test_run_generate_bundled_capacity_rejection_records_retry_telemetry(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(provider, "resolve_local_endpoint", _bundled_endpoint)
    _patch_bundled_server(monkeypatch)

    from solstone.think.providers import local_admission

    records: list[dict] = []
    monkeypatch.setattr(local_admission, "record_local_inference", records.append)

    def fake_post(url, json, timeout):
        del json, timeout
        request = httpx.Request("POST", url)
        return httpx.Response(
            500,
            request=request,
            json={
                "error": {
                    "type": "server_error",
                    "message": "Context size has been exceeded.",
                }
            },
        )

    import httpx

    monkeypatch.setattr(httpx, "post", fake_post)

    with pytest.raises(provider.LocalProviderError):
        provider.run_generate(
            "private prompt text",
            model=LOCAL_MODEL,
            max_output_tokens=16,
        )
    with pytest.raises(provider.LocalProviderError):
        provider.run_generate(
            "private prompt text",
            model=LOCAL_MODEL,
            max_output_tokens=16,
            inference_retry_index=1,
            local_exclusive_admission=True,
        )

    assert len(records) == 2
    assert [record["retry_index"] for record in records] == [0, 1]
    for record in records:
        assert record["reason_code"] == "local_capacity_exhausted"
        assert record["outcome"] == "error"
        serialized = json.dumps(record, sort_keys=True)
        assert "private prompt text" not in serialized
        assert "Context size has been exceeded." not in serialized
        assert "server_error" not in serialized


def test_openhands_local_llm_kwargs(monkeypatch):
    from solstone.think.providers import local_server, openhands

    captured = {}
    served_model_id = (
        "/Users/sol/.cache/huggingface/hub/"
        "models--mlx-community--Qwen3.5-9B/snapshots/abc123"
    )

    class FakeLLM:
        def __init__(self, **kwargs):
            captured.update(kwargs)

    sdk_module = types.ModuleType("openhands.sdk")
    sdk_module.LLM = FakeLLM
    monkeypatch.setitem(sys.modules, "openhands.sdk", sdk_module)
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: SimpleNamespace(port=9876, served_model_id=served_model_id),
    )

    llm = openhands._build_llm("local", LOCAL_MODEL)

    assert isinstance(llm, FakeLLM)
    assert captured == {
        "model": f"openai/{served_model_id}",
        "base_url": "http://127.0.0.1:9876/v1",
        "api_key": "EMPTY",
        "native_tool_calling": False,
        "timeout": openhands.LLM_TIMEOUT_S,
        "num_retries": openhands.LLM_NUM_RETRIES,
        "max_input_tokens": local_server.LOCAL_MIN_CONTEXT_TOKENS,
        "max_output_tokens": openhands._LOCAL_OUTPUT_RESERVE_TOKENS,
        "input_cost_per_token": 0,
        "output_cost_per_token": 0,
        "litellm_extra_body": {"chat_template_kwargs": {"enable_thinking": False}},
    }
    capable_tier = local_server.select_server_tier(24576)
    assert capable_tier.context_tokens == 32768
    assert captured["max_input_tokens"] == 16384
    assert captured["max_input_tokens"] != capable_tier.context_tokens
    assert "chat_template_kwargs" not in captured
    assert openhands._prefixed_model("local", LOCAL_MODEL) == f"openai/{LOCAL_MODEL}"


def _byo_endpoint(
    credential: str | None = "test-token-PLACEHOLDER",
    parallel_slots: int | None = None,
):
    from solstone.think.providers.local_endpoint import (
        LocalEndpoint,
        normalize_local_endpoint_url,
    )

    return LocalEndpoint(
        base_url=normalize_local_endpoint_url("http://byo.example/openai/v1/"),
        served_model_id="served-model",
        credential=credential,
        is_bundled=False,
        parallel_slots=parallel_slots,
    )


def _bundled_endpoint():
    from solstone.think.providers.local_endpoint import LocalEndpoint

    return LocalEndpoint("", "", None, is_bundled=True)


def _patch_bundled_server(monkeypatch):
    from solstone.think.providers import local_server

    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: SimpleNamespace(
            port=4321,
            base_url="http://127.0.0.1:4321",
            served_model_id=LOCAL_MODEL,
        ),
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.read_server_capacity",
        lambda: local_server.ServerCapacity(1, "test", "floor"),
    )


def test_run_generate_byo_posts_to_normalized_endpoint_and_skips_connect(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(provider, "resolve_local_endpoint", _byo_endpoint)
    from solstone.think.providers import local_budget

    def fail_count(*_args, **_kwargs):
        raise AssertionError("count_tokens not expected")

    monkeypatch.setattr(local_budget, "count_tokens", fail_count)
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: (_ for _ in ()).throw(AssertionError("connect not expected")),
    )
    captured = {}

    class Response:
        def raise_for_status(self):
            return None

        def json(self):
            return {
                "choices": [
                    {
                        "message": {"content": "hello"},
                        "finish_reason": "stop",
                    }
                ],
            }

    def fake_post(url, **kwargs):
        captured.update({"url": url, **kwargs})
        return Response()

    import httpx

    monkeypatch.setattr(httpx, "post", fake_post)

    result = provider.run_generate("hello", model=LOCAL_MODEL)

    assert captured["url"] == "http://byo.example/openai/v1/chat/completions"
    assert captured["json"]["model"] == "served-model"
    assert captured["headers"] == {"Authorization": "Bearer test-token-PLACEHOLDER"}
    assert local_budget.TRUNCATION_MARKER not in str(captured["json"])
    assert result["text"] == "hello"


def test_run_generate_byo_acquires_and_releases_permit(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(
        provider,
        "resolve_local_endpoint",
        lambda: _byo_endpoint(parallel_slots=1),
    )
    captured = {}

    def fake_post(url, **kwargs):
        captured.update({"url": url, **kwargs})
        return _ChatResponse()

    import httpx

    from solstone.think.providers import local_admission

    monkeypatch.setattr(httpx, "post", fake_post)

    result = provider.run_generate("hello", model=LOCAL_MODEL)

    assert result["text"] == "hello"
    assert captured["url"] == "http://byo.example/openai/v1/chat/completions"
    with local_admission.acquire_local_slot(1, 0.1) as permit:
        assert permit.slot_index == 0


def test_run_generate_byo_queue_timeout_preserves_exact_type_and_skips_post(
    monkeypatch,
):
    provider = _provider()
    monkeypatch.setattr(
        provider,
        "resolve_local_endpoint",
        lambda: _byo_endpoint(parallel_slots=1),
    )

    def fake_post(*_args, **_kwargs):
        raise AssertionError("httpx.post must not run after queue timeout")

    import httpx

    from solstone.think.providers import local_admission

    monkeypatch.setattr(httpx, "post", fake_post)

    holder = local_admission.acquire_local_slot(1, 0.1)
    try:
        with pytest.raises(local_admission.LocalAdmissionTimeout) as exc:
            provider.run_generate("hello", model=LOCAL_MODEL, timeout_s=0.03)
        assert exc.type is local_admission.LocalAdmissionTimeout
        assert exc.value.reason_code == "local_queue_timeout"
    finally:
        holder.release()


def test_run_generate_byo_http_timeout_uses_remaining_deadline(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(
        provider,
        "resolve_local_endpoint",
        lambda: _byo_endpoint(parallel_slots=1),
    )
    captured = {}
    times = iter([100.0, 100.0, 100.35])

    def fake_monotonic():
        try:
            return next(times)
        except StopIteration:
            return 100.35

    def fake_acquire(capacity, timeout_s, *, exclusive=False):
        captured["capacity"] = capacity
        captured["permit_timeout"] = timeout_s
        captured["exclusive"] = exclusive
        return contextlib.nullcontext()

    def fake_post(url, **kwargs):
        captured.update({"url": url, **kwargs})
        return _ChatResponse()

    import contextlib

    import httpx

    from solstone.think.providers import local_admission

    monkeypatch.setattr(provider.time, "monotonic", fake_monotonic)
    monkeypatch.setattr(local_admission, "acquire_local_slot", fake_acquire)
    monkeypatch.setattr(httpx, "post", fake_post)

    provider.run_generate("hello", model=LOCAL_MODEL, timeout_s=1.0)

    assert captured["capacity"] == 1
    assert captured["permit_timeout"] == pytest.approx(1.0)
    assert captured["exclusive"] is False
    assert captured["timeout"] == pytest.approx(0.65)


def test_run_generate_confidential_with_stray_slots_skips_admission(monkeypatch):
    provider = _provider()
    configured_endpoint = "https://spp.example.test"
    config = {
        "providers": {
            "local": {
                "endpoint_url": configured_endpoint,
                "served_model_id": "confidential-model",
                "credential": "confidential-token",
                "parallel_slots": 1,
            }
        },
        "services": {"confidential": {"account_id": "acct"}},
    }
    captured = {}
    current_time = {"value": 100.0}

    def fake_monotonic():
        return current_time["value"]

    def fail_acquire(*_args, **_kwargs):
        raise AssertionError("confidential generate must not acquire admission")

    def fake_post(url, **kwargs):
        captured.update({"url": url, **kwargs})
        return _ChatResponse()

    def fake_egress_base_url(base_url):
        current_time["value"] += 2.0
        return "http://127.0.0.1:4567" if base_url == configured_endpoint else base_url

    import httpx

    from solstone.think.providers import local_admission, local_endpoint

    monkeypatch.setattr(provider.time, "monotonic", fake_monotonic)
    monkeypatch.setattr(local_endpoint, "read_journal_config", lambda: config)
    monkeypatch.setattr(local_admission, "acquire_local_slot", fail_acquire)
    monkeypatch.setattr(
        "solstone.think.services.spp_transport.confidential_egress_base_url",
        fake_egress_base_url,
    )
    monkeypatch.setattr(httpx, "post", fake_post)

    result = provider.run_generate("hello", model=LOCAL_MODEL, timeout_s=1.0)

    assert result["text"] == "hello"
    assert captured["url"] == "http://127.0.0.1:4567/v1/chat/completions"
    assert captured["timeout"] == pytest.approx(1.0)


def test_run_generate_confidential_posts_to_forwarder_not_configured_endpoint(
    monkeypatch,
):
    provider = _provider()

    from solstone.think.providers import local_budget
    from solstone.think.providers.local_endpoint import LocalEndpoint

    configured_endpoint = "https://spp.example.test"
    forwarder = "http://127.0.0.1:4567"
    monkeypatch.setattr(
        provider,
        "resolve_local_endpoint",
        lambda: LocalEndpoint(
            base_url=configured_endpoint,
            served_model_id="confidential-model",
            credential="confidential-token",
            is_bundled=False,
        ),
    )
    monkeypatch.setattr(
        "solstone.think.services.spp_transport.confidential_egress_base_url",
        lambda base_url: forwarder if base_url == configured_endpoint else base_url,
    )
    monkeypatch.setattr(local_budget, "count_tokens", lambda *_args, **_kwargs: 1)
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: (_ for _ in ()).throw(AssertionError("connect not expected")),
    )
    captured = {}

    class Response:
        def raise_for_status(self):
            return None

        def json(self):
            return {
                "choices": [
                    {
                        "message": {"content": "hello"},
                        "finish_reason": "stop",
                    }
                ],
            }

    def fake_post(url, **kwargs):
        captured.update({"url": url, **kwargs})
        return Response()

    import httpx

    monkeypatch.setattr(httpx, "post", fake_post)

    result = provider.run_generate("hello", model=LOCAL_MODEL)

    assert result["text"] == "hello"
    assert captured["url"] == f"{forwarder}/v1/chat/completions"
    assert configured_endpoint not in captured["url"]


def test_run_agenerate_confidential_posts_to_forwarder_not_configured_endpoint(
    monkeypatch,
):
    provider = _provider()

    from solstone.think.providers.local_endpoint import LocalEndpoint

    configured_endpoint = "https://spp.example.test"
    forwarder = "http://127.0.0.1:4567"
    monkeypatch.setattr(
        provider,
        "resolve_local_endpoint",
        lambda: LocalEndpoint(
            base_url=configured_endpoint,
            served_model_id="confidential-model",
            credential="confidential-token",
            is_bundled=False,
        ),
    )
    monkeypatch.setattr(
        "solstone.think.services.spp_transport.confidential_egress_base_url",
        lambda base_url: forwarder if base_url == configured_endpoint else base_url,
    )
    captured = {}

    class Response:
        def raise_for_status(self):
            return None

        def json(self):
            return {
                "choices": [
                    {
                        "message": {"content": "hello"},
                        "finish_reason": "stop",
                    }
                ],
            }

    class AsyncClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *_args):
            return None

        async def post(self, url, **kwargs):
            captured.update({"url": url, **kwargs})
            return Response()

    import httpx

    monkeypatch.setattr(httpx, "AsyncClient", AsyncClient)

    result = asyncio.run(provider.run_agenerate("hello", model=LOCAL_MODEL))

    assert result["text"] == "hello"
    assert captured["url"] == f"{forwarder}/v1/chat/completions"
    assert configured_endpoint not in captured["url"]


def test_run_agenerate_byo_acquires_and_releases_permit(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(
        provider,
        "resolve_local_endpoint",
        lambda: _byo_endpoint(parallel_slots=1),
    )
    captured = {}

    class AsyncClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *_args):
            return None

        async def post(self, url, **kwargs):
            captured.update({"url": url, **kwargs})
            return _ChatResponse()

    import httpx

    from solstone.think.providers import local_admission

    monkeypatch.setattr(httpx, "AsyncClient", AsyncClient)

    result = asyncio.run(provider.run_agenerate("hello", model=LOCAL_MODEL))

    assert result["text"] == "hello"
    assert captured["url"] == "http://byo.example/openai/v1/chat/completions"
    with local_admission.acquire_local_slot(1, 0.1) as permit:
        assert permit.slot_index == 0


def test_run_agenerate_byo_queue_timeout_preserves_exact_type_and_skips_post(
    monkeypatch,
):
    provider = _provider()
    monkeypatch.setattr(
        provider,
        "resolve_local_endpoint",
        lambda: _byo_endpoint(parallel_slots=1),
    )
    entered = False

    class AsyncClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *_args):
            return None

        async def post(self, *_args, **_kwargs):
            nonlocal entered
            entered = True
            raise AssertionError("AsyncClient.post must not run after queue timeout")

    import httpx

    from solstone.think.providers import local_admission

    monkeypatch.setattr(httpx, "AsyncClient", AsyncClient)

    holder = local_admission.acquire_local_slot(1, 0.1)
    try:
        with pytest.raises(local_admission.LocalAdmissionTimeout) as exc:
            asyncio.run(
                provider.run_agenerate("hello", model=LOCAL_MODEL, timeout_s=0.03)
            )
        assert exc.type is local_admission.LocalAdmissionTimeout
        assert exc.value.reason_code == "local_queue_timeout"
        assert entered is False
    finally:
        holder.release()


def test_run_agenerate_byo_http_timeout_uses_remaining_deadline(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(
        provider,
        "resolve_local_endpoint",
        lambda: _byo_endpoint(parallel_slots=1),
    )
    captured = {}
    after_wait = False

    def fake_monotonic():
        return 200.25 if after_wait else 200.0

    async def fake_acquire(capacity, timeout_s, *, exclusive=False):
        nonlocal after_wait
        captured["capacity"] = capacity
        captured["permit_timeout"] = timeout_s
        captured["exclusive"] = exclusive
        after_wait = True
        return contextlib.nullcontext()

    class AsyncClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *_args):
            return None

        async def post(self, url, **kwargs):
            captured.update({"url": url, **kwargs})
            return _ChatResponse()

    import contextlib

    import httpx

    from solstone.think.providers import local_admission

    monkeypatch.setattr(provider.time, "monotonic", fake_monotonic)
    monkeypatch.setattr(local_admission, "acquire_local_slot_async", fake_acquire)
    monkeypatch.setattr(httpx, "AsyncClient", AsyncClient)

    asyncio.run(provider.run_agenerate("hello", model=LOCAL_MODEL, timeout_s=1.0))

    assert captured["capacity"] == 1
    assert captured["permit_timeout"] == pytest.approx(1.0)
    assert captured["exclusive"] is False
    assert captured["timeout"] == pytest.approx(0.75)


def test_run_agenerate_confidential_with_stray_slots_skips_admission(monkeypatch):
    provider = _provider()
    configured_endpoint = "https://spp.example.test"
    config = {
        "providers": {
            "local": {
                "endpoint_url": configured_endpoint,
                "served_model_id": "confidential-model",
                "credential": "confidential-token",
                "parallel_slots": 1,
            }
        },
        "services": {"confidential": {"account_id": "acct"}},
    }
    captured = {}
    current_time = {"value": 200.0}

    def fake_monotonic():
        return current_time["value"]

    async def fail_acquire(*_args, **_kwargs):
        raise AssertionError("confidential agenerate must not acquire admission")

    def fake_egress_base_url(base_url):
        current_time["value"] += 2.0
        return "http://127.0.0.1:4567" if base_url == configured_endpoint else base_url

    class AsyncClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *_args):
            return None

        async def post(self, url, **kwargs):
            captured.update({"url": url, **kwargs})
            return _ChatResponse()

    import httpx

    from solstone.think.providers import local_admission, local_endpoint

    monkeypatch.setattr(provider.time, "monotonic", fake_monotonic)
    monkeypatch.setattr(local_endpoint, "read_journal_config", lambda: config)
    monkeypatch.setattr(local_admission, "acquire_local_slot_async", fail_acquire)
    monkeypatch.setattr(
        "solstone.think.services.spp_transport.confidential_egress_base_url",
        fake_egress_base_url,
    )
    monkeypatch.setattr(httpx, "AsyncClient", AsyncClient)

    result = asyncio.run(
        provider.run_agenerate("hello", model=LOCAL_MODEL, timeout_s=1.0)
    )

    assert result["text"] == "hello"
    assert captured["url"] == "http://127.0.0.1:4567/v1/chat/completions"
    assert captured["timeout"] == pytest.approx(1.0)


def test_run_generate_byo_body_omits_bundled_qwen_sampling(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(provider, "resolve_local_endpoint", _byo_endpoint)
    captured_posts = []

    class Response:
        def raise_for_status(self):
            return None

        def json(self):
            return {
                "choices": [
                    {
                        "message": {"content": "{}"},
                        "finish_reason": "stop",
                    }
                ],
            }

    def fake_post(url, **kwargs):
        captured_posts.append({"url": url, **kwargs})
        return Response()

    import httpx

    monkeypatch.setattr(httpx, "post", fake_post)
    schema = {
        "type": "object",
        "properties": {
            "items": {"type": "array", "items": {"type": "string"}},
        },
    }

    provider.run_generate(
        "hello",
        model=LOCAL_MODEL,
        temperature=0.4,
        max_output_tokens=7,
    )
    provider.run_generate(
        "hello",
        model=LOCAL_MODEL,
        temperature=0.5,
        max_output_tokens=11,
        json_schema=schema,
    )

    assert len(captured_posts) == 2
    assert captured_posts[0]["json"] == {
        "model": "served-model",
        "messages": [{"role": "user", "content": "hello"}],
        "temperature": 0.4,
        "max_tokens": 7,
        "stream": False,
        "chat_template_kwargs": {"enable_thinking": False},
    }
    assert captured_posts[1]["json"] == {
        "model": "served-model",
        "messages": [{"role": "user", "content": "hello"}],
        "temperature": 0.5,
        "max_tokens": 11,
        "stream": False,
        "chat_template_kwargs": {"enable_thinking": False},
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "local_schema",
                "schema": provider._prepare_local_schema(schema),
                "strict": True,
            },
        },
    }
    for post in captured_posts:
        for key in ("top_p", "top_k", "min_p", "presence_penalty"):
            assert key not in post["json"]


def test_run_generate_byo_omits_auth_header_without_credential(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(provider, "resolve_local_endpoint", lambda: _byo_endpoint(None))
    captured = {}

    class Response:
        def raise_for_status(self):
            return None

        def json(self):
            return {
                "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}]
            }

    def fake_post(url, **kwargs):
        captured.update({"url": url, **kwargs})
        return Response()

    import httpx

    monkeypatch.setattr(httpx, "post", fake_post)

    provider.run_generate("hello", model=LOCAL_MODEL)

    assert "headers" not in captured


def _load_schema(path: str) -> dict:
    return json.loads(Path(path).read_text(encoding="utf-8"))


def _sense_collection_bounds(schema: dict) -> dict[str, int]:
    properties = schema["properties"]
    return {
        "entities": properties["entities"]["maxItems"],
        "facets": properties["facets"]["maxItems"],
        "speakers": properties["speakers"]["maxItems"],
    }


def test_generate_schema_files_declare_only_safe_sense_collection_bounds():
    bounded_keys = {
        "minItems",
        "maxItems",
        "minLength",
        "maxLength",
        "minimum",
        "maximum",
    }
    sense = _load_schema("solstone/talent/sense.schema.json")

    assert _sense_collection_bounds(sense) == {
        "entities": 96,
        "facets": 16,
        "speakers": 16,
    }
    assert _schema_keyword_paths(sense, {"maxItems"}) == [
        "$/properties/entities/maxItems",
        "$/properties/facets/maxItems",
        "$/properties/speakers/maxItems",
    ]
    assert _schema_keyword_paths(sense, {"pattern", "minLength", "maxLength"}) == []

    for path in (
        "solstone/talent/participation.schema.json",
        "solstone/talent/participation_entry.schema.json",
    ):
        assert _schema_keyword_paths(_load_schema(path), bounded_keys) == []


def test_sense_collection_bounds_survive_runtime_and_local_schema_prep():
    from solstone.think.talent import hydrate_runtime_enums

    provider = _provider()
    sense = _load_schema("solstone/talent/sense.schema.json")

    hydrated = hydrate_runtime_enums(sense)
    prepared = provider._prepare_local_schema(hydrated)

    assert _sense_collection_bounds(hydrated) == {
        "entities": 96,
        "facets": 16,
        "speakers": 16,
    }
    assert _sense_collection_bounds(prepared) == {
        "entities": 96,
        "facets": 16,
        "speakers": 16,
    }


def test_local_input_budget_reserve_for_changed_caps():
    from solstone.think.providers.local_budget import compute_input_budget

    floor = 16384
    capable = 32768

    assert compute_input_budget(512, floor) - compute_input_budget(1024, floor) == 512
    assert compute_input_budget(2048, floor) - compute_input_budget(4096, floor) == 2048
    assert compute_input_budget(12288, floor) == compute_input_budget(6144, floor)
    assert compute_input_budget(12288, floor) == floor - 4096 - 256
    assert compute_input_budget(12288, capable) == capable - 8192 - 256
    assert compute_input_budget(6144, capable) == capable - 6144 - 256


def test_run_generate_byo_network_error_maps_to_unreachable(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(provider, "resolve_local_endpoint", _byo_endpoint)

    import httpx

    monkeypatch.setattr(
        httpx,
        "post",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            httpx.ConnectError("connection refused")
        ),
    )

    with pytest.raises(provider.LocalProviderError) as exc:
        provider.run_generate("hello", model=LOCAL_MODEL)

    assert exc.value.reason_code == "local_endpoint_unreachable"
    assert str(exc.value) == provider.LOCAL_ENDPOINT_UNREACHABLE_COPY
    assert isinstance(exc.value.__cause__, httpx.ConnectError)


@pytest.mark.parametrize(
    "exc_name",
    [
        "ConnectError",
        "APIConnectionError",
        "ConnectTimeout",
        "ReadTimeout",
        "PoolTimeout",
        "TimeoutException",
        "NetworkError",
        "RequestError",
    ],
)
def test_classify_byo_generate_error_uses_shared_network_predicate(exc_name):
    provider = _provider()
    inner = type(exc_name, (Exception,), {})(f"{exc_name} failed")
    exc = RuntimeError("outer")
    exc.__cause__ = inner

    classified = provider._classify_byo_generate_error(exc)

    assert classified.reason_code == "local_endpoint_unreachable"
    assert str(classified) == provider.LOCAL_ENDPOINT_UNREACHABLE_COPY


def test_classify_byo_generate_error_500_stays_contract_failed():
    provider = _provider()

    class InternalServerError(Exception):
        status_code = 500

    classified = provider._classify_byo_generate_error(
        InternalServerError("server failed")
    )

    assert classified.reason_code == "local_endpoint_contract_failed"
    assert str(classified) == provider.LOCAL_ENDPOINT_CONTRACT_COPY


def test_run_generate_byo_http_status_maps_to_contract_failed(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(provider, "resolve_local_endpoint", _byo_endpoint)

    import httpx

    request = httpx.Request("POST", "http://byo.example/openai/v1/chat/completions")
    response = httpx.Response(400, request=request)

    class Response:
        def raise_for_status(self):
            raise httpx.HTTPStatusError(
                "bad request", request=request, response=response
            )

    monkeypatch.setattr(httpx, "post", lambda *args, **kwargs: Response())

    with pytest.raises(provider.LocalProviderError) as exc:
        provider.run_generate("hello", model=LOCAL_MODEL)

    assert exc.value.reason_code == "local_endpoint_contract_failed"
    assert str(exc.value) == provider.LOCAL_ENDPOINT_CONTRACT_COPY


def test_run_generate_byo_invalid_shape_maps_to_response_invalid(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(provider, "resolve_local_endpoint", _byo_endpoint)

    class Response:
        def raise_for_status(self):
            return None

        def json(self):
            return {"choices": []}

    import httpx

    monkeypatch.setattr(httpx, "post", lambda *args, **kwargs: Response())

    with pytest.raises(provider.LocalProviderError) as exc:
        provider.run_generate("hello", model=LOCAL_MODEL)

    assert exc.value.reason_code == "provider_response_invalid"
    assert str(exc.value) == "No response from model."


def test_run_agenerate_byo_invalid_shape_maps_to_response_invalid(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(provider, "resolve_local_endpoint", _byo_endpoint)

    class Response:
        def raise_for_status(self):
            return None

        def json(self):
            return {"choices": []}

    class AsyncClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, exc_type, exc, tb):
            return None

        async def post(self, *_args, **_kwargs):
            return Response()

    import httpx

    monkeypatch.setattr(httpx, "AsyncClient", AsyncClient)

    with pytest.raises(provider.LocalProviderError) as exc:
        asyncio.run(provider.run_agenerate("hello", model=LOCAL_MODEL))

    assert exc.value.reason_code == "provider_response_invalid"
    assert str(exc.value) == "No response from model."


def test_run_generate_byo_json_decode_maps_to_contract_failed(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(provider, "resolve_local_endpoint", _byo_endpoint)

    class Response:
        def raise_for_status(self):
            return None

        def json(self):
            raise json.JSONDecodeError("bad json", "not-json", 0)

    import httpx

    monkeypatch.setattr(httpx, "post", lambda *args, **kwargs: Response())

    with pytest.raises(provider.LocalProviderError) as exc:
        provider.run_generate("hello", model=LOCAL_MODEL)

    assert exc.value.reason_code == "local_endpoint_contract_failed"
    assert str(exc.value) == provider.LOCAL_ENDPOINT_CONTRACT_COPY


def test_run_generate_malformed_response_matches_bundled_and_byo(monkeypatch):
    provider = _provider()
    malformed = {"choices": []}

    class Response:
        def raise_for_status(self):
            return None

        def json(self):
            return malformed

    def fake_post(*_args, **_kwargs):
        return Response()

    import httpx

    monkeypatch.setattr(httpx, "post", fake_post)

    monkeypatch.setattr(provider, "resolve_local_endpoint", _byo_endpoint)
    with pytest.raises(provider.LocalProviderError) as byo_exc:
        provider.run_generate("hello", model=LOCAL_MODEL)

    monkeypatch.setattr(provider, "resolve_local_endpoint", _bundled_endpoint)
    _patch_bundled_server(monkeypatch)
    with pytest.raises(provider.LocalProviderError) as bundled_exc:
        provider.run_generate("hello", model=LOCAL_MODEL)

    assert byo_exc.value.reason_code == bundled_exc.value.reason_code
    assert byo_exc.value.reason_code == "provider_response_invalid"
    assert str(byo_exc.value) == str(bundled_exc.value)


def test_bundled_local_admission_timeout_reason_code_escapes(monkeypatch):
    provider = _provider()
    _patch_bundled_server(monkeypatch)

    from solstone.think.providers import local_admission

    def raise_timeout(*_args, **_kwargs):
        raise local_admission.LocalAdmissionTimeout("busy")

    async def raise_timeout_async(*_args, **_kwargs):
        raise local_admission.LocalAdmissionTimeout("busy")

    monkeypatch.setattr(local_admission, "acquire_local_slot", raise_timeout)
    monkeypatch.setattr(
        local_admission, "acquire_local_slot_async", raise_timeout_async
    )

    with pytest.raises(local_admission.LocalAdmissionTimeout) as sync_exc:
        provider.run_generate("hello", model=LOCAL_MODEL)
    assert sync_exc.value.reason_code == "local_queue_timeout"

    with pytest.raises(local_admission.LocalAdmissionTimeout) as async_exc:
        asyncio.run(provider.run_agenerate("hello", model=LOCAL_MODEL))
    assert async_exc.value.reason_code == "local_queue_timeout"

    with pytest.raises(local_admission.LocalAdmissionTimeout) as cogitate_exc:
        asyncio.run(provider.run_cogitate({"model": LOCAL_MODEL}))
    assert cogitate_exc.value.reason_code == "local_queue_timeout"


def test_run_cogitate_byo_acquires_permit_and_records_no_telemetry(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(
        provider,
        "resolve_local_endpoint",
        lambda: _byo_endpoint(parallel_slots=1),
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: (_ for _ in ()).throw(AssertionError("connect not expected")),
    )

    from solstone.think.providers import local_admission

    records = []
    monkeypatch.setattr(local_admission, "record_local_inference", records.append)

    async def fake_cogitate(*_args, slot_lease=None, **_kwargs):
        assert slot_lease is not None
        slot_lease.yield_slot()
        with local_admission.acquire_local_slot(1, 0.1) as nested:
            assert nested.slot_index == 0
        slot_lease.reacquire()
        with pytest.raises(local_admission.LocalAdmissionTimeout):
            local_admission.acquire_local_slot(1, 0.03)
        return "ok"

    monkeypatch.setattr(
        "solstone.think.providers.openhands.run_cogitate",
        fake_cogitate,
    )

    result = asyncio.run(
        provider.run_cogitate({"model": LOCAL_MODEL, "timeout_seconds": 1})
    )

    assert result == "ok"
    assert records == []
    with local_admission.acquire_local_slot(1, 0.1) as permit:
        assert permit.slot_index == 0


def test_run_cogitate_byo_keeps_permit_for_non_sol_work(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(
        provider,
        "resolve_local_endpoint",
        lambda: _byo_endpoint(parallel_slots=1),
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: (_ for _ in ()).throw(AssertionError("connect not expected")),
    )

    from solstone.think.providers import local_admission

    async def fake_cogitate(*_args, slot_lease=None, **_kwargs):
        assert slot_lease is not None
        with pytest.raises(local_admission.LocalAdmissionTimeout):
            local_admission.acquire_local_slot(1, 0.03)
        return "ok"

    monkeypatch.setattr(
        "solstone.think.providers.openhands.run_cogitate",
        fake_cogitate,
    )

    assert (
        asyncio.run(provider.run_cogitate({"model": LOCAL_MODEL, "timeout_seconds": 1}))
        == "ok"
    )


@pytest.mark.parametrize("bundled", [False, True])
def test_run_cogitate_reacquire_timeout_preserves_exact_type(
    monkeypatch,
    bundled,
):
    provider = _provider()
    if bundled:
        _patch_bundled_server(monkeypatch)
    else:
        monkeypatch.setattr(
            provider,
            "resolve_local_endpoint",
            lambda: _byo_endpoint(parallel_slots=1),
        )
        monkeypatch.setattr(
            "solstone.think.providers.local_server.connect",
            lambda: (_ for _ in ()).throw(AssertionError("connect not expected")),
        )

    from solstone.think.providers import local_admission

    async def fake_cogitate(*_args, slot_lease=None, **_kwargs):
        assert slot_lease is not None
        slot_lease.yield_slot()
        holder = local_admission.acquire_local_slot(1, 0.1)
        try:
            slot_lease.reacquire()
        finally:
            holder.release()

    monkeypatch.setattr(
        "solstone.think.providers.openhands.run_cogitate",
        fake_cogitate,
    )

    with pytest.raises(local_admission.LocalAdmissionTimeout) as exc:
        asyncio.run(
            provider.run_cogitate({"model": LOCAL_MODEL, "timeout_seconds": 0.03})
        )

    assert exc.type is local_admission.LocalAdmissionTimeout
    assert exc.value.reason_code == "local_queue_timeout"
    assert not list(Path(local_admission._admission_dir()).glob("wait-*.ticket"))


def test_run_cogitate_byo_queue_timeout_preserves_exact_type(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(
        provider,
        "resolve_local_endpoint",
        lambda: _byo_endpoint(parallel_slots=1),
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: (_ for _ in ()).throw(AssertionError("connect not expected")),
    )

    async def fail_if_called(*_args, **_kwargs):
        raise AssertionError("openhands must not run after queue timeout")

    from solstone.think.providers import local_admission

    monkeypatch.setattr(
        "solstone.think.providers.openhands.run_cogitate",
        fail_if_called,
    )

    holder = local_admission.acquire_local_slot(1, 0.1)
    try:
        with pytest.raises(local_admission.LocalAdmissionTimeout) as exc:
            asyncio.run(
                provider.run_cogitate({"model": LOCAL_MODEL, "timeout_seconds": 0.03})
            )
        assert exc.type is local_admission.LocalAdmissionTimeout
        assert exc.value.reason_code == "local_queue_timeout"
    finally:
        holder.release()


def test_run_cogitate_confidential_with_stray_slots_skips_admission(monkeypatch):
    provider = _provider()
    configured_endpoint = "https://spp.example.test"
    config = {
        "providers": {
            "local": {
                "endpoint_url": configured_endpoint,
                "served_model_id": "confidential-model",
                "credential": "confidential-token",
                "parallel_slots": 1,
            }
        },
        "services": {"confidential": {"account_id": "acct"}},
    }
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: (_ for _ in ()).throw(AssertionError("connect not expected")),
    )

    from solstone.think.providers import local_admission, local_endpoint

    async def fail_acquire(*_args, **_kwargs):
        raise AssertionError("confidential cogitate must not acquire admission")

    async def fake_cogitate(*_args, **_kwargs):
        return "ok"

    monkeypatch.setattr(local_endpoint, "read_journal_config", lambda: config)
    monkeypatch.setattr(local_admission, "acquire_local_slot_async", fail_acquire)
    monkeypatch.setattr(
        "solstone.think.providers.openhands.run_cogitate",
        fake_cogitate,
    )

    assert asyncio.run(provider.run_cogitate({"model": LOCAL_MODEL})) == "ok"


def test_run_generate_bundled_context_budget_exceeded_reason_code_escapes(
    monkeypatch,
):
    provider = _provider()
    _patch_bundled_server(monkeypatch)

    def raise_context_budget(**_kwargs):
        raise provider.ContextBudgetExceeded("too large")

    monkeypatch.setattr(provider, "_prepare_bundled_request", raise_context_budget)

    with pytest.raises(provider.ContextBudgetExceeded) as exc:
        provider.run_generate("hello", model=LOCAL_MODEL)

    assert exc.value.reason_code == "context_budget_exceeded"


def test_run_cogitate_byo_classified_error_uses_fixed_copy_and_redacts(
    monkeypatch,
):
    provider = _provider()
    token = "test-token-PLACEHOLDER"
    events: list[dict] = []

    class BadRequestError(RuntimeError):
        status_code = 400

    async def fail_cogitate(*_args, **_kwargs):
        raise BadRequestError(f"bad request with {token}")

    monkeypatch.setattr(
        provider, "resolve_local_endpoint", lambda: _byo_endpoint(token)
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: (_ for _ in ()).throw(AssertionError("connect not expected")),
    )
    monkeypatch.setattr(
        "solstone.think.providers.openhands.run_cogitate",
        fail_cogitate,
    )

    with pytest.raises(provider.LocalProviderError) as exc:
        asyncio.run(
            provider.run_cogitate({"model": LOCAL_MODEL}, on_event=events.append)
        )

    assert exc.value.reason_code == "local_endpoint_contract_failed"
    assert str(exc.value) == provider.LOCAL_ENDPOINT_CONTRACT_COPY
    assert token not in str(exc.value)
    assert getattr(exc.value, "_evented") is True
    assert events[0]["error"] == provider.LOCAL_ENDPOINT_CONTRACT_COPY
    assert events[0]["reason_code"] == "local_endpoint_contract_failed"
    assert token not in events[0]["trace"]


def test_run_cogitate_byo_connection_error_records_no_success_telemetry(
    monkeypatch,
):
    provider = _provider()
    sentinel = "SENTINEL-BYO-CRED-9f3a2b"
    events: list[dict] = []
    records: list[dict] = []

    monkeypatch.setattr(
        provider,
        "resolve_local_endpoint",
        lambda: _byo_endpoint(sentinel),
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: (_ for _ in ()).throw(AssertionError("connect not expected")),
    )

    from solstone.think.providers import local_admission

    monkeypatch.setattr(local_admission, "record_local_inference", records.append)

    async def fail_cogitate(*_args, **_kwargs):
        import httpx

        from solstone.think.providers.local_endpoint import redact_exception_credential

        exc = httpx.ConnectError(f"connection refused {sentinel}")
        raise redact_exception_credential(exc, sentinel)

    monkeypatch.setattr(
        "solstone.think.providers.openhands.run_cogitate",
        fail_cogitate,
    )

    with pytest.raises(provider.LocalProviderError) as exc:
        asyncio.run(
            provider.run_cogitate({"model": LOCAL_MODEL}, on_event=events.append)
        )

    assert exc.value.reason_code == "local_endpoint_unreachable"
    assert str(exc.value) == provider.LOCAL_ENDPOINT_UNREACHABLE_COPY
    assert sentinel not in str(exc.value)
    serialized = "".join(
        traceback.format_exception(
            type(exc.value),
            exc.value,
            exc.value.__traceback__,
        )
    )
    assert sentinel not in serialized
    assert sentinel not in json.dumps(events)
    assert records == []


def test_run_cogitate_talent_hook_error_bypasses_local_error_event(monkeypatch):
    provider = _provider()
    events: list[dict] = []
    hook_exc = TalentHookError(
        "post",
        "broken_hook",
        "chat",
        RuntimeError("hook exploded"),
    )

    async def fail_cogitate(*_args, **_kwargs):
        raise hook_exc

    monkeypatch.setattr(provider, "resolve_local_endpoint", _byo_endpoint)
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: (_ for _ in ()).throw(AssertionError("connect not expected")),
    )
    monkeypatch.setattr(
        "solstone.think.providers.openhands.run_cogitate",
        fail_cogitate,
    )

    with pytest.raises(TalentHookError) as raised:
        asyncio.run(
            provider.run_cogitate({"model": LOCAL_MODEL}, on_event=events.append)
        )

    assert raised.value is hook_exc
    assert events == []
    assert not getattr(hook_exc, "_evented", False)


@pytest.mark.parametrize(
    ("credential", "expected_key"),
    [
        ("test-token-PLACEHOLDER", "test-token-PLACEHOLDER"),
        (None, "EMPTY"),
    ],
)
def test_openhands_local_byo_llm_kwargs(monkeypatch, credential, expected_key):
    from solstone.think.providers import local_endpoint, openhands

    captured = {}

    class FakeLLM:
        def __init__(self, **kwargs):
            captured.update(kwargs)

    sdk_module = types.ModuleType("openhands.sdk")
    sdk_module.LLM = FakeLLM
    monkeypatch.setitem(sys.modules, "openhands.sdk", sdk_module)
    monkeypatch.setattr(
        local_endpoint,
        "resolve_local_endpoint",
        lambda: _byo_endpoint(credential),
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: (_ for _ in ()).throw(AssertionError("connect not expected")),
    )

    llm = openhands._build_llm("local", LOCAL_MODEL)

    assert isinstance(llm, FakeLLM)
    assert captured == {
        "model": "openai/served-model",
        "base_url": "http://byo.example/openai/v1",
        "api_key": expected_key,
        "native_tool_calling": False,
        "timeout": openhands.LLM_TIMEOUT_S,
        "num_retries": openhands.LLM_NUM_RETRIES,
        "retry_min_wait": 1,
        "retry_max_wait": 2,
        "retry_multiplier": 1.0,
        "input_cost_per_token": 0,
        "output_cost_per_token": 0,
        "litellm_extra_body": {"chat_template_kwargs": {"enable_thinking": False}},
    }
    assert "max_input_tokens" not in captured
    waits = [
        min(
            captured["retry_max_wait"],
            max(
                captured["retry_min_wait"],
                captured["retry_multiplier"] * 2 ** (k - 1),
            ),
        )
        for k in range(1, captured["num_retries"])
    ]
    assert sum(waits) == 1.0
    assert sum(waits) < openhands.WALL_CLOCK_GRACE_S


def test_openhands_local_confidential_llm_uses_forwarder(monkeypatch):
    from solstone.think.providers import local_endpoint, openhands

    configured_endpoint = "https://spp.example.test"
    forwarder = "http://127.0.0.1:4567"
    captured = {}

    class FakeLLM:
        def __init__(self, **kwargs):
            captured.update(kwargs)

    sdk_module = types.ModuleType("openhands.sdk")
    sdk_module.LLM = FakeLLM
    monkeypatch.setitem(sys.modules, "openhands.sdk", sdk_module)
    monkeypatch.setattr(
        local_endpoint,
        "resolve_local_endpoint",
        lambda: local_endpoint.LocalEndpoint(
            base_url=configured_endpoint,
            served_model_id="confidential-model",
            credential="confidential-token",
            is_bundled=False,
        ),
    )
    monkeypatch.setattr(
        "solstone.think.services.spp_transport.confidential_egress_base_url",
        lambda base_url: forwarder if base_url == configured_endpoint else base_url,
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: (_ for _ in ()).throw(AssertionError("connect not expected")),
    )

    llm = openhands._build_llm("local", LOCAL_MODEL)

    assert isinstance(llm, FakeLLM)
    assert captured["base_url"] == f"{forwarder}/v1"
    assert configured_endpoint not in captured["base_url"]


def test_local_context_window_split_floor_vs_tier():
    import inspect

    from solstone.think import supervisor
    from solstone.think.providers import local_server, openhands

    assert local_server.LOCAL_MIN_CONTEXT_TOKENS == 16384
    removed_name = "_".join(("LOCAL", "SERVER", "CONTEXT", "TOKENS"))
    assert not hasattr(local_server, removed_name)
    src = inspect.getsource(supervisor.start_local_server)
    assert "select_server_tier" in src
    assert "tier.context_tokens" in src
    assert '"16384"' not in src
    llm_src = inspect.getsource(openhands._build_llm)
    assert "LOCAL_MIN_CONTEXT_TOKENS" in llm_src


def test_select_server_tier_vram_thresholds():
    from solstone.think.providers import local_server

    cases = [
        (
            0,
            local_server.ServerTier(
                name="floor",
                context_tokens=16384,
                parallel_slots=1,
                prompt_cache_mib=0,
                resident_mib=4541,
            ),
        ),
        (
            15999,
            local_server.ServerTier(
                name="floor",
                context_tokens=16384,
                parallel_slots=1,
                prompt_cache_mib=0,
                resident_mib=4541,
            ),
        ),
        (
            16000,
            local_server.ServerTier(
                name="capable",
                context_tokens=32768,
                parallel_slots=2,
                prompt_cache_mib=2048,
                resident_mib=None,
            ),
        ),
        (
            24576,
            local_server.ServerTier(
                name="capable",
                context_tokens=32768,
                parallel_slots=2,
                prompt_cache_mib=2048,
                resident_mib=None,
            ),
        ),
    ]

    for vram_mib, expected in cases:
        tier = local_server.select_server_tier(vram_mib)
        assert tier == expected
        assert tier.context_tokens >= 16384
        assert tier.context_tokens > 0
    assert local_server._FLOOR_TIER.resident_mib == 4541
    assert local_server._CAPABLE_TIER.resident_mib is None


@pytest.mark.parametrize(
    ("props", "expected"),
    [
        ({"n_ctx": 32768}, 32768),
        ({"default_generation_settings": {"n_ctx": 16384}}, 16384),
        (
            {"n_ctx": 32768, "default_generation_settings": {"n_ctx": 16384}},
            32768,
        ),
        ({}, None),
        ({"default_generation_settings": {}}, None),
        ({"n_ctx": "abc"}, None),
        ({"n_ctx": None}, None),
        # Numeric strings are acceptable because _extract_n_ctx intentionally
        # uses int() coercion on reported llama-server values.
        ({"n_ctx": "32768"}, 32768),
    ],
)
def test_extract_n_ctx_props_shapes(props, expected):
    from solstone.think.providers import local_server

    assert local_server._extract_n_ctx(props) == expected


def test_read_server_context_window_fetch_props(monkeypatch):
    import httpx

    from solstone.think.providers import local_server

    class FakeResponse:
        status_code = 200

        def __init__(self, body=None, error: Exception | None = None):
            self.body = body
            self.error = error

        def json(self):
            if self.error is not None:
                raise self.error
            return self.body

    monkeypatch.setattr(
        httpx,
        "get",
        lambda url, timeout: FakeResponse({"n_ctx": 32768, "total_slots": 2}),
    )
    assert local_server.read_server_context_window(2468) == 32768

    monkeypatch.setattr(
        httpx,
        "get",
        lambda url, timeout: FakeResponse(error=ValueError("bad json")),
    )
    assert local_server.read_server_context_window(2468) is None

    monkeypatch.setattr(httpx, "get", lambda url, timeout: FakeResponse(["n_ctx"]))
    assert local_server.read_server_context_window(2468) is None

    def raise_get(url, timeout):
        raise RuntimeError("network down")

    monkeypatch.setattr(httpx, "get", raise_get)
    assert local_server.read_server_context_window(2468) is None


def test_context_window_tokens_fallback(monkeypatch):
    from solstone.think import utils
    from solstone.think.providers import local_budget, local_server

    monkeypatch.setattr(utils, "read_service_port", lambda service: 2468)
    monkeypatch.setattr(local_server, "read_server_context_window", lambda port: 32768)
    monkeypatch.setattr(local_server, "read_local_context_window", lambda: None)
    assert local_budget.context_window_tokens() == 32768

    monkeypatch.setattr(local_server, "read_server_context_window", lambda port: None)
    monkeypatch.setattr(local_server, "read_local_context_window", lambda: 32768)
    assert local_budget.context_window_tokens() == 32768

    monkeypatch.setattr(utils, "read_service_port", lambda service: None)
    monkeypatch.setattr(local_server, "read_local_context_window", lambda: None)
    assert local_budget.context_window_tokens() == local_server.LOCAL_MIN_CONTEXT_TOKENS


def test_llama_server_pins_are_real_b9291_digests():
    from solstone.think.providers.local_install import LLAMA_SERVER_PINS

    mac = LLAMA_SERVER_PINS["aarch64-apple-darwin"]
    linux = LLAMA_SERVER_PINS["x86_64-unknown-linux-gnu"]
    assert mac["release_tag"] == "b9291"
    assert mac["filename"] == "llama-b9291-bin-macos-arm64.tar.gz"
    assert (
        mac["sha256"]
        == "0e985f87dd71f96a9cb9ebc3ad26f8388030342d000e7e82d4a38d14913373ff"
    )
    assert linux["release_tag"] == "b9291"
    assert linux["filename"] == "llama-b9291-bin-ubuntu-vulkan-x64.tar.gz"
    assert (
        linux["sha256"]
        == "7e3bf4202bedc71c2c9fbfbe02d10075b8d596bb963e7ab006663582dc2e92c2"
    )


def _select_local_provider(monkeypatch) -> None:
    monkeypatch.setattr(
        "solstone.think.models.get_config",
        lambda: {
            "providers": {"active": {"provider": "local", "model": "local/qwen3.5-4b"}}
        },
    )


def test_build_provider_status_local_not_selected_is_inert(monkeypatch):
    from solstone.think.providers import build_provider_status

    health_calls = []
    monkeypatch.setattr(
        "solstone.think.models.get_config",
        lambda: {
            "providers": {
                "active": {"provider": "google", "model": "gemini-flash-latest"}
            }
        },
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_install.inspect_readiness",
        lambda: {
            "binary_installed": True,
            "model_installed": True,
            "ram_sufficient": True,
            "gpu_available": True,
            "binary_path": "/fake/llama-server",
        },
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.is_healthy",
        lambda: health_calls.append("health") or True,
    )

    status = build_provider_status(
        [{"name": "local", "label": "Local (on-device)", "env_key": ""}]
    )["local"]

    assert status["selected"] is False
    assert status["configured"] is True
    assert status["generate_ready"] is False
    assert status["cogitate_ready"] is False
    assert status["issues"] == []
    assert health_calls == []


def test_build_provider_status_local_readiness(monkeypatch):
    from solstone.think.providers import build_provider_status

    _select_local_provider(monkeypatch)
    monkeypatch.setattr(
        "solstone.think.providers.local_install.inspect_readiness",
        lambda: {
            "binary_installed": True,
            "model_installed": True,
            "ram_sufficient": True,
            "gpu_available": True,
        },
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.is_healthy", lambda: True
    )

    status = build_provider_status(
        [{"name": "local", "label": "Local (on-device)", "env_key": ""}]
    )["local"]

    assert status["configured"] is True
    assert status["generate_ready"] is True
    assert status["cogitate_ready"] is True
    assert status["issues"] == []


def test_build_provider_status_local_launch_failure_adds_probe_detail_and_hint(
    monkeypatch,
):
    from solstone.think.providers import build_provider_status

    _select_local_provider(monkeypatch)
    detail = "dyld: Library not loaded: @rpath/libllama.dylib"
    monkeypatch.setattr(
        "solstone.think.providers.local_install.inspect_readiness",
        lambda: {
            "binary_installed": True,
            "model_installed": True,
            "ram_sufficient": True,
            "gpu_available": True,
            "binary_path": "/fake/llama-server",
        },
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.is_healthy", lambda: False
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_install.probe_binary_runnable",
        lambda _path: (False, detail),
    )

    status = build_provider_status(
        [{"name": "local", "label": "Local (on-device)", "env_key": ""}]
    )["local"]

    assert status["issues"] == [
        f"failed to launch: {detail}",
        "run `journal install-provider local`",
    ]
    assert "server_unhealthy" not in status["issues"]


def test_build_provider_status_local_server_unhealthy_when_probe_runnable(
    monkeypatch,
):
    from solstone.think.providers import build_provider_status

    _select_local_provider(monkeypatch)
    monkeypatch.setattr(
        "solstone.think.providers.local_install.inspect_readiness",
        lambda: {
            "binary_installed": True,
            "model_installed": True,
            "ram_sufficient": True,
            "gpu_available": True,
            "binary_path": "/fake/llama-server",
        },
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.is_healthy", lambda: False
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_install.probe_binary_runnable",
        lambda _path: (True, None),
    )

    status = build_provider_status(
        [{"name": "local", "label": "Local (on-device)", "env_key": ""}]
    )["local"]

    assert status["issues"] == ["server_unhealthy"]


def test_build_provider_status_local_healthy_skips_probe(monkeypatch):
    from solstone.think.providers import build_provider_status

    _select_local_provider(monkeypatch)
    calls: list[str] = []

    def probe(_path):
        calls.append(_path)
        return False, "should not run"

    monkeypatch.setattr(
        "solstone.think.providers.local_install.inspect_readiness",
        lambda: {
            "binary_installed": True,
            "model_installed": True,
            "ram_sufficient": True,
            "gpu_available": True,
            "binary_path": "/fake/llama-server",
        },
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.is_healthy", lambda: True
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_install.probe_binary_runnable", probe
    )

    status = build_provider_status(
        [{"name": "local", "label": "Local (on-device)", "env_key": ""}]
    )["local"]

    assert status["issues"] == []
    assert calls == []


def test_local_provider_status_carries_install_hint_substring(monkeypatch):
    from solstone.think.providers import build_provider_status

    _select_local_provider(monkeypatch)
    monkeypatch.setattr(
        "solstone.think.providers.local_install.inspect_readiness",
        lambda: {
            "binary_installed": False,
            "model_installed": False,
            "ram_sufficient": False,
            "gpu_available": True,
        },
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.is_healthy", lambda: False
    )

    status = build_provider_status(
        [{"name": "local", "label": "Local (on-device)", "env_key": ""}]
    )["local"]

    assert status["configured"] is False
    assert status["generate_ready"] is False
    assert status["cogitate_ready"] is False
    assert status["issues"] == [
        "binary_missing",
        "model_missing",
        "run `journal install-provider local`",
    ]
    assert any("journal install-provider local" in issue for issue in status["issues"])


def test_local_provider_status_reports_gpu_unavailable_issue(monkeypatch):
    from solstone.think.providers import build_provider_status

    _select_local_provider(monkeypatch)
    monkeypatch.setattr(
        "solstone.think.providers.local_install.inspect_readiness",
        lambda: {
            "binary_installed": True,
            "model_installed": True,
            "ram_sufficient": True,
            "gpu_available": False,
            "binary_path": "/fake/llama-server",
        },
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.is_healthy", lambda: True
    )

    status = build_provider_status(
        [{"name": "local", "label": "Local (on-device)", "env_key": ""}]
    )["local"]

    assert status["issues"] == ["gpu_unavailable"]


def test_build_provider_status_local_configured_ignores_ram_flag(monkeypatch):
    from solstone.think.providers import build_provider_status

    _select_local_provider(monkeypatch)
    monkeypatch.setattr(
        "solstone.think.providers.local_install.inspect_readiness",
        lambda: {
            "binary_installed": True,
            "model_installed": True,
            "ram_sufficient": False,
            "gpu_available": True,
            "binary_path": "/fake/llama-server",
        },
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.is_healthy", lambda: True
    )

    status = build_provider_status(
        [{"name": "local", "label": "Local (on-device)", "env_key": ""}]
    )["local"]

    assert status["configured"] is True
    assert status["generate_ready"] is True
    assert status["cogitate_ready"] is True
    assert status["issues"] == []


def test_local_server_connect_returns_healthy_service(monkeypatch):
    from solstone.think.providers import local_server

    monkeypatch.setattr(local_server, "read_service_port", lambda service: 2468)
    monkeypatch.setattr(
        local_server,
        "_fetch_health",
        lambda port: ("ready", None, {"loaded_model": "/path/to/snapshot"}),
    )

    info = local_server.connect()

    assert info.model_id == LOCAL_MODEL
    assert info.served_model_id == "/path/to/snapshot"
    assert info.base_url == "http://127.0.0.1:2468"
    assert info.state == local_server.STATE_READY


def test_resolve_served_model_id_returns_valid_loaded_model_verbatim():
    from solstone.think.providers import local_server

    assert (
        local_server._resolve_served_model_id({"loaded_model": "/snap/dir"})
        == "/snap/dir"
    )


def test_resolve_served_model_id_falls_back_when_loaded_model_absent():
    from solstone.think.providers import local_server

    assert local_server._resolve_served_model_id({}) == LOCAL_MODEL
    assert local_server._resolve_served_model_id(None) == LOCAL_MODEL


@pytest.mark.parametrize(
    "body",
    [
        {"loaded_model": None},
        {"loaded_model": ""},
        {"loaded_model": "   "},
        {"loaded_model": 123},
    ],
)
def test_resolve_served_model_id_rejects_invalid_loaded_model(body):
    from solstone.think.providers import local_server

    assert local_server._resolve_served_model_id(body) is None


def test_local_server_connect_missing_port_raises_named_copy(monkeypatch):
    from solstone.think.providers import local_server

    monkeypatch.setattr(local_server, "read_service_port", lambda service: None)

    with pytest.raises(local_server.LocalProviderError) as exc:
        local_server.connect()

    assert exc.value.reason_code == "local_model_not_ready"
    assert str(exc.value) == local_server.LOCAL_MODEL_NOT_READY_COPY


def test_local_server_connect_failed_health_raises_named_copy(monkeypatch):
    from solstone.think.providers import local_server

    monkeypatch.setattr(local_server, "read_service_port", lambda service: 2468)
    monkeypatch.setattr(
        local_server, "_fetch_health", lambda port: ("starting", None, None)
    )

    with pytest.raises(local_server.LocalProviderError) as exc:
        local_server.connect()

    assert exc.value.reason_code == "local_model_not_ready"
    assert str(exc.value) == local_server.LOCAL_MODEL_NOT_READY_COPY


@pytest.mark.parametrize(
    "body",
    [
        {"loaded_model": None},
        {"loaded_model": ""},
    ],
)
def test_local_server_connect_invalid_loaded_model_raises_named_copy(monkeypatch, body):
    from solstone.think.providers import local_server

    monkeypatch.setattr(local_server, "read_service_port", lambda service: 2468)
    monkeypatch.setattr(
        local_server, "_fetch_health", lambda port: ("ready", None, body)
    )

    with pytest.raises(local_server.LocalProviderError) as exc:
        local_server.connect()

    assert exc.value.reason_code == "local_model_not_ready"
    assert str(exc.value) == local_server.LOCAL_MODEL_NOT_READY_COPY


def test_local_server_connect_linux_health_shape_uses_logical_model(monkeypatch):
    from solstone.think.providers import local_server

    monkeypatch.setattr(local_server, "read_service_port", lambda service: 2468)
    monkeypatch.setattr(
        local_server,
        "_fetch_health",
        lambda port: ("ready", None, {"status": "ok"}),
    )

    info = local_server.connect()

    assert info.model_id == LOCAL_MODEL
    assert info.served_model_id == LOCAL_MODEL


# --- read_server_parallel_slots ---------------------------------------------


def _local_journal(monkeypatch, tmp_path: Path) -> Path:
    from solstone.think.providers import local_server

    journal = tmp_path / "journal"
    (journal / "health").mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    local_server.reset_parallel_slots_cache()
    return journal


def test_read_server_parallel_slots_prefers_live_props(monkeypatch, tmp_path):
    from solstone.think.providers import local_server

    journal = _local_journal(monkeypatch, tmp_path)
    (journal / "health" / "local.port").write_text("2468")
    # A launch-time context window that maps to the floor tier's single slot.
    (journal / "health" / "local.ctx").write_text(
        str(local_server._FLOOR_TIER.context_tokens)
    )
    monkeypatch.setattr(
        local_server,
        "fetch_props",
        lambda port, timeout_s=1.0: {"n_ctx": 32768, "total_slots": 2},
    )

    # /props is ground truth; it wins over the persisted tier.
    assert local_server.read_server_parallel_slots() == 2
    assert local_server.read_server_capacity() == local_server.ServerCapacity(
        parallel_slots=2,
        source="props",
        profile="capable",
    )


def test_read_server_parallel_slots_no_port_returns_floor(
    monkeypatch, tmp_path, caplog
):
    from solstone.think.providers import local_server

    _local_journal(monkeypatch, tmp_path)

    def _no_network(port, timeout_s=1.0):
        raise AssertionError("fetch_props must not run without a port")

    monkeypatch.setattr(local_server, "fetch_props", _no_network)

    caplog.set_level(logging.INFO)
    assert local_server.read_server_parallel_slots() == 1
    assert (
        "local_server_parallel_slots fallback slots=1 port=None "
        "context_tokens=None source=default" in caplog.text
    )


def test_server_capacity_uses_explicit_apple_profile(monkeypatch, tmp_path):
    from solstone.think.providers import local_server

    _local_journal(monkeypatch, tmp_path)
    monkeypatch.setattr(local_server.sys, "platform", "darwin")

    assert local_server.read_server_capacity() == local_server.ServerCapacity(
        parallel_slots=1,
        source="default",
        profile="apple",
    )


@pytest.mark.parametrize("slots", [1, 2])
def test_read_server_parallel_slots_falls_back_to_launched_tier(
    monkeypatch, tmp_path, slots
):
    from solstone.think.providers import local_server

    tier = local_server._FLOOR_TIER if slots == 1 else local_server._CAPABLE_TIER
    journal = _local_journal(monkeypatch, tmp_path)
    (journal / "health" / "local.port").write_text("2468")
    (journal / "health" / "local.ctx").write_text(str(tier.context_tokens))
    monkeypatch.setattr(local_server, "fetch_props", lambda port, timeout_s=1.0: None)

    assert local_server.read_server_parallel_slots() == tier.parallel_slots


def test_read_server_parallel_slots_unknown_context_window_returns_floor(
    monkeypatch, tmp_path
):
    from solstone.think.providers import local_server

    journal = _local_journal(monkeypatch, tmp_path)
    (journal / "health" / "local.port").write_text("2468")
    (journal / "health" / "local.ctx").write_text("99999")
    monkeypatch.setattr(local_server, "fetch_props", lambda port, timeout_s=1.0: None)

    assert local_server.read_server_parallel_slots() == 1


@pytest.mark.parametrize("props", [{}, {"total_slots": 0}, {"total_slots": "many"}])
def test_read_server_parallel_slots_rejects_unusable_total_slots(
    monkeypatch, tmp_path, props
):
    from solstone.think.providers import local_server

    journal = _local_journal(monkeypatch, tmp_path)
    (journal / "health" / "local.port").write_text("2468")
    monkeypatch.setattr(local_server, "fetch_props", lambda port, timeout_s=1.0: props)

    assert local_server.read_server_parallel_slots() == 1


def test_read_server_parallel_slots_is_memoized_and_resettable(monkeypatch, tmp_path):
    from solstone.think.providers import local_server

    journal = _local_journal(monkeypatch, tmp_path)
    (journal / "health" / "local.port").write_text("2468")
    calls = []

    def counting_props(port, timeout_s=1.0):
        calls.append(port)
        return {"total_slots": 2}

    monkeypatch.setattr(local_server, "fetch_props", counting_props)

    assert local_server.read_server_parallel_slots() == 2
    assert local_server.read_server_parallel_slots() == 2
    assert len(calls) == 1

    local_server.reset_parallel_slots_cache()
    assert local_server.read_server_parallel_slots() == 2
    assert len(calls) == 2


def test_local_response_telemetry_normalizes_llama_and_mlx_fields():
    provider = _provider()

    fields = provider._server_response_fields(
        {
            "timings": {
                "cache_n": 236,
                "prompt_n": 12,
                "prompt_ms": 30.5,
                "predicted_n": 35,
                "predicted_ms": 661.0,
                "slot_id": 1,
            },
            "usage": {
                "prompt_tokens": 248,
                "completion_tokens": 35,
            },
        }
    )

    assert fields == {
        "prompt_eval_ms": 30.5,
        "generation_ms": 661.0,
        "server_total_ms": 691.5,
        "prompt_tokens": 248,
        "generated_tokens": 35,
        "prompt_cached_tokens": 236,
        "selected_slot": 1,
        "prompt_cache_state": "warm",
    }
    assert provider._extract_usage(
        {
            "usage": {
                "prompt_tokens": 248,
                "completion_tokens": 35,
                "total_tokens": 283,
                "prompt_tokens_details": {"cached_tokens": 236},
            }
        }
    ) == {
        "input_tokens": 248,
        "output_tokens": 35,
        "total_tokens": 283,
        "cached_tokens": 236,
    }
