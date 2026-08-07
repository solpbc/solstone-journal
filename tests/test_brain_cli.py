# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import argparse
import hashlib
import json
import logging
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from solstone.think import brain_cli
from solstone.think.providers import brain_state as brain_state_module
from solstone.think.providers.brain_state import (
    DEFAULT_READY_EVIDENCE_TTL,
    BrainProbeOutcome,
    BrainStateConflictError,
    begin_brain_refresh,
    brain_state_path,
    finish_brain_refresh,
)
from tests.openhands_fakes import install_fake_openhands

NOW = datetime.now(timezone.utc)
RUNTIME_FP = "b" * 64


def _write_config(journal: Path, config: dict[str, Any]) -> None:
    path = journal / "config" / "journal.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(config), encoding="utf-8")


def _cloud_config(
    *, key: str = "config-secret", model: str = "gpt-5"
) -> dict[str, Any]:
    env = {"OPENAI_API_KEY": key} if key else {}
    return {
        "providers": {"active": {"provider": "openai", "model": model}},
        "env": env,
    }


def _none_config() -> dict[str, Any]:
    return {"providers": {"active": {"provider": "none"}}, "env": {}}


def _bundled_config() -> dict[str, Any]:
    return {
        "providers": {"active": {"provider": "local", "model": "local/qwen3.5-4b"}},
        "env": {},
    }


def _endpoint_config(*, confidential: bool = False) -> dict[str, Any]:
    config: dict[str, Any] = {
        "providers": {
            "active": {"provider": "local", "model": "local/qwen3.5-4b"},
            "local": {
                "endpoint_url": "https://brain.example.test/v1",
                "served_model_id": "served-model",
                "credential": "endpoint-secret",
            },
        },
        "env": {},
    }
    if confidential:
        config["services"] = {
            "confidential": {
                "endpoint_url": "https://brain.example.test",
                "served_model_id": "served-model",
                "credential_fingerprint_sha256": hashlib.sha256(
                    b"endpoint-secret"
                ).hexdigest(),
            }
        }
    return config


def _invalid_endpoint_config() -> dict[str, Any]:
    return {
        "providers": {
            "active": {"provider": "local", "model": "local/qwen3.5-4b"},
            "local": {"endpoint_url": "https://brain.example.test/v1"},
        },
        "env": {},
    }


def _component(
    now: datetime = NOW, *, expires_at: datetime | None = None
) -> dict[str, Any]:
    return {
        "status": "ok",
        "observed_at": now.isoformat(),
        "expires_at": (expires_at or now + DEFAULT_READY_EVIDENCE_TTL).isoformat(),
    }


def _ready_outcome(now: datetime = NOW) -> BrainProbeOutcome:
    return {
        "configuration": _component(now),
        "lane_prerequisites": _component(now),
        "generate": _component(now),
        "cogitate": _component(now),
    }


def _write_ready_record(
    journal: Path,
    config: dict[str, Any],
    *,
    now: datetime = NOW,
    expires_at: datetime | None = None,
) -> None:
    _write_config(journal, config)
    permit = begin_brain_refresh(now, journal_path=journal)
    assert permit is not None
    outcome = _ready_outcome(now)
    if expires_at is not None:
        for component in outcome.values():
            assert component is not None
            component["expires_at"] = expires_at.isoformat()
    finish_brain_refresh(permit, outcome, now, journal_path=journal)


def _health_snapshot(journal: Path) -> dict[str, bytes]:
    root = journal / "health"
    if not root.exists():
        return {}
    return {
        path.relative_to(root).as_posix(): path.read_bytes()
        for path in sorted(root.rglob("*"))
        if path.is_file()
    }


def _args(**kwargs: Any) -> argparse.Namespace:
    defaults = {
        "json": False,
        "expected_fingerprint": None,
        "expected_active_fingerprint": False,
        "expect_active_fingerprint_absent": False,
    }
    defaults.update(kwargs)
    return argparse.Namespace(**defaults)


