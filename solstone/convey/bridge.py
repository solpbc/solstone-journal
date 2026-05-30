# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Bridge between the Callosum message bus and SSE subscribers.

Receives Callosum events and broadcasts them to connected SSE clients.
Also provides emit() for route handlers to send events via the shared connection.
"""

from __future__ import annotations

import json
import logging
import queue
import threading
import time
from dataclasses import dataclass, field
from typing import Any, Dict, Optional

from solstone.think.callosum import CallosumConnection

logger = logging.getLogger(__name__)

_WATCH_LOCK = threading.Lock()
_CALLOSUM_CONNECTION: Optional[CallosumConnection] = None
_SSE_QUEUE_MAXSIZE = 256
_SSE_HEARTBEAT_SECONDS = 20
_SSE_LOCK = threading.Lock()
_STATE_CACHE: Dict[str, Any] = {
    "supervisor_status": None,
    "last_observe_ts": None,
    "link_connection": None,
}


@dataclass(eq=False)
class _SseSubscriber:
    key_prefix: str
    queue: queue.Queue[str] = field(
        default_factory=lambda: queue.Queue(maxsize=_SSE_QUEUE_MAXSIZE)
    )
    dropped: threading.Event = field(default_factory=threading.Event)
    drop_reason: str | None = None


_SSE_SUBSCRIBERS_BY_KEY: dict[str, set[_SseSubscriber]] = {}
_SSE_LAST_CHAT_REQUEST_AT_BY_KEY: dict[str, int] = {}


def register_sse_subscriber(key_prefix: str) -> _SseSubscriber:
    """Register an SSE subscriber for a key prefix."""
    subscriber = _SseSubscriber(key_prefix=key_prefix)
    with _SSE_LOCK:
        _SSE_SUBSCRIBERS_BY_KEY.setdefault(key_prefix, set()).add(subscriber)
    return subscriber


def unregister_sse_subscriber(handle: _SseSubscriber) -> None:
    """Unregister an SSE subscriber handle. Safe to call more than once."""
    with _SSE_LOCK:
        subscribers = _SSE_SUBSCRIBERS_BY_KEY.get(handle.key_prefix)
        if not subscribers:
            return
        subscribers.discard(handle)
        if not subscribers:
            _SSE_SUBSCRIBERS_BY_KEY.pop(handle.key_prefix, None)


def subscription_count(key_prefix: str) -> int:
    """Return the active SSE subscription count for a key prefix."""
    with _SSE_LOCK:
        return len(_SSE_SUBSCRIBERS_BY_KEY.get(key_prefix, set()))


def last_chat_request_at(key_prefix: str) -> int | None:
    """Return the last delivered sol-initiated chat request timestamp."""
    with _SSE_LOCK:
        return _SSE_LAST_CHAT_REQUEST_AT_BY_KEY.get(key_prefix)


def _message_ts_ms(message: dict) -> int:
    try:
        return int(message.get("ts") or int(time.time() * 1000))
    except (TypeError, ValueError):
        return int(time.time() * 1000)


def _broadcast_to_sse_clients(message: dict) -> None:
    """Broadcast a serialized Callosum event to all SSE subscribers."""
    from solstone.convey.sol_initiated.copy import KIND_SOL_CHAT_REQUEST

    with _SSE_LOCK:
        subscribers = [
            subscriber
            for subscribers_for_key in _SSE_SUBSCRIBERS_BY_KEY.values()
            for subscriber in subscribers_for_key
        ]
    if not subscribers:
        return

    serialized = json.dumps(message)
    is_chat_request = (
        message.get("tract") == "chat" and message.get("event") == KIND_SOL_CHAT_REQUEST
    )
    for subscriber in subscribers:
        if subscriber.dropped.is_set():
            continue
        try:
            subscriber.queue.put_nowait(serialized)
            if is_chat_request:
                with _SSE_LOCK:
                    _SSE_LAST_CHAT_REQUEST_AT_BY_KEY[subscriber.key_prefix] = (
                        _message_ts_ms(message)
                    )
        except queue.Full:
            subscriber.drop_reason = "overflow"
            subscriber.dropped.set()
            unregister_sse_subscriber(subscriber)
            logger.info(
                "Dropping slow Callosum SSE subscriber key_prefix=%s",
                subscriber.key_prefix,
            )


def _broadcast_callosum_event(message: Dict[str, Any]) -> None:
    """Broadcast Callosum event to SSE clients and server-side handlers."""
    # Update state cache
    tract = message.get("tract")
    event = message.get("event")
    if tract == "supervisor" and event == "status":
        _STATE_CACHE["supervisor_status"] = message
    if tract == "observe" and event in ("observed", "status"):
        _STATE_CACHE["last_observe_ts"] = time.time()
    if tract == "link" and event in ("connecting", "connected", "disconnect"):
        _STATE_CACHE["link_connection"] = event

    # Broadcast to SSE clients
    try:
        _broadcast_to_sse_clients(message)
    except Exception:  # pragma: no cover - defensive against SSE errors
        logger.exception(
            "Failed to broadcast %s event to SSE clients", message.get("tract")
        )

    # Dispatch to server-side app event handlers
    try:
        from solstone.apps.events import dispatch

        dispatch(message)
    except Exception:  # pragma: no cover - defensive against handler errors
        logger.exception(
            "Failed to dispatch %s event to handlers", message.get("tract")
        )


def start_bridge() -> None:
    """Start listening for Callosum events and forwarding to SSE clients."""
    global _CALLOSUM_CONNECTION
    with _WATCH_LOCK:
        if _CALLOSUM_CONNECTION:
            return

        # Create Callosum connection with callback
        try:
            _CALLOSUM_CONNECTION = CallosumConnection()
            _CALLOSUM_CONNECTION.start(callback=_broadcast_callosum_event)
            logger.info("Callosum bridge connected, forwarding all events to SSE")
        except Exception as e:
            logger.warning(f"Failed to start Callosum bridge: {e}")
            _CALLOSUM_CONNECTION = None


def stop_bridge() -> None:
    """Stop the Callosum bridge."""
    global _CALLOSUM_CONNECTION
    with _WATCH_LOCK:
        if _CALLOSUM_CONNECTION:
            _CALLOSUM_CONNECTION.stop()
            _CALLOSUM_CONNECTION = None
            logger.info("Callosum bridge stopped")


def emit(tract: str, event: str, **fields) -> bool:
    """Emit event via shared Callosum connection.

    Non-blocking: queues message for background thread to send.
    If disconnected, message is dropped (with debug logging).

    Args:
        tract: Event category/namespace
        event: Event type
        **fields: Additional event fields

    Returns:
        True if queued successfully, False if bridge not started or queue full
    """
    if _CALLOSUM_CONNECTION:
        return _CALLOSUM_CONNECTION.emit(tract, event, **fields)
    return False


def get_cached_state() -> Dict[str, Any]:
    """Return a copy of the bridge state cache."""
    return dict(_STATE_CACHE)
