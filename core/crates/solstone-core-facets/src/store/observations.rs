// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use std::path::Path;

pub use solstone_core_entity::{
    HistoryEntry, IncomingObservationRow, ObservationChange, ObservationEntityResolution,
    ObservationErrorSource, ObservationLookup, ObservationLookupError, ObservationOperationCounts,
    ObservationPage, ObservationPageItem, ObservationParseSource, ObservationReadOrder,
    ObservationReadQuery, ObservationRow, ObservationStoreError, ObservationSummary,
    ObservationWriteError, ObservationWriteOutcome, ParsedObservations, PreparedObservationBatch,
    Retired, add_observation, apply_observation_change, apply_ops_to_parsed, count_observations,
    facet_entity_observations_path, hold_facet_trust_lock, load_observations_for_query,
    normalize_observation_content, observation_day_counts, observation_summary,
    parse_observation_file, read_live_observations, record_observation_ops_strict,
    resolve_observation_entity_dir, serialize_observation_rows,
};

use super::error::{FacetStoreError, FacetWriteError};
use super::facet_entities::list_scoped_facet_entities;

/// Read facet-scoped entity observations without interpreting JSONL records.
pub fn read_facet_entity_observations(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
) -> Result<Option<String>, FacetStoreError> {
    let path = facet_entity_observations_path(journal_root, facet_dir, entity_dir)?;
    if !solstone_core_journal_io::path_lexists(&path)? {
        return Ok(None);
    }
    solstone_core_journal_io::read_text(&path, String::new())
        .map(Some)
        .map_err(Into::into)
}

/// Atomically replace facet-scoped entity observations without parsing JSONL.
pub fn write_facet_entity_observations(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
    content: &str,
) -> Result<(), FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let path = facet_entity_observations_path(journal_root, facet_dir, entity_dir)
        .map_err(FacetStoreError::from)?;
    solstone_core_journal_io::write_text(
        &path,
        content,
        solstone_core_journal_io::AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(FacetWriteError::ContentWrite)
}

/// Validate operations against frozen JSONL using the owner's normal target rules.
pub fn validate_observation_operations(
    snapshot: Option<&str>,
    operations: &[Value],
    source_day: Option<&str>,
) -> Result<(), ObservationWriteError> {
    let parsed = if let Some(text) = snapshot {
        parse_observation_file(
            text,
            ObservationParseSource::Path(Path::new("frozen-observation-snapshot")),
        )?
    } else {
        ParsedObservations {
            full_rows: Vec::new(),
        }
    };
    apply_ops_to_parsed(&parsed, operations, source_day).map(|_| ())
}

/// A single observation file replacement prepared before any daily side effect.
pub fn prepare_observation_batch(
    root: &Path,
    facet: &str,
    entity: &str,
    operations: &[Value],
    source_day: Option<&str>,
) -> Result<PreparedObservationBatch, ObservationWriteError> {
    let _trust = hold_facet_trust_lock(root)?;
    let entity_dir = match resolve_observation_entity_dir(root, facet, entity)
        .map_err(|e| ObservationWriteError::Resolve(e.to_string()))?
    {
        ObservationEntityResolution::Resolved { entity_dir } => entity_dir,
        ObservationEntityResolution::NoSuchEntity => {
            return Err(ObservationWriteError::Conflict {
                message: "observation entity disappeared".into(),
            });
        }
    };
    let facet_id = super::declaration::facet_write_identity(root, facet)
        .map_err(|message| ObservationWriteError::Conflict { message })?;
    let scoped = list_scoped_facet_entities(root, facet, true, true)
        .map_err(|e| ObservationWriteError::Resolve(e.to_string()))?;
    let binding = scoped
        .into_iter()
        .find(|entity| entity.relationship_dir == entity_dir && !entity.detached && !entity.blocked)
        .ok_or_else(|| ObservationWriteError::Conflict {
            message: "observation target is not attached and unblocked".into(),
        })?;
    let before = read_facet_entity_observations(root, facet, &entity_dir).map_err(|e| {
        ObservationWriteError::Read(ObservationStoreError::Read(
            solstone_core_journal_io::ReadError::Io {
                path: facet_entity_observations_path(root, facet, &entity_dir).unwrap_or_default(),
                source: std::io::Error::other(e.to_string()),
            },
        ))
    })?;
    let snapshot = if let Some(ref text) = before {
        parse_observation_file(
            text,
            ObservationParseSource::Path(Path::new("observations.jsonl")),
        )?
    } else {
        ParsedObservations {
            full_rows: Vec::new(),
        }
    };
    let (after_rows, counts, _) = apply_ops_to_parsed(&snapshot, operations, source_day)?;
    let content = serialize_observation_rows(&after_rows);
    Ok(PreparedObservationBatch {
        facet: facet.into(),
        facet_id,
        entity_id: binding.entity_id,
        relationship: binding.relationship,
        entity_dir,
        before,
        after: content,
        counts,
    })
}

/// Publish prepared observation batch under the facet trust lock.
pub fn publish_observation_batch(
    root: &Path,
    batch: &PreparedObservationBatch,
    allow_before: bool,
    receipt: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let _trust = hold_facet_trust_lock(root).map_err(|e| e.to_string())?;
    super::declaration::require_facet_write_identity(root, &batch.facet, &batch.facet_id)?;
    let scoped =
        list_scoped_facet_entities(root, &batch.facet, true, true).map_err(|e| e.to_string())?;
    if !scoped.iter().any(|entity| {
        entity.entity_id == batch.entity_id
            && entity.relationship_dir == batch.entity_dir
            && entity.relationship == batch.relationship
            && !entity.detached
            && !entity.blocked
    }) {
        return Err("conflict: observation entity relationship changed after preparation".into());
    }
    let current = read_facet_entity_observations(root, &batch.facet, &batch.entity_dir)
        .map_err(|e| e.to_string())?;
    if current.as_deref() != Some(batch.after.as_str()) {
        if !allow_before || current != batch.before {
            return Err("conflict: observation batch no longer matches its prepared state".into());
        }
        let parsed = parse_observation_file(
            &batch.after,
            ObservationParseSource::Path(Path::new("observations.jsonl")),
        )
        .map_err(|e| e.to_string())?;
        apply_observation_change(
            root,
            &batch.facet,
            &batch.entity_dir,
            ObservationChange::ReplaceFullSet {
                rows: parsed.full_rows,
            },
        )
        .map_err(|e| e.to_string())?;
    }
    receipt()
}
