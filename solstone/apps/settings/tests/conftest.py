# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Self-contained fixtures for retained Settings maintenance tests."""

from __future__ import annotations

import json

import pytest


@pytest.fixture(autouse=True)
def _skip_supervisor_check(monkeypatch):
    """Allow app CLI tests to run without a live solstone supervisor."""
    monkeypatch.setenv("SOL_SKIP_SUPERVISOR_CHECK", "1")


@pytest.fixture
def settings_env(tmp_path, monkeypatch):
    """Create a temporary journal with Settings configuration."""

    def _create(config: dict | None = None):
        config_dir = tmp_path / "config"
        config_dir.mkdir(parents=True, exist_ok=True)
        config_path = config_dir / "journal.json"
        if config is None:
            config = {
                "identity": {
                    "name": "Test User",
                    "preferred": "Tester",
                    "bio": "A test user",
                    "pronouns": {
                        "subject": "they",
                        "object": "them",
                        "possessive": "their",
                        "reflexive": "themselves",
                    },
                    "aliases": ["tester"],
                    "email_addresses": ["test@example.com"],
                    "timezone": "UTC",
                },
                "env": {
                    "GOOGLE_API_KEY": "test-google-key",
                    "OPENAI_API_KEY": "test-openai-key",
                },
                "providers": {
                    "active": {
                        "provider": "google",
                        "model": "gemini-3.5-flash",
                    },
                    "key_validation": {},
                },
                "transcribe": {
                    "backend": "parakeet",
                    "parakeet": {
                        "model_version": "v3",
                        "device": "auto",
                        "timeout_sec": 120.0,
                    },
                },
                "observe": {"tmux": {"enabled": True, "capture_interval": 5}},
            }
        config_path.write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        return tmp_path, config

    return _create
