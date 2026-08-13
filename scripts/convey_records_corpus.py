#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Capture the transcripts, chat, and search read surfaces into one corpus.

The populated journal is wholly synthetic. Search deliberately uses a
fail-closed in-process native-result fixture; no indexer helper, SQLite index,
or child process is needed to capture the Flask route contract.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import locale
import os
import shutil
import subprocess
import sys
from contextlib import contextmanager
from datetime import date, datetime, timezone
from pathlib import Path
from typing import Any, Iterator
from urllib.parse import parse_qs, urlsplit

os.environ["TZ"] = "UTC"
import time  # noqa: E402

if hasattr(time, "tzset"):
    time.tzset()

REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPTS = REPO_ROOT / "scripts"
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))
if str(SCRIPTS) not in sys.path:
    sys.path.insert(0, str(SCRIPTS))

import corpus_scrub  # noqa: E402
from records_corpus_seed import (  # noqa: E402
    D_CACHE,
    D_CORRUPT_CHAT,
    D_FULL,
    D_MIXED,
    D_RAW,
    build_corrupt_journal,
    build_established_journal,
    build_populated_journal,
    build_unestablished_journal,
)

CORPUS_PATH = REPO_ROOT / "core" / "fixtures" / "convey_records_corpus.json"
PLACEHOLDER_ROOT = "<JOURNAL_ROOT>"
PLACEHOLDER_TODAY = "<TODAY>"
PLACEHOLDER_TODAY_TIMESTAMP = "<TODAY_TIMESTAMP>"
RECORDED_HEADERS = (
    "Content-Type",
    "Content-Range",
    "Accept-Ranges",
    "Content-Length",
    "Content-Disposition",
    "Cache-Control",
    "Location",
)


def _capture_day() -> str:
    """Return the capture day, with an explicit test hook for --check stability."""
    override = os.environ.get("CONVEY_RECORDS_CORPUS_TODAY")
    if override:
        if len(override) != 8 or not override.isdigit():
            raise ValueError("CONVEY_RECORDS_CORPUS_TODAY must be YYYYMMDD")
        return override
    return datetime.now(timezone.utc).strftime("%Y%m%d")


def _rev() -> str:
    return subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def _clear_journal_cache() -> None:
    import solstone.think.utils as think_utils

    think_utils._journal_path_cache = None


def _set_capture_day(day: str) -> None:
    """Make chat's route-local ``date.today`` agree with the seeded today day."""
    import solstone.apps.chat.routes as chat_routes

    class CaptureDate(date):
        @classmethod
        def today(cls) -> date:
            return datetime.strptime(day, "%Y%m%d").date()

    chat_routes.date = CaptureDate


def _headers(response: Any) -> dict[str, str]:
    return {
        name: response.headers[name]
        for name in RECORDED_HEADERS
        if name in response.headers
    }


def _normalize_json(value: Any, *, root: Path, today_day: str) -> tuple[Any, list[str]]:
    """Normalize only concrete volatile field paths, never broad value shapes."""
    found: list[str] = []
    current_day_ends_date_nav = (
        isinstance(value, dict)
        and isinstance(value.get("coverage"), dict)
        and value["coverage"].get("end") == today_day
    )

    def today_timestamp_path(field_path: str) -> bool:
        return field_path in {
            "events[].ts",
            "events[].since_ts",
            "events[].queued_at",
            "events[].started_at",
        } or (
            field_path.startswith("sol_message_origins.")
            and field_path.rsplit(".", 1)[-1] in {"ts", "since_ts", "superseded_at"}
        )

    def visit(item: Any, path: str) -> Any:
        if isinstance(item, dict):
            return {
                (
                    PLACEHOLDER_TODAY
                    if (
                        isinstance(key, str)
                        and current_day_ends_date_nav
                        and path == "months"
                        and key == today_day[:6]
                    )
                    else key.replace(today_day, PLACEHOLDER_TODAY)
                    if isinstance(key, str)
                    else key
                ): visit(
                    value, f"{path}.{key}" if path else key
                )
                for key, value in item.items()
            }
        if isinstance(item, list):
            return [visit(value, f"{path}[]") for value in item]
        if isinstance(item, str):
            normalized = item.replace(str(root), PLACEHOLDER_ROOT)
            if normalized != item:
                found.append(path)
            # These are the complete today-derived fields emitted by the one
            # current-day chat-state case. Its locale-bearing display time is
            # intentionally not normalized; the locale pass measures it.
            if path in {
                "today_day",
                "events[].path",
                "events[].ts",
                "events[].since_ts",
                "events[].queued_at",
                "events[].started_at",
            }:
                if today_day in normalized or today_timestamp_path(path):
                    found.append(path)
                    return PLACEHOLDER_TODAY_TIMESTAMP if path != "today_day" and path != "events[].path" else normalized.replace(today_day, PLACEHOLDER_TODAY)
            return normalized.replace(today_day, PLACEHOLDER_TODAY) if today_day in normalized else normalized
        if isinstance(item, (int, float)) and today_timestamp_path(path):
            found.append(path)
            return PLACEHOLDER_TODAY_TIMESTAMP
        return item

    return visit(value, ""), sorted(set(found))


