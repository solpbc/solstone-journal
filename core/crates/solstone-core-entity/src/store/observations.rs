// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use caseless::default_case_fold_str;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use solstone_core_entity_matching::{entity_slug, normalize_resolution_query};
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, DirEntryKind, LockError, PathError, ReadError,
    contained_path, list_dir_entries, path_lexists, read_text, write_text,
};

use super::error::EntityStoreError;
use super::identity::read_entity_identity;
use super::map::read_identity_map;
use crate::trust_lock::{FacetTrustLockError, hold_facet_trust_lock};

const OBSERVATION_RETRY_ATTEMPTS: usize = 3;

/// Source of observation text being parsed.
#[derive(Debug, Clone, Copy)]
pub enum ObservationParseSource<'a> {
    Path(&'a Path),
    CapturedSnapshot,
}

/// Error source for parse error formatting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservationErrorSource {
    Path(PathBuf),
    CapturedSnapshot,
}

impl fmt::Display for ObservationErrorSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(path) => write!(formatter, "{}", path.display()),
            Self::CapturedSnapshot => write!(formatter, "captured snapshot"),
        }
    }
}

/// Failure while reading or inspecting observation state.
#[derive(Debug)]
pub enum ObservationStoreError {
    Read(ReadError),
    Path(PathError),
    MalformedObservation {
        source: ObservationErrorSource,
        line: usize,
        reason: &'static str,
    },
    CorruptObservation {
        source: ObservationErrorSource,
        detail: String,
    },
    Other(String),
}

impl fmt::Display for ObservationStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => error.fmt(formatter),
            Self::Path(error) => error.fmt(formatter),
            Self::MalformedObservation {
                source,
                line,
                reason,
            } => {
                write!(
                    formatter,
                    "malformed observation in {source} at line {line}: {reason}"
                )
            }
            Self::CorruptObservation { source, detail } => {
                write!(formatter, "corrupt observation in {source}: {detail}")
            }
            Self::Other(detail) => write!(formatter, "{detail}"),
        }
    }
}

impl Error for ObservationStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            Self::Path(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ReadError> for ObservationStoreError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

impl From<PathError> for ObservationStoreError {
    fn from(error: PathError) -> Self {
        Self::Path(error)
    }
}

/// Failure while performing observation mutations.
#[derive(Debug)]
pub enum ObservationWriteError {
    TrustLock(FacetTrustLockError),
    Read(ObservationStoreError),
    Write(AtomicWriteError),
    Resolve(String),
    EmptyContent,
    Conflict { message: String },
}

impl fmt::Display for ObservationWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TrustLock(error) => error.fmt(formatter),
            Self::Read(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
            Self::Resolve(message) => write!(formatter, "{message}"),
            Self::EmptyContent => write!(formatter, "Observation content cannot be blank"),
            Self::Conflict { message } => write!(formatter, "{message}"),
        }
    }
}

impl Error for ObservationWriteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::TrustLock(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::Write(error) => Some(error),
            _ => None,
        }
    }
}

impl From<FacetTrustLockError> for ObservationWriteError {
    fn from(error: FacetTrustLockError) -> Self {
        Self::TrustLock(error)
    }
}

impl From<ObservationStoreError> for ObservationWriteError {
    fn from(error: ObservationStoreError) -> Self {
        Self::Read(error)
    }
}

impl From<AtomicWriteError> for ObservationWriteError {
    fn from(error: AtomicWriteError) -> Self {
        Self::Write(error)
    }
}

impl From<PathError> for ObservationWriteError {
    fn from(error: PathError) -> Self {
        Self::Read(ObservationStoreError::Path(error))
    }
}

impl ObservationWriteError {
    pub fn is_lock_timeout(&self) -> bool {
        matches!(
            self,
            Self::TrustLock(FacetTrustLockError::Lock(LockError::Timeout(_)))
        )
    }

    pub fn is_retryable_io(&self) -> bool {
        matches!(
            self,
            Self::TrustLock(FacetTrustLockError::Lock(LockError::Io { .. }))
                | Self::TrustLock(FacetTrustLockError::Path(PathError::Io { .. }))
                | Self::Read(ObservationStoreError::Read(ReadError::Io { .. }))
                | Self::Read(ObservationStoreError::Path(PathError::Io { .. }))
                | Self::Write(AtomicWriteError::Io { .. })
        )
    }
}

/// Failure while looking up observations.
#[derive(Debug)]
pub enum ObservationLookupError {
    Store(ObservationStoreError),
    Resolve(String),
}

impl fmt::Display for ObservationLookupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Resolve(message) => write!(formatter, "{message}"),
        }
    }
}

impl Error for ObservationLookupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ObservationStoreError> for ObservationLookupError {
    fn from(error: ObservationStoreError) -> Self {
        Self::Store(error)
    }
}

impl From<ReadError> for ObservationLookupError {
    fn from(error: ReadError) -> Self {
        Self::Store(ObservationStoreError::Read(error))
    }
}

impl From<PathError> for ObservationLookupError {
    fn from(error: PathError) -> Self {
        Self::Store(ObservationStoreError::Path(error))
    }
}

impl From<EntityStoreError> for ObservationLookupError {
    fn from(error: EntityStoreError) -> Self {
        match error {
            EntityStoreError::Read(r) => Self::Store(ObservationStoreError::Read(r)),
            EntityStoreError::Path(p) => Self::Store(ObservationStoreError::Path(p)),
            other => Self::Resolve(other.to_string()),
        }
    }
}

/// History entry recording a prior state of an observation before an update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub content: String,
    pub observed_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_day: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}

/// Retirement metadata for dropped or deduplicated observations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retired {
    pub at: i64,
    pub by: String,
}

/// One on-disk observation row with provenance, revision history, and lifecycle status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationRow {
    pub id: u64,
    pub content: String,
    pub observed_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_day: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<HistoryEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired: Option<Retired>,
    #[serde(skip)]
    pub raw_json: Option<Map<String, Value>>,
}

/// An incoming observation row for batch append.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingObservationRow {
    pub content: String,
    pub observed_at: i64,
    pub source_day: Option<String>,
    pub relation: Option<Value>,
}

/// Parsed observations container preserving full row ordering and providing live views.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedObservations {
    pub full_rows: Vec<ObservationRow>,
}

