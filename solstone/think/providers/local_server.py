# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Lazy llama-server daemon manager for the local provider."""

from __future__ import annotations

import logging
import threading
import time
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from solstone.think.providers import local_install
from solstone.think.providers.local import LocalProviderError, normalize_model_id
from solstone.think.runner import ManagedProcess as RunnerManagedProcess
from solstone.think.utils import (
    find_available_port,
    get_journal,
    now_ms,
    read_service_port,
    write_service_port,
)

LOG = logging.getLogger(__name__)

STATE_IDLE = "idle"
STATE_STARTING = "starting"
STATE_LOADING = "loading"
STATE_READY = "ready"
STATE_FAILED = "failed"
STATE_STOPPED = "stopped"

_HOST = "127.0.0.1"
_SERVICE_NAME = "local"
_DEFAULT_READY_TIMEOUT_S = 300.0
_HEALTH_POLL_INTERVAL_S = 1.0

_LOCK = threading.RLock()
_PROCESS: RunnerManagedProcess | None = None
_PROCESS_MODEL_ID: str | None = None
_PROCESS_PORT: int | None = None


@dataclass(frozen=True)
class LocalServerInfo:
    model_id: str
    port: int
    base_url: str
    state: str
    binary_path: str | None = None
    model_path: str | None = None


def _emit(
    on_event: Callable[[dict], None] | None,
    state: str,
    *,
    model_id: str,
    port: int | None = None,
    reason_code: str | None = None,
    error: str | None = None,
) -> None:
    if not on_event:
        return
    payload: dict[str, Any] = {
        "event": "local_server",
        "state": state,
        "model": model_id,
        "ts": now_ms(),
    }
    if port is not None:
        payload["port"] = port
    if reason_code:
        payload["reason_code"] = reason_code
    if error:
        payload["error"] = error
    on_event(payload)


@contextmanager
def _server_file_lock() -> Iterator[None]:
    import fcntl

    health_dir = Path(get_journal()) / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    lock_path = health_dir / "local-server.lock"
    with lock_path.open("a+") as handle:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(handle.fileno(), fcntl.LOCK_UN)


def _base_url(port: int) -> str:
    return f"http://{_HOST}:{port}"


def _probe_health(port: int, timeout_s: float = 1.0) -> tuple[str, str | None]:
    import httpx

    try:
        response = httpx.get(f"{_base_url(port)}/health", timeout=timeout_s)
    except Exception as exc:
        return STATE_FAILED, str(exc)
    if response.status_code == 200:
        return STATE_READY, None
    if response.status_code == 503 and "loading model" in response.text.lower():
        return STATE_LOADING, None
    return STATE_FAILED, f"HTTP {response.status_code}: {response.text[:200]}"


def is_healthy() -> bool:
    port = read_service_port(_SERVICE_NAME)
    if port is None:
        return False
    state, _ = _probe_health(port)
    return state == STATE_READY


def _current_process_ready(model_id: str) -> LocalServerInfo | None:
    if (
        _PROCESS is None
        or _PROCESS_MODEL_ID != model_id
        or _PROCESS_PORT is None
        or _PROCESS.poll() is not None
    ):
        return None
    state, _ = _probe_health(_PROCESS_PORT)
    if state == STATE_READY:
        return LocalServerInfo(
            model_id=model_id,
            port=_PROCESS_PORT,
            base_url=_base_url(_PROCESS_PORT),
            state=STATE_READY,
        )
    return None


def _reattach_if_ready(model_id: str) -> LocalServerInfo | None:
    port = read_service_port(_SERVICE_NAME)
    if port is None:
        return None
    state, _ = _probe_health(port)
    if state != STATE_READY:
        return None
    return LocalServerInfo(
        model_id=model_id,
        port=port,
        base_url=_base_url(port),
        state=STATE_READY,
    )


