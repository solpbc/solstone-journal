# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared pairing home-address configuration."""

from __future__ import annotations

import ipaddress
from typing import Any

from solstone.think.journal_config import JournalConfigMutation, mutate_journal_config
from solstone.think.link import interface_watcher
from solstone.think.utils import get_config

HOME_ADDRESS_INVALID = "enter an ipv4 address and port, like 192.168.1.44:7657"
HOME_ADDRESS_HOSTNAME_UNSUPPORTED = (
    "this needs an ip address — to reach home by name from anywhere, "
    "turn on your private network"
)


class InvalidHomeAddress(Exception):
    """Raised when a manual home address cannot be normalized."""


def _pairing_config() -> dict[str, Any]:
    config = get_config()
    pairing = config.get("pairing")
    return pairing if isinstance(pairing, dict) else {}


def _clean_str(value: Any) -> str | None:
    if not isinstance(value, str):
        return None
    cleaned = value.strip()
    return cleaned or None


def _input_looks_like_hostname(host: str) -> bool:
    return any(char.isalpha() for char in host) and all(
        char.isalnum() or char in ".-" for char in host
    )


def is_usable_ipv4(value: Any) -> bool:
    """Return whether value is a non-special IPv4 address usable for pairing."""

    try:
        ipv4 = ipaddress.IPv4Address(value)
    except (TypeError, ValueError):
        return False
    return not (
        ipv4.is_loopback
        or ipv4.is_unspecified
        or ipv4.is_link_local
        or ipv4.is_multicast
    )


def validate_home_address(value: str) -> str:
    """Normalize a manual home address to ``<IPv4>:<secure-listener-port>``."""

    cleaned = value.strip()
    if not cleaned or "://" in cleaned or "/" in cleaned:
        raise InvalidHomeAddress(HOME_ADDRESS_INVALID)

    host, sep, port_text = cleaned.rpartition(":")
    if sep != ":" or not host or not port_text:
        if _input_looks_like_hostname(cleaned):
            raise InvalidHomeAddress(HOME_ADDRESS_HOSTNAME_UNSUPPORTED)
        raise InvalidHomeAddress(HOME_ADDRESS_INVALID)

    try:
        ipv4 = ipaddress.IPv4Address(host)
    except ValueError as exc:
        if _input_looks_like_hostname(host):
            raise InvalidHomeAddress(HOME_ADDRESS_HOSTNAME_UNSUPPORTED) from exc
        raise InvalidHomeAddress(HOME_ADDRESS_INVALID) from exc

    try:
        port = int(port_text)
    except ValueError as exc:
        raise InvalidHomeAddress(HOME_ADDRESS_INVALID) from exc
    if port != interface_watcher.LINK_DIRECT_PORT or not is_usable_ipv4(str(ipv4)):
        raise InvalidHomeAddress(HOME_ADDRESS_INVALID)

    return f"{ipv4}:{port}"


def get_home_address() -> str | None:
    return _clean_str(_pairing_config().get("home_address"))


def set_home_address(canonical: str) -> None:
    def apply(config: dict[str, Any]) -> JournalConfigMutation[None]:
        pairing = config.setdefault("pairing", {})
        changed = pairing.get("home_address") != canonical
        pairing["home_address"] = canonical
        return JournalConfigMutation(changed=changed, value=None)

    mutate_journal_config(apply)


def clear_home_address() -> None:
    def apply(config: dict[str, Any]) -> JournalConfigMutation[None]:
        pairing = config.setdefault("pairing", {})
        changed = pairing.get("home_address") is not None
        pairing["home_address"] = None
        return JournalConfigMutation(changed=changed, value=None)

    mutate_journal_config(apply)


__all__ = [
    "HOME_ADDRESS_HOSTNAME_UNSUPPORTED",
    "HOME_ADDRESS_INVALID",
    "InvalidHomeAddress",
    "clear_home_address",
    "get_home_address",
    "is_usable_ipv4",
    "set_home_address",
    "validate_home_address",
]
