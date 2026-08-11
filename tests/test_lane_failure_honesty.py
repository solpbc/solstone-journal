# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import time
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import httpx
import pytest

from solstone.think import models
from solstone.think import talents as talents_module
from solstone.think.models import (
    CLAUDE_SONNET_4,
    LOCAL_MODEL,
    IncompleteJSONError,
    ProviderResponseInvalidError,
    resolve_provider,
)
from solstone.think.pipeline_health import (
    lookup_segment_progress,
    read_segment_progress,
    segment_fully_thought,
)
from solstone.think.providers.shared import QuotaExhaustedError
from solstone.think.providers.local import LocalCapacityExhausted, LocalProviderError
from solstone.think.talents import _execute_generate, _execute_with_tools
from tests.helpers.journal_config import seed_journal_config

CLOUD_PROVIDERS = ("google", "openai", "anthropic")
DAY = "20240115"
STREAM = "default"
SEGMENT = "090000_300"


class ReasonedProviderError(RuntimeError):
    def __init__(self, reason_code: str, message: str) -> None:
        super().__init__(message)
        self.reason_code = reason_code


class BadRequestError(RuntimeError):
    status_code = 400


@dataclass
class CallObservation:
    target: str
    method: str
    provider: str
    model: str
    kwargs: dict[str, Any]


@dataclass
class LaneProbe:
    observations: list[CallObservation]

    def by_method(self, method: str) -> list[CallObservation]:
        return [obs for obs in self.observations if obs.method == method]

    def assert_only_provider(self, provider: str) -> None:
        assert self.observations, "expected at least one provider call"
        assert {obs.provider for obs in self.observations} == {provider}

    def assert_no_provider(self, provider: str) -> None:
        assert provider not in {obs.provider for obs in self.observations}


def _clear_provider_env(monkeypatch: pytest.MonkeyPatch) -> None:
    for key in ("GOOGLE_API_KEY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY"):
        monkeypatch.delenv(key, raising=False)


def _write_test_config(
    journal: Path,
    *,
    provider: str,
    model: str,
    interface: str = "generate",
    env: dict[str, str] | None = None,
    local_endpoint: dict[str, Any] | None = None,
) -> dict[str, Any]:
    del interface
    config: dict[str, Any] = {
        "env": env or {},
        "providers": {
            "active": {"provider": provider, "model": model},
        },
    }
    if local_endpoint is not None:
        config["providers"]["local"] = local_endpoint
    seed_journal_config(config, journal)
    return config


@contextmanager
def _assert_config_unchanged(journal: Path) -> Iterator[None]:
    path = journal / "config" / "journal.json"
    before = path.read_bytes() if path.exists() else None
    yield
    after = path.read_bytes() if path.exists() else None
    assert after == before


def _generate_config(
    *,
    provider: str = "anthropic",
    model: str = CLAUDE_SONNET_4,
) -> dict:
    return {
        "name": "test_generator",
        "type": "generate",
        "provider": provider,
        "model": model,
        "prompt": "say ok",
        "output": "md",
        "output_path": None,
        "thinking_budget": 0,
        "max_output_tokens": 32,
    }


def _cogitate_config(
    *,
    provider: str = "anthropic",
    model: str = CLAUDE_SONNET_4,
) -> dict:
    return {
        "name": "test_agent",
        "type": "cogitate",
        "provider": provider,
        "model": model,
        "timeout_seconds": 1,
    }


def _result(text: str = "ok") -> dict[str, Any]:
    return {"text": text, "usage": {}}


