# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared pairing host-address configuration."""

from __future__ import annotations

import ipaddress
import socket
from typing import Any
from urllib.parse import urlsplit

from solstone.think.journal_config import write_journal_config
from solstone.think.service import DEFAULT_SERVICE_PORT
from solstone.think.utils import get_config, read_service_port

HOST_URL_INVALID = "enter an ipv4 address and port, like 192.168.1.44:5015"
HOST_URL_HOSTNAME_UNSUPPORTED = (
    "this needs an ip address — to reach home by name from anywhere, "
    "set up sol private link"
)


class InvalidHostUrl(Exception):
    """Raised when a manual host URL cannot be normalized."""


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


def _input_looks_like_hostname(host: str) -> bool:
    return ":" not in host and any(char.isalpha() for char in host)


def validate_host_url(value: str) -> str:
    """Normalize a manual host URL to ``http://<IPv4>:<port>``."""

    cleaned = value.strip()
    if not cleaned:
        raise InvalidHostUrl(HOST_URL_INVALID)

    candidate = cleaned if "://" in cleaned else f"http://{cleaned}"
    parsed = urlsplit(candidate)
    if parsed.scheme != "http" or not parsed.netloc:
        raise InvalidHostUrl(HOST_URL_INVALID)
    if parsed.username is not None or parsed.password is not None:
        raise InvalidHostUrl(HOST_URL_INVALID)
    if parsed.query or parsed.fragment:
        raise InvalidHostUrl(HOST_URL_INVALID)
    if parsed.path not in ("", "/"):
        raise InvalidHostUrl(HOST_URL_INVALID)

    host = parsed.hostname or ""
    try:
        ipv4 = ipaddress.IPv4Address(host)
    except ValueError as exc:
        if _input_looks_like_hostname(host):
            raise InvalidHostUrl(HOST_URL_HOSTNAME_UNSUPPORTED) from exc
        raise InvalidHostUrl(HOST_URL_INVALID) from exc

    try:
        port = parsed.port
    except ValueError as exc:
        raise InvalidHostUrl(HOST_URL_INVALID) from exc
    if port is None or port < 1 or port > 65535:
        raise InvalidHostUrl(HOST_URL_INVALID)

    return f"http://{ipv4}:{port}"


def get_host_url_override() -> str | None:
    return _clean_str(_pairing_config().get("host_url"))


def override_host_port() -> str | None:
    override = get_host_url_override()
    if override is None:
        return None
    parsed = urlsplit(override)
    return f"{parsed.hostname}:{parsed.port}"


def get_host_url() -> str:
    configured = get_host_url_override()
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


def set_host_url(canonical: str) -> None:
    config = get_config()
    config.setdefault("pairing", {})["host_url"] = canonical
    write_journal_config(config)


def clear_host_url() -> None:
    config = get_config()
    config.setdefault("pairing", {})["host_url"] = None
    write_journal_config(config)


__all__ = [
    "HOST_URL_HOSTNAME_UNSUPPORTED",
    "HOST_URL_INVALID",
    "InvalidHostUrl",
    "_detect_lan_ipv4",
    "clear_host_url",
    "get_host_url",
    "get_host_url_override",
    "override_host_port",
    "set_host_url",
    "validate_host_url",
]
