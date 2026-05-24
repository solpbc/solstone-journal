# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Steward health synthesis helpers and repair recipes."""

from __future__ import annotations

import dataclasses
import fcntl
import json
import logging
import os
import re
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

from solstone.observe.hear import format_audio
from solstone.observe.screen import format_screen
from solstone.observe.utils import AUDIO_EXTENSIONS, VIDEO_EXTENSIONS
from solstone.think.data_state import DataState, derive_modality_state
from solstone.think.identity import (
    STEWARD_SECTION_ATTENTION,
    STEWARD_SECTION_AUTO_REPAIRS,
    STEWARD_SECTION_STATUS,
    STEWARD_SECTION_TRENDS,
    write_identity,
)
from solstone.think.pipeline_health import summarize_pipeline_day
from solstone.think.utils import (
    day_path,
    get_journal,
    iter_segments,
    now_ms,
    read_service_port,
)

logger = logging.getLogger(__name__)

STALE_PENDING_AGE_MS = 6 * 60 * 60 * 1000
STALE_PENDING_RECIPE = "stale_pending_segment_reprocess"
_SEVEN_DAYS_MS = 7 * 86_400_000
_GENERATED_AT_RE = re.compile(
    r"^<!-- generated_at: (\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z) -->$"
)
_SECTION_RE = re.compile(r"^## .+$")


@dataclass(frozen=True)
class RecipeOutcome:
    recipe: str
    target: str
    outcome: str
    detail: str | None
    ts: int


@dataclass(frozen=True)
class StalePendingTarget:
    day: str
    stream: str
    segment_key: str
    modality: str
    segment_dir: Path

    @property
    def target(self) -> str:
        return f"{self.day}/{self.stream}/{self.segment_key}:{self.modality}"


def _utc_now_iso_z() -> str:
    return (
        datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")
    )


def _previous_day(day: str) -> str:
    return (datetime.strptime(day, "%Y%m%d") - timedelta(days=1)).strftime("%Y%m%d")


def _journal_path(journal: Path | None = None) -> Path:
    return Path(get_journal()) if journal is None else journal


def _steward_log_path(journal: Path | None = None) -> Path:
    return _journal_path(journal) / "health" / "steward.log"


def append_steward_event(event: str, **fields: Any) -> None:
    """Append one event row to journal/health/steward.log."""
    path = _steward_log_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    record = {"event": event, "ts": int(fields.pop("ts", now_ms())), **fields}
    line = json.dumps(record, separators=(",", ":"), sort_keys=True) + "\n"
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    try:
        os.write(fd, line.encode("utf-8"))
    finally:
        os.close(fd)


def load_steward_log() -> list[dict]:
    """Return all valid steward log rows."""
    path = _steward_log_path()
    rows: list[dict] = []
    try:
        with path.open(encoding="utf-8") as handle:
            for raw_line in handle:
                line = raw_line.strip()
                if not line:
                    continue
                try:
                    row = json.loads(line)
                except json.JSONDecodeError:
                    logger.debug("steward: malformed log line in %s", path)
                    continue
                if isinstance(row, dict):
                    rows.append(row)
    except FileNotFoundError:
        return []
    return rows


def _load_jsonl(path: Path) -> list[dict]:
    rows: list[dict] = []
    with path.open(encoding="utf-8") as handle:
        for raw_line in handle:
            line = raw_line.strip()
            if line:
                rows.append(json.loads(line))
    return rows


def _raw_files(segment_dir: Path, modality: str) -> list[Path]:
    extensions = AUDIO_EXTENSIONS if modality == "audio" else VIDEO_EXTENSIONS
    return [
        path
        for path in segment_dir.iterdir()
        if path.is_file() and path.suffix.lower() in extensions
    ]


