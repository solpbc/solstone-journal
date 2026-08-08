# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Flask routes for the support app.

Provides API endpoints consumed by workspace.html and the background service.
"""

from __future__ import annotations

import logging
import time
import uuid
from typing import Any

from flask import Blueprint, jsonify, request

from solstone.convey.chat_stream import append_chat_event, day_for_ts, record_draft_captured
from solstone.convey.reasons import (
    FEATURE_UNAVAILABLE,
    IDEMPOTENCY_CONFLICT,
    INVALID_REQUEST_VALUE,
    MISSING_REQUIRED_FIELD,
    OPERATION_ERASED,
    OPERATION_IN_PROGRESS,
    OPERATION_RETIRED,
    SUPPORT_INVALID_STATE,
    SUPPORT_PORTAL_FAILED,
    SUPPORT_TOS_CHANGED,
)
from solstone.convey.utils import error_response

logger = logging.getLogger(__name__)

support_bp = Blueprint(
    "app:support",
    __name__,
    url_prefix="/app/support",
    static_folder="static",
    static_url_path="/static",
)


def _drain_pending_acknowledgements() -> None:
    """Run the bounded foreground acknowledgement drain without affecting routes."""
    try:
        _get_client().drain_pending_acknowledgements()
    except Exception:
        logger.info("support acknowledgement drain failed", exc_info=True)


@support_bp.before_request
def _drain_pending_acknowledgements_before_request() -> None:
    _drain_pending_acknowledgements()


@support_bp.after_request
def _drain_pending_acknowledgements_after_request(response: Any) -> Any:
    _drain_pending_acknowledgements()
    return response


def _get_client():
    """Lazy-import portal client."""
    from solstone.apps.support.portal import get_client

    return get_client()


def _enabled() -> bool:
    from solstone.apps.support.portal import is_enabled

    return is_enabled()


def _action_id_or_error() -> str | tuple[Any, int]:
    """Return the required caller-owned parent action id."""
    action_id = request.headers.get("Idempotency-Key")
    if action_id:
        return action_id
    return error_response(
        MISSING_REQUIRED_FIELD, detail="Idempotency-Key header is required"
    )


def _operation_error_response(exc: Exception) -> tuple[Any, int] | None:
    """Map local ledger outcomes before a route falls back to portal failure."""
    from solstone.apps.support import operations

    mappings = (
        (operations.IdempotencyConflictError, IDEMPOTENCY_CONFLICT),
        (operations.OperationInProgressError, OPERATION_IN_PROGRESS),
        (operations.OperationRetiredError, OPERATION_RETIRED),
        (operations.OperationErasedError, OPERATION_ERASED),
        (operations.OperationTosChangedError, SUPPORT_TOS_CHANGED),
        (operations.OperationInvalidStateError, SUPPORT_INVALID_STATE),
        (operations.OperationStateUnavailableError, SUPPORT_PORTAL_FAILED),
    )
    for error_type, reason in mappings:
        if isinstance(exc, error_type):
            return error_response(reason, detail=str(exc))
    return None


# -- Config & Registration ---------------------------------------------------


@support_bp.route("/api/config", methods=["GET"])
def config() -> Any:
    """Report whether support is enabled and the external portal URL (non-secret)."""
    from solstone.apps.support.portal import _get_portal_url_from_settings, is_enabled

    return jsonify(
        {"enabled": is_enabled(), "portal_url": _get_portal_url_from_settings()}
    )


@support_bp.route("/api/draft", methods=["POST"])
def capture_draft() -> Any:
    """Capture a structured support draft into the chat stream — no portal I/O.

    Dormant cutover seam: the support CLI POSTs the exact submit-path payload here
    on a dry-run / no-network capture. Emits a backend-only ``support_draft`` chat
    event and returns its ``draft_id``. Nothing is sent to solstone support.
    """
    if not _enabled():
        return error_response(FEATURE_UNAVAILABLE, detail="Support is disabled")

    if request.files.get("file") is not None:
        verb = request.form.get("verb")
        if verb != "attach":
            return error_response(
                INVALID_REQUEST_VALUE,
                detail="verb must be attach for multipart draft capture",
            )
        try:
            ticket_id = int(request.form.get("ticket_id"))
        except (TypeError, ValueError):
            return error_response(
                INVALID_REQUEST_VALUE, detail="ticket_id must be an integer"
            )

        uploaded = request.files["file"]
        if not uploaded.filename:
            return error_response(MISSING_REQUIRED_FIELD, detail="No filename")

        import base64
        from pathlib import Path

        from solstone.apps.support.portal import PortalClient

        suffix = Path(uploaded.filename).suffix.lower()
        if suffix not in PortalClient.ALLOWED_CONTENT_TYPES:
            return error_response(
                INVALID_REQUEST_VALUE,
                detail=(
                    f"Unsupported file type: {suffix}. "
                    f"Allowed: {', '.join(sorted(PortalClient.ALLOWED_CONTENT_TYPES))}"
                ),
            )

        data = uploaded.read()
        if len(data) > PortalClient.MAX_ATTACHMENT_SIZE:
            return error_response(
                INVALID_REQUEST_VALUE,
                detail=(
                    f"File too large: {len(data) / 1024 / 1024:.1f} MB "
                    f"(max {PortalClient.MAX_ATTACHMENT_SIZE / 1024 / 1024:.0f} MB)"
                ),
            )

        ts = int(time.time() * 1000)
        draft_id = uuid.uuid4().hex
        draft_payload = {
            "ticket_id": ticket_id,
            "filename": uploaded.filename,
            "content_type": PortalClient.ALLOWED_CONTENT_TYPES[suffix],
            "byte_size": len(data),
            "content_b64": base64.b64encode(data).decode("ascii"),
        }
        # Stored draft intentionally retains base64 for confirm-time re-materialization.
        captured_day = day_for_ts(ts)
        record_draft_captured(draft_id, captured_day)
        append_chat_event(
            "support_draft",
            ts=ts,
            draft_id=draft_id,
            captured_day=captured_day,
            verb="attach",
            payload=draft_payload,
            diagnostics_snapshot=None,
        )
        return jsonify({"draft_id": draft_id})

    payload = request.get_json(force=True)
    verb = payload.get("verb")
    draft_payload = payload.get("payload")
    if verb is None or draft_payload is None:
        return error_response(
            MISSING_REQUIRED_FIELD, detail="verb and payload are required"
        )
    if verb not in {
        "create",
        "feedback",
        "reply",
        "close",
        "resolved",
        "still_need_help",
    } or not isinstance(draft_payload, dict):
        return error_response(
            INVALID_REQUEST_VALUE,
            detail=(
                "verb must be create|feedback|reply|close|resolved|still_need_help "
                "and payload must be an object"
            ),
        )

    ts = int(time.time() * 1000)
    draft_id = uuid.uuid4().hex
    captured_day = day_for_ts(ts)
    record_draft_captured(draft_id, captured_day)
    append_chat_event(
        "support_draft",
        ts=ts,
        draft_id=draft_id,
        captured_day=captured_day,
        verb=verb,
        payload=draft_payload,
        diagnostics_snapshot=payload.get("diagnostics_snapshot"),
    )
    return jsonify({"draft_id": draft_id})


@support_bp.route("/api/register", methods=["POST"])
def register() -> Any:
    """Register with the support portal and return the handle."""
    if not _enabled():
        return error_response(FEATURE_UNAVAILABLE, detail="Support is disabled")
    try:
        client = _get_client()
        signup = client.register()
        return jsonify({"handle": signup.get("handle", "")})
    except Exception as exc:
        logger.exception("Failed to register with support portal", exc_info=exc)
        return error_response(
            SUPPORT_PORTAL_FAILED,
            detail="Registration with the support portal failed.",
        )


# -- Tickets -----------------------------------------------------------------


@support_bp.route("/api/tickets", methods=["GET"])
def list_tickets() -> Any:
    """List user's tickets."""
    if not _enabled():
        return error_response(FEATURE_UNAVAILABLE, detail="Support is disabled")

    try:
        status = request.args.get("status")
        client = _get_client()
        tickets = client.list_tickets(status=status)
        return jsonify(tickets)
    except Exception as exc:
        logger.exception("Failed to list tickets")
        return error_response(SUPPORT_PORTAL_FAILED, detail=str(exc))


