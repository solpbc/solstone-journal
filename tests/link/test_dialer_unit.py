# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Unit tests for observer PL dial orchestration."""

from __future__ import annotations

import asyncio

import pytest

from solstone.observe.observer_client import ObserverClient
from solstone.think.link.client import (
    Client,
    ClientIdentity,
    EnrolledDevice,
    StreamResetError,
    TlsError,
    TunnelSession,
    _http_request_bytes,
)


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


def _client_for_identity(identity: ClientIdentity) -> ObserverClient:
    client = object.__new__(ObserverClient)
    client._pl_identity = identity
    client._pl_relay_url = None
    client._pl_enrolled = None
    client._pl_session = None
    client._pl_session_lock = None
    return client


@pytest.mark.asyncio
async def test_lan_direct_race_picks_first_and_cancels_loser(monkeypatch) -> None:
    client = _client_for_identity(
        _identity(
            endpoints=(
                {"ip": "10.0.0.1", "port": 7657},
                {"ip": "10.0.0.2", "port": 7657},
            )
        )
    )
    cancelled: list[str] = []
    winner = object()

    async def dial_direct(endpoint: dict[str, object]):
        if endpoint["ip"] == "10.0.0.2":
            await asyncio.sleep(0)
            return winner
        try:
            await asyncio.sleep(60)
        except asyncio.CancelledError:
            cancelled.append(str(endpoint["ip"]))
            raise

    monkeypatch.setattr(client, "_dial_direct_endpoint", dial_direct)

    assert await client._open_tunnel() is winner
    assert cancelled == ["10.0.0.1"]


@pytest.mark.asyncio
async def test_all_fail_error_names_every_attempt(monkeypatch) -> None:
    client = _client_for_identity(
        _identity(endpoints=({"ip": "10.0.0.1", "port": 7657},))
    )
    client._pl_relay_url = "https://relay.test"

    async def dial_direct(_endpoint: dict[str, object]):
        raise TlsError("lan failed")

    async def dial_relay():
        raise OSError("relay failed")

    monkeypatch.setattr(client, "_dial_direct_endpoint", dial_direct)
    monkeypatch.setattr(client, "_dial_relay", dial_relay)

    with pytest.raises(TlsError) as exc_info:
        await client._open_tunnel()

    message = str(exc_info.value)
    assert "lan-direct 10.0.0.1:7657" in message
    assert "lan failed" in message
    assert "spl-relay" in message
    assert "relay failed" in message


@pytest.mark.asyncio
async def test_cached_session_drops_on_stream_reset() -> None:
    client = _client_for_identity(_identity(endpoints=()))
    client._pl_session_lock = asyncio.Lock()

    class ResetSession:
        def __init__(self) -> None:
            self.closed = False

        async def request(self, *_args, **_kwargs):
            raise StreamResetError("reset")

        async def close(self) -> None:
            self.closed = True

    session = ResetSession()
    client._pl_session = session

    with pytest.raises(StreamResetError):
        await client._pl_request("GET", "/")

    assert session.closed is True
    assert client._pl_session is None