def _modality_signals(segment_dir: Path, modality: str) -> dict[str, bool | str]:
    raw_files = _raw_files(segment_dir, modality)
    has_raw_reference = False
    has_raw_file = False
    has_jsonl = False
    has_chunks = False
    warning = False
    patterns = ("*audio.jsonl",) if modality == "audio" else ("*screen.jsonl",)

    for pattern in patterns:
        for jsonl_path in sorted(segment_dir.glob(pattern)):
            if not jsonl_path.is_file():
                continue
            has_jsonl = True
            try:
                entries = _load_jsonl(jsonl_path)
                if modality == "audio":
                    formatted_chunks, _meta = format_audio(
                        entries, {"file_path": str(jsonl_path)}
                    )
                    for entry in entries:
                        if "start" not in entry and "raw" in entry:
                            raw_name = entry["raw"]
                            if isinstance(raw_name, str) and raw_name.endswith(
                                AUDIO_EXTENSIONS
                            ):
                                has_raw_reference = True
                                has_raw_file = (segment_dir / raw_name).is_file()
                            break
                else:
                    formatted_chunks, _meta = format_screen(
                        entries, {"file_path": str(jsonl_path)}
                    )
                    for entry in entries:
                        if "frame_id" not in entry and "raw" in entry:
                            raw_name = entry["raw"]
                            if isinstance(raw_name, str) and raw_name.endswith(
                                VIDEO_EXTENSIONS
                            ):
                                has_raw_reference = True
                                has_raw_file = (segment_dir / raw_name).is_file()
                            break
                has_chunks = has_chunks or bool(formatted_chunks)
            except Exception:
                warning = True

    media_purged = has_raw_reference and not has_raw_file
    if has_chunks:
        state = derive_modality_state(
            segment_dir,
            modality,
            has_chunks=True,
            has_jsonl=has_jsonl,
            has_raw=bool(raw_files),
        )
    elif media_purged:
        state = DataState.PURGED.value
    else:
        state = derive_modality_state(
            segment_dir,
            modality,
            has_chunks=False,
            has_jsonl=has_jsonl,
            has_raw=bool(raw_files),
        )
        if warning and state == DataState.PENDING.value:
            state = DataState.FAILED.value

    return {
        "state": state,
        "has_raw": bool(raw_files),
        "has_jsonl": has_jsonl,
        "has_chunks": has_chunks,
        "media_purged": media_purged,
    }


def _oldest_raw_mtime_ms(segment_dir: Path, modality: str) -> int | None:
    raw_files = _raw_files(segment_dir, modality)
    if not raw_files:
        return None
    mtimes = []
    for path in raw_files:
        try:
            mtimes.append(int(path.stat().st_mtime * 1000))
        except OSError:
            continue
    return min(mtimes) if mtimes else None


def detect_stale_pending_segments(
    today: str, yesterday: str
) -> list[StalePendingTarget]:
    """Return stale pending audio/screen targets in the two-day scan window."""
    cutoff_ms = now_ms() - STALE_PENDING_AGE_MS
    targets: list[StalePendingTarget] = []
    for day in (today, yesterday):
        for stream, segment_key, segment_dir in iter_segments(day):
            for modality in ("audio", "screen"):
                signals = _modality_signals(segment_dir, modality)
                if str(signals["state"]) != DataState.PENDING.value:
                    continue
                oldest_raw_mtime = _oldest_raw_mtime_ms(segment_dir, modality)
                if oldest_raw_mtime is None or oldest_raw_mtime > cutoff_ms:
                    continue
                targets.append(
                    StalePendingTarget(
                        day=day,
                        stream=stream,
                        segment_key=segment_key,
                        modality=modality,
                        segment_dir=segment_dir,
                    )
                )
    return targets


