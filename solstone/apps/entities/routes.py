# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Entities app routes - facet entity management."""

from __future__ import annotations

import json
import logging
import re
import sqlite3
import time
import uuid
from datetime import datetime
from pathlib import Path
from typing import Any

from flask import Blueprint, current_app, jsonify, request

logger = logging.getLogger(__name__)

import solstone.think.deferred_deletes as deferred_deletes
from solstone.apps.entities.copy import entities_copy_payload
from solstone.apps.utils import log_app_action
from solstone.convey import state
from solstone.convey.day_grid import build_day_grid_payload
from solstone.convey.reasons import (
    AGENT_UNAVAILABLE,
    EDGE_INDEX_UNAVAILABLE,
    ENTITY_ALIAS_CONFLICT,
    ENTITY_ALREADY_EXISTS,
    ENTITY_BLOCKED,
    ENTITY_BUSY,
    ENTITY_NOT_FOUND,
    ENTITY_OPERATION_FAILED,
    INVALID_ENTITY_TYPE,
    INVALID_REQUEST_VALUE,
    MISSING_REQUEST_BODY,
    MISSING_REQUIRED_FIELD,
    OPERATION_NO_LONGER_AVAILABLE,
    PRINCIPAL_ENTITY_PROTECTED,
)
from solstone.convey.utils import (
    created,
    error_response,
    respond_collection,
    success_response,
)
from solstone.think.curation import (
    accept_entity_candidate,
    dismiss_entity_candidate,
    merge_preview_fields,
)
from solstone.think.entities import (
    AkaConflictError,
    EntityAmbiguityError,
    EntityBlockedError,
    EntityDict,
    EntityExistsError,
    EntityHistoryError,
    EntityNotFoundError,
    EntityResolutionOutcome,
    ResolutionOrigin,
    ResolutionScope,
    add_entity_aka,
    add_observation,
    attach_or_reactivate_entity,
    block_journal_entity,
    delete_detected_entity,
    detach_facet_entity,
    entity_last_active_day,
    entity_last_active_ts,
    entity_slug,
    is_valid_entity_type,
    iter_entity_history,
    last_active_day_for_ts,
    load_all_facet_relationships,
    load_all_journal_entities,
    load_ambiguities,
    load_detected_entities_recent,
    load_entities,
    load_facet_relationship,
    load_observations,
    merge_entity,
    observation_day_counts,
    record_ambiguity_choice,
    record_entity_resolution,
    resolve_entity,
    resolve_journal_entity,
    restore_journal_entity_version,
    save_detected_entity,
    save_journal_entity,
    unblock_journal_entity,
    undo_entity_merge,
    update_detected_entity,
    update_facet_entity_description,
    update_facet_entity_identity,
)
from solstone.think.entities.journal import (
    delete_journal_entity,
    get_journal_principal,
    load_journal_entity,
)
from solstone.think.entities.relationships import (
    build_facet_relationships,
    get_entity_metadata,
    move_facet_entity,
)
from solstone.think.entities.review_candidates import (
    load_candidates,
)
from solstone.think.entities.review_candidates import (
    record_merge_candidate as record_entity_merge_candidate,
)
from solstone.think.facets import get_facets, log_call_action
from solstone.think.indexer.edges import (
    ATTENDANCE_KINDS,
    load_edge_evidence,
    load_entity_network,
    load_network_overview,
)
from solstone.think.indexer.journal import search_entities
from solstone.think.journal_io import LockTimeout
from solstone.think.utils import now_ms

entities_bp = Blueprint(
    "app:entities",
    __name__,
    url_prefix="/app/entities",
)
ENTITY_DELETE_TTL = 10.0


@entities_bp.route("/")
def index() -> Any:
    """Serve the entities SPA shell."""
    return current_app.send_static_file("shell.html")


@entities_bp.route("/api/state")
def api_state() -> Any:
    """Return initial entities workspace state."""
    try:
        return jsonify(
            {
                "entities_copy": entities_copy_payload(),
                "attendance_kinds": sorted(ATTENDANCE_KINDS),
            }
        )
    except Exception:
        logger.exception("error loading entities state")
        return error_response(
            ENTITY_OPERATION_FAILED,
            detail="Failed to load entities state.",
        )


def get_facet_entities_data(facet_name: str) -> dict:
    """Get entity data for a facet: attached and detected entities.

    Returns:
        dict with keys:
            - attached: list of entity dicts with type, name, description,
                        attached_at, updated_at, last_seen timestamps,
                        plus observation_count, has_voiceprint, and last_active_ts
            - detected: list of {"type": str, "name": str, "description": str, "count": int, "last_seen": str}
    """
    # Load attached entities (already returns list of dicts)
    attached = load_entities(facet_name)

    # Enrich attached entities with metadata
    for entity in attached:
        name = entity.get("name", "")
        if name:
            metadata = get_entity_metadata(facet_name, name)
            entity["observation_count"] = metadata["observation_count"]
            entity["has_voiceprint"] = metadata["has_voiceprint"]
        # Add computed activity timestamp for frontend sorting/display
        entity["last_active_ts"] = entity_last_active_ts(entity)
        entity["last_active_day"] = entity_last_active_day(entity)

    # Load detected entities directly from files (excludes attached names/akas)
    detected = load_detected_entities_recent(facet_name)

    return {"attached": attached, "detected": detected}


def _json_body() -> dict[str, Any]:
    payload = request.get_json(silent=True) or {}
    return payload if isinstance(payload, dict) else {}


def _body_str(payload: dict[str, Any], name: str) -> str | None:
    value = payload.get(name)
    if value is None:
        return None
    return str(value)


def _required_body_str(payload: dict[str, Any], name: str) -> tuple[str | None, Any]:
    value = _body_str(payload, name)
    if value is None:
        return None, error_response(
            MISSING_REQUIRED_FIELD,
            detail=f"{name} is required",
        )
    return value, None


def _body_bool(
    payload: dict[str, Any],
    name: str,
    *,
    default: bool = False,
) -> bool:
    value = payload.get(name)
    if value is None:
        return default
    if isinstance(value, bool):
        return value
    return str(value).lower() in {"1", "true", "yes", "on"}


def _body_int_or_none(payload: dict[str, Any], name: str) -> int | None:
    value = payload.get(name)
    if value in (None, ""):
        return None
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def _undo_descriptor(merge_id: Any) -> dict[str, Any]:
    value = str(merge_id or "")
    return {
        "available": bool(value),
        "merge_id": value or None,
        "reason": None if value else "No recorded merge id is available.",
    }


def _entity_operation_error(result: dict[str, Any]) -> Any:
    """Translate a think-layer merge/undo error to a standard envelope."""
    error = result.get("error")
    if isinstance(error, dict) and error.get("code") == "repair_required":
        return error_response(
            ENTITY_OPERATION_FAILED,
            detail=str(error.get("message") or "Entity state requires repair."),
            extra={
                key: result.get(key)
                for key in (
                    "merge_id",
                    "source_id",
                    "target_id",
                    "failed_phase",
                    "operation_state",
                    "mutation_applied",
                    "source_state",
                    "target_state",
                    "safe_remediation",
                )
                if key in result
            },
        )

    detail = str(error or "Entity operation failed.")
    lowered = detail.lower()
    if "already undone" in lowered:
        return error_response(OPERATION_NO_LONGER_AVAILABLE, detail=detail)
    if "not found" in lowered:
        return error_response(ENTITY_NOT_FOUND, detail=detail)
    if "blocked" in lowered:
        return error_response(ENTITY_BLOCKED, detail=detail)
    if "must be different" in lowered or "two principal" in lowered:
        return error_response(INVALID_REQUEST_VALUE, detail=detail)
    if "lock" in lowered or "timed out" in lowered or "busy" in lowered:
        return error_response(ENTITY_BUSY, detail=detail)
    return error_response(ENTITY_OPERATION_FAILED, detail=detail)


