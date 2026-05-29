# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import os
from unittest.mock import Mock, patch

import pytest

from solstone.convey import create_app
from solstone.think.providers import anthropic, google, openai, validate_key


@pytest.fixture
def settings_client(journal_copy):
    app = create_app(str(journal_copy))
    app.config["TESTING"] = True
    return app.test_client(), journal_copy


@pytest.fixture(autouse=True)
def reset_google_backend_cache():
    original = google._detected_backend
    google._detected_backend = None
    yield
    google._detected_backend = original


def test_validate_key_anthropic_success():
    client = Mock()
    client.models.list.return_value = [Mock()]

    with patch("anthropic.Anthropic", return_value=client) as mock_cls:
        result = anthropic.validate_key("test-key")

    assert result == {"valid": True}
    mock_cls.assert_called_once_with(api_key="test-key", timeout=10)


def test_validate_key_anthropic_auth_error():
    client = Mock()
    client.models.list.side_effect = Exception("invalid x-api-key")

    with patch("anthropic.Anthropic", return_value=client):
        result = anthropic.validate_key("bad-key")

    assert result["valid"] is False
    assert "invalid x-api-key" in result["error"]


def test_validate_key_openai_success():
    client = Mock()
    client.models.list.return_value = [Mock()]

    with patch("openai.OpenAI", return_value=client) as mock_cls:
        result = openai.validate_key("test-key")

    assert result == {"valid": True}
    mock_cls.assert_called_once_with(api_key="test-key", timeout=10)


def test_validate_key_openai_auth_error():
    client = Mock()
    client.models.list.side_effect = Exception("Incorrect API key")

    with patch("openai.OpenAI", return_value=client):
        result = openai.validate_key("bad-key")

    assert result["valid"] is False
    assert "Incorrect API key" in result["error"]


def test_validate_key_google_success():
    client = Mock()
    client.models.list.return_value = [Mock()]

    with (
        patch.object(google.genai, "Client", return_value=client) as mock_cls,
        patch.object(google, "_probe_backend", return_value="aistudio"),
    ):
        result = google.validate_key("test-key")

    assert result == {"valid": True, "backend": "aistudio"}
    mock_cls.assert_called_once()
    assert mock_cls.call_args.kwargs["api_key"] == "test-key"


def test_validate_key_google_auth_error():
    client = Mock()
    client.models.list.side_effect = Exception("API key not valid")

    with (
        patch.object(google.genai, "Client", return_value=client),
        patch.object(google, "_probe_backend", return_value="aistudio"),
    ):
        result = google.validate_key("bad-key")

    assert result["valid"] is False
    assert "API key not valid" in result["error"]


def test_validate_key_google_returns_backend_aistudio():
    """validate_key returns backend field when successful."""
    client = Mock()
    client.models.list.return_value = [Mock()]

    with (
        patch.object(google.genai, "Client", return_value=client),
        patch.object(google, "_probe_backend", return_value="aistudio"),
    ):
        result = google.validate_key("test-key")

    assert result == {"valid": True, "backend": "aistudio"}


def test_validate_key_google_returns_backend_vertex():
    """validate_key returns vertex backend and uses vertexai=True."""
    client = Mock()
    client.models.list.return_value = [Mock()]

    with (
        patch.object(google.genai, "Client", return_value=client) as mock_cls,
        patch.object(google, "_probe_backend", return_value="vertex"),
    ):
        result = google.validate_key("test-key")

    assert result == {"valid": True, "backend": "vertex"}
    assert mock_cls.call_args.kwargs["vertexai"] is True
    assert mock_cls.call_args.kwargs["api_key"] == "test-key"


