// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable single-lock JSONL storage and owner-hold mutations for facet-scoped
//! Experiences under `facets/<facet>/experiences/<YYYYMMDD>.jsonl`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use jiff::Span;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_format::content::experiences::{
    ActivityRef, ExperienceMoment, ExperienceRecord, MembershipSource, OwnerHolds,
};
use solstone_core_format::segment::is_date_key;
use solstone_core_journal_io::durability::{ArtifactId, artifact};
use solstone_core_journal_io::{
    AtomicWriteOptions, LockError, LockOptions, PathError, ReadError, atomic_replace,
    contained_path, hold_lock, read_text,
};
use solstone_core_segment::effective_activity_stream;

use super::activity_records::{EDIT_BEFORE, EDIT_SET, load_activity_records};
use super::declaration::{
    FacetIdentityError, observe_facet_write_identity, require_observed_facet_write_identity,
};
use super::error::FacetStoreError;
use super::paths::{FacetContentKind, content_file_path, experiences_dir};
use super::publication;

/// Errors returned by Experience store reads and mutations.
#[derive(Debug)]
pub enum ExperienceStoreError {
    InvalidDay(String),
    InvalidExperienceId(String),
    UnreadableExperienceDay(PathBuf),
    EmptyActivities,
    MissingExperience {
        facet: String,
        anchor_day: String,
        id: String,
    },
    AlreadyExists {
        facet: String,
        anchor_day: String,
        id: String,
    },
    DestinationMuted {
        facet: String,
    },
    Path(PathError),
    Lock(LockError),
    Read(ReadError),
    Write(solstone_core_journal_io::AtomicWriteError),
    Json(serde_json::Error),
    Facet(FacetStoreError),
    Identity(FacetIdentityError),
    TrustLock(crate::trust_lock::FacetTrustLockError),
}

impl std::fmt::Display for ExperienceStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDay(day) => write!(f, "invalid experience day key: {day}"),
            Self::InvalidExperienceId(id) => write!(f, "invalid experience id: {id}"),
            Self::UnreadableExperienceDay(path) => {
                write!(f, "unreadable experience records in {}", path.display())
            }
            Self::EmptyActivities => {
                write!(
                    f,
                    "an experience requires at least one constituent activity"
                )
            }
            Self::MissingExperience {
                facet,
                anchor_day,
                id,
            } => write!(f, "experience not found: {facet}/{anchor_day}/{id}"),
            Self::AlreadyExists {
                facet,
                anchor_day,
                id,
            } => write!(f, "experience already exists: {facet}/{anchor_day}/{id}"),
            Self::DestinationMuted { facet } => {
                write!(f, "destination facet is muted: {facet}")
            }
            Self::Path(error) => error.fmt(f),
            Self::Lock(error) => error.fmt(f),
            Self::Read(error) => error.fmt(f),
            Self::Write(error) => error.fmt(f),
            Self::Json(error) => error.fmt(f),
            Self::Facet(error) => error.fmt(f),
            Self::Identity(error) => error.fmt(f),
            Self::TrustLock(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ExperienceStoreError {}

impl From<PathError> for ExperienceStoreError {
    fn from(error: PathError) -> Self {
        Self::Path(error)
    }
}
impl From<LockError> for ExperienceStoreError {
    fn from(error: LockError) -> Self {
        Self::Lock(error)
    }
}
impl From<ReadError> for ExperienceStoreError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}
impl From<solstone_core_journal_io::AtomicWriteError> for ExperienceStoreError {
    fn from(error: solstone_core_journal_io::AtomicWriteError) -> Self {
        Self::Write(error)
    }
}
impl From<serde_json::Error> for ExperienceStoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}
impl From<FacetStoreError> for ExperienceStoreError {
    fn from(error: FacetStoreError) -> Self {
        Self::Facet(error)
    }
}
impl From<FacetIdentityError> for ExperienceStoreError {
    fn from(error: FacetIdentityError) -> Self {
        Self::Identity(error)
    }
}
impl From<crate::trust_lock::FacetTrustLockError> for ExperienceStoreError {
    fn from(error: crate::trust_lock::FacetTrustLockError) -> Self {
        Self::TrustLock(error)
    }
}

/// Direct owner edit patch on an existing Experience.
#[derive(Debug, Clone, Default)]
pub struct ExperienceDirectPatch {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub locations: Option<Vec<String>>,
    pub moments: Option<Vec<ExperienceMoment>>,
    /// Explicit hold names to release (`"title"`, `"summary"`, `"activities"`, or `"moment:<id>"`).
    pub release_holds: Vec<String>,
    pub note: Option<String>,
}

/// Structural owner edit adding and/or removing constituent activities.
#[derive(Debug, Clone, Default)]
pub struct ExperienceActivitiesMutation {
    pub add: Vec<ActivityRef>,
    pub remove: Vec<ActivityRef>,
    pub note: Option<String>,
}

/// Output of a `experience_compose` pass applied onto a stored Experience.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ComposedExperienceUpdate {
    pub title: String,
    pub summary: String,
    pub locations: Vec<String>,
    pub entity_refs: Vec<String>,
    pub moments: Vec<ExperienceMoment>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
}

/// Result of cascading trashed segments or removed activities across stored Experiences.
#[derive(Debug, Clone, Default)]
pub struct ExperienceCascadeResult {
    /// Pre-cascade Experience snapshots saved once for trash recovery.
    pub snapshots: Vec<ExperienceRecord>,
    /// `(primary_facet, anchor_day, experience_id)` triples requiring `experience_compose`.
    pub recompose: Vec<(String, String, String)>,
    /// Journal-relative Experience day file paths modified by the cascade.
    pub changed_files: Vec<String>,
    /// Experience IDs retired cleanly because zero constituent activities remained live.
    pub retired_ids: Vec<String>,
}

/// Preserve malformed or non-Experience JSONL lines at their original positions on rewrite.
#[derive(Default)]
struct ExperienceDayLines {
    rows: Vec<ExperienceRecord>,
    kept: Vec<(usize, Vec<u8>)>,
}

fn validate_day(day: &str) -> Result<(), ExperienceStoreError> {
    if is_date_key(day) && solstone_core_time::parse_day(day).is_some() {
        Ok(())
    } else {
        Err(ExperienceStoreError::InvalidDay(day.to_owned()))
    }
}

fn validate_exp_id(id: &str) -> Result<(), ExperienceStoreError> {
    if !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        Ok(())
    } else {
        Err(ExperienceStoreError::InvalidExperienceId(id.to_owned()))
    }
}

fn experience_day_path(
    root: &Path,
    facet: &str,
    anchor_day: &str,
) -> Result<PathBuf, ExperienceStoreError> {
    validate_day(anchor_day)?;
    let _registered = artifact(ArtifactId::FacetExperiences);
    contained_path(
        root,
        &format!("facets/{facet}/experiences/{anchor_day}.jsonl"),
    )
    .map_err(Into::into)
}

fn read_day_lines_tolerant(path: &Path) -> Result<ExperienceDayLines, ExperienceStoreError> {
    let mut lines = ExperienceDayLines::default();
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(lines),
        Err(source) => {
            return Err(ReadError::Io {
                path: path.to_owned(),
                source,
            }
            .into());
        }
    };
    for (idx, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<ExperienceRecord>(line) {
            Ok(row) => lines.rows.push(row),
            Err(error) => {
                log::warn!(
                    "skipping malformed experience line {}:{} ({error})",
                    path.display(),
                    idx + 1
                );
                lines.kept.push((lines.rows.len(), line.to_vec()));
            }
        }
    }
    Ok(lines)
}