def _query_str(name: str) -> str | None:
    value = request.args.get(name)
    if value is None:
        return None
    value = value.strip()
    return value or None


def _query_int(name: str, default: int) -> int:
    value = request.args.get(name)
    if value in (None, ""):
        return default
    try:
        return int(str(value))
    except (TypeError, ValueError) as exc:
        raise ValueError(f"{name} must be an integer") from exc


def _query_bool(name: str, *, default: bool = False) -> bool:
    value = request.args.get(name)
    if value is None:
        return default
    return value.strip().lower() in {"1", "true", "yes", "on"}


def _query_kinds() -> list[str] | None:
    kinds: list[str] = []
    for raw in request.args.getlist("kinds"):
        for item in raw.split(","):
            kind = item.strip()
            if kind:
                kinds.append(kind)
    return kinds or None


def _edge_filters() -> dict[str, Any]:
    return {
        "kinds": _query_kinds(),
        "facet": _query_str("facet"),
        "day_from": _query_str("day_from"),
        "day_to": _query_str("day_to"),
    }


def _candidate_payload(entity: EntityDict) -> dict[str, Any]:
    return {
        "name": entity.get("name"),
        "id": entity.get("id"),
        "type": entity.get("type"),
    }


def _candidate_payloads(candidates: list[EntityDict] | None) -> list[dict[str, Any]]:
    return [_candidate_payload(candidate) for candidate in candidates or []]


def _resolution_payload(query: str, candidates: list[EntityDict] | None) -> Any:
    return jsonify(
        {
            "resolved": None,
            "query": query,
            "candidates": _candidate_payloads(candidates),
        }
    )


class _EdgeResolutionFailed(Exception):
    def __init__(self, response: Any) -> None:
        super().__init__("edge entity resolution failed")
        self.response = response


def _resolve_edge_entity(query: str, facet: str | None) -> str:
    query = query.strip()
    if not query:
        raise _EdgeResolutionFailed(_resolution_payload(query, []))

    exact = load_journal_entity(query)
    if exact is not None:
        return str(exact.get("id") or query)

    if facet:
        resolved, candidates = resolve_entity(facet, query)
    else:
        resolved, candidates = resolve_journal_entity(query)

    if resolved is None:
        raise _EdgeResolutionFailed(_resolution_payload(query, candidates))

    entity_id = str(resolved.get("id") or "")
    if entity_id and load_journal_entity(entity_id) is not None:
        return entity_id

    logger.warning("resolved entity outside journal identity space: %r", resolved)
    raise _EdgeResolutionFailed(
        error_response(
            ENTITY_OPERATION_FAILED,
            detail=f"Resolved entity '{query}' is not a journal entity.",
        )
    )


def _edge_loader_error(exc: Exception) -> Any:
    if isinstance(exc, FileNotFoundError):
        return error_response(EDGE_INDEX_UNAVAILABLE)
    if isinstance(exc, sqlite3.OperationalError):
        logger.exception("edge index sqlite error")
        return error_response(EDGE_INDEX_UNAVAILABLE)
    if isinstance(exc, ValueError):
        return error_response(INVALID_REQUEST_VALUE, detail=str(exc))
    raise exc


@entities_bp.route("/api/network")
def get_entity_network() -> Any:
    """Return one-hop derived edge neighbors for one journal entity."""
    entity_query = _query_str("entity")
    if not entity_query:
        return error_response(MISSING_REQUIRED_FIELD, detail="entity is required")

    try:
        filters = _edge_filters()
        entity_id = _resolve_edge_entity(entity_query, filters["facet"])
        return jsonify(
            load_entity_network(
                entity_id,
                **filters,
                include_principal=_query_bool("include_principal"),
                limit=_query_int("limit", 25),
                evidence_limit=_query_int("evidence_limit", 5),
            )
        )
    except _EdgeResolutionFailed as exc:
        return exc.response
    except (FileNotFoundError, sqlite3.OperationalError, ValueError) as exc:
        return _edge_loader_error(exc)


@entities_bp.route("/api/history")
def get_entity_history() -> Any:
    """Return newest-first derived edge evidence for one entity pair."""
    entity_query = _query_str("entity")
    if not entity_query:
        return error_response(MISSING_REQUIRED_FIELD, detail="entity is required")

    try:
        filters = _edge_filters()
        entity_id = _resolve_edge_entity(entity_query, filters["facet"])

        peer_query = _query_str("peer")
        if peer_query:
            peer_id = _resolve_edge_entity(peer_query, filters["facet"])
        else:
            principal = get_journal_principal()
            peer_id = str(principal.get("id") or "") if principal else ""
            if not peer_id:
                return error_response(
                    INVALID_REQUEST_VALUE,
                    detail="history requires PEER because no principal entity is configured",
                )

        return jsonify(
            load_edge_evidence(
                entity_id,
                peer_id,
                **filters,
                limit=_query_int("limit", 50),
                offset=_query_int("offset", 0),
            )
        )
    except _EdgeResolutionFailed as exc:
        return exc.response
    except (FileNotFoundError, sqlite3.OperationalError, ValueError) as exc:
        return _edge_loader_error(exc)


@entities_bp.route("/api/overview")
def get_network_overview() -> Any:
    """Return a global derived edge network summary."""
    try:
        return jsonify(
            load_network_overview(
                **_edge_filters(),
                limit=_query_int("limit", 25),
            )
        )
    except (FileNotFoundError, sqlite3.OperationalError, ValueError) as exc:
        return _edge_loader_error(exc)


@entities_bp.route("/api/<facet_name>")
def get_entities(facet_name: str) -> Any:
    """Get entities for a specific facet (attached and detected)."""
    try:
        data = get_facet_entities_data(facet_name)
        return jsonify(data)
    except Exception as e:
        return error_response(ENTITY_OPERATION_FAILED, detail=str(e))


@entities_bp.route("/api/<facet_name>/resolve")
def resolve_facet_entity(facet_name: str) -> Any:
    """Resolve an entity name for the CLI without mutating state."""
    name = request.args.get("name", "").strip()
    if not name:
        return error_response(MISSING_REQUIRED_FIELD, detail="name is required")

    facet_exists = (Path(state.journal_root) / "facets" / facet_name).is_dir()
    resolved, candidates = resolve_entity(facet_name, name)
    blocked = False
    blocked_name: str | None = None
    if resolved is None:
        blocked_match, _ = resolve_entity(facet_name, name, include_blocked=True)
        if blocked_match and blocked_match.get("blocked"):
            blocked = True
            blocked_name = str(blocked_match.get("name") or name)

    return jsonify(
        {
            "facet_exists": facet_exists,
            "resolved": resolved,
            "candidates": candidates or [],
            "blocked": blocked,
            "blocked_name": blocked_name,
        }
    )


@entities_bp.route("/api/<facet_name>/detected", methods=["GET"])
def get_detected_entities(facet_name: str) -> Any:
    """Return detected entities for one facet day."""
    day = request.args.get("day", "")
    if not day:
        return error_response(MISSING_REQUIRED_FIELD, detail="day is required")
    return respond_collection(load_entities(facet_name, day))


