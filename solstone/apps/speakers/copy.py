# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Owner-facing copy constants for the speakers app."""

from __future__ import annotations

from typing import Any

SPK_OVERVIEW_YOUR_VOICE_HEADER = "your voice"
SPK_OVERVIEW_YOUR_VOICE_LEARNING = "solstone is still learning"
SPK_OVERVIEW_OWNER_PROGRESS_SUFFIX = "longer statements"
SPK_OVERVIEW_OWNER_HELP_LABEL = "help solstone learn faster"
SPK_OVERVIEW_OWNER_BUILD_FROM_TAGS_LABEL = "Build from manual tags"
SPK_OVERVIEW_YOUR_VOICE_CONFIRMED = "confirmed"
SPK_OVERVIEW_OWNER_SAMPLES_LABEL = "voice samples"
SPK_OVERVIEW_OWNER_STREAMS_LABEL = "where heard"
SPK_OVERVIEW_OWNER_COHESION_LABEL = "consistency"
SPK_OVERVIEW_OWNER_REFRESHED_PREFIX = "last refreshed"
SPK_OVERVIEW_KNOWN_VOICES_HEADER = "known voices"
SPK_OVERVIEW_KNOWN_VOICES_SORTS = ["recent", "most samples", "alphabetical"]
SPK_OVERVIEW_CARD_SAMPLES_LABEL = "samples"
SPK_OVERVIEW_CARD_SEGMENTS_LABEL = "segments"
SPK_OVERVIEW_CARD_LAST_HEARD_PREFIX = "last heard"
SPK_OVERVIEW_CARD_STREAMS_PREFIX = "streams"
SPK_OVERVIEW_COHESION_LABELS = [
    "learning",
    "early",
    "improving",
    "good",
    "strong",
    "settled",
]
SPK_OVERVIEW_KNOWN_VOICES_EMPTY = "no one else's voice has been learned yet. once solstone hears a recurring voice across a few segments, that person will appear here. you can also tag voices manually in any segment."
SPK_OVERVIEW_NEW_VOICES_HEADER = "new voices"
SPK_OVERVIEW_TODAY_LINK_LABEL = "today's review →"
SPK_FILTER_BY_PREFIX = "filtering by:"
SPK_FILTER_NO_SEGMENTS_TODAY = "no segments attributed to this speaker today"


def speaker_copy_payload() -> dict[str, Any]:
    """Return copy constants for templates and browser code."""
    return {
        name: value
        for name, value in globals().items()
        if name.startswith("SPK_") and name.isupper()
    }


def speaker_copy_values() -> list[str]:
    """Return all verbatim copy values, flattening list constants."""
    values: list[str] = []
    for value in speaker_copy_payload().values():
        if isinstance(value, str):
            values.append(value)
        elif isinstance(value, list):
            values.extend(item for item in value if isinstance(item, str))
    return values
