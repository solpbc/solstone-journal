# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json

import pytest

from solstone.think.providers import bundled


@pytest.mark.integration
def test_legacy_bundled_provider_records_migrate_idempotently(
    tmp_path,
    monkeypatch,
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    config_path = tmp_path / "config" / "journal.json"
    config_path.parent.mkdir(parents=True)
    config_path.write_text(
        json.dumps(
            {
                "env": {"ANTHROPIC_API_KEY": "test-key"},
                "providers": {
                    "auth": {"anthropic": "api_key", "openai": "api_key"},
                    "key_validation": {"anthropic": {"valid": True}},
                    "bundled": {
                        "anthropic": {
                            "state": "valid",
                            "last_transition_at": "2026-05-20T00:00:00+00:00",
                            "sdk_spec": bundled.PINS["anthropic"]["sdk_spec"],
                            "binary_path": "/tmp/solstone-test/anthropic",
                            "install_error": None,
                        },
                        "openai": {
                            "state": "install-failed",
                            "last_transition_at": "2026-05-20T00:00:00+00:00",
                            "sdk_spec": bundled.PINS["openai"]["sdk_spec"],
                            "install_error": "network: timeout",
                        },
                        "openhands": {
                            "state": "disabled",
                            "last_transition_at": "2026-05-20T00:00:00+00:00",
                            "sdk_specs": ["openhands-sdk==1.23.*"],
                            "runtime": "python",
                            "install_error": None,
                        },
                    },
                },
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )

    first = {
        name: bundled.get_provider_state(name)
        for name in ("anthropic", "openai", "openhands")
    }

    assert first["anthropic"]["install_state"] == "installed"
    assert first["anthropic"]["key_status"] == "valid"
    assert first["openai"]["install_state"] == "failed"
    assert first["openai"]["key_status"] == "key-needed"
    assert first["openhands"]["install_state"] == "idle"
    assert first["openhands"]["key_status"] == "not-applicable"
    persisted = json.loads(config_path.read_text(encoding="utf-8"))
    for record in persisted["providers"]["bundled"].values():
        assert "state" not in record
        assert "install_state" in record
        assert "key_state" in record
        assert "disabled" in record

    before_second_read = config_path.read_text(encoding="utf-8")
    second = {
        name: bundled.get_provider_state(name)
        for name in ("anthropic", "openai", "openhands")
    }

    assert second == first
    assert config_path.read_text(encoding="utf-8") == before_second_read
