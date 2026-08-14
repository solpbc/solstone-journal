# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for observer Callosum SSE route and bridge fan-out."""

from __future__ import annotations

import json
import time
from collections.abc import Iterator

import pytest

import solstone.apps.observer.routes as routes_module
import solstone.convey.bridge as convey_bridge
import solstone.convey.root as root_module
from solstone.apps.observer.routes import OBSERVER_CALLOSUM_SSE_ROUTE
from solstone.apps.observer.utils import (
    load_observer,
    save_observer,
)
from solstone.convey.secure_listener import ConveyIdentity
from solstone.convey.sol_initiated.copy import KIND_SOL_CHAT_REQUEST
from solstone.observe.protocol import OBSERVER_HANDLE_HEADER
from solstone.think.link.auth import AuthorizedClients
from solstone.think.link.paths import authorized_clients_path

DEVICE_FINGERPRINT = "sha256:" + ("c" * 64)


@pytest.fixture(autouse=True)
def clear_sse_subscribers() -> Iterator[None]:
    with convey_bridge._SSE_LOCK:
        convey_bridge._SSE_SUBSCRIBERS_BY_KEY.clear()
        convey_bridge._SSE_LAST_CHAT_REQUEST_AT_BY_KEY.clear()
    yield
    with convey_bridge._SSE_LOCK:
        convey_bridge._SSE_SUBSCRIBERS_BY_KEY.clear()
        convey_bridge._SSE_LAST_CHAT_REQUEST_AT_BY_KEY.clear()


def _pl_identity(fingerprint: str = DEVICE_FINGERPRINT) -> ConveyIdentity:
    return ConveyIdentity(
        mode="pl-direct",
        fingerprint=fingerprint,
        device_label="sse-device",
        paired_at="2026-07-01T00:00:00Z",
        session_id=None,
    )


def _authorize_device(fingerprint: str = DEVICE_FINGERPRINT) -> None:
    AuthorizedClients(authorized_clients_path()).add(
        fingerprint,
        "sse-device",
        "instance-1",
    )


def _create_bound_observer(env, name: str = "sse-bound") -> tuple[str, str]:
    key = f"{name}-key-123456789"
    assert save_observer(
        {
            "key": key,
            "name": name,
            "created_at": 1,
            "enabled": True,
            "revoked": False,
            "device_binding": {"device": DEVICE_FINGERPRINT, "kind": "cert"},
            "stats": {"segments_received": 0, "bytes_received": 0},
        }
    )
    return key, key[:8]


def _create_unbound_observer(env, name: str = "sse-unbound") -> tuple[str, str]:
    key = f"{name}-key-123456789"
    assert save_observer(
        {
            "key": key,
            "name": name,
            "created_at": 1,
            "enabled": True,
            "revoked": False,
            "stats": {"segments_received": 0, "bytes_received": 0},
        }
    )
    return key, key[:8]


def _create_null_binding_observer(
    env,
    name: str = "sse-null-binding",
) -> tuple[str, str]:
    key = f"{name}-key-123456789"
    assert save_observer(
        {
            "key": key,
            "name": name,
            "created_at": 1,
            "enabled": True,
            "revoked": False,
            "device_binding": None,
            "stats": {"segments_received": 0, "bytes_received": 0},
        }
    )
    return key, key[:8]


def _route() -> str:
    return OBSERVER_CALLOSUM_SSE_ROUTE


def _next_chunk(response) -> str:
    chunk = next(iter(response.response))
    if isinstance(chunk, bytes):
        return chunk.decode("utf-8")
    return str(chunk)


def _parse_sse_data(chunk: str) -> dict:
    for line in chunk.splitlines():
        if line.startswith("data: "):
            return json.loads(line[len("data: ") :])
    raise AssertionError(f"No data line found in chunk: {chunk!r}")


def _next_data(response) -> dict:
    for chunk in response.response:
        text = chunk.decode("utf-8") if isinstance(chunk, bytes) else str(chunk)
        if "data: " in text:
            return _parse_sse_data(text)
    raise AssertionError("SSE stream ended before a data frame was received")


def _assert_reason(response, *, reason_code: str, detail: str) -> None:
    data = response.get_json()
    assert data["reason_code"] == reason_code
    assert data["detail"] == detail


def test_callosum_sse_missing_bearer_returns_401(observer_env):
    env = observer_env()
    with env.app.test_request_context(_route()):
        response, status = routes_module.callosum_sse()
    assert status == 401
    _assert_reason(
        response,
        reason_code="auth_required",
        detail="Authorization required",
    )


