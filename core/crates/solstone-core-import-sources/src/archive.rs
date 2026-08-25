// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Safe, journal-root-explicit merge of a portable journal archive.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_entity::{
    AmbiguityObservation, EntityResolutionOutcome, archive_dedupe_akas, archive_dedupe_emails,
    archive_dedupe_observations, hold_entity_trust_lock, load_all_journal_entities,
    read_journal_principal, record_ambiguity_observation,
    record_entity_resolution_from_name_evidence, rewrite_identity_map_cache,
};
use solstone_core_entity_matching::normalize_resolution_query;
use solstone_core_facets::{
    hold_facet_trust_lock, load_observations, read_activity_file, read_facet_entity_link,
    read_log_file, read_news_file,
};
use solstone_core_import::ImportPreview;
use solstone_core_journal_io::{
    AtomicWriteOptions, LockError, LockOptions, PathOrDay, RecordIdentity, Segment,
    StagedDirOptions, StreamLocation, append_jsonl, atomic_replace, contained_path, hold_lock,
    iter_segments, publish_staged_dir, write_bytes_exclusive,
};
use solstone_core_segment::touch_stream_health_marker;
use solstone_core_transfer_manifest::{
    ExpectedMember, MANIFEST_NAME, TransferManifest, expected_members, parse_manifest,
    validate_expected_members,
};
use tar::{Archive, EntryType};
use zip::ZipArchive;

use crate::{ArchiveSafetyPhase, ImportSourcesError};

/// Trailing-slash authored tree prunes, copied from `PORTABLE_DENY` in
/// `solstone-core-journal-archive/src/deny.rs` (the `config/` … `solstone/`
/// entries, stored here without the slash).
/// Intentionally independent of that list: import-sources must not depend on
/// the export crate (capability-safe inventory + zip encode) just to warn when
/// a zip still carries those trees. Do not pin the two together in a test.
const AUTHORED_TOP_LEVEL_PRUNES: &[&str] = &[
    "config",
    "link",
    "tokens",
    "awareness",
    "apps",
    "backup",
    "solstone",
];
const JOURNAL_FAMILY_ROOTS: &[&str] = &["chronicle", "entities", "facets", "imports"];
const ZIP_LOCAL: [u8; 4] = [b'P', b'K', 3, 4];
const ZIP_EOCD: [u8; 4] = [b'P', b'K', 5, 6];
const ZIP_SPAN: [u8; 4] = [b'P', b'K', 7, 8];
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

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

/// Read-only listing of what a journal archive would copy. Never writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchivePlan {
    pub days: Vec<String>,
    pub entity_count: u64,
    pub facet_count: u64,
    pub warning_count: u64,
    pub warnings: Vec<String>,
    pub payload_files: usize,
    pub summary: String,
}

impl From<ArchivePlan> for ImportPreview {
    fn from(plan: ArchivePlan) -> Self {
        Self {
            date_range: match (plan.days.first(), plan.days.last()) {
                (Some(first), Some(last)) => (first.clone(), last.clone()),
                _ => (String::new(), String::new()),
            },
            item_count: plan.days.len() as u64,
            entity_count: plan.entity_count,
            summary: plan.summary,
        }
    }
}

/// Plan a journal-archive merge from its archive metadata. Creates nothing.
pub fn plan_journal_archive(archive_path: &Path) -> Result<ArchivePlan, ImportSourcesError> {
    match classify_archive(archive_path)? {
        ArchiveKind::Zip => plan_zip_archive(archive_path),
        ArchiveKind::V1GzipTar => plan_v1_gzip_tar(archive_path),
    }
}

fn plan_zip_archive(archive_path: &Path) -> Result<ArchivePlan, ImportSourcesError> {
    let file = File::open(archive_path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => ImportSourcesError::ArchiveNotFound {
            path: archive_path.to_path_buf(),
        },
        _ => ImportSourcesError::ArchiveInvalid {
            path: archive_path.to_path_buf(),
            detail: error.to_string(),
        },
    })?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| ImportSourcesError::ArchiveInvalid {
            path: archive_path.to_path_buf(),
            detail: error.to_string(),
        })?;
    let mut names = Vec::with_capacity(archive.len());
    let mut directories = Vec::with_capacity(archive.len());
    let mut root_candidates = BTreeSet::new();
    for index in 0..archive.len() {
        let entry =
            archive
                .by_index(index)
                .map_err(|error| ImportSourcesError::ArchiveInvalid {
                    path: archive_path.to_path_buf(),
                    detail: error.to_string(),
                })?;
        let name = entry.name().replace('\\', "/");
        let is_dir = entry.is_dir() || name.ends_with('/');
        if let Some(first) = member_components(&name).first().copied()
            && first != "__MACOSX"
            && first != ".DS_Store"
        {
            root_candidates.insert(std::ffi::OsString::from(first));
        }
        directories.push(is_dir);
        names.push(name);
    }
    let root = archive_root(&root_candidates)?;
    let mut days = BTreeSet::new();
    let mut entity_ids = BTreeSet::new();
    let mut facet_ids = BTreeSet::new();
    let mut prune_warnings = BTreeSet::new();
    let mut has_macosx = false;
    let mut has_ds_store = false;
    let mut payload_files = 0_usize;
    for (name, is_dir) in names.iter().zip(directories) {
        let relative = strip_archive_root(name, &root);
        let components = member_components(relative);
        if components.contains(&".DS_Store")
            || Path::new(relative)
                .file_name()
                .is_some_and(|file_name| file_name == ".DS_Store")
        {
            has_ds_store = true;
        }
        if components.first().copied() == Some("__MACOSX") {
            has_macosx = true;
        }
        let Some(first) = components.first().copied() else {
            continue;
        };
        if AUTHORED_TOP_LEVEL_PRUNES.contains(&first) {
            prune_warnings.insert(first.to_owned());
        }
        if first == "chronicle" && components.get(1).copied().is_some_and(is_eight_digit_day) {
            days.insert(components[1].to_owned());
        }
        if first == "entities" && components.len() == 3 && components[2] == "entity.json" && !is_dir
        {
            entity_ids.insert(components[1].to_owned());
        }
        if first == "facets" && components.len() == 3 && components[2] == "facet.json" && !is_dir {
            facet_ids.insert(components[1].to_owned());
        }
        if !is_dir && JOURNAL_FAMILY_ROOTS.contains(&first) {
            payload_files += 1;
        }
    }
    let mut warnings = prune_warnings.into_iter().collect::<Vec<_>>();
    if has_macosx {
        warnings.push("__MACOSX".to_owned());
    }
    if has_ds_store {
        warnings.push(".DS_Store".to_owned());
    }
    let entity_count = entity_ids.len() as u64;
    let facet_count = facet_ids.len() as u64;
    let warning_count = warnings.len() as u64;
    let days = days.into_iter().collect::<Vec<_>>();
    let day_count = days.len() as u64;
    let summary = format!(
        "Journal archive: {day_count} days, {entity_count} entities, {facet_count} facets ({warning_count} warnings)"
    );
    Ok(ArchivePlan {
        days,
        entity_count,
        facet_count,
        warning_count,
        warnings,
        payload_files,
        summary,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveKind {
    Zip,
    V1GzipTar,
}

fn classify_archive(path: &Path) -> Result<ArchiveKind, ImportSourcesError> {
    let mut file = File::open(path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => ImportSourcesError::ArchiveNotFound {
            path: path.to_path_buf(),
        },
        _ => ImportSourcesError::ArchiveInvalid {
            path: path.to_path_buf(),
            detail: error.to_string(),
        },
    })?;
    let mut magic = [0_u8; 4];
    let read = file
        .read(&mut magic)
        .map_err(|error| ImportSourcesError::ArchiveInvalid {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })?;
    if read >= 2 && magic[..2] == GZIP_MAGIC {
        return Ok(ArchiveKind::V1GzipTar);
    }
    if read >= 4 && (magic == ZIP_LOCAL || magic == ZIP_EOCD || magic == ZIP_SPAN) {
        return Ok(ArchiveKind::Zip);
    }
    Err(ImportSourcesError::ArchiveInvalid {
        path: path.to_path_buf(),
        detail: "unrecognized archive magic".to_owned(),
    })
}

fn plan_v1_gzip_tar(path: &Path) -> Result<ArchivePlan, ImportSourcesError> {
    let validated = validate_v1_gzip_tar(path, u64::MAX)?;
    Ok(v1_plan(&validated.manifest, validated.expected.len()))
}

fn member_components(name: &str) -> Vec<&str> {
    name.trim_end_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect()
}

fn strip_archive_root<'a>(name: &'a str, root: &Path) -> &'a str {
    let Some(root) = root.to_str().filter(|value| !value.is_empty()) else {
        return name;
    };
    match name.strip_prefix(root) {
        Some(rest) if rest.is_empty() => rest,
        Some(rest) => rest.strip_prefix('/').unwrap_or(name),
        None => name,
    }
}

fn is_eight_digit_day(name: &str) -> bool {
    name.len() == 8 && name.as_bytes().iter().all(|byte| byte.is_ascii_digit())
}

