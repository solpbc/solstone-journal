# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import asyncio
import hashlib
import importlib
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path
from unittest.mock import Mock

import numpy as np
import pytest

from solstone.think import models, talents
from solstone.think.models import (
    CLAUDE_SONNET_4,
    GEMINI_FLASH,
    LOCAL_MODEL,
    NO_BRAIN_PROVIDER,
    AttestationFailedError,
    NoBrainConfiguredError,
    is_local_provider_needed,
    resolve_provider,
)
from solstone.think.providers import get_provider_module
from solstone.think.services.spp_attest.cadence import AttestationSession
from tests.helpers.journal_config import seed_journal_config


@pytest.fixture(autouse=True)
def _clear_confidential_transport_state():
    from solstone.think.services import spp, spp_transport

    spp.delete_attestation_state()
    spp_transport.teardown_confidential_transport()
    yield
    spp.delete_attestation_state()
    spp_transport.teardown_confidential_transport()


def _empty_journal(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    for key in ("GOOGLE_API_KEY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY"):
        monkeypatch.delenv(key, raising=False)
    return tmp_path


def _install_native_brain_binary(monkeypatch: pytest.MonkeyPatch) -> None:
    from solstone.think.providers import brain_state

    binary = Path(__file__).resolve().parents[1] / "core/target/debug/solstone-core"
    assert binary.is_file()
    monkeypatch.setattr(brain_state, "_native_binary", lambda **_kwargs: binary)


def _cloud_call_mocks(monkeypatch: pytest.MonkeyPatch) -> list[Mock]:
    mocks: list[Mock] = []
    targets = [
        ("solstone.think.providers.openhands", "run_generate"),
        ("solstone.think.providers.openhands", "run_agenerate"),
        ("solstone.think.providers.openhands", "run_cogitate"),
    ]
    for module_name, attr in targets:
        mock = Mock(side_effect=AssertionError("cloud call attempted"))
        monkeypatch.setattr(f"{module_name}.{attr}", mock)
        mocks.append(mock)
    return mocks


def _seed_journal_config(tmp_path: Path, config: dict) -> str:
    config_path = seed_journal_config(config, tmp_path)
    return config_path.read_text(encoding="utf-8")


def _confidential_config(*, provider_pins: bool = True) -> dict:
    config: dict = {
        "env": {
            "GOOGLE_API_KEY": "test-google-key",
            "ANTHROPIC_API_KEY": "test-anthropic-key",
            "OPENAI_API_KEY": "test-openai-key",
        },
        "services": {
            "confidential": {
                "enabled_at": "2026-05-24T00:00:00Z",
                "account_id": "acct-test",
                "endpoint_url": "https://spp.example.test",
                "served_model_id": "confidential-model",
                "credential_created_at": "2026-05-24T00:00:00Z",
                "credential_fingerprint_sha256": "fingerprint",
                "prior_active": {
                    "provider": "google",
                    "model": GEMINI_FLASH,
                },
                "prior_local_endpoint": None,
            }
        },
    }
    if provider_pins:
        config["providers"] = {
            "active": {"provider": "google", "model": GEMINI_FLASH},
        }
    return config


def test_cloud_key_configured_reads_env_and_journal_config(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.think.providers import state

    _empty_journal(tmp_path, monkeypatch)
    assert state.cloud_key_configured("") is False
    assert state.cloud_key_configured("GOOGLE_API_KEY") is False

    monkeypatch.setenv("GOOGLE_API_KEY", "ambient-key")
    assert state.cloud_key_configured("GOOGLE_API_KEY") is True

    monkeypatch.delenv("GOOGLE_API_KEY")
    _seed_journal_config(tmp_path, {"env": {"GOOGLE_API_KEY": "stored-key"}})
    assert state.cloud_key_configured("GOOGLE_API_KEY") is True


def _install_failing_confidential_transport(
    monkeypatch: pytest.MonkeyPatch,
    reason_code: str = "gateway_unreachable",
) -> Mock:
    from solstone.think.services import spp, spp_transport
    from solstone.think.services.spp_attest.ratls.channel import RatlsChannelError

    spp.delete_attestation_state()
    spp_transport.teardown_confidential_transport()
    monkeypatch.setattr(models, "_CONFIDENTIAL_ATTESTATION_VERIFIER", None)
    _stub_nvattest_installed(monkeypatch)
    establish = Mock(side_effect=RatlsChannelError(reason_code))
    monkeypatch.setattr(spp_transport, "establish_attested_channel", establish)
    return establish


def _stub_nvattest_installed(monkeypatch: pytest.MonkeyPatch) -> None:
    from solstone.think.providers.nvattest_install import NvattestEnsureResult
    from solstone.think.services import spp_transport

    monkeypatch.setattr(
        spp_transport,
        "ensure_nvattest_installed",
        Mock(
            return_value=NvattestEnsureResult(
                status="already_installed",
                nvattest_dir=Path("nvattest-fixture"),
            )
        ),
    )


class _FakeChannel:
    def __init__(self, verdict: object, epoch: int | None = None) -> None:
        from solstone.think.services import spp_transport

        self.verdict = verdict
        self.tls = object()
        self.last_used_monotonic = time.monotonic()
        self.epoch = spp_transport._EPOCH if epoch is None else epoch
        self.closed = False

    def close(self) -> None:
        self.closed = True


class _FakeListener:
    def __init__(self) -> None:
        self.closed = False

    def close(self) -> None:
        self.closed = True


class _AliveThread:
    def is_alive(self) -> bool:
        return True


def _patch_confidential_listener(monkeypatch: pytest.MonkeyPatch) -> None:
    from solstone.think.services import spp_transport

    def fake_start_listener() -> None:
        spp_transport._LISTENER = _FakeListener()
        spp_transport._LISTENER_THREAD = _AliveThread()
        spp_transport._FORWARDER_BASE_URL = "http://127.0.0.1:4567"

    monkeypatch.setattr(spp_transport, "_start_listener_locked", fake_start_listener)


def _stale_session(verdict: object) -> AttestationSession:
    old = datetime.now(timezone.utc) - timedelta(hours=2)
    return AttestationSession(
        verdict=verdict,
        started_at=old,
        tpm_heartbeat_at=old,
        gpu_reattest_at=old,
    )


def _add_local_endpoint(config: dict) -> None:
    config.setdefault("providers", {})["local"] = {
        "endpoint_url": "https://spp.example.test/v1",
        "served_model_id": "confidential-model",
        "credential": "confidential-credential",
    }


def _local_generate_result() -> dict:
    return {
        "text": "local-ok",
        "model": LOCAL_MODEL,
        "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2},
        "finish_reason": "stop",
    }


async def _local_agenerate_success(*args, **kwargs) -> dict:
    return _local_generate_result()


async def _local_cogitate_success(*, config: dict, on_event=None) -> str:
    if on_event is not None:
        on_event(
            {
                "event": "finish",
                "result": "local-cogitate-ok",
                "usage": {"input_tokens": 1, "output_tokens": 1},
            }
        )
    return "local-cogitate-ok"


def _install_local_success_mocks(monkeypatch: pytest.MonkeyPatch) -> dict[str, Mock]:
    local_generate = Mock(return_value=_local_generate_result())
    local_agenerate = Mock(side_effect=_local_agenerate_success)
    local_cogitate = Mock(side_effect=_local_cogitate_success)
    monkeypatch.setattr("solstone.think.providers.local.run_generate", local_generate)
    monkeypatch.setattr("solstone.think.providers.local.run_agenerate", local_agenerate)
    monkeypatch.setattr("solstone.think.providers.local.run_cogitate", local_cogitate)
    return {
        "generate": local_generate,
        "agenerate": local_agenerate,
        "cogitate": local_cogitate,
    }


def _stt_audio() -> np.ndarray:
    return np.zeros(16000, dtype=np.float32)


def _install_stt_backend_mocks(
    monkeypatch: pytest.MonkeyPatch,
    *,
    parakeet_result=AssertionError("parakeet dispatch attempted"),
    confidential_result=AssertionError("confidential dispatch attempted"),
) -> dict[str, Mock]:
    targets = {
        "parakeet": (
            "solstone.observe.transcribe.parakeet.transcribe",
            parakeet_result,
        ),
        "confidential": (
            "solstone.observe.transcribe.confidential.transcribe",
            confidential_result,
        ),
    }
    mocks: dict[str, Mock] = {}
    for name, (target, result) in targets.items():
        if result is None:
            continue
        mock = (
            Mock(side_effect=result)
            if isinstance(result, BaseException)
            else Mock(return_value=result)
        )
        monkeypatch.setattr(target, mock)
        mocks[name] = mock
    return mocks


def _assert_attestation_failed(
    exc: AttestationFailedError,
    reason_code: str = "gateway_unreachable",
) -> None:
    assert exc.reason_code == "attestation_failed"
    assert f"({reason_code})" in exc.detail


def test_unconfigured_journal_resolves_to_no_brain(tmp_path, monkeypatch):
    _empty_journal(tmp_path, monkeypatch)

    for agent_type in ("generate", "cogitate"):
        provider, model = resolve_provider(agent_type)

        assert provider == NO_BRAIN_PROVIDER
        assert provider != "google"
        assert model == ""

    assert not (tmp_path / "config" / "journal.json").exists()


def test_unconfigured_execution_stops_before_cloud(tmp_path, monkeypatch):
    _empty_journal(tmp_path, monkeypatch)
    mocks = _cloud_call_mocks(monkeypatch)

    with pytest.raises(NoBrainConfiguredError):
        models.generate("hello", "any.context")

    for mock in mocks:
        mock.assert_not_called()
    assert not (tmp_path / "config" / "journal.json").exists()


def test_no_brain_configured_error_is_not_retried(tmp_path, monkeypatch):
    _empty_journal(tmp_path, monkeypatch)
    mocks = _cloud_call_mocks(monkeypatch)

    with pytest.raises(NoBrainConfiguredError):
        talents.prepare_config({"name": "chat"})

    for mock in mocks:
        mock.assert_not_called()


def test_confidential_generate_stops_before_any_provider_dispatch(
    tmp_path,
    monkeypatch,
):
    _empty_journal(tmp_path, monkeypatch)
    _seed_journal_config(tmp_path, _confidential_config())
    establish = _install_failing_confidential_transport(monkeypatch)
    mocks = _cloud_call_mocks(monkeypatch)
    httpx_post = Mock(side_effect=AssertionError("local endpoint call attempted"))
    httpx_get = Mock(side_effect=AssertionError("endpoint probe attempted"))
    monkeypatch.setattr("httpx.post", httpx_post)
    monkeypatch.setattr("httpx.get", httpx_get)

    with pytest.raises(AttestationFailedError) as generate_exc:
        models.generate("hello", "any.context")
    _assert_attestation_failed(generate_exc.value)

    with pytest.raises(AttestationFailedError) as result_exc:
        models.generate_with_result("hello", "any.context")
    _assert_attestation_failed(result_exc.value)

    with pytest.raises(AttestationFailedError) as async_exc:
        asyncio.run(models.agenerate("hello", "any.context"))
    _assert_attestation_failed(async_exc.value)

    for mock in mocks:
        mock.assert_not_called()
    httpx_post.assert_not_called()
    httpx_get.assert_not_called()
    assert establish.call_count == 3


def test_persisted_spp_brain_evidence_never_authorizes_generate_egress(
    tmp_path,
    monkeypatch,
):
    from solstone.think.providers.brain_state import (
        begin_brain_refresh,
        finish_brain_refresh,
    )
    from solstone.think.services import spp

    _empty_journal(tmp_path, monkeypatch)
    _install_native_brain_binary(monkeypatch)
    config = _confidential_config()
    config["providers"] = {
        "active": {"provider": "local", "model": LOCAL_MODEL},
    }
    _add_local_endpoint(config)
    config["services"]["confidential"]["credential_fingerprint_sha256"] = (
        hashlib.sha256(b"confidential-credential").hexdigest()
    )
    _seed_journal_config(tmp_path, config)

    now = datetime.now(timezone.utc)
    permit = begin_brain_refresh(now, journal_path=tmp_path)
    assert permit is not None
    component = {
        "status": "ok",
        "observed_at": now.isoformat(),
        "expires_at": (now + timedelta(hours=1)).isoformat(),
    }
    finish_brain_refresh(
        permit,
        {
            "configuration": component,
            "lane_prerequisites": component,
            "generate": component,
            "cogitate": component,
        },
        now,
        journal_path=tmp_path,
    )
    spp.delete_attestation_state()

    establish = _install_failing_confidential_transport(monkeypatch)
    httpx_post = Mock(side_effect=AssertionError("local endpoint call attempted"))
    httpx_get = Mock(side_effect=AssertionError("endpoint probe attempted"))
    monkeypatch.setattr("httpx.post", httpx_post)
    monkeypatch.setattr("httpx.get", httpx_get)

    with pytest.raises(AttestationFailedError) as generate_exc:
        models.generate("hello", "any.context")

    _assert_attestation_failed(generate_exc.value)
    httpx_post.assert_not_called()
    httpx_get.assert_not_called()
    establish.assert_called_once()


def test_persisted_spp_brain_evidence_never_authorizes_cogitate_egress(
    tmp_path,
    monkeypatch,
):
    from solstone.think.providers.brain_state import (
        begin_brain_refresh,
        finish_brain_refresh,
    )
    from solstone.think.services import spp

    _empty_journal(tmp_path, monkeypatch)
    _install_native_brain_binary(monkeypatch)
    config = _confidential_config()
    config["providers"] = {
        "active": {"provider": "local", "model": LOCAL_MODEL},
    }
    _add_local_endpoint(config)
    config["services"]["confidential"]["credential_fingerprint_sha256"] = (
        hashlib.sha256(b"confidential-credential").hexdigest()
    )
    _seed_journal_config(tmp_path, config)

    now = datetime.now(timezone.utc)
    permit = begin_brain_refresh(now, journal_path=tmp_path)
    assert permit is not None
    component = {
        "status": "ok",
        "observed_at": now.isoformat(),
        "expires_at": (now + timedelta(hours=1)).isoformat(),
    }
    finish_brain_refresh(
        permit,
        {
            "configuration": component,
            "lane_prerequisites": component,
            "generate": component,
            "cogitate": component,
        },
        now,
        journal_path=tmp_path,
    )
    spp.delete_attestation_state()

    establish = _install_failing_confidential_transport(monkeypatch)
    local_cogitate = Mock(side_effect=AssertionError("local cogitate dispatched"))
    build_llm = Mock(side_effect=AssertionError("llm build attempted"))
    monkeypatch.setattr(
        "solstone.think.providers.local.run_cogitate",
        local_cogitate,
    )
    monkeypatch.setattr("solstone.think.providers.openhands._build_llm", build_llm)

    with pytest.raises(AttestationFailedError) as cogitate_exc:
        asyncio.run(
            talents._execute_with_tools(
                {"provider": "local", "model": LOCAL_MODEL, "type": "cogitate"},
                lambda _event: None,
            )
        )

    _assert_attestation_failed(cogitate_exc.value)
    local_cogitate.assert_not_called()
    build_llm.assert_not_called()
    establish.assert_called_once()


def test_confidential_cogitate_stops_before_any_provider_dispatch(
    tmp_path,
    monkeypatch,
):
    _empty_journal(tmp_path, monkeypatch)
    _seed_journal_config(tmp_path, _confidential_config())
    establish = _install_failing_confidential_transport(monkeypatch)
    mocks = _cloud_call_mocks(monkeypatch)
    build_llm = Mock(side_effect=AssertionError("llm build attempted"))
    monkeypatch.setattr("solstone.think.providers.openhands._build_llm", build_llm)
    httpx_post = Mock(side_effect=AssertionError("local endpoint call attempted"))
    httpx_get = Mock(side_effect=AssertionError("endpoint probe attempted"))
    monkeypatch.setattr("httpx.post", httpx_post)
    monkeypatch.setattr("httpx.get", httpx_get)

    with pytest.raises(AttestationFailedError) as exc_info:
        asyncio.run(
            talents._execute_with_tools(
                {"provider": "google"},
                lambda _event: None,
            )
        )

    _assert_attestation_failed(exc_info.value)
    build_llm.assert_not_called()
    for mock in mocks:
        mock.assert_not_called()
    httpx_post.assert_not_called()
    httpx_get.assert_not_called()
    establish.assert_called_once()


def test_confidential_local_lane_stops_before_any_provider_dispatch(
    tmp_path,
    monkeypatch,
):
    _empty_journal(tmp_path, monkeypatch)
    config = _confidential_config()
    config["providers"] = {
        "active": {"provider": "local", "model": LOCAL_MODEL},
    }
    _add_local_endpoint(config)
    _seed_journal_config(tmp_path, config)
    establish = _install_failing_confidential_transport(monkeypatch)
    mocks = _cloud_call_mocks(monkeypatch)
    local = _install_local_success_mocks(monkeypatch)
    httpx_post = Mock(side_effect=AssertionError("local endpoint call attempted"))
    httpx_get = Mock(side_effect=AssertionError("endpoint probe attempted"))
    monkeypatch.setattr("httpx.post", httpx_post)
    monkeypatch.setattr("httpx.get", httpx_get)

    with pytest.raises(AttestationFailedError) as generate_exc:
        models.generate("hello", "any.context")
    _assert_attestation_failed(generate_exc.value)

    with pytest.raises(AttestationFailedError) as result_exc:
        models.generate_with_result("hello", "any.context")
    _assert_attestation_failed(result_exc.value)

    with pytest.raises(AttestationFailedError) as async_result_exc:
        asyncio.run(models.agenerate_with_result("hello", "any.context"))
    _assert_attestation_failed(async_result_exc.value)

    with pytest.raises(AttestationFailedError) as async_exc:
        asyncio.run(models.agenerate("hello", "any.context"))
    _assert_attestation_failed(async_exc.value)

    with pytest.raises(AttestationFailedError) as cogitate_exc:
        asyncio.run(
            talents._execute_with_tools(
                {"provider": "local", "model": LOCAL_MODEL, "type": "cogitate"},
                lambda _event: None,
            )
        )
    _assert_attestation_failed(cogitate_exc.value)

    for mock in mocks:
        mock.assert_not_called()
    local["generate"].assert_not_called()
    local["agenerate"].assert_not_called()
    local["cogitate"].assert_not_called()
    httpx_post.assert_not_called()
    httpx_get.assert_not_called()
    assert establish.call_count == 5

    config["providers"].pop("local", None)
    _seed_journal_config(tmp_path, config)
    for local_mock in local.values():
        local_mock.reset_mock()
    events: list[dict] = []

    assert models.generate("hello", "any.context") == "local-ok"
    assert models.generate_with_result("hello", "any.context")["text"] == "local-ok"
    assert (
        asyncio.run(models.agenerate_with_result("hello", "any.context"))["text"]
        == "local-ok"
    )
    assert asyncio.run(models.agenerate("hello", "any.context")) == "local-ok"
    asyncio.run(
        talents._execute_with_tools(
            {"provider": "local", "model": LOCAL_MODEL, "type": "cogitate"},
            events.append,
        )
    )

    assert local["generate"].call_count == 2
    assert local["agenerate"].call_count == 2
    local["cogitate"].assert_called_once()
    assert events[-1]["event"] == "finish"
    assert establish.call_count == 5


def test_confidential_readiness_probe_fails_closed_before_endpoint_get(
    tmp_path,
    monkeypatch,
):
    from solstone.think.providers import state
    from solstone.think.providers.local_endpoint import (
        probe_local_endpoint,
        resolve_local_endpoint,
    )
    from solstone.think.services.spp_transport import confidential_probe_status

    _empty_journal(tmp_path, monkeypatch)
    config = _confidential_config()
    config.setdefault("providers", {})["local"] = {
        "endpoint_url": "https://spp.example.test/v1",
        "served_model_id": "confidential-model",
        "credential": "confidential-credential",
    }
    _seed_journal_config(tmp_path, config)
    httpx_get = Mock(side_effect=AssertionError("endpoint probe attempted"))
    monkeypatch.setattr("httpx.get", httpx_get)

    endpoint = resolve_local_endpoint()
    assert confidential_probe_status() == (False, "attestation_not_yet_verified")
    assert probe_local_endpoint(endpoint) == (False, "attestation_not_yet_verified")
    status = state.local_status_dict()

    assert status["configured"] is True
    assert status["generate_ready"] is False
    assert status["cogitate_ready"] is False
    assert status["issues"] == ["local_endpoint_unreachable"]
    httpx_get.assert_not_called()

    config.pop("services", None)
    _seed_journal_config(tmp_path, config)
    assert confidential_probe_status() is None


def test_confidential_attestation_error_is_non_retryable(tmp_path, monkeypatch):
    _empty_journal(tmp_path, monkeypatch)
    _seed_journal_config(tmp_path, _confidential_config())
    establish = _install_failing_confidential_transport(monkeypatch)
    mocks = _cloud_call_mocks(monkeypatch)

    from solstone.think.services.spp_transport import verify_confidential_attestation

    assert (
        models._confidential_attestation_verifier() is verify_confidential_attestation
    )

    with pytest.raises(AttestationFailedError) as exc_info:
        asyncio.run(
            talents._execute_with_tools(
                {"provider": "google", "type": "cogitate"},
                lambda _event: None,
            )
        )
    _assert_attestation_failed(exc_info.value)

    for mock in mocks:
        mock.assert_not_called()
    establish.assert_called_once()


def test_confidential_gate_keys_on_derived_lane_not_provider_name(
    tmp_path,
    monkeypatch,
):
    _empty_journal(tmp_path, monkeypatch)
    config = _confidential_config(provider_pins=False)
    config["env"] = {"GOOGLE_API_KEY": "stray-google-key"}
    config["providers"] = {
        "active": {"provider": "local", "model": LOCAL_MODEL},
    }
    _seed_journal_config(tmp_path, config)
    establish = _install_failing_confidential_transport(monkeypatch)
    mocks = _cloud_call_mocks(monkeypatch)
    local = _install_local_success_mocks(monkeypatch)

    assert models.generate("hello", "any.context") == "local-ok"
    local["generate"].assert_called_once()
    establish.assert_not_called()

    for mock in mocks:
        mock.assert_not_called()

    _add_local_endpoint(config)
    _seed_journal_config(tmp_path, config)
    local["generate"].reset_mock()

    with pytest.raises(AttestationFailedError) as exc_info:
        models.generate("hello", "any.context")

    _assert_attestation_failed(exc_info.value)
    local["generate"].assert_not_called()
    establish.assert_called_once()


def test_confidential_stt_attestation_failure_blocks_remote_audio_egress(
    tmp_path,
    monkeypatch,
):
    _empty_journal(tmp_path, monkeypatch)
    config = _confidential_config(provider_pins=False)
    _add_local_endpoint(config)
    _seed_journal_config(tmp_path, config)
    establish = _install_failing_confidential_transport(monkeypatch)
    mocks = _install_stt_backend_mocks(
        monkeypatch,
        parakeet_result=[],
        confidential_result=None,
    )
    httpx_post = Mock(side_effect=AssertionError("audio egress attempted"))
    monkeypatch.setattr("httpx.post", httpx_post)

    from solstone.observe.transcribe import (
        BACKEND_REGISTRY,
        ConfidentialAudioEgressError,
        ConfidentialTranscribeDeferral,
        transcribe,
    )

    with pytest.raises(ConfidentialTranscribeDeferral) as confidential_exc:
        transcribe("confidential", _stt_audio(), 16000, {})
    assert confidential_exc.value.reason_code == "attestation_unreachable"

    monkeypatch.setitem(
        BACKEND_REGISTRY,
        "future-remote",
        "solstone.observe.transcribe.parakeet",
    )
    with pytest.raises(ConfidentialAudioEgressError):
        transcribe("future-remote", _stt_audio(), 16000, {})

    assert transcribe("parakeet", _stt_audio(), 16000, {}) == []
    mocks["parakeet"].assert_called_once()
    httpx_post.assert_not_called()
    establish.assert_called_once()


def test_confidential_stt_stale_session_replacement_failure_defers_before_egress(
    tmp_path, monkeypatch
):
    from solstone.think.services import spp, spp_transport
    from solstone.think.services.spp_attest.ratls.channel import RatlsChannelError

    _empty_journal(tmp_path, monkeypatch)
    config = _confidential_config(provider_pins=False)
    _add_local_endpoint(config)
    _seed_journal_config(tmp_path, config)
    spp_transport._LISTENER = _FakeListener()
    spp_transport._LISTENER_THREAD = _AliveThread()
    spp_transport._FORWARDER_BASE_URL = "http://127.0.0.1:4567"
    spp.record_attestation_verified(_stale_session(object()))
    mocks = _install_stt_backend_mocks(monkeypatch, confidential_result=None)
    establish = Mock(side_effect=RatlsChannelError("gateway_unreachable"))
    _stub_nvattest_installed(monkeypatch)
    monkeypatch.setattr(spp_transport, "establish_attested_channel", establish)
    httpx_post = Mock(side_effect=AssertionError("audio egress attempted"))
    monkeypatch.setattr("httpx.post", httpx_post)

    from solstone.observe.transcribe import ConfidentialTranscribeDeferral, transcribe

    with pytest.raises(ConfidentialTranscribeDeferral) as exc_info:
        transcribe("confidential", _stt_audio(), 16000, {})

    assert exc_info.value.reason_code == "attestation_unreachable"
    mocks["parakeet"].assert_not_called()
    httpx_post.assert_not_called()
    establish.assert_called_once()


def test_confidential_stt_setting_off_gate_blocks_confidential_only(
    tmp_path,
    monkeypatch,
):
    _empty_journal(tmp_path, monkeypatch)
    config = _confidential_config(provider_pins=False)
    config["transcribe"] = {"confidential_audio": False}
    _add_local_endpoint(config)
    _seed_journal_config(tmp_path, config)
    mocks = _install_stt_backend_mocks(monkeypatch, parakeet_result=[])
    httpx_post = Mock(side_effect=AssertionError("audio egress attempted"))
    monkeypatch.setattr("httpx.post", httpx_post)

    from solstone.observe.transcribe import (
        ConfidentialTranscribeDeferral,
        transcribe,
    )

    with pytest.raises(ConfidentialTranscribeDeferral) as exc_info:
        transcribe("confidential", _stt_audio(), 16000, {})
    assert exc_info.value.reason_code == "confidential_audio_disabled"

    assert transcribe("parakeet", _stt_audio(), 16000, {}) == []
    mocks["confidential"].assert_not_called()
    mocks["parakeet"].assert_called_once()
    httpx_post.assert_not_called()


def test_stt_registry_has_no_owner_selectable_cloud_backends():
    from solstone.observe.transcribe import BACKEND_METADATA, BACKEND_REGISTRY

    for name in BACKEND_REGISTRY:
        assert BACKEND_METADATA[name]["local"] is True or name == "confidential"


def test_confidential_stt_posts_only_to_verified_forwarder(tmp_path, monkeypatch):
    _empty_journal(tmp_path, monkeypatch)
    config = _confidential_config(provider_pins=False)
    _add_local_endpoint(config)
    _seed_journal_config(tmp_path, config)
    _patch_confidential_listener(monkeypatch)

    from solstone.think.services import spp_transport

    establish = Mock(
        side_effect=lambda *_args, **kwargs: _FakeChannel(
            object(), epoch=kwargs["epoch"]
        )
    )
    _stub_nvattest_installed(monkeypatch)
    monkeypatch.setattr(spp_transport, "establish_attested_channel", establish)
    captured: dict = {}

    def fake_post(url, **kwargs):
        captured["url"] = url
        captured.update(kwargs)
        return Mock(
            status_code=200,
            json=Mock(
                return_value={
                    "text": "hello.",
                    "words": [{"word": "hello.", "start": 0.0, "end": 0.5}],
                }
            ),
        )

    monkeypatch.setattr("httpx.post", fake_post)

    from solstone.observe.transcribe import transcribe

    statements = transcribe("confidential", _stt_audio(), 16000, {})

    assert statements
    assert captured["url"] == "http://127.0.0.1:4567/v1/audio/transcriptions"
    assert "spp.example.test" not in captured["url"]
    assert captured["headers"]["Authorization"] == "Bearer confidential-credential"
    assert captured["headers"]["x-sol-device"] == "fingerprint"
    establish.assert_called_once()


def test_confidential_stt_toggle_off_selection_is_immediate(tmp_path, monkeypatch):
    _empty_journal(tmp_path, monkeypatch)
    config = _confidential_config(provider_pins=False)
    _add_local_endpoint(config)
    _seed_journal_config(tmp_path, config)
    transcribe_main = importlib.import_module("solstone.observe.transcribe.main")
    monkeypatch.setattr(transcribe_main, "read_available_bytes", lambda: 1 * 1024**3)
    monkeypatch.setattr(transcribe_main, "stt_local_floor_bytes", lambda: 4 * 1024**3)
    monkeypatch.setattr(transcribe_main, "local_stt_backend", lambda: "parakeet")
    from solstone.think.utils import get_config

    args = type("Args", (), {"backend": None})()
    assert (
        transcribe_main.resolve_default_backend(
            args, get_config().get("transcribe", {})
        )
        == "confidential"
    )

    config["transcribe"] = {"confidential_audio": False}
    _seed_journal_config(tmp_path, config)

    assert (
        transcribe_main.resolve_default_backend(
            args, get_config().get("transcribe", {})
        )
        == "parakeet"
    )


def test_none_provider_module_fails_closed(tmp_path, monkeypatch):
    _empty_journal(tmp_path, monkeypatch)

    with pytest.raises(NoBrainConfiguredError):
        get_provider_module(NO_BRAIN_PROVIDER)

    assert not (tmp_path / "config" / "journal.json").exists()


@pytest.mark.parametrize(
    ("agent_type", "env_key"),
    [
        ("generate", "GOOGLE_API_KEY"),
        ("generate", "ANTHROPIC_API_KEY"),
        ("generate", "OPENAI_API_KEY"),
        ("cogitate", "GOOGLE_API_KEY"),
        ("cogitate", "ANTHROPIC_API_KEY"),
        ("cogitate", "OPENAI_API_KEY"),
    ],
)
def test_key_presence_does_not_implicitly_select_a_provider(
    tmp_path,
    monkeypatch,
    agent_type: str,
    env_key: str,
):
    _empty_journal(tmp_path, monkeypatch)
    original = _seed_journal_config(tmp_path, {"env": {env_key: "test-key"}})

    provider, model = resolve_provider(agent_type)

    assert provider == NO_BRAIN_PROVIDER
    assert model == ""
    assert (tmp_path / "config" / "journal.json").read_text(
        encoding="utf-8"
    ) == original


def test_model_only_config_does_not_infer_provider_from_key(tmp_path, monkeypatch):
    _empty_journal(tmp_path, monkeypatch)
    _seed_journal_config(
        tmp_path,
        {
            "env": {"GOOGLE_API_KEY": "test-key"},
            "providers": {"active": {"model": "gemini-custom"}},
        },
    )

    assert resolve_provider("generate") == (NO_BRAIN_PROVIDER, "")


def test_explicit_provider_does_not_fall_through_to_keyed_provider(
    tmp_path, monkeypatch
):
    _empty_journal(tmp_path, monkeypatch)
    _seed_journal_config(
        tmp_path,
        {
            "env": {"GOOGLE_API_KEY": "test-key"},
            "providers": {"active": {"provider": "anthropic"}},
        },
    )

    assert resolve_provider("generate") == ("anthropic", CLAUDE_SONNET_4)


def test_key_only_config_requires_maintenance_migration(tmp_path, monkeypatch):
    _empty_journal(tmp_path, monkeypatch)
    _seed_journal_config(tmp_path, {"env": {"GOOGLE_API_KEY": "test-key"}})

    assert resolve_provider("generate") == (NO_BRAIN_PROVIDER, "")


def test_runtime_readiness_does_not_implicitly_select_local(tmp_path, monkeypatch):
    _empty_journal(tmp_path, monkeypatch)

    provider, model = resolve_provider("generate")

    assert provider == NO_BRAIN_PROVIDER
    assert model == ""
    assert is_local_provider_needed() is False
    assert not (tmp_path / "config" / "journal.json").exists()


def test_explicit_local_type_default_neutralizes_cloud_context_pin(
    tmp_path,
    monkeypatch,
):
    _empty_journal(tmp_path, monkeypatch)
    _seed_journal_config(
        tmp_path,
        {
            "providers": {
                "active": {"provider": "local"},
                "contexts": {
                    "talent.timeline.segment_summary": {
                        "provider": "google",
                        "model": "gemini-3.1-flash-lite",
                    },
                },
            },
        },
    )

    provider, model = resolve_provider("generate")

    assert provider == "local"
    assert provider != "google"
    assert model == LOCAL_MODEL
    assert model != "gemini-3.1-flash-lite"