def test_validate_vertex_credentials(tmp_path):
    """validate_vertex_credentials creates SA-authenticated client."""
    import json

    sa_file = tmp_path / "sa.json"
    sa_file.write_text(
        json.dumps(
            {
                "type": "service_account",
                "project_id": "test-project",
                "client_email": "test@project.iam.gserviceaccount.com",
                "private_key": "fake",
            }
        )
    )

    client = Mock()
    client.models.list.return_value = [Mock()]

    mock_creds = Mock()
    mock_creds.service_account_email = "test@project.iam.gserviceaccount.com"

    with (
        patch.object(google.genai, "Client", return_value=client) as mock_cls,
        patch(
            "google.oauth2.service_account.Credentials.from_service_account_file",
            return_value=mock_creds,
        ),
    ):
        result = google.validate_vertex_credentials(str(sa_file))

    assert result == {
        "valid": True,
        "email": "test@project.iam.gserviceaccount.com",
    }
    assert mock_cls.call_args.kwargs["vertexai"] is True
    assert mock_cls.call_args.kwargs["credentials"] is mock_creds
    assert mock_cls.call_args.kwargs["project"] == "test-project"
    assert "api_key" not in mock_cls.call_args.kwargs


def test_probe_backend_aistudio():
    """HTTP 200 from AI Studio endpoint -> aistudio."""
    mock_resp = Mock()
    mock_resp.status_code = 200
    with patch("httpx.get", return_value=mock_resp):
        result = google._probe_backend("test-key")
    assert result == "aistudio"


def test_probe_backend_vertex():
    """Non-200 from AI Studio endpoint -> vertex."""
    mock_resp = Mock()
    mock_resp.status_code = 403
    with patch("httpx.get", return_value=mock_resp):
        result = google._probe_backend("test-key")
    assert result == "vertex"


def test_probe_backend_error_defaults_aistudio():
    """Network error defaults to aistudio."""
    with patch("httpx.get", side_effect=Exception("timeout")):
        result = google._probe_backend("test-key")
    assert result == "aistudio"


def test_detect_backend_caches():
    """Second call returns cached result without probing."""
    import solstone.think.providers.google as gmod

    original = gmod._detected_backend
    try:
        gmod._detected_backend = None
        mock_resp = Mock()
        mock_resp.status_code = 403
        with patch("httpx.get", return_value=mock_resp) as mock_get:
            r1 = gmod._detect_backend("key")
            r2 = gmod._detect_backend("key")
        assert r1 == "vertex"
        assert r2 == "vertex"
        assert mock_get.call_count == 1
    finally:
        gmod._detected_backend = original


def test_get_effective_backend_config_override():
    """Config override skips detection."""
    import solstone.think.providers.google as gmod

    original = gmod._detected_backend
    try:
        gmod._detected_backend = None
        config = {"providers": {"google_backend": "vertex"}}
        with patch("solstone.think.utils.get_config", return_value=config):
            result = gmod._get_effective_backend("key")
        assert result == "vertex"
        assert gmod._detected_backend is None
    finally:
        gmod._detected_backend = original


def test_validate_key_dispatcher_success():
    with patch(
        "solstone.think.providers.google.validate_key", return_value={"valid": True}
    ):
        result = validate_key("google", "test-key")

    assert result == {"valid": True}


def test_validate_key_dispatcher_unknown_provider():
    with pytest.raises(ValueError, match="Unknown provider"):
        validate_key("bogus", "test-key")


def test_validate_key_timeout():
    """Validate that timeout exceptions are caught and reported."""
    client = Mock()
    client.models.list.side_effect = TimeoutError("Connection timed out")

    with patch("openai.OpenAI", return_value=client):
        result = openai.validate_key("test-key")

    assert result["valid"] is False
    assert "timed out" in result["error"]


def test_update_config_saves_key_validation(settings_client):
    client, journal = settings_client

    with patch(
        "solstone.think.providers.validate_key",
        return_value={"valid": False, "error": "bad key"},
    ):
        response = client.put(
            "/app/settings/api/config",
            json={"section": "env", "data": {"GOOGLE_API_KEY": "bad-key"}},
        )

    assert response.status_code == 200
    payload = response.get_json()
    assert payload["success"] is True
    assert payload["key_validation"]["google"]["valid"] is False
    assert payload["key_validation"]["google"]["error"] == "bad key"
    assert "timestamp" in payload["key_validation"]["google"]

    config = json.loads((journal / "config" / "journal.json").read_text())
    assert config["providers"]["key_validation"]["google"]["valid"] is False


