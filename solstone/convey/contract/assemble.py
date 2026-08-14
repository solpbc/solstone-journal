# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Assemble the generated OpenAPI contract for Convey native clients."""

from __future__ import annotations

import importlib
import re
from collections.abc import Iterable
from typing import Any

import solstone.convey.reasons as reasons
from solstone.convey.reasons import Reason

from .spec import FieldSpec, OperationSpec, ParamSpec, RequestSpec, ResponseSpec

FRAGMENT_MODULES = [
    "solstone.apps.network.contract",
    "solstone.apps.observer.contract",
    "solstone.apps.home.contract",
    "solstone.apps.activities.contract",
    "solstone.apps.awareness.contract",
    "solstone.apps.curation.contract",
    "solstone.apps.entities.contract",
    "solstone.apps.sol.contract",
    "solstone.apps.support.contract",
    "solstone.convey.push_contract",
    "solstone.convey.chat_contract",
    "solstone.convey.health_contract",
    "solstone.convey.ledger_contract",
    "solstone.convey.profile_contract",
    "solstone.convey.root_contract",
    "solstone.convey.voice_contract",
    "solstone.apps.import.contract",
    "solstone.apps.settings.contract",
    "solstone.apps.thinking.contract",
    "solstone.apps.transcripts.contract",
]

CALLOSUM_REGISTRY: dict[str, list[str]] = {
    "activity": ["live", "recorded"],
    "chat": [
        "owner_message",
        "sol_message",
        "talent_queued",
        "talent_spawned",
        "talent_finished",
        "talent_errored",
        "reflection_ready",
        "chat_queue_depth",
        "chat_error",
        "sol_chat_request",
        "sol_chat_request_superseded",
        "owner_chat_open",
        "owner_chat_dismissed",
        "support_draft",
        "result",
        "support_submit_claim",
    ],
    "cortex": [
        "request",
        "start",
        "thinking",
        "tool_start",
        "tool_end",
        "finish",
        "error",
        "talent_updated",
        "info",
        "status",
        "cancel",
        "dry_run",
        "progress",
        "text_delta",
        "tool_budget_exhausted",
        "budget_escalation",
    ],
    "importer": [
        "started",
        "status",
        "completed",
        "error",
        "file_imported",
        "enrichment_ready",
    ],
    "link": ["pair_complete", "last_seen", "stream_reset"],
    "logs": ["exec", "line", "exit"],
    "navigate": ["request"],
    "notification": ["*"],
    "observe": [
        "status",
        "observing",
        "detected",
        "described",
        "transcribed",
        "observed",
        "memory_throttle_started",
        "memory_throttle_completed",
    ],
    "storage": ["warning"],
    "supervisor": [
        "started",
        "stopped",
        "restarting",
        "status",
        "queue",
        "scheduled",
        "provider_runtime",
        "request",
        "restart",
        "drain",
        "skipped",
        "sync_conflict",
    ],
    "support": ["proactive_suggestion"],
    "think": [
        "started",
        "status",
        "group_started",
        "group_completed",
        "talent_started",
        "talent_completed",
        "completed",
        "segments_started",
        "segments_completed",
        "memory_throttle_started",
        "memory_throttle_completed",
        "daily_complete",
    ],
}

_RULE_PARAM_RE = re.compile(r"<(?:[^:<>]+:)?([^<>]+)>")


def all_reason_codes() -> list[str]:
    """Return all owner-facing Reason codes from solstone.convey.reasons."""

    return sorted(
        {value.code for value in vars(reasons).values() if isinstance(value, Reason)}
    )


def rule_to_openapi_path(rule: str) -> str:
    """Convert a Werkzeug rule path to OpenAPI path-parameter syntax."""

    return _RULE_PARAM_RE.sub(r"{\1}", rule)


def _field_schema(field: FieldSpec) -> dict[str, Any]:
    if field.raw_schema is not None:
        return dict(field.raw_schema)

    schema: dict[str, Any] = {"type": field.type}
    if field.description:
        schema["description"] = field.description
    if field.type == "array":
        schema["items"] = (
            {"type": field.item_type} if field.item_type is not None else {}
        )
    return schema


def _object_schema(fields: Iterable[FieldSpec], *, free_form: bool = False) -> dict:
    if free_form:
        return {"type": "object", "additionalProperties": True}

    field_tuple = tuple(fields)
    schema: dict[str, Any] = {
        "type": "object",
        "additionalProperties": True,
        "properties": {field.name: _field_schema(field) for field in field_tuple},
    }
    required = [field.name for field in field_tuple if field.required]
    if required:
        schema["required"] = required
    return schema


def _request_schema(request: RequestSpec) -> dict[str, Any]:
    if request.raw_schema is not None:
        return dict(request.raw_schema)
    return _object_schema(request.fields)


def _response_schema(response: ResponseSpec) -> dict[str, Any]:
    if response.raw_schema is not None:
        return dict(response.raw_schema)
    return _object_schema(response.named_fields, free_form=response.free_form)


def _has_response_content(response: ResponseSpec) -> bool:
    return bool(response.raw_schema or response.named_fields or response.free_form)


def _is_named_examples(example: object) -> bool:
    if not isinstance(example, dict) or not example:
        return False
    return all(
        isinstance(value, dict) and "value" in value for value in example.values()
    )


def _apply_example(media: dict[str, Any], example: object | None) -> None:
    if example is None:
        return
    if _is_named_examples(example):
        media["examples"] = example
        return
    media["example"] = example


def _parameter(param: ParamSpec) -> dict[str, Any]:
    required = True if param.location == "path" else param.required
    result: dict[str, Any] = {
        "name": param.name,
        "in": param.location,
        "required": required,
        "schema": {"type": param.type},
    }
    if param.description:
        result["description"] = param.description
    return result