fn write_day_lines(
    path: &Path,
    rows: &[ExperienceRecord],
    kept: &[(usize, Vec<u8>)],
) -> Result<(), ExperienceStoreError> {
    let mut bytes = Vec::new();
    let mut kept_iter = kept.iter().peekable();
    for (index, row) in rows.iter().enumerate() {
        while let Some((_, raw_line)) = kept_iter.next_if(|(before, _)| *before <= index) {
            bytes.extend_from_slice(raw_line);
            bytes.push(b'\n');
        }
        serde_json::to_writer(&mut bytes, row)?;
        bytes.push(b'\n');
    }
    for (_, raw_line) in kept_iter {
        bytes.extend_from_slice(raw_line);
        bytes.push(b'\n');
    }
    atomic_replace(path, &bytes, AtomicWriteOptions { mode: Some(0o600) })?;
    Ok(())
}

fn locked_modify_experiences<T>(
    root: &Path,
    facet: &str,
    anchor_day: &str,
    create_if_missing: bool,
    allow_muted: bool,
    modify: impl FnOnce(
        Vec<ExperienceRecord>,
        &str,
    ) -> Result<(Vec<ExperienceRecord>, bool, T), ExperienceStoreError>,
) -> Result<T, ExperienceStoreError> {
    let _facet_lock = crate::hold_facet_trust_lock(root)?;
    let expected_facet_id = observe_facet_write_identity(root, facet)?;
    if !allow_muted
        && super::declaration::read_facet_declaration(root, facet)?.and_then(|decl| decl.muted)
            == Some(true)
    {
        return Err(ExperienceStoreError::DestinationMuted {
            facet: facet.to_owned(),
        });
    }
    let (result, written_path) = {
        let _entity_lock =
            solstone_core_entity::hold_entity_trust_lock(root).map_err(|error| match error {
                solstone_core_entity::EntityTrustLockError::Path(path_err) => {
                    ExperienceStoreError::Path(path_err)
                }
                solstone_core_entity::EntityTrustLockError::Lock(lock_err) => {
                    ExperienceStoreError::Lock(lock_err)
                }
            })?;
        require_observed_facet_write_identity(root, facet, &expected_facet_id)?;
        let path = experience_day_path(root, facet, anchor_day)?;
        let _file_lock = hold_lock(&path, LockOptions::default())?;
        if !path.exists() && !create_if_missing {
            return Err(ExperienceStoreError::MissingExperience {
                facet: facet.to_owned(),
                anchor_day: anchor_day.to_owned(),
                id: String::new(),
            });
        }
        let ExperienceDayLines { rows, kept } = read_day_lines_tolerant(&path)?;
        let (mut next_rows, changed, output) = modify(rows, &expected_facet_id)?;
        if changed {
            next_rows.sort_by(|a, b| a.started_at.cmp(&b.started_at).then(a.id.cmp(&b.id)));
            write_day_lines(&path, &next_rows, &kept)?;
            (output, Some(path))
        } else {
            (output, None)
        }
    };
    if let Some(path) = written_path {
        publication::publish_saved(root, &path);
    }
    Ok(result)
}

/// List valid `YYYYMMDD` day stems under `facets/<facet>/experiences/` (strict `^\d{8}\.jsonl$`).
pub fn list_experience_day_stems(
    root: &Path,
    facet: &str,
) -> Result<Vec<String>, ExperienceStoreError> {
    let dir = experiences_dir(root, facet)?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(ReadError::Io { path: dir, source }.into());
        }
    };
    let mut stems = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".jsonl") else {
            continue;
        };
        if is_date_key(stem) && solstone_core_time::parse_day(stem).is_some() {
            stems.push(stem.to_owned());
        }
    }
    stems.sort();
    Ok(stems)
}

/// Read raw text of a file under `facets/<facet>/experiences/` (used by archive merge).
pub fn read_experience_file(
    root: &Path,
    facet: &str,
    relative_path: &str,
) -> Result<Option<String>, FacetStoreError> {
    let path = content_file_path(root, facet, FacetContentKind::Experiences, relative_path)?;
    if !path.exists() {
        return Ok(None);
    }
    let content = read_text(&path, String::new())?;
    Ok(Some(content))
}

