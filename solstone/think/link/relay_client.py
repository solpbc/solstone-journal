# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Listen WS client and raw relay tunnel pipe."""

from __future__ import annotations

import asyncio
import contextlib
import json
import logging
import random
import urllib.parse
import urllib.request
from collections.abc import Callable
from typing import Any

import websockets
from websockets.asyncio.client import ClientConnection
from websockets.exceptions import ConnectionClosed

log = logging.getLogger("link.relay_client")

_RECONNECT_MIN = 1.0
_RECONNECT_MAX = 60.0
_LINK_DIRECT_HOST = "127.0.0.1"
_LINK_DIRECT_PORT = 7657
_BUF = 65536

CallosumEmit = Callable[[str, dict[str, Any]], None]


class RelayClient:
    def __init__(
        self,
        *,
        instance_id: str,
        home_label: str,
        relay_endpoint: str,
        service_token: str | None,
        on_service_token: Callable[[str], None],
        ca_pubkey_spki_pem: str,
        callosum_emit: CallosumEmit | None = None,
    ) -> None:
        self._instance_id = instance_id
        self._home_label = home_label
        self._relay_endpoint = relay_endpoint.rstrip("/")
        self._relay_ws_endpoint = _to_ws(self._relay_endpoint)
        self._service_token = service_token
        self._on_service_token = on_service_token
        self._ca_pubkey_spki_pem = ca_pubkey_spki_pem
        self._emit = callosum_emit or (lambda _event, _fields: None)
        self._running = False
        self._tunnels: dict[str, asyncio.Task[None]] = {}

    async def enroll_if_needed(self) -> None:
        if self._service_token:
            return
        endpoint = f"{self._relay_endpoint}/enroll/home"
        result = await asyncio.to_thread(
            _post_json_sync,
            endpoint,
            {
                "instance_id": self._instance_id,
                "ca_pubkey": self._ca_pubkey_spki_pem,
                "home_label": self._home_label,
            },
        )
        # back-compat: relay still returns "account_token" until lode L2 renames it
        token = result.get("service_token") or result.get("account_token")
        if not isinstance(token, str) or not token:
            raise RuntimeError("relay returned no service_token")
        self._service_token = token
        self._on_service_token(token)
        self._emit("enrolled", {"instance_id": self._instance_id})

    async def run(self) -> None:
        self._running = True
        delay = _RECONNECT_MIN
        while self._running:
            try:
                await self._run_once()
                delay = _RECONNECT_MIN
            except ConnectionClosed as exc:
                log.warning("listen WS closed: code=%s reason=%s", exc.code, exc.reason)
            except Exception as exc:  # noqa: BLE001
                log.exception("listen loop error: %s", exc)
            if not self._running:
                break
            self._emit("disconnect", {})
            jitter = delay * 0.25
            wait = delay + random.uniform(-jitter, jitter)  # noqa: S311
            log.info("reconnecting in %.1fs", wait)
            await asyncio.sleep(wait)
            delay = min(_RECONNECT_MAX, delay * 2.0)

    async def stop(self) -> None:
        self._running = False
        for task in self._tunnels.values():
            task.cancel()
        if self._tunnels:
            await asyncio.gather(*self._tunnels.values(), return_exceptions=True)
        self._tunnels.clear()

    async def _run_once(self) -> None:
        await self.enroll_if_needed()
        assert self._service_token is not None
        self._emit("connecting", {})
        listen_url = self._url_for("/session/listen", token=self._service_token)
        log.info("opening listen WS")
        async with websockets.connect(
            listen_url,
            additional_headers={"Authorization": f"Bearer {self._service_token}"},
            max_size=None,
        ) as ws:
            self._emit("connected", {})
            log.info("listen WS open; waiting for incoming")
            async for message in ws:
                control = _parse_control(message)
                tunnel_id = control.get("tunnel_id") if control else None
                if not (control and control.get("type") == "incoming" and tunnel_id):
                    continue
                tunnel_id = str(tunnel_id)
                log.info("incoming tunnel_id=%s", tunnel_id)
                self._emit("tunnel_pair", {"tunnel_id": tunnel_id})
                task = asyncio.create_task(
                    self._handle_tunnel(tunnel_id),
                    name=f"link-tunnel-{tunnel_id}",
                )
                self._tunnels[tunnel_id] = task
                task.add_done_callback(
                    lambda _t, tid=tunnel_id: self._tunnels.pop(tid, None)
                )

    async def _handle_tunnel(self, tunnel_id: str) -> None:
        assert self._service_token is not None
        tcp_writer: asyncio.StreamWriter | None = None
        try:
            async with websockets.connect(
                self._url_for(f"/tunnel/{tunnel_id}", token=self._service_token),
                additional_headers={"Authorization": f"Bearer {self._service_token}"},
                max_size=None,
            ) as ws:
                tcp_reader, tcp_writer = await asyncio.open_connection(
                    _LINK_DIRECT_HOST,
                    _LINK_DIRECT_PORT,
                )
                await _pipe_tunnel(ws, tcp_reader, tcp_writer, tunnel_id)
        except ConnectionClosed as exc:
            log.info(
                "tunnel %s closed: code=%s reason=%s", tunnel_id, exc.code, exc.reason
            )
        except Exception as exc:  # noqa: BLE001
            log.exception("tunnel %s error: %s", tunnel_id, exc)
        finally:
            if tcp_writer is not None:
                tcp_writer.close()
                with contextlib.suppress(OSError, RuntimeError):
                    await tcp_writer.wait_closed()
            self._emit("tunnel_close", {"tunnel_id": tunnel_id})

    def _url_for(self, path: str, *, token: str | None = None) -> str:
        query = {"instance": self._instance_id}
        if token:
            query["token"] = token
        return self._relay_ws_endpoint + path + "?" + urllib.parse.urlencode(query)


