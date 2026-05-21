# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import calendar
import functools
import json
import re
from collections import OrderedDict, defaultdict
from datetime import date, datetime
from pathlib import Path
from typing import Any

from flask import Blueprint, jsonify, redirect, render_template, url_for

from solstone.convey import state
from solstone.convey.reasons import (
    INVALID_DAY,
    INVALID_MONTH,
    INVALID_PATH,
    TIMELINE_MONTH_NOT_FOUND,
)
from solstone.convey.utils import error_response
from solstone.think.utils import DEFAULT_STREAM, iter_segments, segment_key

timeline_bp = Blueprint(
    "app:timeline",
    __name__,
    url_prefix="/app/timeline",
    static_folder="static",
    static_url_path="/static",
)

_master_cache: dict | None = None
_master_key: tuple[str, int] | None = None
_seg_cache: OrderedDict[tuple[str, str, str, str], dict] = OrderedDict()
_SEG_CACHE_MAX = 32
_MONTH_RE = re.compile(r"\d{6}")
_DAY_RE = re.compile(r"\d{8}")


def _journal_root() -> Path:
    return Path(state.journal_root)


def _load_master() -> dict:
    global _master_cache, _master_key
    journal_root = _journal_root()
    master_path = journal_root / "timeline.json"
    if not master_path.is_file():
        return {}
    key = (str(journal_root), master_path.stat().st_mtime_ns)
    if _master_cache is None or _master_key != key:
        _master_cache = json.loads(master_path.read_text(encoding="utf-8"))
        _master_key = key
    return _master_cache


def _recent_12_months(today: date | None = None) -> list[str]:
    if today is None:
        today = date.today()
    out: list[str] = []
    cur_y, cur_m = today.year, today.month
    for delta in range(11, -1, -1):
        y, m = cur_y, cur_m - delta
        while m <= 0:
            m += 12
            y -= 1
        out.append(f"{y:04d}{m:02d}")
    return out


def _days_in_month(ym: str) -> int:
    y = int(ym[:4])
    m = int(ym[4:6])
    nxt = date(y + 1, 1, 1) if m == 12 else date(y, m + 1, 1)
    return (nxt - date(y, m, 1)).days


def _first_weekday(ym: str) -> int:
    return date(int(ym[:4]), int(ym[4:6]), 1).weekday()


def _days_with_data(month_data: dict[str, Any]) -> list[str]:
    days = month_data.get("days") or {}
    if days:
        return sorted(days.keys())
    return sorted(month_data.get("days_with_data") or [])


def _build_index() -> dict[str, Any]:
    master = _load_master()
    months_data = master.get("months", {})
    yms = _recent_12_months()
    ym_set = set(yms)

    months_meta = []
    for ym in yms:
        m = months_data.get(ym, {})
        months_meta.append(
            {
                "ym": ym,
                "mlabel": f"{calendar.month_name[int(ym[4:6])]} {ym[:4]}",
                "year": int(ym[:4]),
                "month_num": int(ym[4:6]),
                "days_in_month": _days_in_month(ym),
                "first_weekday": _first_weekday(ym),
                "day_count": m.get("day_count", 0),
                "days_with_data": _days_with_data(m),
                "month_top": m.get("month_top", []),
                "month_rationale": m.get("month_rationale", ""),
            }
        )

    year_top = [
        entry for entry in master.get("year_top", []) if entry.get("month") in ym_set
    ]
    days_seen: list[str] = []
    for entry in months_meta:
        days_seen.extend(entry.get("days_with_data") or [])
    data_through = max(days_seen) if days_seen else None

    return {
        "now": datetime.now().isoformat(),
        "today": date.today().strftime("%Y%m%d"),
        "generated_at": master.get("generated_at"),
        "model": master.get("model"),
        "data_through": data_through,
        "months": months_meta,
        "year_top": year_top,
    }


def _build_month(ym: str) -> dict[str, Any] | None:
    master = _load_master()
    m = (master.get("months") or {}).get(ym)
    if m is None:
        return None
    return {
        "ym": ym,
        "generated_at": master.get("generated_at"),
        "model": master.get("model"),
        "month_top": m.get("month_top", []),
        "month_rationale": m.get("month_rationale", ""),
        "day_count": m.get("day_count", 0),
        "days_with_data": _days_with_data(m),
        "days": {
            d: {
                "day": d,
                "generated_at": v.get("generated_at"),
                "model": v.get("model"),
                "day_top": v.get("day_top", []),
                "day_rationale": v.get("day_rationale", ""),
            }
            for d, v in sorted((m.get("days") or {}).items())
        },
    }