/// Write raw text of a file under `facets/<facet>/experiences/` atomically (used by archive merge).
pub fn write_experience_file(
    root: &Path,
    facet: &str,
    relative_path: &str,
    content: &str,
) -> Result<(), super::error::FacetWriteError> {
    let path = content_file_path(root, facet, FacetContentKind::Experiences, relative_path)?;
    atomic_replace(
        &path,
        content.as_bytes(),
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(super::error::FacetWriteError::ContentWrite)?;
    publication::publish_saved(root, &path);
    Ok(())
}

/// Tolerantly load Experiences anchored on `(facet, anchor_day)`.
/// Missing directories or malformed lines never fail startup.
pub fn load_experiences(
    root: &Path,
    facet: &str,
    anchor_day: &str,
    include_hidden: bool,
) -> Result<Vec<ExperienceRecord>, ExperienceStoreError> {
    let path = experience_day_path(root, facet, anchor_day)?;
    let mut rows = read_day_lines_tolerant(&path)?.rows;
    if !include_hidden {
        rows.retain(|row| !row.hidden);
    }
    rows.sort_by(|a, b| a.started_at.cmp(&b.started_at).then(a.id.cmp(&b.id)));
    Ok(rows)
}

/// Load Experiences anchored on `day` OR anchored on `day - 1` whose activities/end span into `day`.
pub fn load_experiences_overlapping_day(
    root: &Path,
    facet: &str,
    day: &str,
    include_hidden: bool,
) -> Result<Vec<ExperienceRecord>, ExperienceStoreError> {
    validate_day(day)?;
    let mut out = load_experiences(root, facet, day, include_hidden)?;
    let mut seen_ids: BTreeSet<String> = out.iter().map(|row| row.id.clone()).collect();
    if let Some(prev_day) = prev_day_key(day) {
        let prev_iso = format_iso_day(day);
        for row in load_experiences(root, facet, &prev_day, include_hidden)? {
            let touches_day = row.activities.iter().any(|act| act.day == day)
                || row.ended_at.starts_with(&prev_iso);
            if touches_day && seen_ids.insert(row.id.clone()) {
                out.push(row);
            }
        }
    }
    out.sort_by(|a, b| a.started_at.cmp(&b.started_at).then(a.id.cmp(&b.id)));
    Ok(out)
}

/// Load all Experiences in `facet` whose `anchor_day` is within `[from_day, to_day]` inclusive.
pub fn list_experiences_in_range(
    root: &Path,
    facet: &str,
    from_day: &str,
    to_day: &str,
    include_hidden: bool,
) -> Result<Vec<ExperienceRecord>, ExperienceStoreError> {
    validate_day(from_day)?;
    validate_day(to_day)?;
    let mut out = Vec::new();
    for stem in list_experience_day_stems(root, facet)? {
        if stem.as_str() >= from_day && stem.as_str() <= to_day {
            out.extend(load_experiences(root, facet, &stem, include_hidden)?);
        }
    }
    out.sort_by(|a, b| a.started_at.cmp(&b.started_at).then(a.id.cmp(&b.id)));
    Ok(out)
}

/// Look up a single Experience by `(facet, anchor_day, id)`.
pub fn get_experience(
    root: &Path,
    facet: &str,
    anchor_day: &str,
    exp_id: &str,
) -> Result<Option<ExperienceRecord>, ExperienceStoreError> {
    validate_day(anchor_day)?;
    validate_exp_id(exp_id)?;
    let rows = load_experiences(root, facet, anchor_day, true)?;
    Ok(rows.into_iter().find(|row| row.id == exp_id))
}

/// Whether an owner ask still needs this experience composed. A damaged
/// row could be the requested experience, so absence must be conclusive.
pub fn experience_needs_composition(
    root: &Path,
    facet: &str,
    anchor_day: &str,
    id: &str,
    asked_at_ms: i64,
) -> Result<bool, ExperienceStoreError> {
    validate_exp_id(id)?;
    let path = experience_day_path(root, facet, anchor_day)?;
    let day = read_day_lines_tolerant(&path)?;
    let Some(record) = day.rows.into_iter().find(|r| r.id == id) else {
        return if day.kept.is_empty() {
            Ok(false)
        } else {
            Err(ExperienceStoreError::UnreadableExperienceDay(path))
        };
    };
    Ok(!record.hidden
        && !record.activities.is_empty()
        && record
            .composed_at
            .as_deref()
            .and_then(|s| s.parse::<jiff::Timestamp>().ok())
            .is_none_or(|ts| ts.as_millisecond() < asked_at_ms))
}

/// Adopt `destination_id` and `primary_facet` on Experience rows moved into `facet` by a facet merge.
pub fn adopt_moved_experience_rows(
    root: &Path,
    facet: &str,
    anchor_day: &str,
) -> Result<usize, ExperienceStoreError> {
    let facets_set =
        solstone_core_indexer_store::classification::FacetDeclarationSet::from_journal(root).ok();
    locked_modify_experiences(
        root,
        facet,
        anchor_day,
        false,
        true,
        |mut rows, facet_id| {
            let mut adopted = 0usize;
            for row in &mut rows {
                let needs_adopt = row.destination_id.as_deref().is_some_and(|dest| {
                    dest != facet_id
                        && facets_set
                            .as_ref()
                            .is_some_and(|set| set.id_resolves_to(dest, facet_id))
                }) || row.primary_facet != facet;
                if needs_adopt {
                    row.destination_id = Some(facet_id.to_owned());
                    row.primary_facet = facet.to_owned();
                    adopted += 1;
                }
            }
            Ok((rows, adopted > 0, adopted))
        },
    )
}

/// Deterministic experience ID derived from primary facet, anchor day, and sorted constituent keys.
#[must_use]
pub fn deterministic_experience_id(
    primary_facet: &str,
    anchor_day: &str,
    activities: &[ActivityRef],
) -> String {
    let mut keys = activities
        .iter()
        .map(|act| {
            format!(
                "{}|{}|{}|{}",
                act.day,
                act.effective_facet(primary_facet),
                act.stream,
                act.id
            )
        })
        .collect::<Vec<_>>();
    keys.sort();
    keys.dedup();
    let mut hasher = Sha256::new();
    hasher.update(primary_facet.as_bytes());
    hasher.update(b"\0");
    hasher.update(anchor_day.as_bytes());
    for key in &keys {
        hasher.update(b"\0");
        hasher.update(key.as_bytes());
    }
    let digest = format!("{:x}", hasher.finalize());
    format!("exp_{anchor_day}_{}", &digest[..8])
}

/// Compute chronological `(started_at, ended_at)` across constituent activities from journal files.
pub fn compute_activities_span(
    root: &Path,
    primary_facet: &str,
    activities: &[ActivityRef],
) -> (String, String) {
    let mut min_bound: Option<(String, u32)> = None;
    let mut max_bound: Option<(String, u32)> = None;
    let mut day_cache: BTreeMap<(String, String), Vec<Map<String, Value>>> = BTreeMap::new();

    for act in activities {
        let facet = act.effective_facet(primary_facet).to_owned();
        let rows = day_cache
            .entry((facet.clone(), act.day.clone()))
            .or_insert_with(|| {
                load_activity_records(root, &facet, &act.day, true).unwrap_or_default()
            });
        let found = rows.iter().find(|row| {
            let row_id = row.get("id").and_then(Value::as_str).unwrap_or("");
            let row_stream = effective_activity_stream(root, &act.day, row).unwrap_or("");
            row_id == act.id && (act.stream.is_empty() || row_stream == act.stream)
        });
        if let Some(row) = found
            && let Some((start_s, end_s)) = activity_row_bounds_secs(row)
        {
            let start_pair = (act.day.clone(), start_s);
            let (end_day, end_mod) = if end_s >= 86_400 {
                (
                    next_day_key(&act.day).unwrap_or_else(|| act.day.clone()),
                    end_s - 86_400,
                )
            } else {
                (act.day.clone(), end_s)
            };
            let end_pair = (end_day, end_mod);
            if min_bound.as_ref().is_none_or(|cur| start_pair < *cur) {
                min_bound = Some(start_pair);
            }
            if max_bound.as_ref().is_none_or(|cur| end_pair > *cur) {
                max_bound = Some(end_pair);
            }
        }
    }

    let fallback_day = activities
        .iter()
        .map(|act| act.day.as_str())
        .min()
        .unwrap_or("19700101");
    let started_at = match min_bound {
        Some((day, sec)) => format!("{}T{}:00", format_iso_day(&day), format_hm(sec)),
        None => format!("{}T00:00:00", format_iso_day(fallback_day)),
    };
    let ended_at = match max_bound {
        Some((day, sec)) => format!("{}T{}:00", format_iso_day(&day), format_hm(sec)),
        None => started_at.clone(),
    };
    (started_at, ended_at)
}

/// Create a manual owner Experience from `1..N` activities (supports explicit cross-facet refs).
#[allow(clippy::too_many_arguments)]
pub fn create_owner_experience(
    root: &Path,
    primary_facet: &str,
    mut activities: Vec<ActivityRef>,
    title: Option<String>,
    summary: Option<String>,
    actor: &str,
    now_iso: &str,
) -> Result<ExperienceRecord, ExperienceStoreError> {
    if activities.is_empty() {
        return Err(ExperienceStoreError::EmptyActivities);
    }
    normalize_activity_refs(primary_facet, &mut activities)?;
    let anchor_day = activities
        .iter()
        .map(|act| act.day.clone())
        .min()
        .ok_or(ExperienceStoreError::EmptyActivities)?;
    let (started_at, ended_at) = compute_activities_span(root, primary_facet, &activities);
    let base_id = deterministic_experience_id(primary_facet, &anchor_day, &activities);

    let title_clean = title.map(|v| v.trim().to_owned()).unwrap_or_default();
    let summary_clean = summary.map(|v| v.trim().to_owned()).unwrap_or_default();

    locked_modify_experiences(
        root,
        primary_facet,
        &anchor_day,
        true,
        false,
        |mut rows, facet_id| {
            let mut id = base_id.clone();
            let mut suffix = 2u32;
            while rows.iter().any(|existing| existing.id == id) {
                id = format!("{base_id}_{suffix}");
                suffix += 1;
            }
            let mut owner_holds = OwnerHolds {
                activities: true,
                title: !title_clean.is_empty(),
                summary: !summary_clean.is_empty(),
                moment_ids: BTreeSet::new(),
                excluded_activities: Vec::new(),
            };
            let mut held_fields = vec!["activities"];
            if owner_holds.title {
                held_fields.push("title");
            }
            if owner_holds.summary {
                held_fields.push("summary");
            }
            let edit = json!({
                "timestamp": now_iso,
                "actor": actor,
                "fields": held_fields,
                "note": "created"
            });
            owner_holds.excluded_activities.clear();
            let record = ExperienceRecord {
                id,
                primary_facet: primary_facet.to_owned(),
                destination_id: Some(facet_id.to_owned()),
                anchor_day: anchor_day.clone(),
                started_at: started_at.clone(),
                ended_at: ended_at.clone(),
                membership_source: MembershipSource::Owner,
                hidden: false,
                owner_holds,
                edits: vec![edit],
                title: if title_clean.is_empty() {
                    "Untitled experience".to_owned()
                } else {
                    title_clean.clone()
                },
                summary: summary_clean.clone(),
                locations: Vec::new(),
                activities: activities.clone(),
                entity_refs: Vec::new(),
                moments: Vec::new(),
                category: None,
                composed_at: None,
                created_at: Some(now_iso.to_owned()),
                updated_at: Some(now_iso.to_owned()),
            };
            rows.push(record.clone());
            Ok((rows, true, record))
        },
    )
}

/// Upsert an auto-detected Experience while strictly respecting muted tombstones,
/// `owner_holds`, and `excluded_activities`.
///
/// Returns `Ok(Some(record))` when created or updated, or `Ok(None)` when suppressed by
/// an existing muted Experience or owner exclusion.
#[allow(clippy::too_many_arguments)]
pub fn upsert_detected_experience(
    root: &Path,
    primary_facet: &str,
    anchor_day: &str,
    extends_id: Option<&str>,
    title_hint: &str,
    category: Option<&str>,
    mut activities: Vec<ActivityRef>,
    started_at: &str,
    ended_at: &str,
    now_iso: &str,
) -> Result<Option<ExperienceRecord>, ExperienceStoreError> {
    if activities.is_empty() {
        return Ok(None);
    }
    normalize_activity_refs(primary_facet, &mut activities)?;
    // Enforce single-facet isolation for auto-detected Experiences.
    activities.retain(|act| act.effective_facet(primary_facet) == primary_facet);
    if activities.is_empty() {
        return Ok(None);
    }
    let candidate_id = extends_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| deterministic_experience_id(primary_facet, anchor_day, &activities));

    locked_modify_experiences(
        root,
        primary_facet,
        anchor_day,
        true,
        false,
        |mut rows, facet_id| {
            // Check whether any existing Experience (including hidden: true muted ones!)
            // already covers or explicitly excludes these activities.
            let incoming_keys: BTreeSet<(String, String, String, String)> = activities
                .iter()
                .map(|act| owned_key(primary_facet, act))
                .collect();

            let matched_idx = rows.iter().position(|existing| {
                existing.id == candidate_id
                    || existing
                        .activities
                        .iter()
                        .any(|act| incoming_keys.contains(&owned_key(&existing.primary_facet, act)))
            });

            if let Some(idx) = matched_idx {
                let existing = &mut rows[idx];
                // Muted Experience tombstones keep their activities covered so detection never resurrects them.
                if existing.hidden {
                    return Ok((rows, false, None));
                }
                if existing.owner_holds.activities {
                    return Ok((rows, false, None));
                }
                let mut added_any = false;
                for act in &activities {
                    if existing
                        .owner_holds
                        .excludes_activity(&existing.primary_facet, act)
                    {
                        continue;
                    }
                    let key = owned_key(&existing.primary_facet, act);
                    if !existing
                        .activities
                        .iter()
                        .any(|cur| owned_key(&existing.primary_facet, cur) == key)
                    {
                        existing.activities.push(act.clone());
                        added_any = true;
                    }
                }
                if !added_any {
                    return Ok((rows, false, None));
                }
                existing.activities.sort();
                if started_at < existing.started_at.as_str() {
                    existing.started_at = started_at.to_owned();
                }
                if ended_at > existing.ended_at.as_str() {
                    existing.ended_at = ended_at.to_owned();
                }
                existing.updated_at = Some(now_iso.to_owned());
                let updated = existing.clone();
                return Ok((rows, true, Some(updated)));
            }

            // Filter out any activities explicitly excluded across existing rows on this day.
            let filtered: Vec<ActivityRef> = activities
                .into_iter()
                .filter(|act| {
                    !rows.iter().any(|existing| {
                        existing
                            .owner_holds
                            .excludes_activity(&existing.primary_facet, act)
                    })
                })
                .collect();
            if filtered.is_empty() {
                return Ok((rows, false, None));
            }

            let record = ExperienceRecord {
                id: candidate_id,
                primary_facet: primary_facet.to_owned(),
                destination_id: Some(facet_id.to_owned()),
                anchor_day: anchor_day.to_owned(),
                started_at: started_at.to_owned(),
                ended_at: ended_at.to_owned(),
                membership_source: MembershipSource::Auto,
                hidden: false,
                owner_holds: OwnerHolds::default(),
                edits: Vec::new(),
                title: title_hint.trim().to_owned(),
                summary: String::new(),
                locations: Vec::new(),
                activities: filtered,
                entity_refs: Vec::new(),
                moments: Vec::new(),
                category: category.map(str::to_owned),
                composed_at: None,
                created_at: Some(now_iso.to_owned()),
                updated_at: Some(now_iso.to_owned()),
            };
            rows.push(record.clone());
            Ok((rows, true, Some(record)))
        },
    )
}