impl ParsedObservations {
    pub fn live_rows(&self) -> impl Iterator<Item = &ObservationRow> {
        self.full_rows.iter().filter(|r| r.retired.is_none())
    }

    pub fn live_count(&self) -> usize {
        self.live_rows().count()
    }
}

/// Direction for ordering live observation queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationReadOrder {
    Newest,
    Oldest,
}

/// Query parameters for owner observation reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationReadQuery {
    pub order: ObservationReadOrder,
    pub limit: usize,
    pub offset: Option<usize>,
    pub after_id: Option<u64>,
}

impl Default for ObservationReadQuery {
    fn default() -> Self {
        Self {
            order: ObservationReadOrder::Newest,
            limit: 50,
            offset: None,
            after_id: None,
        }
    }
}

/// A public page item returned by owner observation reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationPageItem {
    pub id: u64,
    pub content: String,
    pub observed_at: i64,
    pub source_day: Option<String>,
    pub relation: Option<Value>,
    pub by: Option<String>,
}

/// A paginated collection of live observations with metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationPage {
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
    pub items: Vec<ObservationPageItem>,
}

/// Summary metrics for an entity's live observations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationSummary {
    pub count: u64,
    pub latest_observed_at: Option<i64>,
}

/// Closed set of mutation operations against an entity's observation store.
#[derive(Debug, Clone, PartialEq)]
pub enum ObservationChange {
    Append {
        content: String,
        source_day: Option<String>,
        relation: Option<Value>,
        actor: &'static str,
    },
    AppendMany {
        rows: Vec<IncomingObservationRow>,
        actor: &'static str,
    },
    EditInPlace {
        full_set_index: usize,
        rewrite: Value,
    },
    ApplyOps {
        ops: Vec<Value>,
        source_day: Option<String>,
    },
    ReplaceFullSet {
        rows: Vec<ObservationRow>,
    },
    RemoveFile,
}

/// Outcome of applying an observation change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservationWriteOutcome {
    Appended { id: u64, count: usize },
    AlreadyPresent { count: usize },
    AppendedMany { count: usize },
    Edited,
    OpsApplied { counts: ObservationOperationCounts },
    Replaced,
    Removed,
    NoOp,
}

/// Result of resolving a name-or-id observation query to a relationship directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservationEntityResolution {
    Resolved { entity_dir: String },
    NoSuchEntity,
}

/// Result of looking up observations through a name-or-id query.
#[derive(Debug, Clone, PartialEq)]
pub enum ObservationLookup {
    Unresolvable,
    Resolved {
        entity_dir: String,
        observations: Vec<Value>,
    },
}

/// Counts returned after applying observation operations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationOperationCounts {
    pub update: usize,
    pub replace: usize,
    pub add: usize,
    pub drop: usize,
    pub keep: usize,
    pub skip: usize,
    pub skipped: usize,
    pub refused: usize,
}

/// Prepared two-phase commit batch for entity observations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedObservationBatch {
    pub facet: String,
    pub facet_id: String,
    pub entity_id: String,
    pub relationship: Value,
    pub entity_dir: String,
    pub before: Option<String>,
    pub after: String,
    pub counts: ObservationOperationCounts,
}