@pytest.fixture
def brain_journal(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(brain_cli, "_now", lambda: NOW)
    monkeypatch.setattr(brain_state_module, "get_journal", lambda: str(tmp_path))
    native_binary = (
        Path(__file__).resolve().parents[1] / "core/target/debug/solstone-core"
    )
    assert native_binary.is_file()
    monkeypatch.setattr(
        brain_state_module,
        "_native_binary",
        lambda **_kwargs: native_binary,
    )
    monkeypatch.setattr(brain_cli, "require_solstone", lambda: None)
    return tmp_path


def _forbid_provider_calls(monkeypatch: pytest.MonkeyPatch) -> None:
    def fail(*_args: Any, **_kwargs: Any) -> None:
        raise AssertionError("provider call was not expected")

    monkeypatch.setattr(brain_cli, "generate_with_result", fail)
    monkeypatch.setattr(brain_cli, "get_provider_module", fail)


def _run_status_json(capsys: pytest.CaptureFixture[str]) -> dict[str, Any]:
    code = brain_cli._run_status(_args(json=True))
    out = capsys.readouterr().out
    payload = json.loads(out)
    payload["exit_code"] = code
    return payload


@pytest.mark.parametrize(
    ("name", "prepare", "expected_text", "expected_exit"),
    [
        (
            "ready",
            lambda journal, monkeypatch: _write_ready_record(journal, _cloud_config()),
            "Brain ready:",
            0,
        ),
        (
            "blocked",
            lambda journal, monkeypatch: (
                _write_config(journal, _none_config()),
                begin_brain_refresh(NOW, journal_path=journal),
            ),
            "Brain blocked: no thinking engine chosen",
            1,
        ),
        (
            "unknown",
            lambda journal, monkeypatch: _write_config(
                journal, _invalid_endpoint_config()
            ),
            "Brain unknown: configuration invalid",
            2,
        ),
        (
            "unhealthy",
            lambda journal, monkeypatch: _write_unhealthy_record(journal),
            "Brain unhealthy: provider response invalid (generate)",
            1,
        ),
        (
            "checking",
            lambda journal, monkeypatch: monkeypatch.setattr(
                brain_cli,
                "_held_permit",
                _prepare_held_check(journal),
                raising=False,
            ),
            "Brain checking: brain check in progress",
            2,
        ),
    ],
)
def test_status_human_renders_key_state_classes(
    brain_journal: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    name: str,
    prepare,
    expected_text: str,
    expected_exit: int,
) -> None:
    del name
    prepare(brain_journal, monkeypatch)
    _forbid_provider_calls(monkeypatch)

    code = brain_cli._run_status(_args(json=False))
    out = capsys.readouterr().out

    assert code == expected_exit
    assert expected_text in out

    held = getattr(brain_cli, "_held_permit", None)
    if held is not None:
        held.release()


def _prepare_interrupted_check(journal: Path) -> None:
    _write_config(journal, _cloud_config())
    permit = begin_brain_refresh(NOW, journal_path=journal)
    assert permit is not None
    permit.release()


def _prepare_held_check(journal: Path):
    _write_config(journal, _cloud_config())
    permit = begin_brain_refresh(NOW, journal_path=journal)
    assert permit is not None
    return permit


def _write_unhealthy_record(journal: Path) -> None:
    _write_config(journal, _cloud_config())
    permit = begin_brain_refresh(NOW, journal_path=journal)
    assert permit is not None
    finish_brain_refresh(
        permit,
        {
            "configuration": _component(),
            "lane_prerequisites": _component(),
            "generate": brain_cli._failed_component(NOW, "provider_response_invalid"),
            "cogitate": _component(),
        },
        NOW,
        journal_path=journal,
    )


@pytest.mark.parametrize(
    ("raw_reason", "expected_reason", "expected_status"),
    [
        ("nvattest_install_in_progress", "nvattest_install_in_progress", "blocked"),
        ("nvattest_platform_unsupported", "nvattest_platform_unsupported", "blocked"),
        ("nvattest_unavailable", "nvattest_unavailable", "blocked"),
        ("nvattest_install_failed", "nvattest_install_failed", "failed"),
        ("nvattest_integrity_failed", "nvattest_integrity_failed", "failed"),
        ("gateway_unreachable", "attestation_not_verified", "blocked"),
        ("attestation_failed", "attestation_rejected", "failed"),
        ("pcr_pin_mismatch", "attestation_rejected", "failed"),
    ],
)
def test_spp_prerequisite_maps_failure_reason_code(
    monkeypatch: pytest.MonkeyPatch,
    caplog: pytest.LogCaptureFixture,
    raw_reason: str,
    expected_reason: str,
    expected_status: str,
) -> None:
    from solstone.think.services import spp

    spp.delete_attestation_state()
    caplog.set_level(logging.WARNING, logger=brain_cli.LOG.name)
    monkeypatch.setattr(
        "solstone.think.services.spp_transport.recheck_confidential_attestation",
        lambda: spp.record_attestation_failed("failed", raw_reason),
    )
    try:
        component, reason = brain_cli._spp_prerequisite(NOW)
    finally:
        spp.delete_attestation_state()

    assert reason == expected_reason
    assert component["reason_code"] == expected_reason
    assert component["status"] == expected_status
    assert "event=spp_attestation_reason_unmapped" not in caplog.text


def test_spp_prerequisite_warns_and_fails_closed_on_unmapped_reason(
    monkeypatch: pytest.MonkeyPatch,
    caplog: pytest.LogCaptureFixture,
) -> None:
    from solstone.think.services import spp

    spp.delete_attestation_state()
    monkeypatch.setattr(
        "solstone.think.services.spp_transport.recheck_confidential_attestation",
        lambda: spp.record_attestation_failed("failed", "new_raw_reason"),
    )
    caplog.set_level(logging.WARNING, logger=brain_cli.LOG.name)
    try:
        component, reason = brain_cli._spp_prerequisite(NOW)
    finally:
        spp.delete_attestation_state()

    assert reason == "attestation_rejected"
    assert component["reason_code"] == "attestation_rejected"
    assert (
        "event=spp_attestation_reason_unmapped raw_reason=new_raw_reason" in caplog.text
    )


def _fake_runtime_inspection(
    *, phase: str = "ready", desired: str = RUNTIME_FP
) -> dict[str, Any]:
    return {
        "status": "ok",
        "provider": "local",
        "record_kind": "health",
        "path": "/tmp/local.json",
        "record": {
            "schema_version": 1,
            "provider": "local",
            "revision": 1,
            "phase": phase,
            "reason_code": None,
            "detail": {},
            "desired_fingerprint_sha256": desired,
            "incarnation": None,
            "generation": 1,
            "attempt": 0,
            "process": None,
            "updated_at": NOW.isoformat(),
            "display_deadline_at": None,
            "owner": None,
        },
        "reason_code": None,
        "error": None,
    }


class _FakeProviderModule:
    def __init__(self, calls: list[dict[str, Any]], result: str | None = "OK") -> None:
        self.calls = calls
        self.result = result

    async def run_cogitate(
        self, *, config: dict[str, Any], on_event: Any
    ) -> str | None:
        assert on_event is None
        self.calls.append(config)
        return self.result


class _ReasonedProviderError(RuntimeError):
    def __init__(self, reason_code: str) -> None:
        super().__init__(reason_code)
        self.reason_code = reason_code


def _patch_generate_and_cogitate(
    monkeypatch: pytest.MonkeyPatch,
    *,
    generate_result: dict[str, Any] | None = None,
    generate_exc: BaseException | None = None,
    cogitate_result: str | None = "OK",
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    generate_calls: list[dict[str, Any]] = []
    cogitate_calls: list[dict[str, Any]] = []

    def fake_generate(*args: Any, **kwargs: Any) -> dict[str, Any]:
        generate_calls.append({"args": args, "kwargs": kwargs})
        if generate_exc is not None:
            raise generate_exc
        return generate_result or {"text": "OK", "finish_reason": "stop"}

    monkeypatch.setattr(brain_cli, "generate_with_result", fake_generate)
    monkeypatch.setattr(
        brain_cli,
        "get_provider_module",
        lambda provider: _FakeProviderModule(cogitate_calls, cogitate_result),
    )
    return generate_calls, cogitate_calls


@pytest.mark.parametrize(
    ("lane", "config", "patch_prereq"),
    [
        ("byo-cloud", _cloud_config(), None),
        ("byo-endpoint", _endpoint_config(), None),
        ("spp", _endpoint_config(confidential=True), "spp"),
    ],
)
def test_refresh_success_proves_each_lane_once(
    brain_journal: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    lane: str,
    config: dict[str, Any],
    patch_prereq: str | None,
) -> None:
    _write_config(brain_journal, config)
    if patch_prereq == "bundled":
        monkeypatch.setattr(
            brain_state_module,
            "_bundled_runtime_fingerprint_sha",
            lambda: RUNTIME_FP,
        )
        monkeypatch.setattr(
            brain_cli,
            "_current_bundled_runtime_fingerprint",
            lambda _config: RUNTIME_FP,
        )
        monkeypatch.setattr(
            brain_cli,
            "inspect_runtime_health",
            lambda _provider: _fake_runtime_inspection(),
        )
    elif patch_prereq == "spp":
        monkeypatch.setattr(
            brain_cli,
            "_spp_prerequisite",
            lambda now: (brain_cli._ok_component(now), None),
        )
    generate_calls, cogitate_calls = _patch_generate_and_cogitate(monkeypatch)

    code = brain_cli._run_refresh(_args(json=True))
    payload = json.loads(capsys.readouterr().out)

    assert code == 0
    assert payload["aggregate_state"] == "ready"
    assert payload["lane"] == lane
    assert len(generate_calls) == 1
    assert generate_calls[0]["kwargs"]["num_retries"] == 0
    assert generate_calls[0]["kwargs"]["thinking_budget"] == 0
    assert len(cogitate_calls) == 1
    assert cogitate_calls[0]["provider"] == payload["provider"]
    assert cogitate_calls[0]["model"] == payload["model"]
    assert cogitate_calls[0]["diagnostic"] is True


def test_refresh_prerequisite_failure_skips_inference_and_commits_not_attempted(
    brain_journal: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _write_config(brain_journal, _bundled_config())
    monkeypatch.setattr(
        brain_state_module,
        "_bundled_runtime_fingerprint_sha",
        lambda: RUNTIME_FP,
    )
    monkeypatch.setattr(
        brain_cli,
        "_current_bundled_runtime_fingerprint",
        lambda _config: RUNTIME_FP,
    )
    monkeypatch.setattr(
        brain_cli,
        "inspect_runtime_health",
        lambda _provider: _fake_runtime_inspection(phase="starting"),
    )
    generate_calls, cogitate_calls = _patch_generate_and_cogitate(monkeypatch)

    code = brain_cli._run_refresh(_args(json=True))
    payload = json.loads(capsys.readouterr().out)
    record = json.loads(brain_state_path(journal_path=brain_journal).read_text())

    assert code == 1
    assert payload["reason_code"] == "local_runtime_not_ready"
    assert generate_calls == []
    assert cogitate_calls == []
    assert record["evidence"]["generate"]["status"] == "not_attempted"
    assert record["evidence"]["cogitate"]["reason_code"] == "local_runtime_not_ready"


def test_renew_prerequisites_updates_spp_only_without_model_probes(
    brain_journal: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    config = _endpoint_config(confidential=True)
    prior_now = NOW - timedelta(minutes=1)
    _write_ready_record(brain_journal, config, now=prior_now)
    before = json.loads(brain_state_path(journal_path=brain_journal).read_text())
    expected = brain_state_module.read_active_brain_fingerprint_sha256(
        journal_path=brain_journal
    )
    assert expected is not None
    _forbid_provider_calls(monkeypatch)
    monkeypatch.setattr(
        brain_cli,
        "_spp_prerequisite",
        lambda now: (
            brain_cli._ok_component(now, expires_at=now + timedelta(minutes=10)),
            None,
        ),
    )

    code = brain_cli._run_renew_prerequisites(
        _args(json=True, expected_fingerprint=expected)
    )
    payload = json.loads(capsys.readouterr().out)
    record = json.loads(brain_state_path(journal_path=brain_journal).read_text())

    assert code == 0
    assert payload["aggregate_state"] == "ready"
    assert record["evidence"]["configuration"] == before["evidence"]["configuration"]
    assert record["evidence"]["generate"] == before["evidence"]["generate"]
    assert record["evidence"]["cogitate"] == before["evidence"]["cogitate"]
    assert record["evidence"]["lane_prerequisites"]["observed_at"] == NOW.isoformat()


def test_renew_prerequisites_unsafe_dispatches_full_refresh(
    brain_journal: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _write_config(brain_journal, _cloud_config())
    calls: list[argparse.Namespace] = []

    def fake_refresh(args: argparse.Namespace) -> int:
        calls.append(args)
        return 7

    monkeypatch.setattr(brain_cli, "_run_refresh", fake_refresh)

    assert brain_cli._run_renew_prerequisites(_args(json=True)) == 7
    assert len(calls) == 1
    assert calls[0].expected_active_fingerprint is True


def test_renew_prerequisites_expected_mismatch_writes_nothing(
    brain_journal: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _write_ready_record(brain_journal, _endpoint_config(confidential=True))
    before = _health_snapshot(brain_journal)
    _forbid_provider_calls(monkeypatch)

    code = brain_cli._run_renew_prerequisites(
        _args(json=True, expected_fingerprint="f" * 64)
    )
    payload = json.loads(capsys.readouterr().out)

    assert code == 3
    assert payload["reason_code"] == "stale_expected_fingerprint"
    assert _health_snapshot(brain_journal) == before


def test_refresh_cloud_missing_key_skips_inference(
    brain_journal: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _write_config(brain_journal, _cloud_config(key=""))
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    generate_calls, cogitate_calls = _patch_generate_and_cogitate(monkeypatch)

    code = brain_cli._run_refresh(_args(json=True))
    payload = json.loads(capsys.readouterr().out)

    assert code == 1
    assert payload["reason_code"] == "provider_key_missing"
    assert generate_calls == []
    assert cogitate_calls == []


def test_refresh_generate_failure_does_not_suppress_cogitate(
    brain_journal: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _write_config(brain_journal, _cloud_config())
    generate_calls, cogitate_calls = _patch_generate_and_cogitate(
        monkeypatch,
        generate_result={"text": "", "finish_reason": "stop"},
        cogitate_result="OK",
    )

    code = brain_cli._run_refresh(_args(json=True))
    payload = json.loads(capsys.readouterr().out)
    record = json.loads(brain_state_path(journal_path=brain_journal).read_text())

    assert code == 1
    assert payload["reason_code"] == "provider_response_invalid"
    assert len(generate_calls) == 1
    assert len(cogitate_calls) == 1
    assert record["evidence"]["generate"]["reason_code"] == "provider_response_invalid"
    assert record["evidence"]["cogitate"]["status"] == "ok"


@pytest.mark.parametrize(
    ("generate_result", "generate_exc", "expected_reason", "expected_exit"),
    [
        ({"text": "", "finish_reason": "max_tokens"}, None, "probe_output_starved", 2),
        ({"text": "", "finish_reason": "stop"}, None, "provider_response_invalid", 1),
        (None, _ReasonedProviderError("network_unreachable"), "network_unreachable", 1),
        (
            None,
            _ReasonedProviderError("context_window_exceeded"),
            "probe_internal_error",
            2,
        ),
    ],
)
def test_refresh_generate_mapping_branches(
    brain_journal: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    generate_result: dict[str, Any] | None,
    generate_exc: BaseException | None,
    expected_reason: str,
    expected_exit: int,
) -> None:
    _write_config(brain_journal, _cloud_config())
    generate_calls, cogitate_calls = _patch_generate_and_cogitate(
        monkeypatch,
        generate_result=generate_result,
        generate_exc=generate_exc,
    )

    code = brain_cli._run_refresh(_args(json=True))
    payload = json.loads(capsys.readouterr().out)
    record = json.loads(brain_state_path(journal_path=brain_journal).read_text())

    assert code == expected_exit
    assert payload["reason_code"] == expected_reason
    assert len(generate_calls) == 1
    assert len(cogitate_calls) == 1
    assert record["evidence"]["generate"]["reason_code"] == expected_reason
    assert record["evidence"]["cogitate"]["status"] == "ok"


def test_refresh_excludes_inactive_providers(
    brain_journal: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _write_config(
        brain_journal,
        {
            "providers": {"active": {"provider": "openai", "model": "gpt-5.4-mini"}},
            "env": {
                "OPENAI_API_KEY": "active-key",
                "ANTHROPIC_API_KEY": "inactive-key",
                "GOOGLE_API_KEY": "inactive-key",
            },
        },
    )
    generate_calls: list[dict[str, Any]] = []
    cogitate_calls: list[dict[str, Any]] = []

    def fake_generate(*args: Any, **kwargs: Any) -> dict[str, Any]:
        from solstone.think.models import resolve_provider

        provider, model = resolve_provider("generate")
        generate_calls.append({"provider": provider, "model": model, "kwargs": kwargs})
        return {"text": "OK", "finish_reason": "stop"}

    def fake_module(provider: str) -> _FakeProviderModule:
        assert provider == "openai"
        return _FakeProviderModule(cogitate_calls)

    monkeypatch.setattr(brain_cli, "generate_with_result", fake_generate)
    monkeypatch.setattr(brain_cli, "get_provider_module", fake_module)

    code = brain_cli._run_refresh(_args(json=True))
    payload = json.loads(capsys.readouterr().out)

    assert code == 0
    assert payload["provider"] == "openai"
    assert len(generate_calls) == 1
    assert generate_calls[0]["provider"] == "openai"
    assert generate_calls[0]["model"] == "gpt-5.4-mini"
    assert cogitate_calls[0]["provider"] == "openai"


def test_byo_endpoint_generate_uses_served_model_post_not_get_probe(
    brain_journal: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    import httpx

    from solstone.think.providers import local_admission, local_endpoint

    class NullAdmission:
        def __enter__(self):
            return self

        def __exit__(self, *_args: Any) -> None:
            return None

        def release(self) -> None:
            return None

    class FakeResponse:
        def raise_for_status(self) -> None:
            return None

        def json(self) -> dict[str, Any]:
            return {
                "choices": [
                    {
                        "message": {"content": "OK"},
                        "finish_reason": "stop",
                    }
                ]
            }

    posts: list[dict[str, Any]] = []
    _write_config(brain_journal, _endpoint_config())
    monkeypatch.setattr(
        local_endpoint,
        "probe_local_endpoint",
        lambda *_args, **_kwargs: pytest.fail("GET / endpoint probe was used"),
    )
    monkeypatch.setattr(
        local_admission,
        "acquire_local_slot",
        lambda *_args, **_kwargs: NullAdmission(),
    )

    def fake_post(url: str, **kwargs: Any) -> FakeResponse:
        posts.append({"url": url, "kwargs": kwargs})
        return FakeResponse()

    monkeypatch.setattr(httpx, "post", fake_post)
    cogitate_calls: list[dict[str, Any]] = []
    monkeypatch.setattr(
        brain_cli,
        "get_provider_module",
        lambda _provider: _FakeProviderModule(cogitate_calls),
    )

    code = brain_cli._run_refresh(_args(json=True))
    payload = json.loads(capsys.readouterr().out)

    assert code == 0
    assert payload["lane"] == "byo-endpoint"
    assert len(posts) == 1
    assert posts[0]["url"] == "https://brain.example.test/v1/chat/completions"
    assert posts[0]["kwargs"]["json"]["model"] == "served-model"
    assert len(cogitate_calls) == 1
    assert cogitate_calls[0]["provider"] == "local"


def test_refresh_none_lane_persists_blocked_without_provider_calls(
    brain_journal: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _write_config(brain_journal, _none_config())
    _forbid_provider_calls(monkeypatch)

    code = brain_cli._run_refresh(_args(json=True))
    payload = json.loads(capsys.readouterr().out)

    assert code == 1
    assert payload["reason_code"] == "thinking_engine_not_chosen"
    assert brain_state_path(journal_path=brain_journal).exists()


def test_refresh_configuration_invalid_writes_nothing(
    brain_journal: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _write_config(brain_journal, _invalid_endpoint_config())
    before = _health_snapshot(brain_journal)
    _forbid_provider_calls(monkeypatch)

    code = brain_cli._run_refresh(_args(json=True))
    payload = json.loads(capsys.readouterr().out)

    assert code == 2
    assert payload["reason_code"] == "configuration_invalid"
    assert _health_snapshot(brain_journal) == before


def test_refresh_lost_fence_exits_three(
    brain_journal: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _write_config(brain_journal, _cloud_config())
    _patch_generate_and_cogitate(monkeypatch)

    def lost(*_args: Any, **_kwargs: Any) -> None:
        raise BrainStateConflictError("lost")

    monkeypatch.setattr(brain_cli, "finish_brain_refresh", lost)

    code = brain_cli._run_refresh(_args(json=True))
    payload = json.loads(capsys.readouterr().out)
    record = json.loads(brain_state_path(journal_path=brain_journal).read_text())

    assert code == 3
    assert payload["reason_code"] == "lost_fence"
    assert record["aggregate_state"] == "checking"


def test_expected_fingerprint_mismatch_or_non_bundled_exits_stale(
    brain_journal: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _write_config(brain_journal, _cloud_config())
    _forbid_provider_calls(monkeypatch)

    code = brain_cli._run_refresh(_args(json=True, expected_fingerprint=RUNTIME_FP))
    payload = json.loads(capsys.readouterr().out)

    assert code == 3
    assert payload["reason_code"] == "stale_expected_fingerprint"


def test_refresh_expected_active_fingerprint_mismatch_writes_nothing(
    brain_journal: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    original = _endpoint_config(confidential=True)
    changed = _endpoint_config(confidential=True)
    changed["providers"]["local"]["served_model_id"] = "new-served-model"
    changed["services"]["confidential"]["served_model_id"] = "new-served-model"
    _write_ready_record(brain_journal, original)
    expected = brain_state_module.read_active_brain_fingerprint_sha256(
        journal_path=brain_journal
    )
    assert expected is not None
    before = _health_snapshot(brain_journal)
    _write_config(brain_journal, changed)
    _forbid_provider_calls(monkeypatch)

    code = brain_cli._run_refresh(
        _args(
            json=True,
            expected_fingerprint=expected,
            expected_active_fingerprint=True,
        )
    )
    payload = json.loads(capsys.readouterr().out)

    assert code == 3
    assert payload["reason_code"] == "stale_expected_fingerprint"
    assert _health_snapshot(brain_journal) == before


def test_refresh_expected_active_fingerprint_race_before_begin_writes_nothing(
    brain_journal: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    original = _endpoint_config(confidential=True)
    changed = _endpoint_config(confidential=True)
    changed["providers"]["local"]["served_model_id"] = "race-served-model"
    changed["services"]["confidential"]["served_model_id"] = "race-served-model"
    _write_ready_record(brain_journal, original)
    expected = brain_state_module.read_active_brain_fingerprint_sha256(
        journal_path=brain_journal
    )
    assert expected is not None
    before = _health_snapshot(brain_journal)
    real_match = brain_cli._expected_refresh_fingerprint_matches

    def match_then_switch(args: argparse.Namespace, value: str) -> bool:
        assert real_match(args, value)
        _write_config(brain_journal, changed)
        return True

    monkeypatch.setattr(
        brain_cli,
        "_expected_refresh_fingerprint_matches",
        match_then_switch,
    )
    _forbid_provider_calls(monkeypatch)

    code = brain_cli._run_refresh(
        _args(
            json=True,
            expected_fingerprint=expected,
            expected_active_fingerprint=True,
        )
    )
    payload = json.loads(capsys.readouterr().out)

    assert code == 3
    assert payload["reason_code"] == "stale_expected_fingerprint"
    assert _health_snapshot(brain_journal) == before


def test_refresh_expected_absent_fingerprint_stale_when_key_appears(
    brain_journal: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _write_ready_record(brain_journal, _endpoint_config(confidential=True))
    before = _health_snapshot(brain_journal)
    _forbid_provider_calls(monkeypatch)

    code = brain_cli._run_refresh(
        _args(json=True, expect_active_fingerprint_absent=True)
    )
    payload = json.loads(capsys.readouterr().out)

    assert code == 3
    assert payload["reason_code"] == "stale_expected_fingerprint"
    assert _health_snapshot(brain_journal) == before


def test_refresh_diagnostic_cogitate_constructs_zero_retry_llm(
    brain_journal: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    from solstone.think.providers import emit_final_tool, openhands

    fake_openhands = install_fake_openhands(monkeypatch)
    emit_final_tool._EMIT_FINAL_TYPES.clear()
    monkeypatch.setattr(openhands, "get_journal", lambda: brain_journal)
    monkeypatch.setattr(openhands, "get_project_root", lambda: brain_journal)
    monkeypatch.setattr(brain_cli, "get_provider_module", lambda _provider: openhands)
    monkeypatch.setenv("OPENAI_API_KEY", "config-secret")
    _write_config(brain_journal, _cloud_config())

    async def emit_final(conversation):
        for callback in conversation.callbacks:
            callback(
                fake_openhands.ActionEvent(
                    reasoning_content=None,
                    thinking_blocks=[],
                    responses_reasoning_item=None,
                    tool_name="emit_final",
                    tool_call=SimpleNamespace(arguments={"content": "OK"}),
                    tool_call_id="emit-1",
                    action=SimpleNamespace(content="OK"),
                )
            )

    fake_openhands.Conversation.arun_impl = emit_final
    generate_calls: list[dict[str, Any]] = []

    def fake_generate(*args: Any, **kwargs: Any) -> dict[str, Any]:
        generate_calls.append({"args": args, "kwargs": kwargs})
        return {"text": "OK", "finish_reason": "stop"}

    monkeypatch.setattr(brain_cli, "generate_with_result", fake_generate)

    code = brain_cli._run_refresh(_args(json=True))
    json.loads(capsys.readouterr().out)

    assert code == 0
    assert generate_calls[0]["kwargs"]["num_retries"] == 0
    assert fake_openhands.LLM.instances[-1].num_retries == 0


@pytest.mark.parametrize(
    ("aggregate_state", "expected"),
    [
        ("ready", 0),
        ("blocked", 1),
        ("unhealthy", 1),
        ("unknown", 2),
        ("checking", 2),
    ],
)
def test_exit_table_for_status_and_refresh_aggregates(
    aggregate_state: str,
    expected: int,
) -> None:
    assert brain_cli.brain_exit_code(aggregate_state=aggregate_state) == expected


@pytest.mark.parametrize(
    "refresh_outcome",
    ["busy", "stale_expected_fingerprint", "lost_fence"],
)
def test_exit_table_for_refresh_only_outcomes(refresh_outcome: str) -> None:
    assert brain_cli.brain_exit_code(refresh_outcome=refresh_outcome) == 3


def test_module_main_help_and_status_are_clean(
    brain_journal: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _write_config(brain_journal, _cloud_config())
    _forbid_provider_calls(monkeypatch)
    before = _health_snapshot(brain_journal)

    monkeypatch.setattr(sys, "argv", ["journal brain", "--help"])
    with pytest.raises(SystemExit) as help_exit:
        brain_cli.main()
    assert help_exit.value.code == 0
    assert "status" in capsys.readouterr().out

    monkeypatch.setattr(sys, "argv", ["journal brain", "status"])
    with pytest.raises(SystemExit) as status_exit:
        brain_cli.main()
    assert status_exit.value.code == 2
    assert "Brain unknown" in capsys.readouterr().out

    monkeypatch.setattr(sys, "argv", ["journal brain", "status", "--json"])
    with pytest.raises(SystemExit) as json_exit:
        brain_cli.main()
    assert json_exit.value.code == 2
    payload = json.loads(capsys.readouterr().out)
    assert payload["reason_code"] == "brain_record_missing"
    assert _health_snapshot(brain_journal) == before
