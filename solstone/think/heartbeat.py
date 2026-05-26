# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""CLI command for periodic self-check agent."""

from __future__ import annotations

import argparse
import logging
import os
import sys
import time
from datetime import datetime
from pathlib import Path

from solstone.think.cortex_client import (
    CortexSpawnUnavailable,
    cortex_request,
    wait_for_uses,
)
from solstone.think.identity import ensure_identity_directory
from solstone.think.utils import get_journal, require_solstone, setup_cli

logger = logging.getLogger(__name__)


RECENCY_WINDOW_HOURS = 12


def _last_success_time(health_dir: Path) -> datetime | None:
    """Return the timestamp of the most recent successful heartbeat run."""
    log_file = health_dir / "heartbeat.log"
    if not log_file.exists():
        return None
    try:
        lines = log_file.read_text().strip().splitlines()
    except OSError:
        return None
    for line in reversed(lines):
        if "outcome=success" in line:
            ts_str = line.split()[0]
            try:
                return datetime.fromisoformat(ts_str)
            except ValueError:
                continue
    return None


def main() -> None:
    """Entry point for ``journal heartbeat``."""
    parser = argparse.ArgumentParser(
        description="Run periodic self-check agent",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="Run full check regardless of recency",
    )
    args = setup_cli(parser)
    require_solstone()

    journal = Path(get_journal())
    ensure_identity_directory()
    health_dir = journal / "health"
    health_dir.mkdir(parents=True, exist_ok=True)

    # Recency check: skip if a recent successful run exists
    if not args.force:
        last_success = _last_success_time(health_dir)
        if last_success is not None:
            hours_since = (datetime.now() - last_success).total_seconds() / 3600
            if hours_since < RECENCY_WINDOW_HOURS:
                logger.info(
                    "Heartbeat succeeded %.1f hours ago (within %d-hour window), skipping",
                    hours_since,
                    RECENCY_WINDOW_HOURS,
                )
                sys.exit(0)

    pid_file = health_dir / "heartbeat.pid"

    try:
        # PID file guard
        if pid_file.exists():
            try:
                existing_pid = int(pid_file.read_text().strip())
                os.kill(existing_pid, 0)
                # Process is alive - exit cleanly
                logger.info("Heartbeat already running (PID %d)", existing_pid)
                sys.exit(0)
            except ProcessLookupError:
                # Dead process - stale PID file, remove and continue
                logger.info("Removing stale PID file (PID %d)", existing_pid)
                pid_file.unlink(missing_ok=True)
            except PermissionError:
                # Process alive but different user
                logger.info(
                    "Heartbeat already running (PID %d, different user)", existing_pid
                )
                sys.exit(0)
            except ValueError:
                # Corrupt PID file
                logger.warning("Corrupt PID file, removing")
                pid_file.unlink(missing_ok=True)

        # Write our PID
        pid_file.write_text(str(os.getpid()))
        start_time = time.monotonic()

        try:
            use_id = cortex_request(
                prompt="Run heartbeat check.",
                name="heartbeat",
            )
        except CortexSpawnUnavailable:
            use_id = None
        if use_id is None:
            logger.error("Failed to send heartbeat request to cortex")
            _log_run(health_dir, start_time, "error")
            sys.exit(1)

        logger.info("Heartbeat agent started (ID: %s)", use_id)

        # Wait for completion
        completed, timed_out = wait_for_uses([use_id], timeout=600)

        # Determine outcome
        if use_id in timed_out:
            logger.error("Heartbeat agent timed out")
            _log_run(health_dir, start_time, "timeout")
            sys.exit(2)

        end_state = completed.get(use_id, "unknown")
        if end_state == "finish":
            logger.info("Heartbeat completed successfully")
            _log_run(health_dir, start_time, "success")
            sys.exit(0)
        else:
            logger.error("Heartbeat agent failed: %s", end_state)
            _log_run(health_dir, start_time, "error")
            sys.exit(1)

    finally:
        pid_file.unlink(missing_ok=True)


def _log_run(health_dir: Path, start_time: float, outcome: str) -> None:
    """Append one line to heartbeat.log."""
    duration = int(time.monotonic() - start_time)
    timestamp = datetime.now().isoformat(timespec="seconds")
    log_file = health_dir / "heartbeat.log"
    with open(log_file, "a") as f:
        f.write(f"{timestamp} duration={duration}s outcome={outcome}\n")