@support_bp.route("/api/tickets/<int:ticket_id>", methods=["GET"])
def get_ticket(ticket_id: int) -> Any:
    """Get a single ticket with thread."""
    if not _enabled():
        return error_response(FEATURE_UNAVAILABLE, detail="Support is disabled")

    try:
        client = _get_client()
        ticket = client.get_ticket(ticket_id)
        return jsonify(ticket)
    except Exception as exc:
        logger.exception("Failed to get ticket %d", ticket_id)
        return error_response(SUPPORT_PORTAL_FAILED, detail=str(exc))


@support_bp.route("/api/tickets", methods=["POST"])
def create_ticket() -> Any:
    """Create a support ticket."""
    if not _enabled():
        return error_response(FEATURE_UNAVAILABLE, detail="Support is disabled")

    action_id = _action_id_or_error()
    if not isinstance(action_id, str):
        return action_id

    payload = request.get_json(force=True)
    subject = payload.get("subject")
    description = payload.get("description")

    if not subject or not description:
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="subject and description are required",
        )

    try:
        from solstone.apps.support.tools import support_create

        result = support_create(
            subject=subject,
            description=description,
            product=payload.get("product", "solstone"),
            severity=payload.get("severity", "medium"),
            category=payload.get("category"),
            user_context=payload.get("user_context"),
            auto_context=payload.get("auto_context", True),
            anonymous=payload.get("anonymous", False),
            action_id=action_id,
        )
        return jsonify(result), 201
    except Exception as exc:
        if response := _operation_error_response(exc):
            return response
        logger.exception("Failed to create ticket")
        return error_response(SUPPORT_PORTAL_FAILED, detail=str(exc))


