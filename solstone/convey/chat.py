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
import threading
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any

from flask import Blueprint, jsonify, request

from solstone.convey.chat_stream import (
    append_chat_event,
    find_unresponded_trigger,
    read_chat_events,
    reduce_chat_state,
)
from solstone.convey.reasons import (
    AGENT_UNAVAILABLE,
    MISSING_REQUIRED_FIELD,
    TALENT_NOT_FOUND,
)
from solstone.convey.sol_initiated import (
    record_owner_chat_dismissed,
    record_owner_chat_open,
)
from solstone.convey.sol_initiated.copy import KIND_SOL_CHAT_REQUEST, SURFACE_CONVEY
from solstone.convey.utils import error_response
from solstone.think.callosum import CallosumConnection, callosum_send
from solstone.think.cortex_client import CortexSpawnUnavailable
from solstone.think.utils import get_journal, now_ms

logger = logging.getLogger(__name__)

chat_bp = Blueprint("chat", __name__, url_prefix="/api/chat")

MAX_ACTIVE_TALENTS = 2
MAX_LOOP_RETRIES = 3
_WATCHDOG_TIMEOUTS = {"chat": 30, "talent": 180}
_DEFAULT_WATCHDOG_SECONDS = 180
_RESERVED_USE_ID_CAP = 256
MAX_ACTIVE_REASON = "max active — waiting for one to finish"

_state_lock = threading.Lock()
_runtime_lock = threading.Lock()
_current_chat_use_id: str | None = None
_current_chat_state: dict[str, Any] | None = None
_queued_trigger: dict[str, Any] | None = None
_active_talents: dict[str, dict[str, Any]] = {}
_reserved_use_ids: dict[str, None] = {}
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
class ChatSpawnResult:
    ok: bool
    reason: str = ""
    detail: str = ""


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
    append_chat_event("owner_message", **event_fields)
    trigger = {
        "type": "owner_message",
        "message": message,
    }

    start_info: dict[str, Any] | None = None
    with _state_lock:
        if _current_chat_use_id is None:
            logical_use_id = _reserve_use_id_locked()
            start_info = _activate_current_locked(logical_use_id, trigger, location)
            queued = False
            response_use_id = logical_use_id
        else:
            response_use_id = _queue_trigger_locked(trigger, location)
            queued = True

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

    return jsonify(use_id=response_use_id, queued=queued)


@chat_bp.route(f"/{KIND_SOL_CHAT_REQUEST}/open", methods=["POST"])
def sol_chat_request_open() -> Any:
    """Record that the owner opened a sol-initiated chat request."""
    payload = request.get_json(force=True, silent=True) or {}
    request_id = str(payload.get("request_id") or "").strip()
    if not request_id:
        return error_response(MISSING_REQUIRED_FIELD, detail="request_id required")
    record_owner_chat_open(request_id, surface=SURFACE_CONVEY)
    return jsonify({"ok": True})


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
    if event_type == "finish":
        _on_cortex_finish(message)
        return
    if event_type == "error":
        _on_cortex_error(message)
        return

    _proxy_progress(message)


def _proxy_progress(message: dict[str, Any]) -> None:
    # Cortex listens on tract=cortex, event=request without checking chat_proxy
    # (cortex.py:199-203). Re-emitting request would spawn a duplicate talent.
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
            _refresh_watchdog_locked(use_id, "chat", str(_current_chat_use_id))
        elif use_id in _active_talents:
            logical_use_id = str(_active_talents[use_id]["chat_use_id"])
            _refresh_watchdog_locked(use_id, "talent", logical_use_id)
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


