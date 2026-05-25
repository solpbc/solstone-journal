# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import threading
import time
from typing import get_args

import pytest
from typer.testing import CliRunner

from solstone.convey import create_app
from solstone.think.call import call_app
from solstone.think.providers import bundled
from solstone.think.providers.install_state import InstallState
from tests.bundled_provider_fixtures import (
    BUNDLED_STATES,
    BundledCase,
    bundled_provider_config,
)

runner = CliRunner()
CANONICAL_INSTALL_STATES = set(get_args(InstallState))


@pytest.fixture
def settings_client(journal_copy, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal_copy))
    monkeypatch.setenv("SOL_SKIP_SUPERVISOR_CHECK", "1")
    app = create_app(str(journal_copy))
    app.config["TESTING"] = True
    return app.test_client(), journal_copy


def _write_config(journal, config: dict) -> None:
    config_path = journal / "config" / "journal.json"
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")


@pytest.mark.parametrize("provider", ["anthropic", "openai"])
@pytest.mark.parametrize("case", BUNDLED_STATES)
def test_cli_status_and_http_bundled_contract_match(settings_client, provider, case):
    client, journal = settings_client
    _write_config(journal, bundled_provider_config(provider, case))

    cli_result = runner.invoke(
        call_app,
        ["settings", "providers", "status", provider, "--json"],
    )
    http_response = client.get("/app/settings/api/providers/bundled")

    assert cli_result.exit_code == 0
    assert http_response.status_code == 200
    assert json.loads(cli_result.output) == http_response.get_json()[provider]


def test_get_providers_includes_bundled(settings_client):
    client, journal = settings_client
    _write_config(
        journal,
        bundled_provider_config(
            "anthropic", BundledCase("installed", "valid", False, True, False)
        ),
    )

    response = client.get("/app/settings/api/providers")

    assert response.status_code == 200
    payload = response.get_json()
    assert set(payload["bundled"]) == {"anthropic", "openai", "openhands"}


def test_http_install_state_contract_covers_status_endpoints(settings_client):
    client, _journal = settings_client

    bundled_response = client.get("/app/settings/api/providers/bundled")
    local_response = client.get("/app/settings/api/local/bootstrap/status")
    mlx_response = client.get("/app/settings/api/mlx/bootstrap/status")

    assert bundled_response.status_code == 200
    bundled_payload = bundled_response.get_json()
    assert set(bundled_payload) == {"anthropic", "openai", "openhands"}
    for provider_payload in bundled_payload.values():
        assert provider_payload["install_state"] in CANONICAL_INSTALL_STATES

    assert local_response.status_code == 200
    assert local_response.get_json()["install_state"] in CANONICAL_INSTALL_STATES

    assert mlx_response.status_code == 200
    assert mlx_response.get_json()["install_state"] in CANONICAL_INSTALL_STATES


def test_get_local_provider_status_shape(settings_client):
    client, _journal = settings_client

    response = client.get("/app/settings/api/providers/local/status")

    assert response.status_code == 200
    payload = response.get_json()
    assert set(payload) == {
        "configured",
        "generate_ready",
        "cogitate_ready",
        "cogitate_cli",
        "cogitate_cli_found",
        "issues",
    }
    assert payload["cogitate_cli"] == "llama-server"


@pytest.mark.parametrize(
    ("endpoint", "function_name"),
    [
        ("install", "install_provider"),
        ("uninstall", "uninstall_provider"),
        ("disable", "disable_provider"),
        ("enable", "enable_provider"),
        ("validate-key", "validate_key"),
    ],
)
def test_bundled_action_routes(settings_client, monkeypatch, endpoint, function_name):
    client, _journal = settings_client
    payload = {"name": "openai", "install_state": "installed"}
    monkeypatch.setattr(bundled, function_name, lambda name: payload)

    response = client.post(f"/app/settings/api/providers/openai/{endpoint}")

    assert response.status_code == 200
    assert response.get_json() == payload


def test_install_route_does_not_block_on_slow_thread(settings_client, monkeypatch):
    client, journal = settings_client
    _write_config(
        journal,
        bundled_provider_config(
            "anthropic", BundledCase("idle", "key-needed", False, False, False)
        ),
    )
    release = threading.Event()
    monkeypatch.setattr(bundled, "_install_thread", lambda _name: release.wait(2))

    start = time.monotonic()
    try:
        response = client.post("/app/settings/api/providers/anthropic/install")
        elapsed = time.monotonic() - start
    finally:
        release.set()
        thread = bundled._INSTALL_THREADS.pop("anthropic", None)
        bundled._OBSERVED_PHASES.pop("anthropic", None)
        if thread is not None:
            thread.join(timeout=1)

    assert elapsed < 0.5
    assert response.status_code == 200
    assert response.get_json()["install_state"] == "installing"


def test_invalid_bundled_provider_route_returns_400(settings_client):
    client, _journal = settings_client

    response = client.post("/app/settings/api/providers/google/install")

    assert response.status_code == 400
    assert response.get_json()["reason_code"] == "invalid_config_value"


def test_install_during_install_route_returns_409(settings_client):
    client, journal = settings_client
    config = bundled_provider_config(
        "anthropic", BundledCase("installing", "key-needed", False, False, False)
    )
    record = config["providers"]["bundled"]["anthropic"]
    timestamp = bundled._now_iso()
    record["last_transition_at"] = timestamp
    record["last_progress_at"] = timestamp
    _write_config(journal, config)

    response = client.post("/app/settings/api/providers/anthropic/install")

    assert response.status_code == 409
    assert response.get_json() == {
        "error": "install in flight",
        "install_state": "installing",
    }
