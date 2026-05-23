# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Fixture builders for bundled provider state tests."""

from __future__ import annotations

from copy import deepcopy
from typing import NamedTuple

from solstone.think.providers.bundled import PINS


class BundledCase(NamedTuple):
    install_state: str
    key_status: str
    disabled: bool
    has_binary: bool
    has_install_error: bool


BUNDLED_STATES = (
    BundledCase("idle", "key-needed", False, False, False),
    BundledCase("installed", "key-needed", False, True, False),
    BundledCase("installed", "validating", False, True, False),
    BundledCase("installed", "valid", False, True, False),
    BundledCase("installed", "invalid", False, True, False),
    BundledCase("installing", "key-needed", False, False, False),
    BundledCase("failed", "key-needed", False, False, True),
    BundledCase("installed", "valid", True, True, False),
)

ENV_KEYS = {
    "anthropic": "ANTHROPIC_API_KEY",
    "openai": "OPENAI_API_KEY",
}


def bundled_provider_config(provider: str, case: BundledCase) -> dict:
    """Return a complete journal config for a bundled provider state."""

    pin = PINS[provider]
    transition_at = "2026-05-20T00:00:00+00:00"
    record = {
        "install_state": case.install_state,
        "last_transition_at": transition_at,
        "last_progress_at": (
            transition_at
            if case.install_state
            in {"resolving", "downloading", "verifying", "installing"}
            else None
        ),
        "progress_bytes_received": None,
        "progress_bytes_total": None,
        "key_state": case.key_status,
        "disabled": case.disabled,
        "binary_path": f"/tmp/solstone-test/{provider}" if case.has_binary else None,
        "sdk_spec": pin["sdk_spec"],
        "install_error": "network: timeout" if case.has_install_error else None,
    }
    if provider == "openai":
        record["codex_version"] = pin["codex_version"]
        artifact = next(iter(pin["codex_artifacts"].values()))
        record["codex_artifact"] = artifact["filename"]
        record["codex_sha256"] = artifact["sha256"]
    config = {
        "identity": {"name": "Test User"},
        "setup": {"completed_at": 1},
        "convey": {"trust_localhost": True},
        "env": {},
        "providers": {
            "auth": {
                "anthropic": "api_key",
                "openai": "api_key",
            },
            "key_validation": {},
            "bundled": {provider: record},
        },
    }
    if case.key_status in {"validating", "valid", "invalid"}:
        config["env"][ENV_KEYS[provider]] = "test-key"
    if case.key_status == "valid":
        config["providers"]["key_validation"][provider] = {
            "valid": True,
            "timestamp": "2026-05-20T00:00:00+00:00",
        }
    elif case.key_status == "invalid":
        config["providers"]["key_validation"][provider] = {
            "valid": False,
            "error": "bad key",
            "timestamp": "2026-05-20T00:00:00+00:00",
        }
    return deepcopy(config)
