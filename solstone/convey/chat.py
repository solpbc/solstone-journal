# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

# Chat backend runs in a single Flask worker process. The threading.Lock plus
# module-level singleton state assumes one convey process per stack.

from __future__ import annotations

import atexit
import json
import logging
import os
import pprint
import re
import threading
from collections import deque
from dataclasses import asdict, dataclass, field
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any

import httpx
from flask import Blueprint, jsonify, request

import solstone.convey.chat_stream as chat_stream
from solstone.apps.chat.copy import (
    CHAT_CLOSER_DIFFERENT_ANGLE_SUFFIX,
    CHAT_CLOSER_LOOP_EXHAUSTED_PREFIX,
    CHAT_CLOSER_SUPPORT_SEND_FAILED,
    CHAT_CLOSER_TALENT_ERRORED_FORMAT,
    CHAT_CLOSER_TALENT_ERRORED_GENERIC,
    CHAT_DEFERRED_NOT_ANALYZED,
    CHAT_LIVENESS_TASK_FORMAT,
    CHAT_OFFER_SUPPORT_DECLINE,
    CHAT_OFFER_SUPPORT_PROMPT,
    CHAT_SUPPORT_ATTACH_FILED_FORMAT,
    CHAT_SUPPORT_CLOSE_SUBMITTED,
    CHAT_SUPPORT_DRAFT_CANCELLED,
    CHAT_SUPPORT_DRAFT_READY,
    CHAT_SUPPORT_IN_PROGRESS,
    CHAT_SUPPORT_RECONSENT_NEEDED,
    CHAT_SUPPORT_RESOLVED_SUBMITTED,
    CHAT_SUPPORT_STILL_NEED_HELP_SUBMITTED,
    CHAT_SUPPORT_SUBMIT_AMBIGUOUS,
    CHAT_SUPPORT_SUBMIT_FAILED,
    CHAT_SUPPORT_SUBMIT_FILED_FORMAT,
    CHAT_THINKING_ENGINE_NOT_CHOSEN,
)
from solstone.apps.support import operations
from solstone.apps.support.tools import (
    support_attach,
    support_close,
    support_create,
    support_feedback,
    support_reply,
    support_resolved,
    support_still_need_help,
)
from solstone.convey.chat_sources import parse_sol_sources
from solstone.convey.chat_stream import (
    append_chat_event,
    find_unresponded_trigger,
    read_chat_events,
    reduce_chat_state,
)
from solstone.convey.reasons import (
    AGENT_UNAVAILABLE,
    CHAT_QUEUE_FULL,
    INVALID_REQUEST_VALUE,
    MISSING_REQUIRED_FIELD,
    TALENT_NOT_FOUND,
)
from solstone.convey.sol_initiated import (
    record_owner_chat_dismissed,
    record_owner_chat_open,
    start_chat,
)
from solstone.convey.sol_initiated.copy import KIND_SOL_CHAT_REQUEST, SURFACE_CONVEY
from solstone.convey.utils import error_response
from solstone.think.callosum import CallosumConnection, callosum_send
from solstone.think.cogitate_policy import DETERMINISTIC_FAILURE_REASON_CODES
from solstone.think.cortex_client import CortexNotClaimed, CortexSpawnUnavailable
from solstone.think.pipeline_health import SegmentBacklog, read_segment_backlog
from solstone.think.processing import (
    ProcessingSettings,
    format_awaiting_analysis,
    load_processing_settings,
)
from solstone.think.utils import get_journal, now_ms

logger = logging.getLogger(__name__)

chat_bp = Blueprint("chat", __name__, url_prefix="/api/chat")

MAX_ACTIVE_TALENTS = 2
_WATCHDOG_TIMEOUTS = {"chat": 30, "talent": 180}
_DEFAULT_WATCHDOG_SECONDS = 180
_RESERVED_USE_ID_CAP = 256
_ABANDONED_RAW_USE_ID_CAP = 256
_RAW_USE_LIVENESS_CAP = _ABANDONED_RAW_USE_ID_CAP
_CORTEX_CANCEL_REASON_CODE = "chat_watchdog_cancelled"

_state_lock = threading.Lock()
_runtime_lock = threading.Lock()
_current_chat_use_id: str | None = None
_current_chat_state: dict[str, Any] | None = None
_queued_triggers: deque[dict[str, Any]] = deque()
_active_talents: dict[str, dict[str, Any]] = {}
_reserved_use_ids: dict[str, None] = {}
_abandoned_raw_use_ids: dict[str, None] = {}
_thinking_buffers: dict[str, list[str]] = {}
_thinking_providers: dict[str, str] = {}
_raw_use_liveness: dict[str, "RawUseLiveness"] = {}
_watchdog_timers: dict[str, threading.Timer] = {}
_last_use_id = 0
_runtime: "ChatRuntimeState | None" = None
_atexit_registered = False


def _normalize_chat_error_detail(raw: str | None) -> str:
    """Normalize a raw provider error message for chat_error.detail.

    None/missing -> "".
    Otherwise: strip; collapse whitespace runs (including \n\r\t) to single spaces;
    truncate to 240 chars total using a single trailing ellipsis (… included in budget).
    """
    if not raw:
        return ""
    collapsed = " ".join(str(raw).split())
    if not collapsed:
        return ""
    if len(collapsed) <= 240:
        return collapsed
    return collapsed[:239] + "…"


@dataclass
class ChatRuntimeState:
    callosum: CallosumConnection
    apps: list[Any] = field(default_factory=list)


@dataclass(frozen=True)
class RawUseLiveness:
    last_event_type: str
    last_seen_ms: int
    observed_progress_count: int = 0


@dataclass(frozen=True)
class ChatSpawnResult:
    ok: bool
    reason: str = ""
    detail: str = ""


@dataclass(frozen=True)
class SupportDraftSubmitResult:
    ok: bool
    outcome: str
    text: str
    result_fields: dict[str, Any]
    ticket_id: Any = None
    terminal: bool = True


@chat_bp.route("", methods=["POST"])
def post_chat() -> Any:
    """Accept an owner message and schedule the chat singleton."""
    payload = request.get_json(force=True) or {}
    message = str(payload.get("message") or "").strip()
    if not message:
        return error_response(MISSING_REQUIRED_FIELD, detail="message is required")

    from solstone.think.identity import ensure_identity_directory

    ensure_identity_directory()

    location = _normalize_location(
        payload.get("app"),
        payload.get("path"),
        payload.get("facet"),
    )
    source = payload.get("source")
    if source is not None and not isinstance(source, dict):
        logger.warning("dropping malformed chat source: %r", source)
        source = None
    event_fields: dict[str, Any] = {
        "text": message,
        "app": location["app"],
        "path": location["path"],
        "facet": location["facet"],
    }
    if source is not None:
        event_fields["source"] = source
    trigger = {
        "type": "owner_message",
        "message": message,
    }

    with _state_lock:
        if _current_chat_use_id is not None and len(_queued_triggers) >= 10:
            return error_response(
                CHAT_QUEUE_FULL,
                extra={"queue_depth": len(_queued_triggers)},
            )

    append_chat_event("owner_message", **event_fields)

    start_info: dict[str, Any] | None = None
    with _state_lock:
        response_use_id, queued, start_info = _activate_or_enqueue_trigger_locked(
            trigger,
            location,
        )
        queue_depth = len(_queued_triggers)

    if start_info is not None:
        spawn_result = _spawn_chat_generate(start_info)
        if not spawn_result.ok:
            _handle_chat_failure(
                response_use_id,
                spawn_result.reason,
                detail=spawn_result.detail,
            )
            return error_response(
                AGENT_UNAVAILABLE,
                detail="Failed to connect to agent service",
            )

    return jsonify(use_id=response_use_id, queued=queued, queue_depth=queue_depth)


@chat_bp.route(f"/{KIND_SOL_CHAT_REQUEST}/open", methods=["POST"])
def sol_chat_request_open() -> Any:
    """Record that the owner opened a sol-initiated chat request."""
    payload = request.get_json(force=True, silent=True) or {}
    request_id = str(payload.get("request_id") or "").strip()
    if not request_id:
        return error_response(MISSING_REQUIRED_FIELD, detail="request_id required")
    record_owner_chat_open(request_id, surface=SURFACE_CONVEY)
    return jsonify({"ok": True})


@chat_bp.route("/start", methods=["POST"])
def chat_start_request() -> Any:
    """Start a sol-initiated chat request (CLI dispatch over start_chat)."""
    payload = request.get_json(force=True, silent=True) or {}
    try:
        result = start_chat(
            summary=payload.get("summary", ""),
            message=payload.get("message"),
            category=payload.get("category", ""),
            dedupe=payload.get("dedupe", ""),
            dedupe_window=payload.get("dedupe_window"),
            since_ts=payload.get("since_ts", 0),
            trigger_talent=payload.get("trigger_talent", ""),
        )
    except ValueError as exc:
        return error_response(INVALID_REQUEST_VALUE, detail=str(exc))
    return jsonify(asdict(result))


@chat_bp.route(f"/{KIND_SOL_CHAT_REQUEST}/dismissed", methods=["POST"])
def sol_chat_request_dismissed() -> Any:
    """Record that the owner dismissed a sol-initiated chat request."""
    payload = request.get_json(force=True, silent=True) or {}
    request_id = str(payload.get("request_id") or "").strip()
    if not request_id:
        return error_response(MISSING_REQUIRED_FIELD, detail="request_id required")
    reason = payload.get("reason")
    reason_str = str(reason).strip() if reason is not None else None
    record_owner_chat_dismissed(
        request_id,
        surface=SURFACE_CONVEY,
        reason=reason_str or None,
    )
    return jsonify({"ok": True})


@chat_bp.route("/session", methods=["GET"])
def chat_session() -> Any:
    """Return reduced state for today's chat stream."""
    _recover_chat_if_needed()
    return jsonify(reduce_chat_state(_today_day()))


@chat_bp.route("/offer/decline", methods=["POST"])
def decline_offer() -> Any:
    """Owner declined a pending support offer. Append a local sol_message with no
    offer field so it supersedes the pending offer (chips clear on reload). This is
    a standalone write — it does not read or mutate active-turn state, so it's safe
    whether or not a turn is in flight. No talent spawn, no brain call."""
    with _state_lock:
        use_id = _reserve_use_id_locked()
    append_chat_event(
        "sol_message",
        use_id=use_id,
        text=CHAT_OFFER_SUPPORT_DECLINE,
        notes="owner declined the support offer",
        requested_target=None,
        requested_task=None,
        sources=[],
        answer_state="answered",
    )
    return jsonify(ok=True)