def fire_stale_pending_recipe(
    target: StalePendingTarget, *, port: int
) -> RecipeOutcome:
    """Request a reprocess for one stale pending target."""
    ts = now_ms()
    day = urllib.parse.quote(target.day, safe="")
    stream = urllib.parse.quote(target.stream, safe="")
    segment_key = urllib.parse.quote(target.segment_key, safe="")
    url = (
        f"http://127.0.0.1:{port}/app/transcripts/api/segment/"
        f"{day}/{stream}/{segment_key}/reprocess"
    )
    body = json.dumps({"modality": target.modality}).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            status = getattr(response, "status", response.getcode())
            detail = response.read().decode("utf-8", errors="replace").strip() or None
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", errors="replace").strip() or str(exc)
        return RecipeOutcome(
            recipe=STALE_PENDING_RECIPE,
            target=target.target,
            outcome="failure",
            detail=detail,
            ts=ts,
        )
    except (OSError, urllib.error.URLError) as exc:
        return RecipeOutcome(
            recipe=STALE_PENDING_RECIPE,
            target=target.target,
            outcome="failure",
            detail=str(exc),
            ts=ts,
        )

    outcome = "success" if 200 <= int(status) < 300 else "failure"
    return RecipeOutcome(
        recipe=STALE_PENDING_RECIPE,
        target=target.target,
        outcome=outcome,
        detail=None if outcome == "success" else detail,
        ts=ts,
    )


def _last_recipe_outcomes(rows: list[dict], *, recipe: str, target: str) -> list[str]:
    outcomes: list[str] = []
    for row in rows:
        if row.get("event") != "recipe.outcome":
            continue
        if row.get("recipe") != recipe or row.get("target") != target:
            continue
        outcome = row.get("outcome")
        if outcome in {"success", "failure"}:
            outcomes.append(str(outcome))
    return outcomes


def _is_escalated(rows: list[dict], *, recipe: str, target: str) -> bool:
    outcomes = _last_recipe_outcomes(rows, recipe=recipe, target=target)
    return len(outcomes) >= 2 and outcomes[-2:] == ["failure", "failure"]


def run_recipe_pass(today: str) -> dict:
    """Run the registered steward recipes for today's two-day scan window."""
    yesterday = _previous_day(today)
    fired: list[RecipeOutcome] = []
    escalated_targets: list[str] = []
    data_source_errors: list[str] = []
    try:
        targets = detect_stale_pending_segments(today, yesterday)
    except Exception as exc:
        logger.warning("steward: stale pending detection failed", exc_info=True)
        data_source_errors.append(f"stale pending segment scan: {exc}")
        targets = []

    rows = load_steward_log()
    try:
        port = read_service_port("convey") or 5015
    except Exception as exc:
        logger.warning("steward: convey port read failed", exc_info=True)
        data_source_errors.append(f"convey port: {exc}")
        port = 5015

    for target in targets:
        if _is_escalated(rows, recipe=STALE_PENDING_RECIPE, target=target.target):
            escalated_targets.append(target.target)
            continue
        outcome = fire_stale_pending_recipe(target, port=port)
        fired.append(outcome)
        append_steward_event(
            "recipe.outcome",
            **dataclasses.asdict(outcome),
        )
        rows.append({"event": "recipe.outcome", **dataclasses.asdict(outcome)})

    return {
        "fired": fired,
        "escalated_targets": escalated_targets,
        "data_source_errors": data_source_errors,
    }


def _parse_sections(body: str) -> tuple[list[str], dict[str, list[str]]]:
    headings: list[str] = []
    sections: dict[str, list[str]] = {}
    current: str | None = None
    for line in body.splitlines():
        if line.startswith("## "):
            headings.append(line)
            sections[line] = []
            current = line
            continue
        if current is not None:
            sections[current].append(line)
    return headings, sections


def _parse_iso_z(value: str) -> datetime:
    return datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)