/// Apply Talent 2 (`experience_compose`) output onto a stored Experience while preserving
/// every owner-held field and every owner-held moment whose backing activities still live.
pub fn apply_composed_experience(
    root: &Path,
    facet: &str,
    anchor_day: &str,
    exp_id: &str,
    composed: ComposedExperienceUpdate,
    now_iso: &str,
) -> Result<ExperienceRecord, ExperienceStoreError> {
    validate_exp_id(exp_id)?;
    locked_modify_experiences(root, facet, anchor_day, false, true, |mut rows, _| {
        let Some(existing) = rows.iter_mut().find(|row| row.id == exp_id) else {
            return Err(ExperienceStoreError::MissingExperience {
                facet: facet.to_owned(),
                anchor_day: anchor_day.to_owned(),
                id: exp_id.to_owned(),
            });
        };
        let live_keys: BTreeSet<(String, String, String, String)> = existing
            .activities
            .iter()
            .map(|act| owned_key(&existing.primary_facet, act))
            .collect();

        if !existing.owner_holds.title && !composed.title.trim().is_empty() {
            existing.title = composed.title.trim().to_owned();
        }
        if !existing.owner_holds.summary && !composed.summary.trim().is_empty() {
            existing.summary = composed.summary.trim().to_owned();
        }
        existing.locations = composed.locations;
        existing.entity_refs = composed.entity_refs;

        // Preserve owner-held moments whose backing activities remain live (or custom moments
        // with empty activity_refs); replace all unheld moments with newly composed ones.
        let held_moments: Vec<ExperienceMoment> = existing
            .moments
            .iter()
            .filter(|moment| {
                existing.owner_holds.moment_ids.contains(&moment.id)
                    && (moment.activity_refs.is_empty()
                        || moment.activity_refs.iter().any(|act| {
                            live_keys.contains(&owned_key(&existing.primary_facet, act))
                        }))
            })
            .cloned()
            .collect();
        let held_ids: BTreeSet<String> = held_moments.iter().map(|m| m.id.clone()).collect();
        let mut merged_moments = held_moments;
        for moment in composed.moments {
            if !held_ids.contains(&moment.id) {
                merged_moments.push(moment);
            }
        }
        merged_moments.sort_by(|a, b| a.started_at.cmp(&b.started_at).then(a.id.cmp(&b.id)));
        existing.moments = merged_moments;

        if let Some(started) = composed.started_at
            && !started.is_empty()
        {
            existing.started_at = started;
        }
        if let Some(ended) = composed.ended_at
            && !ended.is_empty()
        {
            existing.ended_at = ended;
        }
        existing.composed_at = Some(now_iso.to_owned());
        existing.updated_at = Some(now_iso.to_owned());
        existing.edits.push(json!({
            "timestamp": now_iso,
            "actor": "experience_compose",
            "fields": ["title", "summary", "locations", "entity_refs", "moments"]
        }));
        let out = existing.clone();
        Ok((rows, true, out))
    })
}