def _case_path(path: str, today_day: str) -> str:
    return path.replace(today_day, PLACEHOLDER_TODAY)


def _record(
    client: Any,
    *,
    app: str,
    phase: str,
    method: str,
    path: str,
    headers: dict[str, str],
    why: str,
    root: Path,
    today_day: str,
    body: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Drive one route and preserve its JSON or compact raw response exactly."""
    try:
        response = client.open(path, method=method, headers=headers or None, json=body)
        raw_body = response.get_data()
        status = response.status_code
        response_headers = _headers(response)
        response_headers = {
            key: value.replace(str(root), PLACEHOLDER_ROOT).replace(today_day, PLACEHOLDER_TODAY)
            for key, value in response_headers.items()
        }
        content_type = response.headers.get("Content-Type", "")
    except ValueError as error:
        # Flask may propagate the known unregistered-MIME exception under a
        # future test-client configuration. Preserve the reference failure
        # without letting the generator process terminate.
        status = 500
        raw_body = str(error).encode("utf-8")
        response_headers = {"Content-Type": "text/plain; charset=utf-8"}
        content_type = "text/plain"

    if method == "DELETE" or (method == "POST" and "/reprocess" in path):
        if 200 <= status < 300:
            raise RuntimeError(f"forbidden mutating success: {method} {path} -> {status}")

    case: dict[str, Any] = {
        "app": app,
        "phase": phase,
        "method": method,
        "path": _case_path(path, today_day),
        "request_headers": headers,
        "why": why,
        "status": status,
        "response_headers": response_headers,
    }
    if "json" in content_type:
        normalized, normalized_fields = _normalize_json(
            json.loads(raw_body), root=root, today_day=today_day
        )
        case["normalized_fields"] = normalized_fields
        case["body_sha256"] = hashlib.sha256(
            json.dumps(normalized, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
        ).hexdigest()
        case["body_sha256_basis"] = "normalized-json"
        case["json"] = normalized
    else:
        normalized_raw = raw_body.replace(str(root).encode(), PLACEHOLDER_ROOT.encode())
        normalized_raw = normalized_raw.replace(today_day.encode(), PLACEHOLDER_TODAY.encode())
        case["normalized_fields"] = ["raw-body"] if normalized_raw != raw_body else []
        case["body_sha256"] = hashlib.sha256(normalized_raw).hexdigest()
        case["body_sha256_basis"] = "raw-body"
        case["body_bytes"] = base64.b64encode(normalized_raw).decode("ascii")
    return case


def _hit(
    name: str,
    text: str,
    day: str,
    facet: str,
    agent: str,
    stream: str,
    bucket: str,
    idx: int,
) -> dict[str, Any]:
    return {
        "id": f"chronicle/{day}/{stream}/{name}.md:{idx}",
        "text": text,
        "metadata": {
            "day": day,
            "facet": facet,
            "agent": agent,
            "stream": stream,
            "path": f"{day}/{stream}/{name}.md",
            "idx": idx,
        },
        "score": -float(idx + 1),
        "_bucket": bucket,
    }


class CannedNativeSearch:
    """A complete-call-key native search fixture that refuses unknown calls."""

    def __init__(self, http_paths: list[str], *, empty: bool = False) -> None:
        long_text = " ".join(["needle"] + [f"word{index}" for index in range(1, 55)])
        self.hits = [] if empty else [
            *[_hit(f"needle-{index}", long_text if index == 0 else f"needle result {index}", D_FULL, "work", "flow", "field", "morning", index) for index in range(6)],
            _hit("needle-muted", "needle muted facet result", D_MIXED, "muted", "screen", "field", "afternoon", 6),
            _hit("apostrophe", "O'Brien and dogs & <friends>", D_FULL, "work", "flow", "field", "morning", 7),
            _hit("locale", "locale-probe captures a deterministic day", D_FULL, "work", "flow", "field", "morning", 8),
            _hit("meeting", "yesterday's meeting needle", D_CACHE, "work", "meetings", "field", "evening", 9),
            _hit("nebula", "nebula appears after relaxed matching", D_MIXED, "work", "flow", "field", "night", 10),
        ]
        self.responses: dict[tuple[Any, ...], dict[str, Any]] = {}
        for path in http_paths:
            self._precompute_path(path)

    @staticmethod
    def _key(query: str, kwargs: dict[str, Any]) -> tuple[Any, ...]:
        return (
            query,
            kwargs["limit"],
            kwargs["offset"],
            kwargs.get("day"),
            kwargs.get("day_from"),
            kwargs.get("day_to"),
            kwargs.get("facet"),
            kwargs.get("agent"),
            kwargs.get("stream"),
            kwargs.get("time_bucket"),
            kwargs.get("relax", False),
            kwargs.get("rerank", False),
            kwargs.get("include_counts", kwargs.get("include_total", True)),
        )

    def _matching(self, query: str, kwargs: dict[str, Any]) -> tuple[list[dict[str, Any]], bool, str | None]:
        if query == "'":
            return [], False, "not_tokenizable"
        if query == "O'Brien AND dogs":
            selected = [hit for hit in self.hits if hit["metadata"]["path"].endswith("apostrophe.md")]
        elif query == "what nebula":
            selected = [hit for hit in self.hits if hit["metadata"]["path"].endswith("nebula.md")]
        elif query == "locale-probe":
            selected = [hit for hit in self.hits if hit["metadata"]["path"].endswith("locale.md")]
        elif query == "yesterday's meeting":
            selected = [hit for hit in self.hits if hit["metadata"]["path"].endswith("meeting.md")]
        elif query == "needle":
            selected = [hit for hit in self.hits if "needle" in hit["text"]]
        elif not query:
            selected = list(self.hits)
        else:
            selected = []
        for field in ("day", "facet", "agent", "stream"):
            value = kwargs.get(field)
            if value is not None:
                selected = [hit for hit in selected if hit["metadata"][field] == value]
        bucket = kwargs.get("time_bucket")
        if bucket is not None:
            selected = [hit for hit in selected if hit["_bucket"] == bucket]
        return selected, query == "what nebula", None

    def _response(self, query: str, kwargs: dict[str, Any]) -> dict[str, Any]:
        selected, relaxed, reason = self._matching(query, kwargs)
        include_counts = kwargs.get("include_counts", kwargs.get("include_total", True))
        if reason is not None:
            return {
                "results": [], "order": "relevance", "relaxed": False,
                "cleaned_query": query, "reason": reason,
            }
        response: dict[str, Any] = {
            "results": [
                {key: value for key, value in hit.items() if key != "_bucket"}
                for hit in selected[kwargs["offset"] : kwargs["offset"] + kwargs["limit"]]
            ],
            "order": "relevance",
            "relaxed": relaxed,
            "cleaned_query": query,
        }
        if include_counts:
            def counts(field: str) -> dict[str, int]:
                values = sorted({str(hit["metadata"][field]) for hit in selected})
                return {value: sum(hit["metadata"][field] == value for hit in selected) for value in values}

            response["total"] = len(selected)
            response["counts"] = {
                "total": len(selected),
                "facets": counts("facet"),
                "agents": counts("agent"),
                "days": counts("day"),
                "streams": counts("stream"),
                "relaxed": relaxed,
            }
        return response

    def _allow(self, query: str, **kwargs: Any) -> None:
        key = self._key(query, kwargs)
        self.responses[key] = self._response(query, kwargs)

    def _precompute_path(self, path: str) -> None:
        split = urlsplit(path)
        query_values = parse_qs(split.query, keep_blank_values=True)
        query = query_values.get("q", [""])[0].strip()
        if split.path.endswith("/api/search"):
            day_from = query_values.get("day_from", [""])[0]
            day_to = query_values.get("day_to", [""])[0]
            if day_from == "20260230" or (day_from and day_to and day_from > day_to):
                return
            if day_from in {"00000000", "99999999"}:
                day_from = ""
            if day_to in {"00000000", "99999999"}:
                day_to = ""
            limit = int(query_values.get("limit", ["5"])[0])
            filters = {
                field: (query_values.get(field, [""])[0].strip() or None)
                for field in ("facet", "agent", "stream", "time_bucket")
            }
            common = {"limit": 0, "offset": 0, "day": None, "day_from": None, "day_to": None, **filters, "relax": True, "include_counts": True}
            self._allow(query, **{**common, "facet": None, "agent": None})
            self._allow(query, **common)
            selected, _relaxed, reason = self._matching(query, {**common, "facet": filters["facet"], "agent": filters["agent"]})
            if reason is not None:
                return
            days = sorted({hit["metadata"]["day"] for hit in selected}, reverse=True)
            days = [day for day in days if (not day_from or day >= day_from) and (not day_to or day <= day_to)]
            for day in days[:20]:
                self._allow(query, limit=limit, offset=0, day=day, day_from=None, day_to=None, **filters, relax=True, include_counts=False)
            return
        if split.path.endswith("/api/day_results") and query_values.get("day", [""])[0].strip():
            day = query_values["day"][0].strip()
            limit = int(query_values.get("limit", ["20"])[0])
            offset = int(query_values.get("offset", ["0"])[0])
            filters = {field: (query_values.get(field, [""])[0].strip() or None) for field in ("facet", "agent", "stream")}
            common = {"day": day, "day_from": None, "day_to": None, **filters, "time_bucket": None, "relax": True}
            self._allow(query, limit=0, offset=0, **common, include_counts=True)
            self._allow(query, limit=limit, offset=offset, **common, include_counts=False)

    def search(self, query: str, _journal: str, **kwargs: Any) -> dict[str, Any]:
        key = self._key(query, kwargs)
        try:
            return json.loads(json.dumps(self.responses[key]))
        except KeyError as error:
            raise RuntimeError(f"unexpected canned native search call: {key!r}") from error


@contextmanager
def _install_search_dispatcher(paths: list[str], *, empty: bool = False) -> Iterator[None]:
    import solstone.think.indexer.journal as journal_index

    canned = CannedNativeSearch(paths, empty=empty)
    original_search = journal_index.run_native_indexer_search
    original_agents = journal_index.run_native_indexer_agents
    original_coverage = journal_index.run_native_indexer_coverage
    journal_index.run_native_indexer_search = canned.search
    journal_index.run_native_indexer_agents = lambda _journal: ["flow", "screen"]
    journal_index.run_native_indexer_coverage = lambda _journal: {
        "state": "available", "start": D_CACHE, "end": D_RAW
    }
    try:
        yield
    finally:
        journal_index.run_native_indexer_search = original_search
        journal_index.run_native_indexer_agents = original_agents
        journal_index.run_native_indexer_coverage = original_coverage


def _probe(app: str, method: str, path: str, why: str, *, body: dict[str, Any] | None = None, headers: dict[str, str] | None = None) -> dict[str, Any]:
    return {"app": app, "method": method, "path": path, "why": why, "body": body, "headers": headers or {}}


def _populated_probes(today: str) -> list[dict[str, Any]]:
    probes: list[dict[str, Any]] = []
    add = probes.append
    for path, why in [
        ("/app/transcripts/", "redirect to newest segmented day"),
        (f"/app/transcripts/{D_FULL}", "valid day shell"),
        ("/app/transcripts/notaday", "invalid-day shell refusal"),
        ("/app/transcripts/api/index", "multi-month date navigation"),
        (f"/app/transcripts/api/ranges/{D_FULL}", "all range source types"),
        (f"/app/transcripts/api/segments/{D_FULL}", "stream-attached segments"),
        (f"/app/transcripts/api/day/{D_FULL}", "combined ranges and segments"),
    ]:
        add(_probe("transcripts", "GET", path, why))
    source_variants = [
        ("", "whole-day all sources", "transcripts=1&percepts=1&agents=1"),
        ("", "whole-day agents", "agents=1"),
        ("", "whole-day transcript plus percept", "transcripts=1&percepts=1"),
        ("", "whole-day transcript plus agent", "transcripts=1&agents=1"),
        ("segment=090000_300&stream=field&", "segment all sources", "transcripts=1&percepts=1&agents=1"),
        ("segment=090000_300&stream=field&", "segment agents", "agents=1"),
        ("segment=090000_300&stream=field&", "segment transcript plus percept", "transcripts=1&percepts=1"),
        ("segment=090000_300&stream=field&", "segment transcript plus agent", "transcripts=1&agents=1"),
        ("segments=090000_300,100000_120&stream=field&", "span all sources", "transcripts=1&percepts=1&agents=1"),
        ("segments=090000_300,100000_120&stream=field&", "span agents", "agents=1"),
        ("segments=090000_300,100000_120&stream=field&", "span transcript plus percept", "transcripts=1&percepts=1"),
        ("segments=090000_300,100000_120&stream=field&", "span transcript plus agent", "transcripts=1&agents=1"),
        ("start=090000&end=101000&", "time range all sources", "transcripts=1&percepts=1&agents=1"),
        ("start=090000&end=101000&", "time range agents", "agents=1"),
        ("start=090000&end=101000&", "time range transcript plus percept", "transcripts=1&percepts=1"),
        ("start=090000&end=101000&", "time range transcript plus agent", "transcripts=1&agents=1"),
    ]
    for prefix, why, flags in source_variants:
        add(_probe("transcripts", "GET", f"/app/transcripts/api/read/{D_FULL}?{prefix}{flags}", why))
    add(_probe("transcripts", "GET", f"/app/transcripts/api/read/{D_FULL}?segments=090000_300,not-a-key&stream=field&transcripts=1", "malformed span refusal"))
    for month, why in [("202607", "mixed fresh-cache and raw-scan month"), ("202608", "no-cache raw-scan month"), ("202609", "sparse no-cache month"), ("nope", "invalid month refusal")]:
        add(_probe("transcripts", "GET", f"/app/transcripts/api/stats/{month}", why))
    media = f"/app/transcripts/api/serve_file/{D_FULL}/field/090000_300"
    add(_probe("transcripts", "GET", f"{media}/mic_audio.flac", "registered media"))
    add(_probe("transcripts", "GET", f"{media}/mic_audio.flac", "registered ranged media", headers={"Range": "bytes=0-15"}))
    for name, why in [("zero.flac", "zero-byte media"), ("mic_audio.xyz", "unregistered extension native deviation"), ("absent.exe", "missing file refusal")]:
        add(_probe("transcripts", "GET", f"{media}/{name}", why))
    add(_probe("transcripts", "GET", f"/app/transcripts/api/serve_file/{D_FULL}/../config/journal.json", "traversal refusal"))
    for stream, segment, why in [
        ("field", "090000_300", "analyzed segment"), ("field", "100000_120", "analyzing rendering only"),
        ("field", "110000_120", "failed rendering only"), ("field", "120000_120", "purged rendering"), ("notes", "130000_60", "markdown-only segment"),
    ]:
        add(_probe("transcripts", "GET", f"/app/transcripts/api/segment/{D_FULL}/{stream}/{segment}", why))
    for path, body, why in [
        (f"/app/transcripts/api/segment/{D_FULL}/field/090000_300/reprocess", {"modality": "audio"}, "analyzed refusal"),
        (f"/app/transcripts/api/segment/{D_FULL}/field/120000_120/reprocess", {"modality": "audio"}, "purged refusal"),
        (f"/app/transcripts/api/segment/{D_FULL}/field/090000_300/reprocess", {"modality": "browser"}, "invalid modality refusal"),
        ("/app/transcripts/api/segment/notaday/field/090000_300/reprocess", {"modality": "audio"}, "invalid day refusal"),
        (f"/app/transcripts/api/segment/{D_FULL}/field/not-a-key/reprocess", {"modality": "audio"}, "invalid segment refusal"),
        (f"/app/transcripts/api/segment/{D_FULL}/BAD/090000_300/reprocess", {"modality": "audio"}, "invalid stream refusal"),
    ]:
        add(_probe("transcripts", "POST", path, why, body=body))
    for path, why in [
        ("/app/transcripts/api/segment/notaday/field/090000_300", "invalid delete day"),
        (f"/app/transcripts/api/segment/{D_FULL}/field/not-a-key", "invalid delete segment"),
        (f"/app/transcripts/api/segment/{D_FULL}/BAD/090000_300", "invalid delete stream"),
        (f"/app/transcripts/api/segment/{D_FULL}/field/235959_1", "nonexistent delete segment"),
    ]:
        add(_probe("transcripts", "DELETE", path, why))
    for pending, why in [("not-a-pending-id", "malformed cancellation"), ("0" * 32, "unknown cancellation")]:
        add(_probe("transcripts", "POST", f"/app/transcripts/api/cancel-delete/{pending}", why))
    for path, why in [
        ("/app/chat/", "today redirect"), (f"/app/chat/{today}", "today shell"), ("/app/chat/notaday", "invalid shell"),
        (f"/app/chat/api/state?day={today}", "today state and synthesized origins"), (f"/app/chat/api/state?day={D_FULL}", "historical state"),
        (f"/app/chat/api/state?day={D_RAW}", "empty chat state"), (f"/app/chat/api/state?day={D_CORRUPT_CHAT}", "corrupt chat swallowed"),
        ("/app/chat/api/state?day=notaday", "invalid state day"), ("/app/chat/api/index", "chat date navigation"),
        ("/app/chat/api/stats/202607", "chat populated historical month"), ("/app/chat/api/stats/202613", "impossible month"), ("/app/chat/api/stats/nope", "invalid month"),
    ]:
        add(_probe("chat", "GET", path, why))
    for path, why in [
        ("/app/search/api/search?q=O%27Brien%20AND%20dogs", "apostrophe and operator"), ("/app/search/api/search?q=", "empty browse"),
        ("/app/search/api/search?q=needle", "baseline seeded search"),
        ("/app/search/api/search?q=needle&limit=1&offset=1", "pagination"), ("/app/search/api/search?q=needle&facet=work", "facet filter"),
        ("/app/search/api/search?q=needle&agent=flow", "agent filter"), ("/app/search/api/search?q=needle&stream=field", "stream filter"),
        ("/app/search/api/search?q=needle&time_bucket=morning", "time bucket filter"),
        ("/app/search/api/search?q=locale-probe&day_from=20260731&day_to=20260731", "locale date and day range"),
        ("/app/search/api/search?q=yesterday%27s%20meeting&day_from=00000000&day_to=99999999", "sentinel day bounds"),
        ("/app/search/api/search?q=needle&day_from=20260230", "invalid calendar bound"), ("/app/search/api/search?q=needle&day_from=20260802&day_to=20260801", "reversed bounds"),
        ("/app/search/api/search?q=%27", "apostrophe-only query"), ("/app/search/api/search?q=what%20nebula", "relaxed matching"),
        (f"/app/search/api/agents?day={D_FULL}", "daily and segment outputs"), (f"/app/search/api/agents?day={D_FULL}&segment=090000_300", "segment outputs"),
        (f"/app/search/api/agents?day={D_RAW}", "zero outputs"), (f"/app/search/api/read?path={D_FULL}/talents/flow.md", "safe direct read"),
        (f"/app/search/api/read?agent=flow&day={D_FULL}", "daily talent read"), (f"/app/search/api/read?agent=flow&day={D_FULL}&segment=090000_300", "segment talent read"),
        ("/app/search/api/read?path=../config/journal.json", "path containment refusal"), ("/app/search/api/read?path=entity_search:ada", "entity pseudo-path refusal"),
        (f"/app/search/api/read?path={D_FULL}/talents/flow.md:3", "search result suffix refusal"), (f"/app/search/api/read?agent=missing&day={D_FULL}", "missing talent refusal"),
        (f"/app/search/api/day_results?q=needle&day={D_FULL}&limit=1&offset=1&facet=work&agent=flow&stream=field", "reference-only pagination"),
        ("/app/search/api/day_results?q=needle", "empty day fast path"),
    ]:
        add(_probe("search", "GET", path, why))
    return probes


def _capture_phase(
    phase: str,
    root: Path,
    probes: list[dict[str, Any]],
    *,
    today: str,
    dispatcher_empty: bool = False,
) -> dict[str, list[dict[str, Any]]]:
    from solstone.convey import create_app

    os.environ["SOLSTONE_JOURNAL"] = str(root)
    os.environ["SOLSTONE_DISABLE_CONVEY_SIDE_RUNTIMES"] = "1"
    _clear_journal_cache()
    _set_capture_day(today)
    app = create_app(str(root))
    client = app.test_client()
    paths = [str(probe["path"]) for probe in probes if probe["app"] == "search"]
    out: dict[str, list[dict[str, Any]]] = {"transcripts": [], "chat": [], "search": []}
    with _install_search_dispatcher(paths, empty=dispatcher_empty) if paths else _null_context():
        for probe in probes:
            out[str(probe["app"])].append(
                _record(
                    client,
                    app=str(probe["app"]), phase=phase, method=str(probe["method"]), path=str(probe["path"]),
                    headers=dict(probe["headers"]), why=str(probe["why"]), root=root, today_day=today, body=probe["body"],
                )
            )
    return out


@contextmanager
def _null_context() -> Iterator[None]:
    yield


def _deep_diff(before: Any, after: Any, path: str = "") -> list[str]:
    if type(before) is not type(after):
        return [path]
    if isinstance(before, dict):
        fields: list[str] = []
        for key in sorted(set(before) | set(after)):
            child = f"{path}.{key}" if path else key
            if key not in before or key not in after:
                fields.append(child)
            else:
                fields.extend(_deep_diff(before[key], after[key], child))
        return fields
    if isinstance(before, list):
        fields = []
        for index, (left, right) in enumerate(zip(before, after)):
            fields.extend(_deep_diff(left, right, f"{path}[{index}]"))
        if len(before) != len(after):
            fields.append(path)
        return fields
    return [] if before == after else [path]


def _assert_no_host_paths(rendered: str, today: str) -> None:
    forbidden = ("/tmp", "/var/tmp", "/var/folders")
    for value in forbidden:
        if value in rendered:
            raise RuntimeError(f"records corpus leaked temporary path fragment: {value}")
    for value in (today, datetime.strptime(today, "%Y%m%d").strftime("%Y-%m-%d")):
        if value in rendered:
            position = rendered.index(value)
            raise RuntimeError(f"records corpus leaked the real capture day near {rendered[position - 80:position + 80]!r}")


def _assert_egress_helper_sweep() -> None:
    result = subprocess.run(
        ["rg", "-n", r"socket\.(create_connection|getaddrinfo)|\.connect\(", "solstone/think/link"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0 or not result.stdout.strip():
        raise RuntimeError("egress helper positive control did not find a connection primitive")


def build_corpus() -> dict[str, Any]:
    today = _capture_day()
    old_locale = locale.setlocale(locale.LC_ALL)
    locale.setlocale(locale.LC_ALL, "C")
    corpus_scrub.forbid_non_loopback_egress()
    corpus_scrub.assert_egress_guard_can_see("records corpus")
    guard_positive_attempts = corpus_scrub.egress_attempts()
    corpus_scrub.assert_guard_can_see("records corpus")
    _assert_egress_helper_sweep()

    common = [_probe("transcripts", "GET", f"/app/transcripts/{D_FULL}", "journal-state gate")]
    roots = {
        "unestablished": build_unestablished_journal(),
        "established": build_established_journal(),
        "corrupt": build_corrupt_journal(),
    }
    populated_root, manifest = build_populated_journal(today)
    populated_probes = _populated_probes(today)
    phases = {
        "unestablished": _capture_phase("unestablished", roots["unestablished"], common, today=today),
        "established": _capture_phase(
            "established", roots["established"], common + [
                _probe("transcripts", "GET", "/app/transcripts/api/index", "established byte-diff read"),
                _probe("chat", "GET", "/app/chat/api/index", "established byte-diff read"),
                _probe("search", "GET", "/app/search/api/search?q=needle", "established byte-diff read"),
            ], today=today, dispatcher_empty=True,
        ),
        "corrupt": _capture_phase("corrupt", roots["corrupt"], common, today=today),
        "populated": _capture_phase("populated", populated_root, populated_probes, today=today),
    }
    for app, path in (("transcripts", "/app/transcripts/api/index"), ("chat", "/app/chat/api/index"), ("search", "/app/search/api/search?q=needle")):
        established = next(case for case in phases["established"][app] if case["path"] == path)
        populated = next(case for case in phases["populated"][app] if case["path"] == path)
        if established["body_sha256"] == populated["body_sha256"]:
            raise RuntimeError(f"established and populated bodies unexpectedly match: {app} {path}")

    locale_paths = [
        "/app/search/api/search?q=locale-probe&day_from=20260731&day_to=20260731",
        f"/app/chat/api/state?day={today}",
    ]
    old_locale_env = os.environ.get("LC_ALL")
    os.environ["LC_ALL"] = "de_DE.utf8"
    try:
        locale.setlocale(locale.LC_ALL, "de_DE.utf8")
        locale_root, _ = build_populated_journal(today)
        locale_probes = [probe for probe in populated_probes if probe["path"] in locale_paths]
        locale_cases = _capture_phase("populated", locale_root, locale_probes, today=today)
    finally:
        locale.setlocale(locale.LC_ALL, old_locale)
        if old_locale_env is None:
            os.environ.pop("LC_ALL", None)
        else:
            os.environ["LC_ALL"] = old_locale_env
    baseline_locale = {
        (case["app"], case["path"]): case["json"]
        for cases in phases["populated"].values() for case in cases
        if case["path"].replace(PLACEHOLDER_TODAY, today) in locale_paths and "json" in case
    }
    perturbed_locale = {
        (case["app"], case["path"]): case["json"]
        for cases in locale_cases.values() for case in cases if "json" in case
    }
    locale_fields: list[str] = []
    for key, before in baseline_locale.items():
        locale_fields.extend(f"{key[0]}.{field}" for field in _deep_diff(before, perturbed_locale[key]))
    required_locale_paths = {"search.days[0].date", "chat.sol_message_origins.2.time"}
    missing_locale = required_locale_paths - set(locale_fields)
    if missing_locale:
        raise RuntimeError("expected locale fields did not move: " + ", ".join(sorted(missing_locale)))

    corpus = {
        "schema": "solstone-convey-records-corpus-v1",
        "generator": "scripts/convey_records_corpus.py",
        "rev": _rev(),
        "capture_environment": {
            "tz": "UTC",
            "python_version": sys.version,
            "journal_on_path": shutil.which("journal") is not None,
            "supervisor_marker_state": "not applicable — is_supervisor_up() has no capturable behavior under the no-successful-delete rule, record this as a note not a probed value",
        },
        "placeholders": {"journal_root": PLACEHOLDER_ROOT, "today": PLACEHOLDER_TODAY},
        "phases": phases,
        "native_deviations": [
            {"path_or_field": "/app/search/api/day_results", "reference": "200 with a day's paginated results.", "native": "absent.", "why": "it exists only to serve its own now-dropped page and has no other caller (no authority.toml row, no CLI inventory entry, no sibling app)."},
            {"path_or_field": "/app/search/api/search response fields: agent_icon_svg, icon_svg, agent_icon, facet_color, facet_emoji, day_grid, showing_days, has_more", "reference": "includes all eight fields.", "native": "drops all eight; facets[], talents[], total, total_days, relaxed stay.", "why": "these are inline SVG/page chrome consumed only by search's own page; a CLI/talent consumer drills down using the retained counts."},
            {"path_or_field": "/app/transcripts/api/serve_file/{day}/{rel_path} — present file, unregistered extension", "reference": "routes.py raises an uncaught ValueError; Flask renders an HTML 500.", "native": "a typed refusal.", "why": "an unhandled exception is not a contract worth reproducing; the sibling speakers corpus already carries this same deviation for its own media route."},
        ],
        "host_dependent": {"baseline_locale": "C", "perturbed_locale": "de_DE.utf8", "fields": sorted(locale_fields)},
        # The manifest's physical day count includes the capture-day chat
        # stream.  Publish the three fixed calendar months instead, so a
        # capture-day month cannot perturb the deterministic corpus.
        "seeder_manifest": {**manifest, "today_day": PLACEHOLDER_TODAY, "month_count": 3},
    }
    rendered = json.dumps(corpus, indent=2, sort_keys=True, ensure_ascii=False) + "\n"
    corpus_scrub.assert_publishable(rendered, label="records corpus")
    _assert_no_host_paths(rendered, today)
    corpus_scrub.assert_no_egress_attempted("records corpus", ignore=guard_positive_attempts)
    return corpus


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail if the committed corpus differs")
    parser.add_argument("--output", type=Path, help="write to this path instead of the committed fixture")
    args = parser.parse_args()
    corpus = build_corpus()
    rendered = json.dumps(corpus, indent=2, sort_keys=True, ensure_ascii=False) + "\n"
    target = args.output or CORPUS_PATH
    if args.check:
        if not CORPUS_PATH.exists() or CORPUS_PATH.read_text(encoding="utf-8") != rendered:
            print(f"records corpus is stale: {CORPUS_PATH}\nregenerate with: python scripts/convey_records_corpus.py", file=sys.stderr)
            return 1
        print(f"records corpus is current: {CORPUS_PATH}")
        return 0
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(rendered, encoding="utf-8")
    count = sum(len(cases) for phase in corpus["phases"].values() for cases in phase.values())
    print(f"wrote {target} ({count} cases)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
