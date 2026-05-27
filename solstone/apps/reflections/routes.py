# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import logging
from datetime import date, datetime
from pathlib import Path
from typing import Any, Callable
from urllib.parse import urlsplit

import frontmatter
from flask import Blueprint, Response, jsonify, redirect, render_template, url_for
from markdown import Markdown

from solstone.apps.reflections import copy as reflections_copy
from solstone.apps.reflections.dates import next_reflection_sunday
from solstone.convey.reasons import INVALID_MONTH
from solstone.convey.utils import DATE_RE, error_response, format_date
from solstone.think.features import require_extra
from solstone.think.utils import get_journal, get_owner_timezone, sunday_of_week

logger = logging.getLogger(__name__)

SAMPLE_FIXTURE_PATH = (
    Path(__file__).parent.parent.parent.parent
    / "tests"
    / "fixtures"
    / "journal"
    / "reflections"
    / "weekly"
    / "20260308.md"
)

reflections_bp = Blueprint(
    "app:reflections",
    __name__,
    url_prefix="/app/reflections",
)


def _reflections_dir() -> Path:
    return Path(get_journal()) / "reflections" / "weekly"


def _plain_not_found(
    message: str = "Reflection not found",
) -> tuple[str, int, dict[str, str]]:
    return (message, 404, {"Content-Type": "text/plain; charset=utf-8"})


def _parse_day_token(day: str) -> datetime | None:
    if not DATE_RE.fullmatch(day):
        return None
    try:
        return datetime.strptime(day, "%Y%m%d")
    except ValueError:
        return None


def _canonical_week_day(day: str) -> str | None:
    day_dt = _parse_day_token(day)
    if day_dt is None:
        return None
    return sunday_of_week(day_dt, get_owner_timezone())


def _reflection_path(day: str) -> Path:
    return _reflections_dir() / f"{day}.md"


def _list_reflection_days() -> list[str]:
    reflections_dir = _reflections_dir()
    if not reflections_dir.is_dir():
        return []
    days = [
        path.stem
        for path in reflections_dir.glob("*.md")
        if path.is_file() and DATE_RE.fullmatch(path.stem)
    ]
    return sorted(days, reverse=True)


def _load_reflection(day: str) -> tuple[Path, str, frontmatter.Post]:
    path = _reflection_path(day)
    if not path.is_file():
        raise FileNotFoundError(day)
    raw_markdown = path.read_text(encoding="utf-8")
    return path, raw_markdown, frontmatter.loads(raw_markdown)


def _weasyprint() -> tuple[type, Callable[..., Any]]:
    require_extra("pdf")
    from weasyprint import HTML, default_url_fetcher

    return HTML, default_url_fetcher


def _safe_pdf_url_fetcher(url: str, *args: Any, **kwargs: Any) -> dict[str, Any]:
    _, default_url_fetcher = _weasyprint()
    scheme = urlsplit(url).scheme.lower()
    if scheme in {"http", "https"}:
        raise ValueError("Remote assets are disabled for reflection PDFs")
    return default_url_fetcher(url, *args, **kwargs)


def _render_reflection_pdf(path: Path, post: frontmatter.Post) -> bytes:
    HTML, _ = _weasyprint()
    markdown = Markdown(extensions=["extra", "sane_lists"])
    body_html = markdown.convert(post.content)
    html = render_template(
        "reflections/pdf.html",
        week_label=format_date(path.stem),
        reflection_html=body_html,
    )
    return HTML(
        string=html,
        base_url=path.parent.resolve().as_uri(),
        url_fetcher=_safe_pdf_url_fetcher,
    ).write_pdf()


def _canonical_redirect(endpoint: str, day: str) -> Response | None:
    canonical_day = _canonical_week_day(day)
    if canonical_day is None:
        return None
    if canonical_day == day:
        return None
    return redirect(url_for(endpoint, day=canonical_day), code=302)


