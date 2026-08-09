# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Entity management with journal-wide identity and facet-scoped relationships.

Entity System Architecture:
- Journal-level entities: entities/<id>/entity.json - canonical identity (name, type, aka)
- Journal-level memory: entities/<id>/ - voiceprints (identity-specific, cross-facet)
- Facet relationships: facets/<facet>/entities/<id>/entity.json - per-facet data
- Detected entities: facets/<facet>/entities/<day>.jsonl - ephemeral daily discoveries
- Facet entity memory: facets/<facet>/entities/<id>/ - observations (facet-specific)

This package is organized into focused modules:
- core: Types, constants, validation, slug generation
- journal: Journal-level entity CRUD
- relationships: Facet relationships and entity memory
- loading: Entity loading functions
- saving: Entity saving functions
- matching: Entity resolution and fuzzy matching
- activity: Activity tracking and detected entities
- observations: Observation CRUD
- formatting: Indexer formatting
"""

# Activity tracking
from solstone.think.entities.activity import (
    iter_detected_entity_names_since,
    load_detected_entities_recent,
)

# Entity ambiguity storage
from solstone.think.entities.ambiguities import (
    EntityAmbiguityError,
    load_ambiguities,
    record_ambiguity_choice,
)

# Core types and utilities
from solstone.think.entities.core import (
    DEFAULT_ACTIVITY_TS,
    ENTITY_TYPES,
    MAX_ENTITY_SLUG_LENGTH,
    EntityDict,
    entity_last_active_day,
    entity_last_active_ts,
    entity_slug,
    get_identity_names,
    is_valid_entity_type,
    last_active_day_for_ts,
)

# Errors
from solstone.think.entities.errors import (
    AkaConflictError,
    EntityBlockedError,
    EntityExistsError,
    EntityNotFoundError,
    EntityWriteError,
)

# Formatting (for indexer)
from solstone.think.entities.formatting import format_entities, format_observations

# Entity history
from solstone.think.entities.history import (
    EntityHistoryError,
    EntityHistoryRepairRequired,
    EntityOperationContext,
    iter_entity_history,
    load_entity_history_event,
    restore_journal_entity_version,
)

# Journal-level entity management
from solstone.think.entities.journal import (
    block_journal_entity,
    create_journal_entity,
    delete_created_entity_if_unreferenced,
    delete_journal_entity,
    ensure_journal_entity_memory,
    get_journal_principal,
    has_journal_principal,
    journal_entity_memory_path,
    journal_entity_path,
    load_all_journal_entities,
    load_journal_entity,
    save_journal_entity,
    scan_journal_entities,
    unblock_journal_entity,
)

# Entity loading
from solstone.think.entities.loading import (
    detected_entities_path,
    load_all_attached_entities,
    load_entities,
    load_entity_names,
    load_recent_entity_names,
    parse_entity_file,
)

# Entity matching and resolution
from solstone.think.entities.matching import (
    EntityResolution,
    EntityResolutionError,
    EntityResolutionOutcome,
    MatchResult,
    MatchTier,
    ResolutionCandidate,
    ResolutionOrigin,
    ResolutionScope,
    build_name_resolution_map,
    closest_resolution_candidates,
    find_entity_by_email,
    find_matching_entity,
    is_name_variant_match,
    record_entity_resolution,
    resolve_entity,
    resolve_journal_entity,
    validate_aka_uniqueness,
)
from solstone.think.entities.merge import merge_entity, undo_entity_merge

# Observations
from solstone.think.entities.observations import (
    add_observation,
    count_observations,
    load_observations,
    observation_day_counts,
    observations_file_path,
    record_observation_ops,
    save_observations,
)

# Facet relationships and memory
from solstone.think.entities.relationships import (
    ensure_entity_memory,
    entity_memory_path,
    facet_relationship_path,
    load_all_facet_relationships,
    load_all_facet_relationships_across_facets,
    load_facet_relationship,
    rename_entity_memory,
    save_facet_relationship,
    scan_facet_relationships,
)

# Entity saving
from solstone.think.entities.saving import (
    add_entity_aka,
    attach_or_reactivate_entity,
    delete_detected_entity,
    detach_facet_entity,
    save_detected_entity,
    save_entities,
    update_detected_entity,
    update_facet_entity_description,
    update_facet_entity_identity,
)
from solstone.think.entities.voiceprints import (
    load_entity_voiceprints_file,
    load_existing_voiceprint_keys,
    normalize_embedding,
    save_voiceprints_batch,
    voiceprint_file_path,
)

__all__ = [
    # Core
    "DEFAULT_ACTIVITY_TS",
    "ENTITY_TYPES",
    "MAX_ENTITY_SLUG_LENGTH",
    "EntityDict",
    "entity_last_active_day",
    "entity_last_active_ts",
    "last_active_day_for_ts",
    "entity_slug",
    "get_identity_names",
    "is_valid_entity_type",
    # Errors
    "AkaConflictError",
    "EntityBlockedError",
    "EntityExistsError",
    "EntityNotFoundError",
    "EntityWriteError",
    "EntityAmbiguityError",
    # History
    "EntityHistoryError",
    "EntityHistoryRepairRequired",
    "EntityOperationContext",
    "iter_entity_history",
    "load_entity_history_event",
    "restore_journal_entity_version",
    # Ambiguities
    "load_ambiguities",
    "record_ambiguity_choice",
    # Journal
    "block_journal_entity",
    "create_journal_entity",
    "delete_created_entity_if_unreferenced",
    "delete_journal_entity",
    "ensure_journal_entity_memory",
    "get_journal_principal",
    "has_journal_principal",
    "journal_entity_memory_path",
    "journal_entity_path",
    "load_all_journal_entities",
    "load_journal_entity",
    "save_journal_entity",
    "scan_journal_entities",
    "unblock_journal_entity",
    # Relationships
    "ensure_entity_memory",
    "entity_memory_path",
    "facet_relationship_path",
    "load_all_facet_relationships",
    "load_all_facet_relationships_across_facets",
    "load_facet_relationship",
    "rename_entity_memory",
    "save_facet_relationship",
    "scan_facet_relationships",
    # Loading
    "detected_entities_path",
    "load_all_attached_entities",
    "load_entities",
    "load_entity_names",
    "load_recent_entity_names",
    "merge_entity",
    "undo_entity_merge",
    "parse_entity_file",
    # Saving
    "add_entity_aka",
    "attach_or_reactivate_entity",
    "delete_detected_entity",
    "detach_facet_entity",
    "save_detected_entity",
    "save_entities",
    "save_voiceprints_batch",
    "update_facet_entity_description",
    "update_facet_entity_identity",
    "update_detected_entity",
    "voiceprint_file_path",
    # Matching
    "EntityResolution",
    "EntityResolutionError",
    "EntityResolutionOutcome",
    "MatchResult",
    "MatchTier",
    "ResolutionCandidate",
    "ResolutionOrigin",
    "ResolutionScope",
    "build_name_resolution_map",
    "closest_resolution_candidates",
    "find_entity_by_email",
    "find_matching_entity",
    "is_name_variant_match",
    "record_entity_resolution",
    "resolve_entity",
    "resolve_journal_entity",
    "validate_aka_uniqueness",
    # Activity
    "iter_detected_entity_names_since",
    "load_detected_entities_recent",
    # Observations
    "add_observation",
    "count_observations",
    "load_observations",
    "observation_day_counts",
    "observations_file_path",
    "record_observation_ops",
    "save_observations",
    "load_entity_voiceprints_file",
    "load_existing_voiceprint_keys",
    "normalize_embedding",
    # Formatting
    "format_entities",
    "format_observations",
]
