// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use solstone_core_journal_io::AtomicWriteOptions;
use solstone_core_journal_io::DirEntryKind;
use solstone_core_journal_io::JournalSnapshot;
use solstone_core_journal_io::JsonWriteOptions;
use solstone_core_journal_io::LockOptions;
use solstone_core_journal_io::MalformedPolicy;
use solstone_core_journal_io::PathOrDay;
use solstone_core_journal_io::SnapshotError;
use solstone_core_journal_io::append_jsonl;
use solstone_core_journal_io::atomic_replace;
use solstone_core_journal_io::contained_path;
use solstone_core_journal_io::day_dirs;
use solstone_core_journal_io::hold_lock;
use solstone_core_journal_io::iter_segments;
use solstone_core_journal_io::list_dir_entries;
use solstone_core_journal_io::path_lexists;
use solstone_core_journal_io::read_bytes;
use solstone_core_journal_io::read_json;
use solstone_core_journal_io::read_jsonl;
use solstone_core_journal_io::read_text;
use solstone_core_journal_io::restore_snapshot;
use solstone_core_journal_io::write_json;
use solstone_core_journal_io::write_jsonl;

use crate::{
    EntityLifecycleError, EntityOperationContext, EntityOperationKind, EntityStoreError,
    EntityWriteError, hold_entity_trust_lock, read_entity_identity, save_entity_identity,
};

use super::facet_links::{FolderState, LinkDirs, LinkFieldPolicy, LinkFolderError};
use super::lifecycle::resolve_entity_dir;
use super::observations::{
    ObservationChange, ObservationParseSource, apply_observation_change, parse_observation_file,
};

use super::merge_rollback::MergeRollback;
use super::voiceprints::{
    EMBEDDING_WIDTH, EncoderIdentity, VoiceprintArchive, VoiceprintEnvelope, read_voiceprints_npz,
    resolve_voiceprint_path, write_voiceprints_npz,
};

const PHASES: [&str; 8] = [
    "voiceprints",
    "facets",
    "segments",
    "activities",
    "cleanup",
    "observation relation remap",
    "history",
    "audit",
];
type VoiceprintKey = (Option<Value>, Option<Value>, Option<Value>, Option<Value>);
type FailureInjector = dyn Fn(&str, usize) -> bool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EntityMergeOptions {
    pub keep_source_as_aka: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntityMergePreview {
    pub source_id: String,
    pub target_id: String,
    pub target_identity: Value,
    pub aliases_added: usize,
    pub emails_added: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EntityMergeReport {
    pub merge_id: String,
    pub source_id: String,
    pub target_id: String,
    pub completed_phases: Vec<String>,
    pub aliases_added: usize,
    pub emails_added: usize,
    /// Final durable-operation counters, including phases that run after the
    /// audit record is first assembled.
    pub counts: Value,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct VoiceprintMergeStats {
    pub added: usize,
    pub skipped_duplicate: usize,
    pub target_total: usize,
}
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct FacetMergeStats {
    pub moved_count: usize,
    pub merged_count: usize,
    pub observations_appended: usize,
    pub touched_facets: Vec<String>,
    pub removed_source_dirs: Vec<String>,
    pub unreadable_links: usize,
}
#[derive(Debug, Default)]
struct MergeStats {
    voiceprints_added: usize,
    voiceprints_skipped_duplicate: usize,
    voiceprints_target_total: usize,
    facets_moved: usize,
    facets_merged: usize,
    facets_observations_appended: usize,
    segments_labels_rewritten: usize,
    segments_corrections_rewritten: usize,
    segments_files_scanned: usize,
    activities_records_rewritten: usize,
    activities_fields_rewritten: usize,
    activities_files_scanned: usize,
    activities_files_rewritten: usize,
    observation_relations_rewritten: usize,
}
fn audit_counts(
    stats: &MergeStats,
    akas_added: usize,
    emails_added: usize,
    principal_transferred: bool,
) -> Value {
    json!({"identity":{"akas_added":akas_added,"emails_added":emails_added,"principal_transferred":principal_transferred},"voiceprints":{"added":stats.voiceprints_added,"skipped_duplicate":stats.voiceprints_skipped_duplicate,"target_total":stats.voiceprints_target_total},"facets":{"moved":stats.facets_moved,"merged":stats.facets_merged,"observations_appended":stats.facets_observations_appended,"observation_relations_rewritten":stats.observation_relations_rewritten},"segments":{"labels_rewritten":stats.segments_labels_rewritten,"corrections_rewritten":stats.segments_corrections_rewritten,"files_scanned":stats.segments_files_scanned,"errors":0},"activities":{"records_rewritten":stats.activities_records_rewritten,"fields_rewritten":stats.activities_fields_rewritten,"files_scanned":stats.activities_files_scanned,"files_rewritten":stats.activities_files_rewritten,"errors":0},"edges":{"rows_folded":null,"self_edges_dropped":null,"error":null}})
}

#[derive(Debug)]
pub enum EntityMergeError {
    Refused(String),
    VoiceprintEncoderMismatch {
        source_entity_id: String,
        target_entity_id: String,
        source_encoder_id: String,
        target_encoder_id: String,
    },
    Read(EntityStoreError),
    Write(EntityWriteError),
    Lifecycle(EntityLifecycleError),
    Snapshot(SnapshotError),
    Audit(solstone_core_journal_io::AppendError),
    Failed {
        failed_phase: String,
        report: Box<EntityMergeReport>,
        rollback_error: Option<String>,
    },
}
impl fmt::Display for EntityMergeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(message) => f.write_str(message),
            Self::VoiceprintEncoderMismatch {
                source_entity_id,
                target_entity_id,
                source_encoder_id,
                target_encoder_id,
            } => write!(
                f,
                "voiceprint encoders differ for {source_entity_id} ({source_encoder_id}) and {target_entity_id} ({target_encoder_id})"
            ),
            Self::Read(error) => error.fmt(f),
            Self::Write(error) => error.fmt(f),
            Self::Lifecycle(error) => error.fmt(f),
            Self::Snapshot(error) => error.fmt(f),
            Self::Audit(error) => error.fmt(f),
            Self::Failed {
                failed_phase,
                rollback_error: Some(error),
                ..
            } => {
                write!(f, "entity merge failed during {failed_phase}: {error}")
            }
            Self::Failed { failed_phase, .. } => {
                write!(f, "entity merge failed during {failed_phase}")
            }
        }
    }
}
impl Error for EntityMergeError {}
impl From<EntityStoreError> for EntityMergeError {
    fn from(error: EntityStoreError) -> Self {
        Self::Read(error)
    }
}
impl From<EntityWriteError> for EntityMergeError {
    fn from(error: EntityWriteError) -> Self {
        Self::Write(error)
    }
}
impl From<EntityLifecycleError> for EntityMergeError {
    fn from(error: EntityLifecycleError) -> Self {
        Self::Lifecycle(error)
    }
}
impl From<SnapshotError> for EntityMergeError {
    fn from(error: SnapshotError) -> Self {
        Self::Snapshot(error)
    }
}

