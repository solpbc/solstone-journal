# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import re
from datetime import date, datetime
from pathlib import Path
from typing import Any, Callable
from urllib.parse import urlsplit

import frontmatter
from flask import Blueprint, Response, current_app, jsonify, render_template, request, url_for
from markdown import Markdown

from solstone.apps.news import copy as news_copy
from solstone.apps.news.dates import format_news_list_date, next_newsletter_when
from solstone.convey.date_nav import build_date_nav_index
from solstone.convey.day_grid import build_day_grid_payload
from solstone.convey.reasons import (
    FILE_NOT_FOUND,
    INVALID_DAY,
    INVALID_MONTH,
    INVALID_REQUEST_VALUE,
)
from solstone.think.facets import get_facet_news
from solstone.convey.utils import DATE_RE, error_response
from solstone.think.features import require_extra
from solstone.think.utils import get_journal, get_owner_timezone

news_bp = Blueprint(
    "app:news",
    __name__,
    url_prefix="/app/news",
)

# Facet directory names use the same identifier shape as facet slugs.
_FACET_RE = re.compile(r"[A-Za-z0-9_-]+")


def _journal_root() -> Path:
    return Path(get_journal())


def _facets_root() -> Path:
    return _journal_root() / "facets"


def _newsletter_path(facet: str, day: str) -> Path:
    return _facets_root() / facet / "news" / f"{day}.md"


def _plain_not_found() -> tuple[str, int, dict[str, str]]:
    return ("Newsletter not found", 404, {"Content-Type": "text/plain; charset=utf-8"})


def _list_newsletters() -> list[dict[str, str]]:
    """Return reverse-chrono list of (facet, day) newsletters.

    Reads every `facets/*/news/*.md` whose filename matches YYYYMMDD. The list
    is sorted by day desc, then facet asc for stable ordering inside a day.
    """
    facets_root = _facets_root()
    if not facets_root.is_dir():
        return []

    rows: list[dict[str, str]] = []
    for facet_dir in facets_root.iterdir():
        if not facet_dir.is_dir():
            continue
        if not _FACET_RE.fullmatch(facet_dir.name):
            continue
        news_dir = facet_dir / "news"
        if not news_dir.is_dir():
            continue
        for path in news_dir.glob("*.md"):
            if not path.is_file():
                continue
            day = path.stem
            if not DATE_RE.fullmatch(day):
                continue
            rows.append({"facet": facet_dir.name, "day": day})

    rows.sort(key=lambda r: (-int(r["day"]), r["facet"]))
    return rows


def _format_month_name(day: str) -> str:
    return datetime.strptime(day, "%Y%m%d").strftime("%B %Y")


