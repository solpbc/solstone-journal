# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Voiceprint bootstrap and name resolution for speaker attribution.

Bootstrap: Scans the journal for segments where exactly one non-owner
speaker is listed in speakers.json. In those segments, all non-owner
sentence embeddings belong to that speaker. Uses the owner centroid to
subtract the owner's sentences, then saves the remaining embeddings as
that speaker's voiceprint.

Import seeding: Scans import-stream segments for conversation transcripts
with per-line speaker attribution, maps speakers to journal entities, and
saves corresponding embeddings as voiceprints.

Name resolution: Compares voiceprint centroids between name variants.
Pairs with cosine similarity > 0.90 are the same person. Unambiguous
variants are auto-merged by adding the short name as an aka on the
canonical entity.
"""

from __future__ import annotations

import logging
from collections import defaultdict
from typing import Any

from solstone.think.entities import (
    EntityResolution,
    EntityResolutionOutcome,
    ResolutionOrigin,
    ResolutionScope,
    find_matching_entity,
    is_name_variant_match,
    record_entity_resolution,
)
from solstone.think.entities.journal import (
    load_all_journal_entities,
    load_journal_entity,
    save_journal_entity,
)
from solstone.think.utils import get_journal, now_ms

logger = logging.getLogger(__name__)

# Cosine similarity threshold for name variant merging (validated in Experiment B)
NAME_MERGE_THRESHOLD = 0.90


def _ambiguity_payload(resolution: EntityResolution) -> dict[str, Any]:
    """Return a compact API payload for an ambiguous resolution."""
    return {
        "ambiguity_id": resolution.ambiguity_id,
        "candidates": [candidate.to_dict() for candidate in resolution.candidates],
    }


def bootstrap_voiceprints(dry_run: bool = False) -> dict[str, Any]:
    """Bootstrap voiceprints through the native speaker-identity owner."""
    from solstone.apps.speakers import native as native_speakers
    from solstone.apps.speakers.attribution import _speaker_encoder_identity

    return native_speakers.bootstrap_voiceprints(
        get_journal(),
        encoder=_speaker_encoder_identity(),
        added_at=now_ms(),
        dry_run=dry_run,
    )


def merge_names(alias_name: str, canonical_name: str) -> dict[str, Any]:
    """Deep merge a speaker entity into a canonical entity."""
    from solstone.think.entities import merge_entity

    journal_entities = load_all_journal_entities()
    entities_list = list(journal_entities.values())
    resolution_scope = ResolutionScope.journal()

    alias_resolution = record_entity_resolution(
        alias_name,
        entities_list,
        scope=resolution_scope,
        origin=ResolutionOrigin(
            lane="apps.speakers.merge_names",
            field="alias_name",
        ),
    )
    if alias_resolution.outcome == EntityResolutionOutcome.AMBIGUOUS:
        return {
            "error": f"Ambiguous entity for alias: {alias_name}",
            "ambiguous": {"alias": _ambiguity_payload(alias_resolution)},
        }
    if alias_resolution.outcome != EntityResolutionOutcome.RESOLVED:
        return {"error": f"No entity found for alias: {alias_name}"}
    alias_entity = alias_resolution.entity
    if alias_entity is None:
        return {"error": f"No entity found for alias: {alias_name}"}

    canonical_resolution = record_entity_resolution(
        canonical_name,
        entities_list,
        scope=resolution_scope,
        origin=ResolutionOrigin(
            lane="apps.speakers.merge_names",
            field="canonical_name",
        ),
    )
    if canonical_resolution.outcome == EntityResolutionOutcome.AMBIGUOUS:
        return {
            "error": f"Ambiguous entity for canonical: {canonical_name}",
            "ambiguous": {"canonical": _ambiguity_payload(canonical_resolution)},
        }
    if canonical_resolution.outcome != EntityResolutionOutcome.RESOLVED:
        return {"error": f"No entity found for canonical: {canonical_name}"}
    canonical_entity = canonical_resolution.entity
    if canonical_entity is None:
        return {"error": f"No entity found for canonical: {canonical_name}"}

    alias_id = alias_entity["id"]
    canonical_id = canonical_entity["id"]

    if alias_id == canonical_id:
        return {"error": "Alias and canonical resolve to the same entity."}

    alias = load_journal_entity(alias_id)
    if not alias:
        return {"error": f"Failed to load alias entity: {alias_id}"}

    canonical = load_journal_entity(canonical_id)
    if not canonical:
        return {"error": f"Failed to load canonical entity: {canonical_id}"}

    if alias.get("is_principal") or canonical.get("is_principal"):
        return {"error": "Cannot merge the principal entity."}
    result = merge_entity(
        alias_id,
        canonical_id,
        keep_source_as_aka=True,
        commit=True,
        caller="speakers.merge_names",
    )
    if "error" in result:
        return result

    errors = []
    for error in result["segments"]["errors"]:
        path = error.get("path")
        message = error.get("message", "")
        if path:
            errors.append(f"{path}: {message}")
        else:
            errors.append(str(message))

    return {
        "merged": True,
        "alias": alias.get("name", alias_name),
        "alias_id": alias_id,
        "canonical_name": canonical.get("name", canonical_name),
        "canonical_id": canonical_id,
        "akas_added": result["identity"]["akas_added"],
        "voiceprints_merged": result["voiceprints"]["added"],
        "voiceprints_total": result["voiceprints"]["target_total"],
        "facets_merged": result["facets"]["merged"],
        "facets_moved": result["facets"]["moved"],
        "segments_scanned": result["segments"]["files_scanned"],
        "labels_rewritten": result["segments"]["labels_rewritten"],
        "corrections_rewritten": result["segments"]["corrections_rewritten"],
        "errors": errors,
    }


def detect_name_variant_candidates() -> dict[str, Any]:
    """Find actionable speaker name-variant candidates using voiceprint similarity."""
    import numpy as np

    from solstone.think.entities import (
        load_entity_voiceprints_file,
        normalize_embedding,
    )

    journal_entities = load_all_journal_entities()

    result: dict[str, Any] = {
        "candidates": [],
        "entities_with_voiceprints": 0,
        "pairs_compared": 0,
        "matches_found": [],
        "ambiguous": [],
    }

    # Compute centroid for each non-principal entity with voiceprints
    centroids: dict[str, tuple[np.ndarray, str]] = {}  # entity_id -> (centroid, name)

    for entity_id, entity in journal_entities.items():
        if entity.get("blocked") or entity.get("is_principal"):
            continue

        name = entity.get("name", "")
        if not name:
            continue

        voiceprint_result = load_entity_voiceprints_file(entity_id)
        if voiceprint_result is None:
            continue

        embeddings, _ = voiceprint_result
        if len(embeddings) == 0:
            continue

        normalized_list = []
        for emb in embeddings:
            n = normalize_embedding(emb)
            if n is not None:
                normalized_list.append(n)

        if not normalized_list:
            continue

        centroid = normalize_embedding(np.mean(normalized_list, axis=0))
        if centroid is None:
            continue

        centroids[entity_id] = (centroid, name)
        result["entities_with_voiceprints"] += 1

    # Compare all pairs and build match graph
    ids = list(centroids.keys())
    match_graph: dict[str, list[tuple[str, float]]] = defaultdict(list)

    for i in range(len(ids)):
        for j in range(i + 1, len(ids)):
            result["pairs_compared"] += 1
            id_a, id_b = ids[i], ids[j]
            cent_a, name_a = centroids[id_a]
            cent_b, name_b = centroids[id_b]

            similarity = float(np.dot(cent_a, cent_b))

            if similarity >= NAME_MERGE_THRESHOLD:
                result["matches_found"].append(
                    {
                        "name_a": name_a,
                        "name_b": name_b,
                        "similarity": round(similarity, 4),
                    }
                )
                match_graph[id_a].append((id_b, similarity))
                match_graph[id_b].append((id_a, similarity))

    # Process matches — determine auto-merges vs ambiguous
    processed: set[tuple[str, str]] = set()

    for eid, matches in match_graph.items():
        if len(matches) > 1:
            # Multiple high-similarity matches — ambiguous
            _, name = centroids[eid]
            # Deduplicate: only report once per entity
            if any(tuple(sorted([eid, m[0]])) in processed for m in matches):
                continue
            for m in matches:
                processed.add(tuple(sorted([eid, m[0]])))

            result["ambiguous"].append(
                {
                    "name": name,
                    "candidates": [
                        {"name": centroids[m[0]][1], "similarity": round(m[1], 4)}
                        for m in matches
                    ],
                }
            )
            continue

        # Single match — candidate for auto-merge
        other_id, similarity = matches[0]
        pair = tuple(sorted([eid, other_id]))
        if pair in processed:
            continue
        processed.add(pair)

        # Both sides must have exactly one match (mutual exclusivity)
        if len(match_graph.get(other_id, [])) != 1:
            continue  # Other side has multiple matches, handled above

        _, name_a = centroids[eid]
        _, name_b = centroids[other_id]

        # Determine canonical (longer name) vs alias (shorter name)
        if len(name_a) >= len(name_b):
            target_id, target_label = eid, name_a
            source_id, source_label = other_id, name_b
        else:
            target_id, target_label = other_id, name_b
            source_id, source_label = eid, name_a

        # Check name variant pattern: first-word, token-subset, or prefix-token
        if not is_name_variant_match(source_label, target_label):
            result["ambiguous"].append(
                {
                    "name": name_a,
                    "candidates": [
                        {"name": name_b, "similarity": round(similarity, 4)}
                    ],
                }
            )
            continue

        # Pairs reaching this path are ready; "waiting" is reserved for deferred confidence.
        result["candidates"].append(
            {
                "source_id": source_id,
                "source_label": source_label,
                "target_id": target_id,
                "target_label": target_label,
                "similarity": round(similarity, 4),
                "readiness": "ready",
            }
        )

    return result


def resolve_name_variants(dry_run: bool = False) -> dict[str, Any]:
    """Find and merge speaker name variants using voiceprint similarity.

    Computes a centroid for each entity's voiceprints and compares all
    pairs. Pairs with cosine similarity >= NAME_MERGE_THRESHOLD (0.90)
    are flagged as the same person.

    Auto-merge criteria (all must be true):
    - Both entities have exactly one high-similarity match (mutual exclusivity)
    - The shorter name is the first word of the longer name (name variant pattern)

    When auto-merging, the short name is added as an aka on the canonical
    (longer-name) entity. Ambiguous cases are logged but not applied.

    Args:
        dry_run: If True, report merges without applying them

    Returns:
        Dict with merge statistics
    """
    from solstone.think.entities import merge_entity

    detection = detect_name_variant_candidates()
    stats: dict[str, Any] = {
        "entities_with_voiceprints": detection["entities_with_voiceprints"],
        "pairs_compared": detection["pairs_compared"],
        "matches_found": detection["matches_found"],
        "auto_merged": [],
        "ambiguous": detection["ambiguous"],
        "errors": [],
    }

    for candidate in detection["candidates"]:
        source_id = candidate["source_id"]
        source_label = candidate["source_label"]
        target_id = candidate["target_id"]
        target_label = candidate["target_label"]
        similarity = candidate["similarity"]

        if not dry_run:
            try:
                merge_result = merge_entity(
                    source_id,
                    target_id,
                    keep_source_as_aka=True,
                    commit=True,
                    caller="speakers.resolve_name_variants",
                )
                if merge_result.get("error"):
                    stats["errors"].append(
                        f"Failed to merge {source_label} -> {target_label}: "
                        f"{merge_result['error']}"
                    )
                    continue
            except Exception as e:
                stats["errors"].append(
                    f"Failed to merge {source_label} -> {target_label}: {e}"
                )
                continue

        stats["auto_merged"].append(
            {
                "canonical": target_label,
                "alias": source_label,
                "similarity": similarity,
            }
        )

    return stats


# Import streams that contain AI chat (no real speakers to seed)
_AI_CHAT_STREAMS = frozenset({"import.chatgpt", "import.claude", "import.gemini"})


def link_import(name: str, entity_id: str) -> dict[str, Any]:
    """Link an import participant name as an aka on an existing entity.

    Args:
        name: The participant name from an import transcript
        entity_id: The entity ID to link to

    Returns:
        Dict with link result or error
    """
    entity = load_journal_entity(entity_id)
    if not entity:
        return {"error": f"Entity not found: {entity_id}"}

    # Check if the name conflicts with another entity
    all_entities = load_all_journal_entities()
    others = [e for eid, e in all_entities.items() if eid != entity_id]
    conflict = find_matching_entity(name, others)
    if conflict:
        return {"error": f"Name '{name}' conflicts with entity '{conflict['id']}'"}

    existing_aka = set(entity.get("aka", []))
    already_present = name in existing_aka

    if not already_present:
        existing_aka.add(name)
        entity["aka"] = sorted(existing_aka)
        entity["updated_at"] = now_ms()
        save_journal_entity(entity)

    return {
        "linked": True,
        "entity_id": entity_id,
        "name_added": name,
        "already_present": already_present,
    }


def seed_from_imports(dry_run: bool = False) -> dict[str, Any]:
    """Seed import voiceprints through the native speaker-identity owner."""
    from solstone.apps.speakers import native as native_speakers
    from solstone.apps.speakers.attribution import _speaker_encoder_identity

    return native_speakers.seed_from_imports(
        get_journal(),
        encoder=_speaker_encoder_identity(),
        added_at=now_ms(),
        dry_run=dry_run,
    )