pub fn preview_entity_merge(
    journal: &Path,
    source_id: &str,
    target_id: &str,
    options: EntityMergeOptions,
) -> Result<EntityMergePreview, EntityMergeError> {
    let plan = plan_merge(journal, source_id, target_id, options)?;
    check_facet_links(journal, source_id, target_id)?;
    Ok(EntityMergePreview {
        source_id: source_id.to_owned(),
        target_id: target_id.to_owned(),
        target_identity: plan.target_after,
        aliases_added: plan.aliases_added,
        emails_added: plan.emails_added,
    })
}

pub fn commit_entity_merge(
    journal: &Path,
    source_id: &str,
    target_id: &str,
    options: EntityMergeOptions,
    fallback_encoder: &EncoderIdentity,
) -> Result<EntityMergeReport, EntityMergeError> {
    commit_entity_merge_with_injector(
        journal,
        source_id,
        target_id,
        options,
        fallback_encoder,
        None,
    )
}

pub(crate) fn commit_entity_merge_with_injector(
    journal: &Path,
    source_id: &str,
    target_id: &str,
    options: EntityMergeOptions,
    fallback_encoder: &EncoderIdentity,
    injector: Option<&FailureInjector>,
) -> Result<EntityMergeReport, EntityMergeError> {
    let canonical_journal =
        solstone_core_journal_io::realpath_non_strict(journal).map_err(SnapshotError::Path)?;
    let journal = canonical_journal.as_path();
    let _trust = hold_entity_trust_lock(journal).map_err(EntityWriteError::TrustLock)?;
    if let Some(recovered) = super::merge_rollback::recover(journal)?
        && recovered["operation"] == "merge"
        && recovered["report"]["source_id"] == source_id
        && recovered["report"]["target_id"] == target_id
    {
        return serde_json::from_value(recovered["report"].clone())
            .map_err(|error| EntityMergeError::Refused(error.to_string()));
    }
    let plan = plan_merge(journal, source_id, target_id, options)?;
    // A link that can't join the target's is refused here, before anything
    // changes, rather than rolled back from the facets phase.
    check_facet_links(journal, source_id, target_id)?;
    ensure_voiceprint_merge_compatible(journal, source_id, target_id)?;
    let source_dir = resolve_entity_dir(journal, source_id)?;
    let target_dir = resolve_entity_dir(journal, target_id)?;
    let merge_id = format!(
        "em_{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| EntityMergeError::Refused(error.to_string()))?
            .as_nanos()
    );
    let mut rollback = MergeRollback::begin(journal)?;
    for path in [
        format!("entities/{source_dir}"),
        format!("entities/{target_dir}"),
    ] {
        rollback.capture(journal, &path)?;
    }
    let mut report = EntityMergeReport {
        merge_id: merge_id.clone(),
        source_id: source_id.to_owned(),
        target_id: target_id.to_owned(),
        completed_phases: Vec::new(),
        aliases_added: plan.aliases_added,
        emails_added: plan.emails_added,
        counts: Value::Null,
    };
    let mut touched_facets = Vec::new();
    let mut removed_source_dirs = Vec::new();
    let mut stats = MergeStats::default();
    for phase in PHASES {
        let result = match phase {
            "voiceprints" => merge_voiceprints(
                journal,
                source_id,
                target_id,
                fallback_encoder,
                LockOptions::default(),
            )
            .map(|result| {
                stats.voiceprints_added = result.added;
                stats.voiceprints_skipped_duplicate = result.skipped_duplicate;
                stats.voiceprints_target_total = result.target_total;
            }),
            "facets" => merge_facets(journal, source_id, target_id, Some(&mut rollback), injector)
                .map(|result| {
                    stats.facets_moved = result.moved_count;
                    stats.facets_merged = result.merged_count;
                    stats.facets_observations_appended = result.observations_appended;
                    touched_facets = result.touched_facets;
                    removed_source_dirs = result.removed_source_dirs;
                }),
            "history" => (|| {
                let saved = save_entity_identity(journal, target_id, &plan.target_after, Some(&EntityOperationContext {
                    kind: EntityOperationKind::Merge, caller: Value::Null, actor: Value::Null,
                    metadata: json!({"merge_id":merge_id,"source_id":source_id,"target_id":target_id}),
                })).map_err(EntityMergeError::Write)?;
                // The restore guard refuses any restore across this event.
                if saved.event.is_none() {
                    return Err(EntityMergeError::Refused(
                        "merge history event was not written".to_owned(),
                    ));
                }
                Ok(())
            })(),
            "cleanup" => cleanup_merge(
                journal,
                &source_dir,
                &removed_source_dirs,
                Some(&mut rollback),
            ),
            "audit" => (|| {
                // The merge is permanent: record the source id as merged, so it
                // is never created again and derived readers can follow it.
                rollback.capture(journal, super::retired::RETIRED_ENTITIES_FILE)?;
                super::retired::record_merged_entity(
                    journal,
                    source_id,
                    &super::retired::MergedEntity {
                        dir: source_dir.clone(),
                        successor: target_id.to_owned(),
                        name: Some(plan.source_display_name.clone()),
                        merge_id: Some(merge_id.clone()),
                        at: Some(chrono::Utc::now().to_rfc3339()),
                    },
                )
                .map_err(EntityMergeError::Refused)?;
                let path = contained_path(journal, "logs/entity-merges.jsonl")
                    .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
                rollback.capture(journal, "logs/entity-merges.jsonl")?;
                let ts = u64::try_from(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(|error| EntityMergeError::Refused(error.to_string()))?
                        .as_millis(),
                )
                .map_err(|_| {
                    EntityMergeError::Refused("merge audit timestamp exceeds u64".to_owned())
                })?;
                append_jsonl(path, &json!({"ts":ts,"merge_id":merge_id,"source_id":source_id,"source_display_name":plan.source_display_name,"target_id":target_id,"target_display_name":plan.target_display_name,"principal_transferred":plan.principal_transferred,"counts":audit_counts(&stats, plan.aliases_added, plan.emails_added, plan.principal_transferred),"caller":Value::Null})).map_err(EntityMergeError::Audit)
            })(),
            "segments" => {
                merge_segment_labels(journal, source_id, target_id, Some(&mut rollback), injector)
                    .map(|result| {
                        stats.segments_labels_rewritten = result.labels_rewritten;
                        stats.segments_corrections_rewritten = result.corrections_rewritten;
                        stats.segments_files_scanned = result.files_scanned;
                    })
            }
            "activities" => {
                merge_activities(journal, source_id, target_id, Some(&mut rollback), injector).map(
                    |result| {
                        stats.activities_records_rewritten = result.records_rewritten;
                        stats.activities_fields_rewritten = result.fields_rewritten;
                        stats.activities_files_scanned = result.files_scanned;
                        stats.activities_files_rewritten = result.files_rewritten;
                    },
                )
            }
            "observation relation remap" => merge_observation_relations(
                journal,
                source_id,
                target_id,
                Some(&mut rollback),
                injector,
            )
            .map(|result| {
                stats.observation_relations_rewritten = result.rows_rewritten;
            }),
            _ => unreachable!("merge phase list is fixed"),
        };
        let result = result.and_then(|()| {
            rollback
                .checkpoint(journal)
                .map_err(EntityMergeError::Snapshot)
        });
        if let Err(error) = result {
            let rollback_error = rollback
                .restore(journal)
                .err()
                .map(|rollback| rollback.to_string());
            return Err(EntityMergeError::Failed {
                failed_phase: phase.to_owned(),
                report: Box::new(report),
                rollback_error: rollback_error.or_else(|| Some(error.to_string())),
            });
        }
        if !matches!(
            phase,
            "facets" | "segments" | "activities" | "observation relation remap"
        ) && injector.is_some_and(|injector| injector(phase, 0))
        {
            let rollback_error = rollback
                .restore(journal)
                .err()
                .map(|error| error.to_string());
            return Err(EntityMergeError::Failed {
                failed_phase: phase.to_owned(),
                report: Box::new(report),
                rollback_error: rollback_error
                    .or_else(|| Some(format!("injected failure after {phase} artifact 0"))),
            });
        }
        report.completed_phases.push(phase.to_owned());
    }
    report.counts = audit_counts(
        &stats,
        plan.aliases_added,
        plan.emails_added,
        plan.principal_transferred,
    );
    // Index changes are derived work. Once this marker is durable, neither
    // an index lock refusal nor a restart may roll source state back.
    let committed = json!({"operation":"merge", "report":report});
    rollback.commit_source(journal, "merge", &committed["report"])?;
    // Connections read merged ids through `entities/retired.json` and the
    // merge log, so nothing in the edge index is rewritten here. Discovery
    // clusters were built from the old ids; the cache is removed before the
    // recovery record finishes, so a restart in between removes it too.
    if injector.is_some_and(|injector| injector("discovery cache", 0)) {
        return Err(EntityMergeError::Refused(
            "entity merge committed; cleanup pending: injected failure".to_owned(),
        ));
    }
    remove_discovery_cache(journal);
    rollback.finish(journal).map_err(|error| {
        EntityMergeError::Refused(format!(
            "entity merge committed; recovery cleanup pending: {error}"
        ))
    })?;
    drop(_trust);
    Ok(report)
}

