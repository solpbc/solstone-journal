# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Async batch processing through one native generate session."""

from __future__ import annotations

import asyncio
import json
import time
from contextlib import suppress
from typing import Any, List, Optional, Union

from solstone.think import generate_client
from solstone.think.models import (
    SchemaValidationError,
    finish_reason_error,
    resolve_provider,
)
from solstone.think.providers.shared import classify_provider_error


class BatchRequest:
    """Mutable request object for a single LLM API call.

    Callers may add arbitrary tracking attributes. Execution populates
    ``response``, ``error``, ``duration``, ``model_used``, ``reason_code``,
    ``provider``, and ``reset_at_ms``.
    """

    def __init__(
        self,
        contents: Union[str, List[Any]],
        context: str,
        temperature: float = 0.3,
        max_output_tokens: int = 8192 * 2,
        system_instruction: Optional[str] = None,
        json_output: bool = False,
        json_schema: Optional[dict] = None,
        thinking_budget: Optional[int] = None,
        timeout_s: Optional[float] = None,
    ):
        self.contents = contents
        self.context = context
        self.temperature = temperature
        self.max_output_tokens = max_output_tokens
        self.system_instruction = system_instruction
        self.json_output = json_output
        self.json_schema = json_schema
        self.thinking_budget = thinking_budget
        self.timeout_s = timeout_s

        self.response: Optional[str] = None
        self.error: Optional[str] = None
        self.duration: float = 0.0
        self.model_used: str = ""
        self.reason_code: Optional[str] = None
        self.provider: Optional[str] = None
        self.reset_at_ms: Optional[int] = None


