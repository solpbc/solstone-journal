# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""link relay service runtime.

Registered with solstone's supervisor via `think/sol_cli.py` COMMANDS (see `sol link`);
the supervisor launches this as a subprocess alongside callosum, cortex,
convey, etc. Service lifecycle:

  start → load state + CA → ensure service_token (enroll once) →
    open listen WS to spl-relay → accept tunnel pairs → pipe raw bytes to
    Convey's secure listener on 127.0.0.1:7657. On disconnect, reconnect
    with exponential backoff.

Exits on SIGINT/SIGTERM with a clean close of the listen WS and all
in-flight tunnel WSes.

Callosum events are emitted on the `link` tract:
  enrolled     first-run service-token mint
  connecting   opening listen WS
  connected    listen WS open (service is reachable)
  disconnect   listen WS closed (about to reconnect)
  tunnel_pair  incoming tunnel (paired device dialed in)
  tunnel_close tunnel closed
"""

from __future__ import annotations

import argparse
import asyncio
import logging
import signal
import sys
from typing import Any

from solstone.think.callosum import CallosumConnection
from solstone.think.utils import require_solstone

from .ca import load_or_generate_ca
from .paths import (
    LinkState,
    ca_dir,
    load_service_token,
    relay_url,
    save_service_token,
)
from .relay_client import RelayClient

log = logging.getLogger("link.service")


async def run_service() -> None:
    """Build the relay client and run it until signaled."""
    state = LinkState.load_or_create()
    ca = load_or_generate_ca(ca_dir())
    token = load_service_token()

    callosum = CallosumConnection()
    callosum.start()

    def emit(event: str, fields: dict[str, Any]) -> None:
        try:
            callosum.emit("link", event, **fields)
        except Exception:
            log.debug("callosum emit failed", exc_info=True)

    client = RelayClient(
        instance_id=state.instance_id,
        home_label=state.home_label,
        relay_endpoint=relay_url(),
        service_token=token,
        on_service_token=save_service_token,
        ca_pubkey_spki_pem=ca.pubkey_spki_pem,
        callosum_emit=emit,
    )

    stop_event = asyncio.Event()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        with _suppress_not_implemented():
            loop.add_signal_handler(sig, stop_event.set)

    run_task = asyncio.create_task(client.run(), name="link-relay-client")
    try:
        await stop_event.wait()
    finally:
        log.info("link service stopping")
        await client.stop()
        run_task.cancel()
        try:
            await run_task
        except asyncio.CancelledError:
            pass
        callosum.stop()


class _suppress_not_implemented:
    """Context manager that swallows NotImplementedError for Windows/TTYs."""

    def __enter__(self) -> None:
        return None

    def __exit__(self, exc_type: Any, _exc: Any, _tb: Any) -> bool:
        return exc_type is NotImplementedError


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="solstone link tunnel service")
    parser.add_argument(
        "-v", "--verbose", action="store_true", help="Enable verbose output"
    )
    parser.add_argument(
        "-d", "--debug", action="store_true", help="Enable debug logging"
    )
    subparsers = parser.add_subparsers(
        dest="command", metavar="{serve,join,list}", title="commands"
    )
    subparsers.add_parser("serve", help="start the link tunnel service")

    from . import join_cli, list_cli

    join_parser = subparsers.add_parser(
        "join",
        help="join a solstone with a short code or pair link",
    )
    join_cli.add_arguments(join_parser)
    list_parser = subparsers.add_parser(
        "list",
        help="list caller-side link bundles",
    )
    list_cli.add_arguments(list_parser)
    return parser


def _normalize_serve_args(argv: list[str]) -> list[str]:
    if not argv or argv[0] != "serve":
        return argv
    global_flags = [
        arg for arg in argv[1:] if arg in {"-v", "--verbose", "-d", "--debug"}
    ]
    remaining = [
        arg for arg in argv[1:] if arg not in {"-v", "--verbose", "-d", "--debug"}
    ]
    return global_flags + ["serve"] + remaining


def _run_service_command(args: argparse.Namespace) -> int:
    require_solstone()
    logging.basicConfig(
        level=logging.DEBUG if args.verbose or args.debug else logging.INFO,
        format="%(asctime)s %(name)s %(levelname)s %(message)s",
    )

    try:
        asyncio.run(run_service())
    except KeyboardInterrupt:
        log.info("link service interrupted")
    return 0


def main(argv: list[str] | None = None) -> int:
    """CLI entry point for `sol link`."""
    parser = _build_parser()
    parsed_argv = _normalize_serve_args(list(sys.argv[1:] if argv is None else argv))
    args = parser.parse_args(parsed_argv)
    if args.command in (None, "serve"):
        return _run_service_command(args)
    if args.command == "join":
        from . import join_cli

        return join_cli.main(args)
    if args.command == "list":
        from . import list_cli

        return list_cli.main(args)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