/// Apply direct owner edits (`title`, `summary`, `locations`, `moments`, and/or `release_holds`),
/// stamping `owner:update` / `cli:update` and updating `owner_holds`.
pub fn update_experience_direct(
    root: &Path,
    facet: &str,
    anchor_day: &str,
    exp_id: &str,
    patch: ExperienceDirectPatch,
    actor: &str,
    now_iso: &str,
) -> Result<ExperienceRecord, ExperienceStoreError> {
    validate_exp_id(exp_id)?;
    locked_modify_experiences(root, facet, anchor_day, false, true, |mut rows, _| {
        let Some(existing) = rows.iter_mut().find(|row| row.id == exp_id) else {
            return Err(ExperienceStoreError::MissingExperience {
                facet: facet.to_owned(),
                anchor_day: anchor_day.to_owned(),
                id: exp_id.to_owned(),
            });
        };
        let mut fields = Vec::new();
        let mut before = Map::new();
        let mut set = Map::new();

        if let Some(title) = patch.title {
            let trimmed = title.trim().to_owned();
            before.insert("title".to_owned(), Value::String(existing.title.clone()));
            set.insert("title".to_owned(), Value::String(trimmed.clone()));
            existing.title = trimmed;
            existing.owner_holds.title = true;
            fields.push("title".to_owned());
        }
        if let Some(summary) = patch.summary {
            let trimmed = summary.trim().to_owned();
            before.insert(
                "summary".to_owned(),
                Value::String(existing.summary.clone()),
            );
            set.insert("summary".to_owned(), Value::String(trimmed.clone()));
            existing.summary = trimmed;
            existing.owner_holds.summary = true;
            fields.push("summary".to_owned());
        }
        if let Some(locations) = patch.locations {
            before.insert("locations".to_owned(), json!(existing.locations));
            let clean: Vec<String> = locations
                .into_iter()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
                .collect();
            set.insert("locations".to_owned(), json!(clean));
            existing.locations = clean;
            fields.push("locations".to_owned());
        }
        if let Some(new_moments) = patch.moments {
            before.insert("moments".to_owned(), json!(existing.moments));
            let prev_by_id: BTreeMap<String, ExperienceMoment> = existing
                .moments
                .iter()
                .map(|m| (m.id.clone(), m.clone()))
                .collect();
            let new_ids: BTreeSet<String> = new_moments.iter().map(|m| m.id.clone()).collect();
            // Drop holds for moments the owner deleted; stamp holds for added or edited moments.
            existing
                .owner_holds
                .moment_ids
                .retain(|mid| new_ids.contains(mid));
            for moment in &new_moments {
                let changed = prev_by_id.get(&moment.id).is_none_or(|prev| prev != moment);
                if changed {
                    existing.owner_holds.moment_ids.insert(moment.id.clone());
                }
            }
            set.insert("moments".to_owned(), json!(new_moments));
            existing.moments = new_moments;
            fields.push("moments".to_owned());
        }
        for hold in &patch.release_holds {
            match hold.as_str() {
                "title" => {
                    existing.owner_holds.title = false;
                    fields.push("release_hold:title".to_owned());
                }
                "summary" => {
                    existing.owner_holds.summary = false;
                    fields.push("release_hold:summary".to_owned());
                }
                "activities" => {
                    existing.owner_holds.activities = false;
                    fields.push("release_hold:activities".to_owned());
                }
                "moments" => {
                    existing.owner_holds.moment_ids.clear();
                    fields.push("release_hold:moments".to_owned());
                }
                other if other.starts_with("moment:") => {
                    let mid = &other["moment:".len()..];
                    existing.owner_holds.moment_ids.remove(mid);
                    fields.push(format!("release_hold:{other}"));
                }
                _ => {}
            }
        }
        if fields.is_empty() {
            let unchanged = existing.clone();
            return Ok((rows, false, unchanged));
        }
        let mut edit = Map::new();
        edit.insert("timestamp".to_owned(), Value::String(now_iso.to_owned()));
        edit.insert("actor".to_owned(), Value::String(actor.to_owned()));
        edit.insert("fields".to_owned(), json!(fields));
        if !before.is_empty() {
            edit.insert(EDIT_BEFORE.to_owned(), Value::Object(before));
        }
        if !set.is_empty() {
            edit.insert(EDIT_SET.to_owned(), Value::Object(set));
        }
        if let Some(note) = patch.note.filter(|v| !v.trim().is_empty()) {
            edit.insert("note".to_owned(), Value::String(note));
        }
        existing.edits.push(Value::Object(edit));
        existing.updated_at = Some(now_iso.to_owned());
        let out = existing.clone();
        Ok((rows, true, out))
    })
}

/// Structural owner edit adding and/or removing constituent activities (supports cross-facet add).
/// Locks `activities` membership, records removed IDs in `excluded_activities`, drops unheld
/// moments whose backing activities were all removed, and preserves held fields/moments.
pub fn mutate_experience_activities(
    root: &Path,
    facet: &str,
    anchor_day: &str,
    exp_id: &str,
    mut mutation: ExperienceActivitiesMutation,
    actor: &str,
    now_iso: &str,
) -> Result<ExperienceRecord, ExperienceStoreError> {
    validate_exp_id(exp_id)?;
    normalize_activity_refs(facet, &mut mutation.add)?;
    normalize_activity_refs(facet, &mut mutation.remove)?;

    locked_modify_experiences(root, facet, anchor_day, false, true, |mut rows, _| {
        let Some(existing) = rows.iter_mut().find(|row| row.id == exp_id) else {
            return Err(ExperienceStoreError::MissingExperience {
                facet: facet.to_owned(),
                anchor_day: anchor_day.to_owned(),
                id: exp_id.to_owned(),
            });
        };
        let pfacet = existing.primary_facet.clone();
        let before_acts = existing.activities.clone();

        let remove_keys: BTreeSet<(String, String, String, String)> = mutation
            .remove
            .iter()
            .map(|act| owned_key(&pfacet, act))
            .collect();
        let mut next_acts = existing.activities.clone();
        next_acts.retain(|act| !remove_keys.contains(&owned_key(&pfacet, act)));
        for removed in &mutation.remove {
            existing.owner_holds.add_excluded(&pfacet, removed.clone());
        }
        for added in &mutation.add {
            existing.owner_holds.clear_excluded(&pfacet, added);
            let key = owned_key(&pfacet, added);
            if !next_acts.iter().any(|cur| owned_key(&pfacet, cur) == key) {
                next_acts.push(added.clone());
            }
        }
        if next_acts.is_empty() {
            return Err(ExperienceStoreError::EmptyActivities);
        }
        next_acts.sort();

        let live_keys: BTreeSet<(String, String, String, String)> = next_acts
            .iter()
            .map(|act| owned_key(&pfacet, act))
            .collect();

        // Drop unheld moments whose backing activity_refs were ALL removed; prune removed refs.
        let held_moment_ids = existing.owner_holds.moment_ids.clone();
        existing.moments.retain_mut(|moment| {
            let had_refs = !moment.activity_refs.is_empty();
            moment
                .activity_refs
                .retain(|act| live_keys.contains(&owned_key(&pfacet, act)));
            if had_refs && moment.activity_refs.is_empty() && !held_moment_ids.contains(&moment.id)
            {
                return false;
            }
            true
        });

        existing.activities = next_acts;
        existing.membership_source = MembershipSource::Owner;
        existing.owner_holds.activities = true;
        let (started_at, ended_at) = compute_activities_span(root, &pfacet, &existing.activities);
        existing.started_at = started_at;
        existing.ended_at = ended_at;
        existing.updated_at = Some(now_iso.to_owned());

        let mut edit = Map::new();
        edit.insert("timestamp".to_owned(), Value::String(now_iso.to_owned()));
        edit.insert("actor".to_owned(), Value::String(actor.to_owned()));
        edit.insert("fields".to_owned(), json!(["activities"]));
        edit.insert(EDIT_BEFORE.to_owned(), json!({"activities": before_acts}));
        edit.insert(
            EDIT_SET.to_owned(),
            json!({"activities": existing.activities}),
        );
        if let Some(note) = mutation.note.filter(|v| !v.trim().is_empty()) {
            edit.insert("note".to_owned(), Value::String(note));
        }
        existing.edits.push(Value::Object(edit));

        let out = existing.clone();
        Ok((rows, true, out))
    })
}

