# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Owner-facing copy constants for the entities app."""

from __future__ import annotations

from typing import Any

ENT_DETACH_CONFIRM = "Detach {name} from {facet}? It stays in your journal and you can re-attach it anytime."
ENT_DETACH_DONE = "Detached from {facet}. {name} is still in your journal."
ENT_DETACH_REATTACH_ACTION = "Re-attach it →"
ENT_DETACH_FIND_ACTION = "Find it in your journal →"
ENT_OBS_SOURCE_LINK_TITLE = "See this day"


def entities_copy_payload() -> dict[str, Any]:
    """Return copy constants for templates and browser code."""
    return {
        name: value
        for name, value in globals().items()
        if name.startswith("ENT_") and name.isupper()
    }


def entities_copy_values() -> list[str]:
    """Return all verbatim copy values, flattening list constants."""
    values: list[str] = []
    for value in entities_copy_payload().values():
        if isinstance(value, str):
            values.append(value)
        elif isinstance(value, list):
            values.extend(item for item in value if isinstance(item, str))
    return values
