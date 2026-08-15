# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Steward health synthesis helpers and repair recipes."""

from __future__ import annotations

import fcntl
import json
import logging
import os
import re
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

from solstone.think.day_accumulator import read_latest
from solstone.think.identity import (
    STEWARD_SECTION_ATTENTION,
    STEWARD_SECTION_AUTO_REPAIRS,
    STEWARD_SECTION_STATUS,
    write_identity,
)
from solstone.think.pipeline_health import summarize_pipeline_day
from solstone.think.utils import day_path, get_journal, now_ms

logger = logging.getLogger(__name__)

STALE_PENDING_RECIPE = "stale_pending_segment_reprocess"
_SEVEN_DAYS_MS = 7 * 86_400_000
_THIRTY_DAYS_MS = 30 * 86_400_000

# Closed set of summary actions the convey widget can map to an affordance.
# Closed contract; initial values are reviewed. Keep it closed so the
# UI can map each value to a button/link.
SUGGESTED_ACTIONS: tuple[str, ...] = (
    "none",
    "open_health_detail",
    "open_support",
)
_HEADLINE_MAX = 80
_SENTENCE_MAX = 280
_RECIPE_LABELS = {STALE_PENDING_RECIPE: "stale-pending segment reprocess"}
_RECIPE_OUTCOMES = {
    "accepted",
    "running",
    "verified_healed",
    "failed",
    "no_output",
}
_INFLIGHT_RECIPE_OUTCOMES = {"accepted", "running"}
_GENERATED_AT_RE = re.compile(
    r"^<!-- generated_at: (\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z) -->$"
)
_SECTION_RE = re.compile(r"^## .+$")


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


def load_latest_pass_event() -> dict | None:
    """Return the most recent deterministic steward pass event."""
    for row in reversed(load_steward_log()):
        if row.get("event") == "pass":
            return row
    return None


def prune_steward_log() -> None:
    # Serialization: retained lines re-emitted verbatim in original order; only >=30d-old rows and JSON-undecodable lines are removed. Self-acquires .steward.lock so it mutually excludes the render.failed appender; the heartbeat "pass" append happens-before this call and is single-flighted by heartbeat.pid, so no append is lost.
    path = _steward_log_path()
    fd = acquire_steward_lock()
    if fd is None:
        return
    try:
        try:
            cutoff = now_ms() - _THIRTY_DAYS_MS
            kept: list[str] = []
            aged = 0
            malformed = 0
            try:
                with path.open(encoding="utf-8") as handle:
                    for raw_line in handle:
                        line = raw_line.rstrip("\n")
                        stripped = line.strip()
                        if not stripped:
                            kept.append(line)
                            continue
                        try:
                            row = json.loads(stripped)
                        except json.JSONDecodeError:
                            malformed += 1
                            continue
                        if isinstance(row, dict):
                            ts_raw = row.get("ts")
                            if ts_raw is not None:
                                try:
                                    ts = int(ts_raw)
                                except (TypeError, ValueError):
                                    pass
                                else:
                                    if ts < cutoff:
                                        aged += 1
                                        continue
                        kept.append(line)
            except FileNotFoundError:
                return

            dropped = aged + malformed
            if dropped == 0:
                return

            logger.info(
                "steward: pruned %d stale row(s), dropped %d malformed line(s) from steward.log",
                aged,
                malformed,
            )
            new_text = "".join(k + "\n" for k in kept)
            health_dir = path.parent
            tmp_fd, tmp_path = tempfile.mkstemp(
                dir=health_dir, prefix=".steward_", suffix=".tmp"
            )
            tmp_file = Path(tmp_path)
            try:
                with open(tmp_fd, "w", encoding="utf-8") as handle:
                    handle.write(new_text)
                tmp_file.replace(path)
            except BaseException:
                tmp_file.unlink(missing_ok=True)
                raise
        except Exception:
            logger.warning("steward: log prune failed", exc_info=True)
            return
    finally:
        release_steward_lock(fd)


