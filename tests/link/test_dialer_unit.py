# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Unit tests for paired-link dial orchestration."""

from __future__ import annotations

import asyncio
import queue

import pytest

from solstone.think.link import dialer
from solstone.think.link.client import (
    Client,
    ClientIdentity,
    EnrolledDevice,
    StreamResetError,
    TunnelSession,
    _http_request_bytes,
)
from solstone.think.link.dialer import (
    TunnelClient,
    TunnelRequestError,
    TunnelResponseHead,
)
from solstone.think.link.tls import TlsError


def test_link_client_public_imports() -> None:
    assert Client is not None
    assert ClientIdentity is not None
    assert EnrolledDevice is not None
    assert TunnelSession is not None
    assert TlsError is not None
    assert StreamResetError is not None
    assert _http_request_bytes(
        "GET",
        "/",
        headers={},
        body=b"",
    ).startswith(b"GET / HTTP/1.1\r\n")


def _identity(*, endpoints: tuple[dict[str, object], ...]) -> ClientIdentity:
    return ClientIdentity(
        private_key_pem="private",
        client_cert_pem="cert",
        ca_chain_pem="chain",
        fingerprint="sha256:" + ("a" * 64),
        home_instance_id="instance",
        home_label="home",
        home_attestation="attestation",
        local_endpoints=endpoints,
    )


@pytest.mark.asyncio
async def test_lan_direct_race_picks_first_and_cancels_loser(monkeypatch) -> None:
    identity = _identity(
        endpoints=(
            {"ip": "10.0.0.1", "port": 7657},
            {"ip": "10.0.0.2", "port": 7657},
        )
    )
    cancelled: list[str] = []
    winner = object()

    async def dial_direct(_client, endpoint, _identity, _deadline=None):
        if endpoint["ip"] == "10.0.0.2":
            await asyncio.sleep(0)
            return winner
        try:
            await asyncio.sleep(60)
        except asyncio.CancelledError:
            cancelled.append(str(endpoint["ip"]))
            raise

    monkeypatch.setattr(dialer, "_dial_direct_endpoint", dial_direct)

    assert await dialer.open_tunnel(identity, None) is winner
    assert cancelled == ["10.0.0.1"]


@pytest.mark.asyncio
async def test_all_fail_error_names_every_attempt(monkeypatch) -> None:
    identity = _identity(endpoints=({"ip": "10.0.0.1", "port": 7657},))

    async def dial_direct(_client, _endpoint, _identity, _deadline=None):
        raise TlsError("lan failed")

    async def dial_relay(_client, _relay_url, _identity, _deadline=None):
        raise OSError("relay failed")

    monkeypatch.setattr(dialer, "_dial_direct_endpoint", dial_direct)
    monkeypatch.setattr(dialer, "_dial_relay", dial_relay)

    with pytest.raises(TlsError) as exc_info:
        await dialer.open_tunnel(identity, "https://relay.test")

    message = str(exc_info.value)
    assert "lan-direct 10.0.0.1:7657" in message
    assert "lan failed" in message
    assert "spl-relay" in message
    assert "relay failed" in message


def test_cached_session_drops_on_stream_reset(monkeypatch) -> None:
    class ResetSession:
        def __init__(self) -> None:
            self.closed = False

        async def request(self, *_args, **_kwargs):
            raise StreamResetError("reset")

        async def close(self) -> None:
            self.closed = True

    session = ResetSession()

    async def open_tunnel(_identity, _relay_url):
        return session

    monkeypatch.setattr(dialer, "open_tunnel", open_tunnel)
    client = TunnelClient(_identity(endpoints=()), None)
    try:
        with pytest.raises(TunnelRequestError) as exc_info:
            client.request("GET", "/")
    finally:
        client.close()

    assert exc_info.value.reason == "StreamResetError"
    assert session.closed is True
    assert client._session is None


def test_proxy_stream_request_queues_head_body_and_sentinel(monkeypatch) -> None:
    class FakeStream:
        async def read(self):
            yield b"chunk-a"
            yield b"chunk-b"

    client = TunnelClient(_identity(endpoints=()), None)
    calls = []

    async def fake_stream_request_async(method, path, *, headers, body):
        calls.append((method, path, headers, body))
        return 418, {"x-test": "yes"}, b"initial", FakeStream()

    monkeypatch.setattr(client, "_stream_request_async", fake_stream_request_async)
    chunks: queue.Queue[TunnelResponseHead | bytes | Exception | None] = queue.Queue()
    try:
        future = client.proxy_stream_request(
            "POST",
            "/hello",
            headers={"Host": "example"},
            body=b"payload",
            chunks=chunks,
        )
        future.result(timeout=2)
    finally:
        client.close()

    assert calls == [("POST", "/hello", {"Host": "example"}, b"payload")]
    assert chunks.get_nowait() == TunnelResponseHead(418, {"x-test": "yes"})
    assert chunks.get_nowait() == b"initial"
    assert chunks.get_nowait() == b"chunk-a"
    assert chunks.get_nowait() == b"chunk-b"
    assert chunks.get_nowait() is None


def test_proxy_stream_request_queues_tunnel_error_and_sentinel(monkeypatch) -> None:
    client = TunnelClient(_identity(endpoints=()), None)

    async def fake_stream_request_async(_method, _path, *, headers, body):
        _ = (headers, body)
        raise ConnectionError("down")

    monkeypatch.setattr(client, "_stream_request_async", fake_stream_request_async)
    chunks: queue.Queue[TunnelResponseHead | bytes | Exception | None] = queue.Queue()
    try:
        future = client.proxy_stream_request("GET", "/", chunks=chunks)
        future.result(timeout=2)
    finally:
        client.close()

    error = chunks.get_nowait()
    assert isinstance(error, TunnelRequestError)
    assert error.reason == "ConnectionError"
    assert chunks.get_nowait() is None
