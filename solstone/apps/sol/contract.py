# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""OpenAPI fragment for the native-client identity routes."""

from __future__ import annotations

from solstone.convey.contract import FieldSpec, OperationSpec, RequestSpec, ResponseSpec


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


_AGENT_FIELDS = (
    FieldSpec("name", "string"),
    FieldSpec("name_status", "string"),
    FieldSpec("named_date", "string"),
)
_BODY_ERRORS = (
    _json_error(
        400,
        (
            "invalid_json_request",
            "missing_request_body",
            "missing_required_field",
        ),
        "The request body was missing, malformed, or incomplete.",
    ),
    _json_error(
        503,
        ("identity_busy",),
        "The identity config could not be locked.",
    ),
)
_BUSY_ERROR = _json_error(
    503,
    ("identity_busy",),
    "The identity config could not be locked.",
)


OPERATIONS: list[OperationSpec] = [
    OperationSpec(
        operation_id="sol.set-name",
        method="POST",
        rule="/app/thinking/api/set-name",
        summary="Set the agent name",
        description="Set the owner-visible agent name and name status.",
        request=RequestSpec(
            fields=(
                FieldSpec("name", "string", required=True),
                FieldSpec("status", "string"),
            ),
            example={"name": "Sol", "status": "chosen"},
        ),
        responses=(
            ResponseSpec(
                status=200,
                description="Updated agent identity.",
                named_fields=_AGENT_FIELDS,
            ),
            *_BODY_ERRORS,
        ),
    ),
    OperationSpec(
        operation_id="sol.reset",
        method="POST",
        rule="/app/thinking/api/reset",
        summary="Reset the agent name",
        description="Restore the default agent identity fields.",
        responses=(
            ResponseSpec(
                status=200,
                description="Reset agent identity.",
                named_fields=_AGENT_FIELDS,
            ),
            _BUSY_ERROR,
        ),
    ),
    OperationSpec(
        operation_id="sol.set-owner",
        method="POST",
        rule="/app/thinking/api/set-owner",
        summary="Set the journal owner",
        description="Set the journal owner's name and optional bio.",
        request=RequestSpec(
            fields=(
                FieldSpec("name", "string", required=True),
                FieldSpec("bio", "string"),
            ),
            example={"name": "Ada", "bio": "First programmer."},
        ),
        responses=(
            ResponseSpec(
                status=200,
                description="Updated owner identity.",
                named_fields=(
                    FieldSpec("name", "string", required=True),
                    FieldSpec("bio", "string", required=True),
                ),
            ),
            *_BODY_ERRORS,
        ),
    ),
    OperationSpec(
        operation_id="sol.sol-init",
        method="POST",
        rule="/app/thinking/api/sol-init",
        summary="Initialize the identity directory",
        description="Ensure the identity directory exists.",
        responses=(
            ResponseSpec(
                status=200,
                description="Identity directory status.",
                named_fields=(
                    FieldSpec("identity_dir", "string", required=True),
                    FieldSpec("status", "string", required=True),
                ),
            ),
            _BUSY_ERROR,
        ),
    ),
]

__all__ = ["OPERATIONS"]
