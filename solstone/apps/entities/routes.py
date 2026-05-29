# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Entities app routes - facet entity management."""

from __future__ import annotations

import logging
import os
import re
import time
import uuid
from datetime import datetime
from pathlib import Path
from typing import Any

from flask import Blueprint, jsonify, render_template, request

logger = logging.getLogger(__name__)

import solstone.think.deferred_deletes as deferred_deletes
from solstone.apps.entities.copy import entities_copy_payload
from solstone.apps.utils import log_app_action
from solstone.convey import state
from solstone.convey.reasons import (
    AGENT_UNAVAILABLE,
    ENTITY_ALIAS_CONFLICT,
    ENTITY_ALREADY_EXISTS,
    ENTITY_NOT_FOUND,
    ENTITY_OPERATION_FAILED,
    INVALID_ENTITY_TYPE,
    MISSING_REQUEST_BODY,
    MISSING_REQUIRED_FIELD,
    OPERATION_NO_LONGER_AVAILABLE,
    PRINCIPAL_ENTITY_PROTECTED,
    PROVIDER_KEY_MISSING,
)
from solstone.convey.utils import error_response
from solstone.think.entities import (
    block_journal_entity,
    count_observations,
    entity_last_active_ts,
    entity_memory_path,
    entity_slug,
    is_valid_entity_type,
    load_all_facet_relationships,
    load_all_journal_entities,
    load_detected_entities_recent,
    load_entities,
    load_facet_relationship,
    load_observations,
    rename_entity_memory,
    save_entities,
    save_journal_entity,
    unblock_journal_entity,
    validate_aka_uniqueness,
)
from solstone.think.entities.journal import delete_journal_entity, load_journal_entity
from solstone.think.facets import get_facets
from solstone.think.utils import now_ms

entities_bp = Blueprint(
    "app:entities",
    __name__,
    url_prefix="/app/entities",
)
ENTITY_DELETE_TTL = 10.0


@entities_bp.route("/")
def index() -> Any:
    """Render the entities workspace with owner-facing copy injected."""
    return render_template("app.html", entities_copy=entities_copy_payload())


def _get_entity_metadata(facet_name: str, entity_name: str) -> dict:
    """Get observation count and voiceprint status for an entity."""
    try:
        folder = entity_memory_path(facet_name, entity_name)
    except ValueError:
        return {"observation_count": 0, "has_voiceprint": False}
    return {
        "observation_count": count_observations(facet_name, entity_name),
        "has_voiceprint": (folder / "voiceprints.npz").exists(),
    }


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
            metadata = _get_entity_metadata(facet_name, name)
            entity["observation_count"] = metadata["observation_count"]
            entity["has_voiceprint"] = metadata["has_voiceprint"]
        # Add computed activity timestamp for frontend sorting/display
        entity["last_active_ts"] = entity_last_active_ts(entity)

    # Load detected entities directly from files (excludes attached names/akas)
    detected = load_detected_entities_recent(facet_name)

    return {"attached": attached, "detected": detected}


@entities_bp.route("/api/<facet_name>")
def get_entities(facet_name: str) -> Any:
    """Get entities for a specific facet (attached and detected)."""
    try:
        data = get_facet_entities_data(facet_name)
        return jsonify(data)
    except Exception as e:
        return error_response(ENTITY_OPERATION_FAILED, detail=str(e))


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
        metadata = _get_entity_metadata(facet_name, entity_name)
        entity["observation_count"] = metadata["observation_count"]
        entity["has_voiceprint"] = metadata["has_voiceprint"]
        # Add computed activity timestamp for frontend display
        entity["last_active_ts"] = entity_last_active_ts(entity)

        # Ensure id is set
        if "id" not in entity:
            entity["id"] = entity_slug(entity_name)

        # Load observations
        observations = load_observations(facet_name, entity_name)

        return jsonify({"entity": entity, "observations": observations})

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
        # Load ALL attached entities including detached ones
        entities = load_entities(facet_name, include_detached=True)

        # Check for existing entity by name (case-insensitive, active or detached)
        name_lower = name.lower()
        for entity in entities:
            if entity.get("name", "").lower() == name_lower:
                if entity.get("detached"):
                    # Re-activate detached entity
                    entity.pop("detached", None)
                    entity["updated_at"] = now_ms()
                    # Update type and description if provided
                    entity["type"] = etype
                    if desc:
                        entity["description"] = desc
                    save_entities(facet_name, entities)

                    log_app_action(
                        app="entities",
                        facet=facet_name,
                        action="entity_reattach",
                        params={
                            "type": etype,
                            "name": name,
                            "description": entity.get("description", ""),
                        },
                    )
                    return jsonify({"success": True, "reattached": True})
                else:
                    return error_response(
                        ENTITY_ALREADY_EXISTS,
                        detail="Entity with this name already exists in facet",
                    )

        # Add new entity with timestamps (id will be generated by save_entities)
        now = now_ms()
        entities.append(
            {
                "type": etype,
                "name": name,
                "description": desc,
                "attached_at": now,
                "updated_at": now,
            }
        )

        # Save back
        save_entities(facet_name, entities)

        log_app_action(
            app="entities",
            facet=facet_name,
            action="entity_attach",
            params={"type": etype, "name": name, "description": desc},
        )

        return jsonify({"success": True})

    except Exception as e:
        return error_response(ENTITY_OPERATION_FAILED, detail=str(e))