def _request_body(request: RequestSpec) -> dict[str, Any]:
    media: dict[str, Any] = {"schema": _request_schema(request)}
    if request.example is not None:
        media["example"] = request.example

    body: dict[str, Any] = {
        "content": {
            request.content_type: media,
        }
    }
    if request.description:
        body["description"] = request.description
    if any(field.required for field in request.fields):
        body["required"] = True
    return body


def _response(response: ResponseSpec) -> dict[str, Any]:
    result: dict[str, Any] = {"description": response.description}

    if response.reason_codes:
        schema: dict[str, Any] = {"$ref": "#/components/schemas/Error"}
        if response.named_fields:
            schema = {
                "allOf": [
                    {"$ref": "#/components/schemas/Error"},
                    _object_schema(response.named_fields),
                ]
            }
        result["content"] = {
            "application/json": {
                "schema": schema,
            }
        }
        result["x-reason-codes"] = sorted(set(response.reason_codes))
    elif _has_response_content(response):
        media: dict[str, Any] = {"schema": _response_schema(response)}
        _apply_example(media, response.example)
        result["content"] = {
            response.content_type: media,
        }

    if response.extensions:
        result.update(response.extensions)
    return result


def _operation(operation: OperationSpec) -> dict[str, Any]:
    area = operation.operation_id.split(".", 1)[0]
    result: dict[str, Any] = {
        "operationId": operation.operation_id,
        "summary": operation.summary,
        "description": operation.description,
        "tags": [area],
        "responses": {
            str(response.status): _response(response)
            for response in operation.responses
        },
    }
    if operation.parameters:
        result["parameters"] = [_parameter(param) for param in operation.parameters]
    if operation.request is not None:
        result["requestBody"] = _request_body(operation.request)
    if operation.auth:
        result["x-auth"] = operation.auth
    return result


def _components() -> dict[str, Any]:
    segment_file = {
        "type": "object",
        "additionalProperties": True,
        "properties": {
            "name": {"type": "string"},
            "size": {"type": "integer"},
            "sha256": {"type": "string"},
            "status": {
                "type": "string",
                "enum": ["present", "missing", "processed"],
                "x-vocabulary": {
                    "classification": "closed",
                    "id": "SegmentFile.status",
                    "unknown_value_behavior": "reject",
                },
            },
            "submitted_name": {"type": "string"},
        },
        "required": ["name", "size", "sha256", "status"],
    }
    segment_item = {
        "type": "object",
        "additionalProperties": True,
        "properties": {
            "key": {"type": "string"},
            "observed": {"type": "boolean"},
            "files": {
                "type": "array",
                "items": {"$ref": "#/components/schemas/SegmentFile"},
            },
            "original_key": {"type": "string"},
        },
        "required": ["key", "observed", "files"],
    }
    segments_envelope = {
        "type": "object",
        "additionalProperties": True,
        "properties": {
            "items": {
                "type": "array",
                "items": {"$ref": "#/components/schemas/SegmentItem"},
            },
            "total": {"type": "integer"},
            "protocol_version": {"type": "integer"},
        },
        "required": ["items", "total", "protocol_version"],
    }
    return {
        "schemas": {
            "CallosumEvent": {
                "type": "object",
                "additionalProperties": True,
                "properties": {
                    "tract": {"type": "string"},
                    "event": {"type": "string"},
                    "ts": {"type": "integer"},
                },
                "required": ["tract", "event", "ts"],
            },
            "Error": {
                "type": "object",
                "additionalProperties": True,
                "properties": {
                    "error": {"type": "string"},
                    "reason_code": {
                        "type": "string",
                        "enum": all_reason_codes(),
                    },
                    "detail": {"type": "string"},
                },
                "required": ["error", "reason_code", "detail"],
            },
            "SegmentFile": segment_file,
            "SegmentItem": segment_item,
            "SegmentsEnvelope": segments_envelope,
        }
    }


def assemble(fragments: list[list[OperationSpec]]) -> dict[str, Any]:
    """Assemble fragment operations into the generated OpenAPI document."""

    document: dict[str, Any] = {
        "openapi": "3.1.0",
        "info": {
            "title": "Solstone Convey Native-Client Contract",
            "version": "1.0.0",
            "x-generated": True,
            "x-generated-by": "make openapi (scripts/build_openapi_contract.py)",
            "description": (
                "Generated native-client contract for the Convey HTTP surface. "
                "Regenerate with `make openapi`; do not hand-edit this file."
            ),
        },
        "paths": {},
        "components": _components(),
        "x-callosum-registry": {
            key: CALLOSUM_REGISTRY[key] for key in sorted(CALLOSUM_REGISTRY)
        },
        "x-vocabularies": {
            "callosum.tract_event": {
                "classification": "extensible",
                "known_registry": {
                    key: CALLOSUM_REGISTRY[key] for key in sorted(CALLOSUM_REGISTRY)
                },
                "unknown_value_behavior": "preserve",
            }
        },
    }

    for operations in fragments:
        for operation in operations:
            path = rule_to_openapi_path(operation.rule)
            methods = document["paths"].setdefault(path, {})
            methods[operation.method.lower()] = _operation(operation)

    return document


def build_document() -> dict[str, Any]:
    """Import the explicit fragment list and assemble the OpenAPI document."""

    fragments: list[list[OperationSpec]] = []
    for module_name in FRAGMENT_MODULES:
        module = importlib.import_module(module_name)
        fragments.append(list(module.OPERATIONS))
    return assemble(fragments)


__all__ = [
    "CALLOSUM_REGISTRY",
    "FRAGMENT_MODULES",
    "all_reason_codes",
    "assemble",
    "build_document",
    "rule_to_openapi_path",
]