def test_update_config_clears_key_validation(settings_client):
    client, journal = settings_client
    config_path = journal / "config" / "journal.json"
    config = json.loads(config_path.read_text())
    config.setdefault("env", {})["GOOGLE_API_KEY"] = "existing-key"
    config["providers"]["key_validation"] = {
        "google": {"valid": True, "timestamp": "2026-01-01T00:00:00+00:00"}
    }
    config_path.write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")

    response = client.put(
        "/app/settings/api/config",
        json={"section": "env", "data": {"GOOGLE_API_KEY": ""}},
    )

    assert response.status_code == 200
    payload = response.get_json()
    assert payload["success"] is True
    assert "google" not in payload["key_validation"]

    saved = json.loads(config_path.read_text())
    assert "google" not in saved["providers"]["key_validation"]


def test_update_config_env_mirrors_os_environ(settings_client, monkeypatch):
    """The HTTP env-section save path must mirror into os.environ in-process,
    matching the CLI pattern (apps/settings/call.py keys_set/keys_clear).
    Without this, /api/providers reports `configured: false` until restart."""
    client, _ = settings_client
    monkeypatch.delenv("GOOGLE_API_KEY", raising=False)

    with patch(
        "solstone.think.providers.validate_key",
        return_value={"valid": True},
    ):
        response = client.put(
            "/app/settings/api/config",
            json={"section": "env", "data": {"GOOGLE_API_KEY": "live-key"}},
        )

    assert response.status_code == 200
    assert os.environ.get("GOOGLE_API_KEY") == "live-key"

    response = client.put(
        "/app/settings/api/config",
        json={"section": "env", "data": {"GOOGLE_API_KEY": ""}},
    )

    assert response.status_code == 200
    assert "GOOGLE_API_KEY" not in os.environ


def test_get_providers_includes_key_validation(settings_client):
    client, journal = settings_client
    config_path = journal / "config" / "journal.json"
    config = json.loads(config_path.read_text())
    config.setdefault("providers", {})["key_validation"] = {
        "openai": {"valid": True, "timestamp": "2026-01-01T00:00:00+00:00"}
    }
    config_path.write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")

    response = client.get("/app/settings/api/providers")

    assert response.status_code == 200
    payload = response.get_json()
    assert payload["key_validation"]["openai"]["valid"] is True


def test_validate_all_keys_endpoint(settings_client):
    client, journal = settings_client
    config_path = journal / "config" / "journal.json"
    config = json.loads(config_path.read_text())
    config.setdefault("env", {})["GOOGLE_API_KEY"] = "google-key"
    config["env"]["OPENAI_API_KEY"] = "openai-key"
    config_path.write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")

    def fake_validate(provider: str, api_key: str) -> dict:
        return {
            "valid": provider == "google",
            "error": "" if provider == "google" else "bad key",
        }

    with patch("solstone.think.providers.validate_key", side_effect=fake_validate):
        response = client.post("/app/settings/api/validate-keys")

    assert response.status_code == 200
    payload = response.get_json()
    assert payload["success"] is True
    assert payload["key_validation"]["google"]["valid"] is True
    assert payload["key_validation"]["openai"]["valid"] is False
    assert "timestamp" in payload["key_validation"]["google"]

    saved = json.loads(config_path.read_text())
    assert set(saved["providers"]["key_validation"]) == {"google", "openai"}


def test_providers_google_backend_roundtrip(settings_client):
    """PUT/GET google_backend."""
    client, journal = settings_client

    response = client.put(
        "/app/settings/api/providers",
        json={"google_backend": "vertex"},
    )
    assert response.status_code == 200
    payload = response.get_json()
    assert payload["google_backend"] == "vertex"

    # Verify persisted
    config = json.loads((journal / "config" / "journal.json").read_text())
    assert config["providers"]["google_backend"] == "vertex"

    # GET returns the same
    response = client.get("/app/settings/api/providers")
    payload = response.get_json()
    assert payload["google_backend"] == "vertex"