def _install_generate_probe(
    monkeypatch: pytest.MonkeyPatch,
    *,
    active_provider: str,
    active: Callable[[int, dict[str, Any]], dict[str, Any]] | None = None,
) -> LaneProbe:
    observations: list[CallObservation] = []
    counts: dict[str, int] = {}

    def fake_cloud_generate(*_args: Any, **kwargs: Any) -> dict[str, Any]:
        provider, model = resolve_provider("generate")
        observations.append(
            CallObservation(
                target="solstone.think.generate_client",
                method="generate",
                provider=provider,
                model=model,
                kwargs=dict(kwargs),
            )
        )
        if provider != active_provider:
            raise AssertionError(f"inactive provider dispatched: {provider}")
        counts[provider] = counts.get(provider, 0) + 1
        if active is not None:
            result = active(counts[provider], kwargs)
            if isinstance(result, BaseException):
                raise result
            return result
        return _result()

    async def fake_cloud_agenerate(*_args: Any, **kwargs: Any) -> dict[str, Any]:
        provider, model = resolve_provider("generate")
        observations.append(
            CallObservation(
                target="solstone.think.generate_client",
                method="agenerate",
                provider=provider,
                model=model,
                kwargs=dict(kwargs),
            )
        )
        raise AssertionError(f"async generate path dispatched: {provider}")

    monkeypatch.setattr(
        "solstone.think.generate_client.generate_with_result", fake_cloud_generate
    )
    monkeypatch.setattr(
        "solstone.think.generate_client.agenerate_with_result", fake_cloud_agenerate
    )
    return LaneProbe(observations)


def _install_cloud_generate_tripwires(monkeypatch: pytest.MonkeyPatch) -> LaneProbe:
    observations: list[CallObservation] = []

    def fake_run_generate(*_args: Any, **kwargs: Any) -> dict[str, Any]:
        provider, model = resolve_provider("generate")
        observations.append(
            CallObservation(
                target="solstone.think.generate_client",
                method="generate",
                provider=provider,
                model=model,
                kwargs=dict(kwargs),
            )
        )
        raise AssertionError(f"cloud provider dispatched: {provider}")

    async def fake_run_agenerate(*_args: Any, **kwargs: Any) -> dict[str, Any]:
        provider, model = resolve_provider("generate")
        observations.append(
            CallObservation(
                target="solstone.think.generate_client",
                method="agenerate",
                provider=provider,
                model=model,
                kwargs=dict(kwargs),
            )
        )
        raise AssertionError(f"async cloud provider dispatched: {provider}")

    monkeypatch.setattr(
        "solstone.think.generate_client.generate_with_result", fake_run_generate
    )
    monkeypatch.setattr(
        "solstone.think.generate_client.agenerate_with_result", fake_run_agenerate
    )
    return LaneProbe(observations)


def _install_cogitate_probe(
    monkeypatch: pytest.MonkeyPatch,
    *,
    active_provider: str,
    active: Callable[[int, dict[str, Any]], Any] | None = None,
) -> LaneProbe:
    observations: list[CallObservation] = []
    counts: dict[str, int] = {}

    async def fake_native_run_cogitate(
        config: dict[str, Any],
        on_event: Callable[[dict], None] | None = None,
        *,
        slot_lease: Any | None = None,
        context_window: int | None = None,
    ) -> str | None:
        del on_event, slot_lease, context_window
        provider = str(config.get("provider") or "")
        model = str(config.get("model") or "")
        observations.append(
            CallObservation(
                target="solstone.think.cogitate_client",
                method="cogitate",
                provider=provider,
                model=model,
                kwargs={"config": dict(config)},
            )
        )
        if provider != active_provider:
            raise AssertionError(f"inactive cogitate provider dispatched: {provider}")
        counts[provider] = counts.get(provider, 0) + 1
        if active is not None:
            result = active(counts[provider], config)
            if isinstance(result, BaseException):
                raise result
            return result
        return None

    monkeypatch.setattr(
        "solstone.think.cogitate_client.run_cogitate",
        fake_native_run_cogitate,
    )
    return LaneProbe(observations)