@chat_bp.route("/support/draft/confirm", methods=["POST"])
def confirm_support_draft() -> Any:
    """Owner confirmed a captured support draft; submit it through support tools."""
    payload = request.get_json(force=True, silent=True) or {}
    draft_id = str(payload.get("draft_id") or "").strip()
    if not draft_id:
        return error_response(MISSING_REQUIRED_FIELD, detail="draft_id required")

    resolved = _resolve_support_draft(draft_id)
    if resolved is None:
        return jsonify(ok=False, outcome="not_found")
    draft_event, captured_day = resolved

    with chat_stream._CHAT_LOCK:
        # Lock order: hold only _CHAT_LOCK here; accepted v1 crash-window leaves a claim without TTL.
        events = chat_stream.read_chat_events(captured_day)
        if _draft_is_terminal(events, draft_id):
            return jsonify(ok=False, outcome="already_submitted")
        latest_draft = _latest_support_draft(captured_day)
        latest_draft_id = str((latest_draft or {}).get("draft_id") or "")
        if latest_draft_id != draft_id:
            return jsonify(ok=False, outcome="superseded")
        stored_claim = chat_stream.append_chat_events_locked(
            [
                (
                    "support_submit_claim",
                    {
                        "ts": _next_chat_ts_for_day(captured_day, events),
                        "draft_id": draft_id,
                        "generation": sum(
                            event.get("kind") == "support_submit_claim"
                            and str(event.get("draft_id") or "") == draft_id
                            for event in events
                        )
                        + 1,
                    },
                )
            ],
            _lock_already_held=True,
        )
    chat_stream._finalize_chat_event_appends(stored_claim)

    try:
        submit_result = _submit_support_draft(draft_event, draft_id)
    except operations.OperationSupersededError:
        result = next(
            (
                event
                for event in reversed(chat_stream.read_chat_events(captured_day))
                if event.get("kind") == "result"
                and str(event.get("draft_id") or "") == draft_id
            ),
            None,
        )
        if result is None:
            raise
        return jsonify(_support_draft_result_response(result))

    if submit_result.terminal:
        append_chat_event(
            "result",
            ts=_next_chat_ts_for_day(
                captured_day,
                chat_stream.read_chat_events(captured_day),
            ),
            **submit_result.result_fields,
        )
    # Lock order: reserve under _state_lock after _CHAT_LOCK is fully released.
    with _state_lock:
        use_id = _reserve_use_id_locked()
    append_chat_event(
        "sol_message",
        ts=_next_chat_ts_for_day(
            captured_day,
            chat_stream.read_chat_events(captured_day),
        ),
        use_id=use_id,
        text=submit_result.text,
        notes=f"support draft {submit_result.outcome}",
        requested_target=None,
        requested_task=None,
        sources=[],
        answer_state="answered",
    )

    response: dict[str, Any] = {
        "ok": submit_result.ok,
        "outcome": submit_result.outcome,
    }
    if submit_result.outcome == "submitted":
        response["ticket_id"] = submit_result.ticket_id
    return jsonify(response)


@chat_bp.route("/support/draft/cancel", methods=["POST"])
def cancel_support_draft() -> Any:
    """Owner cancelled a captured support draft without contacting support."""
    payload = request.get_json(force=True, silent=True) or {}
    draft_id = str(payload.get("draft_id") or "").strip()
    if not draft_id:
        return error_response(MISSING_REQUIRED_FIELD, detail="draft_id required")

    resolved = _resolve_support_draft(draft_id)
    if resolved is None:
        return jsonify(ok=False, outcome="not_found")
    _draft_event, captured_day = resolved

    # Lock order: reserve under _state_lock before _CHAT_LOCK is acquired.
    with _state_lock:
        use_id = _reserve_use_id_locked()
    with chat_stream._CHAT_LOCK:
        # Lock order: hold only _CHAT_LOCK here; accepted v1 crash-window leaves a claim without TTL.
        events = chat_stream.read_chat_events(captured_day)
        if _draft_is_terminal(events, draft_id):
            return jsonify(ok=False, outcome="already_submitted")
        latest_draft = _latest_support_draft(captured_day)
        latest_draft_id = str((latest_draft or {}).get("draft_id") or "")
        if latest_draft_id != draft_id:
            return jsonify(ok=False, outcome="superseded")
        terminal_ts = _next_chat_ts_for_day(captured_day, events)
        stored = chat_stream.append_chat_events_locked(
            [
                (
                    "result",
                    {
                        "ts": terminal_ts,
                        "draft_id": draft_id,
                        "ok": False,
                        "cancelled": True,
                    },
                ),
                (
                    "sol_message",
                    {
                        "ts": terminal_ts,
                        "use_id": use_id,
                        "text": CHAT_SUPPORT_DRAFT_CANCELLED,
                        "notes": "support draft cancelled",
                        "requested_target": None,
                        "requested_task": None,
                        "sources": [],
                        "answer_state": "answered",
                    },
                ),
            ],
            _lock_already_held=True,
        )
    chat_stream._finalize_chat_event_appends(stored)
    return jsonify(ok=True, outcome="cancelled")


@chat_bp.route("/talent-log/<use_id>", methods=["GET"])
def get_talent_log(use_id: str) -> Any:
    """Return a talent-use timeline from the JSONL log."""
    result = _read_talent_log(use_id)
    if result is None:
        return error_response(
            TALENT_NOT_FOUND,
            detail=f"Talent log not found for use_id {use_id}",
        )
    return jsonify(result)


def start_chat_runtime(app: Any) -> None:
    """Start the chat backend runtime and subscribe to cortex events."""
    global _runtime, _atexit_registered

    if app.debug and os.environ.get("WERKZEUG_RUN_MAIN") != "true":
        logger.info("skipping chat runtime startup in Werkzeug reloader parent")
        app.chat_runtime_started = False
        return

    with _runtime_lock:
        if _runtime is None:
            runtime = ChatRuntimeState(callosum=CallosumConnection())
            runtime.callosum.start(callback=_handle_callosum_message)
            _runtime = runtime
        runtime = _runtime
        if app not in runtime.apps:
            runtime.apps.append(app)
        app.chat_runtime_started = True
        if not _atexit_registered:
            atexit.register(stop_all_chat_runtime)
            _atexit_registered = True

    _recover_chat_if_needed()


def stop_chat_runtime(app: Any) -> None:
    """Detach an app from the shared runtime."""
    app.chat_runtime_started = False
    runtime = _runtime
    if runtime is None:
        return
    with _runtime_lock:
        if app in runtime.apps:
            runtime.apps.remove(app)
        remaining = list(runtime.apps)
    if not remaining:
        stop_all_chat_runtime()


def stop_all_chat_runtime() -> None:
    """Stop the shared runtime."""
    global _runtime

    with _state_lock:
        for timer in _watchdog_timers.values():
            timer.cancel()
        _watchdog_timers.clear()
        _reserved_use_ids.clear()
        _abandoned_raw_use_ids.clear()
        _thinking_buffers.clear()
        _thinking_providers.clear()
        _raw_use_liveness.clear()

    with _runtime_lock:
        runtime = _runtime
        _runtime = None
    if runtime is None:
        return
    for app in list(runtime.apps):
        try:
            app.chat_runtime_started = False
        except Exception:
            logger.exception("chat runtime app cleanup failed")
    runtime.callosum.stop()


def _handle_callosum_message(message: dict[str, Any]) -> None:
    if message.get("chat_proxy"):
        return
    if message.get("tract") != "cortex":
        return

    event_type = message.get("event")
    if event_type == "thinking":
        with _state_lock:
            _capture_thinking_locked(message)
        _proxy_progress(message)
        return
    if event_type == "start":
        with _state_lock:
            _capture_thinking_provider_locked(message)
        _proxy_progress(message)
        return
    if event_type == "finish":
        _on_cortex_finish(message)
        return
    if event_type == "error":
        _on_cortex_error(message)
        return

    _proxy_progress(message)


def _proxy_progress(message: dict[str, Any]) -> None:
    # CortexService._handle_callosum_message accepts tract=cortex/event=request
    # without checking chat_proxy. Re-emitting request would spawn a duplicate talent.
    if message.get("event") == "request":
        return

    logical_use_id: str | None = None
    use_id = str(message.get("use_id") or "")
    if not use_id:
        return

    with _state_lock:
        if _current_chat_state is None or _current_chat_use_id is None:
            return
        raw_chat_use_id = str(_current_chat_state.get("raw_use_id") or "")
        if use_id == raw_chat_use_id:
            logical_use_id = _current_chat_use_id
            _refresh_watchdog_locked(
                use_id,
                "chat",
                str(_current_chat_use_id),
                str(message.get("event") or "progress"),
            )
        elif use_id in _active_talents:
            logical_use_id = str(_active_talents[use_id]["chat_use_id"])
            _refresh_watchdog_locked(
                use_id,
                "talent",
                logical_use_id,
                str(message.get("event") or "progress"),
            )
        elif _is_superseded_raw_use_id_locked(use_id):
            logger.debug(
                "superseded raw cortex event use_id=%s event=%s reason=%s",
                use_id,
                str(message.get("event") or "progress"),
                "raw rotated",
            )

    if logical_use_id is None:
        return

    fields = {
        key: value
        for key, value in message.items()
        if key not in {"tract", "event", "use_id"}
    }
    fields["use_id"] = logical_use_id
    fields["chat_proxy"] = True
    _emit_cortex_event(message["event"], **fields)


def _no_thinking_engine_chosen() -> bool:
    from solstone.think.models import no_thinking_engine_chosen

    return no_thinking_engine_chosen()


def compose_honest_degradation(
    settings: ProcessingSettings,
    backlog: SegmentBacklog,
    *,
    queried_day: str | None = None,
) -> str | None:
    """Return the honest message for deferred analysis or no thinking engine.

    Pure: operates only on already-read inputs. Fires in no-engine state even in
    realtime mode; otherwise fires only in deferred mode when the anchor day
    (the queried day if supplied, else today) has pending backlog. Any per-day
    fold error makes deferred reads indeterminate -> None, while no-engine still
    returns its setup guidance. Pending counts are derived from real backlog
    reads, never fabricated.
    """
    no_engine = _no_thinking_engine_chosen()
    if not no_engine and settings.mode != "deferred":
        return None
    if backlog.errors:
        return CHAT_THINKING_ENGINE_NOT_CHOSEN if no_engine else None
    anchor_day = queried_day if queried_day is not None else _today_day()
    completion = backlog.per_day.get(anchor_day)
    if completion is None:
        return CHAT_THINKING_ENGINE_NOT_CHOSEN if no_engine else None
    pending = completion.not_sensed + completion.not_thought
    if no_engine:
        if pending > 0:
            return (
                f"{CHAT_THINKING_ENGINE_NOT_CHOSEN} {format_awaiting_analysis(pending)}"
            )
        return CHAT_THINKING_ENGINE_NOT_CHOSEN
    if pending <= 0:
        return None
    return f"{CHAT_DEFERRED_NOT_ANALYZED} {format_awaiting_analysis(pending)}"