@entities_bp.route("/api/<facet_name>/detected", methods=["POST"])
def detect_entity_route(facet_name: str) -> Any:
    """Record a detected entity for the CLI."""
    data = _json_body()
    day, error = _required_body_str(data, "day")
    if error is not None:
        return error
    type_, error = _required_body_str(data, "type")
    if error is not None:
        return error
    entity, error = _required_body_str(data, "entity")
    if error is not None:
        return error
    description, error = _required_body_str(data, "description")
    if error is not None:
        return error

    assert day is not None
    assert type_ is not None
    assert entity is not None
    assert description is not None

    if not is_valid_entity_type(type_):
        return error_response(
            INVALID_ENTITY_TYPE,
            detail=f"Invalid entity type '{type_}'",
        )

    resolution = record_entity_resolution(
        entity,
        load_entities(facet_name),
        scope=ResolutionScope.facet_scope(facet_name),
        origin=ResolutionOrigin(
            lane="apps.entities.detect",
            facet=facet_name,
            day=day,
            field="entity",
        ),
    )
    if resolution.outcome != EntityResolutionOutcome.RESOLVED:
        blocked_match, _ = resolve_entity(facet_name, entity, include_blocked=True)
        if blocked_match and blocked_match.get("blocked"):
            return error_response(
                ENTITY_BLOCKED,
                detail=str(blocked_match.get("name") or entity),
            )
    name = (
        str(resolution.entity.get("name", entity))
        if resolution.outcome == EntityResolutionOutcome.RESOLVED and resolution.entity
        else entity
    )

    try:
        save_detected_entity(facet_name, day, type_, name, description)
    except ValueError as exc:
        return error_response(INVALID_REQUEST_VALUE, detail=str(exc))
    except LockTimeout:
        return error_response(ENTITY_BUSY)

    log_call_action(
        facet=facet_name,
        action="entity_detect",
        params={
            "type": type_,
            "entity": entity,
            "name": name,
            "description": description,
        },
        day=day,
    )
    return success_response({"name": name})


@entities_bp.route("/api/<facet_name>/attach", methods=["POST"])
def attach_entity_for_call(facet_name: str) -> Any:
    """Attach an entity for the CLI with call audit identity."""
    data = _json_body()
    if not data:
        return error_response(MISSING_REQUEST_BODY, detail="No data provided")

    type_, error = _required_body_str(data, "type")
    if error is not None:
        return error
    name, error = _required_body_str(data, "name")
    if error is not None:
        return error
    description = _body_str(data, "description") or ""
    assert type_ is not None
    assert name is not None

    if not is_valid_entity_type(type_):
        return error_response(
            INVALID_ENTITY_TYPE,
            detail=f"Invalid entity type '{type_}'",
        )

    try:
        relationship, reattached = attach_or_reactivate_entity(
            facet_name,
            entity_type=type_,
            name=name,
            description=description,
        )
    except EntityExistsError:
        return error_response(ENTITY_ALREADY_EXISTS)
    except EntityBlockedError:
        return error_response(ENTITY_BLOCKED)
    except EntityNotFoundError:
        return error_response(ENTITY_NOT_FOUND)
    except LockTimeout:
        return error_response(ENTITY_BUSY)

    log_call_action(
        facet=facet_name,
        action="entity_attach",
        params={
            "type": type_,
            "entity": name,
            "name": name,
            "description": description,
        },
    )
    if reattached:
        return success_response()
    return created(
        {
            "id": relationship["entity_id"],
            "name": name,
            "type": type_,
            "description": relationship["description"],
            "attached_at": relationship["attached_at"],
            "updated_at": relationship["updated_at"],
        }
    )


@entities_bp.route("/api/<facet_name>/update-description", methods=["POST"])
def update_description_for_call(facet_name: str) -> Any:
    """Update a facet entity description for the CLI."""
    data = _json_body()
    entity_id, error = _required_body_str(data, "entity_id")
    if error is not None:
        return error
    description, error = _required_body_str(data, "description")
    if error is not None:
        return error
    assert entity_id is not None
    assert description is not None

    resolved_name = _body_str(data, "name") or entity_id
    original_query = _body_str(data, "entity") or resolved_name
    try:
        relationship = update_facet_entity_description(
            facet_name,
            entity_id,
            description,
        )
    except EntityNotFoundError:
        return error_response(ENTITY_NOT_FOUND, detail=resolved_name)
    except LockTimeout:
        return error_response(ENTITY_BUSY)

    log_call_action(
        facet=facet_name,
        action="entity_update",
        params={
            "entity": original_query,
            "name": resolved_name,
            "description": description,
        },
    )
    return success_response({"entity": relationship})


@entities_bp.route("/api/<facet_name>/update-detected", methods=["POST"])
def update_detected_for_call(facet_name: str) -> Any:
    """Update a detected entity description for the CLI."""
    data = _json_body()
    day, error = _required_body_str(data, "day")
    if error is not None:
        return error
    entity, error = _required_body_str(data, "entity")
    if error is not None:
        return error
    description, error = _required_body_str(data, "description")
    if error is not None:
        return error
    assert day is not None
    assert entity is not None
    assert description is not None

    try:
        updated = update_detected_entity(facet_name, day, entity, description)
    except ValueError as exc:
        return error_response(INVALID_REQUEST_VALUE, detail=str(exc))
    except LockTimeout:
        return error_response(ENTITY_BUSY)

    log_call_action(
        facet=facet_name,
        action="entity_update",
        params={"entity": entity, "description": description},
        day=day,
    )
    return success_response({"entity": updated})


@entities_bp.route("/api/move", methods=["POST"])
def move_entity_for_call() -> Any:
    """Move a resolved entity between facets for the CLI."""
    data = _json_body()
    entity, error = _required_body_str(data, "entity")
    if error is not None:
        return error
    from_facet, error = _required_body_str(data, "from_facet")
    if error is not None:
        return error
    to_facet, error = _required_body_str(data, "to_facet")
    if error is not None:
        return error
    assert entity is not None
    assert from_facet is not None
    assert to_facet is not None

    merge = _body_bool(data, "merge")
    consent = _body_bool(data, "consent")
    try:
        result = move_facet_entity(
            entity_name=entity,
            from_facet=from_facet,
            to_facet=to_facet,
            merge=merge,
        )
    except EntityNotFoundError:
        return error_response(
            ENTITY_OPERATION_FAILED,
            detail="Entity data directory not found in source facet.",
        )
    except EntityExistsError:
        return error_response(
            ENTITY_ALREADY_EXISTS,
            detail="Entity already exists in destination facet. Use --merge to merge.",
        )

    params: dict[str, object] = {
        "entity": entity,
        "moved_from": from_facet,
        "moved_to": to_facet,
    }
    if merge:
        params["merge"] = True
    if consent:
        params["consent"] = True
    log_call_action(facet=from_facet, action="entity_move", params=params)
    return success_response(
        {
            "entity": entity,
            "moved_from": from_facet,
            "moved_to": to_facet,
            "merged": bool(result["merged"]),
        }
    )