/// Normalize observation content for duplicate comparison.
/// Case-folds, collapses internal whitespace, and strips trailing '.' and '!'.
pub fn normalize_observation_content(content: &str) -> String {
    let folded = default_case_fold_str(content.trim());
    let trimmed = folded.trim_end_matches(['.', '!']);
    trimmed.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Strictly parse JSONL observation records, deriving missing IDs and projecting §6 deduplication collapse.
pub fn parse_observation_file(
    text: &str,
    source: ObservationParseSource<'_>,
) -> Result<ParsedObservations, ObservationStoreError> {
    let source_err = match source {
        ObservationParseSource::Path(path) => ObservationErrorSource::Path(path.to_path_buf()),
        ObservationParseSource::CapturedSnapshot => ObservationErrorSource::CapturedSnapshot,
    };

    let mut raw_rows = Vec::new();

    for (index, line) in text.lines().enumerate() {
        let line_num = index + 1;
        let parsed: Value = serde_json::from_str(line).map_err(|_| {
            ObservationStoreError::MalformedObservation {
                source: source_err.clone(),
                line: line_num,
                reason: "invalid JSON",
            }
        })?;
        let obj =
            parsed
                .as_object()
                .ok_or_else(|| ObservationStoreError::MalformedObservation {
                    source: source_err.clone(),
                    line: line_num,
                    reason: "expected an object with nonblank string content",
                })?;

        let content = obj
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned();
        let relation = obj.get("relation").filter(|v| !v.is_null()).cloned();
        if content.is_empty() && relation.is_none() {
            return Err(ObservationStoreError::MalformedObservation {
                source: source_err.clone(),
                line: line_num,
                reason: "expected an object with nonblank string content or relation",
            });
        }

        let observed_at = match obj.get("observed_at") {
            Some(Value::Number(n)) => n
                .as_i64()
                .or_else(|| n.as_f64().map(|f| f as i64))
                .unwrap_or(0),
            Some(Value::String(s)) => s
                .parse::<i64>()
                .ok()
                .or_else(|| {
                    chrono::NaiveDate::parse_from_str(s, "%Y%m%d")
                        .ok()
                        .and_then(|d| d.and_hms_opt(0, 0, 0))
                        .map(|dt| dt.and_utc().timestamp())
                })
                .or_else(|| {
                    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                        .ok()
                        .and_then(|d| d.and_hms_opt(0, 0, 0))
                        .map(|dt| dt.and_utc().timestamp())
                })
                .or_else(|| {
                    chrono::DateTime::parse_from_rfc3339(s)
                        .ok()
                        .map(|dt| dt.timestamp())
                })
                .unwrap_or(0),
            _ => {
                if let Some(source_day) = obj.get("source_day").and_then(Value::as_str) {
                    chrono::NaiveDate::parse_from_str(source_day, "%Y%m%d")
                        .ok()
                        .and_then(|d| d.and_hms_opt(0, 0, 0))
                        .map(|dt| dt.and_utc().timestamp())
                        .or_else(|| {
                            chrono::NaiveDate::parse_from_str(source_day, "%Y-%m-%d")
                                .ok()
                                .and_then(|d| d.and_hms_opt(0, 0, 0))
                                .map(|dt| dt.and_utc().timestamp())
                        })
                        .unwrap_or(0)
                } else {
                    0
                }
            }
        };

        let explicit_id = match obj.get("id") {
            Some(Value::Number(n)) => n.as_u64(),
            Some(_) => {
                return Err(ObservationStoreError::MalformedObservation {
                    source: source_err.clone(),
                    line: line_num,
                    reason: "id must be an unsigned integer",
                });
            }
            None => None,
        };

        let source_day = obj
            .get("source_day")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let by = obj.get("by").and_then(Value::as_str).map(str::to_owned);

        let history: Vec<HistoryEntry> = if let Some(h) = obj.get("history") {
            serde_json::from_value(h.clone()).map_err(|_| {
                ObservationStoreError::MalformedObservation {
                    source: source_err.clone(),
                    line: line_num,
                    reason: "invalid history field",
                }
            })?
        } else {
            Vec::new()
        };

        let retired: Option<Retired> = if let Some(r) = obj.get("retired").filter(|v| !v.is_null())
        {
            Some(serde_json::from_value(r.clone()).map_err(|_| {
                ObservationStoreError::MalformedObservation {
                    source: source_err.clone(),
                    line: line_num,
                    reason: "invalid retired field",
                }
            })?)
        } else {
            None
        };

        raw_rows.push((
            explicit_id,
            content,
            observed_at,
            source_day,
            relation,
            by,
            history,
            retired,
            obj.clone(),
        ));
    }

    let max_explicit_id = raw_rows.iter().filter_map(|(id, ..)| *id).max();
    let mut next_derived_id = max_explicit_id.unwrap_or(0) + 1;

    let mut full_rows = Vec::with_capacity(raw_rows.len());
    for (explicit_id, content, observed_at, source_day, relation, by, history, retired, raw_json) in
        raw_rows
    {
        let id = match explicit_id {
            Some(id) => id,
            None => {
                let derived = next_derived_id;
                next_derived_id += 1;
                derived
            }
        };
        full_rows.push(ObservationRow {
            id,
            content,
            observed_at,
            source_day,
            relation,
            by,
            history,
            retired,
            raw_json: Some(raw_json),
        });
    }

    // §6 Historical deduplication projection:
    // If multiple unretired rows have identical normalized content AND source_day is present and equal,
    // retire the later rows with by = "dedup" and at = observed_at.
    let mut seen_keys: HashSet<(String, String, Option<String>)> = HashSet::new();
    for row in &mut full_rows {
        if row.retired.is_none()
            && let Some(ref day) = row.source_day
        {
            let norm = normalize_observation_content(&row.content);
            let rel_key = row
                .relation
                .as_ref()
                .map(|v| serde_json::to_string(v).unwrap_or_default());
            let key = (norm, day.clone(), rel_key);
            if !seen_keys.insert(key) {
                row.retired = Some(Retired {
                    at: row.observed_at,
                    by: "dedup".to_string(),
                });
            }
        }
    }

    Ok(ParsedObservations { full_rows })
}

/// Strictly parse observation content from an in-memory string.
pub fn parse_observation_content(text: &str) -> Result<ParsedObservations, ObservationStoreError> {
    parse_observation_file(text, ObservationParseSource::CapturedSnapshot)
}

/// Serialize observation rows to canonical JSONL text.
pub fn serialize_observation_rows(rows: &[ObservationRow]) -> String {
    let mut out = String::new();
    for row in rows {
        let map = if let Some(ref raw) = row.raw_json {
            let mut m = raw.clone();
            // Every write materializes the row's id, derived or explicit, so a
            // legacy row's id cannot shift once anything is appended after it.
            m.insert("id".to_string(), Value::Number(row.id.into()));
            if m.contains_key("content") || !row.content.is_empty() {
                m.insert("content".to_string(), Value::String(row.content.clone()));
            }
            if let Some(ref rel) = row.relation {
                m.insert("relation".to_string(), rel.clone());
            } else {
                m.remove("relation");
            }
            if let Some(ref sd) = row.source_day {
                m.insert("source_day".to_string(), Value::String(sd.clone()));
            } else if m.contains_key("source_day") {
                m.remove("source_day");
            }
            if let Some(ref by) = row.by {
                m.insert("by".to_string(), Value::String(by.clone()));
            } else if m.contains_key("by") {
                m.remove("by");
            }
            if !row.history.is_empty() {
                m.insert(
                    "history".to_string(),
                    serde_json::to_value(&row.history).expect("history serializes"),
                );
            } else if m.contains_key("history") {
                m.remove("history");
            }
            if let Some(ref ret) = row.retired {
                m.insert(
                    "retired".to_string(),
                    serde_json::to_value(ret).expect("retired serializes"),
                );
            } else if m.contains_key("retired") {
                m.remove("retired");
            }
            m
        } else {
            let mut m = Map::new();
            m.insert("id".to_string(), Value::Number(row.id.into()));
            m.insert("content".to_string(), Value::String(row.content.clone()));
            m.insert(
                "observed_at".to_string(),
                Value::Number(row.observed_at.into()),
            );
            if let Some(ref sd) = row.source_day {
                m.insert("source_day".to_string(), Value::String(sd.clone()));
            }
            if let Some(ref rel) = row.relation {
                m.insert("relation".to_string(), rel.clone());
            }
            if let Some(ref by) = row.by {
                m.insert("by".to_string(), Value::String(by.clone()));
            }
            if !row.history.is_empty() {
                m.insert(
                    "history".to_string(),
                    serde_json::to_value(&row.history).expect("history serializes"),
                );
            }
            if let Some(ref ret) = row.retired {
                m.insert(
                    "retired".to_string(),
                    serde_json::to_value(ret).expect("retired serializes"),
                );
            }
            m
        };
        out.push_str(&serde_json::to_string(&map).expect("row serializes"));
        out.push('\n');
    }
    out
}

pub fn facet_entity_observations_path(
    journal_root: &Path,
    facet: &str,
    entity_dir: &str,
) -> Result<PathBuf, PathError> {
    contained_path(
        journal_root,
        &format!("facets/{facet}/entities/{entity_dir}/observations.jsonl"),
    )
}

fn write_facet_entity_observations(
    journal_root: &Path,
    facet: &str,
    entity_dir: &str,
    content: &str,
) -> Result<(), ObservationWriteError> {
    let relative = format!("facets/{facet}/entities/{entity_dir}/observations.jsonl");
    let target = contained_path(journal_root, &relative)?;
    write_text(&target, content, AtomicWriteOptions::default())
        .map_err(ObservationWriteError::Write)
}

/// Single entry point for modifying facet-scoped entity observations.
pub fn apply_observation_change(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
    change: ObservationChange,
) -> Result<ObservationWriteOutcome, ObservationWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let path = facet_entity_observations_path(journal_root, facet_dir, entity_dir)?;

    let (mut parsed, file_existed) = if path_lexists(&path).map_err(ObservationStoreError::from)? {
        let text = read_text(&path, String::new()).map_err(ObservationStoreError::from)?;
        let parsed = parse_observation_file(&text, ObservationParseSource::Path(&path))?;
        (parsed, true)
    } else {
        (
            ParsedObservations {
                full_rows: Vec::new(),
            },
            false,
        )
    };

    match change {
        ObservationChange::Append {
            content,
            source_day,
            relation,
            actor,
        } => {
            let content = content.trim();
            if content.is_empty() {
                return Err(ObservationWriteError::EmptyContent);
            }
            let normalized = normalize_observation_content(content);
            if parsed
                .full_rows
                .iter()
                .any(|r| normalize_observation_content(&r.content) == normalized)
            {
                return Ok(ObservationWriteOutcome::AlreadyPresent {
                    count: parsed.live_count(),
                });
            }

            let next_id = parsed.full_rows.iter().map(|r| r.id).max().unwrap_or(0) + 1;

            let new_row = ObservationRow {
                id: next_id,
                content: content.to_owned(),
                observed_at: Utc::now().timestamp_millis(),
                source_day: source_day.filter(|s| !s.trim().is_empty()),
                relation: relation.filter(|v| !v.is_null()),
                by: Some(actor.to_owned()),
                history: Vec::new(),
                retired: None,
                raw_json: None,
            };

            parsed.full_rows.push(new_row);
            let serialized = serialize_observation_rows(&parsed.full_rows);
            write_facet_entity_observations(journal_root, facet_dir, entity_dir, &serialized)?;

            Ok(ObservationWriteOutcome::Appended {
                id: next_id,
                count: parsed.live_count(),
            })
        }
        ObservationChange::AppendMany { rows, actor } => {
            if rows.is_empty() {
                return Ok(ObservationWriteOutcome::NoOp);
            }

            let next_id = parsed.full_rows.iter().map(|r| r.id).max().unwrap_or(0) + 1;

            for (id, incoming) in (next_id..).zip(rows) {
                parsed.full_rows.push(ObservationRow {
                    id,
                    content: incoming.content,
                    observed_at: incoming.observed_at,
                    source_day: incoming.source_day,
                    relation: incoming.relation,
                    by: Some(actor.to_owned()),
                    history: Vec::new(),
                    retired: None,
                    raw_json: None,
                });
            }

            let serialized = serialize_observation_rows(&parsed.full_rows);
            write_facet_entity_observations(journal_root, facet_dir, entity_dir, &serialized)?;

            Ok(ObservationWriteOutcome::AppendedMany {
                count: parsed.live_count(),
            })
        }
        ObservationChange::EditInPlace {
            full_set_index,
            rewrite,
        } => {
            if full_set_index >= parsed.full_rows.len() {
                return Err(ObservationWriteError::Conflict {
                    message: format!("full set index {full_set_index} out of bounds"),
                });
            }
            let target_row = &mut parsed.full_rows[full_set_index];
            if let Some(obj) = rewrite.as_object() {
                if let Some(target_id) = obj.get("target_entity_id") {
                    if let Some(Value::Object(ref mut map)) = target_row.relation {
                        map.insert("target_entity_id".to_string(), target_id.clone());
                    } else {
                        target_row.relation = Some(rewrite);
                    }
                } else if let Some(rel) = obj.get("relation") {
                    target_row.relation = if rel.is_null() {
                        None
                    } else {
                        Some(rel.clone())
                    };
                } else {
                    target_row.relation = Some(rewrite);
                }
            } else {
                target_row.relation = Some(rewrite);
            }

            let serialized = serialize_observation_rows(&parsed.full_rows);
            write_facet_entity_observations(journal_root, facet_dir, entity_dir, &serialized)?;

            Ok(ObservationWriteOutcome::Edited)
        }
        ObservationChange::ApplyOps { ops, source_day } => {
            let (after_rows, counts, changed) =
                apply_ops_to_parsed(&parsed, &ops, source_day.as_deref())?;
            if changed {
                let serialized = serialize_observation_rows(&after_rows);
                write_facet_entity_observations(journal_root, facet_dir, entity_dir, &serialized)?;
            }
            Ok(ObservationWriteOutcome::OpsApplied { counts })
        }
        ObservationChange::ReplaceFullSet { rows } => {
            let serialized = serialize_observation_rows(&rows);
            write_facet_entity_observations(journal_root, facet_dir, entity_dir, &serialized)?;
            Ok(ObservationWriteOutcome::Replaced)
        }
        ObservationChange::RemoveFile => {
            if file_existed {
                let rel = format!("facets/{facet_dir}/entities/{entity_dir}/observations.jsonl");
                let _ = solstone_core_journal_io::remove_file(journal_root, &rel);
            }
            Ok(ObservationWriteOutcome::Removed)
        }
    }
}

