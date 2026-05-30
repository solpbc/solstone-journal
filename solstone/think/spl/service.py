# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Posture-gated home-side spl relay daemon.

`journal spl` is the supervised home-side rendezvous service. It watches the
journal link posture and, only while posture is exactly `spl` and a cached
service token exists, opens the outbound listen WebSocket to the spl relay and
pipes tunnel bytes to Convey's secure listener on 127.0.0.1:7657.

The internal Callosum tract remains `link` for dashboard continuity:
  connecting   opening listen WS
  connected    listen WS open (service is reachable)
  disconnect   listen WS closed (about to reconnect)
  tunnel_pair  incoming tunnel (paired device dialed in)
  tunnel_close tunnel closed
"""

from __future__ import annotations

import argparse
import asyncio
import contextlib
import logging
import signal
import sys
from typing import Any

from solstone.think.callosum import CallosumConnection
from solstone.think.link.paths import LinkState, load_service_token, relay_url
from solstone.think.link.window import read_posture
from solstone.think.spl.relay_client import RelayClient
from solstone.think.utils import require_solstone

log = logging.getLogger("spl.service")

_POSTURE_POLL_SECONDS = 5.0


async def run_service() -> None:
    """Watch posture and park the relay client only while spl is enabled."""
    callosum = CallosumConnection()
    callosum.start()

    def emit(event: str, fields: dict[str, Any]) -> None:
        try:
            callosum.emit("link", event, **fields)
        except Exception:
            log.debug("callosum emit failed", exc_info=True)

    stop_event = asyncio.Event()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        with _suppress_not_implemented():
            loop.add_signal_handler(sig, stop_event.set)

    client: RelayClient | None = None
    run_task: asyncio.Task[None] | None = None
    missing_token_logged = False

    try:
        while not stop_event.is_set():
            if run_task is None:
                try:
                    posture = read_posture()
                except Exception:
                    log.warning(
                        "spl posture read failed while idle; treating as direct",
                        exc_info=True,
                    )
                    posture = "direct"

                if posture == "spl":
                    token = load_service_token()
                    if token is None:
                        if not missing_token_logged:
                            log.warning(
                                "spl posture is enabled but no service token is present; "
                                "staying idle"
                            )
                            missing_token_logged = True
                    else:
                        missing_token_logged = False
                        state = LinkState.load_or_create()
                        client = RelayClient(
                            instance_id=state.instance_id,
                            relay_endpoint=relay_url(),
                            service_token=token,
                            callosum_emit=emit,
                        )
                        run_task = asyncio.create_task(
                            client.run(),
                            name="spl-relay-client",
                        )
                else:
                    missing_token_logged = False

                await _wait_for_poll_or_stop(stop_event)
                continue

            if run_task.done():
                run_task.result()
                raise RuntimeError("spl relay client stopped unexpectedly")

            try:
                posture = read_posture()
            except Exception:
                log.warning(
                    "spl posture read failed while parked; keeping relay parked",
                    exc_info=True,
                )
            else:
                if posture != "spl":
                    await _stop_client(client, run_task)
                    client = None
                    run_task = None
                    missing_token_logged = False
                    continue

            await _wait_for_poll_or_stop(stop_event)
    finally:
        log.info("spl service stopping")
        if client is not None and run_task is not None:
            await _stop_client(client, run_task)
        callosum.stop()


async def _wait_for_poll_or_stop(stop_event: asyncio.Event) -> None:
    try:
        await asyncio.wait_for(stop_event.wait(), timeout=_POSTURE_POLL_SECONDS)
    except asyncio.TimeoutError:
        return


async def _stop_client(
    client: RelayClient,
    run_task: asyncio.Task[None],
) -> None:
    await client.stop()
    run_task.cancel()
    with contextlib.suppress(asyncio.CancelledError):
        await run_task


class _suppress_not_implemented:
    """Context manager that swallows NotImplementedError for Windows/TTYs."""

    def __enter__(self) -> None:
        return None

    def __exit__(self, exc_type: Any, _exc: Any, _tb: Any) -> bool:
        return exc_type is NotImplementedError


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="solstone spl tunnel service")
    parser.add_argument(
        "-v", "--verbose", action="store_true", help="Enable verbose output"
    )
    parser.add_argument(
        "-d", "--debug", action="store_true", help="Enable debug logging"
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    """CLI entry point for `journal spl`."""
    parser = _build_parser()
    args = parser.parse_args(list(sys.argv[1:] if argv is None else argv))
    require_solstone()
    logging.basicConfig(
        level=logging.DEBUG if args.verbose or args.debug else logging.INFO,
        format="%(asctime)s %(name)s %(levelname)s %(message)s",
    )

    try:
        asyncio.run(run_service())
    except KeyboardInterrupt:
        log.info("spl service interrupted")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