def _spawn_server(
    model_id: str, binary_path: Path, model_path: Path, port: int
) -> None:
    global _PROCESS, _PROCESS_MODEL_ID, _PROCESS_PORT

    cmd = [
        str(binary_path),
        "-m",
        str(model_path),
        "--alias",
        model_id,
        "--host",
        _HOST,
        "--port",
        str(port),
    ]
    if "0.0.0.0" in cmd:
        raise LocalProviderError("unsafe_bind", "Local server may not bind 0.0.0.0.")
    _PROCESS = RunnerManagedProcess.spawn(cmd, ref="local-server")
    _PROCESS_MODEL_ID = model_id
    _PROCESS_PORT = port


def _clear_process() -> None:
    global _PROCESS, _PROCESS_MODEL_ID, _PROCESS_PORT
    if _PROCESS is None:
        return
    try:
        if _PROCESS.poll() is None:
            _PROCESS.terminate(timeout=15)
    finally:
        _PROCESS.cleanup()
        _PROCESS = None
        _PROCESS_MODEL_ID = None
        _PROCESS_PORT = None


def ensure_running(
    model_id: str,
    on_event: Callable[[dict], None] | None = None,
    *,
    ready_timeout_s: float = _DEFAULT_READY_TIMEOUT_S,
) -> LocalServerInfo:
    selected_model = normalize_model_id(model_id)
    with _LOCK:
        with _server_file_lock():
            ready = _current_process_ready(selected_model)
            if ready:
                return ready

            reattached = _reattach_if_ready(selected_model)
            if reattached:
                return reattached

            binary_path, gguf_path = local_install.ensure_artifacts_installed(
                selected_model
            )
            if _PROCESS is not None:
                _clear_process()
            port = find_available_port(_HOST)
            write_service_port(_SERVICE_NAME, port)
            _emit(on_event, STATE_STARTING, model_id=selected_model, port=port)
            try:
                _spawn_server(selected_model, binary_path, gguf_path, port)
            except Exception as exc:
                _emit(
                    on_event,
                    STATE_FAILED,
                    model_id=selected_model,
                    port=port,
                    reason_code="server_crashed",
                    error=str(exc),
                )
                raise

        deadline = time.monotonic() + ready_timeout_s
        loading_emitted = False
        while time.monotonic() < deadline:
            if _PROCESS is not None and _PROCESS.poll() is not None:
                reason = f"llama-server exited with code {_PROCESS.returncode}"
                _emit(
                    on_event,
                    STATE_FAILED,
                    model_id=selected_model,
                    port=port,
                    reason_code="server_crashed",
                    error=reason,
                )
                raise LocalProviderError("server_crashed", reason)

            state, error = _probe_health(port)
            if state == STATE_READY:
                _emit(on_event, STATE_READY, model_id=selected_model, port=port)
                return LocalServerInfo(
                    model_id=selected_model,
                    port=port,
                    base_url=_base_url(port),
                    state=STATE_READY,
                    binary_path=str(binary_path),
                    model_path=str(gguf_path),
                )
            if state == STATE_LOADING and not loading_emitted:
                _emit(on_event, STATE_LOADING, model_id=selected_model, port=port)
                loading_emitted = True
            elif state == STATE_FAILED and error:
                LOG.debug("local server health probe failed: %s", error)
            time.sleep(_HEALTH_POLL_INTERVAL_S)

        _emit(
            on_event,
            STATE_FAILED,
            model_id=selected_model,
            port=port,
            reason_code="model_load_timeout",
            error="Local model did not become ready before timeout.",
        )
        raise LocalProviderError(
            "model_load_timeout",
            "Local model did not become ready before timeout.",
        )


def stop(timeout_s: float = 15.0) -> None:
    global _PROCESS, _PROCESS_MODEL_ID, _PROCESS_PORT
    with _LOCK:
        if _PROCESS is None:
            return
        try:
            if _PROCESS.poll() is None:
                _PROCESS.terminate(timeout=timeout_s)
        finally:
            _clear_process()


__all__ = [
    "LocalServerInfo",
    "STATE_IDLE",
    "STATE_STARTING",
    "STATE_LOADING",
    "STATE_READY",
    "STATE_FAILED",
    "STATE_STOPPED",
    "ensure_running",
    "is_healthy",
    "stop",
]
