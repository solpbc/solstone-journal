# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Resident loopback proxy for `sol link serve`."""

from __future__ import annotations

import argparse
import errno
import logging
import queue
import sys
from collections.abc import Iterable
from concurrent.futures import Future
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import NamedTuple, Protocol, cast
from urllib.parse import urlsplit

from solstone.think.link.bundle import load_client_identity
from solstone.think.link.dialer import (
    TunnelClient,
    TunnelRequestError,
    TunnelResponseHead,
)
from solstone.think.link.observer_paths import observer_bundle_dir, observer_spl_root
from solstone.think.link.paths import relay_url

LOG = logging.getLogger(__name__)
LOOPBACK_HOST = "127.0.0.1"
DEFAULT_PORT = 5015
REQUEST_HOP_BY_HOP = {
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
}
RESPONSE_HOP_BY_HOP = {
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "upgrade",
}


class _ProxyTunnel(Protocol):
    def proxy_stream_request(
        self,
        method: str,
        path: str,
        *,
        headers: dict[str, str] | None = None,
        body: bytes = b"",
        chunks: queue.Queue[TunnelResponseHead | bytes | Exception | None],
    ) -> Future[None]: ...


class _BundleSelection(NamedTuple):
    label: str
    bundle_dir: Path


class _ProxyServer(ThreadingHTTPServer):
    tunnel: _ProxyTunnel


def add_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--label", help="Observer link bundle label")
    parser.add_argument(
        "--port",
        type=int,
        default=DEFAULT_PORT,
        help=f"Loopback port to serve on (default: {DEFAULT_PORT})",
    )
    parser.add_argument("--relay-url", help="Override the spl relay URL")


def main(args: argparse.Namespace) -> int:
    port_error = _port_error(args.port)
    if port_error is not None:
        return _fail(port_error, code=2)

    try:
        selection = _resolve_bundle_dir(args.label)
        identity = load_client_identity(selection.bundle_dir)
    except ValueError as exc:
        return _fail(str(exc), code=1)

    tunnel = TunnelClient(identity, _resolve_relay_url(args.relay_url))
    try:
        server = _build_server(args.port, tunnel)
    except OSError as exc:
        tunnel.close()
        return _fail(_bind_error(args.port, exc), code=1)

    LOG.info(
        "forwarding %s:%d -> home %s over pl",
        LOOPBACK_HOST,
        args.port,
        selection.label,
    )
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        return 0
    finally:
        server.shutdown()
        server.server_close()
        tunnel.close()
    return 0


def _resolve_bundle_dir(label: str | None) -> _BundleSelection:
    if label:
        bundle_dir = observer_bundle_dir(label)
        try:
            load_client_identity(bundle_dir)
        except ValueError as exc:
            raise ValueError(
                f"invalid link bundle for label '{label}' at {bundle_dir}: {exc}. "
                "Run `sol link join` to pair this device."
            ) from exc
        return _BundleSelection(label, bundle_dir)

    bundles = _valid_observer_bundles()
    if not bundles:
        raise ValueError(
            f"no observer link bundles found under {observer_spl_root()}. "
            "Run `sol link join` to pair this device."
        )
    if len(bundles) > 1:
        labels = ", ".join(sorted(bundles))
        raise ValueError(
            f"multiple observer link bundles found: {labels}. "
            "Pass --label to choose one."
        )
    selected_label, bundle_dir = next(iter(bundles.items()))
    return _BundleSelection(selected_label, bundle_dir)


def _valid_observer_bundles() -> dict[str, Path]:
    root = observer_spl_root()
    if not root.is_dir():
        return {}
    bundles: dict[str, Path] = {}
    for bundle_dir in sorted(root.iterdir()):
        if not bundle_dir.is_dir():
            continue
        try:
            load_client_identity(bundle_dir)
        except ValueError:
            continue
        bundles[bundle_dir.name] = bundle_dir
    return bundles


def _resolve_relay_url(value: str | None) -> str:
    if value and value.strip():
        return value.strip().rstrip("/")
    return relay_url()


def _build_server(port: int, tunnel: _ProxyTunnel) -> _ProxyServer:
    server = _ProxyServer((LOOPBACK_HOST, port), _ProxyHandler)
    server.tunnel = tunnel
    return server