/// Validate, extract, and merge an archive while holding the target merge lock.
pub fn merge_journal_archive(
    archive_path: &Path,
    target_journal_root: &Path,
    options: &ArchiveMergeOptions,
    reindexer: Option<&dyn FullReindexRequester>,
) -> Result<ArchiveMergeResult, ImportSourcesError> {
    let kind = classify_archive(archive_path)?;
    let validated = validate_archive(archive_path, kind, options)?;
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
    kind: ValidatedArchiveKind,
    expanded_size: u64,
}

#[derive(Debug)]
enum ValidatedArchiveKind {
    Zip { root: PathBuf },
    V1GzipTar(ValidatedV1Archive),
}

#[derive(Debug)]
struct ValidatedV1Archive {
    manifest: TransferManifest,
    expected: BTreeMap<String, ExpectedMember>,
}

fn validate_archive(
    path: &Path,
    kind: ArchiveKind,
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
    match kind {
        ArchiveKind::Zip => validate_zip_archive(path, options),
        ArchiveKind::V1GzipTar => {
            let validated = validate_v1_gzip_tar(path, options.max_uncompressed_bytes)?;
            let expanded_size = declared_v1_size(&validated.manifest, path)?;
            Ok(ValidatedArchive {
                kind: ValidatedArchiveKind::V1GzipTar(validated),
                expanded_size,
            })
        }
    }
}

fn validate_zip_archive(
    path: &Path,
    options: &ArchiveMergeOptions,
) -> Result<ValidatedArchive, ImportSourcesError> {
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
        kind: ValidatedArchiveKind::Zip { root },
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
    match &validated.kind {
        ValidatedArchiveKind::Zip { root } => {
            extract_zip_archive(path, root, validated.expanded_size, run_dir, options)
        }
        ValidatedArchiveKind::V1GzipTar(validated) => {
            materialize_v1_gzip_tar(path, validated, run_dir, options)
        }
    }
}

fn extract_zip_archive(
    path: &Path,
    root: &Path,
    expanded_size: u64,
    run_dir: &Path,
    options: &ArchiveMergeOptions,
) -> Result<PathBuf, ImportSourcesError> {
    fs::create_dir_all(run_dir).map_err(|error| ImportSourcesError::ExtractionFailed {
        archive: path.to_path_buf(),
        extraction_dir: run_dir.to_path_buf(),
        detail: error.to_string(),
    })?;
    let required = expanded_size.saturating_add(options.free_space_reserve_bytes);
    let stat = nix::sys::statvfs::statvfs(run_dir).map_err(|error| {
        ImportSourcesError::ExtractionFailed {
            archive: path.to_path_buf(),
            extraction_dir: run_dir.to_path_buf(),
            detail: error.to_string(),
        }
    })?;
    // Cast explicitly: `statvfs`'s field widths differ by platform (u64 on Linux,
    // u32 on Darwin), so multiplying them unconverted only compiles on Linux.
    // Same defect class as `solstone-core::check` and the precedent in
    // `solstone-core-local::install::fit_report::free_bytes`.
    let available = (stat.blocks_available() as u64).saturating_mul(stat.fragment_size() as u64);
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
    Ok(run_dir.join(root))
}

fn validate_v1_gzip_tar(
    path: &Path,
    max_uncompressed_bytes: u64,
) -> Result<ValidatedV1Archive, ImportSourcesError> {
    let file = File::open(path).map_err(|error| ImportSourcesError::ArchiveInvalid {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    let mut archive = Archive::new(flate2::read::GzDecoder::new(file));
    let mut entries = archive.entries().map_err(|_| invalid_v1_gzip(path))?;
    let first = entries
        .next()
        .ok_or_else(|| invalid_v1_gzip(path))?
        .map_err(|_| invalid_v1_gzip(path))?;
    let manifest = read_v1_manifest(first, path)?;
    let expected = expected_members(&manifest).map_err(|error| invalid_v1_manifest(path, error))?;
    validate_expected_members(&expected).map_err(|error| invalid_v1_manifest(path, error))?;
    let declared = declared_v1_size(&manifest, path)?;
    if declared > max_uncompressed_bytes {
        return Err(ImportSourcesError::ArchiveUncompressedTooLarge {
            bytes: declared,
            maximum: max_uncompressed_bytes,
        });
    }
    Ok(ValidatedV1Archive { manifest, expected })
}

fn read_v1_manifest<R: Read>(
    mut entry: tar::Entry<'_, R>,
    path: &Path,
) -> Result<TransferManifest, ImportSourcesError> {
    if entry.header().entry_type() != EntryType::Regular {
        return Err(invalid_v1_manifest(
            path,
            "first archive member must be regular manifest.json",
        ));
    }
    let member = tar_member_name(&mut entry, ArchiveSafetyPhase::Validation, path)?;
    if member != MANIFEST_NAME {
        return Err(invalid_v1_manifest(
            path,
            "first archive member must be manifest.json",
        ));
    }
    let mut bytes = Vec::new();
    entry
        .read_to_end(&mut bytes)
        .map_err(|_| invalid_v1_gzip(path))?;
    parse_manifest(&bytes).map_err(|error| invalid_v1_manifest(path, error))
}

fn declared_v1_size(manifest: &TransferManifest, path: &Path) -> Result<u64, ImportSourcesError> {
    manifest
        .segments
        .values()
        .flat_map(|segment| segment.files.iter())
        .try_fold(0_u64, |total, file| total.checked_add(file.size))
        .ok_or_else(|| ImportSourcesError::ArchiveInvalid {
            path: path.to_path_buf(),
            detail: "invalid v1 transfer manifest: declared size overflow".to_owned(),
        })
}

fn v1_plan(manifest: &TransferManifest, payload_files: usize) -> ArchivePlan {
    let days = vec![manifest.day.clone()];
    let day_count = days.len() as u64;
    ArchivePlan {
        days,
        entity_count: 0,
        facet_count: 0,
        warning_count: 0,
        warnings: Vec::new(),
        payload_files,
        summary: format!("Journal archive: {day_count} days, 0 entities, 0 facets (0 warnings)"),
    }
}

fn materialize_v1_gzip_tar(
    path: &Path,
    validated: &ValidatedV1Archive,
    run_dir: &Path,
    options: &ArchiveMergeOptions,
) -> Result<PathBuf, ImportSourcesError> {
    fs::create_dir_all(run_dir).map_err(|error| ImportSourcesError::ExtractionFailed {
        archive: path.to_path_buf(),
        extraction_dir: run_dir.to_path_buf(),
        detail: error.to_string(),
    })?;
    let declared = declared_v1_size(&validated.manifest, path)?;
    let required = declared.saturating_add(options.free_space_reserve_bytes);
    let stat = nix::sys::statvfs::statvfs(run_dir).map_err(|error| {
        ImportSourcesError::ExtractionFailed {
            archive: path.to_path_buf(),
            extraction_dir: run_dir.to_path_buf(),
            detail: error.to_string(),
        }
    })?;
    let available = (stat.blocks_available() as u64).saturating_mul(stat.fragment_size() as u64);
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

    let result = (|| {
        let file = File::open(path).map_err(|error| ImportSourcesError::ExtractionFailed {
            archive: path.to_path_buf(),
            extraction_dir: run_dir.to_path_buf(),
            detail: error.to_string(),
        })?;
        let mut archive = Archive::new(flate2::read::GzDecoder::new(file));
        let mut entries = archive.entries().map_err(|_| invalid_v1_gzip(path))?;
        let first = entries
            .next()
            .ok_or_else(|| invalid_v1_gzip(path))?
            .map_err(|_| invalid_v1_gzip(path))?;
        let manifest = read_v1_manifest(first, path)?;
        if manifest.day != validated.manifest.day {
            return Err(invalid_v1_manifest(
                path,
                "manifest changed during extraction",
            ));
        }
        let mut seen = BTreeSet::new();
        let mut copied = 0_u64;
        let day_root = run_dir.join("chronicle").join(&manifest.day);
        for entry in entries {
            let mut entry = entry.map_err(|_| invalid_v1_gzip(path))?;
            let name = tar_member_name(&mut entry, ArchiveSafetyPhase::Extraction, path)?;
            if entry.header().entry_type() != EntryType::Regular {
                return Err(ImportSourcesError::ArchiveUnsafeEntry {
                    phase: ArchiveSafetyPhase::Extraction,
                    entry: name,
                    reason: "non-regular tar member".to_owned(),
                });
            }
            let Some(expected) = validated.expected.get(&name) else {
                return Err(ImportSourcesError::ArchiveUnsafeEntry {
                    phase: ArchiveSafetyPhase::Extraction,
                    entry: name,
                    reason: "member is not listed in manifest".to_owned(),
                });
            };
            if !seen.insert(name.clone()) {
                return Err(ImportSourcesError::ArchiveUnsafeEntry {
                    phase: ArchiveSafetyPhase::Extraction,
                    entry: name,
                    reason: "duplicate archive member".to_owned(),
                });
            }
            let segment = contained_path(&day_root, &expected.route.archive_key())
                .map_err(|error| invalid_v1_manifest(path, error.to_string()))?;
            let output = contained_path(&segment, &expected.file.name)
                .map_err(|error| invalid_v1_manifest(path, error.to_string()))?;
            let parent = output.parent().expect("contained child has parent");
            fs::create_dir_all(parent).map_err(|error| ImportSourcesError::ExtractionFailed {
                archive: path.to_path_buf(),
                extraction_dir: run_dir.to_path_buf(),
                detail: error.to_string(),
            })?;
            let mut output_file =
                File::create(&output).map_err(|error| ImportSourcesError::ExtractionFailed {
                    archive: path.to_path_buf(),
                    extraction_dir: run_dir.to_path_buf(),
                    detail: error.to_string(),
                })?;
            let (sha256, size) = copy_v1_member(
                &mut entry,
                &mut output_file,
                &mut copied,
                options.max_uncompressed_bytes,
                path,
                run_dir,
            )?;
            output_file
                .sync_all()
                .map_err(|error| ImportSourcesError::ExtractionFailed {
                    archive: path.to_path_buf(),
                    extraction_dir: run_dir.to_path_buf(),
                    detail: error.to_string(),
                })?;
            if size != expected.file.size {
                return Err(ImportSourcesError::ArchiveInvalid {
                    path: path.to_path_buf(),
                    detail: format!("v1 member size mismatch: {name}"),
                });
            }
            if sha256 != expected.file.sha256 {
                return Err(ImportSourcesError::ArchiveInvalid {
                    path: path.to_path_buf(),
                    detail: format!("v1 member sha256 mismatch: {name}"),
                });
            }
        }
        let missing = validated
            .expected
            .keys()
            .filter(|name| !seen.contains(*name))
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(ImportSourcesError::ArchiveInvalid {
                path: path.to_path_buf(),
                detail: format!(
                    "v1 manifest members missing from archive: {}",
                    missing.join(", ")
                ),
            });
        }
        Ok(run_dir.to_path_buf())
    })();
    match result {
        Ok(root) => Ok(root),
        Err(error) => cleanup_extraction(path, run_dir, error),
    }
}

fn copy_v1_member(
    reader: &mut impl Read,
    output: &mut File,
    copied: &mut u64,
    maximum: u64,
    archive_path: &Path,
    extraction_dir: &Path,
) -> Result<(String, u64), ImportSourcesError> {
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 32 * 1024];
    loop {
        let read =
            reader
                .read(&mut buffer)
                .map_err(|error| ImportSourcesError::ArchiveInvalid {
                    path: archive_path.to_path_buf(),
                    detail: format!("read v1 archive member: {error}"),
                })?;
        if read == 0 {
            break;
        }
        *copied = copied.checked_add(read as u64).ok_or(
            ImportSourcesError::ArchiveUncompressedTooLarge {
                bytes: u64::MAX,
                maximum,
            },
        )?;
        if *copied > maximum {
            return Err(ImportSourcesError::ArchiveUncompressedTooLarge {
                bytes: *copied,
                maximum,
            });
        }
        output.write_all(&buffer[..read]).map_err(|error| {
            ImportSourcesError::ExtractionFailed {
                archive: archive_path.to_path_buf(),
                extraction_dir: extraction_dir.to_path_buf(),
                detail: format!("write extracted v1 archive member: {error}"),
            }
        })?;
        digest.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((format!("{:x}", digest.finalize()), size))
}

fn tar_member_name<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    phase: ArchiveSafetyPhase,
    archive_path: &Path,
) -> Result<String, ImportSourcesError> {
    let path = entry
        .path()
        .map_err(|error| ImportSourcesError::ArchiveInvalid {
            path: archive_path.to_path_buf(),
            detail: format!("read v1 archive member path: {error}"),
        })?;
    let name = path
        .to_str()
        .ok_or_else(|| ImportSourcesError::ArchiveUnsafeEntry {
            phase,
            entry: path.display().to_string(),
            reason: "non-UTF-8 path".to_owned(),
        })?;
    if name.starts_with('/')
        || name.starts_with('\\')
        || name.contains('\\')
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
    Ok(name.to_owned())
}