/// Remove discovery clusters derived before an entity merge. Best effort: the
/// cache is rebuilt on its own schedule and a missing file is normal.
pub(super) fn remove_discovery_cache(journal: &Path) {
    let _ = solstone_core_journal_io::remove_file(journal, "awareness/discovery_clusters.json");
}

fn inject_failure(
    injector: Option<&FailureInjector>,
    phase: &str,
    artifact_index: usize,
) -> Result<(), EntityMergeError> {
    if injector.is_some_and(|injector| injector(phase, artifact_index)) {
        return Err(EntityMergeError::Refused(format!(
            "injected failure after {phase} artifact {artifact_index}"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct ObservationRelationMergeStats {
    pub rows_rewritten: usize,
}
pub(crate) fn merge_observation_relations(
    journal: &Path,
    source_id: &str,
    target_id: &str,
    mut rollback: Option<&mut MergeRollback>,
    injector: Option<&FailureInjector>,
) -> Result<ObservationRelationMergeStats, EntityMergeError> {
    let facets = contained_path(journal, "facets")
        .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
    let mut stats = ObservationRelationMergeStats::default();
    let mut artifact_index = 0;
    for facet in
        list_dir_entries(&facets).map_err(|error| EntityMergeError::Refused(error.to_string()))?
    {
        if facet.kind != DirEntryKind::Directory {
            continue;
        }
        let facet_name = facet.name.to_string_lossy();
        let dirs = LinkDirs::for_facet(journal, &facet_name);
        for entity_dir in dirs.all_folders().map_err(refused)? {
            let path = dirs.observations_path(&entity_dir).map_err(refused)?;
            if !path_lexists(&path).map_err(|error| EntityMergeError::Refused(error.to_string()))? {
                continue;
            }
            let text = read_text(&path, String::new())
                .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
            let parsed = parse_observation_file(&text, ObservationParseSource::Path(&path))
                .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
            for (row_index, row) in parsed.full_rows.iter().enumerate() {
                if let Some(relation) = row.relation.as_ref().and_then(Value::as_object)
                    && relation.get("target_entity_id").and_then(Value::as_str) == Some(source_id)
                {
                    capture_rollback_file(&mut rollback, journal, &path)?;
                    apply_observation_change(
                        journal,
                        &facet_name,
                        &entity_dir,
                        ObservationChange::EditInPlace {
                            full_set_index: row_index,
                            rewrite: json!({
                                "target_entity_id": target_id,
                            }),
                        },
                    )
                    .map_err(|error| EntityMergeError::Refused(error.to_string()))?;

                    stats.rows_rewritten += 1;
                    inject_failure(injector, "observation relation remap", artifact_index)?;
                    artifact_index += 1;
                }
            }
        }
    }
    Ok(stats)
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct ActivityMergeStats {
    pub files_scanned: usize,
    pub files_rewritten: usize,
    pub records_rewritten: usize,
    pub fields_rewritten: usize,
}
pub(crate) fn merge_activities(
    journal: &Path,
    source_id: &str,
    target_id: &str,
    mut rollback: Option<&mut MergeRollback>,
    injector: Option<&FailureInjector>,
) -> Result<ActivityMergeStats, EntityMergeError> {
    let facets = contained_path(journal, "facets")
        .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
    let mut stats = ActivityMergeStats::default();
    let mut artifact_index = 0;
    for facet in
        list_dir_entries(&facets).map_err(|error| EntityMergeError::Refused(error.to_string()))?
    {
        if facet.kind != DirEntryKind::Directory {
            continue;
        }
        let activities = facet.path.join("activities");
        for file in list_dir_entries(&activities)
            .map_err(|error| EntityMergeError::Refused(error.to_string()))?
        {
            if file.kind != DirEntryKind::File
                || file.path.extension().and_then(|value| value.to_str()) != Some("jsonl")
            {
                continue;
            }
            stats.files_scanned += 1;
            let mut rows: Vec<Value> =
                read_jsonl(&file.path, Vec::new(), MalformedPolicy::Raise)
                    .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
            let mut file_changed = false;
            for row in rows.iter_mut() {
                let mut changed = false;
                if let Some(object) = row.as_object_mut() {
                    if let Some(active) = object
                        .get_mut("active_entities")
                        .and_then(Value::as_array_mut)
                    {
                        for value in active.iter_mut() {
                            if value.as_str() == Some(source_id) {
                                *value = Value::String(target_id.to_owned());
                                stats.fields_rewritten += 1;
                                changed = true;
                            }
                        }
                    }
                    for (container, keys) in [
                        ("participation", &["entity_id"][..]),
                        (
                            "commitments",
                            &["owner_entity_id", "counterparty_entity_id"][..],
                        ),
                        (
                            "closures",
                            &["owner_entity_id", "counterparty_entity_id"][..],
                        ),
                        (
                            "decisions",
                            &["owner_entity_id", "counterparty_entity_id"][..],
                        ),
                        ("relations", &["from_entity_id", "to_entity_id"][..]),
                    ] {
                        if let Some(items) = object.get_mut(container).and_then(Value::as_array_mut)
                        {
                            for item in items.iter_mut() {
                                if let Some(item) = item.as_object_mut() {
                                    for key in keys {
                                        if item.get(*key).and_then(Value::as_str) == Some(source_id)
                                        {
                                            item.insert(
                                                (*key).to_owned(),
                                                Value::String(target_id.to_owned()),
                                            );
                                            stats.fields_rewritten += 1;
                                            changed = true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if changed {
                    stats.records_rewritten += 1;
                    file_changed = true;
                }
            }
            if file_changed {
                capture_rollback_file(&mut rollback, journal, &file.path)?;
                write_jsonl(&file.path, rows, AtomicWriteOptions::default())
                    .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
                inject_failure(injector, "activities", artifact_index)?;
                artifact_index += 1;
                stats.files_rewritten += 1;
            }
        }
    }
    Ok(stats)
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct SegmentMergeStats {
    pub files_scanned: usize,
    pub labels_rewritten: usize,
    pub corrections_rewritten: usize,
}

/// Whether a speaker file names `id` as a whole JSON string. Ids are slugs
/// (`[a-z0-9_]`), so they appear unescaped; a bare byte search would also
/// match every longer id that contains this one (`sam` in `samantha_ortiz`)
/// and lock thousands of files the merge never changes.
fn mentions_id(raw: &[u8], id: &str) -> bool {
    let quoted = format!("\"{id}\"");
    raw.windows(quoted.len())
        .any(|bytes| bytes == quoted.as_bytes())
}

pub(crate) fn merge_segment_labels(
    journal: &Path,
    source_id: &str,
    target_id: &str,
    mut rollback: Option<&mut MergeRollback>,
    injector: Option<&FailureInjector>,
) -> Result<SegmentMergeStats, EntityMergeError> {
    let mut stats = SegmentMergeStats::default();
    let mut artifact_index = 0;
    for day in day_dirs(journal)
        .map_err(|error| EntityMergeError::Refused(error.to_string()))?
        .into_values()
    {
        for segment in iter_segments(journal, PathOrDay::Directory(&day))
            .map_err(|error| EntityMergeError::Refused(error.to_string()))?
        {
            let path = segment.path().join("talents/speaker_labels.json");
            if path_lexists(&path).map_err(|error| EntityMergeError::Refused(error.to_string()))? {
                stats.files_scanned += 1;
                let raw = read_bytes(&path, Vec::new())
                    .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
                if !mentions_id(&raw, source_id) {
                    continue;
                }
                let newly_locked = match rollback.as_deref_mut() {
                    Some(rollback) => rollback.lock_file(&path)?,
                    None => false,
                };
                let mut value: Value = read_json(&path, Value::Null, MalformedPolicy::Raise)
                    .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
                let mut changed = false;
                if let Some(labels) = value.get_mut("labels").and_then(Value::as_array_mut) {
                    for label in labels.iter_mut() {
                        if let Some(object) = label.as_object_mut()
                            && object.get("speaker").and_then(Value::as_str) == Some(source_id)
                        {
                            object
                                .insert("speaker".to_owned(), Value::String(target_id.to_owned()));
                            changed = true;
                        }
                    }
                }
                if !changed
                    && newly_locked
                    && let Some(rollback) = rollback.as_deref_mut()
                {
                    rollback.unlock_file(&path);
                }
                if changed {
                    capture_rollback_file(&mut rollback, journal, &path)?;
                    write_json(
                        &path,
                        &value,
                        JsonWriteOptions {
                            indent: Some(2),
                            sort_keys: false,
                            mode: None,
                        },
                    )
                    .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
                    inject_failure(injector, "segments", artifact_index)?;
                    artifact_index += 1;
                    stats.labels_rewritten += 1;
                }
            }
            let path = segment.path().join("talents/speaker_corrections.json");
            if !path_lexists(&path).map_err(|error| EntityMergeError::Refused(error.to_string()))? {
                continue;
            }
            stats.files_scanned += 1;
            let raw = read_bytes(&path, Vec::new())
                .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
            if !mentions_id(&raw, source_id) {
                continue;
            }
            let newly_locked = match rollback.as_deref_mut() {
                Some(rollback) => rollback.lock_file(&path)?,
                None => false,
            };
            let mut value: Value = read_json(&path, Value::Null, MalformedPolicy::Raise)
                .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
            let mut changed = false;
            if let Some(corrections) = value.get_mut("corrections").and_then(Value::as_array_mut) {
                for correction in corrections.iter_mut() {
                    if let Some(object) = correction.as_object_mut() {
                        for field in ["original_speaker", "corrected_speaker"] {
                            if object.get(field).and_then(Value::as_str) == Some(source_id) {
                                object
                                    .insert(field.to_owned(), Value::String(target_id.to_owned()));
                                changed = true;
                            }
                        }
                    }
                }
            }
            if !changed
                && newly_locked
                && let Some(rollback) = rollback.as_deref_mut()
            {
                rollback.unlock_file(&path);
            }
            if changed {
                capture_rollback_file(&mut rollback, journal, &path)?;
                write_json(
                    &path,
                    &value,
                    JsonWriteOptions {
                        indent: Some(2),
                        sort_keys: false,
                        mode: None,
                    },
                )
                .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
                inject_failure(injector, "segments", artifact_index)?;
                artifact_index += 1;
                stats.corrections_rewritten += 1;
            }
        }
    }
    Ok(stats)
}

fn capture_rollback_file(
    rollback: &mut Option<&mut MergeRollback>,
    journal: &Path,
    path: &Path,
) -> Result<(), EntityMergeError> {
    let Some(rollback) = rollback.as_deref_mut() else {
        return Ok(());
    };
    let relative = super::merge_rollback::journal_relative(
        path.strip_prefix(journal)
            .map_err(|error| EntityMergeError::Refused(error.to_string()))?,
    )?;
    rollback.capture(journal, &relative)?;
    Ok(())
}

/// Every link folder in `facet` that links `entity_id` by its effective id,
/// the folder named by the id first. Links are matched by that id alone, so a
/// folder whose link names the target is never the source's to fold away.
fn source_link_folders(
    journal: &Path,
    facet: &str,
    entity_id: &str,
    target_id: &str,
) -> Result<Vec<String>, EntityMergeError> {
    let dirs = LinkDirs::for_facet(journal, facet);
    let mut folders: Vec<String> = dirs
        .folders_for(entity_id)
        .map_err(refused)?
        .into_iter()
        .filter(|link| link.entity_id != target_id)
        .map(|link| link.dir)
        .collect();
    // Notes under the source's id that no link claimed go with it too.
    if !folders.is_empty() && dirs.state(entity_id).map_err(refused)? == FolderState::Orphan {
        folders.push(entity_id.to_owned());
    }
    Ok(folders)
}

#[cfg(test)]
pub(crate) fn source_link_folders_for_test(
    journal: &Path,
    facet: &str,
    source_id: &str,
    target_id: &str,
) -> Vec<String> {
    source_link_folders(journal, facet, source_id, target_id).unwrap()
}

/// Whether every link the source has can join the target's, checked without
/// writing anything.
fn check_facet_links(
    journal: &Path,
    source_id: &str,
    target_id: &str,
) -> Result<(), EntityMergeError> {
    let facets = contained_path(journal, "facets").map_err(refused)?;
    if !path_lexists(&facets).map_err(refused)? {
        return Ok(());
    }
    for entry in list_dir_entries(&facets).map_err(refused)? {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let facet = entry.name.to_string_lossy().into_owned();
        let dirs = LinkDirs::for_facet(journal, &facet);
        let source_folders = source_link_folders(journal, &facet, source_id, target_id)?;
        if source_folders.is_empty() {
            continue;
        }
        let incoming: Vec<(&LinkDirs, &str)> = source_folders
            .iter()
            .map(|dir| (&dirs, dir.as_str()))
            .collect();
        dirs.check_take_in_all(target_id, &incoming)
            .map_err(refused)?;
    }
    Ok(())
}

fn refused(error: impl fmt::Display) -> EntityMergeError {
    EntityMergeError::Refused(error.to_string())
}

/// Fold every link the source has in each facet into the target's link, in
/// the folder named by the target's id. The source folders are removed at
/// cleanup; every folder is captured before its first write.
pub(crate) fn merge_facets(
    journal: &Path,
    source_id: &str,
    target_id: &str,
    mut rollback: Option<&mut MergeRollback>,
    injector: Option<&FailureInjector>,
) -> Result<FacetMergeStats, EntityMergeError> {
    let facets = contained_path(journal, "facets").map_err(refused)?;
    let mut stats = FacetMergeStats::default();
    let mut artifact_index = 0;
    for entry in list_dir_entries(&facets).map_err(refused)? {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let facet = entry.name.to_string_lossy().into_owned();
        let dirs = LinkDirs::for_facet(journal, &facet);
        let source_folders = source_link_folders(journal, &facet, source_id, target_id)?;
        if source_folders.is_empty() {
            continue;
        }
        let (_, unreadable) = dirs.scan().map_err(refused)?;
        stats.unreadable_links += unreadable.len();
        let target_was_linked = dirs.find(target_id).map_err(refused)?.is_some();
        let mut captured = HashSet::new();
        let mut capture = |relative: &str| -> Result<(), LinkFolderError> {
            if let Some(rollback) = rollback.as_deref_mut()
                && captured.insert(relative.to_owned())
            {
                rollback
                    .capture(journal, relative)
                    .map_err(|error| LinkFolderError::Hook(error.to_string()))?;
            }
            Ok(())
        };
        for source_dir in &source_folders {
            capture(&dirs.folder_rel(source_dir)).map_err(refused)?;
        }
        // Every folder the source has here joins the target's in one step.
        let incoming: Vec<(&LinkDirs, &str)> = source_folders
            .iter()
            .map(|dir| (&dirs, dir.as_str()))
            .collect();
        let (target_dir, rows) = dirs
            .take_in_all(target_id, &incoming, LinkFieldPolicy::Merge, &mut capture)
            .map_err(refused)?;
        inject_failure(injector, "facets", artifact_index)?;
        artifact_index += 1;
        stats.observations_appended += rows.added;
        for source_dir in &source_folders {
            // A source folder already named by the target's id was relinked in
            // place and is the result; cleanup never removes it.
            if *source_dir != target_dir {
                stats.removed_source_dirs.push(dirs.folder_rel(source_dir));
            }
        }
        if target_was_linked {
            stats.merged_count += 1;
        } else {
            stats.moved_count += 1;
        }
        stats.touched_facets.push(facet.clone());
    }
    Ok(stats)
}
fn cleanup_merge(
    journal: &Path,
    source_dir: &str,
    removed_source_dirs: &[String],
    mut rollback: Option<&mut MergeRollback>,
) -> Result<(), EntityMergeError> {
    for path in removed_source_dirs {
        if let Some(rollback) = rollback.as_deref_mut() {
            rollback.capture(journal, path)?;
        }
        restore_snapshot(journal, &JournalSnapshot::Missing { path: path.clone() })?;
    }
    restore_snapshot(
        journal,
        &JournalSnapshot::Missing {
            path: format!("entities/{source_dir}"),
        },
    )
    .map_err(Into::into)
}

pub(crate) fn merge_voiceprints(
    journal: &Path,
    source_id: &str,
    target_id: &str,
    fallback_encoder: &EncoderIdentity,
    lock_options: LockOptions,
) -> Result<VoiceprintMergeStats, EntityMergeError> {
    let (_source_dir, source_path) = resolve_voiceprint_path(journal, source_id, false)?;
    let (_target_dir, target_path) = resolve_voiceprint_path(journal, target_id, false)?;
    let _lock = hold_lock(&target_path, lock_options)
        .map_err(|error| EntityWriteError::TrustLock(error.into()))?;
    let source = load_voiceprints(&source_path)?;
    let target = load_voiceprints(&target_path)?;
    ensure_loaded_archives_merge_compatible(source_id, target_id, &source, &target)?;
    let Some(source) = source else {
        let target_total = target.map_or(0, |archive| archive.rows);
        return Ok(VoiceprintMergeStats {
            target_total,
            ..VoiceprintMergeStats::default()
        });
    };
    let mut target = target.unwrap_or(VoiceprintArchive {
        embeddings: Vec::new(),
        rows: 0,
        metadata: Vec::new(),
        envelope: VoiceprintEnvelope::default(),
        unrecognized_members: Vec::new(),
    });
    let selected_encoder = target
        .envelope
        .encoder
        .as_ref()
        .or(source.envelope.encoder.as_ref())
        .unwrap_or(fallback_encoder);
    let mut existing = target
        .metadata
        .iter()
        .map(|metadata| voiceprint_key(metadata))
        .collect::<Result<HashSet<_>, _>>()?;
    let mut stats = VoiceprintMergeStats::default();
    for (embedding, metadata) in source
        .embeddings
        .chunks_exact(EMBEDDING_WIDTH)
        .zip(&source.metadata)
    {
        let key = voiceprint_key(metadata)?;
        if !existing.insert(key.clone()) {
            stats.skipped_duplicate += 1;
            continue;
        }
        let norm = embedding
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt();
        if norm > 0.0 {
            target
                .embeddings
                .extend(embedding.iter().map(|value| value / norm));
            target.metadata.push(metadata.clone());
            stats.added += 1;
        }
    }
    target.rows = target.metadata.len();
    stats.target_total = target.rows;
    if stats.added > 0 {
        let bytes = write_voiceprints_npz(
            &target.embeddings,
            &target.metadata,
            &target.envelope,
            selected_encoder,
        )
        .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
        atomic_replace(&target_path, &bytes, AtomicWriteOptions::default())
            .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
    }
    Ok(stats)
}

fn ensure_voiceprint_merge_compatible(
    journal: &Path,
    source_id: &str,
    target_id: &str,
) -> Result<(Option<VoiceprintArchive>, Option<VoiceprintArchive>), EntityMergeError> {
    let (_source_dir, source_path) = resolve_voiceprint_path(journal, source_id, false)?;
    let (_target_dir, target_path) = resolve_voiceprint_path(journal, target_id, false)?;
    let source = load_voiceprints(&source_path)?;
    let target = load_voiceprints(&target_path)?;
    ensure_loaded_archives_merge_compatible(source_id, target_id, &source, &target)?;
    Ok((source, target))
}

fn ensure_loaded_archives_merge_compatible(
    source_id: &str,
    target_id: &str,
    source: &Option<VoiceprintArchive>,
    target: &Option<VoiceprintArchive>,
) -> Result<(), EntityMergeError> {
    if let Some(archive) = source {
        ensure_merge_archive_allowed(archive)?;
    }
    if let Some(archive) = target {
        ensure_merge_archive_allowed(archive)?;
    }
    if let (Some(source_encoder), Some(target_encoder)) = (
        source
            .as_ref()
            .and_then(|archive| archive.envelope.encoder.as_ref()),
        target
            .as_ref()
            .and_then(|archive| archive.envelope.encoder.as_ref()),
    ) && source_encoder != target_encoder
    {
        return Err(EntityMergeError::VoiceprintEncoderMismatch {
            source_entity_id: source_id.to_owned(),
            target_entity_id: target_id.to_owned(),
            source_encoder_id: source_encoder.id.clone(),
            target_encoder_id: target_encoder.id.clone(),
        });
    }
    Ok(())
}

fn ensure_merge_archive_allowed(archive: &VoiceprintArchive) -> Result<(), EntityMergeError> {
    if let Some(member) = archive.unrecognized_members.first() {
        return Err(EntityMergeError::Refused(format!(
            "voiceprint archive has unrecognized member {member}"
        )));
    }
    if archive.envelope.version > 1 {
        return Err(EntityMergeError::Refused(format!(
            "voiceprint envelope version {} exceeds supported version 1",
            archive.envelope.version
        )));
    }
    Ok(())
}

fn load_voiceprints(path: &Path) -> Result<Option<VoiceprintArchive>, EntityMergeError> {
    if !path_lexists(path).map_err(|error| EntityMergeError::Refused(error.to_string()))? {
        return Ok(None);
    }
    let bytes = read_bytes(path, Vec::new())
        .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
    read_voiceprints_npz(&bytes)
        .map(Some)
        .map_err(|error| EntityMergeError::Refused(error.to_string()))
}

fn voiceprint_key(metadata: &str) -> Result<VoiceprintKey, EntityMergeError> {
    let object: Value = serde_json::from_str(metadata).map_err(|error| {
        EntityMergeError::Refused(format!("invalid voiceprint metadata: {error}"))
    })?;
    Ok((
        object.get("day").cloned(),
        object.get("segment_key").cloned(),
        object.get("source").cloned(),
        object.get("sentence_id").cloned(),
    ))
}

struct MergePlan {
    target_after: Value,
    aliases_added: usize,
    emails_added: usize,
    source_display_name: String,
    target_display_name: String,
    principal_transferred: bool,
}

fn plan_merge(
    journal: &Path,
    source_id: &str,
    target_id: &str,
    options: EntityMergeOptions,
) -> Result<MergePlan, EntityMergeError> {
    if source_id == target_id {
        return Err(EntityMergeError::Refused(
            "Source and target must be different entities.".to_owned(),
        ));
    }
    let source_dir =
        resolve_entity_dir(journal, source_id).unwrap_or_else(|_| source_id.to_owned());
    let target_dir =
        resolve_entity_dir(journal, target_id).unwrap_or_else(|_| target_id.to_owned());
    let source = read_entity_identity(journal, &source_dir)?
        .ok_or_else(|| EntityMergeError::Refused(format!("Source entity not found: {source_id}")))?
        .value()
        .clone();
    let target = read_entity_identity(journal, &target_dir)?
        .ok_or_else(|| EntityMergeError::Refused(format!("Target entity not found: {target_id}")))?
        .value()
        .clone();
    // The router classifies refusals by their text ("blocked", "both are
    // marked as you", "isn't a person"); its route tests pin each one.
    if source.get("blocked").and_then(Value::as_bool) == Some(true) {
        return Err(EntityMergeError::Refused(
            "can't merge something that's blocked. unblock it first.".to_owned(),
        ));
    }
    if target.get("blocked").and_then(Value::as_bool) == Some(true) {
        return Err(EntityMergeError::Refused(
            "can't merge into something that's blocked. unblock it first.".to_owned(),
        ));
    }
    if super::lifecycle::identity_is_principal(&source)
        && super::lifecycle::identity_is_principal(&target)
    {
        return Err(EntityMergeError::Refused(
            "can't merge these: both are marked as you.".to_owned(),
        ));
    }
    // A person's voice evidence and speaker names, and the principal flag,
    // only ever sit on a person, so a person never merges into anything else.
    let principal_transferred = super::lifecycle::identity_is_principal(&source);
    let is_person =
        |identity: &Value| identity.get("type").and_then(Value::as_str) == Some("Person");
    if is_person(&source) && !is_person(&target) {
        return Err(EntityMergeError::Refused(if principal_transferred {
            "can't merge you into something that isn't a person.".to_owned()
        } else {
            "can't merge a person into something that isn't a person.".to_owned()
        }));
    }
    check_aka_cross_references(
        journal,
        source_id,
        source
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        target_id,
    )?;
    let mut after = target.clone();
    let object = after.as_object_mut().ok_or_else(|| {
        EntityMergeError::Refused(format!("Target entity not found: {target_id}"))
    })?;
    let aliases_before = values(&target, "aka");
    let mut alias_values = aliases_before.clone();
    let mut source_aliases = values(&source, "aka");
    alias_values.extend(source_aliases.iter().cloned());
    if options.keep_source_as_aka
        && let Some(name) = source.get("name").and_then(Value::as_str)
    {
        let name = name.to_owned();
        alias_values.push(name.clone());
        source_aliases.push(name);
    }
    let aliases = dedupe_akas(&alias_values);
    object.insert(
        "aka".to_owned(),
        Value::Array(aliases.iter().cloned().map(Value::String).collect()),
    );
    let emails_before = values(&target, "emails");
    let source_emails = values(&source, "emails");
    let emails = dedupe_emails(&emails_before, &source_emails);
    object.insert(
        "emails".to_owned(),
        Value::Array(emails.iter().cloned().map(Value::String).collect()),
    );
    for (field, value) in source.as_object().expect("identity object") {
        if ![
            "id",
            "name",
            "aka",
            "emails",
            "created_at",
            "updated_at",
            "merged_into",
            "blocked",
            "is_principal",
        ]
        .contains(&field.as_str())
            && !is_blank(Some(value))
            && is_blank(object.get(field))
        {
            object.insert(field.clone(), value.clone());
        }
    }
    if principal_transferred {
        object.insert("is_principal".to_owned(), Value::Bool(true));
    }
    let source_display_name = source
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(source_id)
        .to_owned();
    let target_display_name = after
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(target_id)
        .to_owned();
    Ok(MergePlan {
        target_after: after,
        aliases_added: aliases.len().saturating_sub(aliases_before.len()),
        emails_added: emails.len().saturating_sub(emails_before.len()),
        source_display_name,
        target_display_name,
        principal_transferred,
    })
}

pub(crate) fn dedupe_akas(values: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    for value in values {
        if seen.insert(value.to_lowercase()) {
            output.push(value.clone());
        }
    }
    output.sort_by_key(|value| value.to_lowercase());
    output
}
pub(crate) fn dedupe_emails(target_values: &[String], source_values: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    target_values
        .iter()
        .chain(source_values)
        .filter(|value| seen.insert(value.to_lowercase()))
        .cloned()
        .collect()
}

fn values(value: &Value, field: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}
fn is_blank(value: Option<&Value>) -> bool {
    value.is_none_or(|value| value.is_null() || value.as_str().is_some_and(str::is_empty))
}
fn check_aka_cross_references(
    journal: &Path,
    source_id: &str,
    source_name: &str,
    target_id: &str,
) -> Result<(), EntityMergeError> {
    let directory = contained_path(journal, "entities")
        .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
    let mut ids = Vec::new();
    for entry in list_dir_entries(&directory)
        .map_err(|error| EntityMergeError::Refused(error.to_string()))?
    {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let id = entry.name.to_string_lossy();
        if id == source_id || id == target_id {
            continue;
        }
        if let Some(identity) = read_entity_identity(journal, &id)?
            && values(identity.value(), "aka")
                .iter()
                .any(|aka| aka == source_id || aka == source_name)
        {
            ids.push(id.into_owned());
        }
    }
    if ids.is_empty() {
        Ok(())
    } else {
        Err(EntityMergeError::Refused(format!(
            "Cannot merge '{source_id}': referenced in aka lists of entity ids: {}",
            ids.join(", ")
        )))
    }
}