@entities_bp.route("/api/<facet_name>/aka", methods=["POST"])
def add_aka_for_call(facet_name: str) -> Any:
    """Add one entity alias for the CLI."""
    data = _json_body()
    entity_id, error = _required_body_str(data, "entity_id")
    if error is not None:
        return error
    aka, error = _required_body_str(data, "aka")
    if error is not None:
        return error
    exclude_name, error = _required_body_str(data, "exclude_name")
    if error is not None:
        return error
    assert entity_id is not None
    assert aka is not None
    assert exclude_name is not None

    original_query = _body_str(data, "entity") or exclude_name
    try:
        aka_list = add_entity_aka(
            facet_name,
            entity_id,
            aka,
        )
    except AkaConflictError as exc:
        return error_response(
            ENTITY_ALIAS_CONFLICT,
            detail=f"Alias '{exc.alias}' conflicts with entity '{exc.conflict_name}'.",
        )
    except EntityNotFoundError:
        return error_response(ENTITY_NOT_FOUND, detail=exclude_name)
    except LockTimeout:
        return error_response(ENTITY_BUSY)

    log_call_action(
        facet=facet_name,
        action="entity_add_aka",
        params={"entity": original_query, "name": exclude_name, "aka": aka},
    )
    return success_response({"aka": aka_list})


@entities_bp.route("/api/record-merge-candidate", methods=["POST"])
def record_merge_candidate_for_call() -> Any:
    """Record or update an entity merge candidate for the CLI."""
    data = _json_body()
    facet, error = _required_body_str(data, "facet")
    if error is not None:
        return error
    day, error = _required_body_str(data, "day")
    if error is not None:
        return error
    source, error = _required_body_str(data, "source")
    if error is not None:
        return error
    target, error = _required_body_str(data, "target")
    if error is not None:
        return error
    evidence, error = _required_body_str(data, "evidence")
    if error is not None:
        return error
    assert facet is not None
    assert day is not None
    assert source is not None
    assert target is not None
    assert evidence is not None

    source_slug = entity_slug(source)
    target_slug = entity_slug(target)
    if source_slug == target_slug:
        return error_response(
            INVALID_REQUEST_VALUE,
            detail="source and target resolve to the same entity.",
        )

    try:
        row, created_row = record_entity_merge_candidate(
            facet=facet,
            day=day,
            source=source,
            source_slug=source_slug,
            target=target,
            target_slug=target_slug,
            evidence=evidence,
            basis=_body_str(data, "basis") or "name-variant",
            detections=_body_int_or_none(data, "detections"),
            needs=_body_int_or_none(data, "needs"),
        )
    except LockTimeout:
        return error_response(ENTITY_BUSY)
    return success_response({"row": row, "created": created_row})


@entities_bp.route("/api/merge-candidates")
def get_merge_candidates_for_call() -> Any:
    """Return entity merge candidates for the CLI."""
    facet = request.args.get("facet")
    status = request.args.get("status")
    rows = load_candidates()
    if facet:
        rows = [row for row in rows if row.get("facet") == facet]
    if status:
        rows = [row for row in rows if row.get("status") == status]
    return respond_collection(rows)


@entities_bp.route("/api/accept-merge-candidate", methods=["POST"])
def accept_merge_candidate_for_call() -> Any:
    """Preview or accept one entity merge candidate for the CLI."""
    data = _json_body()
    facet, error = _required_body_str(data, "facet")
    if error is not None:
        return error
    source_slug, error = _required_body_str(data, "source_slug")
    if error is not None:
        return error
    target_slug, error = _required_body_str(data, "target_slug")
    if error is not None:
        return error
    assert facet is not None
    assert source_slug is not None
    assert target_slug is not None

    try:
        result = accept_entity_candidate(
            facet,
            source_slug,
            target_slug,
            commit=_body_bool(data, "commit"),
        )
    except LockTimeout:
        return error_response(ENTITY_BUSY)
    if result.get("status") == "preview":
        fields = merge_preview_fields(result["merge"])
        body = {
            "status": result.get("status"),
            "kind": result.get("kind"),
            "key": result.get("key"),
            "fields": fields,
        }
        return jsonify(body)
    coerced = json.loads(json.dumps(result, default=str))
    return jsonify(coerced)


@entities_bp.route("/api/dismiss-merge-candidate", methods=["POST"])
def dismiss_merge_candidate_for_call() -> Any:
    """Dismiss one entity merge candidate for the CLI."""
    data = _json_body()
    facet, error = _required_body_str(data, "facet")
    if error is not None:
        return error
    source_slug, error = _required_body_str(data, "source_slug")
    if error is not None:
        return error
    target_slug, error = _required_body_str(data, "target_slug")
    if error is not None:
        return error
    assert facet is not None
    assert source_slug is not None
    assert target_slug is not None

    try:
        result = dismiss_entity_candidate(facet, source_slug, target_slug)
    except LockTimeout:
        return error_response(ENTITY_BUSY)
    coerced = json.loads(json.dumps(result, default=str))
    return jsonify(coerced)


@entities_bp.route("/api/merge", methods=["POST"])
def merge_entities_for_call() -> Any:
    """Merge two journal entities for the CLI."""
    data = _json_body()
    source_slug, error = _required_body_str(data, "source_slug")
    if error is not None:
        return error
    target_slug, error = _required_body_str(data, "target_slug")
    if error is not None:
        return error
    assert source_slug is not None
    assert target_slug is not None

    try:
        result = merge_entity(
            source_slug,
            target_slug,
            keep_source_as_aka=_body_bool(data, "keep_source_as_aka", default=True),
            commit=_body_bool(data, "commit"),
            caller="entities.merge",
        )
    except LockTimeout:
        return error_response(ENTITY_BUSY)
    if "error" in result:
        return _entity_operation_error(result)
    if _body_bool(data, "commit"):
        result["undo"] = _undo_descriptor(result.get("merge_id"))
    coerced = json.loads(json.dumps(result, default=str))
    return jsonify(coerced)


@entities_bp.route("/api/merge/<merge_id>/undo", methods=["POST"])
def undo_entity_merge_for_call(merge_id: str) -> Any:
    """Undo one recorded entity merge."""
    try:
        result = undo_entity_merge(merge_id, caller="entities.merge.undo")
    except LockTimeout:
        return error_response(ENTITY_BUSY)
    if "error" in result:
        return _entity_operation_error(result)
    log_app_action(
        app="entities",
        facet=None,
        action="journal_entity_merge_undo",
        params={
            "merge_id": merge_id,
            "source_id": result.get("source_id"),
            "target_id": result.get("target_id"),
        },
    )
    return jsonify(json.loads(json.dumps(result, default=str)))


def _version_history_payload(entity_id: str) -> dict[str, Any]:
    events = [dict(event) for event in iter_entity_history(entity_id)]
    latest_merge_seq = max(
        (
            int(event.get("seq") or 0)
            for event in events
            if event.get("kind") in {"merge", "merge_undo"}
        ),
        default=0,
    )
    undone_ids = {
        str((event.get("operation") or {}).get("undo_of"))
        for event in events
        if event.get("kind") == "merge_undo"
        and isinstance(event.get("operation"), dict)
    }
    for event in events:
        event["restore_available"] = (
            event.get("kind") not in {"merge", "merge_undo"}
            and int(event.get("seq") or 0) > latest_merge_seq
        )
        operation = event.get("operation")
        if event.get("kind") != "merge" or not isinstance(operation, dict):
            continue
        merge_id = str(operation.get("merge_id") or "")
        event["merge_id"] = merge_id or None
        event["merge_state"] = "undone" if merge_id in undone_ids else "open"
    return {"entity_id": entity_id, "items": events}