def _honest_degradation_message(queried_day: str | None = None) -> str | None:
    """Fail-safe wrapper: read mode + backlog, return honest message or None.

    On ANY read error, return None so the caller keeps the unchanged empty path.
    """
    try:
        settings = load_processing_settings()
        backlog = read_segment_backlog()
    except Exception:
        logger.warning("honest-degradation read failed", exc_info=True)
        return None
    return compose_honest_degradation(settings, backlog, queried_day=queried_day)


def _on_cortex_finish(message: dict[str, Any]) -> None:
    use_id = str(message.get("use_id") or "")
    if not use_id:
        return

    next_actions: list[dict[str, Any] | None] = []
    finish_payload: dict[str, Any] | None = None
    error_payload: dict[str, Any] | None = None

    with _state_lock:
        if _current_chat_state is not None and use_id == _current_chat_state.get(
            "raw_use_id"
        ):
            logical_use_id = str(_current_chat_use_id)
            _cancel_watchdog_locked(use_id)
            try:
                parsed = _parse_chat_result(
                    message.get("result"), use_id=logical_use_id
                )
            except ValueError:
                provider = str(message.get("provider") or "")
                if int(_current_chat_state.get("retry_count", 0) or 0) < 1:
                    retry_use_id = _reserve_use_id_locked()
                    _set_current_raw_use_locked(logical_use_id, retry_use_id)
                    _current_chat_state["retry_count"] = (
                        int(_current_chat_state.get("retry_count", 0) or 0) + 1
                    )
                    next_actions.append(_build_spawn_info_locked(logical_use_id))
                else:
                    _evict_thinking_locked(use_id)
                    append_chat_event(
                        "chat_error",
                        reason="provider_response_invalid",
                        use_id=logical_use_id,
                        provider=provider,
                        detail="",
                    )
                    error_payload = {
                        "use_id": logical_use_id,
                        "reason": "provider_response_invalid",
                    }
                    next_actions.append(_clear_current_locked())
            else:
                message_text = parsed["message"] or ""
                requested_target = (
                    parsed["talent_request"]["target"]
                    if parsed["talent_request"]
                    else None
                )
                requested_task = (
                    parsed["talent_request"]["task"]
                    if parsed["talent_request"]
                    else None
                )
                offer: dict[str, Any] | None = None
                answer_state = "answered"
                trigger = _current_chat_state.get("trigger") or {}
                trigger_type = trigger.get("type")
                if trigger_type in {"talent_finished", "talent_errored"}:
                    exit_mode = (
                        "talent_errored"
                        if trigger_type == "talent_errored"
                        else "loop_exhausted"
                    )
                    message_text = _compose_terminal_closer(
                        exit_mode,
                        message_text,
                        talent_name=trigger.get("name"),
                        talent_errored_reason=trigger.get("reason"),
                        talent_errored_reason_code=trigger.get("reason_code"),
                        talent_finished_summary=trigger.get("summary"),
                    )
                    answer_state = (
                        "failed" if exit_mode == "talent_errored" else "partial"
                    )
                    requested_target = None
                    requested_task = None
                draft: dict[str, Any] | None = None
                if (
                    trigger_type == "talent_finished"
                    and trigger.get("name") in OUTBOUND_TALENTS
                    and _support_draft_state(_today_day()) == "pending"
                ):
                    latest_draft = _latest_support_draft(_today_day())
                    if latest_draft is not None:
                        message_text = CHAT_SUPPORT_DRAFT_READY
                        verb = str(latest_draft.get("verb") or "")
                        if verb == "attach":
                            source_payload = latest_draft["payload"]
                            marker_payload = {
                                "ticket_id": source_payload["ticket_id"],
                                "filename": source_payload["filename"],
                                "content_type": source_payload["content_type"],
                                "byte_size": source_payload["byte_size"],
                            }
                        else:
                            marker_payload = latest_draft.get("payload")
                        draft = {
                            "draft_id": latest_draft.get("draft_id"),
                            "verb": verb,
                            "payload": marker_payload,
                            "diagnostics_snapshot": latest_draft.get(
                                "diagnostics_snapshot"
                            ),
                        }
                        answer_state = "answered"
                if requested_target in OUTBOUND_TALENTS:
                    consent = _support_consent_state(_today_day())
                    if consent == "none":
                        # First outbound dispatch this conversation: intercept it
                        # into an offer. The model proposed support; code gates the
                        # consequence so the offer always precedes the engage.
                        message_text = CHAT_OFFER_SUPPORT_PROMPT
                        offer = {"kind": "support"}
                        requested_target = None
                        requested_task = None
                    # consent in {"pending", "confirmed"}: allow the spawn, no offer.
                if requested_target:
                    message_text = _dispatch_ack_text(
                        requested_target,
                        requested_task,
                        message_text,
                    )
                if not message_text and not requested_target:
                    honest_text = _honest_degradation_message()
                    if honest_text is not None:
                        message_text = honest_text
                thinking = _drain_thinking_locked(use_id, message)
                sol_message_fields: dict[str, Any] = {
                    "use_id": logical_use_id,
                    "text": message_text,
                    "notes": parsed["notes"],
                    "requested_target": requested_target,
                    "requested_task": requested_task,
                    "sources": parse_sol_sources(message_text),
                    "answer_state": answer_state,
                }
                if thinking is not None:
                    sol_message_fields["thinking"] = thinking
                if offer is not None:
                    sol_message_fields["offer"] = offer
                if draft is not None:
                    sol_message_fields["draft"] = draft
                origin = trigger.get("origin")
                if origin is not None and not requested_target:
                    sol_message_fields["origin"] = origin
                append_chat_event(
                    "sol_message",
                    **sol_message_fields,
                )
                _current_chat_state["retry_count"] = 0
                _set_current_raw_use_locked(logical_use_id, None)
                if requested_target:
                    dispatch_job = _build_dispatch_job_locked(
                        logical_use_id,
                        requested_target,
                        requested_task,
                        parsed["talent_request"].get("context") or {},
                    )
                    next_actions.append(_clear_current_locked())
                    next_actions.append(_spawn_or_defer_dispatch_locked(dispatch_job))
                else:
                    if not message_text:
                        provider = str(message.get("provider") or "")
                        append_chat_event(
                            "chat_error",
                            reason="provider_response_invalid",
                            use_id=logical_use_id,
                            provider=provider,
                            detail="",
                        )
                        error_payload = {
                            "use_id": logical_use_id,
                            "reason": "provider_response_invalid",
                        }
                    else:
                        finish_payload = {
                            "use_id": logical_use_id,
                            "message": message_text,
                        }
                    if not message_text:
                        _evict_thinking_locked(use_id)
                    next_actions.append(_clear_current_locked())

        elif use_id in _active_talents:
            summary = str(message.get("result") or "").strip()
            next_actions.extend(
                _handle_talent_terminal_locked(
                    use_id,
                    "talent_finished",
                    "summary",
                    summary,
                    terminal_message=message,
                )
            )
        elif _is_superseded_raw_use_id_locked(use_id):
            logger.debug(
                "superseded raw cortex event use_id=%s event=%s reason=%s",
                use_id,
                "finish",
                "raw rotated",
            )
        else:
            if use_id in _reserved_use_ids:
                logger.warning(
                    "unrouteable cortex event use_id=%s event=%s reason=%s",
                    use_id,
                    "finish",
                    "no matching active chat-generate or talent",
                )

    _run_next_actions(next_actions)
    if finish_payload is not None:
        _emit_finish(finish_payload["use_id"], finish_payload["message"])
    if error_payload is not None:
        _emit_error(error_payload["use_id"], error_payload["reason"])


def _on_cortex_error(message: dict[str, Any]) -> None:
    use_id = str(message.get("use_id") or "")
    if not use_id:
        return

    next_actions: list[dict[str, Any] | None] = []
    error_payload: dict[str, Any] | None = None

    with _state_lock:
        if _current_chat_state is not None and use_id == _current_chat_state.get(
            "raw_use_id"
        ):
            logical_use_id = str(_current_chat_use_id)
            reason_code = str(message.get("reason_code") or "unknown")
            provider = str(message.get("provider") or "")
            detail = _normalize_chat_error_detail(message.get("error"))
            _cancel_watchdog_locked(use_id)
            _evict_thinking_locked(use_id)
            append_chat_event(
                "chat_error",
                reason=reason_code,
                use_id=logical_use_id,
                provider=provider,
                detail=detail,
            )
            error_payload = {
                "use_id": logical_use_id,
                "reason": reason_code,
                "provider": provider,
                "detail": detail,
            }
            next_actions.append(_clear_current_locked())
        elif use_id in _active_talents:
            reason = str(message.get("error") or "unknown")
            reason_code = message.get("reason_code") or None
            _evict_thinking_locked(use_id)
            next_actions.extend(
                _handle_talent_terminal_locked(
                    use_id,
                    "talent_errored",
                    "reason",
                    reason,
                    reason_code=reason_code,
                )
            )
        elif _is_superseded_raw_use_id_locked(use_id):
            logger.debug(
                "superseded raw cortex event use_id=%s event=%s reason=%s",
                use_id,
                "error",
                "raw rotated",
            )
        else:
            if use_id in _reserved_use_ids:
                logger.warning(
                    "unrouteable cortex event use_id=%s event=%s reason=%s",
                    use_id,
                    "error",
                    "no matching active chat-generate or talent",
                )

    _run_next_actions(next_actions)
    if error_payload is not None:
        _emit_error(
            error_payload["use_id"],
            error_payload["reason"],
            provider=error_payload.get("provider", ""),
            detail=error_payload.get("detail", ""),
        )