@pytest.mark.parametrize("interface", ["generate", "cogitate"])
@pytest.mark.asyncio
async def test_byo_endpoint_unreachable_stays_local(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    interface: str,
):
    _clear_provider_env(monkeypatch)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_test_config(
        tmp_path,
        provider="local",
        model=LOCAL_MODEL,
        interface=interface,
        env={"GOOGLE_API_KEY": "test-google-key"},
        local_endpoint={
            "endpoint_url": "http://127.0.0.1:9",
            "served_model_id": "served-model",
            "parallel_slots": 1,
        },
    )

    if interface == "generate":
        probe = _install_generate_probe(
            monkeypatch,
            active_provider="local",
            active=lambda _count, _kwargs: LocalProviderError(
                "local_endpoint_unreachable", "dead endpoint"
            ),
        )
        with _assert_config_unchanged(tmp_path):
            with pytest.raises(LocalProviderError) as exc_info:
                await _execute_generate(
                    _generate_config(provider="local", model=LOCAL_MODEL),
                    lambda _event: None,
        )
        assert exc_info.value.reason_code == "local_endpoint_unreachable"
        probe.assert_only_provider("local")
        return

    connect_error = httpx.ConnectError("dead endpoint")
    probe = _install_cogitate_probe(
        monkeypatch,
        active_provider="local",
        active=lambda _count, _config: connect_error,
    )
    events: list[dict[str, Any]] = []

    with _assert_config_unchanged(tmp_path):
        with pytest.raises(LocalProviderError) as exc_info:
            await _execute_with_tools(
                _cogitate_config(provider="local", model=LOCAL_MODEL),
                events.append,
            )

    assert exc_info.value.reason_code == "local_endpoint_unreachable"
    assert [event["reason_code"] for event in events] == ["local_endpoint_unreachable"]
    assert len(probe.observations) == 1
    assert probe.observations[0].target == "solstone.think.cogitate_client"
    assert probe.observations[0].provider == "local"
    assert probe.observations[0].model == LOCAL_MODEL


@pytest.mark.parametrize("interface", ["generate", "cogitate"])
@pytest.mark.asyncio
async def test_byo_endpoint_contract_failure_stays_local(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    interface: str,
):
    _clear_provider_env(monkeypatch)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_test_config(
        tmp_path,
        provider="local",
        model=LOCAL_MODEL,
        interface=interface,
        env={"GOOGLE_API_KEY": "test-google-key"},
        local_endpoint={
            "endpoint_url": "http://127.0.0.1:8080",
            "served_model_id": "served-model",
            "parallel_slots": 1,
        },
    )

    if interface == "generate":
        probe = _install_generate_probe(
            monkeypatch,
            active_provider="local",
            active=lambda _count, _kwargs: LocalProviderError(
                "local_endpoint_contract_failed", "bad request"
            ),
        )
        with _assert_config_unchanged(tmp_path):
            with pytest.raises(LocalProviderError) as exc_info:
                await _execute_generate(
                    _generate_config(provider="local", model=LOCAL_MODEL),
                    lambda _event: None,
        )
        assert exc_info.value.reason_code == "local_endpoint_contract_failed"
        probe.assert_only_provider("local")
        return

    probe = _install_cogitate_probe(
        monkeypatch,
        active_provider="local",
        active=lambda _count, _config: BadRequestError("bad request"),
    )
    events: list[dict[str, Any]] = []

    with _assert_config_unchanged(tmp_path):
        with pytest.raises(LocalProviderError) as exc_info:
            await _execute_with_tools(
                _cogitate_config(provider="local", model=LOCAL_MODEL),
                events.append,
            )

    assert exc_info.value.reason_code == "local_endpoint_contract_failed"
    assert [event["reason_code"] for event in events] == [
        "local_endpoint_contract_failed"
    ]
    assert len(probe.observations) == 1
    assert probe.observations[0].target == "solstone.think.cogitate_client"
    assert probe.observations[0].provider == "local"
    assert probe.observations[0].model == LOCAL_MODEL


