# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""OpenAPI fragment for the native-client support routes."""

from __future__ import annotations

from solstone.convey.contract import (
    FieldSpec,
    OperationSpec,
    ParamSpec,
    RequestSpec,
    ResponseSpec,
)


def _json_error(
    status: int,
    reason_codes: tuple[str, ...],
    description: str,
) -> ResponseSpec:
    return ResponseSpec(
        status=status,
        description=description,
        reason_codes=reason_codes,
    )


_FREE_OBJECT = {"type": "object", "additionalProperties": True}
_FREE_ARRAY = {"type": "array", "items": _FREE_OBJECT}
_JSON_OBJECT_OR_NULL = {"type": ["object", "null"], "additionalProperties": True}
_TICKET_ID_PARAM = ParamSpec("ticket_id", "path", type="integer")
_IDEMPOTENCY_KEY_PARAM = (
    ParamSpec(
        "Idempotency-Key",
        "header",
        required=True,
        description="Caller-owned parent action id for this mutation.",
    ),
)
_TOMBSTONE_NAMED_FIELDS = (
    FieldSpec("ticket_id", "integer"),
    FieldSpec("status", "string"),
    FieldSpec("closed_at", "string"),
    FieldSpec("close_scheduled_at", "string"),
    FieldSpec("reason_code", "string"),
)
_TOMBSTONE_OBJECT = {
    "type": "object",
    "additionalProperties": True,
    "properties": {
        "ticket_id": {"type": "integer"},
        "status": {"type": "string"},
        "closed_at": {"type": "string"},
        "close_scheduled_at": {"type": "string"},
        "reason_code": {"type": "string"},
    },
}
_SUPPORT_READ_ERRORS = (
    _json_error(
        403,
        ("feature_unavailable",),
        "The support agent is disabled.",
    ),
    _json_error(
        500,
        ("support_portal_failed",),
        "The support portal request failed.",
    ),
)
_SUPPORT_MUTATION_ERRORS = (
    _json_error(
        400,
        ("missing_required_field",),
        "The parent Idempotency-Key header was missing.",
    ),
    _json_error(401, ("tos_changed",), "Support terms require re-consent."),
    _json_error(
        409,
        ("idempotency_conflict", "invalid_state", "operation_in_progress"),
        "The support operation conflicts with its current state.",
    ),
    _json_error(
        410,
        ("operation_erased", "operation_retired"),
        "The support operation is no longer available.",
    ),
    *_SUPPORT_READ_ERRORS,
)


