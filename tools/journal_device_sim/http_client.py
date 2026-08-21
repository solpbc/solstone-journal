# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Dependency-free streaming HTTP client for the local solstone link bridge."""

from __future__ import annotations

import http.client
import hashlib
import json
import secrets
from dataclasses import dataclass
from pathlib import Path
from typing import Any, BinaryIO, Iterable
from urllib.parse import urlencode, urlsplit

PROTOCOL_HEADER = "X-Solstone-Protocol-Version"
MAX_RESPONSE_BYTES = 4 * 1024 * 1024


class HttpRequestError(RuntimeError):
    """The bridge request did not yield a bounded JSON response."""


@dataclass(frozen=True)
class HttpResponse:
    status: int
    body: dict[str, Any]


@dataclass(frozen=True)
class MultipartPart:
    name: str
    filename: str | None
    data: bytes | Path | BinaryIO
    content_type: str
    size: int | None = None
    sha256: str | None = None


@dataclass(frozen=True)
class MultipartUpload:
    filename: str
    handle: BinaryIO
    size: int
    sha256: str


def _quoted_header_value(value: str) -> str:
    if not value or any(
        character == '"' or ord(character) < 32 or ord(character) == 127
        for character in value
    ):
        raise HttpRequestError("multipart name or filename is unsafe")
    return value


def _part_prefix(boundary: str, part: MultipartPart) -> bytes:
    name = _quoted_header_value(part.name)
    disposition = f'Content-Disposition: form-data; name="{name}"'
    if part.filename is not None:
        filename = _quoted_header_value(part.filename)
        disposition += f'; filename="{filename}"'
    return (
        f"--{boundary}\r\n{disposition}\r\nContent-Type: {part.content_type}\r\n\r\n"
    ).encode("utf-8")


def _data_size(part: MultipartPart) -> int:
    if part.size is not None:
        return part.size
    if isinstance(part.data, bytes):
        return len(part.data)
    if isinstance(part.data, Path):
        return part.data.stat().st_size
    raise HttpRequestError("streaming multipart parts require an explicit size")


def multipart_length(boundary: str, parts: Iterable[MultipartPart]) -> int:
    total = 0
    for part in parts:
        total += len(_part_prefix(boundary, part)) + _data_size(part) + 2
    return total + len(f"--{boundary}--\r\n".encode("ascii"))


def _send_part_data(
    connection: http.client.HTTPConnection, part: MultipartPart
) -> None:
    if isinstance(part.data, bytes):
        connection.send(part.data)
        return
    if isinstance(part.data, Path):
        with part.data.open("rb") as handle:
            while chunk := handle.read(1024 * 1024):
                connection.send(chunk)
        return
    if part.size is None or part.sha256 is None:
        raise HttpRequestError(
            "streaming multipart files require an exact size and SHA-256"
        )
    remaining = part.size
    digest = hashlib.sha256()
    while remaining:
        chunk = part.data.read(min(1024 * 1024, remaining))
        if not chunk:
            raise HttpRequestError("fixture file became shorter during upload")
        connection.send(chunk)
        digest.update(chunk)
        remaining -= len(chunk)
    if part.data.read(1):
        raise HttpRequestError("fixture file became longer during upload")
    if digest.hexdigest() != part.sha256:
        raise HttpRequestError("fixture file digest changed during upload")


class BridgeHttpClient:
    """HTTP/1.1 client restricted to a loopback link-bridge origin."""

    def __init__(self, base_url: str, timeout: float = 30.0) -> None:
        parsed = urlsplit(base_url)
        if parsed.scheme != "http" or parsed.username or parsed.password:
            raise HttpRequestError("bridge URL must be an unauthenticated http:// URL")
        if parsed.hostname not in {"127.0.0.1", "::1"}:
            raise HttpRequestError("bridge URL must resolve explicitly to loopback")
        if parsed.query or parsed.fragment:
            raise HttpRequestError("bridge URL cannot contain a query or fragment")
        self._host = parsed.hostname
        self._port = parsed.port or 80
        self._prefix = parsed.path.rstrip("/")
        self._timeout = timeout

    def _connection(self) -> http.client.HTTPConnection:
        return http.client.HTTPConnection(self._host, self._port, timeout=self._timeout)

    def _read_response(self, response: http.client.HTTPResponse) -> HttpResponse:
        declared = response.getheader("Content-Length")
        if declared is not None:
            try:
                declared_size = int(declared)
            except ValueError as error:
                raise HttpRequestError(
                    "response Content-Length is malformed"
                ) from error
            if declared_size > MAX_RESPONSE_BYTES:
                raise HttpRequestError("response exceeds the 4 MiB evidence bound")
        raw = response.read(MAX_RESPONSE_BYTES + 1)
        if len(raw) > MAX_RESPONSE_BYTES:
            raise HttpRequestError("response exceeds the 4 MiB evidence bound")
        try:
            body = json.loads(raw)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise HttpRequestError(
                f"bridge returned HTTP {response.status} with a non-JSON body"
            ) from error
        if not isinstance(body, dict):
            raise HttpRequestError("bridge JSON response must be an object")
        return HttpResponse(status=response.status, body=body)

    def get_json(self, path: str, query: dict[str, str] | None = None) -> HttpResponse:
        target = f"{self._prefix}{path}"
        if query:
            target = f"{target}?{urlencode(query)}"
        connection = self._connection()
        try:
            connection.request(
                "GET",
                target,
                headers={PROTOCOL_HEADER: "3", "Accept": "application/json"},
            )
            return self._read_response(connection.getresponse())
        except (OSError, http.client.HTTPException) as error:
            raise HttpRequestError(
                f"GET {path} failed: {type(error).__name__}"
            ) from error
        finally:
            connection.close()

    def post_multipart(
        self,
        path: str,
        envelope: dict[str, Any],
        files: Iterable[MultipartUpload],
    ) -> HttpResponse:
        envelope_bytes = json.dumps(
            envelope, sort_keys=True, separators=(",", ":"), ensure_ascii=True
        ).encode("ascii")
        parts = [
            MultipartPart(
                name="envelope",
                filename=None,
                data=envelope_bytes,
                content_type="application/json",
            )
        ]
        parts.extend(
            MultipartPart(
                name="files",
                filename=upload.filename,
                data=upload.handle,
                content_type="application/octet-stream",
                size=upload.size,
                sha256=upload.sha256,
            )
            for upload in files
        )
        boundary = f"solstone-device-sim-{secrets.token_hex(16)}"
        content_length = multipart_length(boundary, parts)
        target = f"{self._prefix}{path}"
        connection = self._connection()
        try:
            connection.putrequest("POST", target)
            connection.putheader(
                "Content-Type", f"multipart/form-data; boundary={boundary}"
            )
            connection.putheader("Content-Length", str(content_length))
            connection.putheader(PROTOCOL_HEADER, "3")
            connection.putheader("Accept", "application/json")
            connection.endheaders()
            for part in parts:
                connection.send(_part_prefix(boundary, part))
                _send_part_data(connection, part)
                connection.send(b"\r\n")
            connection.send(f"--{boundary}--\r\n".encode("ascii"))
            return self._read_response(connection.getresponse())
        except (OSError, http.client.HTTPException) as error:
            raise HttpRequestError(
                f"POST {path} failed before a trustworthy response: {type(error).__name__}"
            ) from error
        finally:
            connection.close()
