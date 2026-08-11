# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""CLI entry points for Convey web interface."""

from __future__ import annotations

import argparse
import logging
import os

from flask import Flask

from solstone.apps.events import discover_handlers, start_dispatcher, stop_dispatcher
from solstone.convey.secure_listener import start_secure_listener, stop_secure_listener

from .bridge import start_bridge, stop_bridge

logger = logging.getLogger(__name__)


def _resolve_bind_host() -> str:
    """Return Convey's bind host — always loopback; :5015 is never network-exposed."""
    return "127.0.0.1"


def run_service(
    app: Flask,
    *,
    host: str = "127.0.0.1",
    port: int,
    debug: bool = False,
    start_watcher: bool = True,
) -> None:
    """Run the Convey service, optionally starting the Cortex watcher."""

    if start_watcher:
        # In debug mode with reloader, only start in child process
        # In non-debug mode, always start (no reloader)
        # WERKZEUG_RUN_MAIN is set to 'true' only in the child/main process
        should_start = not debug or os.environ.get("WERKZEUG_RUN_MAIN") == "true"
        if should_start:
            # Discover and start event handlers before bridge
            discover_handlers()
            start_dispatcher()
            logger.info("Starting Callosum bridge")
            start_bridge()
        else:
            logger.debug("Skipping bridge start in reloader parent process")

    try:
        app.run(host=host, port=port, debug=debug)
    finally:
        stop_secure_listener(app)
        stop_bridge()
        stop_dispatcher()


def main() -> None:
    """Main CLI entry point for convey command."""
    from solstone.think import core_handshake
    from solstone.think.utils import setup_cli

    parser = argparse.ArgumentParser(description="Convey web interface")
    parser.add_argument(
        "--port",
        type=int,
        required=True,
        help="Port to serve on",
    )
    args = setup_cli(parser)

    core_binary = str(core_handshake.helper_path_for_executable())
    os.execv(core_binary, [core_binary, "convey", "--port", str(args.port)])
