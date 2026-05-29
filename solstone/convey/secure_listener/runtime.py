# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Runtime lifecycle for the secure PL listener."""

from __future__ import annotations

import asyncio
import atexit
import logging
import socket
import threading
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from typing import Any

from solstone.think.link.auth import AuthorizedClients
from solstone.think.link.ca import load_or_generate_ca
from solstone.think.link.paths import LinkState, authorized_clients_path, ca_dir

from .accept import SecureListener
from .tls import build_relaxed_server_context, build_server_context, issue_server_cert

logger = logging.getLogger("convey.secure_listener.runtime")


@dataclass
class RuntimeState:
    loop: asyncio.AbstractEventLoop | None = None
    thread: threading.Thread | None = None
    started_event: threading.Event = field(default_factory=threading.Event)
    apps: list[Any] = field(default_factory=list)
    authorized: AuthorizedClients | None = None
    executor: ThreadPoolExecutor = field(
        default_factory=lambda: ThreadPoolExecutor(
            max_workers=16,
            thread_name_prefix="secure-listener-wsgi",
        )
    )
    listener: SecureListener | None = None
    start_error: BaseException | None = None
    sockets: tuple[socket.socket, ...] = field(default_factory=tuple)


_RUNTIME_LOCK = threading.Lock()
_runtime: RuntimeState | None = None
_atexit_registered = False


def get_authorized_clients() -> AuthorizedClients:
    runtime = _runtime
    if runtime is None or runtime.authorized is None:
        raise RuntimeError("secure_listener not started")
    return runtime.authorized


def _thread_main(runtime: RuntimeState) -> None:
    loop = asyncio.new_event_loop()
    asyncio.set_event_loop(loop)
    runtime.loop = loop
    try:
        app = runtime.apps[0]
        authorized = AuthorizedClients(authorized_clients_path())
        runtime.authorized = authorized
        state = LinkState.load_or_create()
        ca = load_or_generate_ca(ca_dir())
        server_cert, server_key_pem = issue_server_cert(
            ca,
            common_name=f"solstone link ({state.home_label})",
        )
        strict_tls_ctx = build_server_context(
            ca=ca,
            server_cert=server_cert,
            server_key=server_key_pem,
            authorized=authorized,
        )
        relaxed_tls_ctx = build_relaxed_server_context(
            ca=ca,
            server_cert=server_cert,
            server_key=server_key_pem,
            authorized=authorized,
        )

        def emit(event: str, fields: dict[str, Any]) -> None:
            try:
                from solstone.convey import bridge as convey_bridge

                convey_bridge.emit("link", event, **fields)
            except Exception:
                logger.debug("secure listener callosum emit failed", exc_info=True)

        listener = SecureListener(
            app=app,
            strict_tls_ctx=strict_tls_ctx,
            relaxed_tls_ctx=relaxed_tls_ctx,
            authorized=authorized,
            executor=runtime.executor,
            callosum_emit=emit,
        )
        runtime.listener = listener
        loop.run_until_complete(listener.start())
        runtime.sockets = listener.sockets
        runtime.started_event.set()
        loop.run_forever()
    except BaseException as exc:
        runtime.start_error = exc
        runtime.started_event.set()
        logger.exception("secure listener failed to start")
    finally:
        try:
            if runtime.listener is not None:
                loop.run_until_complete(runtime.listener.stop())
            pending = [task for task in asyncio.all_tasks(loop) if not task.done()]
            for task in pending:
                task.cancel()
            if pending:
                loop.run_until_complete(
                    asyncio.gather(*pending, return_exceptions=True)
                )
        finally:
            runtime.executor.shutdown(wait=True, cancel_futures=True)
            loop.close()


def start_secure_listener(app: Any) -> None:
    """Start the secure PL listener for this Convey app if enabled."""
    global _runtime, _atexit_registered

    if not app.config.get("SECURE_LISTENER_ENABLED", False):
        return

    with _RUNTIME_LOCK:
        if _runtime is None:
            runtime = RuntimeState(apps=[app])
            thread = threading.Thread(
                target=_thread_main,
                args=(runtime,),
                name="secure-listener-runtime",
                daemon=True,
            )
            runtime.thread = thread
            _runtime = runtime
            thread.start()
        runtime = _runtime
        if app not in runtime.apps:
            runtime.apps.append(app)
        app.secure_listener_started = True
        if not _atexit_registered:
            atexit.register(stop_all_secure_listener)
            _atexit_registered = True
        started_event = runtime.started_event
    started_event.wait(timeout=2.0)
    if runtime.start_error is not None:
        raise runtime.start_error


def stop_secure_listener(app: Any) -> None:
    runtime = _runtime
    app.secure_listener_started = False
    if runtime is None:
        return
    with _RUNTIME_LOCK:
        if app in runtime.apps:
            runtime.apps.remove(app)
        remaining = list(runtime.apps)
    if not remaining:
        stop_all_secure_listener()


def stop_all_secure_listener() -> None:
    global _runtime

    with _RUNTIME_LOCK:
        runtime = _runtime
        _runtime = None
    if runtime is None:
        return
    for app in list(runtime.apps):
        try:
            app.secure_listener_started = False
        except Exception:
            logger.exception("secure listener app cleanup failed")

    closed = 0
    for sock in runtime.sockets:
        try:
            sock.close()
            closed += 1
        except OSError:
            pass
    if closed:
        logger.info("secure_listener: closed %d listening socket(s)", closed)

    if runtime.loop is not None:
        try:
            runtime.loop.call_soon_threadsafe(runtime.loop.stop)
        except (RuntimeError, OSError) as exc:
            logger.debug("secure_listener: loop stop best-effort skipped: %s", exc)

    if runtime.thread is not None:
        try:
            runtime.thread.join(timeout=5.0)
        except RuntimeError as exc:
            logger.debug("secure_listener: thread join best-effort skipped: %s", exc)


__all__ = [
    "RuntimeState",
    "get_authorized_clients",
    "start_secure_listener",
    "stop_all_secure_listener",
    "stop_secure_listener",
]