def _port_error(port: int) -> str | None:
    if port < 1 or port > 65535:
        return "--port must be between 1 and 65535"
    return None


def _bind_error(port: int, exc: OSError) -> str:
    if exc.errno == errno.EADDRINUSE:
        return (
            f"cannot bind {LOOPBACK_HOST}:{port}: address already in use. "
            "Another `sol link serve` or Convey may already be using that port."
        )
    return f"cannot bind {LOOPBACK_HOST}:{port}: {exc}"


def _fail(message: str, *, code: int) -> int:
    print(message, file=sys.stderr)
    return code


class _ProxyHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self) -> None:
        self._handle()

    def do_POST(self) -> None:
        self._handle()

    def do_PUT(self) -> None:
        self._handle()

    def do_DELETE(self) -> None:
        self._handle()

    def do_PATCH(self) -> None:
        self._handle()

    def do_HEAD(self) -> None:
        self._handle()

    def do_OPTIONS(self) -> None:
        self._handle()

    def _handle(self) -> None:
        body = self._read_body()
        if body is None:
            return

        chunks: queue.Queue[TunnelResponseHead | bytes | Exception | None] = (
            queue.Queue()
        )
        server = cast(_ProxyServer, self.server)
        future = server.tunnel.proxy_stream_request(
            self.command,
            _origin_path(self.path),
            headers=_forward_request_headers(self.headers.items(), len(body)),
            body=body,
            chunks=chunks,
        )

        first = chunks.get()
        if isinstance(first, Exception):
            self._send_gateway_error(first)
            return
        if first is None:
            self._send_gateway_error(RuntimeError("empty response from link tunnel"))
            return
        if not isinstance(first, TunnelResponseHead):
            self._send_gateway_error(RuntimeError("bad response from link tunnel"))
            return

        self.send_response_only(first.status)
        for name, value in first.headers.items():
            if name.lower() in RESPONSE_HOP_BY_HOP:
                continue
            self.send_header(name, value)
        self.end_headers()

        write_body = self.command != "HEAD"
        while True:
            item = chunks.get()
            if item is None:
                return
            if isinstance(item, Exception):
                LOG.warning("link proxy stream failed after response head: %s", item)
                self.close_connection = True
                return
            if not isinstance(item, bytes):
                LOG.warning("link proxy stream returned unexpected item: %r", item)
                self.close_connection = True
                return
            if not write_body:
                continue
            try:
                self.wfile.write(item)
                self.wfile.flush()
            except (BrokenPipeError, ConnectionError):
                future.cancel()
                self.close_connection = True
                return

    def _read_body(self) -> bytes | None:
        if self.headers.get("Transfer-Encoding") is not None:
            self.send_error(400, "Transfer-Encoding is not supported")
            self.close_connection = True
            return None

        content_length = self.headers.get("Content-Length")
        if content_length is None:
            return b""
        try:
            length = int(content_length)
        except ValueError:
            self.send_error(400, "Bad Content-Length")
            self.close_connection = True
            return None
        if length < 0:
            self.send_error(400, "Bad Content-Length")
            self.close_connection = True
            return None
        return self.rfile.read(length)

    def _send_gateway_error(self, exc: Exception) -> None:
        if isinstance(exc, TunnelRequestError):
            detail = exc.reason
        else:
            detail = type(exc).__name__
        self.send_error(502, f"Link tunnel failed: {detail}")
        self.close_connection = True

    def log_message(self, fmt: str, *args: object) -> None:
        LOG.debug("%s - %s", self.address_string(), fmt % args)


def _origin_path(raw_path: str) -> str:
    if raw_path.startswith("http://") or raw_path.startswith("https://"):
        parsed = urlsplit(raw_path)
        path = parsed.path or "/"
        if parsed.query:
            return f"{path}?{parsed.query}"
        return path
    return raw_path or "/"


def _forward_request_headers(
    headers: Iterable[tuple[str, str]],
    body_length: int,
) -> dict[str, str]:
    forwarded: dict[str, str] = {}
    for name, value in headers:
        if name.lower() == "authorization":
            continue
        if name.lower() in REQUEST_HOP_BY_HOP:
            continue
        forwarded[name] = value
    forwarded["Content-Length"] = str(body_length)
    return forwarded
