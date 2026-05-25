# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Push trigger handlers."""

from __future__ import annotations

import json
import logging
import time
from datetime import datetime
from pathlib import Path
from typing import Any

from solstone.apps.home.routes import _load_briefing_md
from solstone.convey.chat_stream import append_chat_event, read_chat_events
from solstone.convey.sol_initiated.copy import (
    KIND_OWNER_CHAT_DISMISSED,
    KIND_OWNER_CHAT_OPEN,
    KIND_SOL_CHAT_REQUEST,
)
from solstone.think.activities import load_activity_records
from solstone.think.facets import get_enabled_facets
from solstone.think.push.config import get_bundle_id, get_environment, is_configured
from solstone.think.push.devices import load_devices
from solstone.think.push.dispatch import (
    CATEGORY_AGENT_ALERT,
    CATEGORY_DAILY_BRIEFING,
    CATEGORY_PRE_MEETING_PREP,
    build_agent_alert_collapse_id,
    build_agent_alert_payload,
    build_daily_briefing_collapse_id,
    build_daily_briefing_payload,
    build_pre_meeting_collapse_id,
    build_pre_meeting_payload,
    build_silent_chat_lifecycle_collapse_id,
    build_silent_chat_lifecycle_payload,
    build_sol_chat_request_collapse_id,
    build_sol_chat_request_payload,
    send_many,
)
from solstone.think.push.portal_dispatch import (
    dispatch_dedup_via_portal,
    dispatch_via_portal,
)
from solstone.think.services.scout import scout_provenance
from solstone.think.utils import get_journal

logger = logging.getLogger("solstone.push.triggers")


def _nudge_log_path() -> Path:
    return Path(get_journal()) / "push" / "nudge_log.jsonl"


def _serialize_dedupe_key(dedupe_key: tuple[Any, ...]) -> str:
    return json.dumps(list(dedupe_key), separators=(",", ":"), ensure_ascii=False)


def _has_nudged(dedupe_key: tuple[Any, ...]) -> bool:
    path = _nudge_log_path()
    if not path.exists():
        return False
    encoded = _serialize_dedupe_key(dedupe_key)
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        try:
            payload = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(payload, dict) and payload.get("dedupe_key") == encoded:
            return True
    return False


def _append_nudge_log(line: dict[str, Any]) -> None:
    path = _nudge_log_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(line, ensure_ascii=False) + "\n")


def _eligible_devices() -> list[dict[str, Any]]:
    if not is_configured():
        logger.debug("push skipped configured=false")
        return []
    bundle_id = get_bundle_id()
    environment = get_environment()
    matched = [
        device
        for device in load_devices()
        if device.get("bundle_id") == bundle_id
        and device.get("environment") == environment
        and device.get("platform") == "ios"
    ]
    if not matched:
        logger.debug("push skipped devices=0")
    return matched


def _metadata_generated(metadata: dict[str, Any] | None) -> str | None:
    if not isinstance(metadata, dict):
        return None
    generated = metadata.get("generated")
    if isinstance(generated, str):
        return generated
    if hasattr(generated, "isoformat"):
        return generated.isoformat()
    return None


def _record_send(
    *,
    dedupe_key: tuple[Any, ...],
    category: str,
    sent: int,
    failed: int,
    **payload: Any,
) -> None:
    _append_nudge_log(
        {
            "ts": int(time.time()),
            "category": category,
            "dedupe_key": _serialize_dedupe_key(dedupe_key),
            "sent": sent,
            "failed": failed,
            **payload,
        }
    )


def _has_reflection_ready_event(day: str, route: str) -> bool:
    today = datetime.now().strftime("%Y%m%d")
    return any(
        event.get("kind") == "reflection_ready"
        and event.get("day") == day
        and event.get("url") == route
        for event in read_chat_events(today)
    )


