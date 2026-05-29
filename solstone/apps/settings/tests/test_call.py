# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for settings CLI commands (``sol call settings ...``)."""

from __future__ import annotations

import json
import os
from datetime import datetime, timezone
from unittest.mock import patch

import pytest
from typer.testing import CliRunner

from solstone.think.call import call_app

runner = CliRunner()


class TestShow:
    def test_show(self, settings_env):
        settings_env()

        result = runner.invoke(call_app, ["settings", "show"])

        assert result.exit_code == 0
        payload = json.loads(result.output)
        assert "identity" in payload
        assert "keys" in payload
        assert "providers" in payload
        assert "transcribe" in payload
        assert "observe" in payload
        assert "auth" not in payload["providers"]


class TestKeysShow:
    def test_keys_show(self, settings_env):
        settings_env()

        result = runner.invoke(call_app, ["settings", "keys", "show"])

        assert result.exit_code == 0
        payload = json.loads(result.output)
        assert payload["GOOGLE_API_KEY"] is True
        assert payload["ANTHROPIC_API_KEY"] is False


class TestKeysSet:
    def test_keys_set(self, settings_env):
        tmp_path, config = settings_env()
        config["providers"].pop("auth", None)
        (tmp_path / "config" / "journal.json").write_text(
            json.dumps(config, indent=2) + "\n",
            encoding="utf-8",
        )

        with (
            patch.dict(os.environ, {}, clear=False),
            patch(
                "solstone.think.providers.validate_key", return_value={"valid": True}
            ),
        ):
            result = runner.invoke(
                call_app,
                ["settings", "keys", "set", "ANTHROPIC_API_KEY", "test-key"],
            )

        assert result.exit_code == 0
        payload = json.loads(result.output)
        assert payload["set"] is True
        saved = json.loads((tmp_path / "config" / "journal.json").read_text())
        assert saved["env"]["ANTHROPIC_API_KEY"] == "test-key"
        assert "auth" not in saved.get("providers", {})

    def test_keys_set_invalid_var(self, settings_env):
        settings_env()

        result = runner.invoke(
            call_app, ["settings", "keys", "set", "BAD_KEY", "value"]
        )

        assert result.exit_code == 1


class TestKeysClear:
    def test_keys_clear(self, settings_env):
        tmp_path, config = settings_env()
        config["providers"].pop("auth", None)
        (tmp_path / "config" / "journal.json").write_text(
            json.dumps(config, indent=2) + "\n",
            encoding="utf-8",
        )

        result = runner.invoke(
            call_app, ["settings", "keys", "clear", "GOOGLE_API_KEY"]
        )

        assert result.exit_code == 0
        saved = json.loads((tmp_path / "config" / "journal.json").read_text())
        assert "GOOGLE_API_KEY" not in saved["env"]
        assert "auth" not in saved.get("providers", {})


