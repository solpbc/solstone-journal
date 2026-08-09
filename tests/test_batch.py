# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the native-session Batch processor."""

import asyncio
import json
from pathlib import Path
from unittest.mock import patch

import pytest

from solstone.think import generate_client
from solstone.think.batch import Batch, BatchRequest


def _generated_response(
    request_id: str, text: str, *, finish_reason: str = "stop"
) -> dict:
    return {
        "schema": "solstone-generate-response-v2",
        "id": request_id,
        "outcome": "generated",
        "text": text,
        "model": "native-model",
        "usage": {"input_tokens": 2, "output_tokens": 1, "total_tokens": 3},
        "finish_reason": finish_reason,
        "thinking": None,
        "schema_validation": None,
        "input_budget": None,
        "request_budget": None,
        "inference": None,
        "hints_applied": [],
    }


def _refused_response(request_id: str) -> dict:
    return {
        "schema": "solstone-generate-response-v2",
        "id": request_id,
        "outcome": "refused",
        "reason": "provider-response-invalid",
        "reason_code": "provider_response_invalid",
        "retryable": True,
        "blocking": False,
        "reset_at_ms": None,
        "provider": "local",
        "detail": "mocked provider failure",
    }


class _FakeReader:
    def __init__(self):
        self._lines: asyncio.Queue[bytes] = asyncio.Queue()

    def feed_data(self, line: bytes) -> None:
        self._lines.put_nowait(line)

    def feed_eof(self) -> None:
        self._lines.put_nowait(b"")

    async def readline(self) -> bytes:
        return await self._lines.get()


class _FakeSessionProcess:
    def __init__(
        self, *, reverse_first_two: bool = False, eof_after_first_request: bool = False
    ):
        self.stdout = _FakeReader()
        self.stderr = _FakeReader()
        self.stderr.feed_eof()
        self.stdin = _FakeSessionStdin(
            self,
            reverse_first_two=reverse_first_two,
            eof_after_first_request=eof_after_first_request,
        )
        self.returncode: int | None = None
        self._exited = asyncio.Event()

    async def wait(self) -> int:
        await self._exited.wait()
        assert self.returncode is not None
        return self.returncode

    def exit(self) -> None:
        if self.returncode is None:
            self.returncode = 0
            self.stdout.feed_eof()
            self._exited.set()


class _FakeSessionStdin:
    def __init__(
        self,
        process: _FakeSessionProcess,
        *,
        reverse_first_two: bool,
        eof_after_first_request: bool,
    ):
        self._process = process
        self._reverse_first_two = reverse_first_two
        self._eof_after_first_request = eof_after_first_request
        self.requests: list[dict] = []
        self.terminal: dict | None = None

    def write(self, data: bytes) -> None:
        record = json.loads(data)
        if record["schema"] == "solstone-generate-session-terminal-v2":
            self.terminal = record
            return
        self.requests.append(record)
        if self._eof_after_first_request and len(self.requests) == 1:
            self._process.exit()
            return
        if self._reverse_first_two and len(self.requests) == 1:
            return
        if self._reverse_first_two and len(self.requests) == 2:
            self._respond(self.requests[1])
            self._respond(self.requests[0])
            return
        self._respond(record)

    async def drain(self) -> None:
        return None

    def close(self) -> None:
        self._process.exit()

    async def wait_closed(self) -> None:
        return None

    def _respond(self, request: dict) -> None:
        contents = request["contents"]
        text = contents[0]["text"]
        response = (
            _refused_response(request["id"])
            if text == "refuse"
            else _generated_response(request["id"], text)
        )
        self._process.stdout.feed_data(json.dumps(response).encode() + b"\n")


def test_batch_request_creation_and_custom_attributes():
    request = BatchRequest(contents="Test prompt", context="test.context")
    request.frame_id = 123

    assert request.contents == "Test prompt"
    assert request.temperature == 0.3
    assert request.frame_id == 123
    assert request.response is None
    assert request.error is None


def test_batch_update_clears_error_metadata():
    batch = Batch(max_concurrent=5)
    request = batch.create(contents="Retry prompt", context="test.context")
    request.reason_code = "provider_key_invalid"
    request.provider = "anthropic"
    request.reset_at_ms = 12345
    request.error = "bad key"

    with patch.object(batch, "add") as mock_add:
        batch.update(request)

    assert request.reason_code is None
    assert request.provider is None
    assert request.reset_at_ms is None
    assert request.error is None
    mock_add.assert_called_once_with(request)


@pytest.mark.asyncio
async def test_batch_uses_one_session_and_routes_success_and_refusal(monkeypatch):
    spawned: list[_FakeSessionProcess] = []

    async def fake_create_subprocess_exec(*_args, **_kwargs):
        process = _FakeSessionProcess(reverse_first_two=True)
        spawned.append(process)
        return process

    monkeypatch.setattr(generate_client, "_native_binary", lambda: Path("/native/core"))
    monkeypatch.setattr(asyncio, "create_subprocess_exec", fake_create_subprocess_exec)

    batch = Batch(max_concurrent=2)
    first = batch.create(contents="first", context="test.context")
    first.label = "first"
    refusal = batch.create(contents="refuse", context="test.context")
    refusal.label = "refusal"
    third = batch.create(contents="third", context="test.context")
    third.label = "third"
    batch.add(first)
    batch.add(refusal)
    batch.add(third)

    completed = [request async for request in batch.drain_batch()]
    await batch.aclose()

    assert len(spawned) == 1
    assert len(spawned[0].stdin.requests) == 3
    assert spawned[0].stdin.terminal == {
        "schema": "solstone-generate-session-terminal-v2"
    }
    by_label = {request.label: request for request in completed}
    assert by_label["first"].response == "first"
    assert by_label["third"].response == "third"
    assert by_label["refusal"].response is None
    assert by_label["refusal"].reason_code == "provider_response_invalid"
    assert by_label["refusal"].provider == "local"


