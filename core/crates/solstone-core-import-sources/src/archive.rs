// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Safe, journal-root-explicit merge of a portable journal archive.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_entity::{
    EntityResolutionOutcome, archive_dedupe_akas, archive_dedupe_emails,
    archive_dedupe_observations, load_all_journal_entities, read_journal_principal,
    record_entity_resolution_from_name_evidence, save_entity_identity,
};
use solstone_core_facets::{
    load_observations, read_activity_file, read_facet_entity_link, read_log_file, read_news_file,
    save_facet_entity_link, save_observations, write_activity_file, write_log_file,
    write_news_file,
};
use solstone_core_journal_io::{
    DEFAULT_STREAM, LockError, LockOptions, PathOrDay, StagedDirOptions, append_jsonl,
    contained_path, hold_lock, iter_segments, publish_staged_dir,
};
use zip::ZipArchive;

use crate::{ArchiveSafetyPhase, ImportSourcesError};

const GIB: u64 = 1024 * 1024 * 1024;
const DEFAULT_ARCHIVE_CAP: u64 = 50 * GIB;
const DEFAULT_EXPANDED_CAP: u64 = 100 * GIB;
const DEFAULT_SPACE_RESERVE: u64 = GIB;

/// A request sink for the one full reindex requested after a completed merge.
pub trait FullReindexRequester {
    /// Returns whether the request was accepted. An error is retained as rejection detail.
    fn request_full_reindex(&self) -> Result<bool, String>;
}

/// Options for one archive merge transaction.
#[derive(Debug, Clone)]
pub struct ArchiveMergeOptions {
    /// Directory that receives transient extraction, staging, and decision-log artifacts.
    pub working_root: PathBuf,
    /// Upper bound for the compressed archive file.
    pub max_archive_bytes: u64,
    /// Upper bound for all extracted file payloads.
    pub max_uncompressed_bytes: u64,
    /// Bytes that must remain free after the planned extraction.
    pub free_space_reserve_bytes: u64,
    /// Bounded advisory lock acquisition options.
    pub lock_options: LockOptions,
}

impl Default for ArchiveMergeOptions {
    fn default() -> Self {
        Self {
            working_root: std::env::temp_dir().join("solstone-archive-merge"),
            max_archive_bytes: DEFAULT_ARCHIVE_CAP,
            max_uncompressed_bytes: DEFAULT_EXPANDED_CAP,
            free_space_reserve_bytes: DEFAULT_SPACE_RESERVE,
            lock_options: LockOptions::default(),
        }
    }
}

/// Aggregate copy/merge results, matching the Python merge-summary counters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeSummary {
    pub segments_copied: usize,
    pub segments_skipped: usize,
    pub segments_errored: usize,
    pub entities_created: usize,
    pub entities_merged: usize,
    pub entities_skipped: usize,
    pub entities_staged: usize,
    pub facets_created: usize,
    pub facets_merged: usize,
    pub imports_copied: usize,
    pub imports_skipped: usize,
    pub errors: Vec<String>,
}

/// What happened to a source segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentDisposition {
    pub day: String,
    pub stream: String,
    pub key: String,
    pub disposition: SegmentDispositionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SegmentDispositionKind {
    Copied,
    IdenticalExisting,
    DifferingContentCollision,
}

/// Whether an archive entity's principal claim was retained, cleared, or separately reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrincipalAdoption {
    NotClaimed,
    PreservedExistingTargetPrincipal,
    RefusedOnNameMatch,
    ClearedOnCreate,
    ConflictReportedSeparately,
}

/// Result for one source entity. Staged records are readable below `staging_path`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityDisposition {
    pub source_id: String,
    pub target_id: Option<String>,
    pub disposition: EntityDispositionKind,
    pub fields_changed: Vec<String>,
    pub principal_adoption: PrincipalAdoption,
    pub staging_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntityDispositionKind {
    Created,
    Merged,
    Skipped,
    StagedAmbiguous,
    StagedIdCollision,
}

/// Informational collision only; it never changes merge control flow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrincipalCollision {
    pub target_entity_id: String,
    pub source_entity_id: String,
}

/// Signal consumed by the future dispatcher dedupe/retry layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetryDisposition {
    IdempotentNoop,
    Applied,
    Incomplete,
}

/// Outcome of the explicitly injected reindex request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReindexStatus {
    Accepted,
    NotAccepted { detail: String },
    NotRequested,
}

/// Complete library result; no CLI or process-global state is involved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArchiveMergeResult {
    pub entries_written: usize,
    pub entities_seeded: usize,
    pub owner_entity_after: Option<Value>,
    pub principal_collision: Option<PrincipalCollision>,
    pub errors: Vec<String>,
    pub merge_summary: MergeSummary,
    pub entity_dispositions: Vec<EntityDisposition>,
    pub segment_dispositions: Vec<SegmentDisposition>,
    pub retry_disposition: RetryDisposition,
    pub reindex_status: ReindexStatus,
    pub decision_log_path: PathBuf,
    pub staging_path: PathBuf,
}