@entities_bp.route("/api/journal/entity/<entity_id>/history")
def get_journal_entity_version_history(entity_id: str) -> Any:
    """List durable identity versions for one journal entity."""
    if load_journal_entity(entity_id) is None:
        return error_response(ENTITY_NOT_FOUND, detail=entity_id)
    try:
        return jsonify(_version_history_payload(entity_id))
    except (EntityHistoryError, OSError, ValueError) as exc:
        return error_response(ENTITY_OPERATION_FAILED, detail=str(exc))


@entities_bp.route(
    "/api/journal/entity/<entity_id>/restore",
    methods=["POST"],
)
def restore_journal_entity_version_for_call(entity_id: str) -> Any:
    """Restore one ordinary durable identity version."""
    data = _json_body()
    version_id, error = _required_body_str(data, "version_id")
    if error is not None:
        return error
    assert version_id is not None
    if load_journal_entity(entity_id) is None:
        return error_response(ENTITY_NOT_FOUND, detail=entity_id)
    try:
        event = restore_journal_entity_version(
            entity_id,
            version_id,
            caller="entities.restore-version",
        )
    except LockTimeout:
        return error_response(ENTITY_BUSY)
    except EntityHistoryError as exc:
        detail = str(exc)
        if "was not found" in detail:
            return error_response(ENTITY_NOT_FOUND, detail=detail)
        if "recorded-merge undo" in detail or "principal" in detail:
            return error_response(INVALID_REQUEST_VALUE, detail=detail)
        return error_response(ENTITY_OPERATION_FAILED, detail=detail)
    entity = load_journal_entity(entity_id)
    log_app_action(
        app="entities",
        facet=None,
        action="journal_entity_restore",
        params={"entity_id": entity_id, "version_id": version_id},
    )
    return jsonify({"restored": True, "entity": entity, "event": event})


@entities_bp.route("/api/ambiguities")
def get_entity_ambiguities() -> Any:
    """List entity ambiguities, failing if the persisted store is corrupt."""
    status = request.args.get("status")
    if status not in {None, "", "open", "resolved"}:
        return error_response(
            INVALID_REQUEST_VALUE,
            detail="status must be open or resolved",
        )
    try:
        rows = load_ambiguities(strict=True)
    except EntityAmbiguityError as exc:
        return error_response(ENTITY_OPERATION_FAILED, detail=str(exc))
    if status:
        rows = [row for row in rows if row.get("status") == status]
    return respond_collection(rows)


@entities_bp.route(
    "/api/ambiguities/<ambiguity_id>/resolve",
    methods=["POST"],
)
def resolve_entity_ambiguity(ambiguity_id: str) -> Any:
    """Resolve or re-resolve one ambiguity to an existing scoped entity."""
    data = _json_body()
    entity_id, error = _required_body_str(data, "entity_id")
    if error is not None:
        return error
    assert entity_id is not None
    try:
        rows = load_ambiguities(strict=True)
        row = next(
            (item for item in rows if item.get("ambiguity_id") == ambiguity_id),
            None,
        )
        if row is None:
            return error_response(ENTITY_NOT_FOUND, detail=ambiguity_id)
        scope_data = row["scope"]
        if scope_data["kind"] == "journal":
            scope = ResolutionScope.journal()
            entities = list(load_all_journal_entities().values())
        else:
            facet = str(scope_data["facet"])
            scope = ResolutionScope.facet_scope(facet)
            entities = load_entities(facet)
        query = str(row.get("latest_query") or row.get("original_query") or "")
        updated = record_ambiguity_choice(
            query,
            entity_id,
            entities,
            scope=scope,
            origin=ResolutionOrigin(
                lane="apps.entities.resolve_ambiguity",
                field="entity_id",
            ),
        )
    except LockTimeout:
        return error_response(ENTITY_BUSY)
    except EntityAmbiguityError as exc:
        return error_response(INVALID_REQUEST_VALUE, detail=str(exc))
    chosen = next(
        (entity for entity in entities if entity.get("id") == entity_id),
        None,
    )
    log_app_action(
        app="entities",
        facet=scope.facet,
        action="entity_ambiguity_resolve",
        params={"ambiguity_id": ambiguity_id, "entity_id": entity_id},
    )
    return jsonify({"ambiguity": updated, "entity": chosen})


@entities_bp.route("/api/<facet_name>/observations")
def get_observations_for_call(facet_name: str) -> Any:
    """Return observations for one resolved entity name."""
    name = request.args.get("name", "")
    if not name:
        return error_response(MISSING_REQUIRED_FIELD, detail="name is required")
    return respond_collection(load_observations(facet_name, name))


@entities_bp.route("/api/<facet_name>/observe", methods=["POST"])
def observe_entity_for_call(facet_name: str) -> Any:
    """Add an observation for the CLI."""
    data = _json_body()
    name, error = _required_body_str(data, "name")
    if error is not None:
        return error
    content, error = _required_body_str(data, "content")
    if error is not None:
        return error
    assert name is not None
    assert content is not None

    source_day = _body_str(data, "source_day")
    original_query = _body_str(data, "entity") or name
    try:
        result = add_observation(facet_name, name, content, source_day)
    except ValueError as exc:
        return error_response(INVALID_REQUEST_VALUE, detail=str(exc))
    except LockTimeout:
        return error_response(ENTITY_BUSY)

    log_call_action(
        facet=facet_name,
        action="entity_observe",
        params={"entity": original_query, "name": name, "content": content},
    )
    return success_response({"result": result})


@entities_bp.route("/api/search")
def search_entities_for_call() -> Any:
    """Search entities for the CLI."""
    limit = 20
    try:
        limit = int(request.args.get("limit", "20"))
    except ValueError:
        limit = 20
    results = search_entities(
        query=request.args.get("query") or None,
        entity_type=request.args.get("type") or None,
        facet=request.args.get("facet") or None,
        since=request.args.get("since") or None,
        limit=limit,
    )
    return respond_collection(results)


@entities_bp.route("/api/<facet_name>/entity/<entity_id>")
def get_entity(facet_name: str, entity_id: str) -> Any:
    """Get a single entity by id.

    Uses exact id matching only. URL fragments always contain the entity id,
    so fuzzy matching is not needed here (it's used by tool functions instead).
    Includes detached entities so they can be viewed and re-attached.

    If entity is not found in facet but exists in journal, returns journal
    entity with needs_attachment=True to allow attaching to this facet.
    """
    try:
        # Load all entities including detached, find by exact id match
        entities = load_entities(facet_name, include_detached=True)
        entity = next((e for e in entities if e.get("id") == entity_id), None)

        if entity is None:
            # Fall back to journal entity - allows viewing/attaching to new facet
            journal_entity = load_journal_entity(entity_id)
            if journal_entity is None:
                return error_response(
                    ENTITY_NOT_FOUND,
                    detail=f"Entity '{entity_id}' not found",
                )

            # Return journal entity data with flag indicating it needs attachment
            entity = {
                "id": entity_id,
                "name": journal_entity.get("name", ""),
                "type": journal_entity.get("type", ""),
                "aka": journal_entity.get("aka", []),
                "is_principal": journal_entity.get("is_principal", False),
                "needs_attachment": True,
                "observation_count": 0,
                "has_voiceprint": False,
            }
            return jsonify({"entity": entity, "observations": []})

        entity_name = entity.get("name", "")
        entity = entity.copy()

        # Add metadata
        metadata = get_entity_metadata(facet_name, entity_name)
        entity["observation_count"] = metadata["observation_count"]
        entity["has_voiceprint"] = metadata["has_voiceprint"]
        # Add computed activity timestamp for frontend display
        entity["last_active_ts"] = entity_last_active_ts(entity)
        entity["last_active_day"] = entity_last_active_day(entity)

        # Ensure id is set
        if "id" not in entity:
            entity["id"] = entity_slug(entity_name)

        # Load observations
        observations = load_observations(facet_name, entity_name)

        return jsonify({"entity": entity, "observations": observations})

    except Exception as e:
        return error_response(ENTITY_OPERATION_FAILED, detail=str(e))