/// Read live observations for owner surfaces with sorting and pagination.
pub fn read_live_observations(
    journal_root: &Path,
    facet: &str,
    entity_dir: &str,
    query: ObservationReadQuery,
) -> Result<ObservationPage, ObservationStoreError> {
    let path = facet_entity_observations_path(journal_root, facet, entity_dir)?;
    if !path_lexists(&path)? {
        return Ok(ObservationPage {
            total: 0,
            offset: query.offset.unwrap_or(0),
            limit: query.limit,
            has_more: false,
            items: Vec::new(),
        });
    }

    let text = read_text(&path, String::new())?;
    let parsed = parse_observation_file(&text, ObservationParseSource::Path(&path))?;
    let mut live: Vec<&ObservationRow> = parsed.live_rows().collect();

    match query.order {
        ObservationReadOrder::Newest => {
            live.sort_by(|a, b| {
                b.observed_at
                    .cmp(&a.observed_at)
                    .then_with(|| b.id.cmp(&a.id))
            });
        }
        ObservationReadOrder::Oldest => {
            live.sort_by(|a, b| {
                a.observed_at
                    .cmp(&b.observed_at)
                    .then_with(|| a.id.cmp(&b.id))
            });
        }
    }

    let total = live.len();
    if let Some(after_id) = query.after_id {
        // Cursor pages walk live rows by ascending id, independent of the
        // requested order: an update keeps its id, an append takes a higher
        // one, and a retirement removes nothing from the sequence, so a walk
        // by id skips no row that stays live and repeats none.
        let mut cursor: Vec<&ObservationRow> =
            live.iter().copied().filter(|r| r.id > after_id).collect();
        cursor.sort_by_key(|r| r.id);
        let has_more = cursor.len() > query.limit;
        cursor.truncate(query.limit);
        let items = cursor
            .iter()
            .map(|r| ObservationPageItem {
                id: r.id,
                content: r.content.clone(),
                observed_at: r.observed_at,
                source_day: r.source_day.clone(),
                relation: r.relation.clone(),
                by: r.by.clone(),
            })
            .collect();
        return Ok(ObservationPage {
            total,
            offset: 0,
            limit: query.limit,
            has_more,
            items,
        });
    }
    let offset = query.offset.unwrap_or(0);

    let items = if offset >= total {
        Vec::new()
    } else {
        let end = (offset + query.limit).min(total);
        live[offset..end]
            .iter()
            .map(|r| ObservationPageItem {
                id: r.id,
                content: r.content.clone(),
                observed_at: r.observed_at,
                source_day: r.source_day.clone(),
                relation: r.relation.clone(),
                by: r.by.clone(),
            })
            .collect()
    };

    let has_more = (offset + items.len()) < total;

    Ok(ObservationPage {
        total,
        offset,
        limit: query.limit,
        has_more,
        items,
    })
}

