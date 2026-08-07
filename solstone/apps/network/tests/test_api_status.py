# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
from typing import Any

import pytest

import solstone.convey.bridge as convey_bridge
from solstone.apps.network import copy as link_copy
from solstone.apps.network import routes as link_routes
from solstone.apps.network.tests.conftest import _StubWatcher
from solstone.think.link.link_health import (
    REASON_HOME_MISSING_MOBILE,
    REASON_LOCAL_PRIVATE_LISTENER_UNREACHABLE,
    REASON_RELAY_TUNNEL_REJECTED,
    REASON_RELAY_TUNNEL_UNREACHABLE,
    REASON_SERVICE_TOKEN_REJECTED,
)
from solstone.think.link.local_endpoints import LocalEndpoint
from solstone.think.link.paths import LinkState
from solstone.think.link.window import read_posture

NOW = 1_700_000_000_000
STATUS_FIELD_SET = {
    "instance_id",
    "home_label",
    "enrolled",
    "relay_url",
    "ca_fingerprint",
    "lan_accessible",
    "posture",
    "reachability",
    "relay_state",
    "last_link_event_at",
    "relay_listen_generation",
    "last_successful_relay_tunnel_at",
    "last_relay_tunnel_error",
    "last_relay_tunnel_error_at",
    "last_relay_listener_ack_at",
    "last_relay_listener_ack_generation",
    "home_address",
    "vpn",
    "home_candidates",
    "home_candidates_state",
    "home_candidates_error",
}


@pytest.fixture(autouse=True)
def clear_link_health_cache() -> None:
    convey_bridge._STATE_CACHE["link_health"] = None
    yield
    convey_bridge._STATE_CACHE["link_health"] = None


def _health(
    *,
    state: str = "connected",
    ts: int = NOW,
    generation: int = 1,
    success_at: int | None = None,
    error: str | None = None,
    error_at: int | None = None,
    ack_at: int | None = None,
    ack_generation: int | None = None,
) -> dict[str, Any]:
    return {
        "state": state,
        "listen_generation": generation,
        "last_successful_relay_tunnel_at": success_at,
        "last_relay_tunnel_error": error,
        "last_relay_tunnel_error_at": error_at,
        "relay_tunnel_error_status": None,
        "last_relay_listener_ack_at": ack_at,
        "last_relay_listener_ack_generation": ack_generation,
        "ts": ts,
    }


def _write_config(env: Any, *, link: Any = None, include_link: bool = True) -> None:
    config: dict[str, Any] = {
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


def _write_home_address(env: Any, address: str) -> None:
    config_path = env.journal / "config" / "journal.json"
    config = json.loads(config_path.read_text("utf-8"))
    config["pairing"] = {"home_address": address}
    config_path.write_text(json.dumps(config, indent=2), encoding="utf-8")


def _get_status(env: Any) -> dict[str, Any]:
    response = env.client.get(
        "/app/network/api/status",
        base_url="http://localhost:7657",
    )
    assert response.status_code == 200
    payload = response.get_json()
    assert isinstance(payload, dict)
    return payload


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
    assert data["home_address"] is None
    assert data["posture"] == "direct"
    assert data["reachability"] == "online"
    assert data["relay_state"] == "not-enrolled"


def test_direct_reports_manual_home_address(link_env, monkeypatch) -> None:
    env = link_env()
    _write_home_address(env, "192.168.1.44:7657")
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")

    data = _get_status(env)

    assert data["lan_accessible"] is True
    assert data["home_address"] == "192.168.1.44:7657"
    assert data["reachability"] == "online"


def test_host_address_override_unblocks_lan_unreachable(link_env, monkeypatch) -> None:
    env = link_env()
    _write_home_address(env, "192.168.1.44:7657")
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: None)

    data = _get_status(env)

    assert data["lan_accessible"] is True
    assert data["home_address"] == "192.168.1.44:7657"
    assert data["reachability"] == "online"


