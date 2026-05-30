# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
from typing import Any

from solstone.apps.link import routes as link_routes
from solstone.think.link.local_endpoints import LocalEndpoint
from solstone.think.link.window import read_posture

TOTP_SECRET = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"


def _write_config(env: Any, *, link: Any = None, include_link: bool = True) -> None:
    config: dict[str, Any] = {
        "convey": {"trust_localhost": True},
        "setup": {"completed_at": 1700000000000},
    }
    if include_link:
        config["link"] = link
    (env.journal / "config" / "journal.json").write_text(
        json.dumps(config, indent=2),
        encoding="utf-8",
    )


def _write_service_token(env: Any, token: str = "secret-token-xyz") -> None:
    token_path = env.journal / "link" / "tokens" / "account.json"
    token_path.parent.mkdir(parents=True, exist_ok=True)
    token_path.write_text(
        json.dumps({"service_token": token}),
        encoding="utf-8",
    )


def _get_status(env: Any) -> dict[str, Any]:
    response = env.client.get(
        "/app/link/api/status",
        base_url="http://localhost:7657",
    )
    assert response.status_code == 200
    payload = response.get_json()
    assert isinstance(payload, dict)
    return payload


class _StubWatcher:
    def __init__(self, endpoints: list[LocalEndpoint]) -> None:
        self._endpoints = endpoints

    def snapshot(self) -> list[LocalEndpoint]:
        return list(self._endpoints)


def test_posture_defaults_and_spl(link_env) -> None:
    env = link_env()

    _write_config(env, include_link=False)
    assert read_posture() == "direct"

    for link_cfg in (
        {"posture": 123},
        {"posture": "relay"},
        {"posture": "spl "},
    ):
        _write_config(env, link=link_cfg)
        assert read_posture() == "direct"

    _write_config(env, link={"posture": "spl"})
    assert read_posture() == "spl"


def test_direct_healthy_reports_online(link_env, monkeypatch) -> None:
    env = link_env()
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")

    data = _get_status(env)

    assert data["lan_accessible"] is True
    assert data["home_address"] == "192.168.1.50:7657"
    assert data["posture"] == "direct"
    assert data["reachability"] == "online"
    assert data["relay_state"] == "not-enrolled"


def test_direct_reports_host_address_override(link_env, monkeypatch) -> None:
    env = link_env()
    config_path = env.journal / "config" / "journal.json"
    config = json.loads(config_path.read_text("utf-8"))
    config["pairing"] = {"host_url": "http://192.168.1.44:7657"}
    config_path.write_text(json.dumps(config, indent=2), encoding="utf-8")
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")

    data = _get_status(env)

    assert data["lan_accessible"] is True
    assert data["home_address"] == "192.168.1.44:7657"
    assert data["reachability"] == "online"


def test_host_address_override_unblocks_lan_unreachable(link_env, monkeypatch) -> None:
    env = link_env()
    config_path = env.journal / "config" / "journal.json"
    config = json.loads(config_path.read_text("utf-8"))
    config["pairing"] = {"host_url": "http://192.168.1.44:7657"}
    config_path.write_text(json.dumps(config, indent=2), encoding="utf-8")
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: None)

    data = _get_status(env)

    assert data["lan_accessible"] is True
    assert data["home_address"] == "192.168.1.44:7657"
    assert data["reachability"] == "online"


def test_loopback_only_is_lan_unreachable(link_env, monkeypatch) -> None:
    env = link_env()
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: None)

    data = _get_status(env)

    assert data["lan_accessible"] is False
    assert data["home_address"] is None
    assert data["reachability"] == "lan-unreachable"


def test_lan_unreachable_precedence_over_spl(link_env, monkeypatch) -> None:
    env = link_env()
    _write_config(env, link={"posture": "spl"})
    _write_service_token(env)
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: None)
    monkeypatch.setattr(
        link_routes, "_read_link_connection_event", lambda: "disconnect"
    )

    data = _get_status(env)

    assert data["relay_state"] == "offline"
    assert data["reachability"] == "lan-unreachable"


def test_relay_state_helper() -> None:
    assert link_routes._derive_relay_state(False) == "not-enrolled"
    assert link_routes._derive_relay_state(True) == "offline"


def test_spl_reachability_mapping() -> None:
    assert (
        link_routes._derive_reachability(True, "spl", "connecting") == "finishing-setup"
    )
    assert link_routes._derive_reachability(True, "spl", "parked") == "online"
    assert link_routes._derive_reachability(True, "spl", "offline") == "offline"
    assert (
        link_routes._derive_reachability(True, "spl", "not-enrolled")
        == "finishing-setup"
    )
    assert link_routes._derive_reachability(True, "direct", "offline") == "online"
    assert link_routes._derive_reachability(False, "spl", "parked") == "lan-unreachable"


def test_spl_relay_state_never_parks_without_connected() -> None:
    assert link_routes._derive_spl_relay_state(False, "connected") == "not-enrolled"
    for event in (None, "connecting", "disconnect", "enrolled", "tunnel_pair"):
        assert link_routes._derive_spl_relay_state(True, event) != "parked"
    assert link_routes._derive_spl_relay_state(True, "connected") == "parked"