/// Compute live observation metrics for an attached entity.
pub fn observation_summary(
    journal_root: &Path,
    facet: &str,
    entity_dir: &str,
) -> Result<ObservationSummary, ObservationStoreError> {
    let path = facet_entity_observations_path(journal_root, facet, entity_dir)?;
    if !path_lexists(&path)? {
        return Ok(ObservationSummary {
            count: 0,
            latest_observed_at: None,
        });
    }

    let text = read_text(&path, String::new())?;
    let parsed = parse_observation_file(&text, ObservationParseSource::Path(&path))?;
    let live_rows: Vec<&ObservationRow> = parsed.live_rows().collect();
    let count = live_rows.len() as u64;
    let latest_observed_at = live_rows.iter().map(|r| r.observed_at).max();

    Ok(ObservationSummary {
        count,
        latest_observed_at,
    })
}

/// Count live observations for an entity relationship.
pub fn count_observations(
    journal_root: &Path,
    facet: &str,
    entity_dir: &str,
) -> Result<usize, ObservationStoreError> {
    let path = facet_entity_observations_path(journal_root, facet, entity_dir)?;
    if !path_lexists(&path)? {
        return Ok(0);
    }
    let text = read_text(&path, String::new())?;
    let parsed = parse_observation_file(&text, ObservationParseSource::Path(&path))?;
    Ok(parsed.live_count())
}

/// Compute day-by-day observation counts for an entity relationship.
pub fn observation_day_counts(
    journal_root: &Path,
    facet: &str,
    entity_dir: &str,
) -> Result<BTreeMap<String, usize>, ObservationStoreError> {
    let path = facet_entity_observations_path(journal_root, facet, entity_dir)?;
    if !path_lexists(&path)? {
        return Ok(BTreeMap::new());
    }
    let text = read_text(&path, String::new())?;
    let parsed = parse_observation_file(&text, ObservationParseSource::Path(&path))?;
    let mut day_counts = BTreeMap::new();
    for row in parsed.live_rows() {
        let day = if let Some(ref source_day) = row.source_day {
            source_day.clone()
        } else {
            let dt =
                chrono::DateTime::from_timestamp(row.observed_at / 1000, 0).unwrap_or_default();
            dt.format("%Y%m%d").to_string()
        };
        *day_counts.entry(day).or_insert(0) += 1;
    }
    Ok(day_counts)
}

pub fn list_facet_entity_directories(
    journal_root: &Path,
    facet: &str,
) -> Result<Vec<String>, ObservationStoreError> {
    let entities_dir = contained_path(journal_root, &format!("facets/{facet}/entities"))?;
    let mut dirs = Vec::new();
    for entry in list_dir_entries(&entities_dir)? {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        if let Some(name) = entry.name.to_str() {
            dirs.push(name.to_owned());
        }
    }
    Ok(dirs)
}

fn read_facet_entity_id(
    journal_root: &Path,
    facet: &str,
    entity_dir: &str,
) -> Result<Option<String>, ObservationStoreError> {
    let path = contained_path(
        journal_root,
        &format!("facets/{facet}/entities/{entity_dir}/entity.json"),
    )?;
    if !path_lexists(&path)? {
        return Ok(None);
    }
    let text = read_text(&path, String::new())?;
    if let Ok(val) = serde_json::from_str::<Value>(&text)
        && let Some(id) = val.get("entity_id").and_then(Value::as_str)
    {
        return Ok(Some(id.to_owned()));
    }
    Ok(None)
}

