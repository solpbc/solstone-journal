# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""OpenAPI fragment for the native-client settings routes."""

from __future__ import annotations

from solstone.convey.contract import (
    FieldSpec,
    OperationSpec,
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


OPERATIONS: list[OperationSpec] = [
    OperationSpec(
        operation_id="settings.config.get",
        method="GET",
        rule="/app/settings/api/config",
        summary="Read journal settings config",
        description="Return the public journal configuration for settings CLI views.",
        responses=(
            ResponseSpec(
                status=200,
                description="Public journal configuration.",
                named_fields=(
                    FieldSpec("identity", "object", raw_schema=_FREE_OBJECT),
                    FieldSpec("transcribe", "object", raw_schema=_FREE_OBJECT),
                    FieldSpec("observe", "object", raw_schema=_FREE_OBJECT),
                    FieldSpec("env", "object", raw_schema=_FREE_OBJECT),
                    FieldSpec("runtime_env", "object", raw_schema=_FREE_OBJECT),
                    FieldSpec("key_validation", "object", raw_schema=_FREE_OBJECT),
                ),
            ),
            _json_error(
                500,
                ("settings_operation_failed",),
                "Settings config could not be read.",
            ),
        ),
    ),
    OperationSpec(
        operation_id="settings.config.update",
        method="POST",
        rule="/app/settings/api/config",
        summary="Update journal settings config",
        description=(
            "Persist a typed journal settings update for identity, env, "
            "transcribe, or processing configuration."
        ),
        request=RequestSpec(
            fields=(
                FieldSpec("section", "string", required=True),
                FieldSpec("data", "object", raw_schema=_FREE_OBJECT),
                FieldSpec("key", "string"),
                FieldSpec("value", "string"),
            ),
            example={"section": "identity", "data": {"name": "Rae"}},
        ),
        responses=(
            ResponseSpec(
                status=200,
                description="Updated public journal configuration.",
                named_fields=(
                    FieldSpec(
                        "config", "object", required=True, raw_schema=_FREE_OBJECT
                    ),
                    FieldSpec(
                        "key_validation",
                        "object",
                        required=True,
                        raw_schema=_FREE_OBJECT,
                    ),
                    FieldSpec("success", "boolean", required=True),
                ),
            ),
            _json_error(
                400,
                (
                    "invalid_config_value",
                    "missing_request_body",
                    "missing_required_field",
                ),
                "The config update request was invalid.",
            ),
            _json_error(
                503,
                ("config_busy",),
                "Settings config could not be locked for update.",
            ),
            _json_error(
                500,
                ("settings_operation_failed",),
                "Settings config update failed.",
            ),
        ),
    ),
    OperationSpec(
        operation_id="settings.convey.status",
        method="GET",
        rule="/app/settings/api/convey/status",
        summary="Read Convey status text",
        description="Return the rendered Convey bind and dashboard URL status.",
        responses=(
            ResponseSpec(
                status=200,
                description="Convey status payload.",
                named_fields=(
                    FieldSpec("dashboard_url", "string", required=True),
                    FieldSpec("status_text", "string", required=True),
                ),
            ),
            _json_error(
                500,
                ("settings_operation_failed",),
                "Convey status could not be read.",
            ),
        ),
    ),
    OperationSpec(
        operation_id="settings.transcribe.get",
        method="GET",
        rule="/app/settings/api/transcribe",
        summary="Read transcription settings",
        description="Return transcription backend metadata and current config.",
        responses=(
            ResponseSpec(
                status=200,
                description="Transcription settings.",
                named_fields=(
                    FieldSpec(
                        "backends", "array", required=True, raw_schema=_FREE_ARRAY
                    ),
                    FieldSpec(
                        "api_keys", "object", required=True, raw_schema=_FREE_OBJECT
                    ),
                    FieldSpec(
                        "config", "object", required=True, raw_schema=_FREE_OBJECT
                    ),
                    FieldSpec("runtime_label", "string"),
                    FieldSpec("resource", "object", raw_schema=_FREE_OBJECT),
                ),
            ),
            _json_error(
                500,
                ("settings_operation_failed",),
                "Transcription settings could not be read.",
            ),
        ),
    ),
    OperationSpec(
        operation_id="settings.processing.get",
        method="GET",
        rule="/app/settings/api/processing",
        summary="Read processing settings",
        description="Return effective deferred-processing settings.",
        responses=(
            ResponseSpec(
                status=200,
                description="Processing settings.",
                free_form=True,
            ),
            _json_error(
                500,
                ("settings_operation_failed",),
                "Processing settings could not be read.",
            ),
        ),
    ),
    OperationSpec(
        operation_id="settings.keys.validate",
        method="GET",
        rule="/app/settings/api/validate-keys",
        summary="Validate configured service tokens",
        description="Validate service tokens without persisting the validation cache.",
        responses=(
            ResponseSpec(
                status=200,
                description="Service-token validation results.",
                named_fields=(
                    FieldSpec(
                        "key_validation",
                        "object",
                        required=True,
                        raw_schema=_FREE_OBJECT,
                    ),
                ),
            ),
            _json_error(
                503,
                ("config_busy",),
                "Settings config could not be locked for validation-cache update.",
            ),
            _json_error(
                500,
                ("settings_operation_failed",),
                "Service-token validation failed.",
            ),
        ),
    ),
    OperationSpec(
        operation_id="settings.keys.validate-cache",
        method="POST",
        rule="/app/settings/api/validate-keys",
        summary="Validate and persist configured service-token status",
        description="Validate service tokens and persist validation-cache results.",
        responses=(
            ResponseSpec(
                status=200,
                description="Persisted service-token validation results.",
                named_fields=(
                    FieldSpec("success", "boolean", required=True),
                    FieldSpec(
                        "key_validation",
                        "object",
                        required=True,
                        raw_schema=_FREE_OBJECT,
                    ),
                ),
            ),
            _json_error(
                503,
                ("config_busy",),
                "Settings config could not be locked for validation-cache update.",
            ),
            _json_error(
                500,
                ("settings_operation_failed",),
                "Service-token validation failed.",
            ),
        ),
    ),
    OperationSpec(
        operation_id="settings.observe.get",
        method="GET",
        rule="/app/settings/api/observe",
        summary="Read observer settings",
        description="Return observer capture settings with defaults and bounds.",
        responses=(
            ResponseSpec(
                status=200,
                description="Observer settings.",
                named_fields=(
                    FieldSpec("tmux", "object", required=True, raw_schema=_FREE_OBJECT),
                    FieldSpec(
                        "defaults", "object", required=True, raw_schema=_FREE_OBJECT
                    ),
                ),
            ),
            _json_error(
                500,
                ("settings_operation_failed",),
                "Observer settings could not be read.",
            ),
        ),
    ),
    OperationSpec(
        operation_id="settings.observe.update",
        method="POST",
        rule="/app/settings/api/observe",
        summary="Update observer settings",
        description="Persist observer capture settings.",
        request=RequestSpec(
            fields=(FieldSpec("tmux", "object", raw_schema=_FREE_OBJECT),),
            example={"tmux": {"enabled": True, "capture_interval": 5}},
        ),
        responses=(
            ResponseSpec(
                status=200,
                description="Updated observer settings.",
                named_fields=(
                    FieldSpec("tmux", "object", required=True, raw_schema=_FREE_OBJECT),
                    FieldSpec(
                        "defaults", "object", required=True, raw_schema=_FREE_OBJECT
                    ),
                ),
            ),
            _json_error(
                400,
                ("invalid_config_value", "missing_request_body"),
                "The observer update request was invalid.",
            ),
            _json_error(
                503,
                ("config_busy",),
                "Settings config could not be locked for observer update.",
            ),
            _json_error(
                500,
                ("settings_operation_failed",),
                "Observer settings update failed.",
            ),
        ),
    ),
]

__all__ = ["OPERATIONS"]
