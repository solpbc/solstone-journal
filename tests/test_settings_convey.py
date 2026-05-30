# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
from pathlib import Path
from unittest.mock import patch

from typer.testing import CliRunner

from solstone.apps.settings.copy import (
    CONVEY_HOST_URL_CLEARED,
    CONVEY_HOST_URL_SET_DONE,
    CONVEY_NETWORK_DISABLE_DONE,
    CONVEY_NETWORK_DISABLE_PROGRESS,
    CONVEY_NETWORK_ENABLE_DONE,
    CONVEY_NETWORK_ENABLE_PROGRESS,
    CONVEY_REFUSE_NO_PASSWORD_NETWORK,
    CONVEY_REFUSE_NO_PASSWORD_TRUST,
    CONVEY_RESTART_TIMEOUT,
    CONVEY_TRUST_DISABLE_DONE,
    format_convey_status,
)
from solstone.convey import create_app
from solstone.think.call import call_app
from solstone.think.pairing.config import (
    HOST_URL_HOSTNAME_UNSUPPORTED,
    HOST_URL_INVALID,
)

runner = CliRunner()


def _read_config(journal_dir: Path) -> dict:
    return json.loads((journal_dir / "config" / "journal.json").read_text("utf-8"))


def _write_config(journal_dir: Path, payload: dict) -> None:
    (journal_dir / "config" / "journal.json").write_text(
        json.dumps(payload, indent=2) + "\n",
        encoding="utf-8",
    )


def _clear_password(journal_dir: Path) -> None:
    config = _read_config(journal_dir)
    config["convey"].pop("password_hash", None)
    config["convey"].pop("password", None)
    _write_config(journal_dir, config)


def _settings_client(journal_dir: Path):
    app = create_app(str(journal_dir))
    app.config["TESTING"] = True
    return app.test_client()


def test_cli_status_exact_output(journal_copy):
    result = runner.invoke(call_app, ["settings", "convey", "status"])

    expected = format_convey_status(
        network_access="localhost only",
        bind="127.0.0.1:5015",
        password="set",
        trust_localhost="yes",
        host_url="http://localhost:5015 (localhost — network access off)",
    )
    assert result.exit_code == 0
    assert result.output == expected + "\n"


def test_cli_status_reports_manual_host_override(journal_copy):
    config = _read_config(journal_copy)
    config["pairing"] = {"host_url": "http://192.168.1.44:5015"}
    _write_config(journal_copy, config)

    result = runner.invoke(call_app, ["settings", "convey", "status"])

    assert result.exit_code == 0
    assert (
        "host url:          http://192.168.1.44:5015 (manual override)" in result.output
    )


def test_cli_status_reports_auto_detected_host(journal_copy):
    config = _read_config(journal_copy)
    config["convey"]["allow_network_access"] = True
    _write_config(journal_copy, config)
    health_dir = journal_copy / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    (health_dir / "convey.port").write_text("6123", encoding="utf-8")

    with patch(
        "solstone.apps.settings.call.get_host_url",
        return_value="http://192.168.1.44:6123",
    ):
        result = runner.invoke(call_app, ["settings", "convey", "status"])

    assert result.exit_code == 0
    assert (
        result.output
        == format_convey_status(
            network_access="on",
            bind="0.0.0.0:6123",
            password="set",
            trust_localhost="yes",
            host_url="http://192.168.1.44:6123 (auto-detected)",
        )
        + "\n"
    )


def test_cli_network_access_enable_refuses_without_password(journal_copy):
    _clear_password(journal_copy)

    result = runner.invoke(call_app, ["settings", "convey", "network-access", "enable"])

    assert result.exit_code == 1
    assert result.stderr.strip() == CONVEY_REFUSE_NO_PASSWORD_NETWORK
    assert _read_config(journal_copy)["convey"]["allow_network_access"] is False


def test_cli_network_access_enable_restarts_and_prints_host_url(journal_copy):
    with (
        patch(
            "solstone.convey.restart.wait_for_convey_restart", return_value=(True, [])
        ) as restart,
        patch(
            "solstone.think.pairing.config.get_host_url",
            return_value="http://192.168.1.44:5015",
        ),
    ):
        result = runner.invoke(
            call_app, ["settings", "convey", "network-access", "enable"]
        )

    assert result.exit_code == 0
    assert result.stdout == (
        CONVEY_NETWORK_ENABLE_PROGRESS
        + "\n"
        + CONVEY_NETWORK_ENABLE_DONE.format(host_url="http://192.168.1.44:5015")
        + "\n"
    )
    restart.assert_called_once_with(timeout=15.0)
    assert _read_config(journal_copy)["convey"]["allow_network_access"] is True


