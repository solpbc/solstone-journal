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
from solstone.think.providers import bundled

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
        tmp_path, _config = settings_env()

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
        assert saved["providers"]["auth"]["anthropic"] == "api_key"

    def test_keys_set_invalid_var(self, settings_env):
        settings_env()

        result = runner.invoke(
            call_app, ["settings", "keys", "set", "BAD_KEY", "value"]
        )

        assert result.exit_code == 1


class TestKeysClear:
    def test_keys_clear(self, settings_env):
        tmp_path, _config = settings_env()

        result = runner.invoke(
            call_app, ["settings", "keys", "clear", "GOOGLE_API_KEY"]
        )

        assert result.exit_code == 0
        saved = json.loads((tmp_path / "config" / "journal.json").read_text())
        assert "GOOGLE_API_KEY" not in saved["env"]
        assert saved["providers"]["auth"]["google"] == "platform"


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

    def test_provider_status_key_set_cli_found(self, settings_env):
        """Provider with key set and bundled CLI installed."""
        tmp_path, config = settings_env()
        config["providers"]["bundled"] = {
            "openai": {
                "state": "installed-no-key",
                "last_transition_at": "2026-05-20T00:00:00+00:00",
                "sdk_spec": "openai-codex-sdk==0.1.11",
                "binary_path": "/tmp/codex",
                "install_error": None,
            }
        }
        (tmp_path / "config" / "journal.json").write_text(
            json.dumps(config, indent=2) + "\n",
            encoding="utf-8",
        )

        result = runner.invoke(call_app, ["settings", "providers", "show"])

        assert result.exit_code == 0
        payload = json.loads(result.output)
        status = payload["provider_status"]["openai"]
        assert status["configured"] is True
        assert status["generate_ready"] is True
        assert status["cogitate_ready"] is True
        assert status["cogitate_cli"] == "codex"
        assert status["cogitate_cli_found"] is True
        assert status["issues"] == []

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
        assert status["configured"] is False
        assert status["generate_ready"] is False
        assert status["cogitate_ready"] is False
        assert "OPENAI_API_KEY not set" in status["issues"]

    def test_provider_status_key_set_cli_missing(self, settings_env):
        """Provider with key set but bundled CLI not installed."""
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
        assert status["cogitate_ready"] is False
        assert status["cogitate_cli_found"] is False
        assert (
            "bundled CLI not installed — run `sol call settings providers install anthropic`"
            in status["issues"]
        )

    def test_providers_show_human_mode(self, settings_env):
        settings_env()
        provider_status = {
            "anthropic": {
                "configured": False,
                "generate_ready": False,
                "cogitate_ready": False,
                "cogitate_cli": "claude",
                "cogitate_cli_found": False,
                "issues": ["ANTHROPIC_API_KEY not set"],
            },
            "google": {
                "configured": True,
                "generate_ready": True,
                "cogitate_ready": True,
                "cogitate_cli": "gemini",
                "cogitate_cli_found": True,
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
            "ollama": {
                "configured": True,
                "generate_ready": True,
                "cogitate_ready": False,
                "cogitate_cli": "opencode",
                "cogitate_cli_found": False,
                "issues": [
                    "opencode CLI not found on PATH — run: "
                    "curl -fsSL https://opencode.ai/install | bash"
                ],
            },
            "openai": {
                "configured": True,
                "generate_ready": True,
                "cogitate_ready": True,
                "cogitate_cli": "codex",
                "cogitate_cli_found": True,
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
        assert (
            "ollama: opencode CLI not found on PATH — run: "
            "curl -fsSL https://opencode.ai/install | bash"
        ) in result.output.splitlines()
        assert not result.output.lstrip().startswith("{")


class TestProvidersBundled:
    def test_status_single_json(self, settings_env):
        tmp_path, config = settings_env()
        config["providers"]["bundled"] = {
            "anthropic": {
                "state": "valid",
                "last_transition_at": "2026-05-20T00:00:00+00:00",
                "sdk_spec": "claude-agent-sdk==0.2.82",
                "binary_path": "/tmp/claude",
                "install_error": None,
            }
        }
        config["env"]["ANTHROPIC_API_KEY"] = "test-key"
        config["providers"]["key_validation"]["anthropic"] = {"valid": True}
        (tmp_path / "config" / "journal.json").write_text(
            json.dumps(config, indent=2) + "\n",
            encoding="utf-8",
        )

        result = runner.invoke(
            call_app, ["settings", "providers", "status", "anthropic"]
        )

        assert result.exit_code == 0
        payload = json.loads(result.output)
        assert payload["name"] == "anthropic"
        assert payload["state"] == "valid"

    def test_status_all_json(self, settings_env):
        settings_env()

        result = runner.invoke(call_app, ["settings", "providers", "status"])

        assert result.exit_code == 0
        payload = json.loads(result.output)
        assert set(payload) == {"anthropic", "openai"}

    def test_status_human(self, settings_env):
        settings_env()

        result = runner.invoke(
            call_app,
            ["settings", "providers", "status", "--human"],
        )

        assert result.exit_code == 0
        assert "provider" in result.output
        assert "stuck" in result.output
        assert "anthropic" in result.output

    def test_status_human_surfaces_stuck_enabling(self, settings_env):
        tmp_path, config = settings_env()
        config["providers"]["bundled"] = {
            "anthropic": {
                "state": "enabling",
                "last_transition_at": "2000-01-01T00:00:00+00:00",
                "sdk_spec": "claude-agent-sdk==0.2.82",
                "install_error": None,
            }
        }
        (tmp_path / "config" / "journal.json").write_text(
            json.dumps(config, indent=2) + "\n",
            encoding="utf-8",
        )

        result = runner.invoke(
            call_app,
            ["settings", "providers", "status", "anthropic", "--human"],
        )

        assert result.exit_code == 0
        assert "stuck" in result.output
        assert "yes" in result.output

    def test_status_json_human_conflict(self, settings_env):
        settings_env()

        result = runner.invoke(
            call_app,
            ["settings", "providers", "status", "--json", "--human"],
        )

        assert result.exit_code == 1
        assert "--json and --human cannot be used together" in result.output

    @pytest.mark.parametrize(
        ("command", "function_name"),
        [
            ("install", "install_provider"),
            ("uninstall", "uninstall_provider"),
            ("disable", "disable_provider"),
            ("enable", "enable_provider"),
            ("validate-key", "validate_key"),
        ],
    )
    def test_write_verbs_emit_json(
        self, settings_env, monkeypatch, command, function_name
    ):
        settings_env()
        payload = {"name": "openai", "state": "valid"}
        monkeypatch.setattr(bundled, function_name, lambda name: payload)

        result = runner.invoke(call_app, ["settings", "providers", command, "openai"])

        assert result.exit_code == 0
        assert json.loads(result.output) == payload

    def test_write_verb_error_exits_nonzero(self, settings_env, monkeypatch):
        settings_env()

        def fail(_name):
            raise bundled.UnsupportedBundledProvider("bad provider")

        monkeypatch.setattr(bundled, "install_provider", fail)

        result = runner.invoke(call_app, ["settings", "providers", "install", "google"])

        assert result.exit_code == 1
        payload = json.loads(result.stderr)
        assert payload["error"] == "bad provider"
        assert payload["type"] == "UnsupportedBundledProvider"


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