def _normalize_recipe_outcome(outcome: Any) -> str | None:
    if outcome == "success":
        return "verified_healed"
    if outcome == "failure":
        return "failed"
    if isinstance(outcome, str) and outcome in _RECIPE_OUTCOMES:
        return outcome
    return None


def run_recipe_pass(today: str) -> dict:
    """Report-only health pass retained for the heartbeat pass-event contract.

    The deterministic steward no longer fires reprocesses; genuinely-pending
    segments are re-run by the daily sensing pre-phase. Returns the three keys
    heartbeat.py logs, all empty.
    """
    return {"fired": [], "escalated_targets": [], "data_source_errors": []}


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
    if status_lead.startswith("sol is well.") and not _has_bullets(attention_lines):
        return None

    bullet = _first_bullet(attention_lines)
    if bullet is None:
        return None
    return {"status": "warning", "message": bullet}


def _recipe_outcomes_7d(rows: list[dict]) -> list[dict]:
    cutoff = now_ms() - _SEVEN_DAYS_MS
    latest: dict[tuple[str, str], dict[str, Any]] = {}
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
        target = str(row.get("target") or "")
        if not recipe or not target:
            continue
        outcome = _normalize_recipe_outcome(row.get("outcome"))
        if outcome is None:
            continue
        key = (recipe, target)
        previous = latest.get(key)
        if previous is None or ts >= int(previous["ts"]):
            latest[key] = {
                "recipe": recipe,
                "target": target,
                "outcome": outcome,
                "ts": ts,
            }

    groups: dict[str, dict[str, Any]] = {}
    for latest_row in latest.values():
        recipe = str(latest_row["recipe"])
        group = groups.setdefault(
            recipe,
            {
                "recipe": recipe,
                "accepted": 0,
                "running": 0,
                "verified_healed": 0,
                "failed": 0,
                "no_output": 0,
                "unverified": 0,
                "last_ts": int(latest_row["ts"]),
            },
        )
        outcome = str(latest_row["outcome"])
        group[outcome] += 1
        if outcome in _INFLIGHT_RECIPE_OUTCOMES:
            group["unverified"] += 1
        group["last_ts"] = max(int(group["last_ts"]), int(latest_row["ts"]))

    result = []
    for group in groups.values():
        last_dt = datetime.fromtimestamp(int(group["last_ts"]) / 1000, tz=timezone.utc)
        total = sum(int(group[outcome]) for outcome in _RECIPE_OUTCOMES)
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


def gather_health_facts(today: str) -> dict:
    """Gather the deterministic facts the steward surfaces consume.

    Returns parsed (not JSON-encoded) objects so the deterministic renderer and
    the summary helpers can use them directly.
    """
    yesterday = _previous_day(today)
    errors: list[str] = []

    try:
        pipeline_day = summarize_pipeline_day(yesterday)
    except Exception as exc:
        logger.warning("steward: pipeline summary failed", exc_info=True)
        pipeline_day = None
        errors.append(f"pipeline_day: {exc}")

    rollup = _recipe_outcomes_7d(load_steward_log())
    return {
        "generated_at": _utc_now_iso_z(),
        "pipeline_day": pipeline_day,
        "recipe_outcomes_7d": rollup,
        "data_source_errors": errors,
    }


# ---------------------------------------------------------------------------
# Deterministic health.md renderer (no LLM in the write path)
# ---------------------------------------------------------------------------


