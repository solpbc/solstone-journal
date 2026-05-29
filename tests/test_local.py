# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import contextlib
import importlib
import sys
import types
from pathlib import Path
from types import SimpleNamespace

import pytest

from solstone.think.models import (
    LOCAL_FLASH,
    LOCAL_LITE,
    LOCAL_PRO,
    PROVIDER_DEFAULTS,
    TIER_FLASH,
    TIER_LITE,
    TIER_PRO,
    get_model_provider,
)


def _provider():
    return importlib.reload(importlib.import_module("solstone.think.providers.local"))


def test_local_model_prefix_maps_to_provider():
    assert get_model_provider(LOCAL_LITE) == "local"
    assert get_model_provider(LOCAL_FLASH) == "local"
    assert get_model_provider(LOCAL_PRO) == "local"


def test_local_model_specs():
    provider = _provider()

    assert set(provider.LOCAL_MODEL_SPECS) == {LOCAL_FLASH, LOCAL_PRO}
    lite = provider.LOCAL_MODEL_SPECS[LOCAL_FLASH]
    pro = provider.LOCAL_MODEL_SPECS[LOCAL_PRO]
    assert lite.repo == "Qwen/Qwen2.5-Coder-7B-Instruct-GGUF"
    assert (
        lite.sha256
        == "509287f78cb4d4cf6b3843734733b914b2c158e43e22a7f4bf5e963800894d3c"
    )
    assert lite.min_ram_bytes == 12 * 1024**3
    assert pro.repo == "giladgd/Qwen3-Coder-30B-A3B-Instruct-Q4_K_M-GGUF"
    assert (
        pro.sha256 == "ab4fc2b27b2043483a9e346c802809dfbe9b775efbeea7ca74dc2fd1aa4a0f71"
    )
    assert pro.min_ram_bytes == 32 * 1024**3


def test_local_provider_defaults_and_registry():
    from solstone.think.providers import PROVIDER_METADATA, PROVIDER_REGISTRY

    assert PROVIDER_DEFAULTS["local"][TIER_PRO] == LOCAL_PRO
    assert PROVIDER_DEFAULTS["local"][TIER_FLASH] == LOCAL_FLASH
    assert PROVIDER_DEFAULTS["local"][TIER_LITE] == LOCAL_LITE
    assert "ollama" not in PROVIDER_DEFAULTS
    assert PROVIDER_REGISTRY["local"] == "solstone.think.providers.local"
    assert "ollama" not in PROVIDER_REGISTRY
    assert PROVIDER_METADATA["local"] == {
        "label": "Local (on-device)",
        "env_key": "",
    }


def test_list_models_returns_specs():
    models = _provider().list_models("local")

    assert [model["model"] for model in models] == [LOCAL_FLASH, LOCAL_PRO]
    assert models[0]["min_ram_bytes"] == 12 * 1024**3


def test_validate_key_uses_tiny_generate(monkeypatch):
    provider = _provider()
    calls = []

    def fake_generate(*args, **kwargs):
        calls.append((args, kwargs))
        return {"text": "OK"}

    monkeypatch.setattr(provider, "run_generate", fake_generate)

    assert provider.validate_key("local", "") == {"valid": True}
    assert calls[0][0] == ("Say OK",)
    assert calls[0][1]["model"] == LOCAL_FLASH
    assert calls[0][1]["max_output_tokens"] == 8


