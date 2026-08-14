# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Facet relationship management and entity memory.

Facet relationships link journal entities to specific facets with context:
    facets/<facet>/entities/<id>/entity.json

Facet entity memory (observations) is stored alongside relationships:
    facets/<facet>/entities/<id>/observations.jsonl

Note: Voiceprints are stored at journal level (entities/<id>/voiceprints.npz)
since they are identity-specific, not facet-specific.
"""

import copy
import json
import shutil
from pathlib import Path
from typing import Any

from solstone.think.entities.core import (
    EntityDict,
    entity_last_active_day,
    entity_last_active_ts,
    entity_slug,
)
from solstone.think.entities.errors import EntityExistsError, EntityNotFoundError
from solstone.think.journal_io import (
    MalformedPolicy,
    atomic_replace,
    hold_lock,
    read_json,
)
from solstone.think.utils import get_journal


def get_entity_metadata(facet_name: str, entity_name: str) -> dict:
    """Get observation count and voiceprint status for an entity."""
    from solstone.think.entities.observations import count_observations

    try:
        folder = entity_memory_path(facet_name, entity_name)
    except ValueError:
        return {"observation_count": 0, "has_voiceprint": False}
    return {
        "observation_count": count_observations(facet_name, entity_name),
        "has_voiceprint": (folder / "voiceprints.npz").exists(),
    }


def facet_relationship_path(facet: str, entity_id: str) -> Path:
    """Return path to facet relationship file.

    Args:
        facet: Facet name
        entity_id: Entity ID (slug)

    Returns:
        Path to facets/<facet>/entities/<id>/entity.json
    """
    return (
        Path(get_journal()) / "facets" / facet / "entities" / entity_id / "entity.json"
    )


def load_facet_relationship(facet: str, entity_id: str) -> EntityDict | None:
    """Load a facet relationship for an entity.

    Args:
        facet: Facet name
        entity_id: Entity ID (slug)

    Returns:
        Relationship dict with entity_id, description, timestamps, etc.,
        or None if not found.
    """
    path = facet_relationship_path(facet, entity_id)
    if not path.exists():
        return None

    try:
        with open(path, "r", encoding="utf-8") as f:
            data = json.load(f)
        # Ensure entity_id is present
        data["entity_id"] = entity_id

        return data
    except (json.JSONDecodeError, OSError):
        return None


def build_facet_relationships(
    entity_id: str,
    entity_name: str,
    facets_config: dict,
    *,
    all_relationships: dict[str, dict[str, EntityDict]] | None = None,
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
        if all_relationships is None:
            relationship = load_facet_relationship(facet_name, entity_id)
        else:
            relationship = all_relationships.get(facet_name, {}).get(entity_id)
        if not relationship:
            continue

        is_detached = relationship.get("detached", False)
        facet_config = facets_config.get(facet_name, {})
        metadata = get_entity_metadata(facet_name, entity_name)

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
        facet_rel["last_active_day"] = entity_last_active_day(relationship)

        # Only count observations and activity from non-detached relationships
        if not is_detached:
            total_observation_count += metadata["observation_count"]
            if rel_active_ts > latest_active_ts:
                latest_active_ts = rel_active_ts

        facet_relationships.append(facet_rel)

    # Sort facet relationships by last_active_ts (most recent first)
    facet_relationships.sort(key=lambda r: r.get("last_active_ts", 0), reverse=True)

    return facet_relationships, total_observation_count, latest_active_ts


def save_facet_relationship(
    facet: str, entity_id: str, relationship: EntityDict
) -> None:
    """Save a facet relationship using atomic write.

    Creates the directory if needed.

    Args:
        facet: Facet name
        entity_id: Entity ID (slug)
        relationship: Relationship dict with description, timestamps, etc.
    """
    path = facet_relationship_path(facet, entity_id)

    # Ensure entity_id is in the relationship
    relationship["entity_id"] = entity_id

    content = json.dumps(relationship, ensure_ascii=False, indent=2) + "\n"
    atomic_replace(path, content)


def apply_entity_merge_relationship_inverse(
    *,
    target_id: str,
    entries: list[dict[str, Any]],
    active_entries: list[dict[str, Any]],
    merge_seq: int,
) -> None:
    """Undo recorded facet relationship merge contributions under owner locks."""

    for entry in entries:
        facet = str(entry["facet"])
        if entry.get("kind") == "move":
            _remove_moved_relationship(facet, target_id, entry)
            continue
        path = facet_relationship_path(facet, target_id)
        with hold_lock(path):
            rel = read_json(path, on_error=MalformedPolicy.RAISE, default=None)
            if not isinstance(rel, dict):
                raise ValueError(f"facet relationship inverse target missing: {path}")
            _replay_facet_scalars(
                rel,
                facet,
                entry.get("scalar_support", []),
                active_entries,
                merge_seq,
            )
            rel["entity_id"] = target_id
            atomic_replace(path, json.dumps(rel, ensure_ascii=False, indent=2) + "\n")


def _remove_moved_relationship(
    facet: str,
    target_id: str,
    entry: dict[str, Any],
) -> None:
    path = facet_relationship_path(facet, target_id)
    with hold_lock(path):
        current = read_json(path, on_error=MalformedPolicy.RAISE, default=None)
        expected = copy.deepcopy(entry.get("relationship"))
        if isinstance(expected, dict):
            expected["entity_id"] = target_id
        if current != expected:
            raise ValueError(
                "facet relationship inverse locator did not match preimage"
            )
        path.unlink(missing_ok=False)
        try:
            path.parent.rmdir()
        except OSError:
            pass


def _replay_facet_scalars(
    rel: dict[str, Any],
    facet: str,
    support: list[dict[str, Any]],
    active_entries: list[dict[str, Any]],
    merge_seq: int,
) -> None:
    for entry in support:
        field = entry["field"]
        if entry.get("target_prevalue_missing"):
            value: Any = None
            missing = True
        else:
            value = copy.deepcopy(entry["target_prevalue"])
            missing = False
        for active in active_entries:
            if int(active.get("_commit_seq") or 0) <= merge_seq:
                continue
            if active.get("facet") != facet:
                continue
            for other in active.get("scalar_support", []):
                if other.get("field") != field:
                    continue
                value, missing = _replay_facet_scalar_contribution(
                    field, value, missing, other.get("source_value")
                )
        if missing:
            rel.pop(field, None)
        else:
            rel[field] = value


def _replay_facet_scalar_contribution(
    field: str,
    current_value: Any,
    current_missing: bool,
    source_value: Any,
) -> tuple[Any, bool]:
    if source_value in (None, "", [], {}):
        return current_value, current_missing
    if field == "attached_at":
        if current_missing or source_value < current_value:
            return copy.deepcopy(source_value), False
        return current_value, current_missing
    if field in {"updated_at", "last_seen"}:
        if current_missing or source_value > current_value:
            return copy.deepcopy(source_value), False
        return current_value, current_missing
    if current_missing:
        return copy.deepcopy(source_value), False
    return current_value, current_missing


def scan_facet_relationships(facet: str) -> list[str]:
    """List all entity IDs with relationships in a facet.

    Scans facets/<facet>/entities/ for subdirectories containing entity.json.

    Args:
        facet: Facet name

    Returns:
        List of entity IDs (directory names)
    """
    entities_dir = Path(get_journal()) / "facets" / facet / "entities"
    if not entities_dir.exists():
        return []

    entity_ids = []
    for entry in entities_dir.iterdir():
        if entry.is_dir() and (entry / "entity.json").exists():
            entity_ids.append(entry.name)

    entity_ids.sort()
    return entity_ids


def load_all_facet_relationships(facet: str) -> dict[str, EntityDict]:
    """Load all facet relationships for a facet.

    Returns:
        Dict mapping entity_id to relationship dict
    """
    entity_ids = scan_facet_relationships(facet)
    relationships = {}
    for entity_id in entity_ids:
        relationship = load_facet_relationship(facet, entity_id)
        if relationship:
            relationships[entity_id] = relationship

    return relationships


def load_all_facet_relationships_across_facets() -> dict[
    str, list[tuple[str, EntityDict]]
]:
    """Load facet relationships across every facet in sorted facet order.

    Returns:
        Dict mapping entity_id to [(facet_name, relationship_dict), ...]
    """
    from solstone.think.facets import get_facets

    relationships_by_entity: dict[str, list[tuple[str, EntityDict]]] = {}
    facet_names = set(get_facets())
    facets_dir = Path(get_journal()) / "facets"
    if facets_dir.is_dir():
        facet_names.update(path.name for path in facets_dir.iterdir() if path.is_dir())

    for facet_name in sorted(facet_names):
        for entity_id, relationship in load_all_facet_relationships(facet_name).items():
            relationships_by_entity.setdefault(entity_id, []).append(
                (facet_name, relationship)
            )

    return relationships_by_entity


def enrich_relationship_with_journal(
    relationship: EntityDict,
    journal_entity: EntityDict | None,
) -> EntityDict:
    """Merge journal entity fields into relationship for unified view.

    Creates a combined entity dict that has identity fields (name, type, aka,
    is_principal, blocked) from journal and relationship fields (description,
    timestamps, etc.) from facet.

    Args:
        relationship: Facet relationship dict
        journal_entity: Journal-level entity dict (or None)

    Returns:
        Merged entity dict with all fields
    """
    # Start with relationship data
    result = dict(relationship)

    # Add identity fields from journal entity
    if journal_entity:
        result["id"] = journal_entity.get("id", relationship.get("entity_id", ""))
        result["name"] = journal_entity.get("name", "")
        result["type"] = journal_entity.get("type", "")
        if journal_entity.get("aka"):
            result["aka"] = journal_entity["aka"]
        if journal_entity.get("is_principal"):
            result["is_principal"] = True
        if journal_entity.get("blocked"):
            result["blocked"] = True
    else:
        # No journal entity - use entity_id as id
        result["id"] = relationship.get("entity_id", "")

    # Remove entity_id from result (use id instead)
    result.pop("entity_id", None)

    return result


def entity_memory_path(facet: str, name: str) -> Path:
    """Return path to entity's facet-scoped memory folder.

    Facet entity memory folders store facet-specific data about entities,
    such as observations (durable facts learned in this facet's context).

    Args:
        facet: Facet name (e.g., "personal", "work")
        name: Entity name (will be slugified)

    Returns:
        Path to facets/{facet}/entities/{entity_slug}/

    Raises:
        ValueError: If name slugifies to empty string
    """
    slug = entity_slug(name)
    if not slug:
        raise ValueError(f"Entity name '{name}' slugifies to empty string")

    return Path(get_journal()) / "facets" / facet / "entities" / slug


def ensure_entity_memory(facet: str, name: str) -> Path:
    """Create entity memory folder if needed, return path.

    Args:
        facet: Facet name (e.g., "personal", "work")
        name: Entity name (will be slugified)

    Returns:
        Path to the created/existing folder

    Raises:
        ValueError: If name slugifies to empty string
    """
    folder = entity_memory_path(facet, name)
    folder.mkdir(parents=True, exist_ok=True)
    return folder


def rename_entity_memory(facet: str, old_name: str, new_name: str) -> bool:
    """Rename entity memory folder if it exists.

    Called when an entity is renamed to keep folder in sync.

    Args:
        facet: Facet name
        old_name: Previous entity name
        new_name: New entity name

    Returns:
        True if folder was renamed, False if old folder didn't exist
        or names slugify to the same value

    Raises:
        ValueError: If either name slugifies to empty string
        OSError: If rename fails (e.g., target exists)
    """
    old_folder = entity_memory_path(facet, old_name)
    new_folder = entity_memory_path(facet, new_name)

    # No rename needed if slugified names are the same
    if old_folder == new_folder:
        return False

    if not old_folder.exists():
        return False

    if new_folder.exists():
        raise OSError(f"Target folder already exists: {new_folder}")

    shutil.move(str(old_folder), str(new_folder))
    return True


def move_facet_entity(
    *,
    entity_name: str,
    from_facet: str,
    to_facet: str,
    merge: bool = False,
) -> dict[str, Any]:
    """Move or merge an entity's facet-scoped memory between facets."""
    entity_id = entity_slug(entity_name)
    src_dir = entity_memory_path(from_facet, entity_name)
    dst_dir = entity_memory_path(to_facet, entity_name)

    if not src_dir.exists():
        raise EntityNotFoundError(entity_name)

    if dst_dir.exists() and not merge:
        raise EntityExistsError(entity_name)

    if dst_dir.exists():
        from solstone.think.entities.observations import (
            load_observations,
            save_observations,
        )

        src_relationship = load_facet_relationship(from_facet, entity_id)
        dst_relationship = load_facet_relationship(to_facet, entity_id)
        if src_relationship is not None and dst_relationship is None:
            save_facet_relationship(to_facet, entity_id, src_relationship)

        src_obs = load_observations(from_facet, entity_name)
        dst_obs = load_observations(to_facet, entity_name)

        existing_keys = {(o["content"], o.get("observed_at")) for o in dst_obs}
        merged = list(dst_obs) + [
            o
            for o in src_obs
            if (o["content"], o.get("observed_at")) not in existing_keys
        ]
        save_observations(to_facet, entity_name, merged)

        shutil.rmtree(str(src_dir))
        did_merge = True
    else:
        dst_dir.parent.mkdir(parents=True, exist_ok=True)
        shutil.move(str(src_dir), str(dst_dir))
        did_merge = False

    return {
        "entity": entity_name,
        "entity_id": entity_id,
        "moved_from": from_facet,
        "moved_to": to_facet,
        "merged": did_merge,
    }