OPERATIONS: list[OperationSpec] = [
    OperationSpec(
        operation_id="support.config",
        method="GET",
        rule="/app/support/api/config",
        summary="Read support configuration",
        description="Return whether the support agent is enabled and its portal URL.",
        responses=(
            ResponseSpec(
                status=200,
                description="Support configuration.",
                named_fields=(
                    FieldSpec("enabled", "boolean", required=True),
                    FieldSpec("portal_url", "string", required=True),
                ),
            ),
        ),
    ),
    OperationSpec(
        operation_id="support.draft",
        method="POST",
        rule="/app/support/api/draft",
        summary="Capture a support draft",
        description=(
            "Capture the exact submit-path payload for a dry-run support flow. "
            "JSON create, feedback, and reply drafts use verb plus payload; "
            "attach drafts use multipart/form-data with verb, ticket_id, and file."
        ),
        request=RequestSpec(
            fields=(
                FieldSpec("verb", "string", required=True),
                FieldSpec("payload", "object", required=True, raw_schema=_FREE_OBJECT),
                FieldSpec(
                    "diagnostics_snapshot",
                    "object",
                    raw_schema=_JSON_OBJECT_OR_NULL,
                ),
            ),
            example={"verb": "reply", "payload": {"ticket_id": 7, "content": "hi"}},
        ),
        responses=(
            ResponseSpec(
                status=200,
                description="Captured draft id.",
                named_fields=(FieldSpec("draft_id", "string", required=True),),
            ),
            _json_error(
                400,
                ("invalid_request_value", "missing_required_field"),
                "The draft payload was missing required fields or had invalid values.",
            ),
            _json_error(
                403,
                ("feature_unavailable",),
                "The support agent is disabled.",
            ),
        ),
    ),
    OperationSpec(
        operation_id="support.register",
        method="POST",
        rule="/app/support/api/register",
        summary="Register with support",
        description="Register this journal with the support portal.",
        responses=(
            ResponseSpec(
                status=200,
                description="Support handle assigned by the portal.",
                named_fields=(FieldSpec("handle", "string", required=True),),
            ),
            *_SUPPORT_READ_ERRORS,
        ),
    ),
    OperationSpec(
        operation_id="support.search",
        method="GET",
        rule="/app/support/api/articles",
        summary="Search support articles",
        description="Search knowledge-base articles.",
        parameters=(ParamSpec("q", "query", description="Search query."),),
        responses=(
            ResponseSpec(
                status=200,
                description="Matching knowledge-base articles.",
                raw_schema=_FREE_ARRAY,
            ),
            *_SUPPORT_READ_ERRORS,
        ),
    ),
    OperationSpec(
        operation_id="support.article",
        method="GET",
        rule="/app/support/api/articles/<slug>",
        summary="Read a support article",
        description="Return one knowledge-base article by slug.",
        parameters=(ParamSpec("slug", "path"),),
        responses=(
            ResponseSpec(
                status=200,
                description="Knowledge-base article.",
                raw_schema=_FREE_OBJECT,
            ),
            *_SUPPORT_READ_ERRORS,
        ),
    ),
    OperationSpec(
        operation_id="support.list",
        method="GET",
        rule="/app/support/api/tickets",
        summary="List support tickets",
        description="List the owner's support tickets.",
        parameters=(ParamSpec("status", "query", description="Ticket status filter."),),
        responses=(
            ResponseSpec(
                status=200,
                description="Support tickets.",
                raw_schema=_FREE_ARRAY,
            ),
            *_SUPPORT_READ_ERRORS,
        ),
    ),
    OperationSpec(
        operation_id="support.show",
        method="GET",
        rule="/app/support/api/tickets/<int:ticket_id>",
        summary="Read a support ticket",
        description="Return one support ticket with its thread.",
        parameters=(_TICKET_ID_PARAM,),
        responses=(
            ResponseSpec(
                status=200,
                description="Support ticket.",
                raw_schema=_FREE_OBJECT,
            ),
            *_SUPPORT_READ_ERRORS,
        ),
    ),
    OperationSpec(
        operation_id="support.history",
        method="GET",
        rule="/app/support/api/tickets/closed",
        summary="List closed support tickets",
        description="List closed-ticket tombstones with an opaque server cursor.",
        parameters=(ParamSpec("cursor", "query", description="Opaque server cursor."),),
        responses=(
            ResponseSpec(
                status=200,
                description="Closed-ticket tombstones and the next opaque cursor.",
                named_fields=(
                    FieldSpec(
                        "tickets",
                        "array",
                        required=True,
                        raw_schema={"type": "array", "items": _TOMBSTONE_OBJECT},
                    ),
                    FieldSpec("next_cursor", "string"),
                ),
            ),
            *_SUPPORT_READ_ERRORS,
        ),
    ),
    OperationSpec(
        operation_id="support.close",
        method="POST",
        rule="/app/support/api/tickets/<int:ticket_id>/close",
        summary="Close a support ticket",
        description="Close a ticket after the caller supplies a parent action id.",
        parameters=(_TICKET_ID_PARAM, *_IDEMPOTENCY_KEY_PARAM),
        responses=(
            ResponseSpec(
                status=201,
                description="Closed-ticket tombstone.",
                named_fields=_TOMBSTONE_NAMED_FIELDS,
            ),
            *_SUPPORT_MUTATION_ERRORS,
        ),
    ),
    OperationSpec(
        operation_id="support.resolved",
        method="POST",
        rule="/app/support/api/tickets/<int:ticket_id>/resolution/confirm",
        summary="Confirm a support resolution",
        description="Confirm a proposed resolution and close the ticket.",
        parameters=(_TICKET_ID_PARAM, *_IDEMPOTENCY_KEY_PARAM),
        responses=(
            ResponseSpec(
                status=201,
                description="Closed-ticket tombstone.",
                named_fields=_TOMBSTONE_NAMED_FIELDS,
            ),
            *_SUPPORT_MUTATION_ERRORS,
        ),
    ),
    OperationSpec(
        operation_id="support.still_need_help",
        method="POST",
        rule="/app/support/api/tickets/<int:ticket_id>/resolution/still-need-help",
        summary="Reject a support resolution",
        description="Keep the ticket open after rejecting a proposed resolution.",
        parameters=(_TICKET_ID_PARAM, *_IDEMPOTENCY_KEY_PARAM),
        responses=(
            ResponseSpec(
                status=201,
                description="Active support ticket detail.",
                raw_schema=_FREE_OBJECT,
            ),
            *_SUPPORT_MUTATION_ERRORS,
        ),
    ),
    OperationSpec(
        operation_id="support.create",
        method="POST",
        rule="/app/support/api/tickets",
        summary="Create a support ticket",
        description="Create a support ticket after the CLI consent flow.",
        parameters=_IDEMPOTENCY_KEY_PARAM,
        request=RequestSpec(
            fields=(
                FieldSpec("subject", "string", required=True),
                FieldSpec("description", "string", required=True),
                FieldSpec("product", "string"),
                FieldSpec("severity", "string"),
                FieldSpec("category", "string"),
                FieldSpec("user_context", "object", raw_schema=_JSON_OBJECT_OR_NULL),
                FieldSpec("auto_context", "boolean"),
                FieldSpec("anonymous", "boolean"),
            ),
            example={
                "subject": "Chat stopped responding",
                "description": "It timed out after submitting a prompt.",
                "product": "solstone",
                "severity": "medium",
            },
        ),
        responses=(
            ResponseSpec(
                status=201,
                description="Created support ticket.",
                raw_schema=_FREE_OBJECT,
            ),
            _json_error(
                400,
                ("missing_required_field",),
                "subject or description was missing.",
            ),
            *_SUPPORT_MUTATION_ERRORS,
        ),
    ),
    OperationSpec(
        operation_id="support.reply",
        method="POST",
        rule="/app/support/api/tickets/<int:ticket_id>/reply",
        summary="Reply to a support ticket",
        description="Append a reply to an existing support ticket.",
        parameters=(_TICKET_ID_PARAM, *_IDEMPOTENCY_KEY_PARAM),
        request=RequestSpec(
            fields=(FieldSpec("content", "string", required=True),),
            example={"content": "I can reproduce this on today's build."},
        ),
        responses=(
            ResponseSpec(
                status=201,
                description="Portal reply response.",
                raw_schema=_FREE_OBJECT,
            ),
            _json_error(
                400,
                ("missing_required_field",),
                "content was missing.",
            ),
            *_SUPPORT_MUTATION_ERRORS,
        ),
    ),
    OperationSpec(
        operation_id="support.attach",
        method="POST",
        rule="/app/support/api/tickets/<int:ticket_id>/attachments",
        summary="Attach a file to a support ticket",
        description="Upload one attachment to an existing support ticket.",
        parameters=(_TICKET_ID_PARAM, *_IDEMPOTENCY_KEY_PARAM),
        request=RequestSpec(
            content_type="multipart/form-data",
            fields=(
                FieldSpec(
                    "file",
                    "string",
                    required=True,
                    raw_schema={"type": "string", "format": "binary"},
                ),
            ),
            description="Multipart form with one file field.",
        ),
        responses=(
            ResponseSpec(
                status=201,
                description="Portal attachment response.",
                raw_schema=_FREE_OBJECT,
            ),
            *_SUPPORT_MUTATION_ERRORS,
            _json_error(
                400,
                ("invalid_request_value", "missing_required_field"),
                "The upload was missing a file or had an invalid attachment value.",
            ),
        ),
    ),
    OperationSpec(
        operation_id="support.feedback",
        method="POST",
        rule="/app/support/api/feedback",
        summary="Submit support feedback",
        description="Submit lower-friction product feedback.",
        parameters=_IDEMPOTENCY_KEY_PARAM,
        request=RequestSpec(
            fields=(
                FieldSpec("body", "string", required=True),
                FieldSpec("product", "string"),
                FieldSpec("anonymous", "boolean"),
                FieldSpec("user_email", "string"),
            ),
            example={"body": "The activity summary was useful.", "product": "solstone"},
        ),
        responses=(
            ResponseSpec(
                status=201,
                description="Portal feedback response.",
                raw_schema=_FREE_OBJECT,
            ),
            _json_error(
                400,
                ("missing_required_field",),
                "body was missing.",
            ),
            *_SUPPORT_MUTATION_ERRORS,
        ),
    ),
    OperationSpec(
        operation_id="support.announcements",
        method="GET",
        rule="/app/support/api/announcements",
        summary="List support announcements",
        description="Return active product announcements and known issues.",
        responses=(
            ResponseSpec(
                status=200,
                description="Active support announcements.",
                raw_schema=_FREE_ARRAY,
            ),
            *_SUPPORT_READ_ERRORS,
        ),
    ),
    OperationSpec(
        operation_id="support.diagnose",
        method="GET",
        rule="/app/support/api/diagnostics",
        summary="Read support diagnostics",
        description="Return local diagnostics collected by the journal host.",
        responses=(
            ResponseSpec(
                status=200,
                description="Support diagnostics.",
                raw_schema=_FREE_OBJECT,
            ),
        ),
    ),
]

__all__ = ["OPERATIONS"]
