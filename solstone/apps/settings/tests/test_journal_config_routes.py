# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from pathlib import Path
from typing import Any

from solstone.convey import create_app
from solstone.convey.reasons import INVALID_CONFIG_VALUE
from solstone.think.utils import get_config


def _base_config() -> dict:
    return {
        "setup": {"completed_at": 1700000000000},
    }


def _client(journal_path: Path):
    app = create_app(str(journal_path))
    app.config["TESTING"] = True
    return app.test_client()


def test_journal_name_round_trips_through_settings_config(settings_env):
    journal_path, _config = settings_env(_base_config())
    client = _client(journal_path)

    response = client.put(
        "/app/settings/api/config",
        json={"section": "journal", "data": {"name": "Home Base"}},
    )

    assert response.status_code == 200
    assert response.get_json()["config"]["journal"]["name"] == "Home Base"

    get_response = client.get("/app/settings/api/config")
    assert get_response.status_code == 200
    assert get_response.get_json()["journal"]["name"] == "Home Base"
    assert get_config()["journal"]["name"] == "Home Base"

    legacy_response = client.put(
        "/app/settings/api/config",
        json={"section": "journal", "key": "name", "value": "Legacy"},
    )

    assert legacy_response.status_code == 200
    assert get_config()["journal"]["name"] == "Legacy"


def test_journal_config_filters_unknown_fields(settings_env):
    journal_path, _config = settings_env(_base_config())
    client = _client(journal_path)

    response = client.put(
        "/app/settings/api/config",
        json={"section": "journal", "data": {"name": "Named", "bogus": "Y"}},
    )

    assert response.status_code == 200
    journal_config = response.get_json()["config"]["journal"]
    assert journal_config["name"] == "Named"
    assert "bogus" not in journal_config

    persisted_journal_config = get_config()["journal"]
    assert persisted_journal_config["name"] == "Named"
    assert "bogus" not in persisted_journal_config


def test_journal_name_rejects_empty_strings_but_allows_null(settings_env):
    journal_path, _config = settings_env(_base_config())
    client = _client(journal_path)

    empty_response = client.put(
        "/app/settings/api/config",
        json={"section": "journal", "data": {"name": ""}},
    )

    assert empty_response.status_code == 400
    empty_payload = empty_response.get_json()
    assert empty_payload["reason_code"] == INVALID_CONFIG_VALUE.code
    assert empty_payload["detail"] == "Journal name cannot be empty"

    whitespace_response = client.put(
        "/app/settings/api/config",
        json={"section": "journal", "data": {"name": "   "}},
    )

    assert whitespace_response.status_code == 400
    whitespace_payload = whitespace_response.get_json()
    assert whitespace_payload["reason_code"] == INVALID_CONFIG_VALUE.code
    assert whitespace_payload["detail"] == "Journal name cannot be empty"

    null_response = client.put(
        "/app/settings/api/config",
        json={"section": "journal", "data": {"name": None}},
    )

    assert null_response.status_code == 200
    assert get_config()["journal"]["name"] is None


def test_journal_config_get_without_journal_section(settings_env):
    journal_path, _config = settings_env(_base_config())
    client = _client(journal_path)

    response = client.get("/app/settings/api/config")

    assert response.status_code == 200
    assert "journal" not in response.get_json()


def _payload_keys_and_strings(value: Any):
    if isinstance(value, dict):
        for key, item in value.items():
            yield str(key)
            yield from _payload_keys_and_strings(item)
    elif isinstance(value, list):
        for item in value:
            yield from _payload_keys_and_strings(item)
    elif isinstance(value, str):
        yield value


def test_settings_config_projection_drops_thinking_provider_secrets(
    settings_env,
    monkeypatch,
):
    endpoint_credential = "SETTINGS-ENDPOINT-CREDENTIAL-SENTINEL"
    config = {
        **_base_config(),
        "env": {
            "GOOGLE_API_KEY": "google-secret",
            "OPENAI_API_KEY": "openai-secret",
            "ANTHROPIC_API_KEY": "anthropic-secret",
            "TEST_LEAK_SENTINEL": "arbitrary-secret",
            "REVAI_ACCESS_TOKEN": "revai-secret",
        },
        "providers": {
            "active": {
                "provider": "anthropic",
                "model": "claude-settings-leak-model",
            },
            "local": {
                "endpoint_url": "http://settings-leak.example/v1",
                "served_model_id": "settings-local-model",
                "credential": endpoint_credential,
            },
        },
    }
    journal_path, _config = settings_env(config)
    monkeypatch.setenv("REVAI_ACCESS_TOKEN", "runtime-revai")
    monkeypatch.delenv("PLAUD_ACCESS_TOKEN", raising=False)
    client = _client(journal_path)

    response = client.get("/app/settings/api/config")

    assert response.status_code == 200
    payload = response.get_json()
    serialized_projection = "\n".join(_payload_keys_and_strings(payload))
    for forbidden in (
        "GOOGLE_API_KEY",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "TEST_LEAK_SENTINEL",
        "anthropic",
        "claude-settings-leak-model",
        "endpoint_url",
        endpoint_credential,
    ):
        assert forbidden not in serialized_projection
    assert set(payload["env"]) == {"REVAI_ACCESS_TOKEN", "PLAUD_ACCESS_TOKEN"}
    assert payload["env"] == {
        "REVAI_ACCESS_TOKEN": True,
        "PLAUD_ACCESS_TOKEN": False,
    }
    assert set(payload["runtime_env"]) == {"REVAI_ACCESS_TOKEN", "PLAUD_ACCESS_TOKEN"}
    assert payload["runtime_env"] == {
        "REVAI_ACCESS_TOKEN": True,
        "PLAUD_ACCESS_TOKEN": False,
    }


def test_settings_config_projection_treats_malformed_env_as_empty(settings_env):
    journal_path, _config = settings_env({**_base_config(), "env": "not-a-map"})
    client = _client(journal_path)

    response = client.get("/app/settings/api/config")

    assert response.status_code == 200
    assert response.get_json()["env"] == {
        "REVAI_ACCESS_TOKEN": False,
        "PLAUD_ACCESS_TOKEN": False,
    }
