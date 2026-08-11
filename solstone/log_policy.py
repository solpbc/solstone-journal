# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared logging policy for process-wide logger state."""

from __future__ import annotations

import logging


def snapshot_root_logging() -> tuple[int, tuple[logging.Handler, ...]]:
    root = logging.getLogger()
    return root.level, tuple(root.handlers)


def apply_http_logging_policy(
    root_baseline: tuple[int, tuple[logging.Handler, ...]] | None = None,
) -> None:
    """Re-assert solstone's HTTP-logging policy.

    httpx logs the full request URL at INFO; Gemini authenticates via
    `?key=AIzaSy...`, so an httpx INFO record reaching a handler leaks a
    live key into logs. Force httpx to WARNING. When a root baseline
    snapshot is supplied (i.e. a third-party library may
    have reconfigured global logging), reconcile the root logger back to it:
    restore the level and remove only handlers added beyond the snapshot.
    Never blind-reassign root.handlers; that would evict pytest's caplog
    handler and other legitimately-installed handlers.
    """
    logging.getLogger("httpx").setLevel(logging.WARNING)
    if root_baseline is None:
        return

    level, baseline_handlers = root_baseline
    baseline_handler_ids = {id(handler) for handler in baseline_handlers}
    root = logging.getLogger()
    root.setLevel(level)
    for handler in list(root.handlers):
        if id(handler) not in baseline_handler_ids:
            root.removeHandler(handler)