@entities_bp.route("/api/<facet_name>/entity/<entity_id>/grid")
def api_grid(facet_name: str, entity_id: str) -> Any:
    """Return observation day-grid data for a facet entity."""
    try:
        entities = load_entities(facet_name, include_detached=True)
        entity = next((e for e in entities if e.get("id") == entity_id), None)

        if entity is None:
            if load_journal_entity(entity_id) is None:
                return error_response(
                    ENTITY_NOT_FOUND,
                    detail=f"Entity '{entity_id}' not found",
                )
            counts: dict[str, int] = {}
        else:
            counts = observation_day_counts(facet_name, entity.get("name", ""))

        return jsonify(build_day_grid_payload(counts, max(counts, default=None)))

    except Exception as e:
        return error_response(ENTITY_OPERATION_FAILED, detail=str(e))


@entities_bp.route("/api/<facet_name>", methods=["POST"])
def add_entity(facet_name: str) -> Any:
    """Add/attach an entity to a facet.

    Entity names must be unique within a facet (regardless of type).
    If a previously detached entity with the same name exists,
    re-activates it instead of creating a duplicate.
    """
    data = request.get_json()
    if not data:
        return error_response(MISSING_REQUEST_BODY, detail="No data provided")

    etype = data.get("type", "").strip()
    name = data.get("name", "").strip()
    desc = data.get("description", "").strip()

    if not etype or not name:
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="Type and name are required",
        )

    # Validate entity type
    if not is_valid_entity_type(etype):
        return error_response(
            INVALID_ENTITY_TYPE,
            detail=f"Invalid entity type '{etype}'",
        )

    try:
        relationship, reattached = attach_or_reactivate_entity(
            facet_name,
            entity_type=etype,
            name=name,
            description=desc,
        )

        if reattached:
            log_app_action(
                app="entities",
                facet=facet_name,
                action="entity_reattach",
                params={
                    "type": etype,
                    "name": name,
                    "description": relationship.get("description", ""),
                },
            )
            return success_response({"reattached": True})

        log_app_action(
            app="entities",
            facet=facet_name,
            action="entity_attach",
            params={"type": etype, "name": name, "description": desc},
        )

        return created(
            {
                "id": relationship["entity_id"],
                "name": name,
                "type": etype,
                "description": relationship["description"],
                "attached_at": relationship["attached_at"],
                "updated_at": relationship["updated_at"],
            }
        )

    except EntityBlockedError:
        return error_response(ENTITY_BLOCKED)
    except EntityExistsError:
        return error_response(
            ENTITY_ALREADY_EXISTS,
            detail="Entity with this name already exists in facet",
        )
    except LockTimeout:
        return error_response(ENTITY_BUSY)
    except Exception as e:
        return error_response(ENTITY_OPERATION_FAILED, detail=str(e))


@entities_bp.route("/api/<facet_name>/entity/<entity_id>", methods=["DELETE"])
def detach_entity(facet_name: str, entity_id: str) -> Any:
    """Detach an entity from a facet (soft delete).

    Sets detached=True instead of removing the entity, preserving
    all metadata for potential re-attachment later.
    """
    try:
        relationship = detach_facet_entity(facet_name, entity_id)
        journal_entity = load_journal_entity(entity_id) or {}

        log_app_action(
            app="entities",
            facet=facet_name,
            action="entity_detach",
            params={
                "entity_id": entity_id,
                "type": journal_entity.get("type", ""),
                "name": journal_entity.get("name", ""),
                "description": relationship.get("description", ""),
                "aka": journal_entity.get("aka", []),
            },
        )

        return success_response()

    except EntityNotFoundError:
        return error_response(
            ENTITY_NOT_FOUND,
            detail="Entity not found in facet",
        )
    except LockTimeout:
        return error_response(ENTITY_BUSY)
    except Exception as e:
        return error_response(ENTITY_OPERATION_FAILED, detail=str(e))


@entities_bp.route("/api/<facet_name>/update", methods=["PUT"])
def update_entity(facet_name: str) -> Any:
    """Update entity name, type, and AKA list."""
    data = request.get_json()
    if not data:
        return error_response(MISSING_REQUEST_BODY, detail="No data provided")

    old_name = data.get("old_name", "").strip()
    new_name = data.get("new_name", "").strip()
    new_type = data.get("type", "").strip()
    aka_list_str = data.get("aka_list", "").strip()

    if not old_name or not new_name:
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="old_name and new_name are required",
        )

    # Parse comma-delimited aka list
    if aka_list_str:
        aka_list = [item.strip() for item in aka_list_str.split(",") if item.strip()]
    else:
        aka_list = []

    try:
        old_journal_entity = load_journal_entity(entity_slug(old_name)) or {}
        old_aka = old_journal_entity.get("aka", [])
        old_type = old_journal_entity.get("type", "")

        journal_entity = update_facet_entity_identity(
            facet_name,
            old_name=old_name,
            new_name=new_name,
            entity_type=new_type,
            aka_list=aka_list,
        )

        log_app_action(
            app="entities",
            facet=facet_name,
            action="entity_update",
            params={
                "old_type": old_type,
                "new_type": new_type or old_type,
                "old_name": old_name,
                "new_name": new_name,
                "old_aka": old_aka,
                "new_aka": aka_list,
            },
        )

        return success_response({"entity": journal_entity})

    except EntityNotFoundError:
        return error_response(ENTITY_NOT_FOUND, detail="Entity not found")
    except EntityExistsError:
        return error_response(
            ENTITY_ALREADY_EXISTS,
            detail=f"Entity '{new_name}' already exists",
        )
    except AkaConflictError as e:
        return error_response(
            ENTITY_ALIAS_CONFLICT,
            detail=f"Alias '{e.alias}' conflicts with entity '{e.conflict_name}'",
        )
    except LockTimeout:
        return error_response(ENTITY_BUSY)
    except Exception as e:
        return error_response(ENTITY_OPERATION_FAILED, detail=str(e))


@entities_bp.route(
    "/api/<facet_name>/entity/<entity_id>/description",
    methods=["PUT"],
)
def update_description(facet_name: str, entity_id: str) -> Any:
    """Update an entity's description."""
    data = request.get_json()
    if not data:
        return error_response(MISSING_REQUEST_BODY, detail="No data provided")

    new_description = data.get("description", "").strip()

    try:
        old_relationship = load_facet_relationship(facet_name, entity_id) or {}
        journal_entity = load_journal_entity(entity_id) or {}
        update_facet_entity_description(facet_name, entity_id, new_description)

        log_app_action(
            app="entities",
            facet=facet_name,
            action="entity_update_description",
            params={
                "type": journal_entity.get("type", ""),
                "name": journal_entity.get("name", ""),
                "old_description": old_relationship.get("description", ""),
                "new_description": new_description,
            },
        )

        return success_response()

    except EntityNotFoundError:
        return error_response(
            ENTITY_NOT_FOUND,
            detail="Entity not found in facet",
        )
    except LockTimeout:
        return error_response(ENTITY_BUSY)
    except Exception as e:
        return error_response(ENTITY_OPERATION_FAILED, detail=str(e))