def _handle_talent_terminal_locked(
    use_id: str,
    kind: str,
    result_field_name: str,
    result_value: str,
    *,
    reason_code: str | None = None,
    terminal_message: dict[str, Any] | None = None,
) -> list[dict[str, Any] | None]:
    _cancel_watchdog_locked(use_id)
    talent_state = _active_talents.pop(use_id)
    logical_use_id = str(talent_state["chat_use_id"])
    talent_name = str(talent_state["target"])
    origin = {
        "logical_use_id": logical_use_id,
        "ask": str(talent_state.get("ask") or ""),
    }
    trigger = _talent_terminal_trigger(
        kind,
        use_id,
        talent_name,
        result_field_name,
        result_value,
        reason_code=reason_code,
        origin=origin,
    )
    event_fields: dict[str, Any] = {
        "use_id": use_id,
        "name": talent_name,
        result_field_name: result_value,
    }
    if reason_code:
        event_fields["reason_code"] = reason_code
    if kind == "talent_finished" and terminal_message is not None:
        thinking = _drain_thinking_locked(use_id, terminal_message)
        if thinking is not None:
            event_fields["thinking"] = thinking
    append_chat_event(kind, **event_fields)
    _, _, synth_action = _activate_or_enqueue_trigger_locked(
        trigger,
        dict(talent_state["location"]),
    )
    return [synth_action, _promote_deferred_spawn_locked(_today_day())]


def _run_next_action(action: dict[str, Any] | None) -> None:
    if action is None:
        return
    if action.get("kind") == "chat":
        spawn_result = _spawn_chat_generate(action)
        if not spawn_result.ok:
            _handle_chat_failure(
                action["logical_use_id"],
                spawn_result.reason,
                detail=spawn_result.detail,
            )
        return
    if action.get("kind") == "talent":
        if not _spawn_talent(action):
            _handle_talent_spawn_failure(action)
            return
        with _state_lock:
            _arm_watchdog_locked(
                str(action["use_id"]),
                "talent",
                str(action["logical_use_id"]),
            )


def _run_next_actions(actions: list[dict[str, Any] | None]) -> None:
    for action in actions:
        _run_next_action(action)


def _spawn_chat_generate(action: dict[str, Any]) -> ChatSpawnResult:
    logger.info(
        "starting chat generate logical=%s raw=%s trigger=%s",
        action["logical_use_id"],
        action["raw_use_id"],
        action["trigger"]["type"],
    )
    from solstone.convey.utils import spawn_agent

    config = {
        "app": action["location"]["app"],
        "path": action["location"]["path"],
        "facet": action["location"]["facet"],
        "trigger": action["trigger"],
        "chat_request_use_id": action["logical_use_id"],
    }
    try:
        spawn_agent(
            prompt="",
            name="chat",
            config=config,
            use_id=action["raw_use_id"],
        )
    except CortexSpawnUnavailable as exc:
        return ChatSpawnResult(
            ok=False,
            reason="chat_pipeline_unavailable",
            detail=exc.detail or "",
        )
    except CortexNotClaimed as exc:
        return ChatSpawnResult(
            ok=False,
            reason="chat_pipeline_unavailable",
            detail=exc.detail or "",
        )
    _emit_cortex_event("thinking", use_id=action["logical_use_id"], chat_proxy=True)
    return ChatSpawnResult(ok=True)


DISPATCH_SPAWN_NAMES = {
    # App talents spawn under "app:talent"; dispatch vocabulary stays bare.
    "support": "support:support",
}

# Outbound-tier dispatch vocab — mirrors the talent-config `access_tier: outbound`
# declared in solstone/apps/support/talent/support.md. The first dispatch of one
# of these in a conversation is intercepted into an offer (see _support_consent_state).
# Hardcoded to match this file's LOCKED-enum convention (TARGET_ALIASES); do NOT
# couple chat.py to heavy talent-config loading. Generalize here if more outbound
# talents appear.
OUTBOUND_TALENTS = {"support"}


def _spawn_talent(action: dict[str, Any]) -> bool:
    from solstone.convey.utils import spawn_agent

    prompt = _build_talent_prompt(
        action["target"],
        action["task"],
        action["context"],
        action["location"],
    )
    config = {
        "app": action["location"]["app"],
        "path": action["location"]["path"],
        "facet": action["location"]["facet"],
        "chat_parent_use_id": action["logical_use_id"],
    }
    spawn_name = DISPATCH_SPAWN_NAMES.get(action["target"], action["target"])
    try:
        spawn_agent(
            prompt=prompt,
            name=spawn_name,
            config=config,
            use_id=action["use_id"],
        )
    except CortexSpawnUnavailable:
        return False
    except CortexNotClaimed:
        return False
    _emit_cortex_event("thinking", use_id=action["logical_use_id"], chat_proxy=True)
    return True


def _handle_talent_spawn_failure(action: dict[str, Any]) -> None:
    next_actions: list[dict[str, Any] | None] = []
    with _state_lock:
        use_id = str(action["use_id"])
        _cancel_watchdog_locked(use_id)
        talent_state = _active_talents.pop(use_id, None)
        logical_use_id = str(
            (talent_state or {}).get("chat_use_id") or action["logical_use_id"]
        )
        talent_name = str((talent_state or {}).get("target") or action["target"])
        ask = str((talent_state or {}).get("ask") or "")
        location = dict((talent_state or {}).get("location") or action["location"])
        append_chat_event(
            "talent_errored",
            use_id=use_id,
            name=talent_name,
            reason="unknown",
        )
        trigger = _talent_terminal_trigger(
            "talent_errored",
            use_id,
            talent_name,
            "reason",
            "unknown",
            origin={"logical_use_id": logical_use_id, "ask": ask},
        )
        _, _, synth_action = _activate_or_enqueue_trigger_locked(trigger, location)
        next_actions.extend(
            [synth_action, _promote_deferred_spawn_locked(_today_day())]
        )
    _run_next_actions(next_actions)


def _handle_chat_failure(
    logical_use_id: str,
    reason: str,
    *,
    detail: str = "",
) -> None:
    normalized_detail = _normalize_chat_error_detail(detail)
    next_info: dict[str, Any] | None = None
    with _state_lock:
        append_chat_event(
            "chat_error",
            reason=reason,
            use_id=logical_use_id,
            provider="",
            detail=normalized_detail,
        )
        if _current_chat_use_id == logical_use_id:
            if _current_chat_state is not None:
                _evict_thinking_locked(str(_current_chat_state.get("raw_use_id") or ""))
                _cancel_watchdog_locked(
                    str(_current_chat_state.get("raw_use_id") or "")
                )
            next_info = _clear_current_locked()
    _emit_error(logical_use_id, reason, detail=normalized_detail)
    _run_next_action(next_info)


def _recover_active_talents_locked(day: str) -> None:
    events = read_chat_events(day)
    latest_owner_message: dict[str, Any] | None = None
    latest_sol_message: dict[str, Any] | None = None
    queued_events: dict[str, dict[str, Any]] = {}
    spawned: dict[str, dict[str, Any]] = {}
    latest_parent_kind: str | None = None

    for event in events:
        kind = event.get("kind")
        if kind == "owner_message":
            latest_owner_message = event
            latest_parent_kind = "owner_message"
            continue
        if kind == "sol_message":
            latest_sol_message = event
            latest_parent_kind = "sol_message"
            continue
        if kind == "talent_queued":
            use_id = str(event.get("use_id") or "")
            if use_id:
                queued_events[use_id] = event
            continue
        if kind == "talent_spawned":
            use_id = str(event.get("use_id") or "")
            if not use_id:
                continue
            queued_event = queued_events.get(use_id)
            if queued_event is None and (
                latest_sol_message is None or latest_owner_message is None
            ):
                logger.warning(
                    "skipping active-talent recovery for %s: no parent chat turn",
                    use_id,
                )
                continue
            chat_use_id = str(
                (queued_event or {}).get("chat_use_id")
                or (latest_sol_message or {}).get("use_id")
                or ""
            )
            if not chat_use_id:
                logger.warning(
                    "skipping active-talent recovery for %s: sol_message missing use_id",
                    use_id,
                )
                continue
            location_source = (
                queued_event.get("location")
                if queued_event is not None
                and isinstance(queued_event.get("location"), dict)
                else None
            )
            spawned[use_id] = {
                "chat_use_id": chat_use_id,
                "target": str(event.get("name") or ""),
                "task": str(event.get("task") or ""),
                "trigger": latest_parent_kind or "sol_message",
                "location": (
                    _normalize_location(
                        location_source.get("app"),
                        location_source.get("path"),
                        location_source.get("facet"),
                    )
                    if location_source is not None
                    else _normalize_location(
                        (latest_owner_message or {}).get("app"),
                        (latest_owner_message or {}).get("path"),
                        (latest_owner_message or {}).get("facet"),
                    )
                ),
                "ask": str(
                    (queued_event or {}).get("ask")
                    or (latest_owner_message or {}).get("text")
                    or ""
                ),
            }
            continue
        if kind in {"talent_finished", "talent_errored"}:
            spawned.pop(str(event.get("use_id") or ""), None)
            queued_events.pop(str(event.get("use_id") or ""), None)

    for use_id, state in spawned.items():
        # recovery blind spot: pre-crash reservations are not seen here
        _reserved_use_ids[use_id] = None
        _reserved_use_ids[state["chat_use_id"]] = None
        if use_id in _active_talents:
            continue
        _active_talents[use_id] = state
        logger.info(
            "reactivated talent during recovery",
            extra={"use_id": use_id, "day": day, "trigger": state["trigger"]},
        )
        if use_id not in _watchdog_timers:
            _arm_watchdog_locked(use_id, "talent", state["chat_use_id"])


def _recover_chat_if_needed() -> None:
    day = _today_day()
    start_actions: list[dict[str, Any]] = []

    with _state_lock:
        _recover_active_talents_locked(day)
        while _active_talent_count_for_today_locked() < MAX_ACTIVE_TALENTS:
            promotion = _promote_deferred_spawn_locked(day)
            if promotion is None:
                break
            start_actions.append(promotion)
        if _current_chat_use_id is not None:
            unresolved = None
        else:
            unresolved = find_unresponded_trigger(day)
        if unresolved is not None:
            location = _location_for_trigger(day, unresolved)
            trigger = _trigger_from_stream_event(day, unresolved)
            _, _, start_info = _activate_or_enqueue_trigger_locked(trigger, location)
            if start_info is not None:
                start_actions.append(start_info)

    _run_next_actions(start_actions)


def _activate_or_enqueue_trigger_locked(
    trigger: dict[str, Any],
    location: dict[str, str],
) -> tuple[str, bool, dict[str, Any] | None]:
    if _current_chat_use_id is None:
        logical_use_id = _reserve_use_id_locked()
        return (
            logical_use_id,
            False,
            _activate_current_locked(logical_use_id, trigger, location),
        )
    queued_use_id = _enqueue_trigger_locked(trigger, location)
    return queued_use_id, True, None