@support_bp.route("/api/tickets/<int:ticket_id>/reply", methods=["POST"])
def reply_to_ticket(ticket_id: int) -> Any:
    """Reply to a ticket."""
    if not _enabled():
        return error_response(FEATURE_UNAVAILABLE, detail="Support is disabled")

    action_id = _action_id_or_error()
    if not isinstance(action_id, str):
        return action_id

    payload = request.get_json(force=True)
    content = payload.get("content", "")
    if not content:
        return error_response(MISSING_REQUIRED_FIELD, detail="content is required")

    try:
        from solstone.apps.support.tools import support_reply

        result = support_reply(ticket_id, content, action_id=action_id)
        return jsonify(result), 201
    except Exception as exc:
        if response := _operation_error_response(exc):
            return response
        logger.exception("Failed to reply to ticket %d", ticket_id)
        return error_response(SUPPORT_PORTAL_FAILED, detail=str(exc))


# -- Attachments -------------------------------------------------------------


@support_bp.route("/api/tickets/<int:ticket_id>/attachments", methods=["POST"])
def upload_attachment(ticket_id: int) -> Any:
    """Upload a file attachment to a ticket."""
    if not _enabled():
        return error_response(FEATURE_UNAVAILABLE, detail="Support is disabled")

    action_id = _action_id_or_error()
    if not isinstance(action_id, str):
        return action_id

    if "file" not in request.files:
        return error_response(MISSING_REQUIRED_FIELD, detail="No file provided")

    uploaded = request.files["file"]
    if not uploaded.filename:
        return error_response(MISSING_REQUIRED_FIELD, detail="No filename")

    try:
        index = int(request.form.get("index", "0"))
    except ValueError:
        return error_response(INVALID_REQUEST_VALUE, detail="index must be an integer")
    if index < 0:
        return error_response(INVALID_REQUEST_VALUE, detail="index must be non-negative")

    try:
        import tempfile
        from pathlib import Path

        from solstone.apps.support.portal import PortalClient

        # Validate content type by extension
        suffix = Path(uploaded.filename).suffix.lower()
        if suffix not in PortalClient.ALLOWED_CONTENT_TYPES:
            return error_response(
                INVALID_REQUEST_VALUE,
                detail=(
                    f"Unsupported file type: {suffix}. "
                    f"Allowed: {', '.join(sorted(PortalClient.ALLOWED_CONTENT_TYPES))}"
                ),
            )

        # Save to temp file, then upload via portal client
        with tempfile.NamedTemporaryFile(suffix=suffix, delete=False) as tmp:
            uploaded.save(tmp)
            tmp_path = Path(tmp.name)

        try:
            from solstone.apps.support.tools import support_attach

            result = support_attach(
                ticket_id,
                str(tmp_path),
                action_id=action_id,
                index=index,
                filename=uploaded.filename,
            )
            return jsonify(result), 201
        finally:
            tmp_path.unlink(missing_ok=True)

    except ValueError as exc:
        return error_response(INVALID_REQUEST_VALUE, detail=str(exc))
    except Exception as exc:
        if response := _operation_error_response(exc):
            return response
        logger.exception("Failed to upload attachment to ticket %d", ticket_id)
        return error_response(SUPPORT_PORTAL_FAILED, detail=str(exc))