class TestKeysValidate:
    class _FixedDateTime:
        @classmethod
        def now(cls, tz=None):
            return datetime(2026, 4, 17, 12, 0, tzinfo=tz or timezone.utc)

    def test_keys_validate_default_does_not_write_config(self, settings_env):
        tmp_path, _config = settings_env()
        config_path = tmp_path / "config" / "journal.json"
        before = config_path.read_text(encoding="utf-8")

        with (
            patch("solstone.apps.settings.call.datetime", self._FixedDateTime),
            patch(
                "solstone.think.providers.validate_key",
                side_effect=[
                    {"valid": True, "provider": "google"},
                    {"valid": True, "provider": "openai"},
                ],
            ),
        ):
            result = runner.invoke(call_app, ["settings", "keys", "validate"])

        assert result.exit_code == 0
        assert config_path.read_text(encoding="utf-8") == before
        payload = json.loads(result.output)
        assert payload == {
            "key_validation": {
                "google": {
                    "valid": True,
                    "provider": "google",
                    "timestamp": "2026-04-17T12:00:00+00:00",
                },
                "openai": {
                    "valid": True,
                    "provider": "openai",
                    "timestamp": "2026-04-17T12:00:00+00:00",
                },
            }
        }

    def test_keys_validate_cache_result_persists(self, settings_env):
        tmp_path, _config = settings_env()
        config_path = tmp_path / "config" / "journal.json"

        with (
            patch("solstone.apps.settings.call.datetime", self._FixedDateTime),
            patch(
                "solstone.think.providers.validate_key",
                side_effect=[
                    {"valid": True, "provider": "google"},
                    {"valid": True, "provider": "openai"},
                ],
            ),
        ):
            result = runner.invoke(
                call_app,
                ["settings", "keys", "validate", "--cache-result"],
            )

        assert result.exit_code == 0
        assert json.loads(result.output) == {
            "key_validation": {
                "google": {
                    "valid": True,
                    "provider": "google",
                    "timestamp": "2026-04-17T12:00:00+00:00",
                },
                "openai": {
                    "valid": True,
                    "provider": "openai",
                    "timestamp": "2026-04-17T12:00:00+00:00",
                },
            }
        }
        saved = json.loads(config_path.read_text(encoding="utf-8"))
        assert saved["providers"]["key_validation"] == {
            "google": {
                "valid": True,
                "provider": "google",
                "timestamp": "2026-04-17T12:00:00+00:00",
            },
            "openai": {
                "valid": True,
                "provider": "openai",
                "timestamp": "2026-04-17T12:00:00+00:00",
            },
        }


class TestProvidersShow:
    def test_providers_show(self, settings_env):
        settings_env()

        result = runner.invoke(call_app, ["settings", "providers", "show"])

        assert result.exit_code == 0
        payload = json.loads(result.output)
        assert payload["generate"]["provider"] == "google"
        assert payload["cogitate"]["provider"] == "openai"
        assert "auth" not in payload

    def test_provider_status_key_set_baseline_ready(self, settings_env):
        tmp_path, config = settings_env()
        config["env"]["OPENAI_API_KEY"] = "test-key"
        (tmp_path / "config" / "journal.json").write_text(
            json.dumps(config, indent=2) + "\n",
            encoding="utf-8",
        )

        result = runner.invoke(call_app, ["settings", "providers", "show"])

        assert result.exit_code == 0
        payload = json.loads(result.output)
        status = payload["provider_status"]["openai"]
        assert status == {
            "provider": "openai",
            "configured": True,
            "generate_ready": True,
            "cogitate_ready": True,
            "issues": [],
        }

    def test_provider_status_key_missing(self, settings_env):
        """Provider with key not set."""
        tmp_path, config = settings_env()
        config["env"].pop("OPENAI_API_KEY", None)
        (tmp_path / "config" / "journal.json").write_text(
            json.dumps(config, indent=2) + "\n",
            encoding="utf-8",
        )

        result = runner.invoke(call_app, ["settings", "providers", "show"])

        assert result.exit_code == 0
        payload = json.loads(result.output)
        status = payload["provider_status"]["openai"]
        assert status == {
            "provider": "openai",
            "configured": False,
            "generate_ready": False,
            "cogitate_ready": False,
            "issues": ["OPENAI_API_KEY not set"],
        }

    def test_provider_status_key_set_has_no_install_gate(self, settings_env):
        tmp_path, config = settings_env()
        config["env"]["ANTHROPIC_API_KEY"] = "test-key"
        (tmp_path / "config" / "journal.json").write_text(
            json.dumps(config, indent=2) + "\n",
            encoding="utf-8",
        )

        result = runner.invoke(call_app, ["settings", "providers", "show"])

        assert result.exit_code == 0
        payload = json.loads(result.output)
        status = payload["provider_status"]["anthropic"]
        assert status["configured"] is True
        assert status["generate_ready"] is True
        assert status["cogitate_ready"] is True
        assert "cogitate_cli" not in status
        assert "cogitate_cli_found" not in status
        assert status["issues"] == []

    def test_providers_show_human_mode(self, settings_env):
        settings_env()
        provider_status = {
            "anthropic": {
                "provider": "anthropic",
                "configured": False,
                "generate_ready": False,
                "cogitate_ready": False,
                "issues": ["ANTHROPIC_API_KEY not set"],
            },
            "google": {
                "provider": "google",
                "configured": True,
                "generate_ready": True,
                "cogitate_ready": True,
                "issues": [],
            },
            "mlx": {
                "configured": False,
                "generate_ready": False,
                "cogitate_ready": False,
                "cogitate_cli": "",
                "cogitate_cli_found": False,
                "issues": [],
            },
            "local": {
                "configured": False,
                "generate_ready": False,
                "cogitate_ready": False,
                "cogitate_cli": "llama-server",
                "cogitate_cli_found": False,
                "issues": ["binary_missing"],
            },
            "openai": {
                "provider": "openai",
                "configured": True,
                "generate_ready": True,
                "cogitate_ready": True,
                "issues": [],
            },
        }

        with patch(
            "solstone.think.providers.build_provider_status",
            return_value=provider_status,
        ):
            result = runner.invoke(
                call_app, ["settings", "providers", "show", "--human"]
            )

        assert result.exit_code == 0
        assert "local: binary_missing" in result.output.splitlines()
        assert not result.output.lstrip().startswith("{")