/// Validate, extract, and merge an archive while holding the target merge lock.
pub fn merge_journal_archive(
    archive_path: &Path,
    target_journal_root: &Path,
    options: &ArchiveMergeOptions,
    reindexer: Option<&dyn FullReindexRequester>,
) -> Result<ArchiveMergeResult, ImportSourcesError> {
    let validated = validate_archive(archive_path, options)?;
    let protected = target_journal_root.join("health/locks/archive-merge");
    let owner_path = protected
        .parent()
        .expect("lock has parent")
        .join("archive-merge.owner.json");
    let sidecar_path = protected.with_file_name("archive-merge.lock");
    let _lock = hold_lock(&protected, options.lock_options).map_err(|error| match error {
        LockError::Timeout(_) => ImportSourcesError::LockBusy {
            protected_path: protected.clone(),
            sidecar_path,
            owner_metadata_path: owner_path.clone(),
            owner: read_owner_metadata(&owner_path),
            remedy: format!(
                "wait for the holder or inspect/remove only verified-stale owner metadata at {}",
                owner_path.display()
            ),
        },
        LockError::Io { .. } => ImportSourcesError::LockFailed {
            path: protected.clone(),
            detail: error.to_string(),
        },
    })?;
    write_owner_metadata(&owner_path)?;

    let token = run_token();
    let extraction_dir = options.working_root.join(format!("extract-{token}"));
    let artifact_dir = options.working_root.join("runs").join(token);
    let extraction = match extract_archive(archive_path, &validated, &extraction_dir, options) {
        Ok(extraction) => extraction,
        Err(error) => {
            let _ = fs::remove_file(&owner_path);
            return Err(error);
        }
    };
    let outcome = merge_extracted(&extraction, target_journal_root, &artifact_dir, reindexer);
    let extraction_cleanup = fs::remove_dir_all(&extraction_dir).and_then(|()| {
        if extraction_dir.exists() {
            Err(io::Error::other("directory remains"))
        } else {
            Ok(())
        }
    });
    let _ = fs::remove_file(&owner_path);
    match (outcome, extraction_cleanup) {
        (Ok(result), Ok(())) => Ok(result),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(ImportSourcesError::ExtractionCleanupFailed {
            extraction_dir,
            detail: error.to_string(),
        }),
    }
}

#[derive(Debug)]
struct ValidatedArchive {
    root: PathBuf,
    expanded_size: u64,
}

fn validate_archive(
    path: &Path,
    options: &ArchiveMergeOptions,
) -> Result<ValidatedArchive, ImportSourcesError> {
    let metadata = fs::metadata(path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => ImportSourcesError::ArchiveNotFound {
            path: path.to_path_buf(),
        },
        _ => ImportSourcesError::ArchiveInvalid {
            path: path.to_path_buf(),
            detail: error.to_string(),
        },
    })?;
    if metadata.len() > options.max_archive_bytes {
        return Err(ImportSourcesError::ArchiveTooLarge {
            path: path.to_path_buf(),
            bytes: metadata.len(),
            maximum: options.max_archive_bytes,
        });
    }
    let file = File::open(path).map_err(|error| ImportSourcesError::ArchiveInvalid {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| ImportSourcesError::ArchiveInvalid {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })?;
    let mut root_candidates = BTreeSet::new();
    let mut expanded_size = 0_u64;
    for index in 0..archive.len() {
        let entry =
            archive
                .by_index(index)
                .map_err(|error| ImportSourcesError::ArchiveInvalid {
                    path: path.to_path_buf(),
                    detail: error.to_string(),
                })?;
        validate_entry(
            entry.name(),
            entry.unix_mode(),
            entry.encrypted(),
            ArchiveSafetyPhase::Validation,
        )?;
        let normalized = Path::new(entry.name());
        if let Some(Component::Normal(first)) = normalized.components().next()
            && first != "__MACOSX"
            && first != ".DS_Store"
        {
            root_candidates.insert(first.to_os_string());
        }
        if !entry.is_dir() {
            expanded_size = expanded_size.checked_add(entry.size()).ok_or(
                ImportSourcesError::ArchiveUncompressedTooLarge {
                    bytes: u64::MAX,
                    maximum: options.max_uncompressed_bytes,
                },
            )?;
        }
    }
    if expanded_size > options.max_uncompressed_bytes {
        return Err(ImportSourcesError::ArchiveUncompressedTooLarge {
            bytes: expanded_size,
            maximum: options.max_uncompressed_bytes,
        });
    }
    let root = archive_root(&root_candidates)?;
    Ok(ValidatedArchive {
        root,
        expanded_size,
    })
}

fn validate_entry(
    name: &str,
    unix_mode: Option<u32>,
    encrypted: bool,
    phase: ArchiveSafetyPhase,
) -> Result<(), ImportSourcesError> {
    if encrypted {
        return Err(ImportSourcesError::ArchiveEntryEncrypted {
            phase,
            entry: name.to_owned(),
        });
    }
    let path = Path::new(name);
    if name.starts_with('/')
        || name.starts_with('\\')
        || name.contains("\\")
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(ImportSourcesError::ArchiveUnsafeEntry {
            phase,
            entry: name.to_owned(),
            reason: "absolute or traversal path".to_owned(),
        });
    }
    if unix_mode.is_some_and(|mode| mode & 0o170000 == 0o120000) {
        return Err(ImportSourcesError::ArchiveUnsafeEntry {
            phase,
            entry: name.to_owned(),
            reason: "symbolic link".to_owned(),
        });
    }
    Ok(())
}

