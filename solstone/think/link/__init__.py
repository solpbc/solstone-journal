# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Caller-side link commands and shared pairing/runtime helpers.

`sol link` dispatches caller-side commands such as `join` and `list`. The
supervised home-side spl rendezvous daemon lives in `solstone.think.spl` and is
launched as `journal spl`.
"""

__version__ = "0.1.0"

from .cli import main  # noqa: E402 — re-exported so `sol link` can import it

__all__ = ["main"]