fn invalid_v1_gzip(path: &Path) -> ImportSourcesError {
    ImportSourcesError::ArchiveInvalid {
        path: path.to_path_buf(),
        detail: "invalid v1 gzip tar archive".to_owned(),
    }
}

fn invalid_v1_manifest(path: &Path, detail: impl std::fmt::Display) -> ImportSourcesError {
    ImportSourcesError::ArchiveInvalid {
        path: path.to_path_buf(),
        detail: format!("invalid v1 transfer manifest: {detail}"),
    }
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
    let staged_publish = run_dir.join("staged-publish");
    let publish_undo = run_dir.join("publish-undo");
    let mut state = MergeState::new(
        decision_log_path.clone(),
        staging_path.clone(),
        staged_publish,
        publish_undo,
    );
    fs::create_dir_all(&state.staged_publish).map_err(|error| {
        ImportSourcesError::StagingWrite {
            path: state.staged_publish.clone(),
            detail: error.to_string(),
        }
    })?;
    log_skipped_extras(source, &mut state)?;
    stage_segments(source, target, &mut state)?;
    {
        let _entity_lock =
            hold_entity_trust_lock(target).map_err(|error| ImportSourcesError::EntityMerge {
                entity_id: "lock".to_owned(),
                detail: error.to_string(),
            })?;
        stage_entities(source, target, &mut state)?;
    }
    {
        let _facet_lock =
            hold_facet_trust_lock(target).map_err(|error| ImportSourcesError::FacetMerge {
                facet: "lock".to_owned(),
                detail: error.to_string(),
            })?;
        stage_facets(source, target, &mut state)?;
    }
    stage_imports(source, target, &mut state)?;
    publish_transaction(target, &mut state)?;
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

#[derive(Clone)]
enum PublishUnit {
    Tree { relative: String },
    File { relative: String },
}

impl PublishUnit {
    fn relative(&self) -> &str {
        match self {
            Self::Tree { relative } | Self::File { relative } => relative,
        }
    }
}

#[derive(Clone, Copy)]
enum UndoKind {
    UnlinkNew,
    Restore,
}

struct UndoRecord {
    kind: UndoKind,
    relative: String,
}

struct MergeState {
    summary: MergeSummary,
    entity_dispositions: Vec<EntityDisposition>,
    segment_dispositions: Vec<SegmentDisposition>,
    owner_entity_after: Option<Value>,
    principal_collision: Option<PrincipalCollision>,
    decision_log_path: PathBuf,
    staging_path: PathBuf,
    staged_publish: PathBuf,
    publish_undo: PathBuf,
    chronicle_units: Vec<PublishUnit>,
    entity_units: Vec<PublishUnit>,
    facet_units: Vec<PublishUnit>,
    import_units: Vec<PublishUnit>,
    pending_ambiguities: Vec<AmbiguityObservation>,
    published: Vec<UndoRecord>,
    published_entity_json: bool,
    writes: usize,
    has_conflict: bool,
    has_staged: bool,
}

impl MergeState {
    fn new(
        decision_log_path: PathBuf,
        staging_path: PathBuf,
        staged_publish: PathBuf,
        publish_undo: PathBuf,
    ) -> Self {
        Self {
            summary: MergeSummary::default(),
            entity_dispositions: Vec::new(),
            segment_dispositions: Vec::new(),
            owner_entity_after: None,
            principal_collision: None,
            decision_log_path,
            staging_path,
            staged_publish,
            publish_undo,
            chronicle_units: Vec::new(),
            entity_units: Vec::new(),
            facet_units: Vec::new(),
            import_units: Vec::new(),
            pending_ambiguities: Vec::new(),
            published: Vec::new(),
            published_entity_json: false,
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

fn stage_segments(
    source: &Path,
    target: &Path,
    state: &mut MergeState,
) -> Result<(), ImportSourcesError> {
    let chronicle = source.join("chronicle");
    if !chronicle.is_dir() {
        return Ok(());
    }
    let mut days = Vec::new();
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
        days.push((day.clone(), day_name, segments));
    }
    solstone_core_journal_io::utf8_identities(
        days.iter().flat_map(|(_, _, segments)| segments.iter()),
    )
    .map_err(|error| ImportSourcesError::SegmentMerge {
        path: chronicle.clone(),
        detail: error.to_string(),
    })?;
    for (_day, day_name, segments) in days {
        for segment in segments {
            let identity =
                segment
                    .record_identity()
                    .map_err(|error| ImportSourcesError::SegmentMerge {
                        path: segment.path().to_path_buf(),
                        detail: error.to_string(),
                    })?;
            // Disposition (c): destination follows StreamLocation + exact UTF-8
            // basename so Named("_default") stays under `_default/` and same-key
            // siblings (`093000_300_a` / `_b`) land at distinct paths.
            let destination = segment_destination_for(target, &day_name, &segment, identity);
            if destination.exists() {
                let kind = if tree_digest(segment.path())? == tree_digest(&destination)? {
                    SegmentDispositionKind::IdenticalExisting
                } else {
                    state.has_conflict = true;
                    SegmentDispositionKind::DifferingContentCollision
                };
                state.summary.segments_skipped += 1;
                state.segment_dispositions.push(SegmentDisposition {
                    day: day_name.clone(),
                    stream: identity.stream.to_owned(),
                    key: identity.key.to_owned(),
                    disposition: kind,
                });
                continue;
            }
            let relative = segment_relative_for(&day_name, &segment, identity);
            state.decision(
                "prepared",
                "segments",
                json!({"day": day_name, "stream": identity.stream, "key": identity.key}),
            )?;
            stage_tree(state, segment.path(), &relative, |error| {
                ImportSourcesError::SegmentMerge {
                    path: destination.clone(),
                    detail: error.to_string(),
                }
            })?;
            state.decision("committed", "segments", json!({"destination": relative}))?;
            state.summary.segments_copied += 1;
            state.writes += 1;
            state.chronicle_units.push(PublishUnit::Tree { relative });
            state.segment_dispositions.push(SegmentDisposition {
                day: day_name.clone(),
                stream: identity.stream.to_owned(),
                key: identity.key.to_owned(),
                disposition: SegmentDispositionKind::Copied,
            });
        }
    }
    Ok(())
}

fn stage_entities(
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
            true,
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
                    let relative = format!("entities/{target_id}/entity.json");
                    stage_json_file(state, &relative, &merged)?;
                    target_entity.value = merged.clone();
                    state.summary.entities_merged += 1;
                    state.writes += 1;
                    state.owner_entity_after = Some(merged);
                    state.entity_units.push(PublishUnit::File { relative });
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
            EntityResolutionOutcome::Ambiguous => {
                if resolution
                    .tier
                    .is_some_and(|tier| !tier.is_high_confidence())
                {
                    state.pending_ambiguities.push(AmbiguityObservation {
                        scope: json!({"kind":"journal"}),
                        query: name.to_owned(),
                        normalized_query: normalize_resolution_query(name),
                        observed_tier: resolution
                            .tier
                            .map(|tier| i64::from(tier as u8))
                            .unwrap_or_default(),
                        ranked_candidates: resolution
                            .candidates
                            .iter()
                            .map(|candidate| {
                                json!({
                                    "id": candidate.id,
                                    "name": candidate.name,
                                    "tier": i64::from(candidate.tier as u8),
                                    "score": candidate.score,
                                })
                            })
                            .collect(),
                        origin: json!({"source_entity_id": source_id, "lane":"archive_merge"}),
                    });
                }
                stage_entity(
                    &source_id,
                    &source_value,
                    EntityDispositionKind::StagedAmbiguous,
                    PrincipalAdoption::NotClaimed,
                    state,
                )?;
            }
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
                let relative = format!("entities/{source_id}/entity.json");
                stage_json_file(state, &relative, &created)?;
                state.decision(
                    "committed",
                    "entities",
                    json!({"source_id": source_id, "create": true}),
                )?;
                target_entities.push(solstone_core_entity::JournalEntity {
                    id: source_id.clone(),
                    value: created,
                });
                state.entity_units.push(PublishUnit::File { relative });
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

fn stage_facets(
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
            let relative = format!("facets/{facet}");
            state.decision(
                "prepared",
                "facets",
                json!({"facet": facet, "create": true}),
            )?;
            stage_tree(state, &facet_path, &relative, |error| {
                ImportSourcesError::FacetMerge {
                    facet: facet.clone(),
                    detail: error.to_string(),
                }
            })?;
            state.decision(
                "committed",
                "facets",
                json!({"facet": facet, "create": true}),
            )?;
            state.summary.facets_created += 1;
            state.writes += 1;
            state.facet_units.push(PublishUnit::Tree { relative });
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
                    let relative =
                        format!("facets/{facet}/entities/{entity_dir}/observations.jsonl");
                    stage_bytes(
                        state,
                        &relative,
                        observations_jsonl(&observations).as_bytes(),
                    )?;
                    state.facet_units.push(PublishUnit::File { relative });
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
                let mut fields = source_link.value().as_object().cloned().unwrap_or_default();
                fields.insert(
                    "entity_id".to_owned(),
                    Value::String(source_link.entity_id().to_owned()),
                );
                state.decision(
                    "prepared",
                    "facets",
                    json!({"facet": facet, "relationship": entity_dir, "create": true}),
                )?;
                let link_relative = format!("facets/{facet}/entities/{entity_dir}/entity.json");
                stage_json_file(state, &link_relative, &Value::Object(fields))?;
                state.facet_units.push(PublishUnit::File {
                    relative: link_relative,
                });
                let source_observations =
                    load_observations(source, facet, &entity_dir).map_err(|error| {
                        ImportSourcesError::FacetMerge {
                            facet: facet.to_owned(),
                            detail: error.to_string(),
                        }
                    })?;
                if !source_observations.is_empty() {
                    let relative =
                        format!("facets/{facet}/entities/{entity_dir}/observations.jsonl");
                    stage_bytes(
                        state,
                        &relative,
                        observations_jsonl(&source_observations).as_bytes(),
                    )?;
                    state.facet_units.push(PublishUnit::File { relative });
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
            let staged_relative = format!("facets/{facet}/{relative}/{relative_file}");
            stage_bytes(state, &staged_relative, merged.as_bytes())?;
            state.facet_units.push(PublishUnit::File {
                relative: staged_relative,
            });
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

fn stage_imports(
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
        let name = file_name(&source_import)?;
        let destination = target.join("imports").join(&name);
        if destination.exists() {
            state.summary.imports_skipped += 1;
            continue;
        }
        let relative = format!("imports/{name}");
        state.decision("prepared", "imports", json!({"destination": relative}))?;
        stage_tree(state, &source_import, &relative, |error| {
            ImportSourcesError::ImportMerge {
                path: destination.clone(),
                detail: error.to_string(),
            }
        })?;
        state.decision("committed", "imports", json!({"destination": relative}))?;
        state.summary.imports_copied += 1;
        state.writes += 1;
        state.import_units.push(PublishUnit::Tree { relative });
    }
    Ok(())
}

fn log_skipped_extras(source: &Path, state: &mut MergeState) -> Result<(), ImportSourcesError> {
    let Ok(entries) = fs::read_dir(source) else {
        return Ok(());
    };
    let mut names = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    names.sort();
    for path in names {
        let name = file_name(&path)?;
        if JOURNAL_FAMILY_ROOTS.contains(&name.as_str()) {
            continue;
        }
        state.decision("skipped", "extra", json!({"path": name}))?;
        if path.is_dir() {
            for file in extra_files(&path) {
                let relative = file
                    .strip_prefix(source)
                    .expect("under source")
                    .to_string_lossy()
                    .replace('\\', "/");
                state.decision("skipped", "extra", json!({"path": relative}))?;
            }
        }
    }
    Ok(())
}

fn stage_tree(
    state: &MergeState,
    source: &Path,
    relative: &str,
    map_error: impl FnOnce(io::Error) -> ImportSourcesError,
) -> Result<(), ImportSourcesError> {
    stage_tree_inner(state, source, relative).map_err(map_error)
}

fn stage_tree_inner(state: &MergeState, source: &Path, relative: &str) -> io::Result<()> {
    let destination = join_contained(&state.staged_publish, relative)
        .map_err(|error| io::Error::other(error.to_string()))?;
    fs::create_dir_all(&destination)?;
    copy_tree(source, &destination)
}

fn stage_json_file(
    state: &MergeState,
    relative: &str,
    value: &Value,
) -> Result<(), ImportSourcesError> {
    let bytes = serde_json::to_vec_pretty(value).expect("Value serializes");
    stage_bytes(state, relative, &bytes)
}

fn stage_bytes(state: &MergeState, relative: &str, bytes: &[u8]) -> Result<(), ImportSourcesError> {
    let path = join_contained(&state.staged_publish, relative).map_err(|error| {
        ImportSourcesError::StagingWrite {
            path: state.staged_publish.clone(),
            detail: error.to_string(),
        }
    })?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| ImportSourcesError::StagingWrite {
            path: path.clone(),
            detail: error.to_string(),
        })?;
    }
    fs::write(&path, bytes).map_err(|error| ImportSourcesError::StagingWrite {
        path: path.clone(),
        detail: error.to_string(),
    })
}

fn observations_jsonl(observations: &[Value]) -> String {
    let mut content = String::new();
    for observation in observations {
        content.push_str(&serde_json::to_string(observation).expect("Value serializes"));
        content.push('\n');
    }
    content
}

fn publish_transaction(target: &Path, state: &mut MergeState) -> Result<(), ImportSourcesError> {
    let mut units = Vec::new();
    units.extend(sorted_units(&state.chronicle_units));
    let entity_units = sorted_units(&state.entity_units);
    let facet_units = sorted_units(&state.facet_units);
    let import_units = sorted_units(&state.import_units);
    if entity_units.is_empty()
        && facet_units.is_empty()
        && import_units.is_empty()
        && units.is_empty()
        && state.pending_ambiguities.is_empty()
    {
        return Ok(());
    }
    fs::create_dir_all(&state.publish_undo).map_err(|error| ImportSourcesError::StagingWrite {
        path: state.publish_undo.clone(),
        detail: error.to_string(),
    })?;
    // Entity and facet locks were held for stage and dropped; re-acquire them
    // here for publish. Holding them across staging would overlap the two
    // locks (forbidden) because all four families stage before any publish.
    // The archive-merge lock still excludes a concurrent merge for the whole
    // operation. Do not "fix" this by holding both.
    let result: Result<(), ImportSourcesError> = (|| {
        publish_units(target, state, &units)?;
        {
            let _entity_lock = hold_entity_trust_lock(target).map_err(|error| {
                ImportSourcesError::EntityMerge {
                    entity_id: "lock".to_owned(),
                    detail: error.to_string(),
                }
            })?;
            publish_units(target, state, &entity_units)?;
            if state.published_entity_json {
                rewrite_identity_map_cache(target).map_err(|error| {
                    ImportSourcesError::MergePublishFailed {
                        detail: error.to_string(),
                    }
                })?;
            }
            publish_pending_ambiguities(target, state)?;
        }
        {
            let _facet_lock =
                hold_facet_trust_lock(target).map_err(|error| ImportSourcesError::FacetMerge {
                    facet: "lock".to_owned(),
                    detail: error.to_string(),
                })?;
            publish_units(target, state, &facet_units)?;
        }
        publish_units(target, state, &import_units)?;
        Ok(())
    })();
    if let Err(error) = result {
        let undo_failures = undo_publish(target, state);
        let detail = if undo_failures.is_empty() {
            error.to_string()
        } else {
            format!("{error}; undo incomplete: {}", undo_failures.join("; "))
        };
        return Err(ImportSourcesError::MergePublishFailed { detail });
    }
    let published_days = state
        .chronicle_units
        .iter()
        .filter_map(|unit| {
            let mut components = unit.relative().split('/');
            match (components.next(), components.next()) {
                (Some("chronicle"), Some(day)) if is_eight_digit_day(day) => Some(day.to_owned()),
                _ => None,
            }
        })
        .collect::<BTreeSet<_>>();
    let marker_failures = published_days
        .into_iter()
        .filter_map(|day| {
            touch_stream_health_marker(target, &day)
                .err()
                .map(|error| format!("{day}: {error}"))
        })
        .collect::<Vec<_>>();
    if !marker_failures.is_empty() {
        return Err(ImportSourcesError::MergePublishFailed {
            detail: format!(
                "stream marker update failed after archive content was published: {}; published content was not rolled back",
                marker_failures.join("; ")
            ),
        });
    }
    Ok(())
}

fn sorted_units(units: &[PublishUnit]) -> Vec<PublishUnit> {
    let mut units = units.to_vec();
    units.sort_by(|left, right| left.relative().cmp(right.relative()));
    units
}

fn publish_units(
    target: &Path,
    state: &mut MergeState,
    units: &[PublishUnit],
) -> Result<(), ImportSourcesError> {
    for unit in units {
        publish_one(target, state, unit)?;
    }
    Ok(())
}

fn publish_one(
    target: &Path,
    state: &mut MergeState,
    unit: &PublishUnit,
) -> Result<(), ImportSourcesError> {
    let relative = unit.relative().to_owned();
    let destination = join_contained(target, &relative).map_err(|error| {
        ImportSourcesError::MergePublishFailed {
            detail: error.to_string(),
        }
    })?;
    let staged = join_contained(&state.staged_publish, &relative).map_err(|error| {
        ImportSourcesError::MergePublishFailed {
            detail: error.to_string(),
        }
    })?;
    match unit {
        PublishUnit::Tree { .. } => {
            if destination.exists() {
                return Ok(());
            }
            publish_staged_dir(&destination, StagedDirOptions::default(), |staging| {
                copy_tree(&staged, staging)
            })
            .map_err(|error| ImportSourcesError::MergePublishFailed {
                detail: error.to_string(),
            })?;
            state.published.push(UndoRecord {
                kind: UndoKind::UnlinkNew,
                relative,
            });
        }
        PublishUnit::File { .. } => match fs::symlink_metadata(&destination) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let bytes =
                    fs::read(&staged).map_err(|error| ImportSourcesError::MergePublishFailed {
                        detail: error.to_string(),
                    })?;
                write_bytes_exclusive(
                    &destination,
                    &bytes,
                    AtomicWriteOptions { mode: Some(0o600) },
                )
                .map_err(|error| ImportSourcesError::MergePublishFailed {
                    detail: error.to_string(),
                })?;
                mark_entity_json(state, &relative);
                state.published.push(UndoRecord {
                    kind: UndoKind::UnlinkNew,
                    relative,
                });
            }
            Ok(metadata) if metadata.is_file() => {
                let undo = join_contained(&state.publish_undo, &relative).map_err(|error| {
                    ImportSourcesError::MergePublishFailed {
                        detail: error.to_string(),
                    }
                })?;
                if let Some(parent) = undo.parent() {
                    fs::create_dir_all(parent).map_err(|error| {
                        ImportSourcesError::MergePublishFailed {
                            detail: error.to_string(),
                        }
                    })?;
                }
                fs::copy(&destination, &undo).map_err(|error| {
                    ImportSourcesError::MergePublishFailed {
                        detail: error.to_string(),
                    }
                })?;
                let bytes =
                    fs::read(&staged).map_err(|error| ImportSourcesError::MergePublishFailed {
                        detail: error.to_string(),
                    })?;
                atomic_replace(
                    &destination,
                    &bytes,
                    AtomicWriteOptions { mode: Some(0o600) },
                )
                .map_err(|error| ImportSourcesError::MergePublishFailed {
                    detail: error.to_string(),
                })?;
                mark_entity_json(state, &relative);
                state.published.push(UndoRecord {
                    kind: UndoKind::Restore,
                    relative,
                });
            }
            Ok(_) => {
                return Err(ImportSourcesError::MergePublishFailed {
                    detail: format!("kind mismatch at {relative}"),
                });
            }
            Err(error) => {
                return Err(ImportSourcesError::MergePublishFailed {
                    detail: error.to_string(),
                });
            }
        },
    }
    maybe_crash_publish(state.published.len());
    maybe_fail_publish(state.published.len())?;
    Ok(())
}

fn mark_entity_json(state: &mut MergeState, relative: &str) {
    if relative.starts_with("entities/") && relative.ends_with("/entity.json") {
        state.published_entity_json = true;
    }
}

fn publish_pending_ambiguities(
    target: &Path,
    state: &mut MergeState,
) -> Result<(), ImportSourcesError> {
    if state.pending_ambiguities.is_empty() {
        return Ok(());
    }
    let live = target.join("entities/ambiguities.jsonl");
    let relative = "entities/ambiguities.jsonl";
    match fs::symlink_metadata(&live) {
        Ok(metadata) if metadata.is_file() => {
            let undo = join_contained(&state.publish_undo, relative).map_err(|error| {
                ImportSourcesError::MergePublishFailed {
                    detail: error.to_string(),
                }
            })?;
            if let Some(parent) = undo.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    ImportSourcesError::MergePublishFailed {
                        detail: error.to_string(),
                    }
                })?;
            }
            fs::copy(&live, &undo).map_err(|error| ImportSourcesError::MergePublishFailed {
                detail: error.to_string(),
            })?;
            state.published.push(UndoRecord {
                kind: UndoKind::Restore,
                relative: relative.to_owned(),
            });
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            state.published.push(UndoRecord {
                kind: UndoKind::UnlinkNew,
                relative: relative.to_owned(),
            });
        }
        Ok(_) => {
            return Err(ImportSourcesError::MergePublishFailed {
                detail: "kind mismatch at entities/ambiguities.jsonl".to_owned(),
            });
        }
        Err(error) => {
            return Err(ImportSourcesError::MergePublishFailed {
                detail: error.to_string(),
            });
        }
    }
    for observation in &state.pending_ambiguities {
        record_ambiguity_observation(target, observation).map_err(|error| {
            ImportSourcesError::MergePublishFailed {
                detail: error.to_string(),
            }
        })?;
    }
    Ok(())
}