def _activate_current_locked(
    logical_use_id: str,
    trigger: dict[str, Any],
    location: dict[str, str],
) -> dict[str, Any]:
    global _current_chat_use_id, _current_chat_state

    raw_use_id = _reserve_use_id_locked()
    _current_chat_use_id = logical_use_id
    _current_chat_state = {
        "raw_use_id": None,
        "raw_use_ids_seen": set(),
        "trigger": dict(trigger),
        "location": dict(location),
        "retry_count": 0,
    }
    _set_current_raw_use_locked(logical_use_id, raw_use_id)
    return _build_spawn_info_locked(logical_use_id)


def _build_spawn_info_locked(logical_use_id: str) -> dict[str, Any]:
    assert _current_chat_state is not None
    return {
        "kind": "chat",
        "logical_use_id": logical_use_id,
        "raw_use_id": str(_current_chat_state["raw_use_id"]),
        "trigger": dict(_current_chat_state["trigger"]),
        "location": dict(_current_chat_state["location"]),
    }


def _dispatch_ack_text(target: str, task: str | None, message_text: str) -> str:
    text = message_text.strip()
    if text:
        return text
    return CHAT_LIVENESS_TASK_FORMAT.format(
        label=chat_stream._talent_label(target, "running"),
        task=task or "",
    ).strip()


def _current_trigger_ask_locked() -> str:
    if _current_chat_state is None:
        return ""
    trigger = _current_chat_state.get("trigger") or {}
    message = trigger.get("message")
    if message:
        return str(message)
    origin = trigger.get("origin")
    if isinstance(origin, dict) and origin.get("ask"):
        return str(origin["ask"])
    return ""


def _build_dispatch_job_locked(
    logical_use_id: str,
    target: str,
    task: str | None,
    context: dict[str, Any],
) -> dict[str, Any]:
    assert _current_chat_state is not None
    return {
        "use_id": _reserve_use_id_locked(),
        "chat_use_id": logical_use_id,
        "target": target,
        "task": task,
        "context": dict(context),
        "location": dict(_current_chat_state["location"]),
        "ask": _current_trigger_ask_locked(),
    }


def _spawn_or_defer_dispatch_locked(job: dict[str, Any]) -> dict[str, Any] | None:
    if _active_talent_count_for_today_locked() >= MAX_ACTIVE_TALENTS:
        append_chat_event(
            "talent_queued",
            use_id=job["use_id"],
            name=job["target"],
            task=job["task"],
            queued_at=now_ms(),
            chat_use_id=job["chat_use_id"],
            ask=job["ask"],
            context=dict(job["context"]),
            location=dict(job["location"]),
        )
        return None
    return _register_talent_spawn_locked(job, started_at=int(str(job["use_id"])))


def _register_talent_spawn_locked(
    job: dict[str, Any],
    *,
    started_at: int,
) -> dict[str, Any]:
    use_id = str(job["use_id"])
    chat_use_id = str(job["chat_use_id"])
    _reserved_use_ids[use_id] = None
    _reserved_use_ids[chat_use_id] = None
    _active_talents[use_id] = {
        "chat_use_id": chat_use_id,
        "target": str(job["target"]),
        "task": job["task"],
        "location": dict(job["location"]),
        "ask": str(job.get("ask") or ""),
    }
    append_chat_event(
        "talent_spawned",
        use_id=use_id,
        name=str(job["target"]),
        task=job["task"],
        started_at=started_at,
    )
    return {
        "kind": "talent",
        "logical_use_id": chat_use_id,
        "target": str(job["target"]),
        "use_id": use_id,
        "task": job["task"],
        "context": dict(job["context"]),
        "location": dict(job["location"]),
    }


def _promote_deferred_spawn_locked(day: str) -> dict[str, Any] | None:
    if _active_talent_count_for_today_locked() >= MAX_ACTIVE_TALENTS:
        return None
    event = _oldest_unpromoted_queued_talent(day)
    if event is None:
        return None
    job = {
        "use_id": str(event["use_id"]),
        "chat_use_id": str(event["chat_use_id"]),
        "target": str(event["name"]),
        "task": event.get("task"),
        "context": dict(event.get("context") or {}),
        "location": _normalize_location(
            (event.get("location") or {}).get("app")
            if isinstance(event.get("location"), dict)
            else "",
            (event.get("location") or {}).get("path")
            if isinstance(event.get("location"), dict)
            else "",
            (event.get("location") or {}).get("facet")
            if isinstance(event.get("location"), dict)
            else "",
        ),
        "ask": str(event.get("ask") or ""),
    }
    return _register_talent_spawn_locked(job, started_at=now_ms())


def _oldest_unpromoted_queued_talent(day: str) -> dict[str, Any] | None:
    queued: dict[str, dict[str, Any]] = {}
    for event in read_chat_events(day):
        use_id = str(event.get("use_id") or "")
        if not use_id:
            continue
        kind = event.get("kind")
        if kind == "talent_queued":
            queued[use_id] = event
            continue
        if kind in {"talent_spawned", "talent_finished", "talent_errored"}:
            queued.pop(use_id, None)
    if not queued:
        return None
    return sorted(
        queued.values(),
        key=lambda event: (
            int(event.get("queued_at", 0) or 0),
            str(event.get("use_id") or ""),
        ),
    )[0]


def _enqueue_trigger_locked(
    trigger: dict[str, Any],
    location: dict[str, str],
) -> str:
    queued = {
        "use_id": _reserve_use_id_locked(),
        "trigger": dict(trigger),
        "location": dict(location),
    }
    _queued_triggers.append(queued)
    append_chat_event("chat_queue_depth", depth=len(_queued_triggers))
    return str(queued["use_id"])


def _pop_next_trigger_locked() -> dict[str, Any] | None:
    if not _queued_triggers:
        return None
    return _queued_triggers.popleft()


def _abandon_raw_use_ids_locked(use_ids: set[str] | None) -> None:
    if not use_ids:
        return
    for use_id in sorted(use_ids):
        _abandoned_raw_use_ids[use_id] = None
    while len(_abandoned_raw_use_ids) > _ABANDONED_RAW_USE_ID_CAP:
        _abandoned_raw_use_ids.pop(next(iter(_abandoned_raw_use_ids)))


def _evict_raw_use_liveness_locked(use_id: str | None) -> None:
    if not use_id:
        return
    _raw_use_liveness.pop(str(use_id), None)


def _record_raw_use_liveness_locked(use_id: str, event_type: str) -> None:
    if not use_id:
        return
    previous = _raw_use_liveness.pop(use_id, None)
    observed_progress_count = (
        previous.observed_progress_count if previous is not None else 0
    )
    if event_type == "progress":
        observed_progress_count += 1
    _raw_use_liveness[use_id] = RawUseLiveness(
        last_event_type=event_type,
        last_seen_ms=now_ms(),
        observed_progress_count=observed_progress_count,
    )
    while len(_raw_use_liveness) > _RAW_USE_LIVENESS_CAP:
        _raw_use_liveness.pop(next(iter(_raw_use_liveness)))


def _chat_timeout_detail(liveness: RawUseLiveness | None) -> str:
    if liveness is None:
        last_event_type = "none"
        observed_progress_count = 0
        elapsed_s = float(_WATCHDOG_TIMEOUTS["chat"])
    else:
        last_event_type = liveness.last_event_type
        observed_progress_count = liveness.observed_progress_count
        elapsed_s = max(0.0, (now_ms() - liveness.last_seen_ms) / 1000.0)
    return _normalize_chat_error_detail(
        "silence "
        f"{elapsed_s:.1f}s; last event {last_event_type}; "
        f"liveness events {observed_progress_count}"
    )


def _clear_current_locked() -> dict[str, Any] | None:
    global _current_chat_use_id, _current_chat_state

    if _current_chat_state is not None:
        raw_use_ids_seen = _current_chat_state.get("raw_use_ids_seen")
        _abandon_raw_use_ids_locked(raw_use_ids_seen)
        if raw_use_ids_seen:
            for use_id in raw_use_ids_seen:
                _evict_raw_use_liveness_locked(str(use_id))
    _current_chat_use_id = None
    _current_chat_state = None
    queued = _pop_next_trigger_locked()
    if queued is None:
        return None

    append_chat_event("chat_queue_depth", depth=len(_queued_triggers))
    return _activate_current_locked(
        str(queued["use_id"]),
        dict(queued["trigger"]),
        dict(queued["location"]),
    )


def _arm_watchdog_locked(use_id: str, kind: str, logical_use_id: str) -> None:
    _cancel_watchdog_locked(use_id)
    timer = threading.Timer(
        _WATCHDOG_TIMEOUTS.get(kind, _DEFAULT_WATCHDOG_SECONDS),
        _on_watchdog_timeout,
        args=(use_id, kind, logical_use_id),
    )
    timer.daemon = True
    _watchdog_timers[use_id] = timer
    timer.start()


def _cancel_watchdog_locked(use_id: str | None) -> None:
    if not use_id:
        return
    timer = _watchdog_timers.pop(str(use_id), None)
    if timer is not None:
        timer.cancel()


def _refresh_watchdog_locked(
    use_id: str,
    kind: str,
    logical_use_id: str,
    event_type: str = "progress",
) -> None:
    if not use_id or use_id not in _watchdog_timers:
        return
    _arm_watchdog_locked(use_id, kind, logical_use_id)
    if kind == "chat":
        _record_raw_use_liveness_locked(use_id, event_type)


def _set_current_raw_use_locked(logical_use_id: str, raw_use_id: str | None) -> None:
    assert _current_chat_state is not None
    old_raw_use_id = str(_current_chat_state.get("raw_use_id") or "")
    _cancel_watchdog_locked(old_raw_use_id)
    if old_raw_use_id and old_raw_use_id != str(raw_use_id or ""):
        _evict_thinking_locked(old_raw_use_id)
    if raw_use_id is not None:
        _current_chat_state["raw_use_ids_seen"].add(str(raw_use_id))
    _current_chat_state["raw_use_id"] = raw_use_id
    if raw_use_id is not None:
        _arm_watchdog_locked(str(raw_use_id), "chat", logical_use_id)


def _is_superseded_raw_use_id_locked(use_id: str) -> bool:
    if _current_chat_state is not None:
        raw_chat_use_id = str(_current_chat_state.get("raw_use_id") or "")
        if use_id == raw_chat_use_id:
            return False
        if use_id in _current_chat_state["raw_use_ids_seen"]:
            return True
    return use_id in _abandoned_raw_use_ids


def _capture_thinking_locked(message: dict[str, Any]) -> None:
    use_id = str(message.get("use_id") or "")
    summary = message.get("summary")
    if not use_id or not isinstance(summary, str) or not summary.strip():
        return
    if not _is_routeable_cortex_use_id_locked(use_id):
        reason = (
            "raw rotated"
            if _is_superseded_raw_use_id_locked(use_id)
            else "no matching active chat-generate or talent"
        )
        logger.debug(
            "dropping late thinking event use_id=%s reason=%s",
            use_id,
            reason,
        )
        return
    _thinking_buffers.setdefault(use_id, []).append(summary)


