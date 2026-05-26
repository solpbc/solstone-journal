# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""CLI for service health status and logs.

Usage:
    sol health                  Show current service health status
    sol health logs             View service health logs
"""

from __future__ import annotations

import argparse
import sys
import threading
from datetime import timedelta
from pathlib import Path
from typing import Any

from solstone.think.callosum import CallosumConnection
from solstone.think.utils import get_journal, setup_cli

STATUS_TIMEOUT = 10


def format_uptime(seconds: int) -> str:
    """Format uptime in human-readable format."""
    if seconds < 60:
        return f"{seconds}s"

    delta = timedelta(seconds=seconds)
    parts = []
    if delta.days:
        parts.append(f"{delta.days}d")

    hours = delta.seconds // 3600
    if hours:
        parts.append(f"{hours}h")

    mins = (delta.seconds % 3600) // 60
    if mins:
        parts.append(f"{mins}m")

    return " ".join(parts)


def print_status(status: dict[str, Any]) -> None:
    """Print supervisor status in a human-readable format."""
    print("Services:")
    for service in status.get("services", []):
        name = service.get("name", "?")
        pid = service.get("pid", "?")
        uptime_seconds = int(service.get("uptime_seconds", 0) or 0)
        print(f"  {name:16} pid {pid}  uptime {format_uptime(uptime_seconds)}")

    crashed = status.get("crashed") or []
    if crashed:
        print()
        print("Crashed:")
        for service in crashed:
            name = service.get("name", "?")
            attempts = service.get("restart_attempts", 0)
            print(f"  {name:16} {attempts} restart attempts")

    print()
    tasks = status.get("tasks") or []
    queues = status.get("queues") or {}
    non_zero_queues = [(name, count) for name, count in sorted(queues.items()) if count]

    if tasks:
        print("Tasks:")
        for task in tasks:
            name = task.get("name", "?")
            duration = task.get("duration_seconds", 0)
            line = f"  {name:16} {duration}s"
            if task.get("stuck"):
                line += f"  STUCK (cap {task['max_runtime_seconds']}s)"
            print(line)
        for name, count in non_zero_queues:
            print(f"  queued {name:9} {count}")
    elif non_zero_queues:
        print("Tasks:")
        for name, count in non_zero_queues:
            print(f"  queued {name:9} {count}")
    else:
        print("Tasks: none")

    stale = status.get("stale_heartbeats") or []
    if stale:
        print()
        print(f"Heartbeat: STALE ({', '.join(stale)})")
    else:
        print("Heartbeat: ok")
    callosum_clients = status.get("callosum_clients", 0)
    print(f"Callosum: {callosum_clients} clients")


def health_check() -> int:
    """Request and print one-shot supervisor status."""
    sock_path = Path(get_journal()) / "health" / "callosum.sock"
    if not sock_path.exists():
        print(
            f"Cannot connect: callosum socket not found at {sock_path}",
            file=sys.stderr,
        )
        return 1

    status_event = threading.Event()
    status_holder: dict[str, dict[str, Any]] = {}

    def callback(msg: dict[str, Any]) -> None:
        if msg.get("tract") == "supervisor" and msg.get("event") == "status":
            status_holder["data"] = msg
            status_event.set()

    conn = CallosumConnection(socket_path=sock_path)
    conn.start(callback=callback)
    try:
        got_status = status_event.wait(timeout=STATUS_TIMEOUT)
    finally:
        conn.stop()

    if not got_status:
        print(
            f"Timed out waiting for supervisor status ({STATUS_TIMEOUT:g}s)",
            file=sys.stderr,
        )
        return 1

    print_status(status_holder["data"])
    return 0


def main() -> None:
    """Entry point for ``sol health``."""
    args = sys.argv[1:]
    if args and args[0] == "logs":
        sys.argv = ["sol health logs"] + args[1:]
        from solstone.think.logs_cli import main as logs_main

        logs_main()
        return

    parser = argparse.ArgumentParser(
        prog="sol health",
        description=(
            "Show service health status.\n\n"
            "Subcommands:\n"
            "  logs    View service health logs (sol health logs -h for details)"
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    setup_cli(parser)
    sys.exit(health_check())


if __name__ == "__main__":
    main()