fn archive_root(candidates: &BTreeSet<std::ffi::OsString>) -> Result<PathBuf, ImportSourcesError> {
    let journal_names = ["chronicle", "entities", "facets", "imports", "_export.json"];
    if candidates
        .iter()
        .any(|name| journal_names.iter().any(|known| name == known))
    {
        return Ok(PathBuf::new());
    }
    if candidates.len() == 1 {
        return Ok(PathBuf::from(
            candidates.iter().next().expect("one candidate"),
        ));
    }
    Err(ImportSourcesError::ArchiveInvalid {
        path: PathBuf::new(),
        detail: "archive has no unambiguous journal root".to_owned(),
    })
}

fn extract_archive(
    path: &Path,
    validated: &ValidatedArchive,
    run_dir: &Path,
    options: &ArchiveMergeOptions,
) -> Result<PathBuf, ImportSourcesError> {
    fs::create_dir_all(run_dir).map_err(|error| ImportSourcesError::ExtractionFailed {
        archive: path.to_path_buf(),
        extraction_dir: run_dir.to_path_buf(),
        detail: error.to_string(),
    })?;
    let required = validated
        .expanded_size
        .saturating_add(options.free_space_reserve_bytes);
    let stat = nix::sys::statvfs::statvfs(run_dir).map_err(|error| {
        ImportSourcesError::ExtractionFailed {
            archive: path.to_path_buf(),
            extraction_dir: run_dir.to_path_buf(),
            detail: error.to_string(),
        }
    })?;
    let available = stat.blocks_available().saturating_mul(stat.fragment_size());
    if available < required {
        return cleanup_extraction(
            path,
            run_dir,
            ImportSourcesError::ArchiveInsufficientSpace {
                available,
                required,
                path: run_dir.to_path_buf(),
            },
        );
    }
    let file = File::open(path).map_err(|error| ImportSourcesError::ExtractionFailed {
        archive: path.to_path_buf(),
        extraction_dir: run_dir.to_path_buf(),
        detail: error.to_string(),
    })?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| ImportSourcesError::ExtractionFailed {
            archive: path.to_path_buf(),
            extraction_dir: run_dir.to_path_buf(),
            detail: error.to_string(),
        })?;
    let mut copied = 0_u64;
    for index in 0..archive.len() {
        let mut entry =
            archive
                .by_index(index)
                .map_err(|error| ImportSourcesError::ExtractionFailed {
                    archive: path.to_path_buf(),
                    extraction_dir: run_dir.to_path_buf(),
                    detail: error.to_string(),
                })?;
        // With stable archive bytes this duplicates validation; it detects entries that change
        // between validation's read and this extraction read.
        if let Err(error) = validate_entry(
            entry.name(),
            entry.unix_mode(),
            entry.encrypted(),
            ArchiveSafetyPhase::Extraction,
        ) {
            return cleanup_extraction(path, run_dir, error);
        }
        let output = run_dir.join(entry.name());
        if entry.is_dir() {
            if let Err(error) = fs::create_dir_all(&output) {
                return cleanup_extraction(
                    path,
                    run_dir,
                    ImportSourcesError::ExtractionFailed {
                        archive: path.to_path_buf(),
                        extraction_dir: run_dir.to_path_buf(),
                        detail: error.to_string(),
                    },
                );
            };
            continue;
        }
        if let Some(parent) = output.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            return cleanup_extraction(
                path,
                run_dir,
                ImportSourcesError::ExtractionFailed {
                    archive: path.to_path_buf(),
                    extraction_dir: run_dir.to_path_buf(),
                    detail: error.to_string(),
                },
            );
        }
        let mut output_file = match File::create(&output) {
            Ok(file) => file,
            Err(error) => {
                return cleanup_extraction(
                    path,
                    run_dir,
                    ImportSourcesError::ExtractionFailed {
                        archive: path.to_path_buf(),
                        extraction_dir: run_dir.to_path_buf(),
                        detail: error.to_string(),
                    },
                );
            }
        };
        let mut buffer = [0_u8; 32 * 1024];
        loop {
            let read = match entry.read(&mut buffer) {
                Ok(read) => read,
                Err(error) => {
                    return cleanup_extraction(
                        path,
                        run_dir,
                        ImportSourcesError::ExtractionFailed {
                            archive: path.to_path_buf(),
                            extraction_dir: run_dir.to_path_buf(),
                            detail: error.to_string(),
                        },
                    );
                }
            };
            if read == 0 {
                break;
            }
            copied = copied.saturating_add(read as u64);
            if copied > options.max_uncompressed_bytes {
                return cleanup_extraction(
                    path,
                    run_dir,
                    ImportSourcesError::ArchiveUncompressedTooLarge {
                        bytes: copied,
                        maximum: options.max_uncompressed_bytes,
                    },
                );
            }
            if let Err(error) = std::io::Write::write_all(&mut output_file, &buffer[..read]) {
                return cleanup_extraction(
                    path,
                    run_dir,
                    ImportSourcesError::ExtractionFailed {
                        archive: path.to_path_buf(),
                        extraction_dir: run_dir.to_path_buf(),
                        detail: error.to_string(),
                    },
                );
            }
        }
    }
    Ok(run_dir.join(&validated.root))
}

fn cleanup_extraction(
    path: &Path,
    run_dir: &Path,
    original: ImportSourcesError,
) -> Result<PathBuf, ImportSourcesError> {
    match fs::remove_dir_all(run_dir).and_then(|()| {
        if run_dir.exists() {
            Err(io::Error::other("directory remains"))
        } else {
            Ok(())
        }
    }) {
        Ok(()) => Err(original),
        Err(error) => Err(ImportSourcesError::ExtractionCleanupFailed {
            extraction_dir: run_dir.to_path_buf(),
            detail: format!("while handling {}: {error}", path.display()),
        }),
    }
}