def test_cli_network_access_disable_timeout_exits_nonzero(journal_copy):
    config = _read_config(journal_copy)
    config["convey"]["allow_network_access"] = True
    _write_config(journal_copy, config)

    with patch(
        "solstone.convey.restart.wait_for_convey_restart", return_value=(False, [])
    ):
        result = runner.invoke(
            call_app, ["settings", "convey", "network-access", "disable"]
        )

    assert result.exit_code == 1
    assert result.stdout == CONVEY_NETWORK_DISABLE_PROGRESS + "\n"
    assert result.stderr.strip() == CONVEY_RESTART_TIMEOUT
    assert _read_config(journal_copy)["convey"]["allow_network_access"] is False


def test_cli_network_access_disable_success_uses_localhost_copy(journal_copy):
    config = _read_config(journal_copy)
    config["convey"]["allow_network_access"] = True
    _write_config(journal_copy, config)

    with patch(
        "solstone.convey.restart.wait_for_convey_restart", return_value=(True, [])
    ):
        result = runner.invoke(
            call_app, ["settings", "convey", "network-access", "disable"]
        )

    assert result.exit_code == 0
    assert result.stdout == (
        CONVEY_NETWORK_DISABLE_PROGRESS
        + "\n"
        + CONVEY_NETWORK_DISABLE_DONE.format(port=5015)
        + "\n"
    )


def test_cli_trust_localhost_disable_refuses_without_password(journal_copy):
    _clear_password(journal_copy)

    result = runner.invoke(
        call_app, ["settings", "convey", "trust-localhost", "disable"]
    )

    assert result.exit_code == 1
    assert result.stderr.strip() == CONVEY_REFUSE_NO_PASSWORD_TRUST


def test_cli_trust_localhost_disable_does_not_restart(journal_copy):
    with patch("solstone.convey.restart.wait_for_convey_restart") as restart:
        result = runner.invoke(
            call_app, ["settings", "convey", "trust-localhost", "disable"]
        )

    assert result.exit_code == 0
    assert result.stdout == CONVEY_TRUST_DISABLE_DONE + "\n"
    restart.assert_not_called()
    assert _read_config(journal_copy)["convey"]["trust_localhost"] is False


def test_cli_host_url_set_auto_and_show(journal_copy):
    set_result = runner.invoke(
        call_app,
        ["settings", "convey", "host-url", "192.168.1.44:5015"],
    )
    assert set_result.exit_code == 0
    assert set_result.stdout == (
        CONVEY_HOST_URL_SET_DONE.format(url="http://192.168.1.44:5015") + "\n"
    )

    show_result = runner.invoke(call_app, ["settings", "convey", "host-url", "--show"])
    assert show_result.exit_code == 0
    assert show_result.stdout == "http://192.168.1.44:5015\n"

    auto_result = runner.invoke(call_app, ["settings", "convey", "host-url", "--auto"])
    assert auto_result.exit_code == 0
    assert auto_result.stdout == CONVEY_HOST_URL_CLEARED + "\n"
    assert _read_config(journal_copy)["pairing"]["host_url"] is None


def test_cli_host_url_rejects_relative_url(journal_copy):
    result = runner.invoke(call_app, ["settings", "convey", "host-url", "/bad"])

    assert result.exit_code == 1
    assert result.stderr.strip() == HOST_URL_INVALID


def test_cli_host_url_rejects_hostname(journal_copy):
    result = runner.invoke(
        call_app, ["settings", "convey", "host-url", "mylab.local:5015"]
    )

    assert result.exit_code == 1
    assert result.stderr.strip() == HOST_URL_HOSTNAME_UNSUPPORTED


def test_api_get_config_masks_password_without_effective_host_url(journal_copy):
    client = _settings_client(journal_copy)

    response = client.get("/app/settings/api/config")

    assert response.status_code == 200
    payload = response.get_json()
    assert payload["convey"]["allow_network_access"] is False
    assert payload["convey"]["has_password"] is True
    assert "password_hash" not in payload["convey"]
    assert "pairing" not in payload