def _status_sentence(
    *,
    pipeline_day: dict | None,
    data_source_errors: list,
    recipe_outcomes_7d: list,
) -> str:
    """Pick the single status sentence deterministically.

    ``sol is well.`` (byte-exact, so ``read_steward_health`` reads healthy) only
    when nothing is wrong; otherwise one terse factual sentence by priority.
    """
    pd = pipeline_day if isinstance(pipeline_day, dict) else {}
    anomalies = pd.get("anomalies", []) or []
    recent_failures = sum(
        int(row.get("failed", 0) or 0) + int(row.get("no_output", 0) or 0)
        for row in (recipe_outcomes_7d or [])
    )
    unverified = sum(
        int(row.get("unverified", 0) or 0) for row in (recipe_outcomes_7d or [])
    )
    if (
        not data_source_errors
        and not anomalies
        and not recent_failures
        and not unverified
    ):
        return "sol is well."
    if data_source_errors:
        return (
            "sol has a partial health picture: some health sources could not be read."
        )
    if anomalies:
        return (
            "sol detected pipeline issues during yesterday's processing "
            "that need attention."
        )
    if recent_failures:
        return "Recent auto-repairs include failures."
    n = unverified
    noun = "repair" if n == 1 else "repairs"
    return f"{n} stale segment {noun} in progress, not yet verified."


def _attention_bullets(
    *,
    pipeline_day: dict | None,
    data_source_errors: list,
) -> list[str]:
    """Render the canonical "Needs your attention" bullets deterministically."""
    bullets: list[str] = []
    pd = pipeline_day if isinstance(pipeline_day, dict) else {}
    anomalies = [a for a in (pd.get("anomalies", []) or []) if isinstance(a, dict)]
    kinds = {a.get("kind") for a in anomalies}

    if "activity_agents_missing" in kinds:
        n = int((pd.get("activities") or {}).get("detected", 0))
        bullets.append(
            f"- **Pipeline gap:** {n} activities ended yesterday but activity "
            "agents didn't fire — meeting notes, decisions, and follow-ups may "
            "be missing."
        )

    failures = [a for a in anomalies if a.get("kind") == "talent_failure"]
    if failures:
        n = int((pd.get("talents") or {}).get("outstanding_failed", len(failures)))
        names = [str(a.get("name")) for a in failures if a.get("name")]
        verb = (
            "couldn't start"
            if all(a.get("state") == "request_lost" for a in failures)
            else "timed out"
            if all(a.get("state") == "timeout" for a in failures)
            else "failed"
        )
        names_str = f" ({', '.join(names)})" if names else ""
        bullets.append(
            f"- **Pipeline issue:** {n} agents {verb} during yesterday's "
            f"processing{names_str}. Some insights may be incomplete."
        )

    if "daily_agents_missing" in kinds:
        bullets.append(
            "- **Pipeline gap:** Daily agents didn't run yesterday despite "
            "journal data. Facet newsletters may be missing."
        )

    seg = next((a for a in anomalies if a.get("kind") == "segments_not_thought"), None)
    if seg is not None:
        if seg.get("error"):
            bullets.append(
                "- **Pipeline gap:** Segment thinking status could not be determined."
            )
        else:
            n = int(seg.get("not_thought", 0))
            plural = "s" if n != 1 else ""
            bullets.append(
                f"- **Pipeline gap:** {n} segment{plural} sensed yesterday but "
                "not yet processed."
            )

    for err in data_source_errors:
        text = str(err).replace("\n", " ").strip()
        if ": " in text:
            source, detail = text.split(": ", 1)
            bullets.append(f"- could not read {source}: {detail}")
        else:
            bullets.append(f"- could not read {text}")

    return bullets


def _auto_repair_bullets(recipe_outcomes_7d: list) -> list[str]:
    """Render one bullet per recipe class in the 7-day rollup."""
    bullets: list[str] = []
    for row in recipe_outcomes_7d or []:
        recipe = str(row.get("recipe", ""))
        label = _RECIPE_LABELS.get(recipe, recipe.replace("_", " "))
        total = int(row.get("total", 0))
        verified_healed = int(row.get("verified_healed", 0))
        inflight = int(row.get("accepted", 0)) + int(row.get("running", 0))
        failed = int(row.get("failed", 0)) + int(row.get("no_output", 0))
        last_iso = row.get("last_iso", "")
        bullets.append(
            f"- {label} — {total}x in 7d ({verified_healed} verified-healed, "
            f"{inflight} in-flight, {failed} failed), last {last_iso}"
        )
    return bullets