def _capture_thinking_provider_locked(message: dict[str, Any]) -> None:
    use_id = str(message.get("use_id") or "")
    provider = str(message.get("provider") or "")
    if not use_id or not provider:
        return
    if not _is_routeable_cortex_use_id_locked(use_id):
        return
    _thinking_providers[use_id] = provider


def _drain_thinking_locked(
    use_id: str,
    terminal_message: dict[str, Any],
) -> dict[str, Any] | None:
    parts = _thinking_buffers.pop(use_id, [])
    provider = _thinking_providers.pop(use_id, "")
    _evict_raw_use_liveness_locked(use_id)
    if not parts:
        return None
    usage = terminal_message.get("usage")
    reasoning_tokens = (
        int(usage.get("reasoning_tokens") or 0) if isinstance(usage, dict) else 0
    )
    return {
        "content": "\n\n".join(parts),
        "provider": provider or str(terminal_message.get("provider") or ""),
        "model": str(terminal_message.get("model") or ""),
        "tokens": reasoning_tokens or None,
    }


def _evict_thinking_locked(use_id: str | None) -> None:
    if not use_id:
        return
    _thinking_buffers.pop(str(use_id), None)
    _thinking_providers.pop(str(use_id), None)
    _evict_raw_use_liveness_locked(str(use_id))


def _is_routeable_cortex_use_id_locked(use_id: str) -> bool:
    if _current_chat_state is not None:
        raw_chat_use_id = str(_current_chat_state.get("raw_use_id") or "")
        if use_id == raw_chat_use_id:
            return True
    return use_id in _active_talents


def _on_watchdog_timeout(use_id: str, kind: str, logical_use_id: str) -> None:
    next_actions: list[dict[str, Any] | None] = []
    should_emit = False
    should_cancel = False
    timeout_detail = ""

    with _state_lock:
        liveness_snapshot = _raw_use_liveness.get(use_id)
        _watchdog_timers.pop(use_id, None)
        if kind == "chat":
            _evict_raw_use_liveness_locked(use_id)

        if kind == "chat":
            if _current_chat_use_id != logical_use_id or _current_chat_state is None:
                return
            if str(_current_chat_state.get("raw_use_id") or "") != use_id:
                return
            logger.warning(
                "chat watchdog timed out use_id=%s kind=%s logical_use_id=%s",
                use_id,
                kind,
                logical_use_id,
            )
            timeout_detail = _chat_timeout_detail(liveness_snapshot)
            _evict_thinking_locked(use_id)
            append_chat_event(
                "chat_error",
                reason="chat_timeout",
                use_id=logical_use_id,
                provider="",
                detail=timeout_detail,
            )
            next_actions.append(_clear_current_locked())
            should_emit = True
            should_cancel = True
        elif kind == "talent":
            talent_state = _active_talents.get(use_id)
            if (
                talent_state is None
                or str(talent_state.get("chat_use_id")) != logical_use_id
            ):
                return
            logger.warning(
                "chat watchdog timed out use_id=%s kind=%s logical_use_id=%s",
                use_id,
                kind,
                logical_use_id,
            )
            _evict_thinking_locked(use_id)
            next_actions.extend(
                _handle_talent_terminal_locked(
                    use_id,
                    "talent_errored",
                    "reason",
                    "talent took too long",
                )
            )
            should_emit = False
        else:
            return

    if should_emit:
        _emit_error(logical_use_id, "chat_timeout", detail=timeout_detail)
    if should_cancel:
        _emit_cortex_cancel(use_id)
    _run_next_actions(next_actions)


def _active_talent_count_for_today_locked() -> int:
    return len(reduce_chat_state(_today_day())["active_talents"])


# LOCKED — see cpo/specs/in-flight/chat-schema-tolerance-audit.md
# Spec amendment required to expand. No fuzzy matching, no LLM classification.

# target field — accepted aliases → canonical
TARGET_ALIASES = {
    "read": "read",
    "Read": "read",
    "READ": "read",
    "exec": "exec",
    "execute": "exec",
    "Exec": "exec",
    "EXEC": "exec",
    "support": "support",
    "Support": "support",
    "SUPPORT": "support",
    "reflection": "read",
    "Reflection": "read",
    "REFLECTION": "read",
    "reflect": "read",
}
# Values outside read/exec/support still raise ValueError.

# LOCKED — see scope doc.
# Spec amendment required to expand. No fuzzy matching, no LLM classification.

# opener forms — sentence-start prefix removal; case-insensitive matching.
# trailing forms — literal span removal; seed flash-bridge holding phrase.
CLOSER_STRIP_PATTERNS = {
    "openers": (
        "Let me look up ",
        "Let me check ",
        "Let me find out ",
        "Let me also ",
        "I'll look up ",
        "I'll check ",
        "I'll find ",
        "And one more thing — ",
        "And let me ",
    ),
    "trailing": (" and I'll let you know",),
}

# task field — whitespace trim, then non-empty check
#   coerce: leading/trailing whitespace stripped before non-empty check
#   keep: empty-after-trim still raises

# context field — fix shipped in parallel lode at d03aa3ad
#   (prose → {"hint": str}); this lode ratifies, adds no new context behavior.

# talent_request itself — keep "must be dict or null" strict;
#   total structural violation is a real-bug-guard.
# Chat parser classification record (audit: chat-schema-tolerance-audit, 2026-05-26).
# Pre-change line refs in _parse_chat_result:
#   1035 result non-str/non-dict     : keep      — structural, no recoverable envelope.
#   1038 payload non-object          : keep      — schema requires object.
#   1040 notes non-string            : keep      — field-type contract; notes-list deferred.
#   1044 message non-string/non-null : keep      — field-type contract.
#   1050 talent_request non-dict/null: keep      — spec call-out: keep strict.
#   1053 target non-string           : keep      — aliases apply only after type check.
#   1055 target unknown              : coerce    — TARGET_ALIASES, then raise if unresolved outside read/exec/support.
#   1058 task non-empty              : coerce    — strip whitespace; empty-after-strip raises.
#   1079 context odd shape           : ratified  — d03aa3ad shipped prose fallback; no new behavior.
# Sibling sweep: chat_stream.py ValueErrors guard the state↔disk JSONL/path seam, out of scope.


def _parse_chat_result(result: Any, use_id: str | None = None) -> dict[str, Any]:
    if isinstance(result, str):
        payload = json.loads(result)
    elif isinstance(result, dict):
        payload = result
    else:
        raise ValueError("chat result must be JSON text")

    if not isinstance(payload, dict):
        raise ValueError("chat result must be an object")
    if not isinstance(payload.get("notes"), str):
        raise ValueError("chat result notes must be a string")

    message = payload.get("message")
    if message is not None and not isinstance(message, str):
        raise ValueError("chat result message must be a string or null")

    talent_request = payload.get("talent_request")
    if talent_request is None:
        return {
            "message": message,
            "notes": payload["notes"],
            "talent_request": None,
        }
    if not isinstance(talent_request, dict):
        raise ValueError("chat talent_request must be an object or null")
    target = talent_request.get("target")
    if not isinstance(target, str):
        raise ValueError("chat talent_request.target must be a string")
    raw_target = target
    target = TARGET_ALIASES.get(target, target)
    if target != raw_target:
        logger.debug(
            "chat parser coerced target=%s -> %s (use_id=%s)",
            raw_target,
            target,
            use_id,
        )
    if target not in {"read", "exec", "support"}:
        raise ValueError(f"unknown talent target: {target}")
    task = talent_request.get("task")
    if not isinstance(task, str):
        raise ValueError("chat talent_request.task must be a non-empty string")
    raw_task = task
    task = task.strip()
    if task != raw_task:
        logger.debug(
            "chat parser coerced task whitespace raw=%r -> %r (use_id=%s)",
            raw_task,
            task,
            use_id,
        )
    if not task:
        raise ValueError("chat talent_request.task must be a non-empty string")
    raw_context = talent_request.get("context")
    if raw_context is None:
        context = {}
    elif isinstance(raw_context, str):
        stripped = raw_context.strip()
        if not stripped:
            context = {}
        else:
            # Provider-shaped non-dict context used to raise; now absorbed so a single odd
            # response doesn't fail the turn. Strictness rollback is deliberate.
            try:
                decoded = json.loads(stripped)
            except ValueError:
                context = {"_raw": stripped}
            else:
                context = decoded if isinstance(decoded, dict) else {"_raw": stripped}
    elif isinstance(raw_context, dict):
        # Scope-mandated defensive shim; no confirmed live replay/cache path sends dict context.
        context = raw_context
    else:
        raise ValueError("chat talent_request.context must be a JSON object string")
    normalized_talent_request = {
        "target": target,
        "task": task,
        "context": context,
    }
    return {
        "message": message,
        "notes": payload["notes"],
        "talent_request": normalized_talent_request,
    }


def _compose_terminal_closer(
    exit_mode: str,
    raw_message: str | None,
    *,
    talent_name: str | None = None,
    talent_errored_reason: str | None = None,
    talent_errored_reason_code: str | None = None,
    talent_finished_summary: str | None = None,
) -> str:
    if exit_mode == "loop_exhausted":
        raw_clean = _strip_closer_patterns(raw_message or "")
        if len(raw_clean.split()) >= 15:
            return raw_clean
        if raw_clean:
            return _frame_loop_exhausted_body(raw_clean)

        summary_clean = _strip_closer_patterns(talent_finished_summary or "")
        if summary_clean:
            return _frame_loop_exhausted_body(summary_clean)
        return (
            f"{CHAT_CLOSER_LOOP_EXHAUSTED_PREFIX} {CHAT_CLOSER_DIFFERENT_ANGLE_SUFFIX}"
        )

    if exit_mode == "talent_errored":
        if (
            talent_name in OUTBOUND_TALENTS
            and talent_errored_reason_code in DETERMINISTIC_FAILURE_REASON_CODES
        ):
            return CHAT_CLOSER_SUPPORT_SEND_FAILED
        reason = _clean_talent_errored_reason(talent_errored_reason)
        if reason:
            return CHAT_CLOSER_TALENT_ERRORED_FORMAT.format(reason=reason)
        return CHAT_CLOSER_TALENT_ERRORED_GENERIC

    raise ValueError(f"unknown exit_mode: {exit_mode!r}")