@pytest.mark.parametrize("interface", ["generate", "cogitate"])
@pytest.mark.asyncio
async def test_vendor_quota_records_and_does_not_switch(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    interface: str,
):
    _clear_provider_env(monkeypatch)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_test_config(
        tmp_path,
        provider="anthropic",
        model=CLAUDE_SONNET_4,
        interface=interface,
        env={
            "ANTHROPIC_API_KEY": "test-anthropic-key",
            "GOOGLE_API_KEY": "test-google-key",
        },
    )
    quota = QuotaExhaustedError("quota exhausted", retry_delay_ms=5000)
    recorded: list[tuple[str, str]] = []
    monkeypatch.setattr(
        talents_module,
        "_record_brain_runtime_failure",
        lambda reason, component, **_kwargs: recorded.append((reason, component)),
    )

    if interface == "generate":
        probe = _install_generate_probe(
            monkeypatch,
            active_provider="anthropic",
            active=lambda _count, _kwargs: (_ for _ in ()).throw(quota),
        )
        with _assert_config_unchanged(tmp_path):
            with pytest.raises(QuotaExhaustedError, match="quota exhausted"):
                await _execute_generate(_generate_config(), lambda _event: None)
    else:
        probe = _install_cogitate_probe(
            monkeypatch,
            active_provider="anthropic",
            active=lambda _count, _config: quota,
        )
        events: list[dict[str, Any]] = []
        with _assert_config_unchanged(tmp_path):
            with pytest.raises(QuotaExhaustedError, match="quota exhausted"):
                await _execute_with_tools(_cogitate_config(), events.append)
        assert [event["event"] for event in events] == ["error"]

    probe.assert_only_provider("anthropic")
    assert (
        len(probe.by_method("generate" if interface == "generate" else "cogitate")) == 1
    )
    probe.assert_no_provider("google")
    probe.assert_no_provider("openai")
    probe.assert_no_provider("local")
    assert recorded == [("provider_quota_exceeded", interface)]


@pytest.mark.parametrize("interface", ["generate", "cogitate"])
@pytest.mark.asyncio
async def test_non_quota_vendor_failure_does_not_switch(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    interface: str,
):
    _clear_provider_env(monkeypatch)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_test_config(
        tmp_path,
        provider="anthropic",
        model=CLAUDE_SONNET_4,
        interface=interface,
        env={
            "ANTHROPIC_API_KEY": "test-anthropic-key",
            "GOOGLE_API_KEY": "test-google-key",
        },
    )
    failure = ReasonedProviderError("network_unreachable", "network down")

    if interface == "generate":
        probe = _install_generate_probe(
            monkeypatch,
            active_provider="anthropic",
            active=lambda _count, _kwargs: (_ for _ in ()).throw(failure),
        )
        with _assert_config_unchanged(tmp_path):
            with pytest.raises(ReasonedProviderError) as exc_info:
                await _execute_generate(_generate_config(), lambda _event: None)
    else:
        probe = _install_cogitate_probe(
            monkeypatch,
            active_provider="anthropic",
            active=lambda _count, _config: failure,
        )
        with _assert_config_unchanged(tmp_path):
            with pytest.raises(ReasonedProviderError) as exc_info:
                await _execute_with_tools(_cogitate_config(), lambda _event: None)

    assert exc_info.value.reason_code == "network_unreachable"
    probe.assert_only_provider("anthropic")
    assert (
        len(probe.by_method("generate" if interface == "generate" else "cogitate")) == 1
    )
    probe.assert_no_provider("google")
    probe.assert_no_provider("openai")
    probe.assert_no_provider("local")