def test_loopback_only_is_lan_unreachable(link_env, monkeypatch) -> None:
    env = link_env(local_endpoints=[])
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: None)

    data = _get_status(env)

    assert data["lan_accessible"] is False
    assert data["home_address"] is None
    assert data["reachability"] == "lan-unreachable"


def test_empty_snapshot_route_fallback_reports_online(link_env, monkeypatch) -> None:
    env = link_env(local_endpoints=[])
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")

    data = _get_status(env)

    assert data["lan_accessible"] is True
    assert data["home_address"] is None
    assert data["reachability"] == "online"
    assert data["home_candidates"] == [
        {"address": "192.168.1.50:7657", "selected": True, "source": "detected"}
    ]
    assert data["home_candidates_state"] == "ready"
    assert data["home_candidates_error"] is None


def test_lan_unreachable_precedence_over_spl(link_env, monkeypatch) -> None:
    env = link_env(local_endpoints=[])
    _write_config(env, link={"posture": "spl"})
    _write_service_token(env)
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: None)
    monkeypatch.setattr(link_routes, "now_ms", lambda: NOW)
    monkeypatch.setattr(link_routes, "_read_link_health", lambda: _health())

    data = _get_status(env)

    assert data["enrolled"] is True
    assert data["relay_state"] == "parked"
    assert data["reachability"] == "lan-unreachable"
    assert data["last_link_event_at"] == NOW
    assert data["relay_listen_generation"] == 1


def test_api_status_projects_relay_listener_ack_fields(link_env, monkeypatch) -> None:
    env = link_env(local_endpoints=[])
    _write_config(env, link={"posture": "spl"})
    _write_service_token(env)
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")
    monkeypatch.setattr(link_routes, "now_ms", lambda: NOW)
    monkeypatch.setattr(
        link_routes,
        "_read_link_health",
        lambda: _health(ack_at=NOW - 20, ack_generation=3),
    )

    data = _get_status(env)

    assert data["last_relay_listener_ack_at"] == NOW - 20
    assert data["last_relay_listener_ack_generation"] == 3


def test_relay_state_helper() -> None:
    assert link_routes._derive_relay_state(False) == "not-enrolled"
    assert link_routes._derive_relay_state(True) == "offline"


def test_spl_reachability_mapping() -> None:
    assert (
        link_routes._derive_reachability(True, "spl", "connecting") == "finishing-setup"
    )
    assert link_routes._derive_reachability(True, "spl", "parked") == "online"
    assert (
        link_routes._derive_reachability(True, "spl", "reconnecting") == "reconnecting"
    )
    assert link_routes._derive_reachability(True, "spl", "offline") == "offline"
    assert (
        link_routes._derive_reachability(True, "spl", "not-enrolled")
        == "finishing-setup"
    )
    assert link_routes._derive_reachability(True, "direct", "offline") == "online"
    assert link_routes._derive_reachability(False, "spl", "parked") == "lan-unreachable"


def test_spl_relay_state_never_parks_without_connected() -> None:
    assert link_routes._derive_spl_relay_state(False, _health(), NOW) == "not-enrolled"
    assert link_routes._derive_spl_relay_state(True, None, NOW) == "connecting"
    assert (
        link_routes._derive_spl_relay_state(
            True,
            _health(ts=NOW - 200_000),
            NOW,
        )
        == "offline"
    )
    assert (
        link_routes._derive_spl_relay_state(
            True,
            _health(state="reconnecting"),
            NOW,
        )
        == "reconnecting"
    )
    assert link_routes._derive_spl_relay_state(True, _health(), NOW) == "parked"


def test_spl_relay_state_stays_connecting_for_new_generation_before_ack() -> None:
    health = _health(
        state="connecting",
        generation=2,
        ack_at=NOW - 1,
        ack_generation=1,
    )

    assert link_routes._derive_spl_relay_state(True, health, NOW) == "connecting"


def test_current_tunnel_error_ignores_error_older_than_success() -> None:
    health = _health(
        success_at=NOW,
        error=REASON_SERVICE_TOKEN_REJECTED,
        error_at=NOW - 1,
    )

    assert link_routes._current_tunnel_error(health) is None
    assert link_routes._derive_spl_relay_state(True, health, NOW) == "parked"