# -- Ticket lifecycle --------------------------------------------------------


@support_bp.route("/api/tickets/closed", methods=["GET"])
def list_closed_history() -> Any:
    """List closed-ticket tombstones using the portal's opaque cursor."""
    if not _enabled():
        return error_response(FEATURE_UNAVAILABLE, detail="Support is disabled")
    try:
        from solstone.apps.support.tools import support_history

        return jsonify(support_history(cursor=request.args.get("cursor")))
    except Exception as exc:
        logger.exception("Failed to list closed support history")
        return error_response(SUPPORT_PORTAL_FAILED, detail=str(exc))


@support_bp.route("/api/tickets/<int:ticket_id>/close", methods=["POST"])
def close_ticket(ticket_id: int) -> Any:
    """Close a support ticket."""
    if not _enabled():
        return error_response(FEATURE_UNAVAILABLE, detail="Support is disabled")
    action_id = _action_id_or_error()
    if not isinstance(action_id, str):
        return action_id
    try:
        from solstone.apps.support.tools import support_close

        return jsonify(support_close(ticket_id, action_id=action_id)), 201
    except Exception as exc:
        if response := _operation_error_response(exc):
            return response
        logger.exception("Failed to close support ticket %d", ticket_id)
        return error_response(SUPPORT_PORTAL_FAILED, detail=str(exc))


@support_bp.route(
    "/api/tickets/<int:ticket_id>/resolution/confirm", methods=["POST"]
)
def confirm_resolution(ticket_id: int) -> Any:
    """Confirm a proposed support-ticket resolution."""
    if not _enabled():
        return error_response(FEATURE_UNAVAILABLE, detail="Support is disabled")
    action_id = _action_id_or_error()
    if not isinstance(action_id, str):
        return action_id
    try:
        from solstone.apps.support.tools import support_resolved

        return jsonify(support_resolved(ticket_id, action_id=action_id)), 201
    except Exception as exc:
        if response := _operation_error_response(exc):
            return response
        logger.exception("Failed to confirm support resolution for %d", ticket_id)
        return error_response(SUPPORT_PORTAL_FAILED, detail=str(exc))


@support_bp.route(
    "/api/tickets/<int:ticket_id>/resolution/still-need-help", methods=["POST"]
)
def still_need_help(ticket_id: int) -> Any:
    """Reject a proposed support-ticket resolution."""
    if not _enabled():
        return error_response(FEATURE_UNAVAILABLE, detail="Support is disabled")
    action_id = _action_id_or_error()
    if not isinstance(action_id, str):
        return action_id
    try:
        from solstone.apps.support.tools import support_still_need_help

        return jsonify(support_still_need_help(ticket_id, action_id=action_id)), 201
    except Exception as exc:
        if response := _operation_error_response(exc):
            return response
        logger.exception("Failed to reject support resolution for %d", ticket_id)
        return error_response(SUPPORT_PORTAL_FAILED, detail=str(exc))


