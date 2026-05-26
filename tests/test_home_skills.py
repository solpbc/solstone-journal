# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for home pulse skill surfacing."""

from __future__ import annotations

import json
from datetime import datetime, timedelta, timezone

import pytest

from solstone.apps.home.routes import (
    _collect_skills,
    _load_skills_state,
    _save_skills_state,
    home_bp,
)


@pytest.fixture
def home_client():
    """Create a Flask test client with home routes registered."""
    from flask import Flask

    app = Flask(__name__)
    app.register_blueprint(home_bp)
    return app.test_client()


def _write_skill_fixtures(tmp_path, patterns, skill_files):
    """Write owner-wide skill patterns and markdown profiles."""
    skills_dir = tmp_path / "skills"
    skills_dir.mkdir(parents=True, exist_ok=True)
    lines = [json.dumps(pattern) for pattern in patterns]
    (skills_dir / "patterns.jsonl").write_text(
        "\n".join(lines) + ("\n" if lines else ""),
        encoding="utf-8",
    )
    for slug, content in skill_files.items():
        (skills_dir / f"{slug}.md").write_text(content, encoding="utf-8")


def _pattern(
    *,
    slug: str,
    name: str,
    status: str = "mature",
    observations: list[dict] | None = None,
) -> dict:
    rows = observations or [
        {
            "day": "2026-04-10",
            "facet": "work",
            "activity_ids": ["act_1"],
            "notes": "",
            "recorded_at": "2026-04-10T09:15:00Z",
        }
    ]
    return {
        "slug": slug,
        "name": name,
        "status": status,
        "observations": rows,
        "facets_touched": sorted(
            {
                str(observation.get("facet") or "")
                for observation in rows
                if observation.get("facet")
            }
        ),
        "first_seen": rows[0]["day"],
        "last_seen": rows[-1]["day"],
        "needs_profile": False,
        "needs_refresh": False,
        "profile_generated_at": "2026-04-11T10:00:00Z",
        "created_at": "2026-04-10T09:20:00Z",
        "updated_at": "2026-04-11T10:00:00Z",
    }


def _profile_markdown(
    *,
    name: str,
    display_name: str,
    description: str,
    category: str = "coordination",
    confidence: float = 0.7,
    body: str = "## Overview\n\nProfile body.",
) -> str:
    return f"""---
name: "{name}"
display_name: "{display_name}"
description: "{description}"
category: "{category}"
confidence: {confidence}
---

{body}
"""


def test_collect_skills_no_facets(monkeypatch, tmp_path):
    """No owner-wide skills directory yields an empty list."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    assert _collect_skills() == []


def test_collect_skills_no_patterns(monkeypatch, tmp_path):
    """An empty owner-wide skills directory yields an empty list."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    (tmp_path / "skills").mkdir(parents=True)

    assert _collect_skills() == []


def test_collect_skills_with_owner_wide_profile(monkeypatch, tmp_path):
    """Pulse collects owner-wide profiles with the new payload shape."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    _write_skill_fixtures(
        tmp_path,
        [
            _pattern(
                slug="morning-standup",
                name="Standup Pattern",
                observations=[
                    {
                        "day": "2026-03-01",
                        "facet": "work",
                        "activity_ids": ["act_1"],
                        "notes": "",
                        "recorded_at": "2026-03-01T09:00:00Z",
                    },
                    {
                        "day": "2026-04-10",
                        "facet": "work",
                        "activity_ids": ["act_2"],
                        "notes": "",
                        "recorded_at": "2026-04-10T09:15:00Z",
                    },
                ],
            )
        ],
        {
            "morning-standup": _profile_markdown(
                name="morning-standup",
                display_name="Morning Standup",
                description="Daily engineering sync for blockers and updates.",
                category="coordination",
                confidence=0.9,
                body="## when this happens\n\nDaily morning standup with the engineering team.",
            )
        },
    )

    skills = _collect_skills()

    assert len(skills) == 1
    assert skills[0]["id"] == "morning-standup"
    assert skills[0]["slug"] == "morning-standup"
    assert skills[0]["name"] == "Morning Standup"
    assert (
        skills[0]["description"] == "Daily engineering sync for blockers and updates."
    )
    assert skills[0]["category"] == "coordination"
    assert skills[0]["confidence"] == 0.9
    assert skills[0]["status"] == "mature"
    assert skills[0]["facets"] == ["work"]
    assert skills[0]["observations"] == 2
    assert skills[0]["first_seen"] == "2026-03-01T09:00:00Z"
    assert skills[0]["last_seen"] == "2026-04-10T09:15:00Z"
    assert "when this happens" in skills[0]["content"]
    assert skills[0]["seen"] is False


def test_collect_skills_hides_pattern_without_profile(monkeypatch, tmp_path):
    """Observer-only patterns stay hidden until a profile exists."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    _write_skill_fixtures(
        tmp_path,
        [_pattern(slug="random-chat", name="Random Chat")],
        {},
    )

    assert _collect_skills() == []