def validate_steward_health(body: str) -> str | None:
    """Validate the steward health render contract."""
    expected = [
        STEWARD_SECTION_STATUS,
        STEWARD_SECTION_ATTENTION,
        STEWARD_SECTION_AUTO_REPAIRS,
        STEWARD_SECTION_TRENDS,
    ]
    headings, sections = _parse_sections(body)
    if headings != expected:
        missing = [heading for heading in expected if heading not in headings]
        if missing:
            return f"missing section: {missing[0]}"
        extra = [heading for heading in headings if heading not in expected]
        if extra:
            return f"unexpected section: {extra[0]}"
        return "sections out of order"

    for line in body.splitlines():
        if _SECTION_RE.fullmatch(line) and line not in expected:
            return f"unexpected section: {line}"

    status_lines = sections[STEWARD_SECTION_STATUS]
    if not status_lines:
        return "missing generated_at"
    generated_at_line = status_lines[0]
    match = _GENERATED_AT_RE.fullmatch(generated_at_line)
    if not match:
        return "missing or invalid generated_at"
    try:
        _parse_iso_z(match.group(1))
    except ValueError:
        return "invalid generated_at timestamp"

    if not any(line.strip() for line in status_lines[1:]):
        return "empty status section"
    return None


def _first_status_body_line(lines: list[str]) -> str | None:
    for line in lines[1:]:
        stripped = line.strip()
        if stripped:
            return stripped
    return None


def _first_bullet(lines: list[str]) -> str | None:
    bullet: list[str] = []
    for line in lines:
        stripped = line.strip()
        if not bullet and (stripped.startswith("- ") or stripped.startswith("* ")):
            bullet.append(stripped[2:])
            continue
        if bullet:
            if line.startswith((" ", "\t")) and stripped:
                bullet.append(stripped)
                continue
            break
    return "\n".join(bullet) if bullet else None


def _has_bullets(lines: list[str]) -> bool:
    return any(line.strip().startswith(("- ", "* ")) for line in lines)


def read_steward_health(journal: Path | None = None) -> dict | None:
    """Return the home-page pipeline status derived from identity/health.md."""
    path = _journal_path(journal) / "identity" / "health.md"
    try:
        body = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return None

    if validate_steward_health(body) is not None:
        return None

    _headings, sections = _parse_sections(body)
    status_lines = sections[STEWARD_SECTION_STATUS]
    attention_lines = sections[STEWARD_SECTION_ATTENTION]
    status_lead = _first_status_body_line(status_lines) or ""
    if status_lead.startswith("Sol is well.") and not _has_bullets(attention_lines):
        return None

    bullet = _first_bullet(attention_lines)
    if bullet is None:
        return None
    return {"status": "warning", "message": bullet}


def _recipe_outcomes_7d(rows: list[dict]) -> list[dict]:
    cutoff = now_ms() - _SEVEN_DAYS_MS
    groups: dict[str, dict[str, Any]] = {}
    for row in rows:
        if row.get("event") != "recipe.outcome":
            continue
        try:
            ts = int(row.get("ts", 0))
        except (TypeError, ValueError):
            continue
        if ts < cutoff:
            continue
        recipe = str(row.get("recipe") or "")
        if not recipe:
            continue
        group = groups.setdefault(
            recipe,
            {"recipe": recipe, "success": 0, "failure": 0, "last_ts": ts},
        )
        outcome = row.get("outcome")
        if outcome == "success":
            group["success"] += 1
        elif outcome == "failure":
            group["failure"] += 1
        group["last_ts"] = max(int(group["last_ts"]), ts)

    result = []
    for group in groups.values():
        last_dt = datetime.fromtimestamp(int(group["last_ts"]) / 1000, tz=timezone.utc)
        total = int(group["success"]) + int(group["failure"])
        result.append(
            {
                **group,
                "total": total,
                "last_iso": last_dt.isoformat(timespec="seconds").replace(
                    "+00:00", "Z"
                ),
            }
        )
    result.sort(key=lambda row: str(row["recipe"]))
    return result


def _json(value: Any) -> str:
    return json.dumps(value, indent=2, sort_keys=True, default=str)