def test_callosum_sse_without_bearer_returns_401(observer_env):
    env = observer_env()
    resp = env.client.get(_route(), buffered=False)
    assert resp.status_code == 401
    _assert_reason(
        resp,
        reason_code="auth_required",
        detail="Authorization required",
    )


def test_legacy_keyed_callosum_path_is_gone(observer_env, monkeypatch):
    env = observer_env()
    _authorize_device()
    monkeypatch.setattr(
        root_module,
        "get_authorized_clients",
        lambda: AuthorizedClients(authorized_clients_path()),
    )
    key, _ = _create_bound_observer(env)

    resp = env.client.get(
        f"/app/observer/{key}/callosum",
        headers={"Authorization": f"Bearer {key}"},
        environ_overrides={"pl.identity": _pl_identity()},
        buffered=False,
    )
    assert resp.status_code == 404


def test_callosum_sse_revoked_key_returns_403(observer_env, monkeypatch):
    env = observer_env()
    _authorize_device()
    monkeypatch.setattr(
        root_module,
        "get_authorized_clients",
        lambda: AuthorizedClients(authorized_clients_path()),
    )
    key, key_prefix = _create_bound_observer(env, "revoked-sse")
    from solstone.apps.observer.utils import revoke_observer_record

    revoke_observer_record(key_prefix)

    resp = env.client.get(
        _route(),
        headers={"Authorization": f"Bearer {key}"},
        environ_overrides={"pl.identity": _pl_identity()},
        buffered=False,
    )
    assert resp.status_code == 403
    _assert_reason(
        resp,
        reason_code="pl_revoked",
        detail="Observer revoked",
    )


def test_callosum_sse_disabled_key_returns_403(observer_env, monkeypatch):
    env = observer_env()
    _authorize_device()
    monkeypatch.setattr(
        root_module,
        "get_authorized_clients",
        lambda: AuthorizedClients(authorized_clients_path()),
    )
    key, _ = _create_bound_observer(env, "disabled-sse")
    observer = load_observer(key)
    assert observer is not None
    observer["enabled"] = False
    assert save_observer(observer)

    resp = env.client.get(
        _route(),
        headers={"Authorization": f"Bearer {key}"},
        environ_overrides={"pl.identity": _pl_identity()},
        buffered=False,
    )
    assert resp.status_code == 403
    _assert_reason(
        resp,
        reason_code="feature_unavailable",
        detail="Observer disabled",
    )


def test_callosum_sse_bearer_header_authenticates(observer_env, monkeypatch):
    env = observer_env()
    _authorize_device()
    monkeypatch.setattr(
        root_module,
        "get_authorized_clients",
        lambda: AuthorizedClients(authorized_clients_path()),
    )
    valid_key, _ = _create_bound_observer(env, "valid-sse")

    resp = env.client.get(
        _route(),
        headers={"Authorization": f"Bearer {valid_key}"},
        environ_overrides={"pl.identity": _pl_identity()},
        buffered=False,
    )
    try:
        assert resp.status_code == 200
        assert resp.content_type.startswith("text/event-stream")
    finally:
        resp.close()

    resp = env.client.get(
        _route(),
        headers={"Authorization": "Bearer invalid-key"},
        environ_overrides={"pl.identity": _pl_identity()},
        buffered=False,
    )
    assert resp.status_code == 401
    _assert_reason(resp, reason_code="auth_key_invalid", detail="Invalid key")


def test_callosum_sse_success_content_type(observer_env, monkeypatch):
    env = observer_env()
    _authorize_device()
    monkeypatch.setattr(
        root_module,
        "get_authorized_clients",
        lambda: AuthorizedClients(authorized_clients_path()),
    )
    key, _ = _create_bound_observer(env)

    resp = env.client.get(
        _route(),
        headers={"Authorization": f"Bearer {key}"},
        environ_overrides={"pl.identity": _pl_identity()},
        buffered=False,
    )
    try:
        assert resp.status_code == 200
        assert resp.content_type.startswith("text/event-stream")
    finally:
        resp.close()


def test_callosum_sse_round_trip_payload(observer_env, monkeypatch):
    env = observer_env()
    _authorize_device()
    monkeypatch.setattr(
        root_module,
        "get_authorized_clients",
        lambda: AuthorizedClients(authorized_clients_path()),
    )
    key, key_prefix = _create_bound_observer(env)
    resp = env.client.get(
        _route(),
        headers={"Authorization": f"Bearer {key}"},
        environ_overrides={"pl.identity": _pl_identity()},
        buffered=False,
    )
    try:
        assert resp.status_code == 200
        assert convey_bridge.subscription_count(key_prefix) == 1
        message = {"tract": "test", "event": "ping", "ts": 0, "extra": "value"}
        convey_bridge._broadcast_callosum_event(message)

        parsed = _next_data(resp)
        assert parsed == message
    finally:
        resp.close()