class TestProvidersInstall:
    def test_install_local_dispatches_to_local_install(self, settings_env, monkeypatch):
        settings_env()
        calls = []

        def install_local():
            calls.append(True)
            return {"name": "local", "install_state": "installed"}

        monkeypatch.setattr(
            "solstone.think.providers.local_install.install_local", install_local
        )

        result = runner.invoke(call_app, ["settings", "providers", "install", "local"])

        assert result.exit_code == 0
        assert calls == [True]
        assert json.loads(result.output) == {
            "name": "local",
            "install_state": "installed",
        }

    @pytest.mark.parametrize("name", ["anthropic", "openai", "openhands"])
    def test_install_non_local_raises_bad_parameter(self, settings_env, name):
        settings_env()

        result = runner.invoke(call_app, ["settings", "providers", "install", name])

        assert result.exit_code != 0
        combined = result.output + result.stderr
        assert name in combined
        assert "only 'local' is supported" in combined

    @pytest.mark.parametrize("verb", ["uninstall", "disable", "enable", "validate-key"])
    @pytest.mark.parametrize("name", ["anthropic", "openai", "openhands"])
    def test_retired_verbs_return_no_such_command(self, settings_env, verb, name):
        settings_env()

        result = runner.invoke(call_app, ["settings", "providers", verb, name])

        assert result.exit_code != 0
        assert "No such command" in (result.output + result.stderr)


class TestProvidersSetGenerate:
    def test_set_generate_provider(self, settings_env):
        tmp_path, _config = settings_env()

        result = runner.invoke(
            call_app,
            ["settings", "providers", "set-generate", "--provider", "openai"],
        )

        assert result.exit_code == 0
        saved = json.loads((tmp_path / "config" / "journal.json").read_text())
        assert saved["providers"]["generate"]["provider"] == "openai"

    def test_set_generate_invalid_provider(self, settings_env):
        settings_env()

        result = runner.invoke(
            call_app,
            ["settings", "providers", "set-generate", "--provider", "invalid"],
        )

        assert result.exit_code == 1

    def test_set_generate_invalid_tier(self, settings_env):
        settings_env()

        result = runner.invoke(
            call_app,
            ["settings", "providers", "set-generate", "--tier", "5"],
        )

        assert result.exit_code == 1


class TestGoogleBackend:
    def test_show(self, settings_env):
        settings_env()

        result = runner.invoke(call_app, ["settings", "google-backend", "show"])

        assert result.exit_code == 0
        payload = json.loads(result.output)
        assert payload["google_backend"] == "auto"

    def test_set(self, settings_env):
        tmp_path, _config = settings_env()

        result = runner.invoke(
            call_app, ["settings", "google-backend", "set", "vertex"]
        )

        assert result.exit_code == 0
        saved = json.loads((tmp_path / "config" / "journal.json").read_text())
        assert saved["providers"]["google_backend"] == "vertex"

    def test_set_invalid(self, settings_env):
        settings_env()

        result = runner.invoke(
            call_app, ["settings", "google-backend", "set", "invalid"]
        )

        assert result.exit_code == 1