@reflections_bp.route("/")
def index() -> str:
    tz = get_owner_timezone()
    today: date = datetime.now(tz).date()
    journal = Path(get_journal())
    next_sunday = next_reflection_sunday(journal, today, tz)
    if next_sunday is None:
        empty_next = reflections_copy.EMPTY_NEXT_NO_DATE
        populated_next_footer = None
    else:
        empty_next = reflections_copy.EMPTY_NEXT_WITH_DATE.format(sunday=next_sunday)
        populated_next_footer = reflections_copy.POPULATED_NEXT_FOOTER.format(
            sunday=next_sunday
        )

    weeks = [
        {
            "day": day,
            "label": format_date(day),
            "url": url_for("app:reflections.week_view", day=day),
        }
        for day in _list_reflection_days()
    ]
    return render_template(
        "app.html",
        app="reflections",
        view_mode="index",
        weeks=weeks,
        subtitle=reflections_copy.SUBTITLE,
        empty_body=reflections_copy.EMPTY_BODY,
        empty_next=empty_next,
        empty_until_then=reflections_copy.EMPTY_UNTIL_THEN,
        sample_link_label=reflections_copy.SAMPLE_LINK_LABEL,
        sample_url=url_for("app:reflections.sample"),
        populated_framing=reflections_copy.POPULATED_FRAMING,
        populated_sample_link=reflections_copy.POPULATED_SAMPLE_LINK,
        populated_next_footer=populated_next_footer,
    )


@reflections_bp.route("/<day>")
def week_view(day: str) -> Any:
    redirect_response = _canonical_redirect("app:reflections.week_view", day)
    if redirect_response is not None:
        return redirect_response

    canonical_day = _canonical_week_day(day)
    if canonical_day is None:
        return _plain_not_found("Reflection not found")

    try:
        _path, _raw_markdown, post = _load_reflection(canonical_day)
    except FileNotFoundError:
        return _plain_not_found("Reflection not found")

    return render_template(
        "app.html",
        app="reflections",
        day=canonical_day,
        view_mode="detail",
        reflection_day=canonical_day,
        reflection_week_label=format_date(canonical_day),
        reflection_markdown=post.content,
        raw_url=url_for("app:reflections.week_raw", day=canonical_day),
        pdf_url=url_for("app:reflections.week_pdf", day=canonical_day),
    )


@reflections_bp.route("/<day>/raw")
def week_raw(day: str) -> Any:
    redirect_response = _canonical_redirect("app:reflections.week_raw", day)
    if redirect_response is not None:
        return redirect_response

    canonical_day = _canonical_week_day(day)
    if canonical_day is None:
        return _plain_not_found("Reflection not found")

    try:
        _path, raw_markdown, _post = _load_reflection(canonical_day)
    except FileNotFoundError:
        return _plain_not_found("Reflection not found")

    return (
        raw_markdown,
        200,
        {"Content-Type": "text/markdown; charset=utf-8"},
    )


@reflections_bp.route("/<day>/pdf")
def week_pdf(day: str) -> Any:
    redirect_response = _canonical_redirect("app:reflections.week_pdf", day)
    if redirect_response is not None:
        return redirect_response

    canonical_day = _canonical_week_day(day)
    if canonical_day is None:
        return _plain_not_found("Reflection not found")

    try:
        path, _raw_markdown, post = _load_reflection(canonical_day)
        pdf_bytes = _render_reflection_pdf(path, post)
    except FileNotFoundError:
        return _plain_not_found("Reflection not found")
    except ValueError as exc:
        return (str(exc), 400, {"Content-Type": "text/plain; charset=utf-8"})

    return Response(
        pdf_bytes,
        mimetype="application/pdf",
        headers={
            "Content-Disposition": (
                f'attachment; filename="reflection-{canonical_day}.pdf"'
            )
        },
    )


@reflections_bp.route("/sample")
def sample() -> Any:
    if not SAMPLE_FIXTURE_PATH.exists():
        # Source-only: fixture is excluded from packaged installs (pyproject.toml).
        logger.warning("sample reflection fixture not found at %s", SAMPLE_FIXTURE_PATH)
        return _plain_not_found("Sample reflection unavailable.")
    post = frontmatter.loads(SAMPLE_FIXTURE_PATH.read_text(encoding="utf-8"))
    return render_template(
        "app.html",
        app="reflections",
        view_mode="sample",
        reflection_markdown=post.content,
        raw_url=url_for("app:reflections.sample_raw"),
        sample_banner=reflections_copy.SAMPLE_BANNER,
    )


@reflections_bp.route("/sample/raw")
def sample_raw() -> Any:
    if not SAMPLE_FIXTURE_PATH.exists():
        return _plain_not_found("Sample reflection unavailable.")
    return (
        SAMPLE_FIXTURE_PATH.read_text(encoding="utf-8"),
        200,
        {"Content-Type": "text/markdown; charset=utf-8"},
    )


@reflections_bp.route("/api/stats/<month>")
def api_stats(month: str) -> Any:
    if len(month) != 6 or not month.isdigit():
        return error_response(
            INVALID_MONTH,
            detail="Invalid month format, expected YYYYMM",
        )

    stats = {day: 1 for day in _list_reflection_days() if day.startswith(month)}
    return jsonify(stats)
