# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import re
import shutil
from html import unescape
from pathlib import Path
from unittest.mock import patch

from solstone.apps.reflections import copy as reflections_copy
from solstone.convey import create_app

REFLECTION_FIXTURE = Path("tests/fixtures/journal/reflections/weekly/20260308.md")


def _seed_reflection(journal: Path, content: str | None = None) -> None:
    target = journal / "reflections" / "weekly" / "20260308.md"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(
        content
        if content is not None
        else REFLECTION_FIXTURE.read_text(encoding="utf-8"),
        encoding="utf-8",
    )


def _make_client(journal: Path):
    app = create_app(str(journal))
    app.config["TESTING"] = True
    client = app.test_client()
    with client.session_transaction() as session:
        session["logged_in"] = True
        session.permanent = True
    return client


def _html(response) -> str:
    return unescape(response.get_data(as_text=True))


def _clear_weekly_reflections(journal: Path) -> None:
    shutil.rmtree(journal / "reflections" / "weekly", ignore_errors=True)


def test_reflections_index_lists_available_weeks(journal_copy):
    _seed_reflection(journal_copy)
    client = _make_client(journal_copy)

    response = client.get("/app/reflections/")
    html = _html(response)

    assert response.status_code == 200
    assert "Every Sunday, sol writes one reflection" in html
    assert "Available weekly reflections" not in html
    assert "No weekly reflections yet." not in html
    assert 'href="/app/reflections/20260308"' in html


def test_reflections_index_empty_state_shows_new_copy_next_date_and_sample_link(
    monkeypatch, journal_copy
):
    _clear_weekly_reflections(journal_copy)
    monkeypatch.setattr(
        "solstone.apps.reflections.routes.next_reflection_sunday",
        lambda journal, today, tz: "Sunday, March 15",
    )
    client = _make_client(journal_copy)

    response = client.get("/app/reflections/")
    html = _html(response)

    assert response.status_code == 200
    assert reflections_copy.SUBTITLE in html
    assert "Every Sunday, sol takes the week you've just lived through" in html
    assert "Your first reflection arrives on Sunday, March 15." in html
    assert reflections_copy.EMPTY_UNTIL_THEN in html
    assert 'href="/app/reflections/sample"' in html
    assert reflections_copy.SAMPLE_LINK_LABEL in html
    assert "Available weekly reflections" not in html
    assert "No weekly reflections yet." not in html


def test_reflections_index_empty_state_uses_fallback_when_next_date_unavailable(
    monkeypatch, journal_copy
):
    _clear_weekly_reflections(journal_copy)
    monkeypatch.setattr(
        "solstone.apps.reflections.routes.next_reflection_sunday",
        lambda journal, today, tz: None,
    )
    client = _make_client(journal_copy)

    response = client.get("/app/reflections/")
    html = _html(response)

    assert response.status_code == 200
    assert reflections_copy.EMPTY_NEXT_NO_DATE in html
    assert "next reflection:" not in html


def test_reflections_index_populated_state_shows_framing_sample_link_and_next_footer(
    monkeypatch, journal_copy
):
    _seed_reflection(journal_copy)
    monkeypatch.setattr(
        "solstone.apps.reflections.routes.next_reflection_sunday",
        lambda journal, today, tz: "Sunday, March 15",
    )
    client = _make_client(journal_copy)

    response = client.get("/app/reflections/")
    html = _html(response)

    assert response.status_code == 200
    assert reflections_copy.POPULATED_FRAMING in html
    assert 'href="/app/reflections/sample"' in html
    assert reflections_copy.POPULATED_SAMPLE_LINK in html
    assert "next reflection: Sunday, March 15" in html


def test_reflections_detail_renders_week(journal_copy):
    _seed_reflection(journal_copy)
    client = _make_client(journal_copy)

    response = client.get("/app/reflections/20260308")
    html = _html(response)

    assert response.status_code == 200
    assert "weekly reflection" in html
    assert "week of Sunday March 8th" in html
    assert ">copy<" in html
    assert ">download PDF<" in html


def test_reflections_sample_renders_fixture_markdown(journal_copy):
    client = _make_client(journal_copy)

    response = client.get("/app/reflections/sample")
    html = _html(response)

    assert response.status_code == 200
    assert reflections_copy.SAMPLE_BANNER in html
    assert "sample reflection" in html
    assert "boardroom balcony inflection" in html
    assert 'const rawUrl = "/app/reflections/sample/raw";' in html
    assert "download PDF" not in html


def test_reflections_sample_raw_returns_markdown(journal_copy):
    client = _make_client(journal_copy)

    response = client.get("/app/reflections/sample/raw")
    text = response.get_data(as_text=True)

    assert response.status_code == 200
    assert response.headers["Content-Type"] == "text/markdown; charset=utf-8"
    assert text.startswith("---\ntype: weekly_reflection")
    assert "boardroom balcony inflection" in text