class TestVertexCredentials:
    def test_import(self, settings_env, tmp_path):
        journal_path, _config = settings_env()
        creds_path = tmp_path / "creds.json"
        creds_path.write_text(
            json.dumps(
                {
                    "type": "service_account",
                    "project_id": "test-project",
                    "client_email": "test@test.iam.gserviceaccount.com",
                    "private_key": "private-key",
                }
            ),
            encoding="utf-8",
        )

        with patch(
            "solstone.think.providers.google.validate_vertex_credentials",
            return_value={
                "valid": True,
                "email": "test@test.iam.gserviceaccount.com",
            },
        ):
            result = runner.invoke(
                call_app,
                ["settings", "vertex-credentials", "import", str(creds_path)],
            )

        assert result.exit_code == 0
        canonical = journal_path / ".config" / "vertex-credentials.json"
        assert canonical.exists()
        saved = json.loads((journal_path / "config" / "journal.json").read_text())
        assert saved["providers"]["vertex_credentials"] == str(canonical)

    def test_import_missing_fields(self, settings_env, tmp_path):
        settings_env()
        creds_path = tmp_path / "creds.json"
        creds_path.write_text(json.dumps({"type": "service_account"}), encoding="utf-8")

        result = runner.invoke(
            call_app,
            ["settings", "vertex-credentials", "import", str(creds_path)],
        )

        assert result.exit_code == 1

    def test_import_skip_validation(self, settings_env, tmp_path):
        journal_path, _config = settings_env()
        creds_path = tmp_path / "creds.json"
        creds_path.write_text(
            json.dumps(
                {
                    "type": "service_account",
                    "project_id": "test-project",
                    "client_email": "test@test.iam.gserviceaccount.com",
                    "private_key": "private-key",
                }
            ),
            encoding="utf-8",
        )

        with patch(
            "solstone.think.providers.google.validate_vertex_credentials"
        ) as mock_validate:
            result = runner.invoke(
                call_app,
                [
                    "settings",
                    "vertex-credentials",
                    "import",
                    str(creds_path),
                    "--skip-validation",
                ],
            )

        assert result.exit_code == 0
        assert mock_validate.call_count == 0
        assert (journal_path / ".config" / "vertex-credentials.json").exists()

    def test_clear(self, settings_env):
        journal_path, config = settings_env()
        creds_dir = journal_path / ".config"
        creds_dir.mkdir(parents=True, exist_ok=True)
        creds_file = creds_dir / "vertex-credentials.json"
        creds_file.write_text(
            json.dumps(
                {
                    "type": "service_account",
                    "project_id": "test-project",
                    "client_email": "test@test.iam.gserviceaccount.com",
                    "private_key": "private-key",
                }
            ),
            encoding="utf-8",
        )

        config_path = journal_path / "config" / "journal.json"
        saved = json.loads(config_path.read_text())
        saved["providers"]["vertex_credentials"] = str(creds_file)
        saved["providers"]["key_validation"]["google_vertex"] = {"valid": True}
        config_path.write_text(json.dumps(saved, indent=2) + "\n", encoding="utf-8")

        result = runner.invoke(call_app, ["settings", "vertex-credentials", "clear"])

        assert result.exit_code == 0
        assert not creds_file.exists()
        updated = json.loads(config_path.read_text())
        assert "vertex_credentials" not in updated["providers"]
        assert "google_vertex" not in updated["providers"]["key_validation"]

    def test_show(self, settings_env):
        settings_env()

        result = runner.invoke(call_app, ["settings", "vertex-credentials", "show"])

        assert result.exit_code == 0
        payload = json.loads(result.output)
        assert payload["configured"] is False