def _frame_loop_exhausted_body(body: str) -> str:
    return (
        f"{CHAT_CLOSER_LOOP_EXHAUSTED_PREFIX} "
        f"{body.strip()} "
        f"{CHAT_CLOSER_DIFFERENT_ANGLE_SUFFIX}"
    ).strip()


def _strip_closer_patterns(text: str) -> str:
    source = str(text or "")
    parts = re.split(r"([.!?])", source)
    survivors: list[str] = []
    opener_matches: list[str] = []

    for index in range(0, len(parts), 2):
        sentence = parts[index]
        if sentence == "":
            continue
        if index + 1 < len(parts):
            sentence += parts[index + 1]

        stripped_sentence = sentence.lstrip()
        opener = _matching_closer_opener(stripped_sentence)
        if opener is not None:
            opener_matches.append(opener)
            continue
        survivors.append(sentence)

    stripped = "".join(survivors)
    for opener in opener_matches:
        logger.debug(
            "chat closer stripped opener pattern=%r stripped_output=%r",
            opener,
            _normalize_stripped_closer_output(stripped),
        )

    for pattern in CLOSER_STRIP_PATTERNS["trailing"]:
        regex = re.compile(re.escape(pattern), flags=re.IGNORECASE)
        matches = list(regex.finditer(stripped))
        if not matches:
            continue
        stripped_after = regex.sub("", stripped)
        for _match in matches:
            logger.debug(
                "chat closer stripped trailing pattern=%r stripped_output=%r",
                pattern,
                _normalize_stripped_closer_output(stripped_after),
            )
        stripped = stripped_after

    return _normalize_stripped_closer_output(stripped)


def _normalize_stripped_closer_output(text: str) -> str:
    return re.sub(r"\s+", " ", text).strip()


def _matching_closer_opener(sentence: str) -> str | None:
    lowered = sentence.lower()
    for opener in CLOSER_STRIP_PATTERNS["openers"]:
        if lowered.startswith(opener.lower()):
            return opener
    return None


_TRACEBACK_SENTINEL = "Traceback (most recent call last)"
_PYTHON_PATH_RE = re.compile(r"/[A-Za-z0-9_./-]+\.py")


def _clean_talent_errored_reason(reason: str | None) -> str | None:
    reason_clean = re.sub(r"\s+", " ", str(reason or "")).strip()
    if not reason_clean:
        return None
    if len(reason_clean) > 160:
        return None
    if _TRACEBACK_SENTINEL in reason_clean:
        return None
    if _PYTHON_PATH_RE.search(reason_clean):
        return None
    reason_clean = reason_clean.rstrip(".!?")
    return reason_clean or None


def _build_talent_prompt(
    target: str,
    task: str,
    context_hints: dict[str, Any],
    location: dict[str, str],
) -> str:
    parts = [f"Task: {task}"]
    if context_hints:
        parts.append(
            "Context hints:\n" + pprint.pformat(context_hints, sort_dicts=True)
        )
    parts.append(
        "Location: "
        f"app={location['app']} path={location['path']} facet={location['facet']}"
    )

    history_lines: list[str] = []
    for event in read_chat_events(_today_day()):
        kind = event.get("kind")
        if kind == "owner_message":
            history_lines.append(f"**Owner**: {event['text']}")
        elif kind == "sol_message":
            history_lines.append(f"**sol**: {event['text']}")
    if history_lines:
        parts.append("Recent chat:\n" + "\n".join(history_lines[-6:]))

    if target != "exec":
        parts.append(f"Target: {target}")

    return "\n\n".join(parts)


def _emit_finish(use_id: str, message: str) -> None:
    _emit_cortex_event(
        "finish",
        use_id=use_id,
        result=message,
        chat_proxy=True,
    )


def _emit_error(
    use_id: str,
    reason: str,
    *,
    provider: str = "",
    detail: str = "",
) -> None:
    _emit_cortex_event(
        "error",
        use_id=use_id,
        error=reason,
        provider=provider,
        detail=detail,
        chat_proxy=True,
    )


def _emit_cortex_cancel(use_id: str) -> None:
    try:
        _emit_cortex_event(
            "cancel",
            use_id=use_id,
            reason_code=_CORTEX_CANCEL_REASON_CODE,
        )
    except Exception:
        logger.exception("failed to emit cortex cancel use_id=%s", use_id)


def _emit_cortex_event(event: str, **fields: Any) -> None:
    runtime = _runtime
    if runtime is not None and runtime.callosum.emit("cortex", event, **fields):
        return
    callosum_send("cortex", event, **fields)


def _normalize_location(app_name: Any, path: Any, facet: Any) -> dict[str, str]:
    return {
        "app": str(app_name or ""),
        "path": str(path or ""),
        "facet": str(facet or ""),
    }


def _location_for_trigger(day: str, trigger: dict[str, Any]) -> dict[str, str]:
    if trigger.get("kind") == "owner_message":
        return _normalize_location(
            trigger.get("app"),
            trigger.get("path"),
            trigger.get("facet"),
        )
    for event in reversed(read_chat_events(day)):
        if event.get("kind") == "owner_message":
            return _normalize_location(
                event.get("app"),
                event.get("path"),
                event.get("facet"),
            )
    return _normalize_location("", "", "")


def _trigger_from_stream_event(day: str, event: dict[str, Any]) -> dict[str, Any]:
    kind = event.get("kind")
    if kind == "owner_message":
        return {"type": "owner_message", "message": event.get("text", "")}
    if kind == KIND_SOL_CHAT_REQUEST:
        return {
            "type": KIND_SOL_CHAT_REQUEST,
            "summary": event.get("summary", ""),
            "message": event.get("message"),
            "category": event.get("category", ""),
            "since_ts": event.get("since_ts"),
            "trigger_talent": event.get("trigger_talent", ""),
            "request_id": event.get("request_id", ""),
        }
    if kind == "talent_finished":
        return _talent_terminal_trigger(
            "talent_finished",
            event.get("use_id"),
            event.get("name", "exec"),
            "summary",
            event.get("summary", ""),
            origin=_reconstruct_origin_for_terminal(day, event),
        )
    if kind == "talent_errored":
        return _talent_terminal_trigger(
            "talent_errored",
            event.get("use_id"),
            event.get("name", "exec"),
            "reason",
            event.get("reason", ""),
            reason_code=event.get("reason_code"),
            origin=_reconstruct_origin_for_terminal(day, event),
        )
    raise ValueError(f"unsupported trigger event: {kind}")


def _reconstruct_origin_for_terminal(
    day: str,
    terminal_event: dict[str, Any],
) -> dict[str, str] | None:
    terminal_use_id = str(terminal_event.get("use_id") or "")
    if not terminal_use_id:
        return None

    latest_owner_message = ""
    latest_dispatch_origin: dict[str, str] | None = None
    origins_by_talent_use_id: dict[str, dict[str, str]] = {}

    for event in read_chat_events(day):
        kind = event.get("kind")
        use_id = str(event.get("use_id") or "")

        if event is terminal_event or (
            kind == terminal_event.get("kind")
            and use_id == terminal_use_id
            and event.get("ts") == terminal_event.get("ts")
        ):
            return origins_by_talent_use_id.get(terminal_use_id)

        if kind == "owner_message":
            latest_owner_message = str(event.get("text") or "")
            continue

        if kind == "sol_message":
            if event.get("requested_target") is not None:
                latest_dispatch_origin = {
                    "logical_use_id": str(event.get("use_id") or ""),
                    "ask": latest_owner_message,
                }
            continue

        if kind == "talent_queued" and use_id:
            origins_by_talent_use_id[use_id] = {
                "logical_use_id": str(event.get("chat_use_id") or ""),
                "ask": str(event.get("ask") or ""),
            }
            continue

        if kind == "talent_spawned" and use_id and latest_dispatch_origin is not None:
            origins_by_talent_use_id.setdefault(use_id, latest_dispatch_origin)

    return origins_by_talent_use_id.get(terminal_use_id)


def _talent_terminal_trigger(
    kind: str,
    use_id: Any,
    name: Any,
    result_field_name: str,
    result_value: Any,
    *,
    reason_code: str | None = None,
    origin: dict[str, str] | None = None,
) -> dict[str, Any]:
    trigger = {
        "type": kind,
        "use_id": use_id,
        "name": name,
        result_field_name: result_value,
    }
    if reason_code:
        trigger["reason_code"] = reason_code
    if origin is not None:
        trigger["origin"] = dict(origin)
    return trigger


def _read_talent_log(use_id: str) -> dict[str, Any] | None:
    log_path = _find_talent_log_path(use_id)
    if log_path is None:
        return None

    request_event: dict[str, Any] | None = None
    events: list[dict[str, Any]] = []
    started_at: int | None = None
    finished_at: int | None = None

    for index, event in enumerate(_read_jsonl_events(log_path)):
        event_type = str(event.get("event") or "").strip()
        if index == 0 and event_type == "request":
            request_event = event
            continue
        if request_event is None and event_type == "request":
            request_event = event
            continue

        event.pop("raw", None)
        events.append(event)

        event_ts = _event_ts(event)
        if event_type == "start" and started_at is None:
            started_at = event_ts
        elif event_type == "finish":
            finished_at = event_ts
        elif event_type == "error":
            finished_at = event_ts

    request_ts = _event_ts(request_event)
    task = None
    if request_event is not None:
        task = request_event.get("task") or request_event.get("prompt")
    if started_at is None:
        started_at = request_ts

    last_event_type = str(events[-1].get("event") or "").strip() if events else ""
    if last_event_type == "finish":
        status = "completed"
    elif last_event_type == "error":
        status = "errored"
    else:
        status = "running"

    return {
        "use_id": use_id,
        "status": status,
        "task": task,
        "started_at": started_at,
        "finished_at": finished_at,
        "events": events,
    }


def _find_talent_log_path(use_id: str) -> Path | None:
    talents_dir = Path(get_journal()) / "talents"
    if not talents_dir.is_dir():
        return None

    for pattern in (f"*/{use_id}_active.jsonl", f"*/{use_id}.jsonl"):
        matches = sorted(talents_dir.glob(pattern))
        if matches:
            return matches[0]
    return None


def _read_jsonl_events(path: Path) -> list[dict[str, Any]]:
    parsed: list[dict[str, Any]] = []
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if not line:
                continue
            try:
                parsed.append(json.loads(line))
            except json.JSONDecodeError:
                continue
    return parsed


def _event_ts(event: dict[str, Any] | None) -> int | None:
    if event is None:
        return None
    value = event.get("ts")
    return value if isinstance(value, int) else None


