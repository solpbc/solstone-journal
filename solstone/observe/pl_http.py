# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json as json_lib
from typing import Any
from urllib.parse import urlsplit

from urllib3.filepost import encode_multipart_formdata

from solstone.think.link.dialer import TunnelClient


class PlHttpResponse:
    def __init__(
        self,
        status_code: int,
        headers: dict[str, str],
        content: bytes,
    ) -> None:
        self.status_code = status_code
        self.headers = headers
        self.content = content

    @property
    def text(self) -> str:
        return self.content.decode("utf-8", errors="replace")

    def json(self) -> Any:
        return json_lib.loads(self.content.decode("utf-8") or "{}")

    def raise_for_status(self) -> None:
        if self.status_code >= 400:
            raise RuntimeError(f"HTTP {self.status_code}: {self.text[:200]}")


class PlHttpSession:
    def __init__(self, tunnel: TunnelClient) -> None:
        self._tunnel = tunnel
        self.headers: dict[str, str] = {}

    def post(
        self,
        url: str,
        *,
        data: Any = None,
        json: Any = None,
        files: Any = None,
        headers: dict[str, str] | None = None,
        timeout: float | tuple[float, float | None] | None = None,
    ) -> PlHttpResponse:
        request_headers = self._headers(headers, url)
        body = b""

        if files is not None:
            body, content_type = _encode_multipart(data=data, files=files)
            request_headers["Content-Type"] = content_type
        elif json is not None:
            body = json_lib.dumps(json).encode("utf-8")
            request_headers.setdefault("Content-Type", "application/json")
        elif data is not None:
            if isinstance(data, bytes):
                body = data
            elif isinstance(data, str):
                body = data.encode("utf-8")
            elif isinstance(data, dict):
                body, content_type = encode_multipart_formdata(list(data.items()))
                request_headers["Content-Type"] = content_type
            else:
                body = bytes(data)

        return self._request("POST", url, headers=request_headers, body=body)

    def get(
        self,
        url: str,
        *,
        headers: dict[str, str] | None = None,
        timeout: float | tuple[float, float | None] | None = None,
    ) -> PlHttpResponse:
        return self._request("GET", url, headers=self._headers(headers, url), body=b"")

    def close(self) -> None:
        self._tunnel.close()

    def _request(
        self,
        method: str,
        url: str,
        *,
        headers: dict[str, str],
        body: bytes,
    ) -> PlHttpResponse:
        path = _path_from_url(url)
        status, response_headers, response_body = self._tunnel.request(
            method,
            path,
            headers=headers,
            body=body,
        )
        return PlHttpResponse(status, dict(response_headers), response_body)

    def _headers(self, headers: dict[str, str] | None, url: str) -> dict[str, str]:
        merged = dict(self.headers)
        if headers:
            merged.update(headers)
        # PL is already mutually authenticated; never forward bearer credentials.
        for name in list(merged):
            if name.lower() == "authorization":
                del merged[name]
        netloc = urlsplit(url).netloc
        if netloc:
            merged["Host"] = netloc
        return merged


def _path_from_url(url: str) -> str:
    parsed = urlsplit(url)
    if not parsed.scheme:
        return url
    path = parsed.path or "/"
    if parsed.query:
        return f"{path}?{parsed.query}"
    return path


def _encode_multipart(data: Any, files: Any) -> tuple[bytes, str]:
    fields: list[tuple[str, Any]] = []
    if isinstance(data, dict):
        fields.extend(data.items())
    elif data:
        fields.extend(data)

    for field_name, file_value in files:
        if isinstance(file_value, tuple):
            filename = file_value[0]
            payload = file_value[1]
            content_type = (
                file_value[2] if len(file_value) > 2 else "application/octet-stream"
            )
        else:
            filename = getattr(file_value, "name", "file")
            payload = file_value
            content_type = "application/octet-stream"
        if hasattr(payload, "read"):
            payload = payload.read()
        fields.append((field_name, (filename, payload, content_type)))

    return encode_multipart_formdata(fields)