@entities_bp.route("/api/<facet_name>", methods=["DELETE"])
def detach_entity(facet_name: str) -> Any:
    """Detach an entity from a facet (soft delete).

    Sets detached=True instead of removing the entity, preserving
    all metadata for potential re-attachment later.
    """
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
        # Load ALL attached entities including detached ones
        entities = load_entities(facet_name, include_detached=True)

        # Find the entity to detach by name
        target_entity = None
        for e in entities:
            if e.get("name") == name:
                if not e.get("detached"):
                    target_entity = e
                break

        if not target_entity:
            return error_response(
                ENTITY_NOT_FOUND,
                detail="Entity not found in facet",
            )

        # Soft delete: set detached flag and update timestamp
        target_entity["detached"] = True
        target_entity["updated_at"] = now_ms()

        # Save updated list (entity remains in file with detached=True)
        save_entities(facet_name, entities)

        log_app_action(
            app="entities",
            facet=facet_name,
            action="entity_detach",
            params={
                "type": target_entity.get("type", ""),
                "name": name,
                "description": target_entity.get("description", ""),
                "aka": target_entity.get("aka", []),
            },
        )

        return jsonify({"success": True})

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

    try:
        # Parse comma-delimited aka list
        if aka_list_str:
            aka_list = [
                item.strip() for item in aka_list_str.split(",") if item.strip()
            ]
        else:
            aka_list = []

        # Load ALL attached entities including detached to avoid data loss on save
        entities = load_entities(facet_name, include_detached=True)

        # Find target entity by name (only search active entities)
        target = None
        target_index = -1
        for i, entity in enumerate(entities):
            if entity.get("detached"):
                continue  # Skip detached entities
            if entity.get("name") == old_name:
                target = entity
                target_index = i
                break

        if not target:
            return error_response(ENTITY_NOT_FOUND, detail="Entity not found")

        # Capture old values before modification
        old_aka = target.get("aka", [])
        old_type = target.get("type", "")

        # Check if new name conflicts with existing active entities (excluding current)
        # Use case-insensitive comparison to match save_entities validation
        if new_name.lower() != old_name.lower():
            new_name_lower = new_name.lower()
            for i, entity in enumerate(entities):
                if entity.get("detached"):
                    continue  # Skip detached entities in conflict check
                if (
                    i != target_index
                    and entity.get("name", "").lower() == new_name_lower
                ):
                    return error_response(
                        ENTITY_ALREADY_EXISTS,
                        detail=f"Entity '{new_name}' already exists",
                    )

        # Validate akas don't conflict with other entities
        for aka in aka_list:
            conflict = validate_aka_uniqueness(
                aka, entities, exclude_entity_name=old_name
            )
            if conflict:
                return error_response(
                    ENTITY_ALIAS_CONFLICT,
                    detail=f"Alias '{aka}' conflicts with entity '{conflict}'",
                )

        # Update entity
        target["name"] = new_name
        if new_type:
            target["type"] = new_type
        if aka_list:
            target["aka"] = aka_list
        else:
            target.pop("aka", None)
        target["updated_at"] = now_ms()

        # Save updated entities (id will be regenerated by save_entities)
        save_entities(facet_name, entities)

        # Rename entity memory folder if name changed
        if new_name != old_name:
            try:
                rename_entity_memory(facet_name, old_name, new_name)
            except OSError as e:
                # Log but don't fail - folder rename is best-effort
                logger.warning(
                    f"Failed to rename entity memory folder for '{old_name}' -> '{new_name}': {e}"
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

        return jsonify({"success": True, "entity": target})

    except Exception as e:
        return error_response(ENTITY_OPERATION_FAILED, detail=str(e))


@entities_bp.route("/api/<facet_name>/description", methods=["PUT"])
def update_description(facet_name: str) -> Any:
    """Update an entity's description."""
    data = request.get_json()
    if not data:
        return error_response(MISSING_REQUEST_BODY, detail="No data provided")

    entity_name = data.get("name", "").strip()
    new_description = data.get("description", "").strip()

    if not entity_name:
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="Entity name is required",
        )

    try:
        # Load ALL attached entities including detached to avoid data loss on save
        entities = load_entities(facet_name, include_detached=True)

        # Find and update the entity by name (active only), capturing old description
        updated = False
        old_description = ""
        entity_type = ""
        for entity in entities:
            if entity.get("detached"):
                continue  # Skip detached entities
            if entity.get("name") == entity_name:
                old_description = entity.get("description", "")
                entity_type = entity.get("type", "")
                entity["description"] = new_description
                entity["updated_at"] = now_ms()
                updated = True
                break

        if not updated:
            return error_response(
                ENTITY_NOT_FOUND,
                detail="Entity not found in facet",
            )

        # Save updated list
        save_entities(facet_name, entities)

        log_app_action(
            app="entities",
            facet=facet_name,
            action="entity_update_description",
            params={
                "type": entity_type,
                "name": entity_name,
                "old_description": old_description,
                "new_description": new_description,
            },
        )

        return jsonify({"success": True})

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

    # Check for Google API key
    api_key = os.getenv("GOOGLE_API_KEY")
    if not api_key:
        return error_response(
            PROVIDER_KEY_MISSING,
            detail="GOOGLE_API_KEY not set",
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
            provider="google",
        )
        if use_id is None:
            return error_response(
                AGENT_UNAVAILABLE,
                detail="Failed to connect to agent service",
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
        if use_id is None:
            return error_response(
                AGENT_UNAVAILABLE,
                detail="Failed to connect to agent service",
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
            return jsonify({"success": True, "days_modified": []})

        # Iterate through all day files and remove the entity
        days_modified = []
        deleted_entries = []
        for day_file in sorted(entities_dir.glob("*.jsonl")):
            day = day_file.stem
            entities = load_entities(facet_name, day)

            # Capture entities being removed before filtering
            for e in entities:
                if e.get("name") == entity_name:
                    deleted_entries.append(
                        {
                            "day": day,
                            "type": e.get("type", ""),
                            "description": e.get("description", ""),
                        }
                    )

            # Filter out entities matching this name (any type)
            original_count = len(entities)
            filtered_entities = [e for e in entities if e.get("name") != entity_name]

            # Only save if we actually removed something
            if len(filtered_entities) < original_count:
                save_entities(facet_name, filtered_entities, day)
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

        return jsonify({"success": True, "days_modified": days_modified})

    except Exception as e:
        return error_response(ENTITY_OPERATION_FAILED, detail=str(e))


# =============================================================================
# Journal-wide entity endpoints (all-facet mode)
# =============================================================================


def _build_facet_relationships(
    entity_id: str, entity_name: str, facets_config: dict
) -> tuple[list, int, int]:
    """Build facet relationships list for a journal entity.

    Args:
        entity_id: The entity id
        entity_name: The entity name
        facets_config: Dict of facet configs from get_facets()

    Returns:
        Tuple of (facet_relationships list, total_observation_count, latest_active_ts)
    """
    facet_relationships = []
    total_observation_count = 0
    latest_active_ts = 0

    for facet_name in facets_config:
        relationship = load_facet_relationship(facet_name, entity_id)
        if not relationship:
            continue

        is_detached = relationship.get("detached", False)
        facet_config = facets_config.get(facet_name, {})
        metadata = _get_entity_metadata(facet_name, entity_name)

        facet_rel = {
            "name": facet_name,
            "title": facet_config.get("title", facet_name),
            "color": facet_config.get("color", "#888"),
            "emoji": facet_config.get("emoji", ""),
            "description": relationship.get("description", ""),
            "last_seen": relationship.get("last_seen"),
            "attached_at": relationship.get("attached_at"),
            "updated_at": relationship.get("updated_at"),
            "observation_count": metadata["observation_count"],
            "has_voiceprint": metadata["has_voiceprint"],
        }

        # Include detached flag if true
        if is_detached:
            facet_rel["detached"] = True

        # Compute last_active_ts for this relationship
        rel_active_ts = entity_last_active_ts(relationship)
        facet_rel["last_active_ts"] = rel_active_ts

        # Only count observations and activity from non-detached relationships
        if not is_detached:
            total_observation_count += metadata["observation_count"]
            if rel_active_ts > latest_active_ts:
                latest_active_ts = rel_active_ts

        facet_relationships.append(facet_rel)

    # Sort facet relationships by last_active_ts (most recent first)
    facet_relationships.sort(key=lambda r: r.get("last_active_ts", 0), reverse=True)

    return facet_relationships, total_observation_count, latest_active_ts


def get_journal_entities_data() -> dict:
    """Get all journal entities with facet relationship data.

    Returns:
        dict with:
            - entities: list of journal entities enriched with facet info
    """
    facets_config = get_facets()
    journal_entities = load_all_journal_entities()
    for facet_name in facets_config:
        load_all_facet_relationships(facet_name)

    entities = []
    for entity_id, journal_entity in journal_entities.items():
        entity_name = journal_entity.get("name", "")

        # Build facet relationships
        facet_relationships, total_observation_count, latest_active_ts = (
            _build_facet_relationships(entity_id, entity_name, facets_config)
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
            _build_facet_relationships(entity_id, entity_name, facets_config)
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
