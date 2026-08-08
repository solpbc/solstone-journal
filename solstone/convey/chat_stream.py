# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import logging
import sys
import threading
import time
from datetime import date, datetime
from pathlib import Path
from typing import Any

from solstone.apps.chat.copy import talent_label_for
from solstone.think.journal_io.atomic import atomic_replace
from solstone.think.utils import (
    day_path,
    get_journal,
    segment_key,
    segment_parse,
    segment_path,
)

logger = logging.getLogger(__name__)

_CHAT_LOCK = threading.Lock()
_CHAT_STREAM = "chat"
_SEGMENT_WINDOW_MS = 300_000
_APPENDED_CHAT_PATHS: dict[int, Path] = {}
# owner_message may carry optional `source`; extras flow through unchanged.
# sol_message may carry optional `thinking`, `offer`, `draft`, `sources`,
# `answer_state`, and folded-turn `origin`; result may carry optional `ticket_id`,
# `error`, `ambiguous`, and `cancelled`; talent_finished may carry optional
# `thinking`. talent_queued records a deferred spawn intent. Extras flow through
# unchanged and are not part of the required-field tuples below.
_VALID_KINDS = {
    "owner_message": ("text", "app", "path", "facet"),
    "sol_message": (
        "use_id",
        "text",
        "notes",
        "requested_target",
        "requested_task",
    ),
    "talent_spawned": ("use_id", "name", "task", "started_at"),
    "talent_queued": (
        "use_id",
        "name",
        "task",
        "queued_at",
        "chat_use_id",
        "ask",
        "context",
        "location",
    ),
    "talent_finished": ("use_id", "name", "summary"),
    "talent_errored": ("use_id", "name", "reason"),
    "reflection_ready": ("day", "url"),
    "chat_queue_depth": ("depth",),
    # chat_error also accepts optional `provider` (provider slug or "") and `detail`
    # (normalized raw provider error text, "" when absent); neither is validated.
    "chat_error": ("reason", "use_id"),
    "sol_chat_request": (
        "request_id",
        "summary",
        "message",
        "category",
        "dedupe",
        "dedupe_window",
        "since_ts",
        "trigger_talent",
    ),
    "sol_chat_request_superseded": ("request_id", "replaced_by"),
    "owner_chat_open": ("request_id", "surface"),
    "owner_chat_dismissed": ("request_id", "surface", "reason"),
    "support_draft": (
        "draft_id",
        "captured_day",
        "verb",
        "payload",
        "diagnostics_snapshot",
    ),
    "result": ("draft_id", "ok"),
    "support_submit_claim": ("draft_id",),
}
_TRIGGER_KINDS = {
    "owner_message",
    "talent_finished",
    "talent_errored",
    "sol_chat_request",
}


def _talent_label(name: Any, status: str) -> str:
    try:
        return talent_label_for(str(name), status)
    except ValueError:
        return str(name)


def append_chat_event(kind: str, **fields: Any) -> dict[str, Any]:
    """Append a chat event to the current 5-minute segment."""
    return append_chat_events_locked([(kind, fields)])[0]


def append_chat_events_locked(
    events: list[tuple[str, dict[str, Any]]],
    *,
    _lock_already_held: bool = False,
) -> list[dict[str, Any]]:
    """Append one or more chat events.

    Normal callers let this helper acquire ``_CHAT_LOCK``. Callers that must keep
    policy checks and append in a single critical section may pass
    ``_lock_already_held=True`` while holding ``_CHAT_LOCK``; in that mode this
    function only writes the stream and the caller must invoke
    ``_finalize_chat_event_appends`` after releasing the lock.
    """
    prepared = _prepare_chat_events(events)
    _require_journal_root()

    if _lock_already_held:
        return _append_prepared_chat_events_locked_already_held(prepared)

    with _CHAT_LOCK:
        stored_events = _append_prepared_chat_events_locked_already_held(prepared)

    _finalize_chat_event_appends(stored_events)
    return stored_events


def _prepare_chat_events(
    events: list[tuple[str, dict[str, Any]]],
) -> list[tuple[str, dict[str, Any]]]:
    prepared: list[tuple[str, dict[str, Any]]] = []
    for kind, fields in events:
        if kind not in _VALID_KINDS:
            raise ValueError(f"Unknown chat event kind: {kind}")
        event = dict(fields)
        event.setdefault("ts", int(time.time() * 1000))
        _validate_event(kind, event)
        prepared.append((kind, event))
    return prepared