class Batch:
    """Submit many generate requests through one native session child.

    Requests may be added while results are being drained and a drained batch
    remains reusable until :meth:`aclose` is called.
    """

    def __init__(self, max_concurrent: int = 5):
        self.max_concurrent = max_concurrent
        self.result_queue: asyncio.Queue[BatchRequest] = asyncio.Queue()
        self._pending: dict[str, tuple[BatchRequest, float]] = {}
        self._outbound: asyncio.Queue[str] = asyncio.Queue()
        self._next_request_id = 0
        self._process: asyncio.subprocess.Process | None = None
        self._start_task: asyncio.Task[None] | None = None
        self._writer_task: asyncio.Task[None] | None = None
        self._stdout_task: asyncio.Task[None] | None = None
        self._stderr_task: asyncio.Task[None] | None = None
        self._close_task: asyncio.Task[None] | None = None
        self._session_error: Exception | None = None
        self._closed = False

    def create(
        self,
        contents: Union[str, List[Any]],
        context: str,
        temperature: float = 0.3,
        max_output_tokens: int = 8192 * 2,
        system_instruction: Optional[str] = None,
        json_output: bool = False,
        json_schema: Optional[dict] = None,
        thinking_budget: Optional[int] = None,
        timeout_s: Optional[float] = None,
    ) -> BatchRequest:
        """Create a request that can be customized before :meth:`add`."""
        return BatchRequest(
            contents=contents,
            context=context,
            temperature=temperature,
            max_output_tokens=max_output_tokens,
            system_instruction=system_instruction,
            json_output=json_output,
            json_schema=json_schema,
            thinking_budget=thinking_budget,
            timeout_s=timeout_s,
        )

    def add(self, request: BatchRequest) -> None:
        """Queue a request without blocking, starting the session lazily.

        This may be called while :meth:`drain_batch` is yielding results or
        after a prior drain completes.
        """
        if self._closed:
            raise RuntimeError("cannot add a request to a closed batch")
        if self._session_error is not None:
            raise RuntimeError(f"batch session is unavailable: {self._session_error}")

        request_id = f"batch-{self._next_request_id}"
        self._next_request_id += 1
        payload = generate_client._request(
            contents=request.contents,
            context=request.context,
            temperature=request.temperature,
            max_output_tokens=request.max_output_tokens,
            system_instruction=request.system_instruction,
            json_output=request.json_output or request.json_schema is not None,
            json_schema=request.json_schema,
            thinking_budget=request.thinking_budget,
            timeout_s=request.timeout_s,
            num_retries=None,
            inference_retry_index=0,
            local_exclusive_admission=False,
            enforce_responsiveness=True,
        )
        payload["id"] = request_id
        self._pending[request_id] = (request, time.time())
        self._outbound.put_nowait(json.dumps(payload, allow_nan=False) + "\n")
        if self._start_task is None:
            self._start_task = asyncio.create_task(self._start_session())

    def update(self, request: BatchRequest, **kwargs) -> None:
        """Update request attributes, clear its result fields, and resubmit it."""
        for key, value in kwargs.items():
            setattr(request, key, value)
        request.response = None
        request.error = None
        request.duration = 0.0
        request.reason_code = None
        request.provider = None
        request.reset_at_ms = None
        self.add(request)

    def is_drained(self) -> bool:
        """Return true when no requests remain pending and no results are queued."""
        return not self._pending and self.result_queue.empty()

    async def wait_until_drained(self) -> None:
        """Wait until all submitted work completes and its results are consumed."""
        while not self.is_drained():
            await asyncio.sleep(0.1)

    async def _start_session(self) -> None:
        try:
            self._process = await asyncio.create_subprocess_exec(
                str(generate_client._native_binary()),
                "generate",
                "--session",
                "--max-in-flight",
                str(self.max_concurrent),
                stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
        except Exception as exc:
            await self._fail_session(
                RuntimeError(f"generate native session could not start: {exc}")
            )
            return

        assert self._process.stdin is not None
        assert self._process.stdout is not None
        assert self._process.stderr is not None
        self._writer_task = asyncio.create_task(self._write_requests())
        self._stdout_task = asyncio.create_task(self._drain_stdout())
        self._stderr_task = asyncio.create_task(self._drain_stderr())

    async def _write_requests(self) -> None:
        assert self._process is not None and self._process.stdin is not None
        try:
            while True:
                line = await self._outbound.get()
                try:
                    self._process.stdin.write(line.encode())
                    await self._process.stdin.drain()
                except (BrokenPipeError, ConnectionError, OSError) as exc:
                    await self._fail_session(
                        RuntimeError(f"generate native session write failed: {exc}")
                    )
                    return
                finally:
                    self._outbound.task_done()
        except asyncio.CancelledError:
            raise

    async def _drain_stdout(self) -> None:
        assert self._process is not None and self._process.stdout is not None
        while line := await self._process.stdout.readline():
            try:
                response = generate_client._decode_protocol_response(
                    line.decode(), session=True
                )
                request_id = response["id"]
                pending = self._pending.pop(request_id, None)
                if pending is None:
                    raise RuntimeError(
                        f"generate native session returned unknown response id {request_id!r}"
                    )
                request, start_time = pending
                await self._execute_request(request, start_time, response)
            except Exception as exc:
                await self._fail_session(
                    RuntimeError(f"generate native session protocol failure: {exc}")
                )
                break

        if self._pending and self._session_error is None:
            await self._fail_session(
                RuntimeError(
                    "generate native session ended before all requests completed"
                )
            )
        elif not self._closed and self._session_error is None:
            await self._fail_session(
                RuntimeError("generate native session ended unexpectedly")
            )

    async def _drain_stderr(self) -> None:
        assert self._process is not None and self._process.stderr is not None
        while await self._process.stderr.readline():
            pass

    async def _execute_request(
        self,
        request: BatchRequest,
        start_time: float,
        response: dict[str, Any],
    ) -> None:
        """Apply caller-side result validation and publish one completion."""
        try:
            result = generate_client._response_result_from_response(response)
            error = finish_reason_error(result, json_output=request.json_output)
            if error is not None:
                raise error
            validation = result.get("schema_validation")
            if isinstance(validation, dict) and validation.get("valid") is False:
                raise SchemaValidationError(
                    validation.get("errors") or [], result.get("text", "")
                )
            request.duration = time.time() - start_time
            request.response = result["text"]
            request.error = None
            request.model_used = str(result.get("model") or "")
        except Exception as exc:
            self._set_request_error(request, start_time, exc)
        await self.result_queue.put(request)

    def _set_request_error(
        self, request: BatchRequest, start_time: float, exc: Exception
    ) -> None:
        request.duration = time.time() - start_time
        request.response = None
        request.error = str(exc)
        request.reason_code = getattr(exc, "reason_code", None) or (
            classify_provider_error(exc, request.context or "")
        )
        request.reset_at_ms = getattr(exc, "reset_at_ms", None)
        request.provider = getattr(exc, "provider", None)
        if request.provider is None:
            try:
                request.provider = resolve_provider("generate")[0]
            except (KeyError, TypeError, ValueError):
                request.provider = None

    async def _fail_session(self, exc: Exception) -> None:
        if self._session_error is not None:
            return
        self._session_error = exc
        current_task = asyncio.current_task()
        if self._writer_task is not None and self._writer_task is not current_task:
            self._writer_task.cancel()
        self._discard_outbound()
        self._close_stdin()
        pending = list(self._pending.values())
        self._pending.clear()
        for request, start_time in pending:
            self._set_request_error(request, start_time, exc)
            await self.result_queue.put(request)

    def _discard_outbound(self) -> None:
        while not self._outbound.empty():
            self._outbound.get_nowait()
            self._outbound.task_done()

    def _close_stdin(self) -> None:
        if self._process is not None and self._process.stdin is not None:
            self._process.stdin.close()

    async def aclose(self) -> None:
        """Drain the session, reap its child process, and reject later additions."""
        if self._close_task is not None:
            await self._close_task
            return
        self._closed = True
        self._close_task = asyncio.create_task(self._close_session())
        await self._close_task

    async def _close_session(self) -> None:
        if self._start_task is None:
            return
        await self._start_task
        if self._process is None:
            return

        if self._session_error is None:
            await self._outbound.join()
            if self._writer_task is not None:
                self._writer_task.cancel()
                with suppress(asyncio.CancelledError):
                    await self._writer_task
            assert self._process.stdin is not None
            terminal_schema = generate_client._generate_contract()["framing"][
                "session"
            ]["terminal"]["schema"]
            self._process.stdin.write(
                (json.dumps({"schema": terminal_schema}) + "\n").encode()
            )
            await self._process.stdin.drain()
            self._close_stdin()
        else:
            self._close_stdin()

        if self._process.stdin is not None:
            with suppress(AttributeError, BrokenPipeError, ConnectionError):
                await self._process.stdin.wait_closed()
        if self._stdout_task is not None:
            await self._stdout_task
        if self._stderr_task is not None:
            await self._stderr_task
        await self._process.wait()

    async def drain_batch(self):
        """Yield completed requests until no pending work or results remain.

        This may be called multiple times; each invocation drains work that is
        currently pending or added while iteration is in progress. It does not
        close the native session.
        """
        while True:
            if not self._pending and self.result_queue.empty():
                break
            try:
                yield await asyncio.wait_for(self.result_queue.get(), timeout=0.1)
            except asyncio.TimeoutError:
                continue


__all__ = ["BatchRequest", "Batch"]
