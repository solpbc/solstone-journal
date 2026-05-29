# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import asyncio
import contextlib
import socket
import threading
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest

from solstone.convey.secure_listener.accept import CERTLESS_TUNNEL_CAP, SecureListener
from solstone.think.link.nonces import NONCE_TTL_SECONDS, NonceStore
from solstone.think.link.paths import nonces_path
from tests.link.certless_helpers import write_config


def test_reuse_port_allows_coexisting_bind():
    executor = ThreadPoolExecutor(max_workers=1)
    listener = SecureListener(
        app=MagicMock(),
        strict_tls_ctx=MagicMock(),
        relaxed_tls_ctx=MagicMock(),
        authorized=set(),
        executor=executor,
        callosum_emit=lambda *a, **kw: None,
        host="127.0.0.1",
        port=0,
    )
    loop = asyncio.new_event_loop()
    s2 = None
    try:
        loop.run_until_complete(listener.start())
        assert listener.sockets
        port = listener.sockets[0].getsockname()[1]
        assert port != 0

        s2 = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        s2.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        s2.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
        s2.bind(("127.0.0.1", port))
    finally:
        if listener.sockets:
            loop.run_until_complete(listener.stop())
        if s2 is not None:
            s2.close()
        loop.close()
        executor.shutdown(wait=True, cancel_futures=True)


def test_stop_all_after_loop_closed_does_not_raise():
    from solstone.convey.secure_listener import runtime as rt

    previous_runtime = rt._runtime
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    executor = ThreadPoolExecutor(max_workers=1)
    try:
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
        s.bind(("127.0.0.1", 0))
        s.listen(1)

        loop = asyncio.new_event_loop()
        loop.close()
        thread = threading.Thread(target=lambda: None)
        thread.start()
        thread.join()
        app = SimpleNamespace(secure_listener_started=True)
        listener = SimpleNamespace(sockets=(s,))
        state = rt.RuntimeState(
            loop=loop,
            thread=thread,
            apps=[app],
            executor=executor,
            listener=listener,
            sockets=(s,),
        )
        rt._runtime = state

        rt.stop_all_secure_listener()

        assert s.fileno() == -1
    finally:
        rt._runtime = previous_runtime
        s.close()
        executor.shutdown(wait=True, cancel_futures=True)


@pytest.mark.asyncio
async def test_certless_reap_tears_down_on_passive_expiry(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    journal = _journal(tmp_path, monkeypatch, link={"posture": "spl"})
    NonceStore(nonces_path()).add("live", "phone", now=1000)
    listener = _listener()
    handle, writer, mux, task = _register_fake_certless(listener)

    await listener._reap_certless_if_window_closed(now=1000 + NONCE_TTL_SECONDS + 1)
    await asyncio.sleep(0)

    assert handle.connection_id not in listener._certless_connections
    assert writer.closed is True
    assert mux.closed is True
    assert task.cancelled()
    assert journal.exists()


@pytest.mark.asyncio
async def test_certless_reap_tears_down_after_nonce_consume(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _journal(tmp_path, monkeypatch, link={"posture": "spl"})
    store = NonceStore(nonces_path())
    store.add("live", "phone", now=1000)
    store.consume("live", now=1001)
    listener = _listener()
    handle, writer, mux, _task = _register_fake_certless(listener)

    await listener._reap_certless_if_window_closed(now=1002)

    assert handle.connection_id not in listener._certless_connections
    assert writer.closed is True
    assert mux.closed is True


@pytest.mark.asyncio
async def test_certless_reap_tears_down_when_posture_leaves_spl(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _journal(tmp_path, monkeypatch, link={"posture": "direct"})
    NonceStore(nonces_path()).add("live", "phone", now=1000)
    listener = _listener()
    handle, writer, mux, _task = _register_fake_certless(listener)

    await listener._reap_certless_if_window_closed(now=1001)

    assert handle.connection_id not in listener._certless_connections
    assert writer.closed is True
    assert mux.closed is True


@pytest.mark.asyncio
async def test_certless_concurrent_cap_refuses_fifth() -> None:
    listener = _listener()
    registered_tasks: list[asyncio.Task[None]] = []
    rejected_task = asyncio.create_task(_sleep_forever())
    try:
        for index in range(CERTLESS_TUNNEL_CAP):
            handle, _writer, _mux, task = _register_fake_certless(
                listener,
                connection_id=f"conn-{index}",
            )
            registered_tasks.append(task)
            assert handle is not None

        rejected = listener._register_certless_connection(
            "conn-rejected",
            _FakeWriter(),
            rejected_task,
            _FakeMux(),
        )

        assert rejected is None
        assert len(listener._certless_connections) == CERTLESS_TUNNEL_CAP
    finally:
        for handle in list(listener._certless_connections.values()):
            await listener._close_certless_connection(handle)
        rejected_task.cancel()
        for task in registered_tasks + [rejected_task]:
            with contextlib.suppress(asyncio.CancelledError):
                await task


def _listener() -> SecureListener:
    return SecureListener(
        app=MagicMock(),
        strict_tls_ctx=MagicMock(),
        relaxed_tls_ctx=MagicMock(),
        authorized=MagicMock(),
        executor=MagicMock(),
        callosum_emit=lambda *a, **kw: None,
        host="127.0.0.1",
        port=0,
    )


def _journal(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    *,
    link: dict[str, object],
) -> Path:
    journal = tmp_path / "journal"
    journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    write_config(journal, link=link)
    return journal


def _register_fake_certless(
    listener: SecureListener,
    *,
    connection_id: str = "conn",
) -> tuple[object, "_FakeWriter", "_FakeMux", asyncio.Task[None]]:
    writer = _FakeWriter()
    mux = _FakeMux()
    task = asyncio.create_task(_sleep_forever())
    handle = listener._register_certless_connection(connection_id, writer, task, mux)
    assert handle is not None
    return handle, writer, mux, task


async def _sleep_forever() -> None:
    await asyncio.Event().wait()


class _FakeWriter:
    def __init__(self) -> None:
        self.closed = False

    def close(self) -> None:
        self.closed = True


class _FakeMux:
    def __init__(self) -> None:
        self.closed = False

    async def close(self) -> None:
        self.closed = True
