# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

from solstone.apps import AppRegistry
from solstone.convey.icons import (
    APP_LUCIDE_MAP,
    _lucide_tags,
    emoji_to_lucide,
    is_lucide_icon,
    lucide_svg,
    lucide_svg_for_emoji,
    resolve_icon_svg,
    search_lucide_icons,
)

REPO_ROOT = Path(__file__).resolve().parents[1]


def test_lucide_svg_hit_and_miss() -> None:
    assert "<svg" in (lucide_svg("house") or "")
    assert lucide_svg("not-a-lucide-icon") is None


def test_is_lucide_icon() -> None:
    assert is_lucide_icon("house") is True
    assert is_lucide_icon("not-a-lucide-icon") is False
    assert is_lucide_icon("") is False


def test_emoji_to_lucide_required_cases() -> None:
    assert emoji_to_lucide("📚") == "library"
    assert emoji_to_lucide("🤝") == "handshake"
    assert emoji_to_lucide("⚙️") == "settings"
    assert emoji_to_lucide("⚙") == "settings"
    assert emoji_to_lucide("⚙️") == emoji_to_lucide("⚙")
    assert emoji_to_lucide("🪮", default="fallback") == "fallback"
    assert emoji_to_lucide("") is None


def test_emoji_to_lucide_preserves_raw_zwj_key() -> None:
    assert emoji_to_lucide("⛓‍💥") == "bone-fracture"


def test_lucide_svg_for_emoji_hit_and_miss() -> None:
    assert "<svg" in (lucide_svg_for_emoji("📚") or "")
    assert lucide_svg_for_emoji("🪮") is None


def test_resolve_icon_svg_precedence_and_fallback() -> None:
    assert resolve_icon_svg("brain", "📚") == lucide_svg("brain")
    assert resolve_icon_svg(None, "📚") == lucide_svg("library")
    assert resolve_icon_svg("", "📚") == lucide_svg("library")
    assert resolve_icon_svg("definitely-not-an-icon", "📚") == lucide_svg("library")
    assert resolve_icon_svg("coins", "🪮") == lucide_svg("coins")


def test_activity_icon_overrides_use_explicit_lucide_names() -> None:
    from solstone.think.activities import DEFAULT_ACTIVITIES

    by_id = {activity["id"]: activity for activity in DEFAULT_ACTIVITIES}

    appointment = by_id["appointment"]
    assert resolve_icon_svg(appointment["icon"], appointment["emoji"]) == lucide_svg(
        "pin"
    )
    assert resolve_icon_svg(appointment["icon"], appointment["emoji"]) != lucide_svg(
        "map-pin"
    )

    event = by_id["event"]
    assert resolve_icon_svg(event["icon"], event["emoji"]) == lucide_svg("ticket")
    assert resolve_icon_svg(event["icon"], event["emoji"]) != lucide_svg("tags")


def test_search_lucide_icons_lock_matches_name_or_tag() -> None:
    tags = _lucide_tags()
    results = search_lucide_icons("lock")

    assert results
    assert all(
        "lock" in result["name"]
        or any("lock" in tag for tag in tags.get(result["name"], []))
        for result in results
    )


def test_search_lucide_icons_ranks_relevance_over_substring() -> None:
    # "lock" must surface the lock icon itself ahead of substring-only hits
    # like "alarm-clock"/"clock" (which only contain "lock" inside "clock").
    names = [r["name"] for r in search_lucide_icons("lock", limit=40)]
    assert names[0] == "lock"
    assert "alarm-clock" not in names[:5]
    if "clock" in names and "alarm-clock" in names:
        assert names.index("lock") < names.index("clock")
        assert names.index("lock") < names.index("alarm-clock")

    # Exact name matches rank first for other queries too.
    assert search_lucide_icons("heart")[0]["name"] == "heart"
    assert search_lucide_icons("shield")[0]["name"] == "shield"


def test_search_lucide_icons_empty_is_alphabetical_and_limited() -> None:
    results = search_lucide_icons("", limit=7)
    names = [result["name"] for result in results]

    assert len(results) == 7
    assert names == sorted(names)


def test_search_lucide_icons_respects_limit() -> None:
    assert len(search_lucide_icons("lock", limit=5)) <= 5


def test_search_lucide_icons_orders_name_matches_before_tag_only() -> None:
    results = search_lucide_icons("brain", limit=20)
    names = [result["name"] for result in results]
    tag_only_start = next(i for i, name in enumerate(names) if "brain" not in name)

    assert tag_only_start > 0
    assert all("brain" in name for name in names[:tag_only_start])
    assert all("brain" not in name for name in names[tag_only_start:])


def test_lucide_loading_is_package_relative(tmp_path: Path) -> None:
    env = os.environ.copy()
    pythonpath = env.get("PYTHONPATH")
    env["PYTHONPATH"] = (
        f"{REPO_ROOT}{os.pathsep}{pythonpath}" if pythonpath else str(REPO_ROOT)
    )
    result = subprocess.run(
        [
            sys.executable,
            "-c",
            (
                "from solstone.convey.icons import lucide_svg; "
                "raise SystemExit(0 if lucide_svg('house') else 1)"
            ),
        ],
        cwd=tmp_path,
        env=env,
        check=False,
    )

    assert result.returncode == 0


def test_app_registry_is_covered_by_lucide_map() -> None:
    registry = AppRegistry()
    registry.discover()

    assert set(registry.apps) <= set(APP_LUCIDE_MAP)


def test_app_lucide_map_values_exist_in_vendored_lucide_data() -> None:
    lucide_path = REPO_ROOT / "solstone" / "convey" / "static" / "icons" / "lucide.json"
    lucide_data = json.loads(lucide_path.read_text())

    assert set(APP_LUCIDE_MAP.values()) <= set(lucide_data)