@entities_bp.route("/api/<facet_name>/generate-description", methods=["POST"])
def generate_description(facet_name: str) -> Any:
    """Generate a description for an entity using AI agent."""
    data = request.get_json()
    if not data:
        return error_response(MISSING_REQUEST_BODY, detail="No data provided")

    entity_type = data.get("type", "").strip()
    entity_name = data.get("name", "").strip()
    current_description = data.get("current_description", "")

    if not entity_type or not entity_name:
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="Type and name are required",
        )

    try:
        from solstone.convey.utils import spawn_agent

        # Build concise prompt - agent has detailed instructions
        current_desc = current_description or "(none)"
        prompt = (
            f"Entity Type: {entity_type}\n"
            f"Entity Name: {entity_name}\n"
            f"Facet: {facet_name}\n"
            f"Current Description: {current_desc}"
        )

        use_id = spawn_agent(
            prompt=prompt,
            name="entities:entity_describe",
        )

        return jsonify({"success": True, "use_id": use_id})

    except Exception as e:
        return error_response(AGENT_UNAVAILABLE, detail=str(e))


@entities_bp.route("/api/<facet_name>/assist", methods=["POST"])
def assist_add(facet_name: str) -> Any:
    """Use entity_assist agent to quickly add an entity with AI-generated details."""
    data = request.get_json()
    if not data:
        return error_response(MISSING_REQUEST_BODY, detail="No data provided")

    name = data.get("name", "").strip()
    if not name:
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="Entity name is required",
        )

    try:
        from solstone.convey.utils import spawn_agent

        # Format prompt as specified by entity_assist agent
        prompt = f"For the '{facet_name}' facet, this is the user's request to attach a new entity: {name}"

        # Create agent request - entity_assist agent already has provider configured
        use_id = spawn_agent(
            prompt=prompt,
            name="entities:entity_assist",
        )

        return jsonify({"success": True, "use_id": use_id})

    except Exception as e:
        return error_response(AGENT_UNAVAILABLE, detail=str(e))


@entities_bp.route("/api/<facet_name>/detected/preview")
def preview_delete(facet_name: str) -> Any:
    """Preview which days contain a detected entity before deletion."""
    entity_name = request.args.get("name", "").strip()
    if not entity_name:
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="Entity name is required",
        )

    try:
        entities_dir = Path(state.journal_root) / "facets" / facet_name / "entities"
        if not entities_dir.exists():
            return jsonify({"success": True, "days": []})

        # Scan all day files for this entity
        found_days = []
        for day_file in sorted(entities_dir.glob("*.jsonl")):
            day = day_file.stem
            entities = load_entities(facet_name, day)

            # Find all occurrences of this entity name (any type)
            for entity in entities:
                if entity.get("name") == entity_name:
                    found_days.append(
                        {
                            "day": day,
                            "type": entity.get("type", ""),
                            "description": entity.get("description", ""),
                        }
                    )

        return jsonify({"success": True, "days": found_days})

    except Exception as e:
        return error_response(ENTITY_OPERATION_FAILED, detail=str(e))


@entities_bp.route("/api/<facet_name>/detected", methods=["DELETE"])
def delete_detected(facet_name: str) -> Any:
    """Delete a detected entity from all day files."""
    data = request.get_json()
    if not data:
        return error_response(MISSING_REQUEST_BODY, detail="No data provided")

    entity_name = data.get("name", "").strip()
    if not entity_name:
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="Entity name is required",
        )

    try:
        entities_dir = Path(state.journal_root) / "facets" / facet_name / "entities"
        if not entities_dir.exists():
            return success_response({"days_modified": []})

        # Iterate through all day files and remove the entity
        days_modified = []
        deleted_entries = []
        for day_file in sorted(entities_dir.glob("*.jsonl")):
            day = day_file.stem
            entities = load_entities(facet_name, day)

            if any(e.get("name") == entity_name for e in entities):
                removed = delete_detected_entity(facet_name, day, entity_name)
                for entity in removed:
                    deleted_entries.append(
                        {
                            "day": day,
                            "type": entity.get("type", ""),
                            "description": entity.get("description", ""),
                        }
                    )
                days_modified.append(day)

        if deleted_entries:
            log_app_action(
                app="entities",
                facet=facet_name,
                action="entity_delete_detected",
                params={
                    "name": entity_name,
                    "deleted_entries": deleted_entries,
                },
            )

        return success_response({"days_modified": days_modified})

    except LockTimeout:
        return error_response(ENTITY_BUSY)
    except Exception as e:
        return error_response(ENTITY_OPERATION_FAILED, detail=str(e))


# =============================================================================
# Journal-wide entity endpoints (all-facet mode)
# =============================================================================


def get_journal_entities_data() -> dict:
    """Get all journal entities with facet relationship data.

    Returns:
        dict with:
            - entities: list of journal entities enriched with facet info
    """
    facets_config = get_facets()
    journal_entities = load_all_journal_entities()
    all_relationships = {
        facet_name: load_all_facet_relationships(facet_name)
        for facet_name in facets_config
    }

    entities = []
    for entity_id, journal_entity in journal_entities.items():
        entity_name = journal_entity.get("name", "")

        # Build facet relationships
        facet_relationships, total_observation_count, latest_active_ts = (
            build_facet_relationships(
                entity_id,
                entity_name,
                facets_config,
                all_relationships=all_relationships,
            )
        )

        # Build enriched entity
        enriched = {
            "id": entity_id,
            "name": entity_name,
            "type": journal_entity.get("type", ""),
            "aka": journal_entity.get("aka", []),
            "is_principal": journal_entity.get("is_principal", False),
            "blocked": journal_entity.get("blocked", False),
            "facets": facet_relationships,
            "total_observation_count": total_observation_count,
            "last_active_ts": latest_active_ts,
            "last_active_day": (
                last_active_day_for_ts(latest_active_ts) if latest_active_ts else None
            ),
        }

        entities.append(enriched)

    # Sort by last_active_ts (most recent first)
    entities.sort(key=lambda e: e.get("last_active_ts", 0), reverse=True)

    return {"entities": entities}


@entities_bp.route("/api/types")
def get_entity_types() -> Any:
    """Return the standard entity types for UI suggestions."""
    from solstone.think.entities import ENTITY_TYPES

    return jsonify({"types": ENTITY_TYPES})


@entities_bp.route("/api/journal")
def get_journal_entities() -> Any:
    """Get all journal entities with facet relationship summaries."""
    try:
        data = get_journal_entities_data()
        return jsonify(data)
    except Exception as e:
        logger.exception("Failed to get journal entities")
        return error_response(ENTITY_OPERATION_FAILED, detail=str(e))