def test_spl_status_without_token_reports_not_enrolled(link_env, monkeypatch) -> None:
    env = link_env(posture="spl", totp_secret=TOTP_SECRET)
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")
    monkeypatch.setattr(link_routes, "_read_link_connection_event", lambda: "connected")

    data = _get_status(env)

    assert data["enrolled"] is False
    assert data["relay_state"] == "not-enrolled"
    assert data["reachability"] == "finishing-setup"


def test_spl_status_without_cached_event_reports_connecting(
    link_env,
    monkeypatch,
) -> None:
    env = link_env(posture="spl", totp_secret=TOTP_SECRET)
    _write_service_token(env)
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")
    monkeypatch.setattr(link_routes, "_read_link_connection_event", lambda: None)

    data = _get_status(env)

    assert data["relay_state"] == "connecting"
    assert data["reachability"] == "finishing-setup"


def test_spl_status_connecting_event_reports_connecting(link_env, monkeypatch) -> None:
    env = link_env(posture="spl", totp_secret=TOTP_SECRET)
    _write_service_token(env)
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")
    monkeypatch.setattr(
        link_routes, "_read_link_connection_event", lambda: "connecting"
    )

    data = _get_status(env)

    assert data["relay_state"] == "connecting"
    assert data["reachability"] == "finishing-setup"


def test_spl_status_connected_event_reports_parked_online(
    link_env, monkeypatch
) -> None:
    env = link_env(posture="spl", totp_secret=TOTP_SECRET)
    _write_service_token(env)
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")
    monkeypatch.setattr(link_routes, "_read_link_connection_event", lambda: "connected")

    data = _get_status(env)

    assert data["relay_state"] == "parked"
    assert data["reachability"] == "online"


def test_spl_status_disconnect_event_reports_offline(link_env, monkeypatch) -> None:
    env = link_env(posture="spl", totp_secret=TOTP_SECRET)
    _write_service_token(env)
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")
    monkeypatch.setattr(
        link_routes, "_read_link_connection_event", lambda: "disconnect"
    )

    data = _get_status(env)

    assert data["relay_state"] == "offline"
    assert data["reachability"] == "offline"


def test_relay_state_flips_with_real_token(link_env, monkeypatch) -> None:
    env = link_env()
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")

    data = _get_status(env)

    assert data["enrolled"] is False
    assert data["relay_state"] == "not-enrolled"

    _write_service_token(env, "secret-token-abc")

    data = _get_status(env)

    assert data["enrolled"] is True
    assert data["relay_state"] == "offline"


def test_vpn_empty_when_no_watcher(link_env, monkeypatch) -> None:
    env = link_env()
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")
    monkeypatch.setattr(link_routes, "get_interface_watcher", lambda: None)

    data = _get_status(env)

    assert data["vpn"] == {"active": None, "candidates": []}


def test_vpn_filters_non_vpn_scopes(link_env, monkeypatch) -> None:
    env = link_env()
    stub = _StubWatcher([LocalEndpoint(ip="192.168.1.50", port=7657, scope="lan")])
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")
    monkeypatch.setattr(link_routes, "get_interface_watcher", lambda: stub)

    data = _get_status(env)

    assert data["vpn"]["candidates"] == []


def test_vpn_maps_synthetic_vpn_endpoint(link_env, monkeypatch) -> None:
    env = link_env()
    stub = _StubWatcher([LocalEndpoint(ip="100.64.0.5", port=7657, scope="vpn")])
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")
    monkeypatch.setattr(link_routes, "get_interface_watcher", lambda: stub)

    data = _get_status(env)

    assert data["vpn"]["candidates"] == [{"label": "vpn", "address": "100.64.0.5:7657"}]
    assert data["vpn"]["active"] is None


def test_no_secrets_in_response(link_env, monkeypatch) -> None:
    env = link_env()
    _write_config(env, link={"totp": "TOPSECRET_TOTP_VALUE"})
    _write_service_token(env, "TOPSECRET_TOKEN_VALUE")
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")

    data = _get_status(env)
    serialized = json.dumps(data).lower()

    for forbidden in (
        "topsecret_token_value",
        "token",
        "totp",
        "attestation",
        "account_token",
        "service_token",
    ):
        assert forbidden not in serialized


def test_back_compat_field_set(link_env, monkeypatch) -> None:
    env = link_env()
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")

    data = _get_status(env)

    assert set(data) == {
        "instance_id",
        "home_label",
        "enrolled",
        "relay_url",
        "ca_fingerprint",
        "has_password",
        "lan_accessible",
        "posture",
        "reachability",
        "relay_state",
        "home_address",
        "vpn",
    }
    assert isinstance(data["instance_id"], str)
    assert isinstance(data["home_label"], str)
    assert isinstance(data["enrolled"], bool)
    assert isinstance(data["relay_url"], str)
    assert isinstance(data["ca_fingerprint"], str) or data["ca_fingerprint"] is None
    assert isinstance(data["has_password"], bool)
    assert isinstance(data["lan_accessible"], bool)


def test_status_reports_convey_password_state(link_env, monkeypatch) -> None:
    env = link_env()
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")

    data = _get_status(env)
    assert data["has_password"] is False

    config_path = env.journal / "config" / "journal.json"
    config = json.loads(config_path.read_text("utf-8"))
    config.setdefault("convey", {})["password_hash"] = "hashed"
    config_path.write_text(json.dumps(config, indent=2), encoding="utf-8")

    data = _get_status(env)
    assert data["has_password"] is True