# -- Feedback ----------------------------------------------------------------


@support_bp.route("/api/feedback", methods=["POST"])
def submit_feedback() -> Any:
    """Submit feedback."""
    if not _enabled():
        return error_response(FEATURE_UNAVAILABLE, detail="Support is disabled")

    action_id = _action_id_or_error()
    if not isinstance(action_id, str):
        return action_id

    payload = request.get_json(force=True)
    body = payload.get("body", "")
    if not body:
        return error_response(MISSING_REQUIRED_FIELD, detail="body is required")

    try:
        from solstone.apps.support.tools import support_feedback

        product = payload.get("product", "solstone")
        anonymous = bool(payload.get("anonymous"))
        feedback_kwargs: dict[str, object] = {
            "body": body,
            "product": product,
            "anonymous": anonymous,
            "action_id": action_id,
        }
        if not anonymous:
            raw_email = (payload.get("user_email") or "").strip()
            if raw_email:
                feedback_kwargs["user_email"] = raw_email

        result = support_feedback(**feedback_kwargs)
        return jsonify(result), 201
    except Exception as exc:
        if response := _operation_error_response(exc):
            return response
        logger.exception("Failed to submit feedback")
        return error_response(SUPPORT_PORTAL_FAILED, detail=str(exc))


# -- KB & Announcements ------------------------------------------------------


@support_bp.route("/api/articles", methods=["GET"])
def search_articles() -> Any:
    """Search KB articles."""
    if not _enabled():
        return error_response(FEATURE_UNAVAILABLE, detail="Support is disabled")

    try:
        query = request.args.get("q")
        client = _get_client()
        articles = client.search_articles(query=query)
        return jsonify(articles)
    except Exception as exc:
        logger.exception("Failed to search articles")
        return error_response(SUPPORT_PORTAL_FAILED, detail=str(exc))


@support_bp.route("/api/articles/<slug>", methods=["GET"])
def get_article(slug: str) -> Any:
    """Read a single KB article from the portal."""
    if not _enabled():
        return error_response(FEATURE_UNAVAILABLE, detail="Support is disabled")
    try:
        client = _get_client()
        article = client.get_article(slug)
        return jsonify(article)
    except Exception as exc:
        logger.exception("Failed to read article %s", slug)
        return error_response(SUPPORT_PORTAL_FAILED, detail=str(exc))


@support_bp.route("/api/announcements", methods=["GET"])
def list_announcements() -> Any:
    """List active announcements."""
    if not _enabled():
        return error_response(FEATURE_UNAVAILABLE, detail="Support is disabled")

    try:
        client = _get_client()
        items = client.list_announcements()
        return jsonify(items)
    except Exception as exc:
        logger.exception("Failed to list announcements")
        return error_response(SUPPORT_PORTAL_FAILED, detail=str(exc))


# -- Diagnostics -------------------------------------------------------------


@support_bp.route("/api/diagnostics", methods=["GET"])
def diagnostics() -> Any:
    """Run local diagnostics."""
    from solstone.apps.support.diagnostics import collect_all

    return jsonify(collect_all())


# -- Badge -------------------------------------------------------------------


@support_bp.route("/api/badge-count", methods=["GET"])
def badge_count() -> Any:
    """Return count of tickets with new responses (for app badge)."""
    if not _enabled():
        return error_response(FEATURE_UNAVAILABLE, detail="Support is not enabled")

    try:
        client = _get_client()
        tickets = client.list_tickets(status="open")
        count = sum(
            1 for t in tickets if t.get("updated_at", "") > t.get("created_at", "")
        )
        return jsonify({"count": count})
    except Exception as exc:
        logger.exception("Failed to fetch badge count")
        return error_response(SUPPORT_PORTAL_FAILED, detail=str(exc))