pub fn resolve_observation_entity_dir(
    journal_root: &Path,
    facet: &str,
    entity_query: &str,
) -> Result<ObservationEntityResolution, ObservationLookupError> {
    let map = read_identity_map(journal_root)?;
    let mut scoped = Vec::new();
    let relationship_dirs = list_facet_entity_directories(journal_root, facet)?;
    for relationship_dir in relationship_dirs {
        let Some(link_id) = read_facet_entity_id(journal_root, facet, &relationship_dir)? else {
            continue;
        };
        let entity_dir = map.resolved.get(&link_id).cloned();
        let name = if let Some(edir) = entity_dir.as_ref() {
            if let Ok(Some(identity)) = read_entity_identity(journal_root, edir) {
                identity
                    .value()
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            } else {
                None
            }
        } else {
            None
        };
        scoped.push((relationship_dir, link_id, entity_dir, name));
    }

    if let Some((rel_dir, _, _, _)) = scoped.iter().find(|(_, id, _, _)| id == entity_query) {
        return Ok(ObservationEntityResolution::Resolved {
            entity_dir: rel_dir.clone(),
        });
    }

    if let Some((rel_dir, _, _, _)) = scoped
        .iter()
        .find(|(_, _, edir, _)| edir.as_deref() == Some(entity_query))
    {
        return Ok(ObservationEntityResolution::Resolved {
            entity_dir: rel_dir.clone(),
        });
    }

    if let Some((rel_dir, _, _, _)) = scoped
        .iter()
        .find(|(rel_dir, _, _, _)| rel_dir == entity_query)
    {
        return Ok(ObservationEntityResolution::Resolved {
            entity_dir: rel_dir.clone(),
        });
    }

    let wanted = normalize_resolution_query(entity_query);
    if let Some((rel_dir, _, _, _)) = scoped.iter().find(|(_, _, _, name)| {
        name.as_deref().map(normalize_resolution_query).as_deref() == Some(&wanted)
    }) {
        return Ok(ObservationEntityResolution::Resolved {
            entity_dir: rel_dir.clone(),
        });
    }

    let derived = entity_slug(entity_query);
    if !derived.is_empty()
        && list_facet_entity_directories(journal_root, facet)?
            .iter()
            .any(|entity_dir| entity_dir == &derived)
    {
        return Ok(ObservationEntityResolution::Resolved {
            entity_dir: derived,
        });
    }

    Ok(ObservationEntityResolution::NoSuchEntity)
}

pub fn load_observations_for_query(
    journal_root: &Path,
    facet: &str,
    entity_query: &str,
) -> Result<ObservationLookup, ObservationLookupError> {
    let entity_dir = match resolve_observation_entity_dir(journal_root, facet, entity_query)? {
        ObservationEntityResolution::Resolved { entity_dir } => entity_dir,
        ObservationEntityResolution::NoSuchEntity => return Ok(ObservationLookup::Unresolvable),
    };

    let path = facet_entity_observations_path(journal_root, facet, &entity_dir)?;
    if !path_lexists(&path)? {
        return Ok(ObservationLookup::Resolved {
            entity_dir,
            observations: Vec::new(),
        });
    }

    let text = read_text(&path, String::new())?;
    let parsed = parse_observation_file(&text, ObservationParseSource::Path(&path))?;
    let mut live: Vec<&ObservationRow> = parsed.live_rows().collect();
    live.sort_by(|a, b| {
        b.observed_at
            .cmp(&a.observed_at)
            .then_with(|| b.id.cmp(&a.id))
    });

    let observations = live
        .into_iter()
        .map(|r| {
            let mut obj = Map::new();
            obj.insert("id".to_string(), Value::Number(r.id.into()));
            obj.insert("content".to_string(), Value::String(r.content.clone()));
            obj.insert(
                "observed_at".to_string(),
                Value::Number(r.observed_at.into()),
            );
            if let Some(ref sd) = r.source_day {
                obj.insert("source_day".to_string(), Value::String(sd.clone()));
            }
            if let Some(ref rel) = r.relation {
                obj.insert("relation".to_string(), rel.clone());
            }
            if let Some(ref by) = r.by {
                obj.insert("by".to_string(), Value::String(by.clone()));
            }
            Value::Object(obj)
        })
        .collect();

    Ok(ObservationLookup::Resolved {
        entity_dir,
        observations,
    })
}

/// Append an observation row for an entity relationship with retry.
pub fn add_observation(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
    content: &str,
    source_day: Option<&str>,
    relation: Option<&Value>,
) -> Result<(Vec<Value>, usize, bool), ObservationWriteError> {
    let content = content.trim();
    if content.is_empty() {
        return Err(ObservationWriteError::EmptyContent);
    }
    retry_add_operation(|| {
        let outcome = apply_observation_change(
            journal_root,
            facet_dir,
            entity_dir,
            ObservationChange::Append {
                content: content.to_owned(),
                source_day: source_day.map(str::to_owned),
                relation: relation.cloned(),
                actor: "owner",
            },
        )?;
        let (count, already_present) = match outcome {
            ObservationWriteOutcome::Appended { count, .. } => (count, false),
            ObservationWriteOutcome::AlreadyPresent { count } => (count, true),
            _ => (0, false),
        };
        let page = read_live_observations(
            journal_root,
            facet_dir,
            entity_dir,
            ObservationReadQuery {
                limit: 50,
                ..Default::default()
            },
        )?;
        let values = page
            .items
            .into_iter()
            .map(|item| {
                json!({
                    "id": item.id,
                    "content": item.content,
                    "observed_at": item.observed_at,
                    "source_day": item.source_day,
                    "relation": item.relation,
                    "by": item.by,
                })
            })
            .collect();
        Ok((values, count, already_present))
    })
}

fn ensure_facet_relationship_internal(
    journal_root: &Path,
    facet: &str,
    entity_id: &str,
    name: &str,
) -> Result<String, ObservationWriteError> {
    let slug = entity_slug(entity_id);
    let entity_dir = if slug.is_empty() {
        entity_slug(name)
    } else {
        slug
    };
    let rel = format!("facets/{facet}/entities/{entity_dir}/entity.json");
    let target = contained_path(journal_root, &rel)?;
    if !path_lexists(&target).map_err(ObservationStoreError::from)? {
        let payload = json!({
            "entity_id": entity_id,
            "created_at": Utc::now().timestamp_millis(),
        });
        write_text(
            &target,
            &serde_json::to_string_pretty(&payload).unwrap(),
            AtomicWriteOptions::default(),
        )?;
    }
    Ok(entity_dir)
}

/// Apply batch observation operations with strict JSON validation under the facet trust lock.
pub fn record_observation_ops_strict(
    journal_root: &Path,
    facet_dir: &str,
    entity_query: &str,
    operations: &[Value],
    source_day: Option<&str>,
) -> Result<ObservationOperationCounts, ObservationWriteError> {
    retry_record_operation(|| {
        let _trust = hold_facet_trust_lock(journal_root)?;
        let entity_dir = match resolve_observation_entity_dir(journal_root, facet_dir, entity_query)
            .map_err(|e| ObservationWriteError::Resolve(e.to_string()))?
        {
            ObservationEntityResolution::Resolved { entity_dir } => entity_dir,
            ObservationEntityResolution::NoSuchEntity => {
                let entity_id = entity_slug(entity_query);
                ensure_facet_relationship_internal(
                    journal_root,
                    facet_dir,
                    &entity_id,
                    entity_query,
                )?
            }
        };
        let outcome = apply_observation_change(
            journal_root,
            facet_dir,
            &entity_dir,
            ObservationChange::ApplyOps {
                ops: operations.to_vec(),
                source_day: source_day.map(str::to_owned),
            },
        )?;
        match outcome {
            ObservationWriteOutcome::OpsApplied { counts } => Ok(counts),
            _ => Ok(ObservationOperationCounts::default()),
        }
    })
}

