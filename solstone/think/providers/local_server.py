# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Connect-only client for the supervisor-owned local llama-server."""

from __future__ import annotations

import json
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from solstone.think import core_handshake
from solstone.think.models import LOCAL_MODEL
from solstone.think.providers.local import LocalProviderError
from solstone.think.utils import get_journal

STATE_IDLE = "idle"
STATE_STARTING = "starting"
STATE_LOADING = "loading"
STATE_READY = "ready"
STATE_FAILED = "failed"
STATE_STOPPED = "stopped"

_HOST = "127.0.0.1"

# Minimum/floor context window. This is the OpenHands agent
# `max_input_tokens` floor, the floor-tier llama-server `-c`, and the sizing
# function's lower clamp. Capable GPU server launch `-c` comes from
# select_server_tier().
LOCAL_MIN_CONTEXT_TOKENS = 16384


@dataclass(frozen=True)
class ServerTier:
    name: str
    context_tokens: int
    parallel_slots: int
    prompt_cache_mib: int
    resident_mib: int | None


# Tunable estimates — keep all tier values in these two instances; do not
# scatter literals elsewhere. The threshold is the only other tunable.
_CAPABLE_TIER_MIN_VRAM_MIB = 16000
# ``resident_mib`` is measured floor-tier brain residency under production
# launch args. ``None`` on capable is load-bearing: unmeasured residency means
# the co-location placement predicate can never fire at >=16 GiB.
_CAPABLE_TIER = ServerTier(
    name="capable",
    context_tokens=32768,
    parallel_slots=2,
    prompt_cache_mib=2048,
    resident_mib=None,
)
_FLOOR_TIER = ServerTier(
    name="floor",
    context_tokens=LOCAL_MIN_CONTEXT_TOKENS,
    parallel_slots=1,
    prompt_cache_mib=0,
    resident_mib=4147,
)

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
    served_model_id: str = LOCAL_MODEL
    parallel_slots: int = 1
    capacity_source: str = "default"
    profile: str = "floor"


@dataclass(frozen=True)
class ServerCapacity:
    parallel_slots: int
    source: str
    profile: str


@dataclass(frozen=True)
class ServerContextProps:
    n_ctx: int
    total_slots: int | None


def _base_url(port: int) -> str:
    return f"http://{_HOST}:{port}"


def select_server_tier(vram_mib: int) -> ServerTier:
    if vram_mib >= _CAPABLE_TIER_MIN_VRAM_MIB:
        return _CAPABLE_TIER
    return _FLOOR_TIER


def _fetch_health(
    port: int, timeout_s: float = 1.0
) -> tuple[str, str | None, dict[str, Any] | None]:
    import httpx

    try:
        response = httpx.get(f"{_base_url(port)}/health", timeout=timeout_s)
    except Exception as exc:
        return STATE_FAILED, str(exc), None
    if response.status_code == 200:
        try:
            body = response.json()
        except Exception:
            body = None
        return STATE_READY, None, body if isinstance(body, dict) else None
    if response.status_code == 503 and "loading model" in response.text.lower():
        return STATE_LOADING, None, None
    return STATE_FAILED, f"HTTP {response.status_code}: {response.text[:200]}", None


def _probe_health(port: int, timeout_s: float = 1.0) -> tuple[str, str | None]:
    state, error, _ = _fetch_health(port, timeout_s)
    return state, error


def fetch_props(port: int, timeout_s: float = 1.0) -> dict[str, Any] | None:
    """Read llama-server GET /props.

    Returns parsed JSON dict or None on any failure (never raises).
    """
    import httpx

    try:
        response = httpx.get(f"{_base_url(port)}/props", timeout=timeout_s)
        if response.status_code != 200:
            return None
        body = response.json()
    except Exception:
        return None
    return body if isinstance(body, dict) else None


def _extract_n_ctx(props: dict[str, Any]) -> int | None:
    """Effective context window from a /props body.

    Prefer top-level 'n_ctx'; fall back to
    props['default_generation_settings']['n_ctx']. Coerce to int; return None
    if absent or non-numeric.
    """
    if "n_ctx" in props:
        value = props["n_ctx"]
    else:
        settings = props.get("default_generation_settings")
        if not isinstance(settings, dict) or "n_ctx" not in settings:
            return None
        value = settings["n_ctx"]

    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def read_server_context_props(port: int) -> ServerContextProps | None:
    props = fetch_props(port)
    if props is None:
        return None
    n_ctx = _extract_n_ctx(props)
    if n_ctx is None or n_ctx <= 0:
        return None
    return ServerContextProps(n_ctx=n_ctx, total_slots=_extract_total_slots(props))


def write_local_context_window(tokens: int) -> None:
    health_dir = Path(get_journal()) / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    (health_dir / "local.ctx").write_text(str(tokens))


def clear_local_context_window() -> None:
    (Path(get_journal()) / "health" / "local.ctx").unlink(missing_ok=True)


def read_local_context_window() -> int | None:
    context_file = Path(get_journal()) / "health" / "local.ctx"
    try:
        return int(context_file.read_text().strip())
    except (FileNotFoundError, ValueError):
        return None


# Slot count assumed when no launched tier can be identified. Not a tier value:
# the mlx-vlm server on darwin exposes neither /props nor a ServerTier, and a
# server that never launched has no slots at all.
_UNKNOWN_SLOTS = 1

_SERVER_CAPACITY_CACHE: ServerCapacity | None = None


def _extract_total_slots(props: dict[str, Any]) -> int | None:
    """Positive ``total_slots`` from a /props body, else None."""
    try:
        slots = int(props["total_slots"])
    except (KeyError, TypeError, ValueError):
        return None
    return slots if slots > 0 else None