def test_callosum_sse_handle_revocation_midstream_emits_error(
    observer_env,
    monkeypatch,
):
    env = observer_env()
    _authorize_device()
    monkeypatch.setattr(
        root_module,
        "get_authorized_clients",
        lambda: AuthorizedClients(authorized_clients_path()),
    )
    key, _key_prefix = _create_bound_observer(env, "handle-sse")
    monkeypatch.setattr(routes_module, "_SSE_HEARTBEAT_SECONDS", 0.01)
    monkeypatch.setattr(
        routes_module, "_SSE_DEVICE_RECHECK_SECONDS", 0.01, raising=False
    )

    resp = env.client.get(
        _route(),
        headers={OBSERVER_HANDLE_HEADER: key},
        environ_overrides={"pl.identity": _pl_identity()},
        buffered=False,
    )
    try:
        assert resp.status_code == 200
        assert _next_chunk(resp) == ": heartbeat\n\n"
        observer = load_observer(key)
        assert observer is not None
        observer["revoked"] = True
        assert save_observer(observer)

        chunk = _next_chunk(resp)
        assert chunk.startswith("event: error\n")
        data = _parse_sse_data(chunk)
        assert data["reason_code"] == "pl_revoked"
        assert data["detail"] == "Observer revoked"
        with pytest.raises(StopIteration):
            next(iter(resp.response))
    finally:
        resp.close()


def test_unbound_callosum_sse_survives_device_rechecks(observer_env, monkeypatch):
    env = observer_env()
    key, _key_prefix = _create_unbound_observer(env, "unbound-steady-sse")
    monkeypatch.setattr(routes_module, "_SSE_HEARTBEAT_SECONDS", 0.01)
    monkeypatch.setattr(
        routes_module, "_SSE_DEVICE_RECHECK_SECONDS", 0.01, raising=False
    )

    resp = env.client.get(
        _route(),
        headers={OBSERVER_HANDLE_HEADER: key},
        buffered=False,
    )
    try:
        assert resp.status_code == 200
        assert _next_chunk(resp) == ": heartbeat\n\n"
        for _ in range(2):
            chunk = _next_chunk(resp)
            assert not chunk.startswith("event: error\n")
    finally:
        resp.close()


def test_callosum_sse_refuses_present_null_device_binding(observer_env):
    env = observer_env()
    key, key_prefix = _create_null_binding_observer(env)

    resp = env.client.get(
        _route(),
        headers={OBSERVER_HANDLE_HEADER: key},
        buffered=False,
    )
    try:
        assert resp.status_code == 403
        _assert_reason(
            resp,
            reason_code="pl_revoked",
            detail="Paired device revoked",
        )
        assert not resp.content_type.startswith("text/event-stream")
        assert b"event: " not in resp.get_data()
        assert convey_bridge.subscription_count(key_prefix) == 0
    finally:
        resp.close()


def test_unbound_callosum_sse_revoked_record_closes_midstream(
    observer_env,
    monkeypatch,
):
    env = observer_env()
    key, _key_prefix = _create_unbound_observer(env, "unbound-revoked-sse")
    monkeypatch.setattr(routes_module, "_SSE_HEARTBEAT_SECONDS", 60)
    monkeypatch.setattr(
        routes_module, "_SSE_DEVICE_RECHECK_SECONDS", 0.01, raising=False
    )

    resp = env.client.get(
        _route(),
        headers={OBSERVER_HANDLE_HEADER: key},
        buffered=False,
    )
    try:
        assert resp.status_code == 200
        assert _next_chunk(resp) == ": heartbeat\n\n"
        observer = load_observer(key)
        assert observer is not None
        observer["revoked"] = True
        assert save_observer(observer)

        chunk = _next_chunk(resp)
        assert chunk.startswith("event: error\n")
        data = _parse_sse_data(chunk)
        assert data["reason_code"] == "pl_revoked"
        assert data["detail"] == "Observer revoked"
        with pytest.raises(StopIteration):
            next(iter(resp.response))
    finally:
        resp.close()