pub fn apply_ops_to_parsed(
    parsed: &ParsedObservations,
    operations: &[Value],
    source_day: Option<&str>,
) -> Result<(Vec<ObservationRow>, ObservationOperationCounts, bool), ObservationWriteError> {
    let mut rows = parsed.full_rows.clone();
    let mut counts = ObservationOperationCounts::default();
    let mut changed = false;

    let now = Utc::now().timestamp_millis();
    let next_id_start = rows.iter().map(|r| r.id).max().unwrap_or(0) + 1;
    let mut next_id = next_id_start;

    for op_val in operations {
        let Some(op_obj) = op_val.as_object() else {
            counts.refused += 1;
            counts.skipped += 1;
            continue;
        };
        let op_type = op_obj.get("op").and_then(Value::as_str).unwrap_or("");

        if op_type == "add" {
            let content = op_obj
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if content.is_empty() {
                counts.refused += 1;
                counts.skipped += 1;
                continue;
            }

            let rel_val = op_obj.get("relation").filter(|v| !v.is_null());
            let candidate_norm = normalize_observation_content(content);

            // Normalized-content guard against ALL rows (live or retired), matching Append
            let already_present = rows
                .iter()
                .any(|r| normalize_observation_content(&r.content) == candidate_norm);

            if already_present {
                counts.skip += 1;
                counts.skipped += 1;
                continue;
            }

            rows.push(ObservationRow {
                id: next_id,
                content: content.to_owned(),
                observed_at: now,
                source_day: source_day.map(str::to_owned),
                relation: rel_val.cloned(),
                by: Some("model".to_owned()),
                history: Vec::new(),
                retired: None,
                raw_json: None,
            });
            next_id += 1;
            counts.add += 1;
            changed = true;
            continue;
        }

        if op_type == "skip" {
            counts.skip += 1;
            counts.skipped += 1;
            continue;
        }

        if op_type == "keep" {
            counts.keep += 1;
            continue;
        }

        if !matches!(op_type, "replace" | "update" | "drop") {
            counts.refused += 1;
            counts.skipped += 1;
            continue;
        }

        let target_id_opt = op_obj.get("target_id").and_then(Value::as_u64);
        let full_idx_opt = if let Some(target_id) = target_id_opt {
            rows.iter()
                .position(|r| r.id == target_id && r.retired.is_none())
        } else {
            None
        };

        let target_quote_val = op_obj.get("target_quote").and_then(Value::as_str);

        let Some(full_idx) = full_idx_opt else {
            counts.refused += 1;
            counts.skipped += 1;
            continue;
        };

        let candidate = &rows[full_idx];
        if let Some(target_quote) = target_quote_val {
            if !target_quote.trim().is_empty() && !matches_quote(&candidate.content, target_quote) {
                counts.refused += 1;
                counts.skipped += 1;
                continue;
            }
        } else if op_type != "drop" {
            counts.refused += 1;
            counts.skipped += 1;
            continue;
        }

        match op_type {
            "replace" | "update" => {
                let content = op_obj
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                if content.is_empty() {
                    counts.refused += 1;
                    counts.skipped += 1;
                    continue;
                }
                let target_row = &mut rows[full_idx];
                let old_history = HistoryEntry {
                    content: target_row.content.clone(),
                    observed_at: target_row.observed_at,
                    source_day: target_row.source_day.clone(),
                    relation: target_row.relation.clone(),
                    by: target_row.by.clone(),
                };
                target_row.history.push(old_history);
                target_row.content = content.to_owned();
                target_row.observed_at = now;
                target_row.source_day = source_day.map(str::to_owned);
                if let Some(rel) = op_obj.get("relation").filter(|v| !v.is_null()) {
                    target_row.relation = Some(rel.clone());
                }
                target_row.by = Some("model".to_owned());
                counts.replace += 1;
                counts.update += 1;
                changed = true;
            }
            "drop" => {
                rows[full_idx].retired = Some(Retired {
                    at: now,
                    by: "model".to_owned(),
                });
                counts.drop += 1;
                changed = true;
            }
            _ => unreachable!(),
        }
    }

    Ok((rows, counts, changed))
}

fn matches_quote(candidate_content: &str, target_quote: &str) -> bool {
    let cand_norm = normalize_observation_content(candidate_content);
    let quote_norm = normalize_observation_content(target_quote);
    cand_norm.contains(&quote_norm)
}

fn retry_add_operation<T>(
    mut operation: impl FnMut() -> Result<T, ObservationWriteError>,
) -> Result<T, ObservationWriteError> {
    for attempt in 0..OBSERVATION_RETRY_ATTEMPTS {
        match operation() {
            Ok(result) => return Ok(result),
            Err(ObservationWriteError::Write(AtomicWriteError::Io { .. }))
                if attempt + 1 < OBSERVATION_RETRY_ATTEMPTS =>
            {
                jitter_sleep(25 * (attempt + 1));
            }
            Err(other) => return Err(other),
        }
    }
    operation()
}

fn retry_record_operation<T>(
    mut operation: impl FnMut() -> Result<T, ObservationWriteError>,
) -> Result<T, ObservationWriteError> {
    for attempt in 0..OBSERVATION_RETRY_ATTEMPTS {
        match operation() {
            Ok(result) => return Ok(result),
            Err(
                ObservationWriteError::TrustLock(_)
                | ObservationWriteError::Write(AtomicWriteError::Io { .. }),
            ) if attempt + 1 < OBSERVATION_RETRY_ATTEMPTS => {
                jitter_sleep(25 * (attempt + 1));
            }
            Err(other) => return Err(other),
        }
    }
    operation()
}