def _append_prepared_chat_events_locked_already_held(
    events: list[tuple[str, dict[str, Any]]],
) -> list[dict[str, Any]]:
    from solstone.think.streams import update_stream, write_segment_stream

    stored_events: list[dict[str, Any]] = []

    for kind, event in events:
        day = _day_for_ts(event["ts"])
        segment = _current_segment_key(day, event["ts"])
        segment_dir = segment_path(day, segment, _CHAT_STREAM)
        chat_path = segment_dir / "chat.jsonl"
        had_segment_file = chat_path.exists()

        file_events = _read_events_file(chat_path)
        stored_event = {"kind": kind, **event}
        file_events.append(stored_event)
        _write_events_file(chat_path, file_events)

        if not had_segment_file:
            stream_info = update_stream(_CHAT_STREAM, day, segment, type=_CHAT_STREAM)
            write_segment_stream(
                segment_dir,
                _CHAT_STREAM,
                stream_info["prev_day"],
                stream_info["prev_segment"],
                stream_info["seq"],
            )

        _APPENDED_CHAT_PATHS[id(stored_event)] = chat_path
        stored_events.append(stored_event)

    return stored_events


def _finalize_chat_event_appends(stored_events: list[dict[str, Any]]) -> None:
    from solstone.think.indexer.journal import index_file

    indexed_paths: set[Path] = set()
    for stored_event in stored_events:
        chat_path = _APPENDED_CHAT_PATHS.pop(id(stored_event), None)
        if chat_path is None:
            chat_path = _chat_path_for_stored_event(stored_event)
        if chat_path in indexed_paths:
            continue
        indexed_paths.add(chat_path)
        try:
            index_file(get_journal(), str(chat_path))
        except Exception:
            logger.warning(
                "chat-event-index-failed",
                extra={
                    "kind": str(stored_event.get("kind") or ""),
                    "use_id": str(stored_event.get("use_id") or ""),
                    "chat_path": str(chat_path),
                },
                exc_info=True,
            )

    for stored_event in stored_events:
        _broadcast_chat_event(stored_event)


def read_chat_events(day: str, limit: int | None = None) -> list[dict[str, Any]]:
    """Return chat events for ``day`` in ascending timestamp order."""
    chat_root = day_path(day, create=False) / _CHAT_STREAM
    if not chat_root.exists():
        return []

    ordered: list[tuple[int, str, int, dict[str, Any]]] = []
    for segment_dir in sorted(chat_root.iterdir(), key=lambda path: path.name):
        if not segment_dir.is_dir() or segment_key(segment_dir.name) is None:
            continue
        chat_path = segment_dir / "chat.jsonl"
        if not chat_path.exists():
            continue
        for line_no, event in enumerate(_read_events_file(chat_path)):
            ordered.append(
                (int(event.get("ts", 0) or 0), segment_dir.name, line_no, event)
            )

    ordered.sort(key=lambda item: (item[0], item[1], item[2]))
    events = [item[3] for item in ordered]
    if limit is None:
        return events
    if limit == 0:
        return []
    return events[-limit:]


def read_chat_tail(day: str, limit: int = 20) -> list[dict[str, Any]]:
    """Return the most recent ``limit`` chat events for ``day``."""
    return read_chat_events(day, limit=limit)