async def _pipe_tunnel(
    ws: ClientConnection,
    tcp_reader: asyncio.StreamReader,
    tcp_writer: asyncio.StreamWriter,
    tunnel_id: str,
) -> None:
    async def ws_to_tcp() -> None:
        async for frame in ws:
            tcp_writer.write(frame if isinstance(frame, bytes) else frame.encode())
            await tcp_writer.drain()
        with contextlib.suppress(OSError, RuntimeError):
            tcp_writer.write_eof()

    async def tcp_to_ws() -> None:
        while data := await tcp_reader.read(_BUF):
            await ws.send(data)

    tasks = [
        asyncio.create_task(ws_to_tcp()),
        asyncio.create_task(tcp_to_ws()),
    ]
    done, pending = await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
    for task in pending:
        task.cancel()
    if pending:
        await asyncio.gather(*pending, return_exceptions=True)
    for task in done:
        task.result()


def _post_json_sync(url: str, body: dict[str, Any]) -> dict[str, Any]:
    if not url.startswith(("http://", "https://")):
        raise ValueError(f"unsupported url scheme: {url!r}")
    req = urllib.request.Request(  # noqa: S310
        url,
        data=json.dumps(body).encode(),
        headers={
            "content-type": "application/json",
            "user-agent": "solstone-link/0.1",
        },
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=30) as resp:  # noqa: S310
        parsed = json.loads(resp.read())
    if not isinstance(parsed, dict):
        raise RuntimeError("relay returned invalid JSON response")
    return parsed


def _to_ws(endpoint: str) -> str:
    if endpoint.startswith("http://"):
        return "ws://" + endpoint[len("http://") :]
    if endpoint.startswith("https://"):
        return "wss://" + endpoint[len("https://") :]
    return endpoint


def _parse_control(message: str | bytes) -> dict[str, Any] | None:
    try:
        text = message.decode() if isinstance(message, bytes) else message
        parsed = json.loads(text)
    except (UnicodeDecodeError, json.JSONDecodeError):
        return None
    return parsed if isinstance(parsed, dict) else None