def test_busy_sse_closes_when_bound_device_removed(observer_env, monkeypatch):
    env = observer_env()
    _authorize_device()
    monkeypatch.setattr(
        root_module,
        "get_authorized_clients",
        lambda: AuthorizedClients(authorized_clients_path()),
    )
    key, _key_prefix = _create_bound_observer(env, "busy-sse")
    monkeypatch.setattr(routes_module, "_SSE_HEARTBEAT_SECONDS", 60)
    monkeypatch.setattr(
        routes_module, "_SSE_DEVICE_RECHECK_SECONDS", 0.01, raising=False
    )

    resp = env.client.get(
        _route(),
        headers={OBSERVER_HANDLE_HEADER: key},
        environ_overrides={"pl.identity": _pl_identity()},
        buffered=False,
    )
    try:
        assert resp.status_code == 200
        assert _next_chunk(resp) == ": heartbeat\n\n"
        AuthorizedClients(authorized_clients_path()).remove(DEVICE_FINGERPRINT)
        time.sleep(0.02)
        convey_bridge._broadcast_callosum_event(
            {"tract": "test", "event": "busy", "ts": 1}
        )

        chunk = _next_chunk(resp)
        assert chunk.startswith("event: error\n")
        data = _parse_sse_data(chunk)
        assert data["reason_code"] == "pl_revoked"

        observer = load_observer(key)
        assert observer is not None
        assert observer.get("revoked") is False
        assert observer.get("enabled") is True
    finally:
        resp.close()


def test_callosum_sse_heartbeat(observer_env, monkeypatch):
    env = observer_env()
    _authorize_device()
    monkeypatch.setattr(
        root_module,
        "get_authorized_clients",
        lambda: AuthorizedClients(authorized_clients_path()),
    )
    key, _ = _create_bound_observer(env)
    monkeypatch.setattr(routes_module, "_SSE_HEARTBEAT_SECONDS", 0.01)

    resp = env.client.get(
        _route(),
        headers={"Authorization": f"Bearer {key}"},
        environ_overrides={"pl.identity": _pl_identity()},
        buffered=False,
    )
    try:
        assert resp.status_code == 200
        assert _next_chunk(resp) == ": heartbeat\n\n"
    finally:
        resp.close()


def test_sse_registry_lifecycle():
    handle = convey_bridge.register_sse_subscriber("aaaaaaaa")
    assert convey_bridge.subscription_count("aaaaaaaa") == 1

    convey_bridge.unregister_sse_subscriber(handle)
    assert convey_bridge.subscription_count("aaaaaaaa") == 0

    convey_bridge.unregister_sse_subscriber(handle)
    assert convey_bridge.subscription_count("aaaaaaaa") == 0


def test_slow_sse_subscriber_is_dropped_without_blocking_healthy_subscriber():
    slow = convey_bridge.register_sse_subscriber("aaaaaaaa")
    healthy = convey_bridge.register_sse_subscriber("bbbbbbbb")
    received: list[dict] = []

    start = time.perf_counter()
    for i in range(convey_bridge._SSE_QUEUE_MAXSIZE + 1):
        convey_bridge._broadcast_callosum_event(
            {"tract": "test", "event": "ping", "ts": i}
        )
        received.append(json.loads(healthy.queue.get_nowait()))
    elapsed = time.perf_counter() - start

    assert slow.dropped.is_set()
    assert slow.drop_reason == "overflow"
    assert "aaaaaaaa" not in convey_bridge._SSE_SUBSCRIBERS_BY_KEY
    assert len(received) == convey_bridge._SSE_QUEUE_MAXSIZE + 1
    assert [message["ts"] for message in received] == list(
        range(convey_bridge._SSE_QUEUE_MAXSIZE + 1)
    )
    assert elapsed < 0.5

    convey_bridge.unregister_sse_subscriber(healthy)


def test_sol_chat_request_delivery_updates_last_request_at() -> None:
    handle = convey_bridge.register_sse_subscriber("aaaaaaaa")
    message = {"tract": "chat", "event": KIND_SOL_CHAT_REQUEST, "ts": 1234}

    convey_bridge._broadcast_to_sse_clients(message)

    assert json.loads(handle.queue.get_nowait()) == message
    assert convey_bridge.last_chat_request_at("aaaaaaaa") == 1234


def test_non_chat_sse_delivery_does_not_update_last_request_at() -> None:
    handle = convey_bridge.register_sse_subscriber("aaaaaaaa")

    convey_bridge._broadcast_to_sse_clients(
        {"tract": "supervisor", "event": KIND_SOL_CHAT_REQUEST, "ts": 1234}
    )

    assert json.loads(handle.queue.get_nowait())["tract"] == "supervisor"
    assert convey_bridge.last_chat_request_at("aaaaaaaa") is None


def test_other_chat_event_does_not_update_last_request_at() -> None:
    handle = convey_bridge.register_sse_subscriber("aaaaaaaa")

    convey_bridge._broadcast_to_sse_clients(
        {"tract": "chat", "event": "owner_message", "ts": 1234}
    )

    assert json.loads(handle.queue.get_nowait())["event"] == "owner_message"
    assert convey_bridge.last_chat_request_at("aaaaaaaa") is None
