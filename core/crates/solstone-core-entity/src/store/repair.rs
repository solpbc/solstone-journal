// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One-shot repair for durable entity identity ids.

#[cfg(test)]
use std::cell::Cell;
use std::error::Error;
use std::fmt;
#[cfg(test)]
use std::io;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::{Map, Value};
use solstone_core_journal_io::AtomicWriteError;
use solstone_core_journal_io::DirEntryKind;
use solstone_core_journal_io::JsonWriteOptions;
use solstone_core_journal_io::PathError;
use solstone_core_journal_io::ReadError;
use solstone_core_journal_io::contained_path;
use solstone_core_journal_io::list_dir_entries;
use solstone_core_journal_io::path_lexists;
use solstone_core_journal_io::write_json;

use crate::{EntityTrustLockError, hold_entity_trust_lock};

use super::error::EntityStoreError;
use super::history::read_prepared_history;
use super::identity::read_entity_identity;
use super::paths::identity_path;
use super::write::write_identity_snapshot;

const COMPLETION_MARKER_RELATIVE_PATH: &str = "health/migrations/entity-identity-repair.json";

#[cfg(test)]
thread_local! {
    static REPAIR_WRITE_FAILURE_ON_ATTEMPT: Cell<usize> = const { Cell::new(0) };
    static REPAIR_WRITE_ATTEMPTS: Cell<usize> = const { Cell::new(0) };
}

/// Result of a completed or incomplete identity repair scan.
///
/// Each branch retains entity directory names rather than duplicated counters, so
/// callers can act on the exact files a destructive migration encountered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntityIdentityRepairReport {
    pub added: Vec<String>,
    pub overwritten: Vec<String>,
    pub left_alone: Vec<String>,
    pub refused: Vec<EntityIdentityRepairRefusal>,
    pub skipped: Vec<EntityIdentityRepairSkip>,
    pub completion_marker: PathBuf,
}

/// One entity the repair deliberately left untouched for operator intervention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntityIdentityRepairRefusal {
    pub entity_dir: String,
    pub guard: EntityIdentityRepairGuard,
    pub detail: String,
}

/// Guard that prevents a one-shot repair from destroying durable evidence.
///
/// Prepared history is refused on presence rather than classified: publishing or
/// discarding it during repair would mutate evidence the migration must preserve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum EntityIdentityRepairGuard {
    StagedPreparedHistory,
    Malformed,
}

/// One directory skipped because it has no repairable identity object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntityIdentityRepairSkip {
    pub entity_dir: String,
    pub reason: EntityIdentityRepairSkipReason,
}

/// Why a directory did not supply an identity artifact to repair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum EntityIdentityRepairSkipReason {
    NotAnEntity,
    EmptyIdentityFile,
}

/// Failure while running the one-shot entity identity repair.
#[derive(Debug)]
pub enum EntityIdentityRepairError {
    AlreadyCompleted {
        completion_marker: PathBuf,
    },
    TrustLock(EntityTrustLockError),
    MarkerPath(PathError),
    DirectoryScan(PathError),
    PreparedHistoryRead {
        entity_dir: String,
        report: Box<EntityIdentityRepairReport>,
        source: Box<EntityStoreError>,
    },
    IdentityPath {
        entity_dir: String,
        report: Box<EntityIdentityRepairReport>,
        source: Box<EntityStoreError>,
    },
    IdentityRead {
        entity_dir: String,
        report: Box<EntityIdentityRepairReport>,
        source: Box<EntityStoreError>,
    },
    IdentityWrite {
        entity_dir: String,
        report: Box<EntityIdentityRepairReport>,
        source: Box<AtomicWriteError>,
    },
    Incomplete {
        report: Box<EntityIdentityRepairReport>,
    },
    CompletionMarkerWrite {
        report: Box<EntityIdentityRepairReport>,
        source: Box<AtomicWriteError>,
    },
}

