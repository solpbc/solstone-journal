# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Read-only pairing-window predicates for cert-less PL admission."""

from __future__ import annotations

import logging
import time

from solstone.think.link.nonces import NonceStore
from solstone.think.link.paths import nonces_path
from solstone.think.utils import get_config

log = logging.getLogger(__name__)


def read_posture() -> str:
    """Read link.posture from journal config; exact-match, no normalization."""
    cfg = get_config()
    link_cfg = cfg.get("link") if isinstance(cfg, dict) else None
    if isinstance(link_cfg, dict):
        posture = link_cfg.get("posture")
        if isinstance(posture, str) and posture == "spl":
            return "spl"
    return "direct"


def window_open(now: float | None = None) -> bool:
    """Return whether cert-less pairing admission is currently allowed."""
    # Unreadable or corrupt nonce state closes the cert-less pairing window. window_open() returns False on any read error.
    try:
        if read_posture() != "spl":
            return False
        ts = time.time() if now is None else now
        nonces = NonceStore(nonces_path()).snapshot()
        return any(not nonce.used and nonce.expires_at > ts for nonce in nonces)
    except Exception:
        log.warning(
            "cert-less pairing window read failed; treating closed",
            exc_info=True,
        )
        return False