def _segment_origin(day: str, stream: str, seg_key: str) -> str:
    if stream == DEFAULT_STREAM:
        return f"{day}/{seg_key}"
    return f"{day}/{stream}/{seg_key}"


def _build_day(day: str) -> dict[str, Any]:
    master = _load_master()
    ym = day[:6]
    day_data = ((master.get("months") or {}).get(ym, {}).get("days", {}) or {}).get(
        day, {}
    )

    buckets: dict[int, dict[int, list[dict[str, Any]]]] = defaultdict(
        lambda: defaultdict(list)
    )
    day_dir = _journal_root() / "chronicle" / day
    for stream, seg, seg_path in iter_segments(day_dir):
        hh = int(seg[0:2])
        mm = int(seg[2:4])
        bucket = (mm // 5) * 5
        buckets[hh][bucket].append(
            {
                "origin": _segment_origin(day, stream, seg),
                "has_audio": (seg_path / "audio.jsonl").is_file(),
                "has_screen": any(seg_path.glob("*screen.jsonl")),
            }
        )

    def rank(seg: dict[str, Any]) -> int:
        if seg["has_audio"] and seg["has_screen"]:
            return 0
        if seg["has_screen"]:
            return 1
        if seg["has_audio"]:
            return 2
        return 3

    hours_avail: dict[str, dict[str, Any]] = {}
    for hh in range(24):
        bucket_list = []
        hour_buckets = buckets.get(hh, {})
        for minute in range(0, 60, 5):
            segs = hour_buckets.get(minute, [])
            if segs:
                segs.sort(key=rank)
                best = segs[0]
                bucket_list.append(
                    {
                        "minute": minute,
                        "best_origin": best["origin"],
                        "has_audio": best["has_audio"],
                        "has_screen": best["has_screen"],
                        "segment_count": len(segs),
                    }
                )
            else:
                bucket_list.append(
                    {
                        "minute": minute,
                        "best_origin": None,
                        "has_audio": False,
                        "has_screen": False,
                        "segment_count": 0,
                    }
                )
        if any(bucket["best_origin"] for bucket in bucket_list):
            hours_avail[f"{hh:02d}"] = {"buckets": bucket_list}

    return {
        "day": day,
        "generated_at": day_data.get("generated_at"),
        "model": day_data.get("model"),
        "day_top": day_data.get("day_top", []),
        "day_rationale": day_data.get("day_rationale", ""),
        "hours": day_data.get("hours", {}),
        "hours_avail": hours_avail,
    }


def _segment_dir(day: str, stream: str, seg: str) -> Path:
    day_dir = _journal_root() / "chronicle" / day
    if stream == DEFAULT_STREAM:
        return day_dir / seg
    return day_dir / stream / seg


def _read_jsonl(path: Path) -> tuple[dict[str, Any], list[dict[str, Any]]] | None:
    try:
        lines = [
            line
            for line in path.read_text(encoding="utf-8").splitlines()
            if line.strip()
        ]
        if not lines:
            return None
        return json.loads(lines[0]), [json.loads(line) for line in lines[1:]]
    except (OSError, json.JSONDecodeError):
        return None


def _load_segment(day: str, stream: str, seg: str) -> dict[str, Any]:
    key = (str(_journal_root()), day, stream, seg)
    if key in _seg_cache:
        _seg_cache.move_to_end(key)
        return _seg_cache[key]

    seg_dir = _segment_dir(day, stream, seg)
    out: dict[str, Any] = {
        "day": day,
        "stream": "" if stream == DEFAULT_STREAM else stream,
        "segment": seg,
        "audio": None,
        "screen": None,
    }

    if seg_dir.is_dir():
        audio_file = seg_dir / "audio.jsonl"
        if audio_file.is_file():
            audio = _read_jsonl(audio_file)
            if audio:
                header, lines = audio
                out["audio"] = {"header": header, "lines": lines}

        screen_files = sorted(seg_dir.glob("*screen.jsonl"))
        if screen_files:
            screen = _read_jsonl(screen_files[0])
            if screen:
                header, frames = screen
                out["screen"] = {
                    "header": header,
                    "frames": frames,
                    "filename": screen_files[0].name,
                }
    else:
        out["error"] = f"segment dir not found: {seg_dir}"

    _seg_cache[key] = out
    if len(_seg_cache) > _SEG_CACHE_MAX:
        _seg_cache.popitem(last=False)
    return out


@timeline_bp.route("/")
def index():
    today = date.today().strftime("%Y%m%d")
    return redirect(url_for("app:timeline.timeline_value_view", value=today))


@timeline_bp.route("/year")
def timeline_year_view() -> str:
    return render_template(
        "app.html",
        view="year",
        title="timeline",
        initial={"view": "year", "day": None, "month": None},
    )


@timeline_bp.route("/<value>")
def timeline_value_view(value: str) -> tuple[str, int] | str:
    if _DAY_RE.fullmatch(value):
        return render_template(
            "app.html",
            view="day",
            title=f"timeline · {value}",
            initial={"view": "day", "day": value, "month": None},
        )
    if _MONTH_RE.fullmatch(value):
        return render_template(
            "app.html",
            view="month",
            title=f"timeline · {value}",
            initial={"view": "month", "day": None, "month": value},
        )
    return "", 404


def _day_seg_count_mtime(day_dir: Path) -> float:
    """Return latest mtime under a day directory, or 0.0 if missing."""
    try:
        max_mtime = day_dir.stat().st_mtime
    except FileNotFoundError:
        return 0.0

    try:
        for child in day_dir.rglob("*"):
            try:
                child_mtime = child.stat().st_mtime
            except FileNotFoundError:
                continue
            if child_mtime > max_mtime:
                max_mtime = child_mtime
    except FileNotFoundError:
        return max_mtime
    return max_mtime


@functools.lru_cache(maxsize=64)
def _stats_for_month(month: str, mtime_key: float) -> dict[str, int]:
    """Return segment counts by day for a month."""
    del mtime_key

    if not state.journal_root:
        return {}

    chronicle = Path(state.journal_root) / "chronicle"
    if not chronicle.exists():
        return {}

    out: dict[str, int] = {}
    for day_dir in chronicle.iterdir():
        if not day_dir.is_dir():
            continue
        day = day_dir.name
        if not _DAY_RE.fullmatch(day) or not day.startswith(month):
            continue
        seg_count = len(iter_segments(day_dir))
        if seg_count:
            out[day] = seg_count
    return out


@timeline_bp.route("/api/stats/<ym>")
def timeline_stats(ym: str) -> Any:
    if not _MONTH_RE.fullmatch(ym):
        return error_response(INVALID_MONTH, detail="Invalid month format")

    if not state.journal_root:
        return jsonify({})

    chronicle = Path(state.journal_root) / "chronicle"
    if not chronicle.exists():
        return jsonify({})

    matching = [
        day_dir
        for day_dir in chronicle.iterdir()
        if day_dir.is_dir()
        and _DAY_RE.fullmatch(day_dir.name)
        and day_dir.name.startswith(ym)
    ]
    if not matching:
        return jsonify({})

    mtime_key = max(_day_seg_count_mtime(day_dir) for day_dir in matching)
    return jsonify(_stats_for_month(ym, mtime_key))


@timeline_bp.route("/api/index")
def timeline_index() -> Any:
    return jsonify(_build_index())


@timeline_bp.route("/api/month/<ym>")
def timeline_month(ym: str) -> Any:
    if not _MONTH_RE.fullmatch(ym):
        return error_response(INVALID_MONTH, status=400, detail="Invalid month format")
    payload = _build_month(ym)
    if payload is None:
        return error_response(
            TIMELINE_MONTH_NOT_FOUND,
            status=404,
            detail=f"no data for {ym}",
        )
    return jsonify(payload)


@timeline_bp.route("/api/day/<day>")
def timeline_day(day: str) -> Any:
    if not _DAY_RE.fullmatch(day):
        return error_response(INVALID_DAY, status=400, detail="Invalid day format")
    return jsonify(_build_day(day))


@timeline_bp.route("/api/segment/<day>/<stream>/<seg>")
@timeline_bp.route("/api/segment/<day>/<seg>", defaults={"stream": DEFAULT_STREAM})
def timeline_segment(day: str, stream: str, seg: str) -> Any:
    if not _DAY_RE.fullmatch(day):
        return error_response(INVALID_PATH, status=400, detail="Invalid segment path")
    if segment_key(seg) != seg or (stream != DEFAULT_STREAM and "/" in stream):
        return error_response(INVALID_PATH, status=400, detail="Invalid segment path")
    return jsonify(_load_segment(day, stream, seg))