@pytest.mark.asyncio
async def test_batch_can_add_after_draining_until_closed(monkeypatch):
    async def fake_create_subprocess_exec(*_args, **_kwargs):
        return _FakeSessionProcess()

    monkeypatch.setattr(generate_client, "_native_binary", lambda: Path("/native/core"))
    monkeypatch.setattr(asyncio, "create_subprocess_exec", fake_create_subprocess_exec)

    batch = Batch()
    first = batch.create(contents="first", context="test.context")
    batch.add(first)
    assert [request.response async for request in batch.drain_batch()] == ["first"]

    second = batch.create(contents="second", context="test.context")
    batch.add(second)
    assert [request.response async for request in batch.drain_batch()] == ["second"]

    await batch.aclose()
    with pytest.raises(RuntimeError, match="closed batch"):
        batch.add(batch.create(contents="later", context="test.context"))


@pytest.mark.asyncio
async def test_batch_start_failure_completes_every_pending_request(monkeypatch):
    def fail_binary_resolution() -> Path:
        raise RuntimeError("native core unavailable")

    monkeypatch.setattr(generate_client, "_native_binary", fail_binary_resolution)

    batch = Batch()
    batch.add(batch.create(contents="first", context="test.context"))
    batch.add(batch.create(contents="second", context="test.context"))
    completed = [request async for request in batch.drain_batch()]
    await batch.aclose()

    assert len(completed) == 2
    assert all(request.response is None for request in completed)
    assert all(
        "native core unavailable" in (request.error or "") for request in completed
    )


@pytest.mark.asyncio
async def test_batch_eof_completes_every_pending_request(monkeypatch):
    async def fake_create_subprocess_exec(*_args, **_kwargs):
        return _FakeSessionProcess(eof_after_first_request=True)

    monkeypatch.setattr(generate_client, "_native_binary", lambda: Path("/native/core"))
    monkeypatch.setattr(asyncio, "create_subprocess_exec", fake_create_subprocess_exec)

    batch = Batch()
    batch.add(batch.create(contents="first", context="test.context"))
    batch.add(batch.create(contents="second", context="test.context"))
    completed = [request async for request in batch.drain_batch()]
    await batch.aclose()

    assert len(completed) == 2
    assert all(request.response is None for request in completed)
    assert all(
        "ended before all requests" in (request.error or "") for request in completed
    )


@pytest.mark.asyncio
async def test_batch_applies_caller_side_schema_validation(monkeypatch):
    class InvalidSchemaProcess(_FakeSessionProcess):
        pass

    async def fake_create_subprocess_exec(*_args, **_kwargs):
        process = InvalidSchemaProcess()
        original_respond = process.stdin._respond

        def respond_with_invalid_schema(request: dict) -> None:
            original_respond(request)
            line = process.stdout._lines.get_nowait()
            response = json.loads(line)
            response["schema_validation"] = {
                "valid": False,
                "errors": [{"path": "", "constraint": "type", "message": "bad"}],
            }
            process.stdout.feed_data(json.dumps(response).encode() + b"\n")

        process.stdin._respond = respond_with_invalid_schema
        return process

    monkeypatch.setattr(generate_client, "_native_binary", lambda: Path("/native/core"))
    monkeypatch.setattr(asyncio, "create_subprocess_exec", fake_create_subprocess_exec)

    batch = Batch()
    request = batch.create(
        contents="schema", context="test.context", json_schema={"type": "object"}
    )
    batch.add(request)
    completed = [item async for item in batch.drain_batch()]
    await batch.aclose()

    assert len(completed) == 1
    assert completed[0].response is None
    assert completed[0].error is not None
    assert completed[0].reason_code == "unknown"
    assert "JSON response failed schema validation" in completed[0].error


@pytest.mark.asyncio
async def test_batch_applies_caller_side_finish_reason_check(monkeypatch):
    async def fake_create_subprocess_exec(*_args, **_kwargs):
        process = _FakeSessionProcess()
        original_respond = process.stdin._respond

        def respond_with_truncation(request: dict) -> None:
            original_respond(request)
            line = process.stdout._lines.get_nowait()
            response = json.loads(line)
            response["finish_reason"] = "max_tokens"
            process.stdout.feed_data(json.dumps(response).encode() + b"\n")

        process.stdin._respond = respond_with_truncation
        return process

    monkeypatch.setattr(generate_client, "_native_binary", lambda: Path("/native/core"))
    monkeypatch.setattr(asyncio, "create_subprocess_exec", fake_create_subprocess_exec)

    batch = Batch()
    batch.add(batch.create(contents="partial", context="test.context"))
    completed = [item async for item in batch.drain_batch()]
    await batch.aclose()

    assert len(completed) == 1
    assert completed[0].response is None
    assert completed[0].reason_code == "incomplete_text_length"


@pytest.mark.asyncio
async def test_empty_batch_never_spawns_a_session():
    batch = Batch()

    assert [request async for request in batch.drain_batch()] == []
    await batch.aclose()