fn undo_publish(target: &Path, state: &mut MergeState) -> Vec<String> {
    #[cfg(test)]
    maybe_drop_undo_preimages(state);
    let mut failures = Vec::new();
    let mut undone = 0_usize;
    for record in state.published.iter().rev() {
        let destination = target.join(&record.relative);
        match record.kind {
            UndoKind::UnlinkNew => {
                let result = if destination.is_dir() {
                    fs::remove_dir_all(&destination)
                } else {
                    fs::remove_file(&destination)
                };
                if let Err(error) = result {
                    failures.push(format!("unlink {}: {error}", record.relative));
                }
                remove_empty_parents(target, &record.relative);
            }
            UndoKind::Restore => {
                let undo = state.publish_undo.join(&record.relative);
                if let Err(error) = fs::rename(&undo, &destination) {
                    failures.push(format!("restore {}: {error}", record.relative));
                }
            }
        }
        undone += 1;
        maybe_crash_undo(undone);
    }
    if state.published_entity_json
        && let Err(error) = rewrite_identity_map_cache(target)
    {
        failures.push(format!("rewrite identity map: {error}"));
    }
    failures
}

fn extra_files(path: &Path) -> Vec<PathBuf> {
    let mut output = Vec::new();
    fn visit(current: &Path, output: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(current) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let ty = entry.file_type().ok();
            if ty.as_ref().is_some_and(std::fs::FileType::is_dir) {
                visit(&entry.path(), output);
            } else if ty.as_ref().is_some_and(std::fs::FileType::is_file) {
                output.push(entry.path());
            }
        }
    }
    visit(path, &mut output);
    output.sort();
    output
}