def test_collect_skills_seen_flag(monkeypatch, tmp_path):
    """Profiles older than the last seen marker are marked seen."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    _write_skill_fixtures(
        tmp_path,
        [_pattern(slug="daily-review", name="Daily Review")],
        {
            "daily-review": _profile_markdown(
                name="daily-review",
                display_name="Daily Review",
                description="End-of-day review habit.",
                body="Review content.",
            )
        },
    )

    _save_skills_state(
        {
            "skills_last_seen": (
                datetime.now(timezone.utc) + timedelta(minutes=5)
            ).isoformat()
        }
    )

    skills = _collect_skills()

    assert len(skills) == 1
    assert skills[0]["seen"] is True


def test_collect_skills_tolerates_aware_naive_state_mix(monkeypatch, tmp_path):
    """Profile mtime is aware UTC; legacy naive last_seen ISO must not raise.

    Regression for `req_qufugcsv` — pre-fix, _collect_skills compared an aware
    last_seen_dt (writer emits aware UTC) against a naive profile_mtime
    (datetime.fromtimestamp without tz=), raising TypeError on every render and
    silently blanking the Pulse skills list.
    """
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    _write_skill_fixtures(
        tmp_path,
        [_pattern(slug="daily-review", name="Daily Review")],
        {
            "daily-review": _profile_markdown(
                name="daily-review",
                display_name="Daily Review",
                description="End-of-day review habit.",
            )
        },
    )

    aware_state = {
        "skills_last_seen": (
            datetime.now(timezone.utc) + timedelta(minutes=5)
        ).isoformat()
    }
    _save_skills_state(aware_state)
    skills_aware = _collect_skills()
    assert len(skills_aware) == 1
    assert skills_aware[0]["seen"] is True

    naive_state = {
        "skills_last_seen": (datetime.now(timezone.utc) + timedelta(minutes=5))
        .replace(tzinfo=None)
        .isoformat()
    }
    _save_skills_state(naive_state)
    skills_naive = _collect_skills()
    assert len(skills_naive) == 1
    assert skills_naive[0]["seen"] is True


def test_collect_skills_shows_dormant(monkeypatch, tmp_path):
    """Dormant skills stay visible in Pulse."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    _write_skill_fixtures(
        tmp_path,
        [_pattern(slug="deep-work", name="Deep Work", status="dormant")],
        {
            "deep-work": _profile_markdown(
                name="deep-work",
                display_name="Deep Work",
                description="Focused solo execution work.",
            )
        },
    )

    skills = _collect_skills()

    assert len(skills) == 1
    assert skills[0]["status"] == "dormant"