impl fmt::Display for EntityIdentityRepairError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyCompleted { completion_marker } => write!(
                formatter,
                "entity identity repair has already completed: {}",
                completion_marker.display()
            ),
            Self::TrustLock(error) => error.fmt(formatter),
            Self::MarkerPath(error) | Self::DirectoryScan(error) => error.fmt(formatter),
            Self::PreparedHistoryRead {
                entity_dir, source, ..
            } => write!(
                formatter,
                "cannot inspect staged history for entity {entity_dir}: {source}"
            ),
            Self::IdentityPath {
                entity_dir, source, ..
            } => write!(
                formatter,
                "cannot inspect identity path for entity {entity_dir}: {source}"
            ),
            Self::IdentityRead {
                entity_dir, source, ..
            } => write!(
                formatter,
                "cannot read identity for entity {entity_dir}: {source}"
            ),
            Self::IdentityWrite {
                entity_dir, source, ..
            } => write!(
                formatter,
                "cannot repair identity for entity {entity_dir}: {source}"
            ),
            Self::Incomplete { .. } => formatter.write_str(
                "entity identity repair is incomplete; resolve refused entities before re-running",
            ),
            Self::CompletionMarkerWrite { source, .. } => {
                write!(
                    formatter,
                    "cannot record entity identity repair completion: {source}"
                )
            }
        }
    }
}

impl Error for EntityIdentityRepairError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::TrustLock(error) => Some(error),
            Self::MarkerPath(error) | Self::DirectoryScan(error) => Some(error),
            Self::PreparedHistoryRead { source, .. }
            | Self::IdentityPath { source, .. }
            | Self::IdentityRead { source, .. } => Some(source.as_ref()),
            Self::IdentityWrite { source, .. } | Self::CompletionMarkerWrite { source, .. } => {
                Some(source.as_ref())
            }
            Self::AlreadyCompleted { .. } | Self::Incomplete { .. } => None,
        }
    }
}

/// Stamp each repairable identity with its directory-derived id exactly once.
///
/// The whole scan holds the journal-wide trust lock. It never sources its work
/// from the identity map because that map intentionally omits missing identities.
/// Refused entities prevent the completion marker, allowing only a repaired
/// external condition to make a later invocation resume the migration.
pub fn repair_entity_identities(
    journal_root: &Path,
) -> Result<EntityIdentityRepairReport, EntityIdentityRepairError> {
    let completion_marker =
        completion_marker_path(journal_root).map_err(EntityIdentityRepairError::MarkerPath)?;
    let _trust =
        hold_entity_trust_lock(journal_root).map_err(EntityIdentityRepairError::TrustLock)?;
    if path_lexists(&completion_marker).map_err(EntityIdentityRepairError::MarkerPath)? {
        return Err(EntityIdentityRepairError::AlreadyCompleted { completion_marker });
    }

    let mut report = EntityIdentityRepairReport {
        added: Vec::new(),
        overwritten: Vec::new(),
        left_alone: Vec::new(),
        refused: Vec::new(),
        skipped: Vec::new(),
        completion_marker: completion_marker.clone(),
    };
    let entities_dir = contained_path(journal_root, "entities")
        .map_err(EntityIdentityRepairError::DirectoryScan)?;
    let entries =
        list_dir_entries(&entities_dir).map_err(EntityIdentityRepairError::DirectoryScan)?;

    for entry in entries {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let entity_dir = entry.name.to_string_lossy().into_owned();
        let prepared = read_prepared_history(journal_root, &entity_dir).map_err(|source| {
            EntityIdentityRepairError::PreparedHistoryRead {
                entity_dir: entity_dir.clone(),
                report: Box::new(report.clone()),
                source: Box::new(source),
            }
        })?;
        if !prepared.is_empty() {
            report.refused.push(EntityIdentityRepairRefusal {
                entity_dir: entity_dir.clone(),
                guard: EntityIdentityRepairGuard::StagedPreparedHistory,
                detail: format!(
                    "staged prepared history guard for {entity_dir}: resolve or discard the staged history for this entity before re-running the repair"
                ),
            });
            continue;
        }

        let identity_path = identity_path(journal_root, &entity_dir).map_err(|source| {
            EntityIdentityRepairError::IdentityPath {
                entity_dir: entity_dir.clone(),
                report: Box::new(report.clone()),
                source: Box::new(source),
            }
        })?;
        let identity_exists = path_lexists(&identity_path).map_err(|source| {
            EntityIdentityRepairError::IdentityPath {
                entity_dir: entity_dir.clone(),
                report: Box::new(report.clone()),
                source: Box::new(source.into()),
            }
        })?;
        let identity = match read_entity_identity(journal_root, &entity_dir) {
            Ok(identity) => identity,
            Err(source) if is_malformed_identity(&source) => {
                report.refused.push(EntityIdentityRepairRefusal {
                    entity_dir: entity_dir.clone(),
                    guard: EntityIdentityRepairGuard::Malformed,
                    detail: format!(
                        "malformed identity guard for {entity_dir}: the identity file failed to parse or is not an object; fix or restore it before re-running the repair: {source}"
                    ),
                });
                continue;
            }
            Err(source) => {
                return Err(EntityIdentityRepairError::IdentityRead {
                    entity_dir,
                    report: Box::new(report),
                    source: Box::new(source),
                });
            }
        };

        let Some(identity) = identity else {
            report.skipped.push(EntityIdentityRepairSkip {
                entity_dir,
                reason: if identity_exists {
                    EntityIdentityRepairSkipReason::EmptyIdentityFile
                } else {
                    EntityIdentityRepairSkipReason::NotAnEntity
                },
            });
            continue;
        };
        if !identity.was_written() {
            let repaired = identity_with_directory_id_first(&entity_dir, identity.value());
            write_repaired_identity(&identity_path, &repaired).map_err(|source| {
                EntityIdentityRepairError::IdentityWrite {
                    entity_dir: entity_dir.clone(),
                    report: Box::new(report.clone()),
                    source: Box::new(source),
                }
            })?;
            report.added.push(entity_dir);
        } else if identity.entity_id() == entity_dir {
            report.left_alone.push(entity_dir);
        } else {
            let mut repaired = identity.value().clone();
            repaired
                .as_object_mut()
                .expect("identity snapshots always contain objects")
                .insert("id".to_owned(), Value::String(entity_dir.clone()));
            write_repaired_identity(&identity_path, &repaired).map_err(|source| {
                EntityIdentityRepairError::IdentityWrite {
                    entity_dir: entity_dir.clone(),
                    report: Box::new(report.clone()),
                    source: Box::new(source),
                }
            })?;
            report.overwritten.push(entity_dir);
        }
    }

    if !report.refused.is_empty() {
        return Err(EntityIdentityRepairError::Incomplete {
            report: Box::new(report),
        });
    }
    let marker = completion_marker_value(&report);
    write_json(
        &completion_marker,
        &marker,
        JsonWriteOptions {
            mode: Some(0o600),
            indent: Some(2),
            sort_keys: false,
        },
    )
    .map_err(|source| EntityIdentityRepairError::CompletionMarkerWrite {
        report: Box::new(report.clone()),
        source: Box::new(source),
    })?;
    Ok(report)
}