def test_brain_runtime_failure_helper_records_only_allowed_ingress(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    recorded: list[dict[str, Any]] = []
    monkeypatch.setattr(
        talents_module,
        "_read_runtime_fingerprint",
        lambda: "fingerprint",
    )
    monkeypatch.setattr(
        "solstone.think.providers.brain_state.record_brain_runtime_failure",
        lambda reason_code, *_args, **kwargs: (
            recorded.append({"reason_code": reason_code, **kwargs})
            or {"accepted": True}
        ),
    )

    talents_module._record_brain_runtime_failure("network_unreachable", "cogitate")
    talents_module._record_brain_runtime_failure(
        "provider_request_rejected", "generate"
    )
    talents_module._record_brain_runtime_failure("context_window_exceeded", "generate")
    talents_module._record_brain_runtime_failure("cogitate_terminal_error", "generate")

    assert recorded == [
        {
            "reason_code": "network_unreachable",
            "expected_fingerprint_sha256": "fingerprint",
            "component": "cogitate",
            "diagnostic": {},
        },
        {
            "reason_code": "provider_request_rejected",
            "expected_fingerprint_sha256": "fingerprint",
            "component": "generate",
            "diagnostic": {},
        },
    ]


def test_bad_request_local_and_no_brain_do_not_record_brain_runtime_failure(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.think.providers.shared import classify_provider_error

    recorded: list[dict[str, Any]] = []
    monkeypatch.setattr(
        talents_module,
        "_read_runtime_fingerprint",
        lambda: "fingerprint",
    )
    monkeypatch.setattr(
        "solstone.think.providers.brain_state.record_brain_runtime_failure",
        lambda reason_code, *_args, **kwargs: (
            recorded.append({"reason_code": reason_code, **kwargs})
            or {"accepted": True}
        ),
    )

    for provider in ("local", "none"):
        exc = BadRequestError("Invalid value for parameter 'temperature'")
        reason_code = classify_provider_error(exc, provider)
        talents_module._record_brain_runtime_failure(reason_code, "generate")

    assert recorded == []


@pytest.mark.parametrize(
    ("reason", "finish_reason", "expected"),
    [
        ("blank_visible_output", "stop", [("provider_response_invalid", "generate")]),
        ("blank_visible_output", None, [("provider_response_invalid", "generate")]),
        ("content_filter", "content_filter", []),
        ("recitation", "recitation", []),
    ],
)
@pytest.mark.asyncio
async def test_provider_response_invalid_records_only_clean_stop_blank_response(
    monkeypatch: pytest.MonkeyPatch,
    reason: str,
    finish_reason: str | None,
    expected: list[tuple[str, str]],
) -> None:
    recorded: list[tuple[str, str]] = []
    monkeypatch.setattr(
        talents_module,
        "_record_brain_runtime_failure",
        lambda reason_code, component, **_kwargs: recorded.append(
            (reason_code, component)
        ),
    )

    def fake_generate_with_result(*_args: Any, **_kwargs: Any) -> dict[str, Any]:
        raise ProviderResponseInvalidError(
            reason,
            finish_reason=finish_reason,
            model="model",
        )

    monkeypatch.setattr(
        "solstone.think.models.generate_with_result",
        fake_generate_with_result,
    )
    events: list[dict[str, Any]] = []

    await _execute_generate(_generate_config(), events.append)

    assert recorded == expected
    assert events[-1]["reason_code"] == "provider_response_invalid"


@pytest.mark.parametrize("interface", ["generate", "cogitate"])
@pytest.mark.asyncio
async def test_active_brain_does_not_pre_swap_for_previous_failures(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    interface: str,
):
    _clear_provider_env(monkeypatch)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_test_config(
        tmp_path,
        provider="anthropic",
        model=CLAUDE_SONNET_4,
        interface=interface,
        env={
            "ANTHROPIC_API_KEY": "test-anthropic-key",
            "GOOGLE_API_KEY": "test-google-key",
        },
    )
    if interface == "generate":
        probe = _install_generate_probe(monkeypatch, active_provider="anthropic")
        with _assert_config_unchanged(tmp_path):
            result = models.generate_with_result("hello", "test.context")
        assert result["text"] == "ok"
    else:
        provider, model = resolve_provider("cogitate")
        assert (provider, model) == ("anthropic", CLAUDE_SONNET_4)
        probe = _install_cogitate_probe(monkeypatch, active_provider="anthropic")
        with _assert_config_unchanged(tmp_path):
            await _execute_with_tools(
                _cogitate_config(provider=provider, model=model),
                lambda _event: None,
            )

    probe.assert_only_provider("anthropic")
    assert (
        len(probe.by_method("generate" if interface == "generate" else "cogitate")) == 1
    )
    probe.assert_no_provider("google")
    probe.assert_no_provider("openai")
    probe.assert_no_provider("local")


@pytest.mark.parametrize("interface", ["cogitate"])
@pytest.mark.asyncio
async def test_explicit_local_not_ready_does_not_consult_cloud(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    interface: str,
):
    from solstone.think.providers import local_server

    _clear_provider_env(monkeypatch)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_test_config(
        tmp_path,
        provider="local",
        model=LOCAL_MODEL,
        interface=interface,
        env={
            "GOOGLE_API_KEY": "test-google-key",
            "ANTHROPIC_API_KEY": "test-anthropic-key",
        },
    )
    connect_calls = 0
    def spy_connect() -> Any:
        nonlocal connect_calls
        connect_calls += 1
        raise LocalProviderError("local_model_not_ready", "local model not ready")

    monkeypatch.setattr(local_server, "connect", spy_connect)

    if interface == "generate":
        probe = _install_cloud_generate_tripwires(monkeypatch)

        with _assert_config_unchanged(tmp_path):
            with pytest.raises(LocalProviderError) as exc_info:
                await _execute_generate(
                    _generate_config(provider="local", model=LOCAL_MODEL),
                    lambda _event: None,
                )

        assert exc_info.value.reason_code == "local_model_not_ready"
        assert connect_calls == 1
        assert probe.observations == []
        return

    probe = _install_cogitate_probe(
        monkeypatch,
        active_provider="local",
    )

    with _assert_config_unchanged(tmp_path):
        with pytest.raises(LocalProviderError) as exc_info:
            await _execute_with_tools(
                _cogitate_config(provider="local", model=LOCAL_MODEL),
                lambda _event: None,
            )

    assert exc_info.value.reason_code == "local_model_not_ready"
    assert connect_calls == 1
    assert all(obs.provider == "local" for obs in probe.observations)
    probe.assert_no_provider("google")
    probe.assert_no_provider("openai")
    probe.assert_no_provider("anthropic")


@pytest.mark.parametrize("interface", ["cogitate"])
@pytest.mark.asyncio
async def test_explicit_local_loading_does_not_consult_cloud(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    interface: str,
):
    from solstone.think.providers import local_server

    _clear_provider_env(monkeypatch)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_test_config(
        tmp_path,
        provider="local",
        model=LOCAL_MODEL,
        interface=interface,
        env={
            "GOOGLE_API_KEY": "test-google-key",
            "ANTHROPIC_API_KEY": "test-anthropic-key",
        },
    )
    connect_calls = 0
    def spy_connect() -> Any:
        nonlocal connect_calls
        connect_calls += 1
        raise LocalProviderError("local_model_loading", "local model loading")

    monkeypatch.setattr(local_server, "connect", spy_connect)

    if interface == "generate":
        probe = _install_cloud_generate_tripwires(monkeypatch)

        with _assert_config_unchanged(tmp_path):
            with pytest.raises(LocalProviderError) as exc_info:
                await _execute_generate(
                    _generate_config(provider="local", model=LOCAL_MODEL),
                    lambda _event: None,
                )

        assert exc_info.value.reason_code == "local_model_loading"
        assert connect_calls == 1
        assert probe.observations == []
        return

    probe = _install_cogitate_probe(
        monkeypatch,
        active_provider="local",
    )

    with _assert_config_unchanged(tmp_path):
        with pytest.raises(LocalProviderError) as exc_info:
            await _execute_with_tools(
                _cogitate_config(provider="local", model=LOCAL_MODEL),
                lambda _event: None,
            )

    assert exc_info.value.reason_code == "local_model_loading"
    assert connect_calls == 1
    assert all(obs.provider == "local" for obs in probe.observations)
    probe.assert_no_provider("google")
    probe.assert_no_provider("openai")
    probe.assert_no_provider("anthropic")


@pytest.mark.parametrize(
    ("failure", "expected_reason_code"),
    [
        (
            LocalProviderError("provider_unavailable", "local provider unavailable"),
            "provider_unavailable",
        ),
        (RuntimeError("not retryable"), None),
    ],
    ids=["reasoned", "bare"],
)
@pytest.mark.asyncio
async def test_bundled_local_hard_failure_stays_local(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    failure: BaseException,
    expected_reason_code: str | None,
):
    _clear_provider_env(monkeypatch)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_test_config(
        tmp_path,
        provider="local",
        model=LOCAL_MODEL,
        interface="generate",
        env={"GOOGLE_API_KEY": "test-google-key"},
    )
    probe = _install_generate_probe(
        monkeypatch,
        active_provider="local",
        active=lambda _count, _kwargs: (_ for _ in ()).throw(failure),
    )

    with _assert_config_unchanged(tmp_path):
        with pytest.raises(type(failure)) as exc_info:
            await _execute_generate(
                _generate_config(provider="local", model=LOCAL_MODEL),
                lambda _event: None,
            )

    if expected_reason_code is None:
        assert not hasattr(exc_info.value, "reason_code")
    else:
        assert exc_info.value.reason_code == expected_reason_code
    assert len(probe.by_method("generate")) == 1
    probe.assert_only_provider("local")
    probe.assert_no_provider("google")
    probe.assert_no_provider("openai")
    probe.assert_no_provider("anthropic")


@pytest.mark.parametrize(
    ("first_failure", "expected_retry_kwargs"),
    [
        (
            IncompleteJSONError("length", '{"partial":'),
            {"inference_retry_index": 1, "local_exclusive_admission": False},
        ),
        (
            LocalCapacityExhausted(),
            {"inference_retry_index": 1, "local_exclusive_admission": True},
        ),
    ],
)
@pytest.mark.asyncio
async def test_local_honest_retry_stays_same_provider(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    first_failure: BaseException,
    expected_retry_kwargs: dict[str, Any],
):
    _clear_provider_env(monkeypatch)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_test_config(
        tmp_path,
        provider="local",
        model=LOCAL_MODEL,
        interface="generate",
        env={"GOOGLE_API_KEY": "test-google-key"},
    )

    def active(count: int, _kwargs: dict[str, Any]) -> dict[str, Any]:
        if count == 1:
            raise first_failure
        return _result('{"ok": true}')

    probe = _install_generate_probe(
        monkeypatch,
        active_provider="local",
        active=active,
    )
    events: list[dict[str, Any]] = []

    with _assert_config_unchanged(tmp_path):
        await _execute_generate(
            _generate_config(provider="local", model=LOCAL_MODEL),
            events.append,
        )

    calls = probe.by_method("generate")
    assert len(calls) == 2
    assert {call.provider for call in calls} == {"local"}
    assert calls[1].kwargs["inference_retry_index"] == 1
    assert (
        calls[1].kwargs["local_exclusive_admission"]
        is expected_retry_kwargs["local_exclusive_admission"]
    )
    assert events[-1]["event"] == "finish"
    probe.assert_only_provider("local")
    probe.assert_no_provider("google")
    probe.assert_no_provider("openai")
    probe.assert_no_provider("anthropic")


@pytest.mark.asyncio
async def test_local_honest_retry_heartbeat_spans_attempts_and_retry_gap(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
):
    _clear_provider_env(monkeypatch)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(talents_module, "_GENERATE_PROGRESS_INTERVAL_S", 0.01)
    _write_test_config(
        tmp_path,
        provider="local",
        model=LOCAL_MODEL,
        interface="generate",
        env={"GOOGLE_API_KEY": "test-google-key"},
    )
    windows: dict[str, float] = {}

    def slow_warning(*_args: Any, **_kwargs: Any) -> None:
        time.sleep(0.035)

    monkeypatch.setattr(talents_module.LOG, "warning", slow_warning)

    def active(count: int, _kwargs: dict[str, Any]) -> dict[str, Any]:
        if count == 1:
            windows["first_start"] = time.monotonic()
            time.sleep(0.035)
            windows["first_end"] = time.monotonic()
            raise IncompleteJSONError("length", '{"partial":')
        windows["second_start"] = time.monotonic()
        time.sleep(0.035)
        windows["second_end"] = time.monotonic()
        return _result('{"ok": true}')

    probe = _install_generate_probe(
        monkeypatch,
        active_provider="local",
        active=active,
    )
    events: list[dict[str, Any]] = []
    progress_times: list[float] = []

    def emit_event(event: dict[str, Any]) -> None:
        if event["event"] == "progress":
            progress_times.append(time.monotonic())
        events.append(event)

    with _assert_config_unchanged(tmp_path):
        await _execute_generate(
            _generate_config(provider="local", model=LOCAL_MODEL),
            emit_event,
        )

    assert probe.by_method("generate")[1].kwargs["inference_retry_index"] == 1
    assert any(
        windows["first_start"] <= ts <= windows["first_end"] for ts in progress_times
    )
    assert any(
        windows["first_end"] <= ts <= windows["second_start"] for ts in progress_times
    )
    assert any(
        windows["second_start"] <= ts <= windows["second_end"] for ts in progress_times
    )
    assert events[-1]["event"] == "finish"
    assert events[-1]["generate_progress_count"] == len(progress_times)


@pytest.mark.asyncio
async def test_attempted_failed_segment_remains_repair_selectable(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
):
    """Lock the selector for a fixtured attempted-and-failed segment health record.

    The dispatch/fail JSONL rows are authored by the fixture rather than by driving
    the whole segment pipeline. The locked behavior is that, given an attempted
    segment failure with an honest reason, no completion stamp is inferred and the
    repair selector continues to enumerate the segment.
    """
    from solstone.think import thinking as think

    _clear_provider_env(monkeypatch)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_test_config(
        tmp_path,
        provider="anthropic",
        model=CLAUDE_SONNET_4,
        interface="generate",
        env={"ANTHROPIC_API_KEY": "test-anthropic-key"},
    )
    health_dir = tmp_path / "chronicle" / DAY / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    health_rows = [
        {
            "event": "sense.complete",
            "ts": 1,
            "mode": "segment",
            "day": DAY,
            "segment": SEGMENT,
            "stream": STREAM,
            "density": "active",
        },
        {
            "event": "talent.dispatch",
            "ts": 2,
            "mode": "segment",
            "day": DAY,
            "segment": SEGMENT,
            "stream": STREAM,
            "name": "documents",
        },
        {
            "event": "talent.fail",
            "ts": 3,
            "mode": "segment",
            "day": DAY,
            "segment": SEGMENT,
            "stream": STREAM,
            "name": "documents",
            "reason_code": "network_unreachable",
        },
    ]
    (health_dir / "001_segment.jsonl").write_text(
        "\n".join(json.dumps(row) for row in health_rows) + "\n",
        encoding="utf-8",
    )

    failure = ReasonedProviderError("network_unreachable", "network down")
    probe = _install_generate_probe(
        monkeypatch,
        active_provider="anthropic",
        active=lambda _count, _kwargs: (_ for _ in ()).throw(failure),
    )
    segment = {
        "key": SEGMENT,
        "stream": STREAM,
        "data_state": {"screen": "analyzed"},
    }

    with _assert_config_unchanged(tmp_path):
        with pytest.raises(ReasonedProviderError) as exc_info:
            await _execute_generate(_generate_config(), lambda _event: None)

    assert exc_info.value.reason_code == "network_unreachable"
    probe.assert_only_provider("anthropic")
    assert len(probe.by_method("generate")) == 1
    progress = read_segment_progress(DAY)
    segment_progress = lookup_segment_progress(progress, STREAM, SEGMENT)
    assert segment_progress is not None
    complete, reason = segment_fully_thought(segment_progress)
    assert complete is False
    assert reason == "floor:documents"
    selected, counts = think._select_segment_repair_targets(
        DAY,
        [segment],
        force_all=False,
    )
    assert selected == [segment]
    assert counts == {
        "total": 1,
        "selected": 1,
        "complete": 0,
        "raw_blocked": 0,
    }