def reduce_chat_state(day: str) -> dict[str, Any]:
    """Reduce a day's chat stream into the current chat session state."""
    latest_sol_message: dict[str, Any] | None = None
    active_talents: dict[str, dict[str, Any]] = {}
    completed_talents: list[dict[str, Any]] = []
    errored_talents: list[dict[str, Any]] = []
    queued_talents: dict[str, dict[str, Any]] = {}
    chat_error: dict[str, Any] | None = None
    queue_depth = 0

    for event in read_chat_events(day):
        kind = event.get("kind")
        if kind == "chat_queue_depth":
            queue_depth = int(event["depth"])
            continue

        if kind == "sol_message":
            latest_sol_message = {
                "ts": event["ts"],
                "use_id": event["use_id"],
                "text": event["text"],
                "notes": event["notes"],
                "requested_target": event["requested_target"],
                "requested_task": event["requested_task"],
                "offer": event.get("offer"),
                "draft": event.get("draft"),
                "origin": event.get("origin"),
                "sources": event.get("sources", []),
                "answer_state": event.get("answer_state", "answered"),
            }
            chat_error = None
            continue

        if kind == "talent_queued":
            queued_talents[str(event["use_id"])] = {
                "use_id": event["use_id"],
                "name": event["name"],
                "task": event["task"],
                "queued_at": event["queued_at"],
            }
            continue

        if kind == "talent_spawned":
            queued_talents.pop(str(event["use_id"]), None)
            active_talents[str(event["use_id"])] = {
                "use_id": event["use_id"],
                "name": event["name"],
                "task": event["task"],
                "started_at": event["started_at"],
                "label": _talent_label(event["name"], "running"),
            }
            continue

        if kind == "talent_finished":
            queued_talents.pop(str(event["use_id"]), None)
            started = active_talents.pop(str(event["use_id"]), None)
            completed_talents.append(
                {
                    "use_id": event["use_id"],
                    "name": event["name"],
                    "task": started["task"] if started else None,
                    "summary": event["summary"],
                    "finished_at": event["ts"],
                    "label": _talent_label(event["name"], "finished"),
                }
            )
            continue

        if kind == "talent_errored":
            queued_talents.pop(str(event["use_id"]), None)
            active_talents.pop(str(event["use_id"]), None)
            errored_talents.append(
                {
                    "use_id": event["use_id"],
                    "name": event["name"],
                    "finished_at": event["ts"],
                    "label": _talent_label(event["name"], "errored"),
                }
            )
            continue

        if kind == "chat_error":
            chat_error = {
                "reason": event["reason"],
                "provider": event.get("provider", ""),
                "detail": event.get("detail", ""),
            }
            continue

        if kind == "reflection_ready":
            continue

    return {
        "latest_sol_message": latest_sol_message,
        "active_talents": sorted(
            active_talents.values(),
            key=lambda talent: (
                int(talent.get("started_at", 0) or 0),
                str(talent["use_id"]),
            ),
        ),
        "queued_talents": sorted(
            queued_talents.values(),
            key=lambda talent: (
                int(talent.get("queued_at", 0) or 0),
                str(talent["use_id"]),
            ),
        ),
        "completed_talents": completed_talents,
        "errored_talents": errored_talents,
        "chat_error": chat_error,
        "queue_depth": queue_depth,
    }


def find_unresponded_trigger(day: str) -> dict[str, Any] | None:
    """Return the most recent unresolved trigger event for ``day``.

    A turn terminally closed by ``chat_error`` is not unresponded: timeout,
    provider-error, and invalid-response paths all clear the active turn. An
    ``owner_message`` with no later terminal event stays restartable for crash
    recovery.
    """
    for event in reversed(read_chat_events(day)):
        kind = event.get("kind")
        if kind == "sol_message":
            return None
        if kind == "chat_error":
            return None
        if kind in _TRIGGER_KINDS:
            return event
    return None


def _validate_event(kind: str, event: dict[str, Any]) -> None:
    ts = event.get("ts")
    if not isinstance(ts, int):
        raise ValueError(f"chat event ts must be an int, got {type(ts).__name__}")

    missing = [field for field in _VALID_KINDS[kind] if field not in event]
    if missing:
        required = ", ".join(missing)
        raise ValueError(f"{kind} requires fields: {required}")

    if kind == "chat_queue_depth" and not isinstance(event["depth"], int):
        raise ValueError("chat_queue_depth depth must be an int")


def _broadcast_chat_event(stored_event: dict[str, Any]) -> None:
    from solstone.think.callosum import callosum_send

    chat_module = sys.modules.get("solstone.convey.chat")
    runtime = (
        getattr(chat_module, "_runtime", None) if chat_module is not None else None
    )
    if runtime is None:
        return

    kind = str(stored_event.get("kind") or "")
    use_id = str(stored_event.get("use_id") or "")

    try:
        if runtime.callosum.emit("chat", kind, **stored_event):
            return
        if callosum_send("chat", kind, **stored_event):
            return
        logger.warning(
            "Failed to broadcast chat event kind=%s use_id=%s",
            kind,
            use_id or "-",
        )
    except Exception as exc:
        logger.warning(
            "Failed to broadcast chat event kind=%s use_id=%s: %s",
            kind,
            use_id or "-",
            exc,
        )