pub fn retry_add_for_test<T>(
    operation: impl FnMut() -> Result<T, ObservationWriteError>,
) -> Result<T, ObservationWriteError> {
    retry_add_operation(operation)
}

pub fn retry_record_for_test<T>(
    operation: impl FnMut() -> Result<T, ObservationWriteError>,
) -> Result<T, ObservationWriteError> {
    retry_record_operation(operation)
}

fn jitter_sleep(maximum_ms: usize) {
    let entropy = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        ^ u128::from(std::process::id());
    thread::sleep(Duration::from_millis(
        1 + (entropy % (maximum_ms as u128)) as u64,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_handles_strict_legacy_and_derived_ids() {
        let legacy_jsonl = "{\"content\":\"First fact\",\"observed_at\":1000}\n{\"content\":\"Second fact\",\"observed_at\":2000}\n";
        let parsed =
            parse_observation_file(legacy_jsonl, ObservationParseSource::CapturedSnapshot).unwrap();
        assert_eq!(parsed.full_rows.len(), 2);
        assert_eq!(parsed.full_rows[0].id, 1);
        assert_eq!(parsed.full_rows[0].content, "First fact");
        assert_eq!(parsed.full_rows[0].by, None);
        assert_eq!(parsed.full_rows[1].id, 2);
        assert_eq!(parsed.full_rows[1].content, "Second fact");
    }

    #[test]
    fn parser_respects_explicit_ids_and_derives_above_max() {
        let jsonl = "{\"id\":5,\"content\":\"Explicit 5\",\"observed_at\":1000}\n{\"content\":\"Derived 6\",\"observed_at\":2000}\n";
        let parsed =
            parse_observation_file(jsonl, ObservationParseSource::CapturedSnapshot).unwrap();
        assert_eq!(parsed.full_rows[0].id, 5);
        assert_eq!(parsed.full_rows[1].id, 6);
    }

    #[test]
    fn parser_collapses_same_content_and_same_day() {
        let jsonl = "{\"content\":\"Fact A\",\"observed_at\":1000,\"source_day\":\"20260101\"}\n{\"content\":\"Fact A\",\"observed_at\":2000,\"source_day\":\"20260101\"}\n{\"content\":\"Fact A\",\"observed_at\":3000,\"source_day\":\"20260102\"}\n";
        let parsed =
            parse_observation_file(jsonl, ObservationParseSource::CapturedSnapshot).unwrap();
        assert_eq!(parsed.full_rows.len(), 3);
        assert!(parsed.full_rows[0].retired.is_none());
        assert_eq!(
            parsed.full_rows[1].retired,
            Some(Retired {
                at: 2000,
                by: "dedup".to_string()
            })
        );
        assert!(parsed.full_rows[2].retired.is_none());
        assert_eq!(parsed.live_count(), 2);
    }

    #[test]
    fn parser_rejects_malformed_lines() {
        let bad_json = "not json";
        assert!(matches!(
            parse_observation_file(bad_json, ObservationParseSource::CapturedSnapshot),
            Err(ObservationStoreError::MalformedObservation { line: 1, .. })
        ));

        let not_object = "12345";
        assert!(matches!(
            parse_observation_file(not_object, ObservationParseSource::CapturedSnapshot),
            Err(ObservationStoreError::MalformedObservation { line: 1, .. })
        ));

        let invalid_id = "{\"id\":\"abc\",\"content\":\"Fact\"}";
        assert!(matches!(
            parse_observation_file(invalid_id, ObservationParseSource::CapturedSnapshot),
            Err(ObservationStoreError::MalformedObservation { line: 1, .. })
        ));
    }

    #[test]
    fn normalized_duplicate_guard_matches() {
        let base = "Prefers async communication!";
        assert_eq!(
            normalize_observation_content(base),
            normalize_observation_content("  prefers   async COMMUNICATION.  ")
        );
    }

    #[test]
    fn apply_ops_to_parsed_by_target_id() {
        let jsonl = "{\"id\":1,\"content\":\"Original fact one\",\"observed_at\":1000}\n{\"id\":2,\"content\":\"Original fact two\",\"observed_at\":2000}\n";
        let parsed =
            parse_observation_file(jsonl, ObservationParseSource::CapturedSnapshot).unwrap();

        // 1. Unknown target_id -> refused
        let ops = vec![
            json!({"op": "replace", "target_id": 99, "target_quote": "Original", "content": "New content"}),
        ];
        let (_rows, counts, changed) =
            apply_ops_to_parsed(&parsed, &ops, Some("20260910")).unwrap();
        assert_eq!(counts.refused, 1);
        assert!(!changed);

        // 2. Mismatched target_quote -> refused
        let ops = vec![
            json!({"op": "replace", "target_id": 1, "target_quote": "Nonexistent quote", "content": "New content"}),
        ];
        let (_rows, counts, changed) =
            apply_ops_to_parsed(&parsed, &ops, Some("20260910")).unwrap();
        assert_eq!(counts.refused, 1);
        assert!(!changed);

        // 3. Valid replace -> replaced, preserves id=1, records history
        let ops = vec![
            json!({"op": "replace", "target_id": 1, "target_quote": "Original fact one", "content": "Updated fact one"}),
        ];
        let (rows, counts, changed) = apply_ops_to_parsed(&parsed, &ops, Some("20260910")).unwrap();
        assert_eq!(counts.replace, 1);
        assert!(changed);
        assert_eq!(rows[0].id, 1);
        assert_eq!(rows[0].content, "Updated fact one");
        assert_eq!(rows[0].history.len(), 1);
        assert_eq!(rows[0].history[0].content, "Original fact one");

        // 4. Valid drop -> retired
        let ops = vec![json!({"op": "drop", "target_id": 2, "target_quote": "Original fact two"})];
        let (rows, counts, changed) = apply_ops_to_parsed(&parsed, &ops, Some("20260910")).unwrap();
        assert_eq!(counts.drop, 1);
        assert!(changed);
        assert_eq!(rows[1].id, 2);
        assert!(rows[1].retired.is_some());

        // 5. Add duplicate -> skipped
        let ops = vec![json!({"op": "add", "content": "original fact one"})];
        let (_rows, counts, changed) =
            apply_ops_to_parsed(&parsed, &ops, Some("20260910")).unwrap();
        assert_eq!(counts.skip, 1);
        assert!(!changed);
    }
}
