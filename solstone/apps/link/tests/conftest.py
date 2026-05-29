# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Self-contained fixtures for link app tests."""

from __future__ import annotations

import json

import pytest


@pytest.fixture
def link_env(tmp_path, monkeypatch):
    """Create a temporary journal for link app testing."""

    def _create(*, posture: str | None = None, totp_secret: str | None = None):
        journal = tmp_path / "journal"
        journal.mkdir(exist_ok=True)

        config_dir = journal / "config"
        config_dir.mkdir(parents=True, exist_ok=True)
        config_file = config_dir / "journal.json"
        config = {
            "convey": {"trust_localhost": True},
            "setup": {"completed_at": 1700000000000},
        }
        if posture is not None:
            config["link"] = {"posture": posture}
        config_file.write_text(
            json.dumps(config, indent=2),
        )

        monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
        if totp_secret is not None:
            from solstone.think.link.paths import save_totp_secret

            save_totp_secret(totp_secret)

        from solstone.convey import create_app

        app = create_app(journal=str(journal))
        client = app.test_client()

        class Env:
            def __init__(self):
                self.journal = journal
                self.client = client
                self.app = app

        return Env()

    return _create