def render_health_body(
    *,
    generated_at: str,
    pipeline_day: dict | None,
    recipe_outcomes_7d: list,
    data_source_errors: list,
) -> str:
    """Render the byte-exact 3-section health.md body from deterministic facts.

    Output is guaranteed to satisfy ``validate_steward_health``. No LLM is
    involved; the model only appends the human-friendly summaries to steward.jsonl.
    """
    status = _status_sentence(
        pipeline_day=pipeline_day,
        data_source_errors=data_source_errors,
        recipe_outcomes_7d=recipe_outcomes_7d,
    )
    attention = _attention_bullets(
        pipeline_day=pipeline_day,
        data_source_errors=data_source_errors,
    )
    repairs = _auto_repair_bullets(recipe_outcomes_7d)

    lines = [
        STEWARD_SECTION_STATUS,
        f"<!-- generated_at: {generated_at} -->",
        status,
        "",
        STEWARD_SECTION_ATTENTION,
    ]
    lines.extend(attention)
    lines.append("")
    lines.append(STEWARD_SECTION_AUTO_REPAIRS)
    lines.extend(repairs)
    lines.append("")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Human-friendly summaries (the lite generate talent output)
# ---------------------------------------------------------------------------


def _coerce_summary(raw: str | dict) -> dict | None:
    """Parse + validate a summary object; return a clean dict or None."""
    try:
        data = json.loads(raw) if isinstance(raw, str) else raw
    except (json.JSONDecodeError, TypeError):
        return None
    if not isinstance(data, dict):
        return None
    headline = data.get("headline")
    sentence = data.get("summary_sentence")
    action = data.get("suggested_action")
    if not isinstance(headline, str) or not isinstance(sentence, str):
        return None
    if not headline.strip() or not sentence.strip():
        return None
    if action not in SUGGESTED_ACTIONS:
        action = "none"
    return {
        "headline": headline.strip(),
        "summary_sentence": sentence.strip(),
        "suggested_action": action,
    }


def default_summary_from_body(body: str) -> dict:
    """Deterministic fallback summary derived from a rendered health.md body."""
    sections = _parse_sections(body)[1]
    status_lines = sections.get(STEWARD_SECTION_STATUS, [])
    status = _first_status_body_line(status_lines) or "sol is well."
    if status.startswith("sol is well."):
        return {
            "headline": "All clear",
            "summary_sentence": status,
            "suggested_action": "none",
        }
    return {
        "headline": "Needs attention",
        "summary_sentence": status,
        "suggested_action": "open_health_detail",
    }


def normalize_summary(result: str, default: dict) -> dict:
    """Clamp an LLM summary to the contract, falling back to ``default``."""
    summary = _coerce_summary(result)
    if summary is None:
        return default
    summary["headline"] = summary["headline"][:_HEADLINE_MAX] or default["headline"]
    summary["summary_sentence"] = (
        summary["summary_sentence"][:_SENTENCE_MAX] or default["summary_sentence"]
    )
    return summary


def read_steward_summary(day: str | None = None) -> dict | None:
    """Latest human-friendly steward summary for the home widget.

    Reads the newest record from the day-jsonl accumulator
    (chronicle/<day>/talents/steward.jsonl), walking back prior days.
    """
    if day is None:
        day = datetime.now().strftime("%Y%m%d")
    record = read_latest(day, "steward")
    if record is None:
        return None
    return _coerce_summary(record)


def load_previous_summary(today: str) -> dict | None:
    """Previous steward summary for run-to-run continuity.

    Delegates to read_steward_summary: the pre-hook reads before this run
    appends, so this returns the genuinely-previous run (earlier today, else a
    prior day). Empty journal -> None ("first run").
    """
    return read_steward_summary(day=today)


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