/// Mute (`hidden = true`) or unmute (`hidden = false`) an Experience without hard-deleting it.
#[allow(clippy::too_many_arguments)]
pub fn set_experience_hidden(
    root: &Path,
    facet: &str,
    anchor_day: &str,
    exp_id: &str,
    hidden: bool,
    actor: &str,
    reason: Option<&str>,
    now_iso: &str,
) -> Result<ExperienceRecord, ExperienceStoreError> {
    validate_exp_id(exp_id)?;
    locked_modify_experiences(root, facet, anchor_day, false, true, |mut rows, _| {
        let Some(existing) = rows.iter_mut().find(|row| row.id == exp_id) else {
            return Err(ExperienceStoreError::MissingExperience {
                facet: facet.to_owned(),
                anchor_day: anchor_day.to_owned(),
                id: exp_id.to_owned(),
            });
        };
        let was_hidden = existing.hidden;
        existing.hidden = hidden;
        existing.updated_at = Some(now_iso.to_owned());
        let mut edit = Map::new();
        edit.insert("timestamp".to_owned(), Value::String(now_iso.to_owned()));
        edit.insert("actor".to_owned(), Value::String(actor.to_owned()));
        edit.insert("fields".to_owned(), json!(["hidden"]));
        edit.insert(EDIT_BEFORE.to_owned(), json!({"hidden": was_hidden}));
        edit.insert(EDIT_SET.to_owned(), json!({"hidden": hidden}));
        if let Some(reason) = reason.filter(|v| !v.trim().is_empty()) {
            edit.insert("note".to_owned(), Value::String(reason.to_owned()));
        }
        existing.edits.push(Value::Object(edit));
        let out = existing.clone();
        Ok((rows, true, out))
    })
}

/// Cascade trashed segments and/or retired activities across all facets' Experiences.
///
/// - Prunes dead `segment_refs`, `media_refs`, and trashed/missing `activities`.
/// - If `>= 1` constituent activity remains live, strips unheld derived prose/moments
///   touching the removed span and returns coordinates for `OwnerAsk` re-composition.
/// - If `0` constituent activities remain live, retires the Experience row cleanly.
pub fn cascade_experiences_for_removed_segments(
    root: &Path,
    removed_segments: &[(String, String, String)],
    removed_activities: &[(String, String, String, String)],
    now_iso: &str,
) -> Result<ExperienceCascadeResult, ExperienceStoreError> {
    let mut result = ExperienceCascadeResult::default();
    if removed_segments.is_empty() && removed_activities.is_empty() {
        return Ok(result);
    }

    let seg_keys: BTreeSet<String> = removed_segments
        .iter()
        .map(|(day, stream, seg)| format!("{day}/{stream}/{seg}"))
        .collect();
    let seg_days: BTreeSet<String> = removed_segments
        .iter()
        .map(|(day, _, _)| day.clone())
        .chain(removed_activities.iter().map(|(day, _, _, _)| day.clone()))
        .collect();
    let mut candidate_anchor_days = seg_days.clone();
    for day in &seg_days {
        if let Some(prev) = prev_day_key(day) {
            candidate_anchor_days.insert(prev);
        }
    }
    let removed_act_set: BTreeSet<(String, String, String, String)> =
        removed_activities.iter().cloned().collect();

    let facets = crate::list_declared_facet_names(root)?;
    for facet in facets {
        let stems = list_experience_day_stems(root, &facet)?;
        for anchor_day in stems {
            if !candidate_anchor_days.contains(&anchor_day) {
                continue;
            }
            let current = load_experiences(root, &facet, &anchor_day, true)?;
            if current.is_empty() {
                continue;
            }
            // Check quickly whether any row touches the removed segments/activities.
            let touches = current
                .iter()
                .any(|exp| experience_touches_removed(root, exp, &seg_keys, &removed_act_set));
            if !touches {
                continue;
            }

            let (snaps, recomposes, retired, changed) =
                locked_modify_experiences(root, &facet, &anchor_day, false, true, |rows, _| {
                    let mut next = Vec::with_capacity(rows.len());
                    let mut snaps = Vec::new();
                    let mut recomposes = Vec::new();
                    let mut retired = Vec::new();
                    let mut any_changed = false;

                    for mut exp in rows {
                        if !experience_touches_removed(root, &exp, &seg_keys, &removed_act_set) {
                            next.push(exp);
                            continue;
                        }
                        snaps.push(exp.clone());
                        any_changed = true;
                        let pfacet = exp.primary_facet.clone();

                        // Prune activities that were explicitly removed or are no longer live on disk.
                        exp.activities.retain(|act| {
                            let key = owned_key(&pfacet, act);
                            if removed_act_set.contains(&key) {
                                return false;
                            }
                            activity_still_live(root, &pfacet, act)
                        });

                        if exp.activities.is_empty() {
                            retired.push(exp.id.clone());
                            continue;
                        }

                        let live_keys: BTreeSet<(String, String, String, String)> = exp
                            .activities
                            .iter()
                            .map(|act| owned_key(&pfacet, act))
                            .collect();
                        let held_moments = exp.owner_holds.moment_ids.clone();

                        exp.moments.retain_mut(|moment| {
                            let touched_seg = moment
                                .segment_refs
                                .iter()
                                .any(|sref| seg_keys.contains(sref))
                                || moment.media_refs.iter().any(|mref| seg_keys.contains(mref));
                            moment.segment_refs.retain(|sref| !seg_keys.contains(sref));
                            moment.media_refs.retain(|mref| !seg_keys.contains(mref));
                            let had_act_refs = !moment.activity_refs.is_empty();
                            moment
                                .activity_refs
                                .retain(|act| live_keys.contains(&owned_key(&pfacet, act)));
                            let lost_backing = had_act_refs && moment.activity_refs.is_empty();
                            if (touched_seg || lost_backing) && !held_moments.contains(&moment.id) {
                                return false;
                            }
                            true
                        });

                        // Strip unheld derived prose so removed recording details never linger
                        // while experience_compose rebuilds via OwnerAsk.
                        if !exp.owner_holds.summary {
                            exp.summary.clear();
                        }
                        let (started_at, ended_at) =
                            compute_activities_span(root, &pfacet, &exp.activities);
                        exp.started_at = started_at;
                        exp.ended_at = ended_at;
                        exp.composed_at = None;
                        exp.updated_at = Some(now_iso.to_owned());
                        recomposes.push((pfacet, exp.anchor_day.clone(), exp.id.clone()));
                        next.push(exp);
                    }
                    Ok((next, any_changed, (snaps, recomposes, retired, any_changed)))
                })?;
            if changed {
                result.snapshots.extend(snaps);
                result.recompose.extend(recomposes);
                result.retired_ids.extend(retired);
                result
                    .changed_files
                    .push(format!("facets/{facet}/experiences/{anchor_day}.jsonl"));
            }
        }
    }
    Ok(result)
}