fn write_owner_metadata(path: &Path) -> Result<(), ImportSourcesError> {
    let value = json!({"run_token": run_token(), "started_at": Utc::now().to_rfc3339()});
    fs::write(
        path,
        serde_json::to_vec(&value).expect("JSON value serializes"),
    )
    .map_err(|error| ImportSourcesError::LockFailed {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })
}

fn read_owner_metadata(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str::<Value>(&text)
        .ok()
        .map(|value| value.to_string())
}

fn run_token() -> String {
    let mut bytes = [0_u8; 12];
    if getrandom::fill(&mut bytes).is_ok() {
        return bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string()
}

fn merge_extracted(
    source: &Path,
    target: &Path,
    run_dir: &Path,
    reindexer: Option<&dyn FullReindexRequester>,
) -> Result<ArchiveMergeResult, ImportSourcesError> {
    let decision_log_path = run_dir.join("decision-log.jsonl");
    let staging_path = run_dir.join("staged-entities");
    let mut state = MergeState::new(decision_log_path.clone(), staging_path.clone());
    merge_segments(source, target, &mut state)?;
    merge_entities(source, target, &mut state)?;
    merge_facets(source, target, &mut state)?;
    merge_imports(source, target, &mut state)?;
    let reindex_status = match reindexer {
        None => ReindexStatus::NotRequested,
        Some(reindexer) => match reindexer.request_full_reindex() {
            Ok(true) => ReindexStatus::Accepted,
            Ok(false) => ReindexStatus::NotAccepted {
                detail: "request was not accepted".to_owned(),
            },
            Err(detail) => ReindexStatus::NotAccepted { detail },
        },
    };
    let retry_disposition = if matches!(reindex_status, ReindexStatus::NotAccepted { .. })
        || !state.summary.errors.is_empty()
        || state.has_conflict
        || state.has_staged
    {
        RetryDisposition::Incomplete
    } else if state.writes == 0 {
        RetryDisposition::IdempotentNoop
    } else {
        RetryDisposition::Applied
    };
    Ok(ArchiveMergeResult {
        // `ImportResult.entries_written` is the imported stream-entry count, not every
        // owner-state mutation. This retains the archive verb's captured seam contract.
        entries_written: state.summary.segments_copied + state.summary.imports_copied,
        entities_seeded: 0,
        owner_entity_after: state.owner_entity_after,
        principal_collision: state.principal_collision,
        errors: state.summary.errors.clone(),
        merge_summary: state.summary,
        entity_dispositions: state.entity_dispositions,
        segment_dispositions: state.segment_dispositions,
        retry_disposition,
        reindex_status,
        decision_log_path,
        staging_path,
    })
}

struct MergeState {
    summary: MergeSummary,
    entity_dispositions: Vec<EntityDisposition>,
    segment_dispositions: Vec<SegmentDisposition>,
    owner_entity_after: Option<Value>,
    principal_collision: Option<PrincipalCollision>,
    decision_log_path: PathBuf,
    staging_path: PathBuf,
    writes: usize,
    has_conflict: bool,
    has_staged: bool,
}

impl MergeState {
    fn new(decision_log_path: PathBuf, staging_path: PathBuf) -> Self {
        Self {
            summary: MergeSummary::default(),
            entity_dispositions: Vec::new(),
            segment_dispositions: Vec::new(),
            owner_entity_after: None,
            principal_collision: None,
            decision_log_path,
            staging_path,
            writes: 0,
            has_conflict: false,
            has_staged: false,
        }
    }
    fn decision(&self, state: &str, domain: &str, detail: Value) -> Result<(), ImportSourcesError> {
        append_jsonl(&self.decision_log_path, &json!({"state": state, "domain": domain, "detail": detail, "at": Utc::now().to_rfc3339()}))
            .map_err(|error| ImportSourcesError::DecisionLogWrite { path: self.decision_log_path.clone(), detail: error.to_string() })
    }
}

fn merge_segments(
    source: &Path,
    target: &Path,
    state: &mut MergeState,
) -> Result<(), ImportSourcesError> {
    let chronicle = source.join("chronicle");
    if !chronicle.is_dir() {
        return Ok(());
    }
    for day in sorted_dirs(&chronicle).map_err(|error| ImportSourcesError::SegmentMerge {
        path: chronicle.clone(),
        detail: error.to_string(),
    })? {
        let day_name = file_name(&day)?;
        let segments = iter_segments(source, PathOrDay::Directory(&day)).map_err(|error| {
            ImportSourcesError::SegmentMerge {
                path: day.clone(),
                detail: error.to_string(),
            }
        })?;
        for segment in segments {
            let destination = segment_destination(target, &day_name, &segment.stream, &segment.key);
            if destination.exists() {
                let kind = if tree_digest(&segment.path)? == tree_digest(&destination)? {
                    SegmentDispositionKind::IdenticalExisting
                } else {
                    state.has_conflict = true;
                    SegmentDispositionKind::DifferingContentCollision
                };
                state.summary.segments_skipped += 1;
                state.segment_dispositions.push(SegmentDisposition {
                    day: day_name.clone(),
                    stream: segment.stream,
                    key: segment.key,
                    disposition: kind,
                });
                continue;
            }
            state.decision(
                "prepared",
                "segments",
                json!({"day": day_name, "stream": segment.stream, "key": segment.key}),
            )?;
            publish_staged_dir(&destination, StagedDirOptions::default(), |staging| {
                copy_tree(&segment.path, staging)
            })
            .map_err(|error| ImportSourcesError::SegmentMerge {
                path: destination.clone(),
                detail: error.to_string(),
            })?;
            state.decision("committed", "segments", json!({"destination": destination}))?;
            state.summary.segments_copied += 1;
            state.writes += 1;
            state.segment_dispositions.push(SegmentDisposition {
                day: day_name.clone(),
                stream: segment.stream,
                key: segment.key,
                disposition: SegmentDispositionKind::Copied,
            });
        }
    }
    Ok(())
}

fn merge_entities(
    source: &Path,
    target: &Path,
    state: &mut MergeState,
) -> Result<(), ImportSourcesError> {
    let source_entities =
        load_all_journal_entities(source).map_err(|error| ImportSourcesError::EntityMerge {
            entity_id: "source".to_owned(),
            detail: error.to_string(),
        })?;
    let mut target_entities =
        load_all_journal_entities(target).map_err(|error| ImportSourcesError::EntityMerge {
            entity_id: "target".to_owned(),
            detail: error.to_string(),
        })?;
    let target_principal =
        read_journal_principal(target).map_err(|error| ImportSourcesError::EntityMerge {
            entity_id: "principal".to_owned(),
            detail: error.to_string(),
        })?;
    for source_entity in source_entities {
        let source_id = source_entity.id.clone();
        let source_value = source_entity.value;
        let name = source_value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let source_claims_principal = source_value.get("is_principal") == Some(&Value::Bool(true));
        let resolution = record_entity_resolution_from_name_evidence(
            target,
            name,
            &target_entities
                .iter()
                .map(|entity| entity.resolution_entity())
                .collect::<Vec<_>>(),
            json!({"kind":"journal"}),
            json!({"source_entity_id": source_id, "lane":"archive_merge"}),
            0.86,
            false,
        )
        .map_err(|error| ImportSourcesError::EntityMerge {
            entity_id: source_id.clone(),
            detail: error.to_string(),
        })?;
        match resolution.outcome {
            EntityResolutionOutcome::Resolved => {
                let index = resolution.entity_index.expect("resolved entity has index");
                let target_entity = &mut target_entities[index];
                let mut merged = target_entity.value.clone();
                let fields_changed = merge_entity_fields(&mut merged, &source_value);
                let target_is_principal = merged.get("is_principal") == Some(&Value::Bool(true));
                let principal_adoption = if !source_claims_principal {
                    PrincipalAdoption::NotClaimed
                } else if target_is_principal {
                    PrincipalAdoption::PreservedExistingTargetPrincipal
                } else {
                    PrincipalAdoption::RefusedOnNameMatch
                };
                if source_claims_principal
                    && target_principal.is_some()
                    && state.principal_collision.is_none()
                {
                    state.principal_collision = Some(PrincipalCollision {
                        target_entity_id: target_principal
                            .as_ref()
                            .and_then(|value| value.get("id"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        source_entity_id: source_id.clone(),
                    });
                }
                let target_id = target_entity.id.clone();
                state.decision("prepared", "entities", json!({"source_id": source_id, "target_id": target_id, "fields_changed": fields_changed, "principal_adoption": principal_adoption}))?;
                if !fields_changed.is_empty() {
                    save_entity_identity(target, &target_id, &merged, None).map_err(|error| {
                        ImportSourcesError::EntityMerge {
                            entity_id: target_id.clone(),
                            detail: error.to_string(),
                        }
                    })?;
                    target_entity.value = merged.clone();
                    state.summary.entities_merged += 1;
                    state.writes += 1;
                    state.owner_entity_after = Some(merged);
                } else {
                    state.summary.entities_skipped += 1;
                }
                // A committed-log failure after this point can abort without returning the
                // disposition despite the durable entity write above; keep this narrow risk
                // explicit until decision logging gains a recoverable commit protocol.
                state.decision(
                    "committed",
                    "entities",
                    json!({"source_id": source_id, "target_id": target_id}),
                )?;
                state.entity_dispositions.push(EntityDisposition {
                    source_id,
                    target_id: Some(target_id),
                    disposition: if fields_changed.is_empty() {
                        EntityDispositionKind::Skipped
                    } else {
                        EntityDispositionKind::Merged
                    },
                    fields_changed,
                    principal_adoption,
                    staging_path: None,
                });
            }
            EntityResolutionOutcome::Ambiguous => stage_entity(
                &source_id,
                &source_value,
                EntityDispositionKind::StagedAmbiguous,
                PrincipalAdoption::NotClaimed,
                state,
            )?,
            EntityResolutionOutcome::NoMatch => {
                if target_entities.iter().any(|entity| entity.id == source_id) {
                    stage_entity(
                        &source_id,
                        &source_value,
                        EntityDispositionKind::StagedIdCollision,
                        PrincipalAdoption::NotClaimed,
                        state,
                    )?;
                    continue;
                }
                let mut created = source_value.clone();
                let principal_adoption = if source_claims_principal {
                    if target_principal.is_some() {
                        PrincipalAdoption::ConflictReportedSeparately
                    } else {
                        PrincipalAdoption::ClearedOnCreate
                    }
                } else {
                    PrincipalAdoption::NotClaimed
                };
                if source_claims_principal
                    && target_principal.is_some()
                    && state.principal_collision.is_none()
                {
                    state.principal_collision = Some(PrincipalCollision {
                        target_entity_id: target_principal
                            .as_ref()
                            .and_then(|value| value.get("id"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        source_entity_id: source_id.clone(),
                    });
                }
                if source_claims_principal {
                    created
                        .as_object_mut()
                        .expect("entity is object")
                        .remove("is_principal");
                }
                state.decision("prepared", "entities", json!({"source_id": source_id, "create": true, "principal_adoption": principal_adoption}))?;
                save_entity_identity(target, &source_id, &created, None).map_err(|error| {
                    ImportSourcesError::EntityMerge {
                        entity_id: source_id.clone(),
                        detail: error.to_string(),
                    }
                })?;
                state.decision(
                    "committed",
                    "entities",
                    json!({"source_id": source_id, "create": true}),
                )?;
                target_entities.push(solstone_core_entity::JournalEntity {
                    id: source_id.clone(),
                    value: created,
                });
                state.summary.entities_created += 1;
                state.writes += 1;
                state.entity_dispositions.push(EntityDisposition {
                    source_id,
                    target_id: None,
                    disposition: EntityDispositionKind::Created,
                    fields_changed: Vec::new(),
                    principal_adoption,
                    staging_path: None,
                });
            }
        }
    }
    Ok(())
}

fn merge_entity_fields(target: &mut Value, source: &Value) -> Vec<String> {
    let Some(target_object) = target.as_object_mut() else {
        return Vec::new();
    };
    let mut changed = Vec::new();
    let target_akas = strings(target_object.get("aka"));
    let source_akas = strings(source.get("aka"));
    let akas = archive_dedupe_akas(&target_akas, &source_akas);
    if akas != target_akas && !akas.is_empty() {
        target_object.insert("aka".to_owned(), json!(akas));
        changed.push("aka".to_owned());
    }
    let target_emails = strings(target_object.get("emails"));
    let source_emails = strings(source.get("emails"));
    let emails = archive_dedupe_emails(&target_emails, &source_emails);
    if emails != target_emails && !emails.is_empty() {
        target_object.insert("emails".to_owned(), json!(emails));
        changed.push("emails".to_owned());
    }
    changed
}

fn stage_entity(
    source_id: &str,
    source: &Value,
    disposition: EntityDispositionKind,
    principal_adoption: PrincipalAdoption,
    state: &mut MergeState,
) -> Result<(), ImportSourcesError> {
    fs::create_dir_all(&state.staging_path).map_err(|error| ImportSourcesError::StagingWrite {
        path: state.staging_path.clone(),
        detail: error.to_string(),
    })?;
    let path =
        contained_path(&state.staging_path, &format!("{source_id}.json")).map_err(|error| {
            ImportSourcesError::StagingWrite {
                path: state.staging_path.clone(),
                detail: error.to_string(),
            }
        })?;
    state.decision(
        "prepared",
        "entities",
        json!({"source_id": source_id, "staged": true}),
    )?;
    fs::write(
        &path,
        serde_json::to_vec_pretty(source).expect("Value serializes"),
    )
    .map_err(|error| ImportSourcesError::StagingWrite {
        path: path.clone(),
        detail: error.to_string(),
    })?;
    state.decision(
        "committed",
        "entities",
        json!({"source_id": source_id, "staged": true}),
    )?;
    state.summary.entities_staged += 1;
    state.has_staged = true;
    state.writes += 1;
    state.entity_dispositions.push(EntityDisposition {
        source_id: source_id.to_owned(),
        target_id: None,
        disposition,
        fields_changed: Vec::new(),
        principal_adoption,
        staging_path: Some(path),
    });
    Ok(())
}

fn merge_facets(
    source: &Path,
    target: &Path,
    state: &mut MergeState,
) -> Result<(), ImportSourcesError> {
    let source_facets = source.join("facets");
    if !source_facets.is_dir() {
        return Ok(());
    }
    for facet_path in
        sorted_dirs(&source_facets).map_err(|error| ImportSourcesError::FacetMerge {
            facet: "facets".to_owned(),
            detail: error.to_string(),
        })?
    {
        let facet = file_name(&facet_path)?;
        let target_facet = target.join("facets").join(&facet);
        if !target_facet.exists() {
            state.decision(
                "prepared",
                "facets",
                json!({"facet": facet, "create": true}),
            )?;
            publish_staged_dir(&target_facet, StagedDirOptions::default(), |staging| {
                copy_tree(&facet_path, staging)
            })
            .map_err(|error| ImportSourcesError::FacetMerge {
                facet: facet.clone(),
                detail: error.to_string(),
            })?;
            state.decision(
                "committed",
                "facets",
                json!({"facet": facet, "create": true}),
            )?;
            state.summary.facets_created += 1;
            state.writes += 1;
            continue;
        }
        merge_facet_relationships(source, target, &facet, state)?;
        merge_facet_content(&facet_path, &target_facet, source, target, &facet, state)?;
        state.summary.facets_merged += 1;
    }
    Ok(())
}

fn merge_facet_relationships(
    source: &Path,
    target: &Path,
    facet: &str,
    state: &mut MergeState,
) -> Result<(), ImportSourcesError> {
    let entities = source.join("facets").join(facet).join("entities");
    if !entities.is_dir() {
        return Ok(());
    }
    for source_relationship in
        sorted_dirs(&entities).map_err(|error| ImportSourcesError::FacetMerge {
            facet: facet.to_owned(),
            detail: error.to_string(),
        })?
    {
        let entity_dir = file_name(&source_relationship)?;
        let source_link = read_facet_entity_link(source, facet, &entity_dir).map_err(|error| {
            ImportSourcesError::FacetMerge {
                facet: facet.to_owned(),
                detail: error.to_string(),
            }
        })?;
        let target_link = read_facet_entity_link(target, facet, &entity_dir).map_err(|error| {
            ImportSourcesError::FacetMerge {
                facet: facet.to_owned(),
                detail: error.to_string(),
            }
        })?;
        match (source_link, target_link) {
            (Some(source_link), Some(target_link)) => {
                let source_observations =
                    load_observations(source, facet, &entity_dir).map_err(|error| {
                        ImportSourcesError::FacetMerge {
                            facet: facet.to_owned(),
                            detail: error.to_string(),
                        }
                    })?;
                let target_observations =
                    load_observations(target, facet, &entity_dir).map_err(|error| {
                        ImportSourcesError::FacetMerge {
                            facet: facet.to_owned(),
                            detail: error.to_string(),
                        }
                    })?;
                let observations =
                    archive_dedupe_observations(&source_observations, &target_observations);
                if observations != target_observations {
                    state.decision(
                        "prepared",
                        "facets",
                        json!({"facet": facet, "relationship": entity_dir, "observations": true}),
                    )?;
                    save_observations(target, facet, &entity_dir, &observations).map_err(
                        |error| ImportSourcesError::FacetMerge {
                            facet: facet.to_owned(),
                            detail: error.to_string(),
                        },
                    )?;
                    state.decision(
                        "committed",
                        "facets",
                        json!({"facet": facet, "relationship": entity_dir}),
                    )?;
                    state.writes += 1;
                }
                let _ = (source_link, target_link); // Target link fields intentionally win.
            }
            (Some(source_link), None) => {
                let fields = source_link.value().as_object().cloned().unwrap_or_default();
                state.decision(
                    "prepared",
                    "facets",
                    json!({"facet": facet, "relationship": entity_dir, "create": true}),
                )?;
                save_facet_entity_link(
                    target,
                    facet,
                    &entity_dir,
                    source_link.entity_id(),
                    &fields,
                )
                .map_err(|error| ImportSourcesError::FacetMerge {
                    facet: facet.to_owned(),
                    detail: error.to_string(),
                })?;
                let source_observations =
                    load_observations(source, facet, &entity_dir).map_err(|error| {
                        ImportSourcesError::FacetMerge {
                            facet: facet.to_owned(),
                            detail: error.to_string(),
                        }
                    })?;
                if !source_observations.is_empty() {
                    save_observations(target, facet, &entity_dir, &source_observations).map_err(
                        |error| ImportSourcesError::FacetMerge {
                            facet: facet.to_owned(),
                            detail: error.to_string(),
                        },
                    )?;
                }
                state.decision(
                    "committed",
                    "facets",
                    json!({"facet": facet, "relationship": entity_dir}),
                )?;
                state.writes += 1;
            }
            _ => {}
        }
    }
    Ok(())
}

fn merge_facet_content(
    source_facet: &Path,
    _target_facet: &Path,
    source: &Path,
    target: &Path,
    facet: &str,
    state: &mut MergeState,
) -> Result<(), ImportSourcesError> {
    for relative in ["activities", "logs", "news"] {
        let source_dir = source_facet.join(relative);
        if !source_dir.is_dir() {
            continue;
        }
        for file in sorted_files_recursive(&source_dir)? {
            let relative_file = file
                .strip_prefix(&source_dir)
                .expect("under source")
                .to_string_lossy()
                .replace('\\', "/");
            let source_contents = match relative {
                "activities" => read_activity_file(source, facet, &relative_file),
                "logs" => read_log_file(source, facet, &relative_file),
                "news" => read_news_file(source, facet, &relative_file),
                _ => unreachable!(),
            }
            .map_err(|error| ImportSourcesError::FacetMerge {
                facet: facet.to_owned(),
                detail: error.to_string(),
            })?
            .unwrap_or_default();
            let target_contents = match relative {
                "activities" => read_activity_file(target, facet, &relative_file),
                "logs" => read_log_file(target, facet, &relative_file),
                "news" => read_news_file(target, facet, &relative_file),
                _ => unreachable!(),
            }
            .map_err(|error| ImportSourcesError::FacetMerge {
                facet: facet.to_owned(),
                detail: error.to_string(),
            })?;
            let merged = match (relative, target_contents.as_deref()) {
                ("news", Some(_)) => continue, // Target markdown is authoritative.
                ("logs", Some(existing)) => dedupe_lines(existing, &source_contents),
                ("activities", Some(existing)) if relative_file.ends_with(".jsonl") => {
                    dedupe_jsonl(existing, &source_contents)
                }
                (_, Some(_)) => continue, // Nested activity bytes are create-only.
                (_, None) => source_contents,
            };
            if target_contents.as_ref() == Some(&merged) {
                continue;
            }
            state.decision(
                "prepared",
                "facets",
                json!({"facet": facet, "file": relative_file}),
            )?;
            match relative {
                "activities" => write_activity_file(target, facet, &relative_file, &merged),
                "logs" => write_log_file(target, facet, &relative_file, &merged),
                "news" => write_news_file(target, facet, &relative_file, &merged),
                _ => unreachable!(),
            }
            .map_err(|error| ImportSourcesError::FacetMerge {
                facet: facet.to_owned(),
                detail: error.to_string(),
            })?;
            state.decision(
                "committed",
                "facets",
                json!({"facet": facet, "file": relative_file}),
            )?;
            state.writes += 1;
        }
    }
    Ok(())
}

fn merge_imports(
    source: &Path,
    target: &Path,
    state: &mut MergeState,
) -> Result<(), ImportSourcesError> {
    let imports = source.join("imports");
    if !imports.is_dir() {
        return Ok(());
    }
    for source_import in sorted_dirs(&imports).map_err(|error| ImportSourcesError::ImportMerge {
        path: imports.clone(),
        detail: error.to_string(),
    })? {
        let destination = target.join("imports").join(file_name(&source_import)?);
        if destination.exists() {
            state.summary.imports_skipped += 1;
            continue;
        }
        state.decision("prepared", "imports", json!({"destination": destination}))?;
        publish_staged_dir(&destination, StagedDirOptions::default(), |staging| {
            copy_tree(&source_import, staging)
        })
        .map_err(|error| ImportSourcesError::ImportMerge {
            path: destination.clone(),
            detail: error.to_string(),
        })?;
        state.decision("committed", "imports", json!({"destination": destination}))?;
        state.summary.imports_copied += 1;
        state.writes += 1;
    }
    Ok(())
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
fn dedupe_jsonl(target: &str, source: &str) -> String {
    let mut seen = BTreeSet::new();
    let mut rows = Vec::new();
    for line in target.lines().chain(source.lines()) {
        let key = serde_json::from_str::<Value>(line)
            .ok()
            .and_then(|value| value.get("id").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| line.to_owned());
        if seen.insert(key) {
            rows.push(line);
        }
    }
    if rows.is_empty() {
        String::new()
    } else {
        format!("{}\n", rows.join("\n"))
    }
}

fn dedupe_lines(target: &str, source: &str) -> String {
    let mut seen = BTreeSet::new();
    let mut lines = Vec::new();
    for line in target.lines().chain(source.lines()) {
        if seen.insert(line) {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}
fn segment_destination(target: &Path, day: &str, stream: &str, key: &str) -> PathBuf {
    let day = target.join("chronicle").join(day);
    if stream == DEFAULT_STREAM {
        day.join(key)
    } else {
        day.join(stream).join(key)
    }
}
fn file_name(path: &Path) -> Result<String, ImportSourcesError> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .ok_or_else(|| ImportSourcesError::ArchiveInvalid {
            path: path.to_path_buf(),
            detail: "non-UTF-8 or missing path name".to_owned(),
        })
}
fn sorted_dirs(path: &Path) -> io::Result<Vec<PathBuf>> {
    let mut paths = fs::read_dir(path)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_dir())
                .map(|_| entry.path())
        })
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}
fn sorted_files_recursive(path: &Path) -> Result<Vec<PathBuf>, ImportSourcesError> {
    let mut output = Vec::new();
    fn visit(path: &Path, output: &mut Vec<PathBuf>) -> io::Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            if ty.is_symlink() {
                continue;
            }
            if ty.is_dir() {
                visit(&entry.path(), output)?;
            } else if ty.is_file() {
                output.push(entry.path());
            }
        }
        Ok(())
    }
    visit(path, &mut output).map_err(|error| ImportSourcesError::FacetMerge {
        facet: path.display().to_string(),
        detail: error.to_string(),
    })?;
    output.sort();
    Ok(output)
}
fn copy_tree(source: &Path, destination: &Path) -> Result<(), io::Error> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_symlink() {
            return Err(io::Error::other(
                "refusing symbolic link while copying archive",
            ));
        }
        let target = destination.join(entry.file_name());
        if ty.is_dir() {
            fs::create_dir(&target)?;
            copy_tree(&entry.path(), &target)?;
        } else if ty.is_file() {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
fn tree_digest(path: &Path) -> Result<Vec<u8>, ImportSourcesError> {
    let mut hasher = Sha256::new();
    fn visit(root: &Path, current: &Path, hasher: &mut Sha256) -> Result<(), io::Error> {
        let mut entries = fs::read_dir(current)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let ty = entry.file_type()?;
            if ty.is_symlink() {
                return Err(io::Error::other("symbolic link in segment"));
            }
            let relative = entry
                .path()
                .strip_prefix(root)
                .expect("under root")
                .to_string_lossy()
                .to_string();
            hasher.update(relative.as_bytes());
            if ty.is_dir() {
                hasher.update(b"d");
                visit(root, &entry.path(), hasher)?;
            } else if ty.is_file() {
                hasher.update(b"f");
                let mut file = File::open(entry.path())?;
                let mut buffer = [0_u8; 32 * 1024];
                loop {
                    let count = file.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    hasher.update(&buffer[..count]);
                }
            }
        }
        Ok(())
    }
    visit(path, path, &mut hasher).map_err(|error| ImportSourcesError::SegmentMerge {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    Ok(hasher.finalize().to_vec())
}