@pytest.mark.parametrize(
    "reason",
    [
        REASON_HOME_MISSING_MOBILE,
        REASON_RELAY_TUNNEL_REJECTED,
        REASON_RELAY_TUNNEL_UNREACHABLE,
    ],
)
def test_non_forcing_tunnel_errors_do_not_force_offline(reason: str) -> None:
    health = _health(success_at=NOW - 10, error=reason, error_at=NOW)

    assert link_routes._current_tunnel_error(health) == reason
    assert link_routes._derive_spl_relay_state(True, health, NOW) == "parked"


@pytest.mark.parametrize(
    "reason",
    [
        REASON_SERVICE_TOKEN_REJECTED,
        REASON_LOCAL_PRIVATE_LISTENER_UNREACHABLE,
    ],
)
def test_forcing_tunnel_errors_force_relay_offline(reason: str) -> None:
    health = _health(success_at=NOW - 10, error=reason, error_at=NOW)

    assert link_routes._current_tunnel_error(health) == reason
    assert link_routes._derive_spl_relay_state(True, health, NOW) == "offline"


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


def test_home_candidates_ready_empty_when_no_detected_addresses(
    link_env,
    monkeypatch,
) -> None:
    env = link_env(local_endpoints=[])
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: None)

    data = _get_status(env)

    assert data["home_candidates"] == []
    assert data["home_candidates_state"] == "ready"
    assert data["home_candidates_error"] is None


def test_home_candidates_single_detected_selected(link_env, monkeypatch) -> None:
    env = link_env(
        local_endpoints=[LocalEndpoint(ip="192.168.1.50", port=1111, scope="lan")]
    )
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")

    data = _get_status(env)

    assert data["home_candidates"] == [
        {"address": "192.168.1.50:7657", "selected": True, "source": "detected"}
    ]
    assert data["home_candidates_state"] == "ready"
    assert data["home_candidates_error"] is None


def test_home_candidates_route_first_dedupes_and_excludes_ipv6(
    link_env,
    monkeypatch,
) -> None:
    env = link_env(
        local_endpoints=[
            LocalEndpoint(ip="192.0.2.10", port=1111, scope="lan"),
            LocalEndpoint(ip="fd00::1", port=7657, scope="ula"),
            LocalEndpoint(ip="192.0.2.11", port=2222, scope="lan"),
            LocalEndpoint(ip="192.0.2.10", port=3333, scope="lan"),
        ]
    )
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.0.2.11")

    data = _get_status(env)

    assert data["home_candidates"] == [
        {"address": "192.0.2.11:7657", "selected": True, "source": "detected"},
        {"address": "192.0.2.10:7657", "selected": False, "source": "detected"},
    ]


def test_home_candidates_override_in_detected_selects_detected(
    link_env,
    monkeypatch,
) -> None:
    env = link_env(
        local_endpoints=[
            LocalEndpoint(ip="192.168.1.50", port=7657, scope="lan"),
            LocalEndpoint(ip="192.168.1.51", port=7657, scope="lan"),
        ]
    )
    _write_home_address(env, "192.168.1.51:7657")
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")

    data = _get_status(env)

    assert data["home_candidates"] == [
        {"address": "192.168.1.50:7657", "selected": False, "source": "detected"},
        {"address": "192.168.1.51:7657", "selected": True, "source": "detected"},
    ]


def test_home_candidates_override_not_detected_appends_override(
    link_env,
    monkeypatch,
) -> None:
    env = link_env(
        local_endpoints=[LocalEndpoint(ip="192.168.1.50", port=7657, scope="lan")]
    )
    _write_home_address(env, "192.168.1.44:7657")
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")

    data = _get_status(env)

    assert data["home_candidates"] == [
        {"address": "192.168.1.50:7657", "selected": False, "source": "detected"},
        {"address": "192.168.1.44:7657", "selected": True, "source": "override"},
    ]