def _on_cortex_finish(message: dict[str, Any]) -> None:
    use_id = str(message.get("use_id") or "")
    if not use_id:
        return

    next_info: dict[str, Any] | None = None
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
                    next_info = _build_spawn_info_locked(logical_use_id)
                else:
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
                    next_info = _clear_current_locked()
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
                append_chat_event(
                    "sol_message",
                    use_id=logical_use_id,
                    text=message_text,
                    notes=parsed["notes"],
                    requested_target=requested_target,
                    requested_task=requested_task,
                )
                _current_chat_state["retry_count"] = 0
                _set_current_raw_use_locked(logical_use_id, None)
                if requested_target:
                    active_talent_count = _active_talent_count_for_today_locked()
                    if active_talent_count >= MAX_ACTIVE_TALENTS:
                        _current_chat_state["trigger"] = {
                            "type": "synthetic-max-active",
                            "reason": MAX_ACTIVE_REASON,
                        }
                        synthetic_use_id = _reserve_use_id_locked()
                        _set_current_raw_use_locked(logical_use_id, synthetic_use_id)
                        next_info = _build_spawn_info_locked(logical_use_id)
                    elif _talent_loop_count_locked() >= MAX_LOOP_RETRIES:
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
                        next_info = _clear_current_locked()
                    else:
                        talent_use_id = _reserve_use_id_locked()
                        _active_talents[talent_use_id] = {
                            "chat_use_id": logical_use_id,
                            "target": requested_target,
                            "task": requested_task,
                            "location": dict(_current_chat_state["location"]),
                        }
                        append_chat_event(
                            "talent_spawned",
                            use_id=talent_use_id,
                            name=requested_target,
                            task=requested_task,
                            started_at=int(talent_use_id),
                        )
                        next_info = {
                            "kind": "talent",
                            "logical_use_id": logical_use_id,
                            "target": requested_target,
                            "use_id": talent_use_id,
                            "task": requested_task,
                            "context": parsed["talent_request"].get("context") or {},
                            "location": dict(_current_chat_state["location"]),
                        }
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
                    next_info = _clear_current_locked()

        elif use_id in _active_talents:
            summary = str(message.get("result") or "").strip()
            next_info = _handle_talent_terminal_locked(
                use_id,
                "talent_finished",
                "summary",
                summary,
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

    _run_next_action(next_info)
    if finish_payload is not None:
        _emit_finish(finish_payload["use_id"], finish_payload["message"])
    if error_payload is not None:
        _emit_error(error_payload["use_id"], error_payload["reason"])


def _on_cortex_error(message: dict[str, Any]) -> None:
    use_id = str(message.get("use_id") or "")
    if not use_id:
        return

    next_info: dict[str, Any] | None = None
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
            next_info = _clear_current_locked()
        elif use_id in _active_talents:
            reason = str(message.get("error") or "unknown")
            next_info = _handle_talent_terminal_locked(
                use_id,
                "talent_errored",
                "reason",
                reason,
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

    _run_next_action(next_info)
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
) -> dict[str, Any] | None:
    _cancel_watchdog_locked(use_id)
    talent_state = _active_talents.pop(use_id)
    logical_use_id = str(talent_state["chat_use_id"])
    talent_name = str(talent_state["target"])
    trigger = _talent_terminal_trigger(
        kind,
        use_id,
        talent_name,
        result_field_name,
        result_value,
    )
    append_chat_event(
        kind,
        use_id=use_id,
        name=talent_name,
        **{result_field_name: result_value},
    )
    if _current_chat_use_id != logical_use_id or _current_chat_state is None:
        return None

    _current_chat_state["trigger"] = trigger
    _set_current_raw_use_locked(
        logical_use_id,
        _reserve_use_id_locked(),
    )
    _current_chat_state["retry_count"] = 0
    return _build_spawn_info_locked(logical_use_id)


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
        use_id = spawn_agent(
            prompt="",
            name="chat",
            provider=None,
            config=config,
            use_id=action["raw_use_id"],
        )
    except CortexSpawnUnavailable as exc:
        return ChatSpawnResult(
            ok=False,
            reason="chat_pipeline_unavailable",
            detail=exc.detail or "",
        )
    if use_id is None:
        return ChatSpawnResult(ok=False, reason="unknown")
    _emit_cortex_event("thinking", use_id=action["logical_use_id"], chat_proxy=True)
    return ChatSpawnResult(ok=True)


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
    try:
        use_id = spawn_agent(
            prompt=prompt,
            name=action["target"],
            provider=None,
            config=config,
            use_id=action["use_id"],
        )
    except CortexSpawnUnavailable:
        return False
    if use_id is None:
        return False
    _emit_cortex_event("thinking", use_id=action["logical_use_id"], chat_proxy=True)
    return True


def _handle_talent_spawn_failure(action: dict[str, Any]) -> None:
    next_info: dict[str, Any] | None = None
    with _state_lock:
        _cancel_watchdog_locked(str(action["use_id"]))
        _active_talents.pop(str(action["use_id"]), None)
        append_chat_event(
            "talent_errored",
            use_id=action["use_id"],
            name=action["target"],
            reason="unknown",
        )
        if _current_chat_use_id == action["logical_use_id"] and _current_chat_state:
            _current_chat_state["trigger"] = {
                "type": "talent_errored",
                "use_id": action["use_id"],
                "name": action["target"],
                "reason": "unknown",
            }
            _set_current_raw_use_locked(
                str(action["logical_use_id"]),
                _reserve_use_id_locked(),
            )
            _current_chat_state["retry_count"] = 0
            next_info = _build_spawn_info_locked(action["logical_use_id"])
    _run_next_action(next_info)


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
        if kind == "talent_spawned":
            use_id = str(event.get("use_id") or "")
            if not use_id:
                continue
            if latest_sol_message is None or latest_owner_message is None:
                logger.warning(
                    "skipping active-talent recovery for %s: no parent chat turn",
                    use_id,
                )
                continue
            chat_use_id = str(latest_sol_message.get("use_id") or "")
            if not chat_use_id:
                logger.warning(
                    "skipping active-talent recovery for %s: sol_message missing use_id",
                    use_id,
                )
                continue
            spawned[use_id] = {
                "chat_use_id": chat_use_id,
                "target": str(event.get("name") or ""),
                "task": str(event.get("task") or ""),
                "trigger": latest_parent_kind or "sol_message",
                "location": _normalize_location(
                    latest_owner_message.get("app"),
                    latest_owner_message.get("path"),
                    latest_owner_message.get("facet"),
                ),
            }
            continue
        if kind in {"talent_finished", "talent_errored"}:
            spawned.pop(str(event.get("use_id") or ""), None)

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
    start_info: dict[str, Any] | None = None

    with _state_lock:
        _recover_active_talents_locked(day)
        if _current_chat_use_id is not None:
            return
        unresolved = find_unresponded_trigger(day)
        if unresolved is None:
            return
        location = _location_for_trigger(day, unresolved)
        logical_use_id = _reserve_use_id_locked()
        trigger = _trigger_from_stream_event(unresolved)
        start_info = _activate_current_locked(logical_use_id, trigger, location)

    if start_info is not None:
        spawn_result = _spawn_chat_generate(start_info)
        if not spawn_result.ok:
            _handle_chat_failure(
                start_info["logical_use_id"],
                spawn_result.reason,
                detail=spawn_result.detail,
            )


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


def _queue_trigger_locked(trigger: dict[str, Any], location: dict[str, str]) -> str:
    global _queued_trigger
    if _queued_trigger is None:
        _queued_trigger = {
            "use_id": _reserve_use_id_locked(),
            "trigger": dict(trigger),
            "location": dict(location),
        }
    return str(_queued_trigger["use_id"])


def _clear_current_locked() -> dict[str, Any] | None:
    global _current_chat_use_id, _current_chat_state, _queued_trigger

    _current_chat_use_id = None
    _current_chat_state = None
    if _queued_trigger is None:
        return None

    queued = _queued_trigger
    _queued_trigger = None
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


def _refresh_watchdog_locked(use_id: str, kind: str, logical_use_id: str) -> None:
    if not use_id or use_id not in _watchdog_timers:
        return
    _arm_watchdog_locked(use_id, kind, logical_use_id)


def _set_current_raw_use_locked(logical_use_id: str, raw_use_id: str | None) -> None:
    assert _current_chat_state is not None
    _cancel_watchdog_locked(str(_current_chat_state.get("raw_use_id") or ""))
    if raw_use_id is not None:
        _current_chat_state["raw_use_ids_seen"].add(str(raw_use_id))
    _current_chat_state["raw_use_id"] = raw_use_id
    if raw_use_id is not None:
        _arm_watchdog_locked(str(raw_use_id), "chat", logical_use_id)


def _is_superseded_raw_use_id_locked(use_id: str) -> bool:
    if _current_chat_state is None:
        return False
    raw_chat_use_id = str(_current_chat_state.get("raw_use_id") or "")
    if use_id == raw_chat_use_id:
        return False
    return use_id in _current_chat_state["raw_use_ids_seen"]


def _on_watchdog_timeout(use_id: str, kind: str, logical_use_id: str) -> None:
    next_info: dict[str, Any] | None = None
    should_emit = False

    with _state_lock:
        _watchdog_timers.pop(use_id, None)

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
            append_chat_event(
                "chat_error",
                reason="chat_timeout",
                use_id=logical_use_id,
                provider="",
                detail="",
            )
            next_info = _clear_current_locked()
            should_emit = True
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
            append_chat_event(
                "talent_errored",
                use_id=use_id,
                name=str(talent_state["target"]),
                reason="talent took too long",
            )
            _active_talents.pop(use_id, None)
            append_chat_event(
                "chat_error",
                reason="chat_timeout",
                use_id=logical_use_id,
                provider="",
                detail="",
            )
            if (
                _current_chat_use_id == logical_use_id
                and _current_chat_state is not None
                and not _current_chat_state.get("raw_use_id")
            ):
                next_info = _clear_current_locked()
            should_emit = True
        else:
            return

    if should_emit:
        _emit_error(logical_use_id, "chat_timeout")
        _run_next_action(next_info)


def _active_talent_count_for_today_locked() -> int:
    return len(reduce_chat_state(_today_day())["active_talents"])


def _talent_loop_count_locked() -> int:
    """Count trailing redispatch hops for the current owner turn.

    Each hop is a requested-target sol_message paired with the nearest earlier
    talent_finished or talent_errored event. Bookkeeping events between them do
    not break the chain or satisfy a pending hop.
    """
    events = read_chat_events(_today_day())
    count = 0
    pending_redispatch = False

    for event in reversed(events):
        kind = event.get("kind")
        if kind == "owner_message":
            break
        if kind == "sol_message":
            if not event.get("requested_target"):
                continue
            if pending_redispatch:
                break
            pending_redispatch = True
            continue
        if kind in {"talent_finished", "talent_errored"}:
            if pending_redispatch:
                count += 1
                pending_redispatch = False
            continue
    return count


# LOCKED — see cpo/specs/in-flight/chat-schema-tolerance-audit.md
# Spec amendment required to expand. No fuzzy matching, no LLM classification.

# target field — accepted aliases → canonical
TARGET_ALIASES = {
    "exec": "exec",
    "execute": "exec",
    "Exec": "exec",
    "EXEC": "exec",
    "reflection": "reflection",
    "Reflection": "reflection",
    "REFLECTION": "reflection",
    "reflect": "reflection",
}
# Values outside this set still raise ValueError.

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
#   1055 target unknown              : coerce    — TARGET_ALIASES, then raise if unresolved.
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
        return {"message": message, "notes": payload["notes"], "talent_request": None}
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
    if target not in {"exec", "reflection"}:
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
    return {
        "message": message,
        "notes": payload["notes"],
        "talent_request": {
            "target": target,
            "task": task,
            "context": context,
        },
    }


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
            history_lines.append(f"**Sol**: {event['text']}")
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


def _trigger_from_stream_event(event: dict[str, Any]) -> dict[str, Any]:
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
        )
    if kind == "talent_errored":
        return _talent_terminal_trigger(
            "talent_errored",
            event.get("use_id"),
            event.get("name", "exec"),
            "reason",
            event.get("reason", ""),
        )
    raise ValueError(f"unsupported trigger event: {kind}")


def _talent_terminal_trigger(
    kind: str,
    use_id: Any,
    name: Any,
    result_field_name: str,
    result_value: Any,
) -> dict[str, Any]:
    return {
        "type": kind,
        "use_id": use_id,
        "name": name,
        result_field_name: result_value,
    }


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