def build_synthesis_context(today: str) -> dict:
    """Build template variables for the steward talent prompt."""
    yesterday = _previous_day(today)
    errors: list[str] = []

    try:
        from solstone.think.surfaces import health as health_surface

        health_report = dataclasses.asdict(health_surface.for_range(yesterday, today))
    except Exception as exc:
        logger.warning("steward: health report failed", exc_info=True)
        health_report = None
        errors.append(f"health_report: {exc}")

    try:
        pipeline_day = summarize_pipeline_day(yesterday)
    except Exception as exc:
        logger.warning("steward: pipeline summary failed", exc_info=True)
        pipeline_day = None
        errors.append(f"pipeline_day: {exc}")

    rollup = _recipe_outcomes_7d(load_steward_log())
    return {
        "health_report": _json(health_report),
        "pipeline_day": _json(pipeline_day),
        "recipe_outcomes_7d": _json(rollup),
        "escalated_targets": _json([]),
        "data_source_errors": _json(errors),
        "generated_at": _utc_now_iso_z(),
        "status_lead_constraints": (
            "Use byte-exact 'Sol is well.' only when data_source_errors is empty, "
            "pipeline_day has no anomalies, escalated_targets is empty, and the "
            "7-day rollup has no failures."
        ),
    }


def latest_daily_run_complete_ts(today: str) -> int | None:
    """Return the newest daily run.complete timestamp from today/yesterday logs."""
    timestamps: list[int] = []
    for day in (today, _previous_day(today)):
        health_dir = day_path(day, create=False) / "health"
        if not health_dir.is_dir():
            continue
        for path in sorted(health_dir.glob("*_daily.jsonl")):
            try:
                with path.open(encoding="utf-8") as handle:
                    for raw_line in handle:
                        line = raw_line.strip()
                        if not line:
                            continue
                        try:
                            row = json.loads(line)
                        except json.JSONDecodeError:
                            continue
                        if isinstance(row, dict) and row.get("event") == "run.complete":
                            try:
                                timestamps.append(int(row["ts"]))
                            except (KeyError, TypeError, ValueError):
                                continue
            except OSError:
                logger.debug("steward: failed reading %s", path, exc_info=True)
    return max(timestamps) if timestamps else None


def generated_at_from_body(body: str) -> str | None:
    """Return the generated_at ISO-Z stamp from a valid steward body."""
    headings, sections = _parse_sections(body)
    if headings[:1] != [STEWARD_SECTION_STATUS]:
        return None
    status_lines = sections.get(STEWARD_SECTION_STATUS, [])
    if not status_lines:
        return None
    match = _GENERATED_AT_RE.fullmatch(status_lines[0])
    return match.group(1) if match else None


def generated_at_ms_from_body(body: str) -> int | None:
    stamp = generated_at_from_body(body)
    if stamp is None:
        return None
    try:
        return int(_parse_iso_z(stamp).timestamp() * 1000)
    except ValueError:
        return None


def write_health_md(body: str, *, reason: str = "steward synthesis") -> str | None:
    """Validate and write identity/health.md through the identity chokepoint."""
    validation_reason = validate_steward_health(body)
    if validation_reason is not None:
        append_steward_event(
            "render.failed",
            outcome="render_failed",
            target="identity/health.md",
            detail=validation_reason,
        )
        return validation_reason

    write_identity(
        "health.md",
        actor="steward",
        op="replace",
        section=None,
        content=body,
        reason=reason,
    )
    return None


def acquire_steward_lock() -> int | None:
    """Acquire the steward single-flight lock, returning the fd or None."""
    lock_path = Path(get_journal()) / "health" / ".steward.lock"
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(lock_path, os.O_WRONLY | os.O_CREAT, 0o600)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        os.close(fd)
        return None
    return fd


def release_steward_lock(fd: int) -> None:
    """Release and close a steward lock fd."""
    try:
        fcntl.flock(fd, fcntl.LOCK_UN)
    finally:
        os.close(fd)