def test_home_candidates_unavailable_keeps_status_200(
    link_env,
    monkeypatch,
) -> None:
    env = link_env()
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")

    def fail_candidates(endpoints: list[LocalEndpoint]) -> list[str]:
        raise RuntimeError("watcher exploded")

    monkeypatch.setattr(link_routes, "_resolve_pair_link_candidates", fail_candidates)

    data = _get_status(env)

    assert data["home_candidates"] == []
    assert data["home_candidates_state"] == "unavailable"
    assert data["home_candidates_error"] == link_copy.HOME_CANDIDATES_ERROR
    assert data["reachability"] == "lan-unreachable"


def test_home_candidates_exception_with_override_stays_usable(
    link_env,
    monkeypatch,
) -> None:
    env = link_env()
    _write_home_address(env, "192.168.1.44:7657")

    def fail_candidates(endpoints: list[LocalEndpoint]) -> list[str]:
        raise RuntimeError("watcher exploded")

    monkeypatch.setattr(link_routes, "_resolve_pair_link_candidates", fail_candidates)

    data = _get_status(env)

    assert data["lan_accessible"] is True
    assert data["home_address"] == "192.168.1.44:7657"
    assert data["reachability"] == "online"
    assert data["home_candidates"] == [
        {"address": "192.168.1.44:7657", "selected": True, "source": "override"}
    ]
    assert data["home_candidates_state"] == "ready"
    assert data["home_candidates_error"] is None


def test_api_status_does_not_mint_pairing_nonces(link_env, monkeypatch) -> None:
    env = link_env()
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")
    nonce_path = env.journal / "link" / "nonces.json"
    assert not nonce_path.exists()

    _get_status(env)

    assert not nonce_path.exists()


def test_api_status_does_not_write_journal_config(link_env, monkeypatch) -> None:
    env = link_env()
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")
    config_path = env.journal / "config" / "journal.json"
    config_path.write_text(
        json.dumps(
            {
                "setup": {"completed_at": 1700000000000},
                "pairing": {"home_address": "192.168.1.44:7657"},
            },
            indent=2,
        ),
        encoding="utf-8",
    )
    before = config_path.read_bytes()

    _get_status(env)

    assert config_path.read_bytes() == before


def test_no_secrets_in_response(link_env, monkeypatch) -> None:
    env = link_env()
    _write_service_token(env, "TOPSECRET_TOKEN_VALUE")
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")

    data = _get_status(env)
    serialized = json.dumps(data).lower()

    for forbidden in (
        "topsecret_token_value",
        "token",
        "attestation",
        "account_token",
        "service_token",
    ):
        assert forbidden not in serialized


def test_back_compat_field_set(link_env, monkeypatch) -> None:
    env = link_env()
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")

    data = _get_status(env)

    assert set(data) == STATUS_FIELD_SET
    assert isinstance(data["instance_id"], str)
    assert isinstance(data["home_label"], str)
    assert isinstance(data["enrolled"], bool)
    assert isinstance(data["relay_url"], str)
    assert isinstance(data["ca_fingerprint"], str) or data["ca_fingerprint"] is None
    assert isinstance(data["lan_accessible"], bool)
    assert data["last_relay_listener_ack_at"] is None
    assert data["last_relay_listener_ack_generation"] is None


def test_api_status_unprovisioned(link_env, monkeypatch) -> None:
    env = link_env(provision=False)
    monkeypatch.setattr(link_routes, "_detect_lan_ip", lambda: "192.168.1.50")

    def fail_save(self) -> None:
        raise AssertionError("LinkState.save should not be called by status")

    monkeypatch.setattr(LinkState, "save", fail_save)
    assert not (env.journal / "link" / "state.json").exists()

    data = _get_status(env)

    assert set(data) == STATUS_FIELD_SET
    assert data["instance_id"] is None
    assert data["home_label"] is None
    assert data["last_relay_listener_ack_at"] is None
    assert data["last_relay_listener_ack_generation"] is None
    assert not (env.journal / "link" / "state.json").exists()