def _require_journal_root() -> Path:
    journal = Path(get_journal())
    if not journal.exists():
        raise FileNotFoundError(f"Journal root does not exist: {journal}")
    if not journal.is_dir():
        raise NotADirectoryError(f"Journal root is not a directory: {journal}")
    return journal


def _day_for_ts(ts_ms: int) -> str:
    return _ts_to_local_datetime(ts_ms).strftime("%Y%m%d")


def day_for_ts(ts: int) -> str:
    """Return the chat-stream day bucket for an event timestamp (ms)."""
    return _day_for_ts(ts)


def record_draft_captured(draft_id: str, day: str) -> None:
    """Record the day containing a support draft for bounded later resolution."""
    path = _support_draft_index_path(draft_id)
    atomic_replace(
        path,
        json.dumps({"captured_day": day}, separators=(",", ":")) + "\n",
        mode=0o600,
    )


def resolve_draft_day(draft_id: str) -> str | None:
    """Return a captured support draft's indexed day without changing state."""
    try:
        path = _support_draft_index_path(draft_id)
    except (FileNotFoundError, ValueError):
        return None
    if not path.is_file():
        return None
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    day = payload.get("captured_day") if isinstance(payload, dict) else None
    return day if isinstance(day, str) else None


def _support_draft_index_path(draft_id: str) -> Path:
    if not draft_id or Path(draft_id).name != draft_id:
        raise ValueError("invalid support draft id")
    return _require_journal_root() / "chronicle" / "health" / "support-drafts" / (
        f"{draft_id}.json"
    )


def _current_segment_key(day: str, ts_ms: int) -> str:
    event_dt = _ts_to_local_datetime(ts_ms)
    existing = _chat_segments(day)
    if not existing:
        return _segment_key_for_start(event_dt)

    current = existing[-1]
    current_start = _segment_start_datetime(day, current)
    current_start_ms = int(current_start.timestamp() * 1000)
    if ts_ms - current_start_ms >= _SEGMENT_WINDOW_MS:
        return _segment_key_for_start(event_dt)
    return current


def _chat_path_for_stored_event(stored_event: dict[str, Any]) -> Path:
    ts = int(stored_event["ts"])
    day = _day_for_ts(ts)
    segment = _current_segment_key(day, ts)
    return segment_path(day, segment, _CHAT_STREAM) / "chat.jsonl"


def _chat_segments(day: str) -> list[str]:
    chat_root = day_path(day, create=False) / _CHAT_STREAM
    if not chat_root.exists():
        return []
    return sorted(
        entry.name
        for entry in chat_root.iterdir()
        if entry.is_dir() and segment_key(entry.name) is not None
    )


def _segment_key_for_start(start_dt: datetime) -> str:
    return f"{start_dt.strftime('%H%M%S')}_300"


def _segment_start_datetime(day: str, segment: str) -> datetime:
    start_time, _ = segment_parse(segment)
    if start_time is None:
        raise ValueError(f"Invalid chat segment key: {segment}")
    return datetime.combine(
        date.fromisoformat(f"{day[:4]}-{day[4:6]}-{day[6:8]}"), start_time
    )


def _ts_to_local_datetime(ts_ms: int) -> datetime:
    return datetime.fromtimestamp(ts_ms / 1000)


def _read_events_file(path: Path) -> list[dict[str, Any]]:
    if not path.exists():
        return []

    events: list[dict[str, Any]] = []
    with open(path, "r", encoding="utf-8") as handle:
        for line_no, raw_line in enumerate(handle, start=1):
            line = raw_line.strip()
            if not line:
                continue
            try:
                payload = json.loads(line)
            except json.JSONDecodeError as exc:
                raise ValueError(f"Invalid JSON in {path}:{line_no}") from exc
            if not isinstance(payload, dict):
                raise ValueError(f"Expected JSON object in {path}:{line_no}")
            events.append(payload)
    return events


def _write_events_file(path: Path, events: list[dict[str, Any]]) -> None:
    body = "".join(json.dumps(event, ensure_ascii=False) + "\n" for event in events)
    atomic_replace(path, body)