def handle_briefing_finish(message: dict[str, Any]) -> None:
    if message.get("tract") != "cortex":
        return
    if message.get("event") != "finish":
        return
    if message.get("name") != "morning_briefing":
        return
    today = datetime.now().strftime("%Y%m%d")
    dedupe_key = (CATEGORY_DAILY_BRIEFING, today)
    if _has_nudged(dedupe_key):
        return
    sections: dict[str, str] = {}
    metadata: dict[str, Any] | None = None
    needs_attention: list[str] = []
    for _ in range(10):
        sections, metadata, needs_attention = _load_briefing_md(today)
        if sections and metadata:
            break
        time.sleep(1)
    else:
        logger.warning("push briefing unavailable after finish day=%s", today)
        return
    eligible_devices = _eligible_devices()
    if not eligible_devices:
        return
    sent, failed = send_many(
        eligible_devices,
        build_daily_briefing_payload(
            day=today,
            generated=_metadata_generated(metadata),
            needs_attention_count=len(needs_attention),
        ),
        collapse_id=build_daily_briefing_collapse_id(today),
    )
    if sent > 0:
        _record_send(
            dedupe_key=dedupe_key,
            category=CATEGORY_DAILY_BRIEFING,
            day=today,
            sent=sent,
            failed=failed,
        )


def _parse_start(now: datetime, start: str) -> datetime | None:
    for pattern in ("%H:%M", "%H:%M:%S"):
        try:
            parsed = datetime.strptime(start, pattern)
        except ValueError:
            continue
        return now.replace(
            hour=parsed.hour,
            minute=parsed.minute,
            second=parsed.second,
            microsecond=0,
        )
    return None


def check_pre_meeting_prep(now: datetime) -> None:
    today = now.strftime("%Y%m%d")
    eligible_devices = _eligible_devices()
    if not eligible_devices:
        return
    for facet in get_enabled_facets().keys():
        for record in load_activity_records(facet, today):
            if record.get("source") != "anticipated":
                continue
            activity_id = str(record.get("id") or "").strip()
            start = str(record.get("start") or "").strip()
            if not activity_id or not start:
                continue
            event_start = _parse_start(now, start)
            if event_start is None:
                logger.debug("push skipped invalid meeting start id=%s", activity_id)
                continue
            seconds_until = (event_start - now).total_seconds()
            if seconds_until < 14 * 60 or seconds_until > 16 * 60:
                continue
            dedupe_key = (CATEGORY_PRE_MEETING_PREP, activity_id, today)
            if _has_nudged(dedupe_key):
                continue
            sent, failed = send_many(
                eligible_devices,
                build_pre_meeting_payload(activity=record, facet=facet, day=today),
                collapse_id=build_pre_meeting_collapse_id(activity_id),
            )
            if sent > 0:
                _record_send(
                    dedupe_key=dedupe_key,
                    category=CATEGORY_PRE_MEETING_PREP,
                    day=today,
                    facet=facet,
                    activity_id=activity_id,
                    sent=sent,
                    failed=failed,
                )


def send_agent_alert(
    *, title: str, body: str, context_id: str, route: str | None = None
) -> tuple[int, int]:
    dedupe_key = (CATEGORY_AGENT_ALERT, context_id)
    if _has_nudged(dedupe_key):
        return 0, 0
    eligible_devices = _eligible_devices()
    if not eligible_devices:
        return 0, 0
    sent, failed = send_many(
        eligible_devices,
        build_agent_alert_payload(
            title=title,
            body=body,
            context_id=context_id,
            route=route,
        ),
        collapse_id=build_agent_alert_collapse_id(context_id),
    )
    if sent > 0:
        _record_send(
            dedupe_key=dedupe_key,
            category=CATEGORY_AGENT_ALERT,
            context_id=context_id,
            sent=sent,
            failed=failed,
        )
    return sent, failed


def handle_weekly_reflection_finish(message: dict[str, Any]) -> None:
    if message.get("tract") != "cortex":
        return
    if message.get("event") != "finish":
        return
    if message.get("name") != "weekly_reflection":
        return

    day = str(message.get("day") or "").strip()
    if not day:
        return

    context_id = f"weekly_reflection:{day}"
    dedupe_key = (CATEGORY_AGENT_ALERT, context_id)
    if _has_nudged(dedupe_key):
        return

    reflection_path = Path(get_journal()) / "reflections" / "weekly" / f"{day}.md"
    for _ in range(10):
        if reflection_path.is_file():
            break
        time.sleep(1)
    else:
        logger.warning("push weekly reflection unavailable after finish day=%s", day)
        return

    route = f"/app/reflections/{day}"
    send_agent_alert(
        title="your week is ready",
        body="",
        context_id=context_id,
        route=route,
    )
    if not _has_reflection_ready_event(day, route):
        append_chat_event("reflection_ready", day=day, url=route)


