# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Paired-link client primitives.

Raises ``TlsError`` for TLS handshake failures, ``ConnectionError`` or
``OSError`` for transport failures, and ``OSError`` for pre-dial credential file
errors in callers that load identities from disk.
"""

from __future__ import annotations

import asyncio
import contextlib
import dataclasses
import hashlib
import logging
import urllib.parse
from collections.abc import AsyncIterator, Awaitable, Callable
from typing import Protocol

import requests
import websockets
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import NameOID
from OpenSSL import SSL, crypto
from websockets.asyncio.client import ClientConnection
from websockets.exceptions import ConnectionClosed

from solstone.convey.secure_listener.framing import (
    FLAG_CLOSE,
    FLAG_DATA,
    FLAG_OPEN,
    FLAG_RESET,
    FLAG_WINDOW,
    INITIAL_WINDOW,
    MAX_CONCURRENT_STREAMS,
    MAX_PAYLOAD,
    RECOMMENDED_CHUNK,
    RESET_FLOW_CONTROL_ERROR,
    RESET_INTERNAL_ERROR,
    RESET_PROTOCOL_ERROR,
    Frame,
    FrameDecoder,
    ProtocolError,
    build_close,
    build_data,
    build_open,
    build_reset,
    build_window,
    parse_reset_reason,
    parse_window_credit,
)
from solstone.think.link.ca import cert_fingerprint

LOG = logging.getLogger(__name__)
_CONNECT_TIMEOUT_SECONDS = 15
_HTTP_TIMEOUT_SECONDS = 30


class TlsError(RuntimeError):
    """Raised when the client-side TLS handshake or tunnel aborts."""


class StreamResetError(ConnectionError):
    """Raised when the peer sends a RESET frame for an active stream."""


class EncryptedTransport(Protocol):
    async def send(self, data: bytes) -> None: ...

    async def recv(self) -> bytes | None: ...

    async def close(self) -> None: ...


@dataclasses.dataclass(frozen=True)
class ClientIdentity:
    private_key_pem: str
    client_cert_pem: str
    ca_chain_pem: str
    fingerprint: str
    home_instance_id: str
    home_label: str
    home_attestation: str
    local_endpoints: tuple[dict[str, object], ...] = ()


@dataclasses.dataclass(frozen=True)
class EnrolledDevice:
    device_token: str
    identity: ClientIdentity


@dataclasses.dataclass
class _TlsClientState:
    conn: SSL.Connection
    handshake_done: bool = False


@dataclasses.dataclass
class _StreamState:
    stream_id: int
    buffered: list[bytes] = dataclasses.field(default_factory=list)
    waiters: list[asyncio.Future[bytes | None]] = dataclasses.field(
        default_factory=list
    )
    closed_waiters: list[asyncio.Future[None]] = dataclasses.field(default_factory=list)
    send_credit: int = INITIAL_WINDOW
    recv_credit: int = INITIAL_WINDOW
    unacked_recv: int = 0
    writer_closed: bool = False
    reader_closed: bool = False
    reset_reason: int | None = None
    credit_event: asyncio.Event = dataclasses.field(default_factory=asyncio.Event)

    def __post_init__(self) -> None:
        self.credit_event.set()


class _DialerStream:
    def __init__(self, mux: _DialerMultiplexer, state: _StreamState) -> None:
        self._mux = mux
        self._state = state

    @property
    def id(self) -> int:
        return self._state.stream_id

    async def write(self, data: bytes) -> None:
        if self._state.writer_closed:
            raise ConnectionError(f"stream {self._state.stream_id} writer is closed")
        view = memoryview(data)
        while view:
            chunk_len = min(
                len(view),
                RECOMMENDED_CHUNK,
                MAX_PAYLOAD,
                self._state.send_credit,
            )
            if chunk_len <= 0:
                self._state.credit_event.clear()
                await self._state.credit_event.wait()
                continue
            chunk = bytes(view[:chunk_len])
            view = view[chunk_len:]
            self._state.send_credit -= chunk_len
            await self._mux._emit(build_data(self._state.stream_id, chunk))

    async def close(self) -> None:
        if self._state.writer_closed:
            return
        self._state.writer_closed = True
        await self._mux._emit(build_close(self._state.stream_id))
        if self._state.reader_closed:
            self._mux._forget(self._state.stream_id)

    async def reset(self, reason: int = RESET_INTERNAL_ERROR) -> None:
        if self._state.writer_closed and self._state.reader_closed:
            return
        self._state.writer_closed = True
        self._state.reader_closed = True
        self._state.reset_reason = reason
        await self._mux._emit(build_reset(self._state.stream_id, reason))
        self._mux._close_stream(self._state, forget=True)

    async def read(self) -> AsyncIterator[bytes]:
        while True:
            if self._state.buffered:
                yield self._state.buffered.pop(0)
                continue
            if self._state.reader_closed:
                if self._state.reset_reason is not None:
                    raise StreamResetError(
                        f"stream {self._state.stream_id} reset: {self._state.reset_reason}"
                    )
                return
            fut: asyncio.Future[bytes | None] = (
                asyncio.get_running_loop().create_future()
            )
            self._state.waiters.append(fut)
            chunk = await fut
            if chunk is None:
                if self._state.reset_reason is not None:
                    raise StreamResetError(
                        f"stream {self._state.stream_id} reset: {self._state.reset_reason}"
                    )
                return
            yield chunk

    async def read_all(self) -> bytes:
        parts = bytearray()
        async for chunk in self.read():
            parts.extend(chunk)
        return bytes(parts)

    @property
    def closed(self) -> asyncio.Future[None]:
        fut: asyncio.Future[None] = asyncio.get_running_loop().create_future()
        if self._state.reader_closed and self._state.writer_closed:
            fut.set_result(None)
            return fut
        self._state.closed_waiters.append(fut)
        return fut


class _DialerMultiplexer:
    def __init__(self, send_frame: Callable[[bytes], Awaitable[None]]) -> None:
        self._decoder = FrameDecoder()
        self._send_frame = send_frame
        self._streams: dict[int, _StreamState] = {}
        self._next_local_id = 1
        self._closed = False

    async def open_stream(self, initial: bytes = b"") -> _DialerStream:
        if self._closed:
            raise ConnectionError("mux is closed")
        if len(self._streams) >= MAX_CONCURRENT_STREAMS:
            raise ConnectionError("concurrent stream cap reached")
        if len(initial) > MAX_PAYLOAD:
            raise ValueError(f"initial payload exceeds framing max {MAX_PAYLOAD}")
        stream_id = self._next_local_id
        self._next_local_id += 2
        state = _StreamState(stream_id=stream_id)
        if initial:
            state.send_credit -= len(initial)
        self._streams[stream_id] = state
        await self._emit(build_open(stream_id, initial))
        return _DialerStream(self, state)

    async def feed(self, plaintext: bytes) -> None:
        if self._closed or not plaintext:
            return
        self._decoder.feed(plaintext)
        while True:
            try:
                frame = self._decoder.next()
            except ProtocolError:
                self.close()
                return
            if frame is None:
                return
            await self._dispatch(frame)

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        for state in list(self._streams.values()):
            if state.reset_reason is None:
                state.reset_reason = RESET_INTERNAL_ERROR
            self._close_stream(state, forget=True)

    async def _dispatch(self, frame: Frame) -> None:
        if frame.flags & FLAG_OPEN:
            await self._emit(build_reset(frame.stream_id, RESET_PROTOCOL_ERROR))
            return

        state = self._streams.get(frame.stream_id)
        if state is None:
            await self._emit(build_reset(frame.stream_id, RESET_PROTOCOL_ERROR))
            return

        if frame.flags & FLAG_DATA:
            if len(frame.payload) > state.recv_credit:
                await self._emit(build_reset(frame.stream_id, RESET_FLOW_CONTROL_ERROR))
                state.reset_reason = RESET_FLOW_CONTROL_ERROR
                self._close_stream(state, forget=True)
                return
            state.recv_credit -= len(frame.payload)
            state.unacked_recv += len(frame.payload)
            if state.waiters:
                waiter = state.waiters.pop(0)
                if not waiter.done():
                    waiter.set_result(frame.payload)
            else:
                state.buffered.append(frame.payload)
            if state.unacked_recv >= INITIAL_WINDOW // 2:
                grant = state.unacked_recv
                state.recv_credit += grant
                state.unacked_recv = 0
                await self._emit(build_window(frame.stream_id, grant))

        if frame.flags & FLAG_CLOSE:
            state.reader_closed = True
            while state.waiters:
                waiter = state.waiters.pop(0)
                if not waiter.done():
                    waiter.set_result(None)
            if state.writer_closed:
                self._forget(frame.stream_id)
            self._resolve_closed(state)

        if frame.flags & FLAG_WINDOW:
            try:
                credit = parse_window_credit(frame)
            except ProtocolError:
                await self._emit(build_reset(frame.stream_id, RESET_PROTOCOL_ERROR))
                state.reset_reason = RESET_PROTOCOL_ERROR
                self._close_stream(state, forget=True)
                return
            state.send_credit += credit
            state.credit_event.set()

        if frame.flags & FLAG_RESET:
            try:
                state.reset_reason = parse_reset_reason(frame)
            except ProtocolError:
                state.reset_reason = RESET_PROTOCOL_ERROR
            self._close_stream(state, forget=True)

    async def _emit(self, frame: Frame) -> None:
        if self._closed:
            return
        await self._send_frame(frame.encode())

    def _close_stream(self, state: _StreamState, *, forget: bool) -> None:
        state.writer_closed = True
        state.reader_closed = True
        while state.waiters:
            waiter = state.waiters.pop(0)
            if not waiter.done():
                waiter.set_result(None)
        state.credit_event.set()
        self._resolve_closed(state)
        if forget:
            self._forget(state.stream_id)

    def _resolve_closed(self, state: _StreamState) -> None:
        while state.closed_waiters:
            waiter = state.closed_waiters.pop(0)
            if not waiter.done():
                waiter.set_result(None)

    def _forget(self, stream_id: int) -> None:
        self._streams.pop(stream_id, None)


class _WsEncryptedTransport:
    def __init__(self, ws: ClientConnection) -> None:
        self._ws = ws

    async def send(self, data: bytes) -> None:
        await self._ws.send(data)

    async def recv(self) -> bytes | None:
        try:
            message = await self._ws.recv()
        except ConnectionClosed:
            return None
        return message if isinstance(message, bytes) else message.encode("utf-8")

    async def close(self) -> None:
        await self._ws.close()


class _TcpEncryptedTransport:
    def __init__(
        self,
        reader: asyncio.StreamReader,
        writer: asyncio.StreamWriter,
    ) -> None:
        self._reader = reader
        self._writer = writer

    async def send(self, data: bytes) -> None:
        self._writer.write(data)
        await self._writer.drain()

    async def recv(self) -> bytes | None:
        chunk = await self._reader.read(65536)
        return chunk if chunk else None

    async def close(self) -> None:
        self._writer.close()
        with contextlib.suppress(Exception):
            await self._writer.wait_closed()


class TunnelSession:
    def __init__(
        self,
        *,
        transport: EncryptedTransport,
        tls: _TlsClientState,
        identity: ClientIdentity,
    ) -> None:
        self._transport = transport
        self._tls = tls
        self._identity = identity
        self._tls_lock = asyncio.Lock()
        self._mux = _DialerMultiplexer(self._send_plaintext)
        self._closed = asyncio.Event()
        self._reader_task = asyncio.create_task(
            self._read_transport(),
            name=f"link-client-{identity.home_instance_id}",
        )

    async def __aenter__(self) -> TunnelSession:
        return self

    async def __aexit__(self, *_exc: object) -> None:
        await self.close()

    async def request(
        self,
        method: str,
        path: str,
        *,
        headers: dict[str, str] | None = None,
        body: bytes = b"",
    ) -> tuple[int, dict[str, str], bytes]:
        request_bytes = _http_request_bytes(method, path, headers=headers, body=body)
        stream = await self._mux.open_stream(request_bytes)
        await stream.close()
        response = await stream.read_all()
        return _parse_http_response(response)

    async def stream_request(
        self,
        method: str,
        path: str,
        *,
        headers: dict[str, str] | None = None,
        body: bytes = b"",
    ) -> tuple[int, dict[str, str], bytes, _DialerStream]:
        request_bytes = _http_request_bytes(method, path, headers=headers, body=body)
        stream = await self._mux.open_stream(request_bytes)
        await stream.close()
        buffered = bytearray()
        async for chunk in stream.read():
            buffered.extend(chunk)
            split = buffered.find(b"\r\n\r\n")
            if split < 0:
                continue
            status, response_headers = _parse_http_head(bytes(buffered[:split]))
            return status, response_headers, bytes(buffered[split + 4 :]), stream
        raise ValueError("response missing header terminator")

    async def close(self) -> None:
        if self._closed.is_set():
            return
        self._mux.close()
        await self._transport.close()
        await self._reader_task
        self._closed.set()

    async def _read_transport(self) -> None:
        try:
            while True:
                inbound = await self._transport.recv()
                if inbound is None:
                    return
                async with self._tls_lock:
                    outbound, plaintext = _drive_tls_client(self._tls, inbound=inbound)
                if outbound:
                    await self._transport.send(outbound)
                if plaintext:
                    await self._mux.feed(plaintext)
        finally:
            self._mux.close()
            self._closed.set()

    async def _send_plaintext(self, plaintext: bytes) -> None:
        async with self._tls_lock:
            outbound, _ = _drive_tls_client(self._tls, plaintext_out=plaintext)
        if outbound:
            await self._transport.send(outbound)


class Client:
    @staticmethod
    def pair(
        lan_url: str,
        device_label: str,
        *,
        ca_fingerprint_pin: str | None = None,
    ) -> ClientIdentity:
        base_url = lan_url.rstrip("/")
        LOG.info("client %s: pair start", device_label)
        pair_start = _post_json(
            f"{base_url}/app/link/pair-start",
            {"device_label": device_label},
        )
        nonce = pair_start.get("nonce")
        if not isinstance(nonce, str) or not nonce:
            raise RuntimeError("pair-start returned no nonce")

        private_key_pem, csr_pem = _build_csr(device_label)
        paired = _post_json(
            f"{base_url}/app/link/pair",
            {
                "nonce": nonce,
                "csr": csr_pem,
                "device_label": device_label,
            },
        )

        client_cert_pem = _required_str(paired, "client_cert")
        ca_chain = paired.get("ca_chain")
        if not isinstance(ca_chain, list) or not ca_chain:
            raise RuntimeError("pair returned no ca_chain")
        if not all(isinstance(item, str) and item for item in ca_chain):
            raise RuntimeError("pair returned invalid ca_chain")
        ca_chain_pem = "".join(ca_chain)
        ca_fingerprint = _cert_sha256_hex(_first_cert_pem(ca_chain_pem))
        if ca_fingerprint_pin is not None and ca_fingerprint != ca_fingerprint_pin:
            raise RuntimeError(
                f"CA fingerprint mismatch: got {ca_fingerprint}, expected {ca_fingerprint_pin}"
            )

        fingerprint = _required_str(paired, "fingerprint")
        if cert_fingerprint(client_cert_pem) != fingerprint:
            raise RuntimeError("pair returned certificate fingerprint mismatch")

        identity = ClientIdentity(
            private_key_pem=private_key_pem,
            client_cert_pem=client_cert_pem,
            ca_chain_pem=ca_chain_pem,
            fingerprint=fingerprint,
            home_instance_id=_required_str(paired, "instance_id"),
            home_label=_required_str(paired, "home_label"),
            home_attestation=_required_str(paired, "home_attestation"),
            local_endpoints=_optional_endpoint_dicts(paired.get("local_endpoints")),
        )
        LOG.info(
            "client %s: paired to %s",
            device_label,
            identity.home_instance_id,
        )
        return identity

    @staticmethod
    def enroll_device(relay_url: str, identity: ClientIdentity) -> EnrolledDevice:
        endpoint = f"{relay_url.rstrip('/')}/enroll/device"
        LOG.info("client %s: enrolling device token", identity.fingerprint)
        payload = _post_json(
            endpoint,
            {
                "instance_id": identity.home_instance_id,
                "client_cert": identity.client_cert_pem,
                "home_attestation": identity.home_attestation,
            },
        )
        device_token = _required_str(payload, "device_token")
        LOG.info("client %s: enroll complete", identity.fingerprint)
        return EnrolledDevice(device_token=device_token, identity=identity)

    @staticmethod
    async def dial(relay_url: str, enrolled: EnrolledDevice) -> TunnelSession:
        identity = enrolled.identity
        url = (
            _to_ws(relay_url.rstrip("/"))
            + "/session/dial?"
            + urllib.parse.urlencode(
                {
                    "instance": identity.home_instance_id,
                    "token": enrolled.device_token,
                }
            )
        )
        LOG.info("client %s: dialing %s", identity.fingerprint, _redact_url(url))
        ws = await websockets.connect(url, max_size=None)
        return await _open_tunnel_session(_WsEncryptedTransport(ws), identity)

    @staticmethod
    async def dial_direct(
        host: str,
        enrolled: EnrolledDevice,
        *,
        port: int = 7657,
    ) -> TunnelSession:
        identity = enrolled.identity
        LOG.info("client %s: dialing direct %s:%d", identity.fingerprint, host, port)
        reader, writer = await asyncio.open_connection(host, port)
        return await _open_tunnel_session(
            _TcpEncryptedTransport(reader, writer),
            identity,
        )


async def _open_tunnel_session(
    transport: EncryptedTransport,
    identity: ClientIdentity,
) -> TunnelSession:
    try:
        tls = _new_tls_client(_build_tls_client_ctx(identity))
        pending_plaintext = bytearray()
        outbound, plaintext = _drive_tls_client(tls)
        if outbound:
            await transport.send(outbound)
        pending_plaintext.extend(plaintext)
        while not tls.handshake_done:
            inbound = await asyncio.wait_for(
                transport.recv(),
                timeout=_CONNECT_TIMEOUT_SECONDS,
            )
            if inbound is None:
                raise TlsError("transport closed during TLS handshake")
            outbound, plaintext = _drive_tls_client(tls, inbound=inbound)
            if outbound:
                await transport.send(outbound)
            pending_plaintext.extend(plaintext)
    except Exception:
        await transport.close()
        raise

    session = TunnelSession(
        transport=transport,
        tls=tls,
        identity=identity,
    )
    if pending_plaintext:
        await session._mux.feed(bytes(pending_plaintext))
    return session


def _build_csr(device_label: str) -> tuple[str, str]:
    private_key = ec.generate_private_key(ec.SECP256R1())
    csr = (
        x509.CertificateSigningRequestBuilder()
        .subject_name(
            x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, device_label)])
        )
        .sign(private_key, hashes.SHA256())
    )
    private_key_pem = private_key.private_bytes(
        serialization.Encoding.PEM,
        serialization.PrivateFormat.PKCS8,
        serialization.NoEncryption(),
    ).decode("ascii")
    csr_pem = csr.public_bytes(serialization.Encoding.PEM).decode("ascii")
    return private_key_pem, csr_pem


def _build_tls_client_ctx(identity: ClientIdentity) -> SSL.Context:
    ctx = SSL.Context(SSL.TLS_METHOD)
    ctx.set_min_proto_version(SSL.TLS1_3_VERSION)
    ctx.set_max_proto_version(SSL.TLS1_3_VERSION)
    ctx.use_certificate(
        crypto.load_certificate(
            crypto.FILETYPE_PEM,
            identity.client_cert_pem.encode("ascii"),
        )
    )
    ctx.use_privatekey(
        crypto.load_privatekey(
            crypto.FILETYPE_PEM,
            identity.private_key_pem.encode("ascii"),
        )
    )
    store = ctx.get_cert_store()
    assert store is not None, "client TLS context must expose a cert store"
    for cert_pem in _split_pem_chain(identity.ca_chain_pem):
        store.add_cert(
            crypto.X509.from_cryptography(x509.load_pem_x509_certificate(cert_pem)),
        )
    ctx.set_verify(SSL.VERIFY_PEER, _verify_server_cert)
    ctx.check_privatekey()
    return ctx


def _verify_server_cert(
    _conn: SSL.Connection,
    _cert: crypto.X509,
    _errno: int,
    _depth: int,
    preverify_ok: int,
) -> bool:
    return bool(preverify_ok)


def _new_tls_client(ctx: SSL.Context) -> _TlsClientState:
    conn = SSL.Connection(ctx, None)
    conn.set_connect_state()
    return _TlsClientState(conn=conn)


def _drive_tls_client(
    state: _TlsClientState,
    *,
    inbound: bytes = b"",
    plaintext_out: bytes = b"",
) -> tuple[bytes, bytes]:
    if inbound:
        state.conn.bio_write(inbound)
    if plaintext_out:
        try:
            state.conn.send(plaintext_out)
        except SSL.WantReadError:
            pass
        except SSL.Error as exc:
            raise TlsError(f"send failed: {exc}") from exc

    if not state.handshake_done:
        try:
            state.conn.do_handshake()
            state.handshake_done = True
        except SSL.WantReadError:
            pass
        except SSL.Error as exc:
            raise TlsError(f"handshake failed: {exc}") from exc

    plaintext_in = bytearray()
    if state.handshake_done:
        while True:
            try:
                chunk = state.conn.recv(16 * 1024)
            except SSL.WantReadError:
                break
            except SSL.ZeroReturnError:
                break
            except SSL.Error as exc:
                raise TlsError(f"recv failed: {exc}") from exc
            if not chunk:
                break
            plaintext_in.extend(chunk)

    outbound = bytearray()
    while True:
        try:
            chunk = state.conn.bio_read(16 * 1024)
        except SSL.WantReadError:
            break
        if not chunk:
            break
        outbound.extend(chunk)
    return bytes(outbound), bytes(plaintext_in)


def _http_request_bytes(
    method: str,
    path: str,
    *,
    headers: dict[str, str] | None,
    body: bytes,
) -> bytes:
    body_bytes = body or b""
    normalized_headers = {"host": "spl.local"}
    if headers:
        normalized_headers.update({k.lower(): v for k, v in headers.items()})
    normalized_headers["content-length"] = str(len(body_bytes))
    head = (
        f"{method} {path} HTTP/1.1\r\n"
        + "".join(f"{name}: {value}\r\n" for name, value in normalized_headers.items())
        + "\r\n"
    )
    return head.encode("ascii") + body_bytes


def _parse_http_response(raw: bytes) -> tuple[int, dict[str, str], bytes]:
    split = raw.find(b"\r\n\r\n")
    if split < 0:
        raise ValueError("response missing header terminator")
    body = raw[split + 4 :]
    status, headers = _parse_http_head(raw[:split])
    if headers.get("transfer-encoding", "").lower() == "chunked":
        return status, headers, _dechunk(body)
    content_length = headers.get("content-length")
    if content_length is None:
        return status, headers, body
    return status, headers, body[: int(content_length)]


def _parse_http_head(raw: bytes) -> tuple[int, dict[str, str]]:
    head = raw.decode("latin-1")
    lines = head.split("\r\n")
    if not lines:
        raise ValueError("response missing status line")
    parts = lines[0].split(" ", 2)
    if len(parts) < 2 or not parts[1].isdigit():
        raise ValueError(f"bad status line: {lines[0]!r}")
    status = int(parts[1])
    headers: dict[str, str] = {}
    for line in lines[1:]:
        if not line or ":" not in line:
            continue
        name, value = line.split(":", 1)
        headers[name.strip().lower()] = value.strip()
    return status, headers


def _dechunk(raw: bytes) -> bytes:
    out = bytearray()
    index = 0
    while index < len(raw):
        line_end = raw.find(b"\r\n", index)
        if line_end < 0:
            raise ValueError("chunked response missing size terminator")
        size_text = raw[index:line_end].decode("ascii").split(";", 1)[0].strip()
        size = int(size_text, 16)
        index = line_end + 2
        if size == 0:
            return bytes(out)
        out.extend(raw[index : index + size])
        index += size + 2
    return bytes(out)


def _post_json(url: str, payload: dict[str, object]) -> dict[str, object]:
    response = requests.post(url, json=payload, timeout=_HTTP_TIMEOUT_SECONDS)
    if not response.ok:
        raise RuntimeError(
            f"POST {url} failed: HTTP {response.status_code}: {response.text}"
        )
    parsed = response.json()
    if not isinstance(parsed, dict):
        raise RuntimeError(f"unexpected JSON response from {url}")
    return parsed


def _required_str(payload: dict[str, object], key: str) -> str:
    value = payload.get(key)
    if not isinstance(value, str) or not value:
        raise RuntimeError(f"missing string field: {key}")
    return value


def _optional_endpoint_dicts(value: object) -> tuple[dict[str, object], ...]:
    if not isinstance(value, list):
        return ()
    return tuple(item for item in value if isinstance(item, dict))


def _split_pem_chain(pem_bundle: str) -> list[bytes]:
    marker = "-----END CERTIFICATE-----"
    certs: list[bytes] = []
    for chunk in pem_bundle.split(marker):
        chunk = chunk.strip()
        if not chunk:
            continue
        certs.append(f"{chunk}\n{marker}\n".encode("ascii"))
    return certs


def _first_cert_pem(pem_bundle: str) -> str:
    certs = _split_pem_chain(pem_bundle)
    if not certs:
        raise RuntimeError("empty certificate chain")
    return certs[0].decode("ascii")


def _cert_sha256_hex(cert_pem: str) -> str:
    cert = x509.load_pem_x509_certificate(cert_pem.encode("ascii"))
    return hashlib.sha256(cert.public_bytes(serialization.Encoding.DER)).hexdigest()


def _to_ws(url: str) -> str:
    if url.startswith("https://"):
        return "wss://" + url[len("https://") :]
    if url.startswith("http://"):
        return "ws://" + url[len("http://") :]
    return url


def _redact_url(url: str) -> str:
    parsed = urllib.parse.urlparse(url)
    query = urllib.parse.parse_qs(parsed.query)
    if "token" in query:
        query["token"] = ["<redacted>"]
    return urllib.parse.urlunparse(
        parsed._replace(query=urllib.parse.urlencode(query, doseq=True))
    )


__all__ = [
    "Client",
    "ClientIdentity",
    "EncryptedTransport",
    "EnrolledDevice",
    "StreamResetError",
    "TlsError",
    "TunnelSession",
    "_http_request_bytes",
]