def _newsletter_counts_by_day(rows: list[dict[str, str]]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for row in rows:
        day = row["day"]
        counts[day] = counts.get(day, 0) + 1
    return counts


def _newsletter_list_item(row: dict[str, str]) -> dict[str, str]:
    return {
        "facet": row["facet"],
        "day": row["day"],
        "label": format_news_list_date(row["day"]),
        "url": url_for("app:news.detail", facet=row["facet"], day=row["day"]),
    }


def _day_copy(day: str) -> dict[str, str]:
    date_label = format_news_list_date(day)
    return {
        "title": news_copy.NEWS_DAY_TITLE.format(date_label=date_label),
        "subtitle": news_copy.NEWS_DAY_SUBTITLE,
        "empty_title": news_copy.NEWS_DAY_EMPTY_TITLE.format(date_label=date_label),
        "empty_body": news_copy.NEWS_DAY_EMPTY_BODY,
    }


def _empty_detail_payload(facet: str, day: str) -> dict[str, Any]:
    date_label = format_news_list_date(day)
    return {
        "empty": True,
        "facet": facet,
        "day": day,
        "date_label": date_label,
        "day_url": url_for("app:news.day_view", day=day),
        "copy": {
            "empty_title": news_copy.NEWS_DETAIL_EMPTY_TITLE.format(facet=facet),
            "empty_body": news_copy.NEWS_DETAIL_EMPTY_BODY.format(
                facet=facet, date_label=date_label
            ),
            "day_link": news_copy.NEWS_DETAIL_EMPTY_DAY_LINK,
        },
    }


def _load_newsletter(facet: str, day: str) -> tuple[Path, str, frontmatter.Post]:
    path = _newsletter_path(facet, day)
    if not path.is_file():
        raise FileNotFoundError(f"{facet}/{day}")
    raw_markdown = path.read_text(encoding="utf-8")
    return path, raw_markdown, frontmatter.loads(raw_markdown)


def _weasyprint() -> tuple[type, Callable[..., Any]]:
    require_extra("pdf-export")
    from weasyprint import HTML, default_url_fetcher

    return HTML, default_url_fetcher


def _safe_pdf_url_fetcher(url: str, *args: Any, **kwargs: Any) -> dict[str, Any]:
    _, default_url_fetcher = _weasyprint()
    scheme = urlsplit(url).scheme.lower()
    if scheme in {"http", "https"}:
        raise ValueError("Remote assets are disabled for newsletter PDFs")
    return default_url_fetcher(url, *args, **kwargs)


def _render_newsletter_pdf(
    path: Path, post: frontmatter.Post, facet: str, day: str
) -> bytes:
    HTML, _ = _weasyprint()
    markdown = Markdown(extensions=["extra", "sane_lists"])
    body_html = markdown.convert(post.content)
    html = render_template(
        "news/pdf.html",
        facet=facet,
        date_label=format_news_list_date(day),
        newsletter_html=body_html,
    )
    return HTML(
        string=html,
        base_url=path.parent.resolve().as_uri(),
        url_fetcher=_safe_pdf_url_fetcher,
    ).write_pdf()


@news_bp.route("/")
def index() -> Any:
    return current_app.send_static_file("shell.html")


@news_bp.route("/api/state")
def api_state() -> Any:
    rows = _list_newsletters()
    total_count = len(rows)
    when = next_newsletter_when(_today())

    newsletters = [_newsletter_list_item(row) for row in rows[:60]]

    empty_next = news_copy.NEWS_EMPTY_TOMORROW_WITH_DATE.format(tomorrow=when)
    populated_next_footer = news_copy.NEWS_POPULATED_NEXT_FOOTER.format(when=when)
    if not _journal_has_any_observer_input():
        empty_next = news_copy.NEWS_EMPTY_NO_DATE
    if rows:
        template = (
            news_copy.NEWS_GRID_LEDE_ONE
            if total_count == 1
            else news_copy.NEWS_GRID_LEDE_OTHER
        )
        grid_lede = template.format(
            count=total_count, month=_format_month_name(rows[-1]["day"])
        )
    else:
        grid_lede = None

    return jsonify(
        {
            "newsletters": newsletters,
            "total_count": total_count,
            "copy": {
                "kicker": news_copy.NEWS_KICKER,
                "index_h1": news_copy.NEWS_INDEX_H1,
                "subtitle": news_copy.NEWS_SUBTITLE,
                "empty_body": news_copy.NEWS_EMPTY_BODY,
                "empty_next": empty_next,
                "empty_until_then": news_copy.NEWS_EMPTY_UNTIL_THEN,
                "sample_link_label": news_copy.NEWS_SAMPLE_LINK_LABEL,
                "sample_url": url_for("app:news.sample"),
                "populated_framing": news_copy.NEWS_POPULATED_FRAMING,
                "populated_sample_link": news_copy.NEWS_POPULATED_SAMPLE_LINK,
                "populated_next_footer": populated_next_footer,
                "grid_title": news_copy.NEWS_GRID_TITLE,
                "grid_lede": grid_lede,
                "grid_unit_one": news_copy.NEWS_GRID_UNIT_ONE,
                "grid_unit_other": news_copy.NEWS_GRID_UNIT_OTHER,
                "grid_unit_none": news_copy.NEWS_GRID_UNIT_NONE,
            },
        }
    )


@news_bp.route("/api/index")
def api_index() -> Any:
    return jsonify(build_date_nav_index(_newsletter_counts_by_day(_list_newsletters())))


@news_bp.route("/api/grid")
def api_grid() -> Any:
    rows = _list_newsletters()
    counts = _newsletter_counts_by_day(rows)
    coverage = (
        {"start": min(counts), "end": _today().strftime("%Y%m%d")} if counts else None
    )
    return jsonify(
        build_day_grid_payload(
            counts,
            max(counts, default=None),
            coverage=coverage,
        )
    )


@news_bp.route("/api/stats/<month>")
def api_stats(month: str) -> Any:
    if len(month) != 6 or not month.isdigit():
        return error_response(
            INVALID_MONTH,
            detail="Invalid month format, expected YYYYMM",
        )

    counts = _newsletter_counts_by_day(_list_newsletters())
    return jsonify(
        {day: count for day, count in counts.items() if day.startswith(month)}
    )


@news_bp.route("/api/day/<day>")
def api_day(day: str) -> Any:
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, status=404, detail="Day not found")

    rows = [row for row in _list_newsletters() if row["day"] == day]
    payload: dict[str, Any] = {
        "day": day,
        "date_label": format_news_list_date(day),
        "newsletters": [_newsletter_list_item(row) for row in rows],
        "copy": _day_copy(day),
    }
    if not rows:
        payload["empty"] = True
    return jsonify(payload)


