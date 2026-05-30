# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for Convey Callosum SSE route and bridge fan-out."""

from __future__ import annotations

import json
import time
from collections.abc import Callable, Iterator

import pytest

import solstone.convey.bridge as convey_bridge


@pytest.fixture(autouse=True)
def clear_sse_subscribers() -> Iterator[None]:
    with convey_bridge._SSE_LOCK:
        convey_bridge._SSE_SUBSCRIBERS_BY_KEY.clear()
        convey_bridge._SSE_LAST_CHAT_REQUEST_AT_BY_KEY.clear()
    convey_bridge._STATE_CACHE["link_connection"] = None
    yield
    with convey_bridge._SSE_LOCK:
        convey_bridge._SSE_SUBSCRIBERS_BY_KEY.clear()
        convey_bridge._SSE_LAST_CHAT_REQUEST_AT_BY_KEY.clear()
    convey_bridge._STATE_CACHE["link_connection"] = None


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


def _wait_until(predicate: Callable[[], bool], timeout: float = 1.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.01)
    raise AssertionError("timed out waiting for condition")


def test_callosum_sse_success_headers(convey_env):
    env = convey_env()

    resp = env.client.get("/sse/events", buffered=False)
    try:
        assert resp.status_code == 200
        assert resp.content_type.startswith("text/event-stream")
        assert resp.headers["Cache-Control"] == "no-cache"
        assert resp.headers["X-Accel-Buffering"] == "no"
    finally:
        resp.close()


def test_callosum_sse_unauthenticated_redirects_to_login(convey_env):
    env = convey_env()

    resp = env.client.get(
        "/sse/events",
        headers={"X-Forwarded-For": "1.2.3.4"},
    )

    assert resp.status_code == 302
    assert "/login" in resp.headers["Location"]


def test_callosum_sse_round_trip_payload(convey_env):
    env = convey_env()
    resp = env.client.get("/sse/events", buffered=False)
    try:
        assert resp.status_code == 200
        assert _next_chunk(resp) == ": heartbeat\n\n"
        assert convey_bridge.subscription_count("convey-ui") == 1
        message = {"tract": "test", "event": "ping", "ts": 0, "extra": "value"}
        convey_bridge._broadcast_callosum_event(message)

        parsed = _next_data(resp)
        assert parsed == message
    finally:
        resp.close()


def test_bridge_caches_link_connection_events_only() -> None:
    for event in ("connecting", "connected", "disconnect"):
        convey_bridge._broadcast_callosum_event({"tract": "link", "event": event})
        assert convey_bridge.get_cached_state()["link_connection"] == event

    convey_bridge._broadcast_callosum_event({"tract": "link", "event": "enrolled"})
    assert convey_bridge.get_cached_state()["link_connection"] == "disconnect"

    convey_bridge._broadcast_callosum_event({"tract": "link", "event": "tunnel_pair"})
    assert convey_bridge.get_cached_state()["link_connection"] == "disconnect"

    convey_bridge._broadcast_callosum_event({"tract": "link", "event": "tunnel_close"})
    assert convey_bridge.get_cached_state()["link_connection"] == "disconnect"


def test_callosum_sse_multi_client_fanout(convey_env):
    env = convey_env()
    first_client = env.app.test_client()
    second_client = env.app.test_client()
    first = first_client.get("/sse/events", buffered=False)
    second = second_client.get("/sse/events", buffered=False)
    try:
        assert _next_chunk(first) == ": heartbeat\n\n"
        assert _next_chunk(second) == ": heartbeat\n\n"
        assert convey_bridge.subscription_count("convey-ui") == 2

        message = {"tract": "test", "event": "fanout", "ts": 1}
        convey_bridge._broadcast_callosum_event(message)

        assert _next_data(first) == message
        assert _next_data(second) == message
    finally:
        second.close()
        first.close()


def test_slow_sse_subscriber_is_dropped_without_blocking_healthy_subscriber():
    slow = convey_bridge.register_sse_subscriber("convey-ui")
    healthy = convey_bridge.register_sse_subscriber("convey-ui")
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
    assert slow not in convey_bridge._SSE_SUBSCRIBERS_BY_KEY["convey-ui"]
    assert len(received) == convey_bridge._SSE_QUEUE_MAXSIZE + 1
    assert [message["ts"] for message in received] == list(
        range(convey_bridge._SSE_QUEUE_MAXSIZE + 1)
    )
    assert elapsed < 0.5

    convey_bridge.unregister_sse_subscriber(healthy)


def test_callosum_sse_dropped_subscriber_breaks_stream(convey_env):
    env = convey_env()
    resp = env.client.get("/sse/events", buffered=False)
    extra = None
    try:
        assert _next_chunk(resp) == ": heartbeat\n\n"
        extra = convey_bridge.register_sse_subscriber("convey-ui")
        with convey_bridge._SSE_LOCK:
            subscribers = set(convey_bridge._SSE_SUBSCRIBERS_BY_KEY["convey-ui"])
        route_handles = [handle for handle in subscribers if handle is not extra]
        assert len(route_handles) == 1
        route_handle = route_handles[0]

        for i in range(convey_bridge._SSE_QUEUE_MAXSIZE + 1):
            convey_bridge._broadcast_callosum_event(
                {"tract": "test", "event": "overflow", "ts": i}
            )

        assert route_handle.dropped.is_set()
        assert route_handle not in convey_bridge._SSE_SUBSCRIBERS_BY_KEY.get(
            "convey-ui", set()
        )
        with pytest.raises(StopIteration):
            next(iter(resp.response))
    finally:
        if extra is not None:
            convey_bridge.unregister_sse_subscriber(extra)
        resp.close()


def test_callosum_sse_clean_disconnect_unregisters_subscriber(convey_env):
    env = convey_env()
    resp = env.client.get("/sse/events", buffered=False)

    assert _next_chunk(resp) == ": heartbeat\n\n"
    assert convey_bridge.subscription_count("convey-ui") == 1

    resp.close()

    _wait_until(lambda: convey_bridge.subscription_count("convey-ui") == 0)


def test_callosum_sse_heartbeat(convey_env, monkeypatch):
    env = convey_env()
    monkeypatch.setattr(convey_bridge, "_SSE_HEARTBEAT_SECONDS", 0.01)

    resp = env.client.get("/sse/events", buffered=False)
    try:
        assert resp.status_code == 200
        assert _next_chunk(resp) == ": heartbeat\n\n"
        assert _next_chunk(resp) == ": heartbeat\n\n"
    finally:
        resp.close()