fn completion_marker_path(journal_root: &Path) -> Result<PathBuf, PathError> {
    contained_path(journal_root, COMPLETION_MARKER_RELATIVE_PATH)
}

fn completion_marker_value(report: &EntityIdentityRepairReport) -> Value {
    serde_json::to_value(serde_json::json!({
        "completed_at": Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        "report": report,
    }))
    .expect("identity repair marker contains only serializable values")
}

fn is_malformed_identity(error: &EntityStoreError) -> bool {
    matches!(
        error,
        EntityStoreError::IdentityNotObject { .. }
            | EntityStoreError::Read(ReadError::Malformed(_))
    )
}

/// Rebuild an id-less snapshot so durable JSON starts with the authoritative id.
fn identity_with_directory_id_first(entity_dir: &str, identity: &Value) -> Value {
    let mut repaired = Map::new();
    repaired.insert("id".to_owned(), Value::String(entity_dir.to_owned()));
    for (key, value) in identity
        .as_object()
        .expect("identity snapshots always contain objects")
    {
        if key != "id" {
            repaired.insert(key.clone(), value.clone());
        }
    }
    Value::Object(repaired)
}

fn write_repaired_identity(path: &Path, identity: &Value) -> Result<(), AtomicWriteError> {
    #[cfg(test)]
    if should_force_repair_write_failure() {
        return Err(AtomicWriteError::Io {
            path: path.to_path_buf(),
            source: io::Error::other("forced repair identity write failure"),
        });
    }
    write_identity_snapshot(path, identity)
}

#[cfg(test)]
fn should_force_repair_write_failure() -> bool {
    REPAIR_WRITE_FAILURE_ON_ATTEMPT.with(|target| {
        let target = target.get();
        target != 0
            && REPAIR_WRITE_ATTEMPTS.with(|attempts| {
                let attempt = attempts.get() + 1;
                attempts.set(attempt);
                attempt == target
            })
    })
}

#[cfg(test)]
pub(crate) fn set_repair_identity_write_failure_on_attempt(attempt: Option<usize>) {
    REPAIR_WRITE_ATTEMPTS.with(|attempts| attempts.set(0));
    REPAIR_WRITE_FAILURE_ON_ATTEMPT.with(|target| target.set(attempt.unwrap_or(0)));
}
