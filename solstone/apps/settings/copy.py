# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Locked copy for convey settings CLI and restart-aware settings UI flows."""

from __future__ import annotations

CONVEY_REFUSE_NO_PASSWORD_NETWORK = "error: enabling network access requires a password. set one first with: journal password set"
CONVEY_REFUSE_NO_PASSWORD_TRUST = "error: disabling localhost trust requires a password (otherwise no client could authenticate). set one first with: journal password set"
CONVEY_NETWORK_ENABLE_PROGRESS = "enabling network access. restarting convey…"
CONVEY_NETWORK_ENABLE_DONE = (
    "network access enabled. convey is now reachable at: {host_url}"
)
CONVEY_NETWORK_DISABLE_PROGRESS = "restricting to localhost only. restarting convey…"
CONVEY_NETWORK_DISABLE_DONE = (
    "network access disabled. convey is now reachable only at: http://localhost:{port}"
)
CONVEY_RESTART_TIMEOUT = (
    "warning: restart did not complete in 15 seconds. check status with: sol status"
)
CONVEY_TRUST_ENABLE_DONE = (
    "localhost trust enabled. localhost requests skip the password."
)
CONVEY_TRUST_DISABLE_DONE = (
    "localhost trust disabled. localhost requests now require the password."
)
CONVEY_HOST_URL_SET_DONE = "host url set: {url}"
CONVEY_HOST_URL_CLEARED = "host url cleared. auto-detect is active."
CONVEY_HOST_URL_INVALID = "error: host url must be an absolute URL"
CONVEY_HOST_URL_FLAG_CONFLICT = "error: choose exactly one of <url>, --auto, or --show"
FACET_DETAIL_SUCCESS_HEADING = "{title} is ready"
FACET_DETAIL_VALUE_FRAMING = (
    "{title} gathers the people, places, and things that share this context. "
    "as you tag them, they'll show up here and in your journal's filtered views."
)
FACET_DETAIL_PRIMARY_CTA = "tag people, places, and things to {title}"
FACET_DETAIL_SECONDARY_CTA = "create another facet"
FACET_DETAIL_TERTIARY_ESCAPE = "back to settings"


def format_convey_status(
    *,
    network_access: str,
    bind: str,
    password: str,
    trust_localhost: str,
    host_url: str,
) -> str:
    """Return the locked five-line convey status block."""

    return (
        "convey\n"
        f"  network access:    {network_access}\n"
        f"  bind:              {bind}\n"
        f"  password:          {password}\n"
        f"  trust localhost:   {trust_localhost}\n"
        f"  host url:          {host_url}"
    )


__all__ = [
    "CONVEY_HOST_URL_CLEARED",
    "CONVEY_HOST_URL_FLAG_CONFLICT",
    "CONVEY_HOST_URL_INVALID",
    "CONVEY_HOST_URL_SET_DONE",
    "CONVEY_NETWORK_DISABLE_DONE",
    "CONVEY_NETWORK_DISABLE_PROGRESS",
    "CONVEY_NETWORK_ENABLE_DONE",
    "CONVEY_NETWORK_ENABLE_PROGRESS",
    "CONVEY_REFUSE_NO_PASSWORD_NETWORK",
    "CONVEY_REFUSE_NO_PASSWORD_TRUST",
    "CONVEY_RESTART_TIMEOUT",
    "CONVEY_TRUST_DISABLE_DONE",
    "CONVEY_TRUST_ENABLE_DONE",
    "FACET_DETAIL_PRIMARY_CTA",
    "FACET_DETAIL_SECONDARY_CTA",
    "FACET_DETAIL_SUCCESS_HEADING",
    "FACET_DETAIL_TERTIARY_ESCAPE",
    "FACET_DETAIL_VALUE_FRAMING",
    "format_convey_status",
]