def test_api_put_network_access_refuses_without_password(journal_copy):
    _clear_password(journal_copy)
    client = _settings_client(journal_copy)

    response = client.put(
        "/app/settings/api/config",
        json={"section": "convey", "key": "allow_network_access", "value": True},
        content_type="application/json",
    )

    assert response.status_code == 400
    payload = response.get_json()
    assert (
        payload["error"] == "I couldn't change network access until a password is set."
    )
    assert payload["reason_code"] == "network_security_requires_password"
    assert payload["detail"] == CONVEY_REFUSE_NO_PASSWORD_NETWORK


def test_api_put_combined_password_and_network_succeeds(journal_copy):
    _clear_password(journal_copy)
    client = _settings_client(journal_copy)

    with patch(
        "solstone.convey.restart.wait_for_convey_restart", return_value=(True, [])
    ):
        response = client.put(
            "/app/settings/api/config",
            json={
                "section": "convey",
                "data": {"password": "atomicpw8", "allow_network_access": True},
            },
            content_type="application/json",
        )

    assert response.status_code == 200
    config = _read_config(journal_copy)
    assert config["convey"]["password_hash"]
    assert "password" not in config["convey"]
    assert config["convey"]["allow_network_access"] is True


def test_api_put_combined_password_too_short_rejected(journal_copy):
    _clear_password(journal_copy)
    client = _settings_client(journal_copy)

    response = client.put(
        "/app/settings/api/config",
        json={
            "section": "convey",
            "data": {"password": "short", "allow_network_access": True},
        },
        content_type="application/json",
    )

    assert response.status_code == 400
    payload = response.get_json()
    assert payload["reason_code"] == "invalid_config_value"
    assert "8 characters" in payload["detail"]
    config = _read_config(journal_copy)
    assert "password_hash" not in config["convey"]
    assert config["convey"]["allow_network_access"] is False


def test_api_put_network_enable_without_password_field_still_refused(journal_copy):
    _clear_password(journal_copy)
    client = _settings_client(journal_copy)

    response = client.put(
        "/app/settings/api/config",
        json={"section": "convey", "data": {"allow_network_access": True}},
        content_type="application/json",
    )

    assert response.status_code == 400
    payload = response.get_json()
    assert payload["reason_code"] == "network_security_requires_password"
    assert payload["detail"] == CONVEY_REFUSE_NO_PASSWORD_NETWORK
    assert _read_config(journal_copy)["convey"]["allow_network_access"] is False


def test_api_put_trust_localhost_refuses_without_password(journal_copy):
    _clear_password(journal_copy)
    client = _settings_client(journal_copy)

    response = client.put(
        "/app/settings/api/config",
        json={"section": "convey", "data": {"trust_localhost": False}},
        content_type="application/json",
    )

    assert response.status_code == 400
    payload = response.get_json()
    assert (
        payload["error"] == "I couldn't change network access until a password is set."
    )
    assert payload["reason_code"] == "network_security_requires_password"
    assert payload["detail"] == CONVEY_REFUSE_NO_PASSWORD_TRUST


def test_api_put_network_access_returns_restart_payload(journal_copy):
    client = _settings_client(journal_copy)

    with (
        patch(
            "solstone.convey.restart.wait_for_convey_restart", return_value=(True, [])
        ) as restart,
        patch(
            "solstone.think.pairing.config.get_host_url",
            return_value="http://192.168.1.44:5015",
        ),
    ):
        response = client.put(
            "/app/settings/api/config",
            json={"section": "convey", "key": "allow_network_access", "value": True},
            content_type="application/json",
        )

    assert response.status_code == 200
    assert response.get_json() == {
        "effective_host_url": "http://192.168.1.44:5015",
        "ok": True,
        "restart_timeout": False,
    }
    restart.assert_called_once_with(timeout=15.0)
    assert _read_config(journal_copy)["convey"]["allow_network_access"] is True


def test_api_put_network_access_timeout_still_saves(journal_copy):
    client = _settings_client(journal_copy)

    with (
        patch(
            "solstone.convey.restart.wait_for_convey_restart", return_value=(False, [])
        ),
        patch(
            "solstone.think.pairing.config.get_host_url",
            return_value="http://localhost:5015",
        ),
    ):
        response = client.put(
            "/app/settings/api/config",
            json={"section": "convey", "data": {"allow_network_access": True}},
            content_type="application/json",
        )

    assert response.status_code == 200
    assert response.get_json() == {
        "effective_host_url": "http://localhost:5015",
        "ok": True,
        "restart_timeout": True,
    }
    assert _read_config(journal_copy)["convey"]["allow_network_access"] is True
