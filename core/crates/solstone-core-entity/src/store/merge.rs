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
use solstone_core_journal_io::capture_snapshot;
use solstone_core_journal_io::contained_path;
use solstone_core_journal_io::day_dirs;
use solstone_core_journal_io::hold_lock;
use solstone_core_journal_io::iter_segments;
use solstone_core_journal_io::list_dir_entries;
use solstone_core_journal_io::path_lexists;
use solstone_core_journal_io::read_bytes;
use solstone_core_journal_io::read_json;
use solstone_core_journal_io::read_jsonl;
use solstone_core_journal_io::restore_snapshot;
use solstone_core_journal_io::write_json;
use solstone_core_journal_io::write_jsonl;

use crate::{
    EntityLifecycleError, EntityOperationContext, EntityOperationKind, EntityStoreError,
    EntityWriteError, hold_entity_trust_lock, read_entity_identity, save_entity_identity,
};

use super::lifecycle::resolve_entity_dir;

use super::merge_payload::{
    MergePayloadError, list_entity_merge_payload_ids, move_entity_merge_payload,
    record_entity_merge_payload, snapshot_payload,
};
use super::merge_rollback::MergeRollback;
use super::voiceprints::{
    EMBEDDING_WIDTH, EncoderIdentity, VoiceprintArchive, VoiceprintEnvelope, read_voiceprints_npz,
    resolve_voiceprint_path, write_voiceprints_npz,
};