def handle_sol_chat_request(message: dict[str, Any]) -> None:
    if message.get("tract") != "chat" or message.get("event") != KIND_SOL_CHAT_REQUEST:
        return
    request_id = str(message.get("request_id") or "").strip()
    if not request_id:
        return
    summary = str(message.get("summary") or "")
    category = str(message.get("category") or "")

    scout = scout_provenance()
    if scout and scout.get("dispatch_token"):
        portal_result = dispatch_via_portal(
            request_id=request_id,
            summary=summary,
            category=category,
        )
        if portal_result is not None:
            _append_nudge_log(
                {
                    "ts": int(time.time()),
                    "kind": f"{KIND_SOL_CHAT_REQUEST}_push",
                    "dedupe_key": request_id,
                    "category": category,
                    "outcome": "dispatched",
                    "via": "portal",
                }
            )
            return

    if not is_configured():
        return
    eligible_devices = _eligible_devices()
    if not eligible_devices:
        return

    outcome = "dispatched"
    try:
        sent, failed = send_many(
            eligible_devices,
            build_sol_chat_request_payload(
                request_id=request_id,
                summary=summary,
                category=category,
            ),
            collapse_id=build_sol_chat_request_collapse_id(request_id=request_id),
            priority=10,
        )
        if failed:
            logger.warning(
                "sol chat request push had failures request_id=%s sent=%s failed=%s",
                request_id,
                sent,
                failed,
            )
            outcome = "error"
    except Exception:
        logger.warning("sol chat request push dispatch failed", exc_info=True)
        outcome = "error"

    _append_nudge_log(
        {
            "ts": int(time.time()),
            "kind": f"{KIND_SOL_CHAT_REQUEST}_push",
            "dedupe_key": request_id,
            "category": category,
            "outcome": outcome,
            "via": "local",
        }
    )


def handle_chat_lifecycle(message: dict[str, Any]) -> None:
    if message.get("tract") != "chat":
        return
    event = message.get("event")
    if event not in {KIND_OWNER_CHAT_OPEN, KIND_OWNER_CHAT_DISMISSED}:
        return
    raw_request_id = message.get("request_id")
    request_id = raw_request_id.strip() if isinstance(raw_request_id, str) else ""
    if not request_id:
        return

    scout = scout_provenance()
    if scout and scout.get("dispatch_token"):
        portal_result = dispatch_dedup_via_portal(request_id=request_id, action=event)
        if portal_result is not None:
            _append_nudge_log(
                {
                    "ts": int(time.time()),
                    "kind": "sol_chat_lifecycle_push",
                    "dedupe_key": request_id,
                    "category": event,
                    "outcome": "dispatched",
                    "via": "portal",
                }
            )
            return

    if not is_configured():
        return
    eligible_devices = _eligible_devices()
    if not eligible_devices:
        return

    outcome = "dispatched"
    try:
        sent, failed = send_many(
            eligible_devices,
            build_silent_chat_lifecycle_payload(request_id=request_id, action=event),
            collapse_id=build_silent_chat_lifecycle_collapse_id(
                request_id=request_id,
                action=event,
            ),
            priority=5,
            push_type="background",
        )
        if failed:
            logger.warning(
                "sol chat lifecycle push had failures request_id=%s event=%s sent=%s failed=%s",
                request_id,
                event,
                sent,
                failed,
            )
            outcome = "error"
    except Exception:
        logger.warning("sol chat lifecycle push dispatch failed", exc_info=True)
        outcome = "error"

    _append_nudge_log(
        {
            "ts": int(time.time()),
            "kind": "sol_chat_lifecycle_push",
            "dedupe_key": request_id,
            "category": event,
            "outcome": outcome,
            "via": "local",
        }
    )


__all__ = [
    "check_pre_meeting_prep",
    "handle_briefing_finish",
    "handle_chat_lifecycle",
    "handle_sol_chat_" + "request",
    "handle_weekly_reflection_finish",
    "send_agent_alert",
]