def test_reflections_sample_missing_fixture_returns_plain_text_404(
    monkeypatch, tmp_path, journal_copy
):
    monkeypatch.setattr(
        "solstone.apps.reflections.routes.SAMPLE_FIXTURE_PATH",
        tmp_path / "missing.md",
    )
    client = _make_client(journal_copy)

    response = client.get("/app/reflections/sample")

    assert response.status_code == 404
    assert response.mimetype == "text/plain"
    assert "Sample reflection unavailable." in response.get_data(as_text=True)


def test_reflections_no_uppercase_transform_on_title(journal_copy):
    client = _make_client(journal_copy)

    response = client.get("/app/reflections/")
    html = response.get_data(as_text=True)

    assert '<h1 class="visually-hidden">reflections</h1>' in html
    for selector in (
        "body",
        "main",
        ".workspace",
        ".reflection-shell",
        ".reflection-header",
        ".reflection-title",
    ):
        match = re.search(rf"{re.escape(selector)}\s*\{{(?P<body>.*?)\}}", html, re.S)
        if match is None:
            continue
        rule_body = match.group("body")
        assert "text-transform: uppercase" not in rule_body
        assert "text-transform: capitalize" not in rule_body


def test_reflections_no_mirror_string_in_surface(journal_copy):
    _seed_reflection(journal_copy)
    client = _make_client(journal_copy)

    responses = [
        client.get("/app/reflections/"),
        client.get("/app/reflections/20260308"),
        client.get("/app/reflections/sample"),
    ]

    for response in responses:
        html = _html(response)
        assert response.status_code == 200
        assert "mirror" not in html.lower()
        assert "🪞" not in html


def test_reflections_app_json_icon_is_moon():
    data = json.loads(Path("solstone/apps/reflections/app.json").read_text())

    assert data["icon"] == "🌙"


def test_reflections_detail_canonicalizes_to_sunday(journal_copy):
    _seed_reflection(journal_copy)
    client = _make_client(journal_copy)

    response = client.get("/app/reflections/20260310")

    assert response.status_code == 302
    assert response.headers["Location"].endswith("/app/reflections/20260308")


def test_reflections_missing_week_returns_plain_text_404(journal_copy):
    client = _make_client(journal_copy)

    response = client.get("/app/reflections/20260315")

    assert response.status_code == 404
    assert response.mimetype == "text/plain"
    assert "Reflection not found" in response.get_data(as_text=True)


def test_reflections_raw_returns_markdown(journal_copy):
    _seed_reflection(journal_copy)
    client = _make_client(journal_copy)

    response = client.get("/app/reflections/20260308/raw")
    text = response.get_data(as_text=True)

    assert response.status_code == 200
    assert response.headers["Content-Type"] == "text/markdown; charset=utf-8"
    assert text.startswith("---\ntype: weekly_reflection")


def test_reflections_pdf_returns_attachment(journal_copy):
    _seed_reflection(journal_copy)
    client = _make_client(journal_copy)

    response = client.get("/app/reflections/20260308/pdf")

    assert response.status_code == 200
    assert response.mimetype == "application/pdf"
    assert (
        response.headers["Content-Disposition"]
        == 'attachment; filename="reflection-20260308.pdf"'
    )
    assert response.data.startswith(b"%PDF")


def test_reflections_pdf_rejects_remote_assets(journal_copy):
    _seed_reflection(
        journal_copy,
        """---
type: weekly_reflection
week: 20260308
generated: 2026-03-10T19:00:00Z
model: openai/gpt-5
sources:
  newsletters: 0
  activities: 0
  decisions: 0
  followups: 0
  todos: 0
  relationship_signals: 0
gaps: []
---

![remote](https://example.com/reflection.png)
""",
    )
    client = _make_client(journal_copy)

    with (
        patch(
            "urllib.request.urlopen",
            side_effect=AssertionError("network disabled during reflection pdf render"),
        ),
        patch("weasyprint.default_url_fetcher") as mock_fetcher,
    ):
        response = client.get("/app/reflections/20260308/pdf")

    assert response.status_code == 200
    assert response.mimetype == "application/pdf"
    assert response.data.startswith(b"%PDF")
    mock_fetcher.assert_not_called()


def test_reflections_stats_returns_month_counts(journal_copy):
    _seed_reflection(journal_copy)
    client = _make_client(journal_copy)

    response = client.get("/app/reflections/api/stats/202603")

    assert response.status_code == 200
    assert response.get_json() == {"20260308": 1}
