# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Pairing config readers."""

from __future__ import annotations

import socket
from typing import Any

from solstone.think.service import DEFAULT_SERVICE_PORT
from solstone.think.utils import get_config, read_service_port


def _pairing_config() -> dict[str, Any]:
    config = get_config()
    pairing = config.get("pairing")
    return pairing if isinstance(pairing, dict) else {}


def _clean_str(value: Any) -> str | None:
    if not isinstance(value, str):
        return None
    cleaned = value.strip()
    return cleaned or None


def _detect_lan_ipv4() -> str | None:
    """Return this host's outward-facing IPv4, or None on failure.

    Uses a UDP socket connect to a routable address so the kernel resolves the
    outbound interface without sending any packets.
    """

    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
            sock.connect(("8.8.8.8", 80))
            return sock.getsockname()[0]
    except OSError:
        return None


def get_host_url() -> str:
    configured = _clean_str(_pairing_config().get("host_url"))
    if configured is not None:
        return configured
    convey_config = get_config().get("convey", {})
    if convey_config.get("allow_network_access", False):
        lan_ipv4 = _detect_lan_ipv4()
        if lan_ipv4 is not None:
            convey_port = read_service_port("convey") or DEFAULT_SERVICE_PORT
            return f"http://{lan_ipv4}:{convey_port}"
    convey_port = read_service_port("convey") or DEFAULT_SERVICE_PORT
    return f"http://localhost:{convey_port}"


__all__ = [
    "_detect_lan_ipv4",
    "get_host_url",
]