/// `(primary_facet, anchor_day, experience_id)` identifier tuple.
pub type ExperienceTriple = (String, String, String);

/// Restore snapshotted Experiences when a trashed recording is recovered.
pub fn restore_experiences_snapshot(
    root: &Path,
    snapshots: &[ExperienceRecord],
) -> Result<(Vec<ExperienceTriple>, Vec<String>), ExperienceStoreError> {
    let mut by_day: BTreeMap<(String, String), Vec<ExperienceRecord>> = BTreeMap::new();
    for snap in snapshots {
        by_day
            .entry((snap.primary_facet.clone(), snap.anchor_day.clone()))
            .or_default()
            .push(snap.clone());
    }
    let mut recompose = Vec::new();
    let mut changed_files = Vec::new();
    for ((facet, anchor_day), group) in by_day {
        locked_modify_experiences(root, &facet, &anchor_day, true, true, |mut rows, _| {
            for snap in &group {
                if let Some(existing) = rows.iter_mut().find(|row| row.id == snap.id) {
                    *existing = snap.clone();
                } else {
                    rows.push(snap.clone());
                }
            }
            Ok((rows, true, ()))
        })?;
        for snap in &group {
            recompose.push((facet.clone(), anchor_day.clone(), snap.id.clone()));
        }
        changed_files.push(format!("facets/{facet}/experiences/{anchor_day}.jsonl"));
    }
    Ok((recompose, changed_files))
}

/// On trash erase, scrub `before` edit values recorded at or before `before_iso`.
pub fn scrub_experience_edit_befores(
    root: &Path,
    snapshots: &[ExperienceRecord],
    before_iso: &str,
) -> Result<Vec<String>, ExperienceStoreError> {
    let mut by_day: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    for snap in snapshots {
        by_day
            .entry((snap.primary_facet.clone(), snap.anchor_day.clone()))
            .or_default()
            .insert(snap.id.clone());
    }
    let mut changed_files = Vec::new();
    for ((facet, anchor_day), ids) in by_day {
        let changed =
            locked_modify_experiences(root, &facet, &anchor_day, false, true, |mut rows, _| {
                let mut modified = false;
                for row in rows.iter_mut().filter(|row| ids.contains(&row.id)) {
                    for edit in &mut row.edits {
                        if let Some(obj) = edit.as_object_mut() {
                            let ts = obj.get("timestamp").and_then(Value::as_str).unwrap_or("");
                            if !ts.is_empty()
                                && ts <= before_iso
                                && obj.remove(EDIT_BEFORE).is_some()
                            {
                                modified = true;
                            }
                        }
                    }
                }
                Ok((rows, modified, modified))
            })
            .unwrap_or(false);
        if changed {
            changed_files.push(format!("facets/{facet}/experiences/{anchor_day}.jsonl"));
        }
    }
    Ok(changed_files)
}

fn experience_touches_removed(
    root: &Path,
    exp: &ExperienceRecord,
    seg_keys: &BTreeSet<String>,
    removed_act_set: &BTreeSet<(String, String, String, String)>,
) -> bool {
    if exp.activities.iter().any(|act| {
        removed_act_set.contains(&owned_key(&exp.primary_facet, act))
            || !activity_still_live(root, &exp.primary_facet, act)
    }) {
        return true;
    }
    exp.moments.iter().any(|moment| {
        moment
            .segment_refs
            .iter()
            .any(|sref| seg_keys.contains(sref))
            || moment.media_refs.iter().any(|mref| seg_keys.contains(mref))
    })
}

fn activity_still_live(root: &Path, primary_facet: &str, act: &ActivityRef) -> bool {
    let facet = act.effective_facet(primary_facet);
    let rows = match load_activity_records(root, facet, &act.day, false) {
        Ok(rows) => rows,
        Err(_) => return false,
    };
    rows.iter().any(|row| {
        let row_id = row.get("id").and_then(Value::as_str).unwrap_or("");
        if row_id != act.id {
            return false;
        }
        let row_stream = effective_activity_stream(root, &act.day, row).unwrap_or("");
        if !act.stream.is_empty() && row_stream != act.stream {
            return false;
        }
        let segs = row
            .get("segments")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        segs > 0
    })
}

fn owned_key(primary_facet: &str, act: &ActivityRef) -> (String, String, String, String) {
    (
        act.day.clone(),
        act.effective_facet(primary_facet).to_owned(),
        act.stream.clone(),
        act.id.clone(),
    )
}

fn normalize_activity_refs(
    primary_facet: &str,
    refs: &mut Vec<ActivityRef>,
) -> Result<(), ExperienceStoreError> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(refs.len());
    for mut item in std::mem::take(refs) {
        validate_day(&item.day)?;
        item.id = item.id.trim().to_owned();
        item.stream = item.stream.trim().to_owned();
        if item.id.is_empty() {
            return Err(ExperienceStoreError::InvalidExperienceId(
                "empty activity id".to_owned(),
            ));
        }
        if item
            .facet
            .as_deref()
            .is_some_and(|f| f.trim().is_empty() || f.trim() == primary_facet)
        {
            item.facet = None;
        } else if let Some(f) = item.facet.as_mut() {
            *f = f.trim().to_owned();
        }
        let key = owned_key(primary_facet, &item);
        if seen.insert(key) {
            out.push(item);
        }
    }
    out.sort();
    *refs = out;
    Ok(())
}

pub(crate) fn parse_seg_bounds_secs(seg: &str) -> Option<(u32, u32)> {
    let (hms, dur_str) = seg.split_once('_')?;
    if hms.len() < 6 {
        return None;
    }
    let hh: u32 = hms.get(0..2)?.parse().ok()?;
    let mm: u32 = hms.get(2..4)?.parse().ok()?;
    let ss: u32 = hms.get(4..6)?.parse().ok()?;
    let dur: u32 = dur_str.split('_').next()?.parse().ok()?;
    if hh >= 24 || mm >= 60 || ss >= 60 {
        return None;
    }
    let start = hh * 3600 + mm * 60 + ss;
    Some((start, start.saturating_add(dur)))
}