def test_providers_vertex_credentials_roundtrip(settings_client):
    """PUT/GET vertex_credentials saves file and returns email."""
    client, journal = settings_client

    sa_json = json.dumps(
        {
            "type": "service_account",
            "project_id": "test-project",
            "client_email": "test@test-project.iam.gserviceaccount.com",
            "private_key": "-----BEGIN RSA PRIVATE KEY-----\nfake\n-----END RSA PRIVATE KEY-----\n",
            "client_id": "123",
            "token_uri": "https://oauth2.googleapis.com/token",
        }
    )

    # Mock validation (don't actually call Google API)
    with patch(
        "solstone.apps.settings.routes.validate_vertex_credentials",
        return_value={
            "valid": True,
            "email": "test@test-project.iam.gserviceaccount.com",
        },
    ):
        response = client.put(
            "/app/settings/api/providers",
            json={"vertex_credentials": sa_json},
        )
    assert response.status_code == 200
    payload = response.get_json()
    assert payload["vertex_credentials_configured"] is True
    assert (
        payload["vertex_credentials_email"]
        == "test@test-project.iam.gserviceaccount.com"
    )

    # Verify file saved with correct permissions
    creds_file = journal / ".config" / "vertex-credentials.json"
    assert creds_file.exists()
    assert oct(creds_file.stat().st_mode & 0o777) == "0o600"

    # Verify config stores path
    config = json.loads((journal / "config" / "journal.json").read_text())
    assert config["providers"]["vertex_credentials"] == str(creds_file)

    # GET returns status without secrets
    response = client.get("/app/settings/api/providers")
    payload = response.get_json()
    assert payload["vertex_credentials_configured"] is True
    assert (
        payload["vertex_credentials_email"]
        == "test@test-project.iam.gserviceaccount.com"
    )
    assert "private_key" not in json.dumps(payload)

    # Remove credentials
    response = client.put(
        "/app/settings/api/providers",
        json={"vertex_credentials": ""},
    )
    assert response.status_code == 200
    payload = response.get_json()
    assert payload["vertex_credentials_configured"] is False
    assert payload["vertex_credentials_email"] == ""
    assert not creds_file.exists()


def test_providers_vertex_credentials_invalid_json(settings_client):
    """Invalid JSON in vertex_credentials is rejected."""
    client, _journal = settings_client

    response = client.put(
        "/app/settings/api/providers",
        json={"vertex_credentials": "not json"},
    )
    assert response.status_code == 400
    data = response.get_json()
    assert data["reason_code"] == "invalid_json_request"
    assert "Invalid JSON" in data["detail"]


def test_providers_vertex_credentials_missing_fields(settings_client):
    """SA JSON missing required fields is rejected."""
    client, _journal = settings_client

    response = client.put(
        "/app/settings/api/providers",
        json={"vertex_credentials": json.dumps({"type": "service_account"})},
    )
    assert response.status_code == 400
    data = response.get_json()
    assert data["reason_code"] == "missing_required_field"
    assert "Missing required fields" in data["detail"]


def test_providers_google_backend_invalid(settings_client):
    """Invalid google_backend is rejected."""
    client, _journal = settings_client

    response = client.put(
        "/app/settings/api/providers",
        json={"google_backend": "invalid"},
    )
    assert response.status_code == 400
    data = response.get_json()
    assert data["reason_code"] == "invalid_config_value"
    assert "Invalid google_backend" in data["detail"]


def test_validate_all_keys_with_vertex_credentials(settings_client):
    """validate-all-keys validates vertex credentials when configured."""
    client, journal = settings_client
    config_path = journal / "config" / "journal.json"

    # Set up vertex backend + credentials
    config = json.loads(config_path.read_text())
    config.setdefault("providers", {})["google_backend"] = "vertex"
    config["providers"]["vertex_credentials"] = str(
        journal / "config" / "vertex-credentials.json"
    )
    config_path.write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")

    with patch(
        "solstone.think.providers.google.validate_vertex_credentials",
        return_value={"valid": True, "backend": "vertex"},
    ) as mock_validate:
        response = client.post("/app/settings/api/validate-keys")

    assert response.status_code == 200
    payload = response.get_json()
    assert payload["key_validation"]["google"]["valid"] is True
    assert payload["key_validation"]["google"]["backend"] == "vertex"
    mock_validate.assert_called_once()