def test_run_generate_posts_to_loopback(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(
        "solstone.think.providers.local_server.ensure_running",
        lambda model_id: SimpleNamespace(port=4321, base_url="http://127.0.0.1:4321"),
    )
    captured = {}

    class Response:
        def raise_for_status(self):
            return None

        def json(self):
            return {
                "model": LOCAL_FLASH,
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

    result = provider.run_generate("hello", model=LOCAL_FLASH, max_output_tokens=16)

    assert captured["url"] == "http://127.0.0.1:4321/v1/chat/completions"
    assert captured["json"]["model"] == LOCAL_FLASH
    assert captured["json"]["messages"] == [{"role": "user", "content": "hello"}]
    assert captured["json"]["max_tokens"] == 16
    assert result["text"] == "hello"
    assert result["usage"] == {
        "input_tokens": 3,
        "output_tokens": 2,
        "total_tokens": 5,
    }


def test_run_generate_rejects_vision_inputs():
    provider = _provider()

    with pytest.raises(provider.LocalProviderError) as exc:
        provider.run_generate([b"\x89PNG\r\n\x1a\nbad"], model=LOCAL_FLASH)

    assert exc.value.reason_code == "unsupported_capability"


def test_openhands_local_llm_kwargs(monkeypatch):
    from solstone.think.providers import openhands

    captured = {}

    class FakeLLM:
        def __init__(self, **kwargs):
            captured.update(kwargs)

    sdk_module = types.ModuleType("openhands.sdk")
    sdk_module.LLM = FakeLLM
    monkeypatch.setitem(sys.modules, "openhands.sdk", sdk_module)
    monkeypatch.setattr(
        "solstone.think.providers.local_server.ensure_running",
        lambda model_id: SimpleNamespace(port=9876),
    )

    llm = openhands._build_llm("local", LOCAL_FLASH)

    assert isinstance(llm, FakeLLM)
    assert captured == {
        "model": f"openai/{LOCAL_FLASH}",
        "base_url": "http://127.0.0.1:9876/v1",
        "api_key": "EMPTY",
        "native_tool_calling": False,
        "input_cost_per_token": 0,
        "chat_template_kwargs": {"enable_thinking": False},
    }
    assert openhands._prefixed_model("local", LOCAL_FLASH) == f"openai/{LOCAL_FLASH}"


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
    assert linux["filename"] == "llama-b9291-bin-ubuntu-x64.tar.gz"
    assert (
        linux["sha256"]
        == "8cb79eb596cc5cc15a6089ceadaa2723e3d75c1e7b37cfb9977ad1d4dc4a41eb"
    )


def test_build_provider_status_local_readiness(monkeypatch):
    from solstone.think.providers import build_provider_status

    monkeypatch.setattr(
        "solstone.think.providers.local_install.inspect_readiness",
        lambda: {
            "binary_installed": True,
            "model_installed": True,
            "ram_sufficient": True,
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
    assert status["cogitate_cli"] == "llama-server"
    assert status["issues"] == []


def test_build_provider_status_local_launch_failure_adds_probe_detail_and_hint(
    monkeypatch,
):
    from solstone.think.providers import build_provider_status

    detail = "dyld: Library not loaded: @rpath/libllama.dylib"
    monkeypatch.setattr(
        "solstone.think.providers.local_install.inspect_readiness",
        lambda: {
            "binary_installed": True,
            "model_installed": True,
            "ram_sufficient": True,
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
        "run `sol call settings providers install local`",
    ]
    assert "server_unhealthy" not in status["issues"]


def test_build_provider_status_local_server_unhealthy_when_probe_runnable(
    monkeypatch,
):
    from solstone.think.providers import build_provider_status

    monkeypatch.setattr(
        "solstone.think.providers.local_install.inspect_readiness",
        lambda: {
            "binary_installed": True,
            "model_installed": True,
            "ram_sufficient": True,
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

    monkeypatch.setattr(
        "solstone.think.providers.local_install.inspect_readiness",
        lambda: {
            "binary_installed": False,
            "model_installed": False,
            "ram_sufficient": False,
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
    assert status["cogitate_cli"] == "llama-server"
    assert status["cogitate_cli_found"] is False
    assert status["issues"] == [
        "binary_missing",
        "model_missing",
        "ram_insufficient",
        "run `sol call settings providers install local`",
    ]
    assert any(
        "sol call settings providers install local" in issue
        for issue in status["issues"]
    )


def test_local_server_spawn_binds_loopback(monkeypatch):
    from solstone.think.providers import local_server

    captured = {}

    class FakeProcess:
        returncode = None

        def poll(self):
            return None

    def fake_spawn(cmd, **kwargs):
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return FakeProcess()

    monkeypatch.setattr(local_server, "_PROCESS", None)
    monkeypatch.setattr(local_server, "_PROCESS_MODEL_ID", None)
    monkeypatch.setattr(local_server, "_PROCESS_PORT", None)
    monkeypatch.setattr(
        local_server, "_server_file_lock", lambda: contextlib.nullcontext()
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_install.ensure_artifacts_installed",
        lambda model_id: (Path("/tmp/llama-server"), Path("/tmp/model.gguf")),
    )
    monkeypatch.setattr(local_server, "find_available_port", lambda host: 2468)
    monkeypatch.setattr(local_server, "write_service_port", lambda service, port: None)
    monkeypatch.setattr(local_server, "read_service_port", lambda service: None)
    monkeypatch.setattr(local_server, "_probe_health", lambda port: ("ready", None))
    monkeypatch.setattr(local_server.RunnerManagedProcess, "spawn", fake_spawn)

    info = local_server.ensure_running(LOCAL_FLASH)

    assert info.base_url == "http://127.0.0.1:2468"
    assert "--host" in captured["cmd"]
    assert captured["cmd"][captured["cmd"].index("--host") + 1] == "127.0.0.1"
    assert "0.0.0.0" not in captured["cmd"]
    assert captured["cmd"] == [
        "/tmp/llama-server",
        "-m",
        "/tmp/model.gguf",
        "--alias",
        LOCAL_FLASH,
        "--host",
        "127.0.0.1",
        "--port",
        "2468",
    ]


def test_migrate_ollama_to_local_idempotent():
    from solstone.apps.settings.maint._migrate_ollama_to_local import migrate_config

    config = {
        "providers": {
            "generate": {
                "provider": "ollama",
                "backup": "ollama",
                "model": "ollama-local/qwen3.5:9b",
            },
            "cogitate": {
                "provider": "ollama",
                "backup": "anthropic",
                "model": "ollama-local/qwen3.5:35b-a3b-bf16",
            },
            "models": {
                "ollama": {
                    "1": "ollama-local/qwen3.5:2b",
                    "2": "ollama-local/qwen3.5:9b",
                    "3": "ollama-local/qwen3.5:35b-a3b-bf16",
                    "custom": "ollama-local/custom-model",
                }
            },
            "auth": {"ollama": "platform"},
            "key_validation": {"ollama": {"valid": True}},
            "api_keys": {"ollama": True},
            "contexts": {
                "test.ollama": {
                    "provider": "ollama",
                    "model": "ollama-local/qwen3.5:2b",
                }
            },
        }
    }

    migrated, report = migrate_config(config)

    assert report["changed"] is True
    providers = migrated["providers"]
    assert providers["generate"]["provider"] == "local"
    assert providers["generate"]["backup"] == "local"
    assert providers["generate"]["model"] == LOCAL_FLASH
    assert providers["cogitate"]["provider"] == "local"
    assert providers["cogitate"]["backup"] == "anthropic"
    assert providers["cogitate"]["model"] == LOCAL_PRO
    assert "ollama" not in providers["models"]
    assert providers["models"]["local"] == {
        "1": LOCAL_LITE,
        "2": LOCAL_FLASH,
        "3": LOCAL_PRO,
        "custom": "local/custom-model",
    }
    assert providers["auth"] == {"local": "platform"}
    assert providers["key_validation"] == {"local": {"valid": True}}
    assert providers["api_keys"] == {"ollama": True}
    assert providers["contexts"]["test.ollama"] == {
        "provider": "local",
        "model": LOCAL_LITE,
    }
    assert any(
        change.get("warning") == "unsupported_model" for change in report["changes"]
    )

    migrated_again, report_again = migrate_config(migrated)

    assert migrated_again == migrated
    assert report_again == {"changed": False, "changes": []}
