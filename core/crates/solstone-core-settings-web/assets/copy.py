# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Locked copy for convey settings CLI and restart-aware settings UI flows."""

from __future__ import annotations

LOCAL_ENDPOINT_DISCLOSURE = "inference runs on the endpoint you configured; your requests, including screen content for vision, are sent there."
FACET_DETAIL_SUCCESS_HEADING = "{title} is ready"
FACET_DETAIL_VALUE_FRAMING = (
    "{title} gathers the people, places, and things that share this context. "
    "as you tag them, they'll show up here and in your journal's filtered views."
)
FACET_DETAIL_PRIMARY_CTA = "tag people, places, and things to {title}"
FACET_DETAIL_SECONDARY_CTA = "create another facet"
FACET_DETAIL_TERTIARY_ESCAPE = "back to settings"

__all__ = [
    "FACET_DETAIL_PRIMARY_CTA",
    "FACET_DETAIL_SECONDARY_CTA",
    "FACET_DETAIL_SUCCESS_HEADING",
    "FACET_DETAIL_TERTIARY_ESCAPE",
    "FACET_DETAIL_VALUE_FRAMING",
    "LOCAL_ENDPOINT_DISCLOSURE",
]