@entities_bp.route("/api/journal/entity/<entity_id>")
def get_journal_entity(entity_id: str) -> Any:
    """Get a single journal entity by id with full facet relationship details."""
    try:
        journal_entity = load_journal_entity(entity_id)
        if not journal_entity:
            return error_response(
                ENTITY_NOT_FOUND,
                detail=f"Entity '{entity_id}' not found",
            )

        entity_name = journal_entity.get("name", "")
        facets_config = get_facets()

        # Build facet relationships
        facet_relationships, total_observation_count, latest_active_ts = (
            build_facet_relationships(entity_id, entity_name, facets_config)
        )

        # Build enriched entity
        enriched = {
            "id": entity_id,
            "name": entity_name,
            "type": journal_entity.get("type", ""),
            "aka": journal_entity.get("aka", []),
            "is_principal": journal_entity.get("is_principal", False),
            "blocked": journal_entity.get("blocked", False),
            "facets": facet_relationships,
            "total_observation_count": total_observation_count,
            "last_active_ts": latest_active_ts,
            "last_active_day": (
                last_active_day_for_ts(latest_active_ts) if latest_active_ts else None
            ),
        }

        return jsonify({"entity": enriched})

    except Exception as e:
        logger.exception("Failed to get journal entity")
        return error_response(ENTITY_OPERATION_FAILED, detail=str(e))


@entities_bp.route("/api/journal/entity/<entity_id>", methods=["PUT"])
def update_journal_entity(entity_id: str) -> Any:
    """Update a journal entity's name, type, and/or akas."""
    try:
        data = request.get_json()
        if not data:
            return error_response(MISSING_REQUEST_BODY, detail="No data provided")

        # Load existing entity
        journal_entity = load_journal_entity(entity_id)
        if not journal_entity:
            return error_response(
                ENTITY_NOT_FOUND,
                detail=f"Entity '{entity_id}' not found",
            )

        # Track what changed for logging
        changes = {}

        # Update name if provided
        new_name = data.get("name", "").strip()
        if new_name and new_name != journal_entity.get("name", ""):
            changes["name"] = {"old": journal_entity.get("name"), "new": new_name}
            journal_entity["name"] = new_name

        # Update type if provided
        new_type = data.get("type", "").strip()
        if new_type:
            if not is_valid_entity_type(new_type):
                return error_response(
                    INVALID_ENTITY_TYPE,
                    detail=f"Invalid entity type: {new_type}",
                )
            if new_type != journal_entity.get("type", ""):
                changes["type"] = {"old": journal_entity.get("type"), "new": new_type}
                journal_entity["type"] = new_type

        # Update akas if provided
        if "aka" in data:
            new_akas = data["aka"]
            if isinstance(new_akas, str):
                # Parse comma-separated string
                new_akas = [a.strip() for a in new_akas.split(",") if a.strip()]
            elif not isinstance(new_akas, list):
                new_akas = []

            old_akas = journal_entity.get("aka", [])
            if set(new_akas) != set(old_akas):
                changes["aka"] = {"old": old_akas, "new": new_akas}
                journal_entity["aka"] = new_akas

        if not changes:
            return jsonify({"success": True, "message": "No changes made"})

        # Update timestamp
        journal_entity["updated_at"] = now_ms()

        # Save the updated entity
        save_journal_entity(journal_entity)

        # Log the action
        log_app_action(
            app="entities",
            facet=None,  # Journal-level action
            action="journal_entity_update",
            params={
                "entity_id": entity_id,
                "changes": changes,
            },
        )

        return jsonify({"success": True, "entity": journal_entity})

    except Exception as e:
        logger.exception("Failed to update journal entity")
        return error_response(ENTITY_OPERATION_FAILED, detail=str(e))


@entities_bp.route("/api/journal/entity/<entity_id>", methods=["DELETE"])
def delete_journal_entity_route(entity_id: str) -> Any:
    """Permanently delete a journal entity and all facet relationships."""
    try:
        journal_entity = load_journal_entity(entity_id)
        if journal_entity is None:
            return error_response(
                ENTITY_NOT_FOUND,
                status=400,
                detail=f"Entity '{entity_id}' not found",
            )

        if journal_entity.get("is_principal"):
            return error_response(
                PRINCIPAL_ENTITY_PROTECTED,
                detail="Cannot delete the principal (self) entity",
            )

        ttl = ENTITY_DELETE_TTL
        pending_id = uuid.uuid4().hex

        def _commit() -> None:
            try:
                result = delete_journal_entity(entity_id)
                facets = result.get("facets_deleted", [])
            except Exception:
                facets = []
                logger.exception(
                    "deferred journal_entity_delete failed for %s", entity_id
                )
            log_app_action(
                app="entities",
                facet=None,
                action="journal_entity_delete",
                params={
                    "entity_id": entity_id,
                    "facets_deleted": facets,
                    "pending_id": pending_id,
                    "phase": "committed",
                },
            )

        # See deferred_deletes module docstring: process-lifetime, pure-delete only,
        # with pending-without-terminal action-log records as a forensic signature.
        deferred_deletes.schedule_with_id(pending_id, _commit, ttl_seconds=ttl)
        log_app_action(
            app="entities",
            facet=None,
            action="journal_entity_delete",
            params={
                "entity_id": entity_id,
                "pending_id": pending_id,
                "phase": "pending",
            },
        )
        return jsonify(
            {
                "success": True,
                "pending": pending_id,
                "commit_at_ms": int((time.time() + ttl) * 1000),
                "ttl_seconds": ttl,
            }
        )

    except Exception as e:
        logger.exception("Failed to delete journal entity")
        return error_response(ENTITY_OPERATION_FAILED, detail=str(e))


@entities_bp.route("/api/cancel-delete/<pending_id>", methods=["POST"])
def cancel_delete_journal_entity(pending_id: str) -> Any:
    """Cancel a pending deferred journal-entity deletion."""
    if not re.fullmatch(r"[0-9a-f]{32}", pending_id):
        return error_response(
            OPERATION_NO_LONGER_AVAILABLE,
            detail="already committed or unknown",
        )

    if not deferred_deletes.cancel(pending_id):
        return error_response(
            OPERATION_NO_LONGER_AVAILABLE,
            detail="already committed or unknown",
        )

    log_app_action(
        app="entities",
        facet=None,
        action="journal_entity_delete",
        params={"pending_id": pending_id, "phase": "cancelled"},
        day=datetime.now().strftime("%Y%m%d"),
    )
    return jsonify({"cancelled": pending_id})


@entities_bp.route("/api/journal/entity/<entity_id>/block", methods=["POST"])
def block_journal_entity_route(entity_id: str) -> Any:
    """Block a journal entity and detach all facet relationships."""
    try:
        result = block_journal_entity(entity_id)

        log_app_action(
            app="entities",
            facet=None,  # Journal-level action
            action="journal_entity_block",
            params={
                "entity_id": entity_id,
                "facets_detached": result.get("facets_detached", []),
            },
        )

        return jsonify(result)

    except ValueError as e:
        return error_response(ENTITY_OPERATION_FAILED, status=400, detail=str(e))
    except Exception as e:
        logger.exception("Failed to block journal entity")
        return error_response(ENTITY_OPERATION_FAILED, detail=str(e))


@entities_bp.route("/api/journal/entity/<entity_id>/unblock", methods=["POST"])
def unblock_journal_entity_route(entity_id: str) -> Any:
    """Unblock a journal entity."""
    try:
        result = unblock_journal_entity(entity_id)

        log_app_action(
            app="entities",
            facet=None,  # Journal-level action
            action="journal_entity_unblock",
            params={
                "entity_id": entity_id,
            },
        )

        return jsonify(result)

    except ValueError as e:
        return error_response(ENTITY_OPERATION_FAILED, status=400, detail=str(e))
    except Exception as e:
        logger.exception("Failed to unblock journal entity")
        return error_response(ENTITY_OPERATION_FAILED, detail=str(e))