def _reserve_use_id_locked() -> str:
    global _last_use_id

    ts = now_ms()
    if ts <= _last_use_id:
        ts = _last_use_id + 1
    _last_use_id = ts
    use_id = str(ts)
    _reserved_use_ids[use_id] = None
    while len(_reserved_use_ids) > _RESERVED_USE_ID_CAP:
        _reserved_use_ids.pop(next(iter(_reserved_use_ids)))
    return use_id


def _today_day() -> str:
    return datetime.now().strftime("%Y%m%d")


def _resolve_support_draft(draft_id: str) -> tuple[dict[str, Any], str] | None:
    indexed_day = chat_stream.resolve_draft_day(draft_id)
    if indexed_day is not None:
        for event in read_chat_events(indexed_day):
            if (
                event.get("kind") == "support_draft"
                and str(event.get("draft_id") or "") == draft_id
            ):
                return event, str(event["captured_day"])

    today = _today_day()
    yesterday = (datetime.now() - timedelta(days=1)).strftime("%Y%m%d")
    for day in (today, yesterday):
        for event in read_chat_events(day):
            if (
                event.get("kind") == "support_draft"
                and str(event.get("draft_id") or "") == draft_id
            ):
                return event, str(event["captured_day"])
    return None


def _draft_is_terminal(events: list[dict[str, Any]], draft_id: str) -> bool:
    for event in events:
        if str(event.get("draft_id") or "") != draft_id:
            continue
        if event.get("kind") == "result":
            return True
        if event.get("kind") == "support_submit_claim" and "generation" not in event:
            return True
    return False


def _support_draft_result_response(result: dict[str, Any]) -> dict[str, Any]:
    """Turn a persisted terminal result event into the confirm response shape."""
    response: dict[str, Any] = {
        "ok": bool(result.get("ok")),
        "outcome": str(
            result.get("outcome") or ("submitted" if result.get("ok") else "failed")
        ),
    }
    if response["outcome"] == "submitted" and "ticket_id" in result:
        response["ticket_id"] = result["ticket_id"]
    return response


def _next_chat_ts_for_day(day: str, events: list[dict[str, Any]]) -> int:
    day_start = datetime.strptime(day, "%Y%m%d")
    start_ms = int(day_start.timestamp() * 1000)
    end_ms = int((day_start + timedelta(days=1)).timestamp() * 1000) - 1
    max_ts = max(
        (
            event["ts"]
            for event in events
            if isinstance(event.get("ts"), int)
            and chat_stream.day_for_ts(int(event["ts"])) == day
        ),
        default=start_ms,
    )
    return min(max_ts + 1, end_ms)


def _submit_support_draft(
    draft_event: dict[str, Any],
    draft_id: str,
) -> SupportDraftSubmitResult:
    verb = str(draft_event["verb"])
    payload = draft_event["payload"]
    diagnostics_snapshot = draft_event["diagnostics_snapshot"]

    try:
        if verb == "create":
            result_obj = support_create(**payload, action_id=draft_id)
            ticket_id = result_obj.get("id", result_obj.get("ticket_id"))
        elif verb == "feedback":
            result_obj = support_feedback(
                body=payload["body"],
                product=payload["product"],
                anonymous=payload["anonymous"],
                user_context=diagnostics_snapshot,
                action_id=draft_id,
            )
            ticket_id = result_obj.get("id", result_obj.get("ticket_id"))
        elif verb == "reply":
            support_reply(payload["ticket_id"], payload["content"], action_id=draft_id)
            ticket_id = payload["ticket_id"]
        elif verb == "attach":
            import base64
            import tempfile
            from pathlib import Path as AttachmentPath

            suffix = AttachmentPath(payload["filename"]).suffix.lower()
            data = base64.b64decode(payload["content_b64"])
            with tempfile.NamedTemporaryFile(suffix=suffix, delete=False) as tmp:
                tmp.write(data)
                tmp_path = AttachmentPath(tmp.name)
            try:
                support_attach(
                    payload["ticket_id"],
                    str(tmp_path),
                    filename=payload["filename"],
                    action_id=draft_id,
                )
            finally:
                tmp_path.unlink(missing_ok=True)
            ticket_id = payload["ticket_id"]
        elif verb == "close":
            support_close(payload["ticket_id"], action_id=draft_id)
            ticket_id = payload["ticket_id"]
        elif verb == "resolved":
            support_resolved(payload["ticket_id"], action_id=draft_id)
            ticket_id = payload["ticket_id"]
        elif verb == "still_need_help":
            support_still_need_help(payload["ticket_id"], action_id=draft_id)
            ticket_id = payload["ticket_id"]
        else:
            raise ValueError(f"unknown draft verb: {verb}")
    except operations.OperationSupersededError:
        raise
    except operations.OperationInProgressError:
        return _support_submit_status_result(
            draft_id, "in_progress", CHAT_SUPPORT_IN_PROGRESS
        )
    except operations.OperationTosChangedError:
        return _support_submit_status_result(
            draft_id, "re_consent_required", CHAT_SUPPORT_RECONSENT_NEEDED
        )
    except (
        operations.IdempotencyConflictError,
        operations.OperationInvalidStateError,
        operations.OperationRetiredError,
        operations.OperationErasedError,
    ) as exc:
        return _support_submit_terminal_failure_result(draft_id, exc)
    except (httpx.ConnectError, httpx.ConnectTimeout, httpx.PoolTimeout) as exc:
        return _support_submit_exception_result(draft_id, exc, ambiguous=False)
    except httpx.HTTPStatusError as exc:
        return _support_submit_exception_result(
            draft_id,
            exc,
            ambiguous=exc.response.status_code >= 500,
        )
    except RuntimeError as exc:
        return _support_submit_exception_result(draft_id, exc, ambiguous=False)
    except (httpx.ReadTimeout, httpx.WriteTimeout) as exc:
        return _support_submit_exception_result(draft_id, exc, ambiguous=True)
    except httpx.HTTPError as exc:
        return _support_submit_exception_result(draft_id, exc, ambiguous=True)

    if verb == "close":
        text = CHAT_SUPPORT_CLOSE_SUBMITTED
    elif verb == "resolved":
        text = CHAT_SUPPORT_RESOLVED_SUBMITTED
    elif verb == "still_need_help":
        text = CHAT_SUPPORT_STILL_NEED_HELP_SUBMITTED
    elif verb == "attach":
        text = CHAT_SUPPORT_ATTACH_FILED_FORMAT.format(ticket_id=ticket_id)
    else:
        text = CHAT_SUPPORT_SUBMIT_FILED_FORMAT.format(ticket_id=ticket_id)
    return SupportDraftSubmitResult(
        ok=True,
        outcome="submitted",
        text=text,
        ticket_id=ticket_id,
        result_fields={
            "draft_id": draft_id,
            "ok": True,
            "ticket_id": ticket_id,
        },
    )


def _support_submit_status_result(
    draft_id: str, outcome: str, text: str
) -> SupportDraftSubmitResult:
    return SupportDraftSubmitResult(
        ok=False,
        outcome=outcome,
        text=text,
        result_fields={"draft_id": draft_id, "ok": False, "outcome": outcome},
        terminal=False,
    )


def _support_submit_terminal_failure_result(
    draft_id: str, exc: BaseException
) -> SupportDraftSubmitResult:
    logger.warning("Support draft submit failed with %s", exc.__class__.__name__)
    return SupportDraftSubmitResult(
        ok=False,
        outcome="failed",
        text=CHAT_SUPPORT_SUBMIT_FAILED,
        result_fields={
            "draft_id": draft_id,
            "ok": False,
            "error": exc.__class__.__name__,
        },
    )


def _support_submit_exception_result(
    draft_id: str,
    exc: BaseException,
    *,
    ambiguous: bool,
) -> SupportDraftSubmitResult:
    outcome = "ambiguous" if ambiguous else "failed"
    logger.warning(
        "Support draft submit %s with %s",
        outcome,
        exc.__class__.__name__,
        exc_info=True,
    )
    fields: dict[str, Any] = {
        "draft_id": draft_id,
        "ok": False,
        "error": exc.__class__.__name__,
    }
    if ambiguous:
        fields["ambiguous"] = True
    return SupportDraftSubmitResult(
        ok=False,
        outcome=outcome,
        text=CHAT_SUPPORT_SUBMIT_AMBIGUOUS if ambiguous else CHAT_SUPPORT_SUBMIT_FAILED,
        result_fields=fields,
        terminal=False,
    )


def _support_consent_state(day: str) -> str:
    """Deterministic, day-scoped support-consent state for the conversation.

    Day-scoped to match the chat stream / reduce_chat_state granularity. Returns:
      "confirmed" — support already dispatched today (a talent_spawned with
                    name == "support" exists).
      "pending"   — the most recent sol_message carries offer {"kind": "support"}
                    (an offer awaiting the owner's reply) and support has not been
                    dispatched.
      "none"      — otherwise.
    Precedence: confirmed, then pending, else none. Computed from history BEFORE
    the current turn's sol_message is appended.
    """
    latest_sol_message: dict[str, Any] | None = None
    for event in read_chat_events(day):
        kind = event.get("kind")
        if kind == "talent_spawned" and event.get("name") == "support":
            return "confirmed"
        if kind == "sol_message":
            latest_sol_message = event
    if latest_sol_message is not None and latest_sol_message.get("offer") == {
        "kind": "support"
    }:
        return "pending"
    return "none"


def _support_draft_state(day: str) -> str:
    """Deterministic, day-scoped support-draft state for the conversation.

    Mirrors _support_consent_state. Walks history tracking the LATEST support_draft.
    Returns:
      "submitted" — a `result` event back-references the latest draft's draft_id
                    (forward seam; no `result` writer exists yet, so this is
                    present-but-inert today — the next lode adds only that writer).
      "pending"   — a support_draft exists and is not yet submitted.
      "none"      — no support_draft.
    Precedence: submitted, then pending, else none.
    """
    latest_draft_id: str | None = None
    result_draft_ids: set[str] = set()
    for event in read_chat_events(day):
        kind = event.get("kind")
        if kind == "support_draft":
            latest_draft_id = str(event.get("draft_id") or "")
        elif kind == "result":
            result_draft_id = str(event.get("draft_id") or "")
            if result_draft_id:
                result_draft_ids.add(result_draft_id)
    if latest_draft_id and latest_draft_id in result_draft_ids:
        return "submitted"
    if latest_draft_id:
        return "pending"
    return "none"


def _latest_support_draft(day: str) -> dict[str, Any] | None:
    """Return the most recent support_draft event for ``day``, or None."""
    latest: dict[str, Any] | None = None
    for event in read_chat_events(day):
        if event.get("kind") == "support_draft":
            latest = event
    return latest