@news_bp.route("/api/facet/<facet>")
def api_facet_news(facet: str) -> Any:
    """Return a paginated facet-news feed for the native journal command."""
    if not _FACET_RE.fullmatch(facet):
        return error_response(INVALID_REQUEST_VALUE, detail="invalid facet")
    day = request.args.get("day", "").strip() or None
    cursor = request.args.get("cursor", "").strip() or None
    if day is not None and not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, detail="day must be YYYYMMDD")
    if cursor is not None and not DATE_RE.fullmatch(cursor):
        return error_response(INVALID_REQUEST_VALUE, detail="cursor must be YYYYMMDD")
    try:
        limit = int(request.args.get("limit", 5))
    except ValueError:
        return error_response(INVALID_REQUEST_VALUE, detail="limit must be an integer")
    if not 1 <= limit <= 100:
        return error_response(INVALID_REQUEST_VALUE, detail="limit must be between 1 and 100")
    result = get_facet_news(facet, cursor=cursor, limit=limit, day=day)
    return jsonify({"facet": facet, **result})


@news_bp.route("/sample")
def sample() -> Any:
    return current_app.send_static_file("shell.html")


@news_bp.route("/api/sample")
def api_sample() -> Any:
    post = frontmatter.loads(news_copy.SAMPLE_CONTENT)
    return jsonify(
        {
            "markdown": post.content,
            "raw_url": url_for("app:news.sample_raw"),
            "kicker": news_copy.NEWS_KICKER,
            "sample_h1": news_copy.NEWS_SAMPLE_H1,
            "sample_banner": news_copy.NEWS_SAMPLE_BANNER,
        }
    )


@news_bp.route("/sample/raw")
def sample_raw() -> Any:
    return (
        news_copy.SAMPLE_CONTENT,
        200,
        {"Content-Type": "text/markdown; charset=utf-8"},
    )


@news_bp.route("/<day>")
def day_view(day: str) -> Any:
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, status=404, detail="Day not found")

    return current_app.send_static_file("shell.html")


@news_bp.route("/<facet>/<day>")
def detail(facet: str, day: str) -> Any:
    return current_app.send_static_file("shell.html")


@news_bp.route("/api/<facet>/<day>")
def api_detail(facet: str, day: str) -> Any:
    if not _FACET_RE.fullmatch(facet) or not DATE_RE.fullmatch(day):
        return error_response(FILE_NOT_FOUND, detail="Newsletter not found")

    try:
        _path, _raw_markdown, post = _load_newsletter(facet, day)
    except FileNotFoundError:
        return jsonify(_empty_detail_payload(facet, day))

    return jsonify(
        {
            "markdown": post.content,
            "raw_url": url_for("app:news.detail_raw", facet=facet, day=day),
            "pdf_url": url_for("app:news.detail_pdf", facet=facet, day=day),
            "kicker": news_copy.NEWS_KICKER,
            "facet": facet,
            "date_label": format_news_list_date(day),
            "subtitle": news_copy.NEWS_DETAIL_SUBTITLE.format(facet=facet),
            "debug_link_label": news_copy.NEWS_DETAIL_DEBUG_LINK,
            "debug_link_url": f"/app/sol/{day}/talents/facet_newsletter",
        }
    )


@news_bp.route("/<facet>/<day>/raw")
def detail_raw(facet: str, day: str) -> Any:
    if not _FACET_RE.fullmatch(facet) or not DATE_RE.fullmatch(day):
        return _plain_not_found()

    try:
        _path, raw_markdown, _post = _load_newsletter(facet, day)
    except FileNotFoundError:
        return _plain_not_found()

    return (
        raw_markdown,
        200,
        {"Content-Type": "text/markdown; charset=utf-8"},
    )


@news_bp.route("/<facet>/<day>/pdf")
def detail_pdf(facet: str, day: str) -> Any:
    if not _FACET_RE.fullmatch(facet) or not DATE_RE.fullmatch(day):
        return _plain_not_found()

    try:
        path, _raw_markdown, post = _load_newsletter(facet, day)
        pdf_bytes = _render_newsletter_pdf(path, post, facet, day)
    except FileNotFoundError:
        return _plain_not_found()
    except ValueError as exc:
        return (str(exc), 400, {"Content-Type": "text/plain; charset=utf-8"})

    return Response(
        pdf_bytes,
        mimetype="application/pdf",
        headers={
            "Content-Disposition": (
                f'attachment; filename="newsletter-{facet}-{day}.pdf"'
            )
        },
    )


def _today() -> date:
    return datetime.now(get_owner_timezone()).date()


def _journal_has_any_observer_input() -> bool:
    """Has the journal seen at least one observer-stream day?"""
    chronicle_dir = _journal_root() / "chronicle"
    if not chronicle_dir.is_dir():
        return False
    for child in chronicle_dir.iterdir():
        if not child.is_dir():
            continue
        if len(child.name) == 8 and child.name.isdigit():
            return True
    return False