const PHASES: [&str; 10] = [
    "private_payload",
    "voiceprints",
    "facets",
    "segments",
    "activities",
    "lineage",
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
    pub support: Vec<Value>,
}
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct FacetMergeStats {
    pub moved_count: usize,
    pub merged_count: usize,
    pub observations_appended: usize,
    pub touched_facets: Vec<String>,
    pub removed_source_dirs: Vec<String>,
    pub entries: Vec<Value>,
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
    Payload(MergePayloadError),
    Snapshot(SnapshotError),
    Index(solstone_core_indexer_store::StoreError),
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
            Self::Payload(error) => error.fmt(f),
            Self::Snapshot(error) => error.fmt(f),
            Self::Index(error) => error.fmt(f),
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
impl From<MergePayloadError> for EntityMergeError {
    fn from(error: MergePayloadError) -> Self {
        Self::Payload(error)
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
        format!("entities/{target_dir}/history/private/{merge_id}.json"),
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
    let mut payload = payload_for_merge(
        journal,
        &merge_id,
        source_id,
        target_id,
        &source_dir,
        &target_dir,
        &plan,
    )?;
    let mut touched_facets = Vec::new();
    let mut removed_source_dirs = Vec::new();
    let mut stats = MergeStats::default();
    for phase in PHASES {
        if phase == "history" {
            payload["result_counts"] = audit_counts(
                &stats,
                plan.aliases_added,
                plan.emails_added,
                plan.principal_transferred,
            );
        }
        let result = match phase {
            "private_payload" => {
                record_entity_merge_payload(journal, &target_dir, &merge_id, &payload)
                    .map(|_| ())
                    .map_err(Into::into)
            }
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
                payload["manifest"]["voiceprints"]["support"] = Value::Array(result.support);
            }),
            "facets" => merge_facets(journal, source_id, target_id, Some(&mut rollback), injector)
                .map(|result| {
                    stats.facets_moved = result.moved_count;
                    stats.facets_merged = result.merged_count;
                    stats.facets_observations_appended = result.observations_appended;
                    touched_facets = result.touched_facets;
                    removed_source_dirs = result.removed_source_dirs;
                    payload["manifest"]["facets"]["entries"] = Value::Array(result.entries);
                }),
            "history" => (|| {
                capture_undo_expected(journal, &target_dir, &mut payload)?;
                let saved = save_entity_identity(journal, target_id, &plan.target_after, Some(&EntityOperationContext {
                    kind: EntityOperationKind::Merge, caller: Value::Null, actor: Value::Null,
                    metadata: json!({"merge_id":merge_id,"source_id":source_id,"target_id":target_id}),
                })).map_err(EntityMergeError::Write)?;
                let sequence = saved
                    .event
                    .and_then(|event| event.get("seq").cloned())
                    .ok_or_else(|| {
                        EntityMergeError::Refused("merge history event was not written".to_owned())
                    })?;
                payload["commit_seq"] = sequence;
                record_entity_merge_payload(journal, &target_dir, &merge_id, &payload)?;
                Ok(())
            })(),
            "cleanup" => cleanup_merge(
                journal,
                &source_dir,
                &removed_source_dirs,
                Some(&mut rollback),
            ),
            "audit" => (|| {
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
                        payload["manifest"]["segments"]["entries"] = Value::Array(result.entries);
                    })
            }
            "activities" => {
                merge_activities(journal, source_id, target_id, Some(&mut rollback), injector).map(
                    |result| {
                        stats.activities_records_rewritten = result.records_rewritten;
                        stats.activities_fields_rewritten = result.fields_rewritten;
                        stats.activities_files_scanned = result.files_scanned;
                        stats.activities_files_rewritten = result.files_rewritten;
                        payload["manifest"]["activities"]["entries"] = Value::Array(result.entries);
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
                payload["manifest"]["observation_relations"]["entries"] =
                    Value::Array(result.entries);
            }),
            "lineage" => rebase_lineage(
                journal,
                source_id,
                target_id,
                &source_dir,
                &target_dir,
                &plan.target_after,
            )
            .and_then(|ids| {
                if !ids.is_empty() {
                    payload["manifest"]["rebased_merge_ids"] =
                        Value::Array(ids.into_iter().map(Value::String).collect());
                    record_entity_merge_payload(journal, &target_dir, &merge_id, &payload)?;
                }
                Ok(())
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
    let mut committed = json!({"operation":"merge", "report":report});
    rollback.commit_source(journal, "merge", &committed["report"])?;
    if injector.is_some_and(|injector| injector("edges", 0)) {
        return Err(EntityMergeError::Refused(
            "entity merge committed; index repair pending: injected failure".to_owned(),
        ));
    }
    super::merge_rollback::repair_index(journal, &mut committed).map_err(|error| {
        EntityMergeError::Refused(format!(
            "entity merge committed; index repair pending: {error}"
        ))
    })?;
    rollback.finish(journal).map_err(|error| {
        EntityMergeError::Refused(format!(
            "entity merge committed; recovery cleanup pending: {error}"
        ))
    })?;
    serde_json::from_value(committed["report"].clone())
        .map_err(|error| EntityMergeError::Refused(error.to_string()))
}

fn capture_undo_expected(
    journal: &Path,
    target_dir: &str,
    payload: &mut Value,
) -> Result<(), EntityMergeError> {
    // Undo may replace these two artifacts only while they still match
    // this committed merge, including relation remapping above.
    for entry in payload["manifest"]["facets"]["entries"]
        .as_array_mut()
        .into_iter()
        .flatten()
    {
        if entry["kind"] == "merge" {
            let directory = format!(
                "facets/{}/entities/{}",
                entry["facet"].as_str().unwrap(),
                entry["target_dir"].as_str().unwrap()
            );
            for name in ["entity.json", "observations.jsonl"] {
                entry["undo_expected"][name] = json!(super::merge_rollback::fingerprint(
                    &capture_snapshot(journal, &format!("{directory}/{name}"))?
                ));
            }
        }
    }
    let mut paths =
        std::collections::BTreeSet::from([format!("entities/{target_dir}/voiceprints.npz")]);
    for section in ["segments", "activities", "observation_relations"] {
        for entry in payload["manifest"][section]["entries"]
            .as_array()
            .into_iter()
            .flatten()
        {
            paths.insert(entry["path"].as_str().expect("merge entry path").to_owned());
        }
    }
    let mut expected = serde_json::Map::new();
    for path in paths {
        expected.insert(
            path.clone(),
            json!(super::merge_rollback::fingerprint(&capture_snapshot(
                journal, &path
            )?)),
        );
    }
    payload["manifest"]["undo_expected"] = Value::Object(expected);
    Ok(())
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
    pub entries: Vec<Value>,
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
        let entities = facet.path.join("entities");
        for entity in list_dir_entries(&entities)
            .map_err(|error| EntityMergeError::Refused(error.to_string()))?
        {
            if entity.kind != DirEntryKind::Directory {
                continue;
            }
            let path = entity.path.join("observations.jsonl");
            if !path_lexists(&path).map_err(|error| EntityMergeError::Refused(error.to_string()))? {
                continue;
            }
            let mut rows: Vec<Value> = read_jsonl(&path, Vec::new(), MalformedPolicy::Raise)
                .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
            let relative_path = super::merge_rollback::journal_relative(
                path.strip_prefix(journal)
                    .map_err(|error| EntityMergeError::Refused(error.to_string()))?,
            )?;
            let mut changed = false;
            for (row_index, row) in rows.iter_mut().enumerate() {
                if let Some(relation) = row.get_mut("relation").and_then(Value::as_object_mut)
                    && relation.get("target_entity_id").and_then(Value::as_str) == Some(source_id)
                {
                    relation.insert(
                        "target_entity_id".to_owned(),
                        Value::String(target_id.to_owned()),
                    );
                    stats.rows_rewritten += 1;
                    stats.entries.push(json!({
                        "path": relative_path.clone(),
                        "row_index": row_index,
                        "target_before": source_id,
                    }));
                    changed = true;
                }
            }
            if changed {
                capture_rollback_file(&mut rollback, journal, &path)?;
                write_jsonl(&path, rows, AtomicWriteOptions::default())
                    .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
                inject_failure(injector, "observation relation remap", artifact_index)?;
                artifact_index += 1;
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
    pub entries: Vec<Value>,
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
            let relative_path = super::merge_rollback::journal_relative(
                file.path
                    .strip_prefix(journal)
                    .map_err(|error| EntityMergeError::Refused(error.to_string()))?,
            )?;
            let mut file_changed = false;
            for (row_index, row) in rows.iter_mut().enumerate() {
                let mut changed = false;
                if let Some(object) = row.as_object_mut() {
                    if let Some(active) = object
                        .get_mut("active_entities")
                        .and_then(Value::as_array_mut)
                    {
                        for (item_index, value) in active.iter_mut().enumerate() {
                            if value.as_str() == Some(source_id) {
                                *value = Value::String(target_id.to_owned());
                                stats.fields_rewritten += 1;
                                stats.entries.push(json!({
                                    "path": relative_path.clone(),
                                    "row_index": row_index,
                                    "container": "active_entities",
                                    "item_index": item_index,
                                    "before": source_id,
                                }));
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
                            for (item_index, item) in items.iter_mut().enumerate() {
                                if let Some(item) = item.as_object_mut() {
                                    for key in keys {
                                        if item.get(*key).and_then(Value::as_str) == Some(source_id)
                                        {
                                            item.insert(
                                                (*key).to_owned(),
                                                Value::String(target_id.to_owned()),
                                            );
                                            stats.fields_rewritten += 1;
                                            stats.entries.push(json!({
                                                "path": relative_path.clone(),
                                                "row_index": row_index,
                                                "container": container,
                                                "item_index": item_index,
                                                "field": key,
                                                "before": source_id,
                                            }));
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
    pub entries: Vec<Value>,
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
                if !raw
                    .windows(source_id.len())
                    .any(|bytes| bytes == source_id.as_bytes())
                {
                    continue;
                }
                if let Some(rollback) = rollback.as_deref_mut() {
                    rollback.lock_file(&path)?;
                }
                let mut value: Value = read_json(&path, Value::Null, MalformedPolicy::Raise)
                    .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
                let relative_path = super::merge_rollback::journal_relative(
                    path.strip_prefix(journal)
                        .map_err(|error| EntityMergeError::Refused(error.to_string()))?,
                )?;
                let mut changed = false;
                if let Some(labels) = value.get_mut("labels").and_then(Value::as_array_mut) {
                    for (row_index, label) in labels.iter_mut().enumerate() {
                        if let Some(object) = label.as_object_mut()
                            && object.get("speaker").and_then(Value::as_str) == Some(source_id)
                        {
                            object
                                .insert("speaker".to_owned(), Value::String(target_id.to_owned()));
                            stats.entries.push(json!({
                                "path": relative_path.clone(),
                                "section": "labels",
                                "row_index": row_index,
                                "field": "speaker",
                                "before": source_id,
                            }));
                            changed = true;
                        }
                    }
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
            if !raw
                .windows(source_id.len())
                .any(|bytes| bytes == source_id.as_bytes())
            {
                continue;
            }
            if let Some(rollback) = rollback.as_deref_mut() {
                rollback.lock_file(&path)?;
            }
            let mut value: Value = read_json(&path, Value::Null, MalformedPolicy::Raise)
                .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
            let relative_path = super::merge_rollback::journal_relative(
                path.strip_prefix(journal)
                    .map_err(|error| EntityMergeError::Refused(error.to_string()))?,
            )?;
            let mut changed = false;
            if let Some(corrections) = value.get_mut("corrections").and_then(Value::as_array_mut) {
                for (row_index, correction) in corrections.iter_mut().enumerate() {
                    if let Some(object) = correction.as_object_mut() {
                        for field in ["original_speaker", "corrected_speaker"] {
                            if object.get(field).and_then(Value::as_str) == Some(source_id) {
                                object
                                    .insert(field.to_owned(), Value::String(target_id.to_owned()));
                                stats.entries.push(json!({
                                    "path": relative_path.clone(),
                                    "section": "corrections",
                                    "row_index": row_index,
                                    "field": field,
                                    "before": source_id,
                                }));
                                changed = true;
                            }
                        }
                    }
                }
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

fn relationship_dir_for_entity_id(
    journal: &Path,
    facet: &str,
    entity_id: &str,
) -> Result<Option<String>, EntityMergeError> {
    let entities = match contained_path(journal, &format!("facets/{facet}/entities")) {
        Ok(path) => path,
        Err(_) => return Ok(None),
    };
    if !path_lexists(&entities).map_err(|error| EntityMergeError::Refused(error.to_string()))? {
        return Ok(None);
    }
    for entry in
        list_dir_entries(&entities).map_err(|error| EntityMergeError::Refused(error.to_string()))?
    {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let relationship_dir = entry.name.to_string_lossy().into_owned();
        let link_path = contained_path(
            journal,
            &format!("facets/{facet}/entities/{relationship_dir}/entity.json"),
        )
        .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
        if !path_lexists(&link_path)
            .map_err(|error| EntityMergeError::Refused(error.to_string()))?
        {
            continue;
        }
        let link: Value = read_json(&link_path, Value::Null, MalformedPolicy::Raise)
            .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
        let link_id = link.get("entity_id").and_then(Value::as_str);
        if link_id == Some(entity_id) {
            return Ok(Some(relationship_dir));
        }
        if let Ok(Some(identity)) = read_entity_identity(journal, entity_id)
            && link_id == Some(identity.entity_id())
        {
            return Ok(Some(relationship_dir));
        }
    }
    Ok(None)
}

pub(crate) fn merge_facets(
    journal: &Path,
    source_id: &str,
    target_id: &str,
    mut rollback: Option<&mut MergeRollback>,
    injector: Option<&FailureInjector>,
) -> Result<FacetMergeStats, EntityMergeError> {
    let facets = contained_path(journal, "facets")
        .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
    let mut stats = FacetMergeStats::default();
    let mut artifact_index = 0;
    for entry in
        list_dir_entries(&facets).map_err(|error| EntityMergeError::Refused(error.to_string()))?
    {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let facet = entry.name.to_string_lossy().into_owned();
        let Some(source_dir) = relationship_dir_for_entity_id(journal, &facet, source_id)? else {
            continue;
        };
        let target_dir = relationship_dir_for_entity_id(journal, &facet, target_id)?;
        let source_rel = contained_path(
            journal,
            &format!("facets/{facet}/entities/{source_dir}/entity.json"),
        )
        .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
        let source: Value = read_json(&source_rel, Value::Null, MalformedPolicy::Raise)
            .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
        let source_obs_path = source_rel.parent().unwrap().join("observations.jsonl");
        let source_obs: Vec<Value> =
            read_jsonl(&source_obs_path, Vec::new(), MalformedPolicy::Raise)
                .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
        if target_dir.as_deref() == Some(source_dir.as_str()) || target_dir.is_none() {
            if let Some(rollback) = rollback.as_deref_mut() {
                rollback.capture(journal, &format!("facets/{facet}/entities/{source_dir}"))?;
            }
            let original_link_id = source
                .get("entity_id")
                .and_then(Value::as_str)
                .unwrap_or(source_id)
                .to_owned();
            let mut relinked = source;
            relinked
                .as_object_mut()
                .ok_or_else(|| {
                    EntityMergeError::Refused("facet relationship is not an object".to_owned())
                })?
                .insert("entity_id".to_owned(), Value::String(target_id.to_owned()));
            write_json(
                &source_rel,
                &relinked,
                JsonWriteOptions {
                    indent: Some(2),
                    sort_keys: false,
                    mode: None,
                },
            )
            .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
            inject_failure(injector, "facets", artifact_index)?;
            artifact_index += 1;
            stats.moved_count += 1;
            stats.touched_facets.push(facet.clone());
            stats.entries.push(json!({
                "facet": facet,
                "kind": "relink",
                "source_dir": source_dir,
                "target_dir": source_dir,
                "source_entity_id": original_link_id,
            }));
            continue;
        }
        let target_dir = target_dir.expect("both-present branch");
        let target_directory = format!("facets/{facet}/entities/{target_dir}");
        let target_rel = contained_path(journal, &format!("{target_directory}/entity.json"))
            .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
        if let Some(rollback) = rollback.as_deref_mut() {
            rollback.capture(journal, &target_directory)?;
        }
        let mut target: Value = read_json(&target_rel, Value::Null, MalformedPolicy::Raise)
            .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
        let target_before = target.clone();
        let target_obs_path = target_rel.parent().unwrap().join("observations.jsonl");
        let target_observations_existed = path_lexists(&target_obs_path)
            .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
        let target_obs: Vec<Value> =
            read_jsonl(&target_obs_path, Vec::new(), MalformedPolicy::Raise)
                .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
        merge_facet_scalars(&source, &mut target);
        write_json(
            &target_rel,
            &target,
            JsonWriteOptions {
                indent: Some(2),
                sort_keys: false,
                mode: None,
            },
        )
        .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
        inject_failure(injector, "facets", artifact_index)?;
        artifact_index += 1;
        let merged_observations = dedupe_observations(&source_obs, &target_obs);
        stats.observations_appended += merged_observations.len().saturating_sub(target_obs.len());
        write_jsonl(
            target_obs_path,
            merged_observations,
            AtomicWriteOptions::default(),
        )
        .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
        inject_failure(injector, "facets", artifact_index)?;
        artifact_index += 1;
        stats.merged_count += 1;
        stats.touched_facets.push(facet.clone());
        stats
            .removed_source_dirs
            .push(format!("facets/{facet}/entities/{source_dir}"));
        stats.entries.push(json!({
            "facet": facet,
            "kind": "merge",
            "source_dir": source_dir,
            "target_dir": target_dir,
            "source_entity_id": source_id,
            "target_before": target_before,
            "target_observations_before": target_obs,
            "target_observations_existed": target_observations_existed,
        }));
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

fn rebase_lineage(
    journal: &Path,
    source_id: &str,
    target_id: &str,
    source_dir: &str,
    target_dir: &str,
    target: &Value,
) -> Result<Vec<String>, EntityMergeError> {
    let mut rebased = Vec::new();
    for merge_id in list_entity_merge_payload_ids(journal, source_dir)? {
        let (payload, private_payload) = move_entity_merge_payload(
            journal,
            source_dir,
            target_dir,
            target_id,
            &merge_id,
            Some(source_id),
        )?;
        let descendant_source = payload
            .get("source_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        save_entity_identity(
            journal,
            target_id,
            target,
            Some(&EntityOperationContext {
                kind: EntityOperationKind::Merge,
                caller: Value::Null,
                actor: Value::Null,
                metadata: json!({"merge_id":merge_id,"source_id":descendant_source,"target_id":target_id,"rebased_from_entity_id":source_id,"private_payload":private_payload}),
            }),
        )?;
        rebased.push(merge_id);
    }
    Ok(rebased)
}
fn merge_facet_scalars(source: &Value, target: &mut Value) {
    let source = source.as_object().expect("facet relationship object");
    let target = target.as_object_mut().expect("facet relationship object");
    for (field, earlier) in [
        ("attached_at", true),
        ("updated_at", false),
        ("last_seen", false),
    ] {
        if let Some(value) = source.get(field).filter(|value| !is_blank(Some(value))) {
            let replace = is_blank(target.get(field))
                || target.get(field).is_some_and(|existing| {
                    if earlier {
                        value.as_str() < existing.as_str()
                    } else {
                        value.as_str() > existing.as_str()
                    }
                });
            if replace {
                target.insert(field.to_owned(), value.clone());
            }
        }
    }
    if is_blank(target.get("description")) && !is_blank(source.get("description")) {
        target.insert("description".to_owned(), source["description"].clone());
    }
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
    let target_existing = existing.clone();
    let mut stats = VoiceprintMergeStats::default();
    for (embedding, metadata) in source
        .embeddings
        .chunks_exact(EMBEDDING_WIDTH)
        .zip(&source.metadata)
    {
        let key = voiceprint_key(metadata)?;
        let target_preexisting = target_existing.contains(&key);
        if !existing.insert(key.clone()) {
            stats.skipped_duplicate += 1;
            stats
                .support
                .push(voiceprint_support_entry(&key, target_preexisting, false));
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
            stats
                .support
                .push(voiceprint_support_entry(&key, target_preexisting, true));
        } else {
            stats
                .support
                .push(voiceprint_support_entry(&key, target_preexisting, false));
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

fn voiceprint_support_entry(key: &VoiceprintKey, target_preexisting: bool, added: bool) -> Value {
    json!({
        "key": {
            "day": &key.0,
            "segment_key": &key.1,
            "source": &key.2,
            "sentence_id": &key.3,
        },
        "target_preexisting": target_preexisting,
        "added": added,
    })
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
    source_before: Value,
    target_before: Value,
    target_after: Value,
    aliases_added: usize,
    emails_added: usize,
    aka_support: Vec<Value>,
    email_support: Vec<Value>,
    scalar_support: Vec<Value>,
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
    if source.get("blocked").and_then(Value::as_bool) == Some(true) {
        return Err(EntityMergeError::Refused(format!(
            "Cannot merge blocked entity: {source_id}"
        )));
    }
    if target.get("blocked").and_then(Value::as_bool) == Some(true) {
        return Err(EntityMergeError::Refused(format!(
            "Cannot merge blocked entity: {target_id}"
        )));
    }
    if source.get("is_principal").and_then(Value::as_bool) == Some(true)
        && target.get("is_principal").and_then(Value::as_bool) == Some(true)
    {
        return Err(EntityMergeError::Refused(
            "Cannot merge two principal entities.".to_owned(),
        ));
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
    let mut scalar_support = Vec::new();
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
        {
            let target_prevalue = target.get(field).cloned().unwrap_or(Value::Null);
            scalar_support.push(json!({
                "key": field,
                "target_prevalue": target_prevalue,
                "target_prevalue_missing": target.get(field).is_none(),
                "source_value": value,
            }));
            if is_blank(object.get(field)) {
                object.insert(field.clone(), value.clone());
            }
        }
    }
    if source.get("is_principal").and_then(Value::as_bool) == Some(true) {
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
    let principal_transferred = source.get("is_principal").and_then(Value::as_bool) == Some(true);
    Ok(MergePlan {
        source_before: source,
        target_before: target,
        target_after: after,
        aliases_added: aliases.len().saturating_sub(aliases_before.len()),
        emails_added: emails.len().saturating_sub(emails_before.len()),
        aka_support: support_for_values(&source_aliases, &aliases_before),
        email_support: support_for_values(&source_emails, &emails_before),
        scalar_support,
        source_display_name,
        target_display_name,
        principal_transferred,
    })
}

fn support_for_values(source_values: &[String], target_values: &[String]) -> Vec<Value> {
    let target_keys = target_values
        .iter()
        .map(|value| value.to_lowercase())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    source_values
        .iter()
        .filter_map(|value| {
            let key = value.to_lowercase();
            seen.insert(key.clone())
                .then(|| json!({"key": key, "target_preexisting": target_keys.contains(&key)}))
        })
        .collect()
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
pub(crate) fn dedupe_observations(source: &[Value], target: &[Value]) -> Vec<Value> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for value in target.iter().chain(source) {
        let key = (
            value
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            value.get("observed_at").cloned().unwrap_or(Value::Null),
        );
        if seen.insert(key) {
            result.push(value.clone());
        }
    }
    result
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
fn payload_for_merge(
    journal: &Path,
    merge_id: &str,
    source_id: &str,
    target_id: &str,
    source_dir: &str,
    target_dir: &str,
    plan: &MergePlan,
) -> Result<Value, EntityMergeError> {
    let mut snapshots = vec![source_snapshot_payload(
        journal,
        &format!("entities/{source_dir}"),
    )?];
    let target_voiceprints = snapshot_payload(&capture_snapshot(
        journal,
        &format!("entities/{target_dir}/voiceprints.npz"),
    )?);
    let facets = contained_path(journal, "facets")
        .map_err(|error| EntityMergeError::Refused(error.to_string()))?;
    for entry in
        list_dir_entries(&facets).map_err(|error| EntityMergeError::Refused(error.to_string()))?
    {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let facet = entry.name.to_string_lossy();
        if let Some(source_dir) = relationship_dir_for_entity_id(journal, &facet, source_id)? {
            let relative = format!("facets/{facet}/entities/{source_dir}");
            snapshots.push(source_snapshot_payload(journal, &relative)?);
        }
    }
    Ok(
        json!({"schema_version":1,"merge_id":merge_id,"source_id":source_id,"target_id":target_id,"commit_seq":null,"source_state":{"identity":plan.source_before,"snapshots":snapshots},"result_counts":{},"manifest":{"identity":{"target_before":plan.target_before,"aka_support":plan.aka_support,"email_support":plan.email_support,"scalar_support":plan.scalar_support},"voiceprints":{"support":[],"target_before":target_voiceprints},"facets":{"entries":[]},"segments":{"entries":[]},"activities":{"entries":[]},"observation_relations":{"entries":[]},"rebased_merge_ids":[]}}),
    )
}

fn source_snapshot_payload(journal: &Path, relative: &str) -> Result<Value, EntityMergeError> {
    let snapshot = capture_snapshot(journal, relative)?;
    Ok(json!({"rel":relative,"files":[],"snapshot":snapshot_payload(&snapshot)}))
}