pub(crate) fn activity_row_bounds_secs(row: &Map<String, Value>) -> Option<(u32, u32)> {
    let segs = row.get("segments").and_then(Value::as_array)?;
    let bounds: Vec<(u32, u32)> = segs
        .iter()
        .filter_map(Value::as_str)
        .filter_map(parse_seg_bounds_secs)
        .collect();
    if bounds.is_empty() {
        return None;
    }
    let has_late = bounds.iter().any(|(s, _)| *s >= 20 * 3600);
    let has_early = bounds.iter().any(|(s, _)| *s < 4 * 3600);
    if has_late && has_early {
        let min_s = bounds
            .iter()
            .map(|(s, _)| if *s < 4 * 3600 { *s + 86_400 } else { *s })
            .min()?;
        let max_e = bounds
            .iter()
            .map(|(s, e)| if *s < 4 * 3600 { *e + 86_400 } else { *e })
            .max()?;
        Some((min_s, max_e))
    } else {
        let min_s = bounds.iter().map(|(s, _)| *s).min()?;
        let max_e = bounds.iter().map(|(_, e)| *e).max()?;
        Some((min_s, max_e))
    }
}

pub(crate) fn format_iso_day(day: &str) -> String {
    if day.len() == 8 {
        format!("{}-{}-{}", &day[0..4], &day[4..6], &day[6..8])
    } else {
        day.to_owned()
    }
}

pub(crate) fn format_hm(sec: u32) -> String {
    let sec_mod = sec % 86_400;
    let hh = sec_mod / 3600;
    let mm = (sec_mod % 3600) / 60;
    format!("{hh:02}:{mm:02}")
}

pub(crate) fn prev_day_key(day: &str) -> Option<String> {
    let date = solstone_core_time::parse_day(day)?;
    let prev = date.checked_sub(Span::new().days(1)).ok()?;
    Some(solstone_core_time::day_key(prev))
}

pub(crate) fn next_day_key(day: &str) -> Option<String> {
    let date = solstone_core_time::parse_day(day)?;
    let next = date.checked_add(Span::new().days(1)).ok()?;
    Some(solstone_core_time::day_key(next))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::create_facet;
    use tempfile::TempDir;

    fn test_journal() -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        create_facet(
            dir.path(),
            "personal",
            "Personal",
            "Personal facet",
            "#6366f1",
            "✨",
            None,
        )
        .expect("personal facet");
        create_facet(
            dir.path(),
            "work",
            "Work",
            "Work facet",
            "#0ea5e9",
            "💼",
            None,
        )
        .expect("work facet");
        dir
    }

    #[test]
    fn owner_create_edit_holds_cross_facet_and_mute_prevent_resurrection() {
        let dir = test_journal();
        let a1 = ActivityRef {
            day: "20260304".to_owned(),
            stream: "watch".to_owned(),
            id: "social_180000_300".to_owned(),
            facet: None,
        };
        let a2_cross = ActivityRef {
            day: "20260304".to_owned(),
            stream: "notes".to_owned(),
            id: "work_190000_300".to_owned(),
            facet: Some("work".to_owned()),
        };

        let created = create_owner_experience(
            dir.path(),
            "personal",
            vec![a1.clone()],
            Some("Dinner downtown".to_owned()),
            None,
            "owner:create",
            "2026-03-04T21:00:00Z",
        )
        .expect("create");
        assert_eq!(created.membership_source, MembershipSource::Owner);
        assert!(created.owner_holds.activities);
        assert!(created.owner_holds.title);
        assert!(!created.owner_holds.summary);

        // Add explicit cross-facet activity.
        let with_cross = mutate_experience_activities(
            dir.path(),
            "personal",
            "20260304",
            &created.id,
            ExperienceActivitiesMutation {
                add: vec![a2_cross.clone()],
                remove: vec![],
                note: Some("add cross-facet".to_owned()),
            },
            "owner:activities",
            "2026-03-04T21:05:00Z",
        )
        .expect("cross-facet add");
        assert_eq!(with_cross.activities.len(), 2);

        // Direct moment edit stamps hold on m1.
        let edited = update_experience_direct(
            dir.path(),
            "personal",
            "20260304",
            &created.id,
            ExperienceDirectPatch {
                moments: Some(vec![ExperienceMoment {
                    id: "m1".to_owned(),
                    started_at: "2026-03-04 18:00".to_owned(),
                    ended_at: "2026-03-04 19:00".to_owned(),
                    summary: "Owner edited moment.".to_owned(),
                    activity_refs: vec![a1.clone()],
                    segment_refs: vec![],
                    media_refs: vec![],
                }]),
                ..Default::default()
            },
            "owner:update",
            "2026-03-04T21:06:00Z",
        )
        .expect("direct edit");
        assert!(edited.owner_holds.moment_ids.contains("m1"));

        // Re-composition must preserve owner-held title and held moment m1 while writing unheld summary + m2.
        let composed = apply_composed_experience(
            dir.path(),
            "personal",
            "20260304",
            &created.id,
            ComposedExperienceUpdate {
                title: "Model replacement title".to_owned(),
                summary: "Alex had dinner and took notes.".to_owned(),
                locations: vec!["Downtown".to_owned()],
                entity_refs: vec!["maya".to_owned()],
                moments: vec![
                    ExperienceMoment {
                        id: "m1".to_owned(),
                        started_at: "2026-03-04 18:00".to_owned(),
                        ended_at: "2026-03-04 19:00".to_owned(),
                        summary: "Model replacement m1".to_owned(),
                        activity_refs: vec![a1.clone()],
                        segment_refs: vec![],
                        media_refs: vec![],
                    },
                    ExperienceMoment {
                        id: "m2".to_owned(),
                        started_at: "2026-03-04 19:00".to_owned(),
                        ended_at: "2026-03-04 19:30".to_owned(),
                        summary: "Model m2.".to_owned(),
                        activity_refs: vec![a2_cross.clone()],
                        segment_refs: vec![],
                        media_refs: vec![],
                    },
                ],
                started_at: None,
                ended_at: None,
            },
            "2026-03-04T21:07:00Z",
        )
        .expect("compose");
        assert_eq!(composed.title, "Dinner downtown");
        assert_eq!(composed.summary, "Alex had dinner and took notes.");
        assert_eq!(composed.moments.len(), 2);
        assert_eq!(composed.moments[0].summary, "Owner edited moment.");
        assert_eq!(composed.moments[1].summary, "Model m2.");

        // Removing a2_cross drops unheld m2 and tombstones a2_cross in excluded_activities.
        let after_remove = mutate_experience_activities(
            dir.path(),
            "personal",
            "20260304",
            &created.id,
            ExperienceActivitiesMutation {
                add: vec![],
                remove: vec![a2_cross.clone()],
                note: None,
            },
            "owner:activities",
            "2026-03-04T21:08:00Z",
        )
        .expect("remove");
        assert_eq!(after_remove.activities.len(), 1);
        assert_eq!(after_remove.moments.len(), 1);
        assert!(
            after_remove
                .owner_holds
                .excludes_activity("personal", &a2_cross)
        );

        // Muting hides from default loads and prevents auto-detection from resurrecting it.
        let muted = set_experience_hidden(
            dir.path(),
            "personal",
            "20260304",
            &created.id,
            true,
            "owner:mute",
            Some("not worth keeping"),
            "2026-03-04T21:09:00Z",
        )
        .expect("mute");
        assert!(muted.hidden);
        assert!(
            load_experiences(dir.path(), "personal", "20260304", false)
                .expect("load visible")
                .is_empty()
        );
        assert_eq!(
            load_experiences(dir.path(), "personal", "20260304", true)
                .expect("load hidden")
                .len(),
            1
        );
        let resurrected = upsert_detected_experience(
            dir.path(),
            "personal",
            "20260304",
            None,
            "Redetected dinner",
            Some("social_gathering"),
            vec![a1],
            "2026-03-04T18:00:00",
            "2026-03-04T19:00:00",
            "2026-03-04T22:00:00Z",
        )
        .expect("upsert");
        assert!(resurrected.is_none());
    }
}
