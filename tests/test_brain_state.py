# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Public Python transport contracts for active-brain state."""

from __future__ import annotations

import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import pytest

from solstone.think.providers import brain_state as brain_state_module
from solstone.think.providers.brain_state import (
    begin_brain_refresh,
    build_active_brain_fingerprint,
    inspect_brain_state,
    record_brain_runtime_failure,
)

NOW = datetime(2026, 1, 2, 3, 4, 5, tzinfo=timezone.utc)


@pytest.fixture(autouse=True)
def native_brain_binary(monkeypatch: pytest.MonkeyPatch) -> None:
    """Exercise the public shim against the built native authority."""
    binary = Path(__file__).resolve().parents[1] / "core/target/debug/solstone-core"
    assert binary.is_file()
    monkeypatch.setattr(brain_state_module, "_native_binary", lambda **_kwargs: binary)


def _write_config(journal: Path, config: dict[str, Any]) -> None:
    path = journal / "config" / "journal.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(config), encoding="utf-8")


def _cloud_config(key: str = "config-secret", model: str = "gpt-5") -> dict[str, Any]:
    return {
        "providers": {"active": {"provider": "openai", "model": model}},
        "env": {"OPENAI_API_KEY": key},
    }


def _spp_config(*, account_id: str) -> dict[str, Any]:
    credential = "endpoint-secret"
    endpoint_url = "https://brain.example.test/v1"
    served_model_id = "served-model"
    return {
        "providers": {
            "active": {"provider": "local", "model": "local/bundled"},
            "local": {
                "endpoint_url": endpoint_url,
                "served_model_id": served_model_id,
                "credential": credential,
            },
        },
        "services": {
            "confidential": {
                "enabled_at": "2026-01-02T03:04:05+00:00",
                "account_id": account_id,
                "endpoint_url": endpoint_url,
                "served_model_id": served_model_id,
                "credential_created_at": "2026-01-02T03:00:00+00:00",
                "credential_fingerprint_sha256": hashlib.sha256(
                    credential.encode("utf-8")
                ).hexdigest(),
                "prior_active": {"provider": "google", "model": "gemini-flash-latest"},
            }
        },
        "env": {},
    }


def test_none_lane_is_blocked_thinking_engine_not_chosen(tmp_path: Path) -> None:
    _write_config(tmp_path, {"providers": {"active": {"provider": "none"}}, "env": {}})

    assert begin_brain_refresh(NOW, journal_path=tmp_path) is None
    projection = inspect_brain_state(NOW, journal_path=tmp_path)["projection"]

    assert projection["active_lane"] == "none"
    assert projection["aggregate_state"] == "blocked"
    assert projection["reason_code"] == "thinking_engine_not_chosen"


def test_fingerprint_uses_config_env_not_process_env(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    key = b"k" * 32
    config = _cloud_config(key="from-config")
    monkeypatch.setenv("OPENAI_API_KEY", "from-process")

    first = build_active_brain_fingerprint(config, hmac_key=key)
    monkeypatch.setenv("OPENAI_API_KEY", "changed-process")
    second = build_active_brain_fingerprint(config, hmac_key=key)
    third = build_active_brain_fingerprint(_cloud_config(key="changed-config"), hmac_key=key)

    assert first["fingerprint_sha256"] == second["fingerprint_sha256"]
    assert first["fingerprint_sha256"] != third["fingerprint_sha256"]


def test_runtime_failure_ingress_rejects_checking_reason(tmp_path: Path) -> None:
    result = record_brain_runtime_failure(
        "brain_check_in_progress",
        NOW,
        expected_fingerprint_sha256="a" * 64,
        component="lane_prerequisites",
        journal_path=tmp_path,
    )

    assert result["accepted"] is False
    assert result["rejected_reason"] == "reason_not_recordable"


def test_diagnostic_string_must_be_declared_enum(tmp_path: Path) -> None:
    result = record_brain_runtime_failure(
        "local_server_unhealthy",
        NOW,
        expected_fingerprint_sha256="a" * 64,
        component="lane_prerequisites",
        diagnostic={"phase": "sk-secret-credential"},
        journal_path=tmp_path,
    )

    assert result["accepted"] is False
    assert result["rejected_reason"] == "reason_not_recordable"


def test_spp_fingerprint_tracks_confidential_active_provenance() -> None:
    key = b"k" * 32
    before = build_active_brain_fingerprint(_spp_config(account_id="acct-a"), hmac_key=key)
    after = build_active_brain_fingerprint(_spp_config(account_id="acct-b"), hmac_key=key)

    assert before["active_lane"] == "spp"
    assert after["active_lane"] == "spp"
    assert before["fingerprint_sha256"] != after["fingerprint_sha256"]


def test_active_model_changes_fingerprint_but_byo_memory_does_not() -> None:
    key = b"k" * 32
    config = {
        "providers": {
            "active": {"provider": "google", "model": "gemini-3.5-flash"},
            "byo_models": {"google": "gemini-flash-latest"},
        },
        "env": {"GOOGLE_API_KEY": "google-secret"},
    }
    active_changed = json.loads(json.dumps(config))
    active_changed["providers"]["active"]["model"] = "gemini-3.1-flash-lite"
    remembered_changed = json.loads(json.dumps(config))
    remembered_changed["providers"]["byo_models"]["google"] = "gemini-3.5-flash"

    before = build_active_brain_fingerprint(config, hmac_key=key)
    active_after = build_active_brain_fingerprint(active_changed, hmac_key=key)
    remembered_after = build_active_brain_fingerprint(remembered_changed, hmac_key=key)

    assert before["fingerprint_sha256"] != active_after["fingerprint_sha256"]
    assert before["fingerprint_sha256"] == remembered_after["fingerprint_sha256"]