class TestTranscribe:
    def test_show(self, settings_env):
        settings_env()

        result = runner.invoke(call_app, ["settings", "transcribe", "show"])

        assert result.exit_code == 0
        payload = json.loads(result.output)
        assert payload["backends"]

    def test_set_backend(self, settings_env):
        tmp_path, _config = settings_env()

        result = runner.invoke(
            call_app, ["settings", "transcribe", "set-backend", "gemini"]
        )

        assert result.exit_code == 0
        saved = json.loads((tmp_path / "config" / "journal.json").read_text())
        assert saved["transcribe"]["backend"] == "gemini"

    def test_set_backend_parakeet(self, settings_env):
        tmp_path, _config = settings_env()

        result = runner.invoke(
            call_app, ["settings", "transcribe", "set-backend", "parakeet"]
        )

        assert result.exit_code == 0
        saved = json.loads((tmp_path / "config" / "journal.json").read_text())
        assert saved["transcribe"]["backend"] == "parakeet"

    def test_set_backend_invalid(self, settings_env):
        settings_env()

        result = runner.invoke(
            call_app, ["settings", "transcribe", "set-backend", "invalid"]
        )

        assert result.exit_code == 1


class TestIdentity:
    def test_show(self, settings_env):
        settings_env()

        result = runner.invoke(call_app, ["settings", "identity", "show"])

        assert result.exit_code == 0
        payload = json.loads(result.output)
        assert payload["name"] == "Test User"

    def test_set_name(self, settings_env):
        tmp_path, _config = settings_env()

        with patch("solstone.apps.settings.call.subprocess.run") as mock_run:
            result = runner.invoke(
                call_app, ["settings", "identity", "set", "--name", "New Name"]
            )

        assert result.exit_code == 0
        saved = json.loads((tmp_path / "config" / "journal.json").read_text())
        assert saved["identity"]["name"] == "New Name"
        assert mock_run.call_count == 1

    def test_set_add_email(self, settings_env):
        tmp_path, _config = settings_env()

        with patch("solstone.apps.settings.call.subprocess.run"):
            result = runner.invoke(
                call_app,
                ["settings", "identity", "set", "--add-email", "new@example.com"],
            )

        assert result.exit_code == 0
        saved = json.loads((tmp_path / "config" / "journal.json").read_text())
        assert "new@example.com" in saved["identity"]["email_addresses"]

    def test_set_remove_email(self, settings_env):
        tmp_path, _config = settings_env()

        with patch("solstone.apps.settings.call.subprocess.run"):
            result = runner.invoke(
                call_app,
                ["settings", "identity", "set", "--remove-email", "test@example.com"],
            )

        assert result.exit_code == 0
        saved = json.loads((tmp_path / "config" / "journal.json").read_text())
        assert "test@example.com" not in saved["identity"]["email_addresses"]


class TestObserver:
    def test_show(self, settings_env):
        settings_env()

        result = runner.invoke(call_app, ["settings", "observer", "show"])

        assert result.exit_code == 0
        payload = json.loads(result.output)
        assert payload["tmux"]["enabled"] is True
        assert payload["tmux"]["capture_interval"] == 5

    def test_set_enabled(self, settings_env):
        tmp_path, _config = settings_env()

        result = runner.invoke(
            call_app, ["settings", "observer", "set", "--no-enabled"]
        )

        assert result.exit_code == 0
        saved = json.loads((tmp_path / "config" / "journal.json").read_text())
        assert saved["observe"]["tmux"]["enabled"] is False

    def test_set_capture_interval(self, settings_env):
        tmp_path, _config = settings_env()

        result = runner.invoke(
            call_app,
            ["settings", "observer", "set", "--capture-interval", "10"],
        )

        assert result.exit_code == 0
        saved = json.loads((tmp_path / "config" / "journal.json").read_text())
        assert saved["observe"]["tmux"]["capture_interval"] == 10

    def test_set_capture_interval_invalid(self, settings_env):
        settings_env()

        result = runner.invoke(
            call_app,
            ["settings", "observer", "set", "--capture-interval", "100"],
        )

        assert result.exit_code == 1