def _slots_from_launched_tier(context_tokens: int | None) -> int | None:
    """Reverse-map a persisted context window onto its launching tier's slots."""
    for tier in (_FLOOR_TIER, _CAPABLE_TIER):
        if context_tokens == tier.context_tokens:
            return tier.parallel_slots
    return None


def _profile_for_slots(slots: int) -> str:
    if sys.platform == "darwin":
        return "apple"
    if slots == _CAPABLE_TIER.parallel_slots:
        return _CAPABLE_TIER.name
    if slots == _FLOOR_TIER.parallel_slots:
        return _FLOOR_TIER.name
    return "advertised"


def _discover_server_capacity() -> ServerCapacity:
    outcome = _core_connect_outcome()
    if outcome["outcome"] == STATE_READY:
        return _capacity_from_ready(outcome)
    return ServerCapacity(_UNKNOWN_SLOTS, "default", _profile_for_slots(_UNKNOWN_SLOTS))


def read_server_capacity() -> ServerCapacity:
    """Return the memoized bundled-server capacity and its evidence source."""
    global _SERVER_CAPACITY_CACHE
    if _SERVER_CAPACITY_CACHE is None:
        _SERVER_CAPACITY_CACHE = _discover_server_capacity()
    return _SERVER_CAPACITY_CACHE


def read_server_parallel_slots() -> int:
    """Return the bundled local server's parallel-slot count.

    Prefers live ``/props`` ``total_slots``; falls back to the tier implied by
    the context window persisted at launch, then to ``_UNKNOWN_SLOTS``.
    Discovered once per process. Never raises; always returns >= 1.
    """
    return read_server_capacity().parallel_slots


def reset_parallel_slots_cache() -> None:
    """Clear the per-process slot-discovery memo."""
    global _SERVER_CAPACITY_CACHE
    _SERVER_CAPACITY_CACHE = None


def _resolve_served_model_id(health_body: dict[str, Any] | None) -> str | None:
    """Served/wire id from the /health body. None signals present-but-invalid."""
    if not isinstance(health_body, dict) or "loaded_model" not in health_body:
        return LOCAL_MODEL
    loaded = health_body["loaded_model"]
    if isinstance(loaded, str) and loaded.strip():
        return loaded
    return None


def _core_connect_outcome(
    *,
    handshake_checker=core_handshake.check_solstone_core_handshake,
    helper_locator=core_handshake.helper_path_for_executable,
    runner=subprocess.run,
) -> dict[str, Any]:
    handshake = handshake_checker()
    if handshake.status != "ok":
        raise RuntimeError(
            "local connect requires a usable solstone-core helper: "
            f"{handshake.message or 'unknown handshake failure'}"
        )
    payload = {
        "schema": "solstone-local-connect-input-v1",
        "journal_path": str(get_journal()),
        "bind_address": _HOST,
        "default_model_id": LOCAL_MODEL,
        "platform": "darwin" if sys.platform == "darwin" else "linux",
    }
    try:
        completed = runner(
            [str(helper_locator()), "local", "connect"],
            input=json.dumps(payload),
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        raise RuntimeError(f"solstone-core local connect failed to launch: {exc}") from exc
    if completed.returncode != 0:
        raise RuntimeError(f"solstone-core local connect failed: {completed.stderr}")
    return json.loads(completed.stdout)


def _capacity_from_ready(outcome: dict[str, Any]) -> ServerCapacity:
    server = outcome["server"]
    return ServerCapacity(
        int(server["parallel_slots"]),
        str(server["capacity_source"]),
        str(server["profile"]),
    )


def is_healthy() -> bool:
    return _core_connect_outcome()["outcome"] == STATE_READY


def probe_state() -> tuple[str, str | None]:
    outcome = _core_connect_outcome()
    state = outcome["outcome"]
    return (STATE_READY, None) if state == STATE_READY else (state, outcome.get("reason"))


def connect() -> LocalServerInfo:
    global _SERVER_CAPACITY_CACHE
    outcome = _core_connect_outcome()
    state = outcome["outcome"]
    if state == STATE_LOADING:
        raise LocalProviderError("local_model_loading", LOCAL_MODEL_NOT_READY_COPY)
    if state != STATE_READY:
        raise LocalProviderError("local_model_not_ready", LOCAL_MODEL_NOT_READY_COPY)
    server = outcome["server"]
    if _SERVER_CAPACITY_CACHE is None:
        _SERVER_CAPACITY_CACHE = _capacity_from_ready(outcome)
    capacity = _SERVER_CAPACITY_CACHE
    return LocalServerInfo(
        model_id=LOCAL_MODEL,
        port=int(server["port"]),
        base_url=str(server["base_url"]),
        state=STATE_READY,
        served_model_id=str(server["served_model_id"]),
        parallel_slots=capacity.parallel_slots,
        capacity_source=capacity.source,
        profile=capacity.profile,
    )


__all__ = [
    "LOCAL_MIN_CONTEXT_TOKENS",
    "LOCAL_MODEL_NOT_READY_COPY",
    "LocalServerInfo",
    "ServerContextProps",
    "ServerCapacity",
    "ServerTier",
    "STATE_IDLE",
    "STATE_STARTING",
    "STATE_LOADING",
    "STATE_READY",
    "STATE_FAILED",
    "STATE_STOPPED",
    "clear_local_context_window",
    "connect",
    "fetch_props",
    "is_healthy",
    "probe_state",
    "read_local_context_window",
    "read_server_context_props",
    "read_server_capacity",
    "read_server_parallel_slots",
    "reset_parallel_slots_cache",
    "select_server_tier",
    "write_local_context_window",
]
