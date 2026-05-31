# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
from typing import get_args

import pytest

from solstone.apps.settings import routes
from solstone.apps.settings.local_bootstrap import LOCAL_MODEL_SPECS
from solstone.convey import create_app
from solstone.think.providers.install_state import InstallState

INSTALL_STATUS_FIELDS = {
    "name",
    "install_state",
    "last_transition_at",
    "last_progress_at",
    "progress_bytes_received",
    "progress_bytes_total",
    "install_error",
}
CANONICAL_INSTALL_STATES = set(get_args(InstallState))
REMOVED_PROVIDER = "mlx"


@pytest.fixture
def settings_client(settings_env):
    journal_path, config = settings_env()
    config["setup"] = {"completed_at": "2026-05-23T00:00:00Z"}
    config.setdefault("convey", {})["trust_localhost"] = True
    (journal_path / "config" / "journal.json").write_text(
        json.dumps(config, indent=2) + "\n",
        encoding="utf-8",
    )
    app = create_app(str(journal_path))
    app.config["TESTING"] = True
    return app.test_client()


def _assert_install_status(payload: dict) -> None:
    assert INSTALL_STATUS_FIELDS <= set(payload)
    assert payload["install_state"] in CANONICAL_INSTALL_STATES


def test_get_providers_includes_local_install_state(settings_client):
    response = settings_client.get("/app/settings/api/providers")

    assert response.status_code == 200
    payload = response.get_json()
    assert "bundled" not in payload
    assert isinstance(payload["local"], dict)
    assert REMOVED_PROVIDER not in payload
    _assert_install_status(payload["local"])


def test_providers_payload_omits_bundled_block(settings_client):
    response = settings_client.get("/app/settings/api/providers")

    assert response.status_code == 200
    payload = response.get_json()
    assert "bundled" not in payload
    provider_status = payload["provider_status"]
    for name in ("google", "openai", "anthropic"):
        assert set(provider_status[name]) == {
            "provider",
            "configured",
            "generate_ready",
            "cogitate_ready",
            "issues",
        }
    assert provider_status["local"]["cogitate_cli"] == "llama-server"
    assert REMOVED_PROVIDER not in payload
    _assert_install_status(payload["local"])


def test_providers_payload_omits_auth(settings_client):
    response = settings_client.get("/app/settings/api/providers")

    assert response.status_code == 200
    payload = response.get_json()
    assert "auth" not in payload


def test_get_providers_uses_requested_local_model(settings_client, monkeypatch):
    model_id = next(iter(LOCAL_MODEL_SPECS))
    requested_models: list[str] = []

    def fake_get_state(model: str) -> dict:
        requested_models.append(model)
        return {
            "name": model,
            "install_state": "idle",
            "last_transition_at": None,
            "last_progress_at": None,
            "progress_bytes_received": None,
            "progress_bytes_total": None,
            "install_error": None,
        }

    monkeypatch.setattr(routes.local_bootstrap, "get_state", fake_get_state)

    response = settings_client.get(
        "/app/settings/api/providers",
        query_string={"local_model": model_id},
    )

    assert response.status_code == 200
    payload = response.get_json()
    _assert_install_status(payload["local"])
    assert requested_models == [model_id]
    assert payload["local"]["name"] == model_id
