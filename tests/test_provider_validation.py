# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import inspect

import pytest

from solstone.think import cogitate_client
from solstone.think.providers import (
    PROVIDER_REGISTRY,
    get_provider_module,
    local,
    validate_key,
    validate_model,
)


def test_cloud_validate_key_uses_child_only_key_override(monkeypatch) -> None:
    captured: dict[str, object] = {}

    def probe(*_args, **kwargs):
        captured.update(kwargs)
        return {"text": "OK"}

    monkeypatch.setattr("solstone.think.generate_client.generate_with_result", probe)

    assert cogitate_client.validate_key("google", "candidate-secret") == {
        "valid": True
    }
    assert captured["child_environment"] == {
        "SOLSTONE_GENERATE_API_KEY_OVERRIDE": "candidate-secret"
    }


def test_cloud_validate_model_sets_child_only_lane_overrides(monkeypatch) -> None:
    captured: dict[str, object] = {}

    def probe(*_args, **kwargs):
        captured.update(kwargs)
        return {"text": "OK"}

    monkeypatch.setattr("solstone.think.generate_client.generate_with_result", probe)

    assert cogitate_client.validate_model(
        "openai", "candidate-model", "candidate-secret"
    ) == {"valid": True}
    assert captured["child_environment"] == {
        "SOLSTONE_GENERATE_API_KEY_OVERRIDE": "candidate-secret",
        "SOLSTONE_GENERATE_MODEL_OVERRIDE": "candidate-model",
        "SOLSTONE_GENERATE_PROVIDER_OVERRIDE": "openai",
    }


@pytest.mark.parametrize("reason", ["model_not_found", "provider_quota_exceeded"])
def test_cloud_validate_key_preserves_probe_success_carve_out(
    monkeypatch, reason: str
) -> None:
    error = RuntimeError("probe refused")
    error.reason_code = reason  # type: ignore[attr-defined]
    monkeypatch.setattr(
        "solstone.think.generate_client.generate_with_result",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(error),
    )

    assert cogitate_client.validate_key("google", "candidate-secret") == {
        "valid": True,
        "probe_reason_code": reason,
    }


def test_cloud_validate_key_preserves_wire_failure_shape(monkeypatch) -> None:
    error = RuntimeError("probe refused")
    error.reason_code = "provider_key_invalid"  # type: ignore[attr-defined]
    monkeypatch.setattr(
        "solstone.think.generate_client.generate_with_result",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(error),
    )

    assert cogitate_client.validate_key("google", "bad") == {
        "valid": False,
        "error": "probe refused",
        "reason_code": "provider_key_invalid",
    }


def test_cloud_validate_model_preserves_wire_failure_shape(monkeypatch) -> None:
    error = RuntimeError("probe refused")
    error.reason_code = "provider_key_invalid"  # type: ignore[attr-defined]
    monkeypatch.setattr(
        "solstone.think.generate_client.generate_with_result",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(error),
    )

    assert cogitate_client.validate_model("anthropic", "candidate-model", "bad") == {
        "valid": False,
        "error": "probe refused",
        "reason_code": "provider_key_invalid",
    }


def test_local_validate_key_uses_native_probe_and_wire_reason(monkeypatch) -> None:
    captured: dict[str, object] = {}
    error = RuntimeError("native refusal")
    error.reason_code = "network_unreachable"  # type: ignore[attr-defined]

    def probe(*args, **kwargs):
        captured["args"] = args
        captured.update(kwargs)
        raise error

    monkeypatch.setattr("solstone.think.generate_client.generate_with_result", probe)

    assert local.validate_key() == {
        "valid": False,
        "error": "native refusal",
        "reason_code": "network_unreachable",
    }
    assert captured["args"] == ("Say OK", "settings.local.validate_key")
    assert captured["temperature"] == 0
    assert captured["max_output_tokens"] == 8
    assert captured["timeout_s"] == 10
    assert "child_environment" not in captured


def test_registry_validation_resolves_cloud_modules(monkeypatch) -> None:
    calls: list[dict[str, object]] = []

    def probe(*_args, **kwargs):
        calls.append(kwargs)
        return {"text": "OK"}

    monkeypatch.setattr("solstone.think.generate_client.generate_with_result", probe)

    for provider in ("google", "openai", "anthropic"):
        assert get_provider_module(provider) is cogitate_client
        assert validate_key(provider, "candidate-secret") == {"valid": True}
        assert validate_model(provider, "candidate-model", "candidate-secret") == {
            "valid": True
        }

    assert PROVIDER_REGISTRY["local"] == "solstone.think.providers.local"
    assert get_provider_module("local") is local
    assert len(calls) == 6


def test_cloud_registry_module_accepts_brain_cogitate_call_shape() -> None:
    for provider in ("google", "openai", "anthropic"):
        module = get_provider_module(provider)
        assert module is cogitate_client
        inspect.signature(module.run_cogitate).bind(config={}, on_event=None)
