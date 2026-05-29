# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import asyncio
import json
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import pytest

from solstone.convey.secure_listener.identity import ConveyIdentity
from solstone.convey.secure_listener.wsgi import DispatchResult, dispatch_stream
from solstone.think.link.client import _http_request_bytes, _parse_http_response


@dataclass(frozen=True)
class DispatchResponse:
    result: DispatchResult
    status: int
    headers: dict[str, str]
    body: bytes
    writer: FakeStreamWriter


class FakeStreamWriter:
    stream_id = 1

    def __init__(self) -> None:
        self.data = bytearray()
        self.closed = False
        self.reset_called = False

    async def write(self, data: bytes) -> None:
        self.data.extend(data)

    async def close(self) -> None:
        self.closed = True

    async def reset(self) -> None:
        self.reset_called = True
        self.closed = True


def make_convey_app(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    *,
    link: dict[str, Any] | None = None,
) -> tuple[Any, Path]:
    journal = tmp_path / "journal"
    journal.mkdir()
    write_config(journal, link=link)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    import solstone.convey as convey
    import solstone.think.link.runtime as link_runtime
    import solstone.think.push.runtime as push_runtime
    import solstone.think.voice.runtime as voice_runtime

    monkeypatch.setattr(convey, "start_chat_runtime", lambda _app: None)
    monkeypatch.setattr(link_runtime, "start_link_runtime", lambda _app: None)
    monkeypatch.setattr(push_runtime, "start_push_runtime", lambda _app: None)
    monkeypatch.setattr(voice_runtime, "start_voice_runtime", lambda _app: None)

    app = convey.create_app(journal=str(journal))
    return app, journal


def write_config(
    journal: Path,
    *,
    link: dict[str, Any] | None = None,
    trust_localhost: bool = True,
) -> None:
    config: dict[str, Any] = {
        "convey": {"trust_localhost": trust_localhost},
        "setup": {"completed_at": 1700000000000},
    }
    if link is not None:
        config["link"] = link
    config_path = journal / "config" / "journal.json"
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")


def certless_identity() -> ConveyIdentity:
    return ConveyIdentity(
        mode="pl-via-spl",
        fingerprint=None,
        device_label=None,
        paired_at=None,
        session_id="test-certless",
    )


def pl_identity(fingerprint: str) -> ConveyIdentity:
    return ConveyIdentity(
        mode="pl-via-spl",
        fingerprint=fingerprint,
        device_label="phone",
        paired_at="2026-05-29T00:00:00Z",
        session_id="test-pl",
    )


async def dispatch_request(
    app: Any,
    identity: ConveyIdentity,
    method: str,
    path: str,
    *,
    body: bytes = b"",
    headers: dict[str, str] | None = None,
) -> DispatchResponse:
    request_bytes = _http_request_bytes(method, path, headers=headers, body=body)
    reader = asyncio.StreamReader()
    reader.feed_data(request_bytes)
    reader.feed_eof()
    writer = FakeStreamWriter()
    loop = asyncio.get_running_loop()
    with ThreadPoolExecutor(max_workers=1) as executor:
        result = await dispatch_stream(app, identity, reader, writer, loop, executor)
    status, response_headers, response_body = _parse_http_response(bytes(writer.data))
    return DispatchResponse(
        result=result,
        status=status,
        headers=response_headers,
        body=response_body,
        writer=writer,
    )