fn remove_empty_parents(target: &Path, relative: &str) {
    let mut current = target.join(relative);
    while let Some(parent) = current.parent() {
        if parent == target {
            break;
        }
        if fs::remove_dir(parent).is_err() {
            break;
        }
        current = parent.to_path_buf();
    }
}

fn join_contained(root: &Path, relative: &str) -> Result<PathBuf, ImportSourcesError> {
    contained_path(root, relative).map_err(|error| ImportSourcesError::StagingWrite {
        path: root.to_path_buf(),
        detail: error.to_string(),
    })
}

fn segment_relative_for(day: &str, segment: &Segment, identity: RecordIdentity<'_>) -> String {
    match segment.stream() {
        StreamLocation::Direct => format!("chronicle/{day}/{}", identity.name),
        StreamLocation::Named(_) => {
            format!("chronicle/{day}/{}/{}", identity.stream, identity.name)
        }
    }
}

fn segment_destination_for(
    target: &Path,
    day: &str,
    segment: &Segment,
    identity: RecordIdentity<'_>,
) -> PathBuf {
    target.join(segment_relative_for(day, segment, identity))
}

#[cfg(test)]
thread_local! {
    static PUBLISH_CRASH_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static PUBLISH_FAIL_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static UNDO_CRASH_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static UNDO_DROP_PREIMAGES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub fn inject_publish_crash_after(count: Option<usize>) {
    PUBLISH_CRASH_AFTER.with(|cell| cell.set(count));
}

#[cfg(test)]
pub fn inject_publish_fail_after(count: Option<usize>) {
    PUBLISH_FAIL_AFTER.with(|cell| cell.set(count));
}

#[cfg(test)]
pub fn inject_undo_crash_after(count: Option<usize>) {
    UNDO_CRASH_AFTER.with(|cell| cell.set(count));
}

#[cfg(test)]
pub fn inject_undo_drop_preimages(drop: bool) {
    UNDO_DROP_PREIMAGES.with(|cell| cell.set(drop));
}

#[cfg(test)]
fn maybe_drop_undo_preimages(state: &MergeState) {
    if UNDO_DROP_PREIMAGES.with(|cell| cell.replace(false)) {
        let _ = fs::remove_dir_all(&state.publish_undo);
    }
}

fn maybe_crash_publish(count: usize) {
    #[cfg(test)]
    {
        PUBLISH_CRASH_AFTER.with(|cell| {
            if cell.get() == Some(count) {
                cell.set(None);
                panic!("injected crash mid-publish");
            }
        });
    }
    let _ = count;
}

fn maybe_fail_publish(count: usize) -> Result<(), ImportSourcesError> {
    #[cfg(test)]
    {
        let fail = PUBLISH_FAIL_AFTER.with(|cell| {
            if cell.get() == Some(count) {
                cell.set(None);
                true
            } else {
                false
            }
        });
        if fail {
            return Err(ImportSourcesError::MergePublishFailed {
                detail: "injected publish failure".to_owned(),
            });
        }
    }
    let _ = count;
    Ok(())
}

fn maybe_crash_undo(count: usize) {
    #[cfg(test)]
    {
        UNDO_CRASH_AFTER.with(|cell| {
            if cell.get() == Some(count) {
                cell.set(None);
                panic!("injected crash mid-undo");
            }
        });
    }
    let _ = count;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::ffi::OsString;
    use std::io::Write;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    struct FaultReset;
    impl Drop for FaultReset {
        fn drop(&mut self) {
            inject_publish_crash_after(None);
            inject_publish_fail_after(None);
            inject_undo_crash_after(None);
            inject_undo_drop_preimages(false);
        }
    }

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    #[cfg(target_os = "linux")]
    #[test]
    fn later_day_identity_failure_does_not_stage_earlier_day() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let tree = PlanTree::new();
        let source = tree.path.join("source");
        let target = tree.path.join("target");
        let run = tree.path.join("run");
        fs::create_dir_all(source.join("chronicle/20260101/120000_60")).unwrap();
        fs::write(source.join("chronicle/20260101/120000_60/value"), b"early").unwrap();
        let late = source
            .join("chronicle/20260102")
            .join(OsStr::from_bytes(b"s\xff"))
            .join("120000_60");
        fs::create_dir_all(&late).unwrap();
        fs::write(late.join("value"), b"late").unwrap();
        fs::create_dir_all(&target).unwrap();
        let staged = run.join("staged-publish");
        fs::create_dir_all(&staged).unwrap();
        let mut state = MergeState::new(
            run.join("decision-log.jsonl"),
            run.join("staged-entities"),
            staged.clone(),
            run.join("publish-undo"),
        );

        assert!(stage_segments(&source, &target, &mut state).is_err());
        assert_eq!(state.writes, 0);
        assert!(state.chronicle_units.is_empty());
        assert!(!staged.join("chronicle").exists());
        assert!(!target.join("chronicle").exists());
    }

    #[test]
    fn plan_dry_run_leaves_chronicle_day_absent_and_writes_nothing() {
        let tree = PlanTree::new();
        let day = "20260311";
        let archive = write_zip(
            &tree.path,
            &[(&format!("chronicle/{day}/120000_60/value"), b"would copy")],
        );
        let target = tree.path.join("target");
        fs::create_dir(&target).unwrap();
        let before_target = collect_tree(&target);
        let temp = std::env::temp_dir();
        let before_temp = list_names(&temp);
        let default_work = temp.join("solstone-archive-merge");
        let default_work_existed = default_work.exists();
        let before_work = if default_work_existed {
            collect_tree(&default_work)
        } else {
            BTreeSet::new()
        };

        let plan = plan_journal_archive(&archive).unwrap();
        assert_eq!(plan.days, vec![day.to_owned()]);
        assert!(!target.join("chronicle").join(day).exists());
        assert_eq!(collect_tree(&target), before_target);
        assert!(!target.join("health/locks/archive-merge").exists());
        assert!(!target.join("health/locks/archive-merge.lock").exists());
        assert!(
            !target
                .join("health/locks/archive-merge.owner.json")
                .exists()
        );
        assert!(!target.join("imports/archive-merge-work").exists());
        assert!(!target.join("working_root").exists());
        assert!(
            collect_tree(&target)
                .iter()
                .all(|path| !path.ends_with("decision-log.jsonl"))
        );
        if default_work_existed {
            assert_eq!(collect_tree(&default_work), before_work);
        } else {
            assert!(!default_work.exists());
        }
        let gained = list_names(&temp)
            .difference(&before_temp)
            .cloned()
            .collect::<Vec<_>>();
        for name in &gained {
            let text = name.to_string_lossy();
            assert!(
                !text.starts_with("extract-"),
                "plan created extract member {text}"
            );
            assert!(
                !text.contains("solstone-archive-merge"),
                "plan created working_root {text}"
            );
            assert!(
                !text.starts_with(".tmp"),
                "plan created TempDir member {text}"
            );
        }
    }

    #[test]
    fn plan_summary_matches_oracle_wording() {
        let tree = PlanTree::new();
        let archive = write_zip(
            &tree.path,
            &[
                ("chronicle/20260311/120000_60/value", b"day"),
                ("config/journal.json", b"{}"),
            ],
        );
        let plan = plan_journal_archive(&archive).unwrap();
        assert_eq!(
            plan.summary,
            "Journal archive: 1 days, 0 entities, 0 facets (1 warnings)"
        );
        assert_eq!(plan.days, vec!["20260311".to_owned()]);
        assert_eq!(plan.entity_count, 0);
        assert_eq!(plan.facet_count, 0);
        assert_eq!(plan.warning_count, 1);
        assert_eq!(plan.warnings, vec!["config".to_owned()]);
        let preview: ImportPreview = plan.into();
        assert_eq!(preview.item_count, 1);
        assert_eq!(
            preview.summary,
            "Journal archive: 1 days, 0 entities, 0 facets (1 warnings)"
        );
    }

    #[test]
    fn plan_refuses_gzip_magic_without_writing() {
        let tree = PlanTree::new();
        let gzip = tree.path.join("archive.tar.gz");
        fs::write(&gzip, [0x1f, 0x8b, 0x08, 0x00]).unwrap();
        let target = tree.path.join("target");
        fs::create_dir(&target).unwrap();
        let error = plan_journal_archive(&gzip).unwrap_err();
        match error {
            ImportSourcesError::ArchiveInvalid { detail, .. } => {
                assert_eq!(detail, "invalid v1 gzip tar archive");
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert!(collect_tree(&target).is_empty());
    }

    #[test]
    fn merge_refuses_gzip_magic_with_the_same_wording_as_plan() {
        let tree = PlanTree::new();
        let gzip = tree.path.join("archive.tar.gz");
        fs::write(&gzip, [0x1f, 0x8b, 0x08, 0x00]).unwrap();
        let target = tree.path.join("target");
        fs::create_dir(&target).unwrap();
        let options = ArchiveMergeOptions {
            working_root: tree.path.join("work"),
            ..ArchiveMergeOptions::default()
        };
        let planned = plan_journal_archive(&gzip).unwrap_err();
        let applied = merge_journal_archive(&gzip, &target, &options, None).unwrap_err();
        match (&planned, &applied) {
            (
                ImportSourcesError::ArchiveInvalid {
                    detail: plan_detail,
                    ..
                },
                ImportSourcesError::ArchiveInvalid {
                    detail: apply_detail,
                    ..
                },
            ) => {
                assert_eq!(plan_detail, "invalid v1 gzip tar archive");
                assert_eq!(apply_detail, plan_detail);
            }
            other => panic!("unexpected errors: {other:?}"),
        }
        assert!(collect_tree(&target).is_empty());
    }

    #[test]
    fn merge_copies_same_key_siblings_to_distinct_destinations() {
        let tree = PlanTree::new();
        let archive = write_zip(
            &tree.path,
            &[
                ("chronicle/20260101/other/093000_300_a/value", b"a"),
                ("chronicle/20260101/other/093000_300_b/value", b"b"),
            ],
        );
        let target = tree.path.join("target");
        fs::create_dir(&target).unwrap();
        let result = merge_journal_archive(&archive, &target, &merge_options(&tree), None).unwrap();
        assert_eq!(result.merge_summary.segments_copied, 2);
        assert_eq!(
            fs::read(target.join("chronicle/20260101/other/093000_300_a/value")).unwrap(),
            b"a"
        );
        assert_eq!(
            fs::read(target.join("chronicle/20260101/other/093000_300_b/value")).unwrap(),
            b"b"
        );
    }

    #[test]
    fn merge_keeps_named_default_apart_from_direct_layout() {
        let tree = PlanTree::new();
        let archive = write_zip(
            &tree.path,
            &[
                ("chronicle/20260101/080000_300/value", b"direct"),
                ("chronicle/20260101/_default/080000_300/value", b"named"),
            ],
        );
        let target = tree.path.join("target");
        fs::create_dir(&target).unwrap();
        let error =
            merge_journal_archive(&archive, &target, &merge_options(&tree), None).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains(
                "named stream directory \"_default\" cannot be spelled as a record identity"
            ),
            "{message}"
        );
        assert!(!target.join("chronicle/20260101/080000_300/value").exists());
        assert!(
            !target
                .join("chronicle/20260101/_default/080000_300/value")
                .exists()
        );
    }

    #[test]
    fn v1_gzip_tar_merges_under_manifest_day_and_is_magic_not_extension() {
        let tree = PlanTree::new();
        let archive = fixture_v1(&tree.path, "v1-minimal.tar.gz");
        let target = tree.path.join("target");
        fs::create_dir(&target).unwrap();
        let options = merge_options(&tree);

        let result = merge_journal_archive(&archive, &target, &options, None).unwrap();
        let segment = target.join("chronicle/20260203/audio/120000_30");
        assert!(result.merge_summary.segments_copied > 0);
        assert_eq!(fs::read(segment.join("stream.json")).unwrap(), b"stream");
        assert_eq!(fs::read(segment.join("device.json")).unwrap(), b"device");
        assert!(!target.join("audio").exists());

        let renamed = tree.path.join("fixture-named.zip");
        fs::copy(&archive, &renamed).unwrap();
        let renamed_target = tree.path.join("renamed-target");
        fs::create_dir(&renamed_target).unwrap();
        let renamed_result =
            merge_journal_archive(&renamed, &renamed_target, &options, None).unwrap();
        assert!(renamed_result.merge_summary.segments_copied > 0);
        assert_eq!(
            fs::read(renamed_target.join("chronicle/20260203/audio/120000_30/stream.json"))
                .unwrap(),
            b"stream"
        );
    }

    #[test]
    fn v1_gzip_tar_collision_and_identical_retry_preserve_destination() {
        let tree = PlanTree::new();
        let archive = fixture_v1(&tree.path, "v1-minimal.tar.gz");
        let target = tree.path.join("target");
        let segment = target.join("chronicle/20260203/audio/120000_30");
        fs::create_dir_all(&segment).unwrap();
        fs::write(segment.join("stream.json"), b"destination wins").unwrap();
        let options = merge_options(&tree);

        let collision = merge_journal_archive(&archive, &target, &options, None).unwrap();
        assert_eq!(collision.merge_summary.segments_skipped, 1);
        assert_eq!(
            fs::read(segment.join("stream.json")).unwrap(),
            b"destination wins"
        );
        assert_eq!(
            sorted_dirs(&target.join("chronicle/20260203/audio"))
                .unwrap()
                .into_iter()
                .map(|path| file_name(&path).unwrap())
                .collect::<Vec<_>>(),
            ["120000_30"]
        );

        let clean = tree.path.join("clean");
        fs::create_dir(&clean).unwrap();
        let first = merge_journal_archive(&archive, &clean, &options, None).unwrap();
        assert!(first.merge_summary.segments_copied > 0);
        let second = merge_journal_archive(&archive, &clean, &options, None).unwrap();
        assert_eq!(second.merge_summary.segments_copied, 0);
        assert_eq!(second.merge_summary.segments_skipped, 1);
    }

    #[test]
    fn v1_gzip_tar_failures_leave_journal_families_byte_identical() {
        let tree = PlanTree::new();
        let target = tree.path.join("target");
        fs::create_dir_all(target.join("chronicle/existing/120000_30")).unwrap();
        fs::write(target.join("chronicle/existing/120000_30/value"), b"kept").unwrap();
        for family in ["entities", "facets", "imports"] {
            fs::create_dir_all(target.join(family)).unwrap();
        }
        let before = journal_family_snapshot(&target);
        let good_sha = format!("{:x}", Sha256::digest(b"stream"));
        let manifests = [
            (
                "sha.tar.gz",
                v1_manifest("20260203", &"0".repeat(64), 6),
                vec![("audio/120000_30/stream.json", b"stream" as &[u8])],
            ),
            (
                "size.tar.gz",
                v1_manifest("20260203", &good_sha, 7),
                vec![("audio/120000_30/stream.json", b"stream" as &[u8])],
            ),
        ];
        let options = merge_options(&tree);
        for (name, manifest, members) in manifests {
            let archive = write_v1_archive(&tree.path, name, &manifest, &members);
            assert!(merge_journal_archive(&archive, &target, &options, None).is_err());
            assert_eq!(journal_family_snapshot(&target), before, "{name}");
            assert_no_extraction_residue(&options.working_root);
        }
        let traversal = write_v1_archive_with_traversal(
            &tree.path,
            "traversal.tar.gz",
            &v1_manifest("20260203", &good_sha, 6),
        );
        assert!(merge_journal_archive(&traversal, &target, &options, None).is_err());
        assert_eq!(journal_family_snapshot(&target), before, "traversal.tar.gz");
        assert_no_extraction_residue(&options.working_root);
    }

    #[test]
    fn v1_gzip_tar_enforces_declared_and_streamed_caps() {
        let tree = PlanTree::new();
        let archive = fixture_v1(&tree.path, "v1-minimal.tar.gz");
        let target = tree.path.join("target");
        fs::create_dir(&target).unwrap();
        let metadata_cap = ArchiveMergeOptions {
            working_root: tree.path.join("metadata-work"),
            max_archive_bytes: 0,
            ..ArchiveMergeOptions::default()
        };
        assert!(matches!(
            merge_journal_archive(&archive, &target, &metadata_cap, None),
            Err(ImportSourcesError::ArchiveTooLarge { .. })
        ));

        let sha = format!("{:x}", Sha256::digest(b"stream"));
        let underdeclared = write_v1_archive(
            &tree.path,
            "underdeclared.tar.gz",
            &v1_manifest("20260203", &sha, 1),
            &[("audio/120000_30/stream.json", b"stream")],
        );
        let streamed_cap = ArchiveMergeOptions {
            working_root: tree.path.join("stream-work"),
            max_uncompressed_bytes: 5,
            ..ArchiveMergeOptions::default()
        };
        assert!(matches!(
            merge_journal_archive(&underdeclared, &target, &streamed_cap, None),
            Err(ImportSourcesError::ArchiveUncompressedTooLarge { .. })
        ));
    }

    #[test]
    fn plan_v1_gzip_tar_reports_day_without_writing() {
        let tree = PlanTree::new();
        let archive = fixture_v1(&tree.path, "v1-minimal.tar.gz");
        let target = tree.path.join("target");
        fs::create_dir(&target).unwrap();
        let work = tree.path.join("work");
        let before_target = collect_tree(&target);
        let isolated_tmpdir = tree.path.join("isolated-tmpdir");
        fs::create_dir(&isolated_tmpdir).unwrap();
        let before_tmp = list_names(&isolated_tmpdir);
        let plan = plan_journal_archive(&archive).unwrap();
        assert_eq!(plan.days, vec!["20260203".to_owned()]);
        assert_eq!(collect_tree(&target), before_target);
        assert!(!work.exists());
        assert!(!target.join("health/locks").exists());
        assert_eq!(list_names(&isolated_tmpdir), before_tmp);
    }

    #[test]
    fn crash_mid_publish_leaves_undo_and_does_not_auto_resume() {
        let _reset = FaultReset;
        let tree = PlanTree::new();
        let archive = write_zip(
            &tree.path,
            &[
                ("chronicle/20260311/120000_60/value", b"day"),
                (
                    "entities/new-person/entity.json",
                    br#"{"id":"new-person","name":"Qxjvplmzt","type":"Person"}"#,
                ),
            ],
        );
        let target = tree.path.join("target");
        fs::create_dir(&target).unwrap();
        let options = ArchiveMergeOptions {
            working_root: tree.path.join("work"),
            ..ArchiveMergeOptions::default()
        };
        inject_publish_crash_after(Some(1));
        let panicked = catch_unwind(AssertUnwindSafe(|| {
            merge_journal_archive(&archive, &target, &options, None)
        }));
        assert!(panicked.is_err());
        let undo = find_named(&tree.path.join("work"), "publish-undo");
        assert!(
            undo.is_some(),
            "crash mid-publish must leave publish-undo visible"
        );
        let first_run = undo
            .as_ref()
            .and_then(|path| path.parent())
            .unwrap()
            .to_path_buf();
        inject_publish_crash_after(None);
        let second = merge_journal_archive(&archive, &target, &options, None).unwrap();
        assert_ne!(
            second.staging_path.parent().map(Path::to_path_buf),
            Some(first_run),
            "second merge must be a new transaction, not a resume"
        );
        assert!(find_named(&tree.path.join("work"), "publish-undo").is_some());
    }

    #[test]
    fn crash_mid_undo_leaves_operator_visible_undo() {
        let _reset = FaultReset;
        let tree = PlanTree::new();
        let archive = write_zip(
            &tree.path,
            &[
                ("chronicle/20260311/120000_60/value", b"day"),
                (
                    "entities/new-person/entity.json",
                    br#"{"id":"new-person","name":"Qxjvplmzt","type":"Person"}"#,
                ),
            ],
        );
        let target = tree.path.join("target");
        fs::create_dir(&target).unwrap();
        let options = ArchiveMergeOptions {
            working_root: tree.path.join("work"),
            ..ArchiveMergeOptions::default()
        };
        inject_publish_fail_after(Some(1));
        inject_undo_crash_after(Some(1));
        let panicked = catch_unwind(AssertUnwindSafe(|| {
            merge_journal_archive(&archive, &target, &options, None)
        }));
        assert!(panicked.is_err());
        assert!(
            find_named(&tree.path.join("work"), "publish-undo").is_some(),
            "crash mid-undo must leave publish-undo visible"
        );
        inject_publish_fail_after(None);
        inject_undo_crash_after(None);
        let first_undo = find_named(&tree.path.join("work"), "publish-undo").unwrap();
        let first_run = first_undo.parent().unwrap().to_path_buf();
        let second = merge_journal_archive(&archive, &target, &options, None).unwrap();
        assert_ne!(
            second.staging_path.parent().map(Path::to_path_buf),
            Some(first_run)
        );
        assert!(first_undo.is_dir());
    }

    #[test]
    fn failed_undo_is_carried_in_merge_publish_failed_detail() {
        let _reset = FaultReset;
        let tree = PlanTree::new();
        fs::create_dir_all(tree.path.join("target/entities/person")).unwrap();
        fs::write(
            tree.path.join("target/entities/person/entity.json"),
            br#"{"id":"person","name":"Person","type":"Person"}"#,
        )
        .unwrap();
        let archive = write_zip(
            &tree.path,
            &[(
                "entities/person/entity.json",
                br#"{"id":"person","name":"Person","type":"Person","aka":["Alias"]}"#,
            )],
        );
        let target = tree.path.join("target");
        let options = ArchiveMergeOptions {
            working_root: tree.path.join("work"),
            ..ArchiveMergeOptions::default()
        };
        inject_publish_fail_after(Some(1));
        inject_undo_drop_preimages(true);
        let error = merge_journal_archive(&archive, &target, &options, None).unwrap_err();
        match error {
            ImportSourcesError::MergePublishFailed { detail } => {
                assert!(
                    detail.contains("undo incomplete"),
                    "expected nested undo failures in {detail}"
                );
                assert!(
                    detail.contains("restore entities/person/entity.json"),
                    "{detail}"
                );
            }
            other => panic!("expected MergePublishFailed, got {other:?}"),
        }
    }

    fn find_named(root: &Path, name: &str) -> Option<PathBuf> {
        fn visit(current: &Path, name: &str) -> Option<PathBuf> {
            let entries = fs::read_dir(current).ok()?;
            for entry in entries.filter_map(Result::ok) {
                if entry.file_name() == name {
                    return Some(entry.path());
                }
                if entry.file_type().ok()?.is_dir()
                    && let Some(found) = visit(&entry.path(), name)
                {
                    return Some(found);
                }
            }
            None
        }
        visit(root, name)
    }

    fn write_zip(tree: &Path, members: &[(&str, &[u8])]) -> PathBuf {
        let archive = tree.join(format!("plan-{}.zip", NEXT.fetch_add(1, Ordering::Relaxed)));
        let mut writer = ZipWriter::new(File::create(&archive).unwrap());
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, bytes) in members {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
        archive
    }

    fn fixture_v1(tree: &Path, name: &str) -> PathBuf {
        let archive = tree.join(name);
        fs::write(
            &archive,
            include_bytes!("../../solstone-core-transfer/tests/fixtures/v1-minimal-20260203.tgz"),
        )
        .unwrap();
        archive
    }

    fn merge_options(tree: &PlanTree) -> ArchiveMergeOptions {
        ArchiveMergeOptions {
            working_root: tree.path.join("work"),
            ..ArchiveMergeOptions::default()
        }
    }

    fn v1_manifest(day: &str, sha256: &str, size: u64) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "version": 1,
            "day": day,
            "segments": {
                "audio/120000_30": {
                    "files": [{"name": "stream.json", "sha256": sha256, "size": size}]
                }
            }
        }))
        .unwrap()
    }

    fn write_v1_archive(
        tree: &Path,
        name: &str,
        manifest: &[u8],
        members: &[(&str, &[u8])],
    ) -> PathBuf {
        let archive_path = tree.join(name);
        let output = File::create(&archive_path).unwrap();
        let encoder = flate2::write::GzEncoder::new(output, flate2::Compression::default());
        let mut archive = tar::Builder::new(encoder);
        append_tar_member(&mut archive, MANIFEST_NAME, manifest);
        for (member, bytes) in members {
            append_tar_member(&mut archive, member, bytes);
        }
        archive.into_inner().unwrap().finish().unwrap();
        archive_path
    }

    fn write_v1_archive_with_traversal(tree: &Path, name: &str, manifest: &[u8]) -> PathBuf {
        let archive_path = tree.join(name);
        let output = File::create(&archive_path).unwrap();
        let mut encoder = flate2::write::GzEncoder::new(output, flate2::Compression::default());
        for (member, bytes) in [
            (MANIFEST_NAME, manifest),
            ("audio/120000_30/stream.json", b"stream" as &[u8]),
            ("../escape", b"escape" as &[u8]),
        ] {
            let mut header = [0_u8; 512];
            header[..member.len()].copy_from_slice(member.as_bytes());
            header[100..108].copy_from_slice(b"0000644\0");
            header[108..116].copy_from_slice(b"0000000\0");
            header[116..124].copy_from_slice(b"0000000\0");
            let size = format!("{:011o}\0", bytes.len());
            header[124..136].copy_from_slice(size.as_bytes());
            header[136..148].copy_from_slice(b"00000000000\0");
            header[148..156].fill(b' ');
            header[156] = b'0';
            header[257..263].copy_from_slice(b"ustar\0");
            header[263..265].copy_from_slice(b"00");
            let checksum = header.iter().map(|byte| u32::from(*byte)).sum::<u32>();
            let checksum = format!("{:06o}\0 ", checksum);
            header[148..156].copy_from_slice(checksum.as_bytes());
            encoder.write_all(&header).unwrap();
            encoder.write_all(bytes).unwrap();
            let padding = (512 - bytes.len() % 512) % 512;
            encoder.write_all(&vec![0_u8; padding]).unwrap();
        }
        encoder.write_all(&[0_u8; 1024]).unwrap();
        encoder.finish().unwrap();
        archive_path
    }

    fn append_tar_member(
        archive: &mut tar::Builder<flate2::write::GzEncoder<File>>,
        name: &str,
        bytes: &[u8],
    ) {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(EntryType::Regular);
        header.set_mode(0o644);
        header.set_size(bytes.len() as u64);
        header.set_cksum();
        archive.append_data(&mut header, name, bytes).unwrap();
    }

    fn journal_family_snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut snapshot = BTreeMap::new();
        for family in ["chronicle", "entities", "facets", "imports"] {
            let path = root.join(family);
            if !path.exists() {
                continue;
            }
            for entry in walk_files(&path) {
                let relative = entry
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                snapshot.insert(relative, fs::read(entry).unwrap());
            }
        }
        snapshot
    }

    fn walk_files(path: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                files.extend(walk_files(&entry.path()));
            } else if entry.file_type().unwrap().is_file() {
                files.push(entry.path());
            }
        }
        files.sort();
        files
    }

    fn assert_no_extraction_residue(working_root: &Path) {
        let remaining = if working_root.exists() {
            collect_tree(working_root)
        } else {
            BTreeSet::new()
        };
        assert!(
            remaining.iter().all(|path| !path.starts_with("extract-")),
            "v1 failure left extraction residue: {remaining:?}"
        );
    }

    fn list_names(path: &Path) -> BTreeSet<OsString> {
        fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect()
    }

    fn collect_tree(path: &Path) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        fn visit(root: &Path, current: &Path, names: &mut BTreeSet<String>) {
            let Ok(entries) = fs::read_dir(current) else {
                return;
            };
            for entry in entries {
                let entry = entry.unwrap();
                let relative = entry
                    .path()
                    .strip_prefix(root)
                    .expect("under root")
                    .to_string_lossy()
                    .replace('\\', "/");
                names.insert(relative);
                if entry.file_type().unwrap().is_dir() {
                    visit(root, &entry.path(), names);
                }
            }
        }
        visit(path, path, &mut names);
        names
    }

    struct PlanTree {
        path: PathBuf,
    }

    impl PlanTree {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-archive-plan-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for PlanTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
