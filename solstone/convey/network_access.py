# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared Convey network-access configuration helper."""

from __future__ import annotations

from collections.abc import Callable
from typing import Any

from werkzeug.security import generate_password_hash

from solstone.think.journal_config import write_journal_config
from solstone.think.utils import get_config


class NetworkAccessError(Exception):
    """Base class for Convey network-access errors."""


class NetworkAccessPasswordRequired(NetworkAccessError):
    """Raised when enabling network access without a configured password."""


class NetworkAccessPasswordTooShort(NetworkAccessError):
    """Raised when a supplied password is too short."""


def _convey_password_is_set(config: dict[str, Any]) -> bool:
    password_hash = config.get("convey", {}).get("password_hash", "")
    return bool(str(password_hash or "").strip())


def set_network_access(
    *,
    enable: bool,
    password: str | None = None,
    on_restart: Callable[[], None] | None = None,
) -> dict[str, Any]:
    """Set Convey network access, gate on password, and restart Convey."""

    config = get_config()
    convey = config.setdefault("convey", {})

    if password:
        if len(password) < 8:
            raise NetworkAccessPasswordTooShort
        convey["password_hash"] = generate_password_hash(password)

    if enable and not _convey_password_is_set(config):
        raise NetworkAccessPasswordRequired

    convey["allow_network_access"] = enable
    write_journal_config(config)

    from solstone.convey.restart import wait_for_convey_restart
    from solstone.think.pairing.config import get_host_url

    if on_restart is not None:
        on_restart()
    restart_ok, _ = wait_for_convey_restart(timeout=15.0)
    return {
        "ok": True,
        "restart_timeout": not restart_ok,
        "effective_host_url": get_host_url(),
    }


__all__ = [
    "NetworkAccessError",
    "NetworkAccessPasswordRequired",
    "NetworkAccessPasswordTooShort",
    "set_network_access",
]
