# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Lucide icon helpers for Convey rendering."""

from __future__ import annotations

import functools
import json
from pathlib import Path

APP_LUCIDE_MAP: dict[str, str] = {
    "home": "house",
    "sol": "bot",
    "chat": "message-circle",
    "transcripts": "scroll-text",
    "observer": "antenna",
    "search": "search",
    "import": "import",
    "curation": "wand-sparkles",
    "backup": "history",
    "body": "heart-pulse",
    "entities": "contact",
    "health": "stethoscope",
    "network": "network",
    "news": "newspaper",
    "settings": "settings",
    "speakers": "mic-vocal",
    "stats": "chart-column",
    "support": "life-buoy",
    "thinking": "brain",
    "timeline": "calendar-range",
    "tokens": "coins",
}


@functools.lru_cache(maxsize=1)
def _lucide_icons() -> dict[str, str]:
    path = Path(__file__).parent / "static" / "icons" / "lucide.json"
    with path.open(encoding="utf-8") as handle:
        return json.load(handle)


@functools.lru_cache(maxsize=1)
def _lucide_tags() -> dict[str, list[str]]:
    path = Path(__file__).parent / "static" / "icons" / "lucide-tags.json"
    with path.open(encoding="utf-8") as handle:
        return json.load(handle)


@functools.lru_cache(maxsize=1)
def _emoji_lucide_map() -> dict[str, str]:
    path = Path(__file__).parent / "static" / "icons" / "emoji-lucide.json"
    with path.open(encoding="utf-8") as handle:
        return json.load(handle)


def lucide_svg(name: str) -> str | None:
    """Return raw SVG markup for a Lucide icon name."""
    return _lucide_icons().get(name)


def is_lucide_icon(name: str) -> bool:
    """Return whether name is a vendored Lucide icon."""
    return bool(name) and lucide_svg(name) is not None


def emoji_to_lucide(emoji: str, default: str | None = None) -> str | None:
    """Translate an emoji to a Lucide icon name."""
    if not isinstance(emoji, str):
        raise TypeError("emoji must be a string")
    if not emoji:
        return default

    emoji_map = _emoji_lucide_map()

    raw_hit = emoji_map.get(emoji)
    if raw_hit is not None:
        return raw_hit

    stripped = "".join(
        ch for ch in emoji if ch != "\ufe0f" and not (0x1F3FB <= ord(ch) <= 0x1F3FF)
    )
    stripped_hit = emoji_map.get(stripped)
    if stripped_hit is not None:
        return stripped_hit

    if "\u200d" in stripped:
        leading_base = stripped.split("\u200d", 1)[0]
        base_hit = emoji_map.get(leading_base)
        if base_hit is not None:
            return base_hit

    return default


def lucide_svg_for_emoji(emoji: str) -> str | None:
    """Return raw SVG markup for an emoji's mapped Lucide icon."""
    icon_name = emoji_to_lucide(emoji)
    if icon_name is None:
        return None
    return lucide_svg(icon_name)


def resolve_icon_svg(icon: str | None, emoji: str) -> str | None:
    """Resolve a Lucide icon override, falling back to the emoji mapping."""
    if icon:
        svg = lucide_svg(icon)
        if svg is not None:
            return svg
    return lucide_svg_for_emoji(emoji)


def _icon_search_rank(name: str, q: str, tags: dict[str, list[str]]) -> int:
    """Relevance rank for a search hit; lower is better.

    Tiers put exact and whole-token name matches ahead of mere substring
    matches, so searching "lock" surfaces ``lock``/``lock-open`` before
    ``alarm-clock`` (which only contains "lock" inside "clock"). Name matches
    (0-4) always outrank tag-only matches (5-6); 99 = no match.
    """
    tokens = name.split("-")
    if name == q:
        return 0
    if tokens[0] == q:  # lock-open, heart-pulse, shield-alert
        return 1
    if q in tokens:  # book-lock, calendar-heart
        return 2
    if name.startswith(q):  # prefix that isn't a whole token
        return 3
    if q in name:  # substring anywhere (alarm-clock for "lock")
        return 4
    icon_tags = tags.get(name, [])
    if any(q == tag or q in tag.split() for tag in icon_tags):  # tag word match
        return 5
    if any(q in tag for tag in icon_tags):  # tag substring
        return 6
    return 99


def search_lucide_icons(query: str, limit: int = 80) -> list[dict[str, str]]:
    """Search vendored Lucide icons by relevance.

    Order: exact name > whole-token name > prefix > substring, with name
    matches ahead of tag-only matches. Within a tier, shorter names first.
    An empty query returns the alphabetical head of the set.
    """
    q = (query or "").strip().lower()
    names = sorted(_lucide_icons())
    if q:
        tags = _lucide_tags()
        ranked = sorted(
            ((_icon_search_rank(name, q, tags), len(name), name) for name in names),
            key=lambda r: (r[0], r[1], r[2]),
        )
        chosen = [name for rank, _, name in ranked if rank < 99][:limit]
    else:
        chosen = names[:limit]

    results: list[dict[str, str]] = []
    for name in chosen:
        svg = lucide_svg(name)
        if svg is not None:
            results.append({"name": name, "svg": svg})
    return results
