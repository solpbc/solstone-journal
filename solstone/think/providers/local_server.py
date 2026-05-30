# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Connect-only client for the supervisor-owned local llama-server."""

from __future__ import annotations

from dataclasses import dataclass

from solstone.think.models import LOCAL_MODEL
from solstone.think.providers.local import LocalProviderError
from solstone.think.utils import read_service_port

STATE_IDLE = "idle"
STATE_STARTING = "starting"
STATE_LOADING = "loading"
STATE_READY = "ready"
STATE_FAILED = "failed"
STATE_STOPPED = "stopped"

_HOST = "127.0.0.1"
_SERVICE_NAME = "local"

# COPY REVIEW: placeholder owner-facing copy; founder-gated before ship.
LOCAL_MODEL_NOT_READY_COPY = "Local model is not ready yet."


@dataclass(frozen=True)
class LocalServerInfo:
    model_id: str
    port: int
    base_url: str
    state: str
    binary_path: str | None = None
    model_path: str | None = None


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


def connect() -> LocalServerInfo:
    port = read_service_port(_SERVICE_NAME)
    if port is None:
        raise LocalProviderError("local_model_not_ready", LOCAL_MODEL_NOT_READY_COPY)
    state, _ = _probe_health(port)
    if state != STATE_READY:
        raise LocalProviderError("local_model_not_ready", LOCAL_MODEL_NOT_READY_COPY)
    return LocalServerInfo(
        model_id=LOCAL_MODEL,
        port=port,
        base_url=_base_url(port),
        state=STATE_READY,
    )


__all__ = [
    "LOCAL_MODEL_NOT_READY_COPY",
    "LocalServerInfo",
    "STATE_IDLE",
    "STATE_STARTING",
    "STATE_LOADING",
    "STATE_READY",
    "STATE_FAILED",
    "STATE_STOPPED",
    "connect",
    "is_healthy",
]
