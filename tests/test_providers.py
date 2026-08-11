# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import hashlib
import logging

import pytest

from solstone.think.providers import build_provider_status
from tests.helpers.journal_config import seed_journal_config

CLOUD_PROVIDERS = {
    "anthropic": "ANTHROPIC_API_KEY",
    "openai": "OPENAI_API_KEY",
    "google": "GOOGLE_API_KEY",
}


def _provider(name: str, env_key: str) -> dict[str, str]:
    return {"name": name, "label": name.title(), "env_key": env_key}


def test_cogitate_baseline_ready_with_hosted_key(monkeypatch) -> None:
    monkeypatch.setenv("ANTHROPIC_API_KEY", "test-key")

    status = build_provider_status([_provider("anthropic", "ANTHROPIC_API_KEY")])[
        "anthropic"
    ]

    assert status == {
        "provider": "anthropic",
        "configured": True,
        "generate_ready": True,
        "cogitate_ready": True,
        "issues": [],
    }


@pytest.mark.parametrize("name,env_key", CLOUD_PROVIDERS.items())
@pytest.mark.parametrize("configured", [False, True])
def test_cloud_provider_status_never_carries_install_gate(
    monkeypatch, name: str, env_key: str, configured: bool
) -> None:
    if configured:
        monkeypatch.setenv(env_key, "test-key")
    else:
        monkeypatch.delenv(env_key, raising=False)

    status = build_provider_status([_provider(name, env_key)])[name]

    assert set(status) == {
        "provider",
        "configured",
        "generate_ready",
        "cogitate_ready",
        "issues",
    }
    assert status["issues"] in ([], [f"{env_key} not set"])
    assert not any(
        "runtime" in issue.lower()
        or "install" in issue.lower()
        for issue in status["issues"]
    )


def test_local_provider_status_accepts_pre_resolved_row(monkeypatch) -> None:
    sentinel = {
        "configured": True,
        "selected": True,
        "generate_ready": False,
        "cogitate_ready": True,
        "issues": ["provider_unavailable"],
    }
    monkeypatch.setattr(
        "solstone.think.providers.state.local_status_dict",
        lambda: (_ for _ in ()).throw(AssertionError("local status probed")),
    )

    status = build_provider_status(
        [{"name": "local", "label": "Local", "env_key": ""}],
        local_status=sentinel,
    )

    assert status["local"] == sentinel


def test_inert_upgrade_path_for_stale_bundled_config(
    tmp_path, monkeypatch, caplog
) -> None:
    config_path = tmp_path / "config" / "journal.json"
    config_path.parent.mkdir(parents=True)
    config = {
        "env": {},
        "providers": {
            "bundled": {
                "anthropic": {"install_state": "failed", "install_error": "old"},
                "openai": {"install_state": "idle"},
            }
        },
    }
    seed_journal_config(config, tmp_path)
    before_hash = hashlib.sha256(config_path.read_bytes()).hexdigest()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setenv("ANTHROPIC_API_KEY", "test-anthropic")
    monkeypatch.setenv("OPENAI_API_KEY", "test-openai")

    def fail_mutate(*_args, **_kwargs) -> None:
        raise AssertionError("cloud provider status must not write config")

    monkeypatch.setattr(
        "solstone.think.journal_config.mutate_journal_config", fail_mutate
    )
    caplog.set_level(logging.WARNING)

    status = build_provider_status(
        [
            _provider("anthropic", "ANTHROPIC_API_KEY"),
            _provider("openai", "OPENAI_API_KEY"),
        ],
    )

    assert status["anthropic"]["cogitate_ready"] is True
    assert status["openai"]["cogitate_ready"] is True
    assert hashlib.sha256(config_path.read_bytes()).hexdigest() == before_hash
    assert not [
        record
        for record in caplog.records
        if record.levelno >= logging.WARNING
        and any(
            word in record.getMessage().lower()
            for word in ("migrate", "cleanup", "stale")
        )
    ]