def test_collect_skills_hides_retired(monkeypatch, tmp_path):
    """Retired skills are excluded from Pulse."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    _write_skill_fixtures(
        tmp_path,
        [_pattern(slug="legacy", name="Legacy Skill", status="retired")],
        {
            "legacy": _profile_markdown(
                name="legacy",
                display_name="Legacy Skill",
                description="Old profile.",
            )
        },
    )

    assert _collect_skills() == []


def test_collect_skills_sorts_by_confidence_then_last_seen(monkeypatch, tmp_path):
    """Skills sort by confidence first, then recency."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    _write_skill_fixtures(
        tmp_path,
        [
            _pattern(slug="high-confidence", name="High Confidence"),
            _pattern(
                slug="recent-mid",
                name="Recent Mid",
                observations=[
                    {
                        "day": "2026-04-12",
                        "facet": "work",
                        "activity_ids": ["act_2"],
                        "notes": "",
                        "recorded_at": "2026-04-12T08:00:00Z",
                    }
                ],
            ),
            _pattern(
                slug="older-mid",
                name="Older Mid",
                observations=[
                    {
                        "day": "2026-04-05",
                        "facet": "work",
                        "activity_ids": ["act_3"],
                        "notes": "",
                        "recorded_at": "2026-04-05T08:00:00Z",
                    }
                ],
            ),
        ],
        {
            "high-confidence": _profile_markdown(
                name="high-confidence",
                display_name="High Confidence",
                description="High confidence skill.",
                confidence=0.95,
            ),
            "recent-mid": _profile_markdown(
                name="recent-mid",
                display_name="Recent Mid",
                description="Recent mid confidence skill.",
                confidence=0.6,
            ),
            "older-mid": _profile_markdown(
                name="older-mid",
                display_name="Older Mid",
                description="Older mid confidence skill.",
                confidence=0.6,
            ),
        },
    )

    skills = _collect_skills()

    assert [skill["id"] for skill in skills] == [
        "high-confidence",
        "recent-mid",
        "older-mid",
    ]


def test_api_skills_seen(monkeypatch, tmp_path, home_client):
    """Seen endpoint persists the skills seen timestamp."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    resp = home_client.post("/app/home/api/skills/seen")

    assert resp.status_code == 200
    assert resp.get_json() == {"ok": True}
    state = _load_skills_state()
    assert "skills_last_seen" in state


def test_api_pulse_includes_skills(monkeypatch, home_client):
    """Pulse API includes skills data from the context builder."""
    monkeypatch.setattr(
        "solstone.apps.home.routes.get_capture_health",
        lambda: {"status": "active", "observers": []},
    )
    monkeypatch.setattr("solstone.apps.home.routes.get_cached_state", lambda: {})
    monkeypatch.setattr(
        "solstone.apps.home.routes._resolve_attention", lambda awareness: None
    )
    monkeypatch.setattr("solstone.apps.home.routes._load_stats", lambda today: {})
    monkeypatch.setattr(
        "solstone.apps.home.routes._load_flow_md", lambda today: (None, None)
    )
    monkeypatch.setattr(
        "solstone.apps.home.routes._load_pulse_md", lambda: (None, None, [])
    )
    monkeypatch.setattr(
        "solstone.apps.home.routes._collect_anticipated_activities", lambda today: []
    )
    monkeypatch.setattr(
        "solstone.apps.home.routes._collect_activities", lambda today: []
    )
    monkeypatch.setattr("solstone.apps.home.routes._collect_todos", lambda today: [])
    monkeypatch.setattr("solstone.apps.home.routes._collect_routines", lambda: [])
    monkeypatch.setattr(
        "solstone.apps.home.routes._collect_skills",
        lambda: [
            {
                "id": "morning-standup",
                "slug": "morning-standup",
                "name": "Morning Standup",
                "description": "Daily engineering sync for blockers and updates.",
                "category": "coordination",
                "confidence": 0.9,
                "status": "mature",
                "facets": ["work"],
                "observations": 5,
                "first_seen": "2026-03-01T09:00:00Z",
                "last_seen": "2026-04-10T09:15:00Z",
                "content": "# Standup\n\nDaily standup.",
                "seen": False,
            }
        ],
    )

    resp = home_client.get("/app/home/api/pulse")

    assert resp.status_code == 200
    data = resp.get_json()
    assert "skills" in data
    assert data["skills"][0]["name"] == "Morning Standup"
    assert data["skills"][0]["facets"] == ["work"]
    assert "skills_summary" in data
    assert "skills_content" in data
    assert "morning-standup" in data["skills_content"]
