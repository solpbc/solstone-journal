# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import copy
import json
from pathlib import Path
from unittest.mock import patch

from werkzeug.security import check_password_hash

from solstone.apps.link import routes as link_routes


def _read_config(journal: Path) -> dict:
    return json.loads((journal / "config" / "journal.json").read_text("utf-8"))


def _write_config(journal: Path, payload: dict) -> None:
    (journal / "config" / "journal.json").write_text(
        json.dumps(payload, indent=2) + "\n",
        encoding="utf-8",
    )


def test_network_access_requires_password_and_preserves_config(link_env) -> None:
    env = link_env()
    before = copy.deepcopy(_read_config(env.journal))

    with patch("solstone.convey.restart.wait_for_convey_restart") as restart:
        response = env.client.post("/app/link/network-access", json={})

    assert response.status_code == 400
    payload = response.get_json()
    assert payload["reason_code"] == "network_security_requires_password"
    restart.assert_not_called()
    assert _read_config(env.journal) == before


def test_network_access_with_password_persists_and_returns_restart_payload(
    link_env,
) -> None:
    env = link_env()

    with (
        patch(
            "solstone.convey.restart.wait_for_convey_restart", return_value=(True, [])
        ),
        patch(
            "solstone.think.pairing.config.get_host_url",
            return_value="http://192.168.1.44:7657",
        ),
    ):
        response = env.client.post(
            "/app/link/network-access",
            json={"password": "linkpass8"},
        )

    assert response.status_code == 200
    assert response.get_json() == {
        "ok": True,
        "restart_timeout": False,
        "effective_host_url": "http://192.168.1.44:7657",
    }
    config = _read_config(env.journal)
    assert config["convey"]["allow_network_access"] is True
    assert "password" not in config["convey"]
    assert check_password_hash(config["convey"]["password_hash"], "linkpass8")


def test_network_access_short_password_rejected_without_persisting(link_env) -> None:
    env = link_env()
    before = copy.deepcopy(_read_config(env.journal))

    with patch("solstone.convey.restart.wait_for_convey_restart") as restart:
        response = env.client.post(
            "/app/link/network-access",
            json={"password": "short"},
        )

    assert response.status_code == 400
    payload = response.get_json()
    assert payload["reason_code"] == "invalid_config_value"
    assert "8 characters" in payload["detail"]
    restart.assert_not_called()
    assert _read_config(env.journal) == before


def test_network_access_unexpected_failure_returns_convey_error(
    link_env,
    monkeypatch,
) -> None:
    env = link_env()

    def fail(*, enable: bool, password: str | None = None) -> dict:
        raise RuntimeError("boom")

    monkeypatch.setattr(link_routes, "set_network_access", fail)

    response = env.client.post(
        "/app/link/network-access",
        json={"password": "linkpass8"},
    )

    assert response.status_code == 500
    assert response.get_json()["reason_code"] == "convey_operation_failed"
