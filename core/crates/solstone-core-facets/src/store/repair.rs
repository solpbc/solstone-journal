// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One-shot linking of facet relationship directories to effective entity ids.

use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use solstone_core_entity::{
    EntityIdentityGroupMap, EntityStoreError, read_identity_group_map, read_prepared_history,
};
use solstone_core_journal_io::{
    AtomicWriteError, DirEntryKind, JsonWriteOptions, PathError, ReadError, list_dir_entries,
    path_lexists, read_json, write_json,
};

use crate::{FacetTrustLockError, hold_facet_trust_lock};

use super::declaration::read_facet_declaration;
use super::error::{FacetStoreError, FacetWriteError};
use super::identity::read_facet_entity_link;
use super::paths::{
    facet_entities_dir, facet_entity_link_journal_wide_repair_marker_path, facet_entity_link_path,
    facet_entity_link_repair_marker_path, facets_dir,
};
use super::write::save_facet_entity_link;

/// One classified facet relationship candidate from a linking repair scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FacetEntityLinkRepairBranch {
    Linked {
        facet_entity_dir: String,
        journal_entity_id: String,
    },
    Unmatched {
        facet_entity_dir: String,
    },
    MultiMatched {
        facet_entity_dir: String,
        journal_entity_dirs: Vec<String>,
    },
    RefusedUnparseable {
        facet_entity_dir: String,
        path: PathBuf,
        detail: String,
    },
    RefusedPending {
        facet_entity_dir: String,
        journal_entity_dir: String,
    },
    SkippedNotAnEntity {
        facet_entity_dir: String,
    },
}

/// Result of one facet's entity-link repair scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FacetEntityLinkReport {
    pub facet: String,
    pub branches: Vec<FacetEntityLinkRepairBranch>,
    pub completion_marker: PathBuf,
}

/// Result of the journal-wide facet entity-link repair scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FacetEntityLinkRepairReport {
    pub facets: Vec<FacetEntityLinkReport>,
    pub completion_marker: PathBuf,
}

/// Failure while running a facet entity-link repair.
#[derive(Debug)]
pub enum FacetEntityLinkRepairError {
    AlreadyCompleted {
        completion_marker: PathBuf,
    },
    TrustLock(FacetTrustLockError),
    MarkerPath(FacetStoreError),
    IdentityGroupRead {
        facet: String,
        report: Box<FacetEntityLinkReport>,
        source: Box<EntityStoreError>,
    },
    DirectoryScan {
        facet: String,
        report: Box<FacetEntityLinkReport>,
        source: Box<PathError>,
    },
    EntityLinkPath {
        facet: String,
        facet_entity_dir: String,
        report: Box<FacetEntityLinkReport>,
        source: Box<PathError>,
    },
    EntityLinkRead {
        facet: String,
        facet_entity_dir: String,
        report: Box<FacetEntityLinkReport>,
        source: Box<FacetStoreError>,
    },
    PreparedHistoryRead {
        facet: String,
        facet_entity_dir: String,
        journal_entity_dir: String,
        report: Box<FacetEntityLinkReport>,
        source: Box<EntityStoreError>,
    },
    EntityLinkWrite {
        facet: String,
        facet_entity_dir: String,
        report: Box<FacetEntityLinkReport>,
        source: Box<FacetWriteError>,
    },
    Incomplete {
        report: Box<FacetEntityLinkReport>,
    },
    CompletionMarkerWrite {
        report: Box<FacetEntityLinkReport>,
        source: Box<AtomicWriteError>,
    },
    JournalFacetDirectoryScan {
        report: Box<FacetEntityLinkRepairReport>,
        source: Box<PathError>,
    },
    FacetDeclarationRead {
        facet: String,
        report: Box<FacetEntityLinkRepairReport>,
        source: Box<FacetStoreError>,
    },
    CachedMarkerRead {
        facet: String,
        report: Box<FacetEntityLinkRepairReport>,
        source: Box<FacetStoreError>,
    },
    JournalIdentityGroupRead {
        report: Box<FacetEntityLinkRepairReport>,
        source: Box<EntityStoreError>,
    },
    JournalFacetRepair {
        facet: String,
        report: Box<FacetEntityLinkRepairReport>,
        source: Box<FacetEntityLinkRepairError>,
    },
    JournalWideIncomplete {
        report: Box<FacetEntityLinkRepairReport>,
    },
    JournalCompletionMarkerWrite {
        report: Box<FacetEntityLinkRepairReport>,
        source: Box<AtomicWriteError>,
    },
}

impl fmt::Display for FacetEntityLinkRepairError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyCompleted { completion_marker } => write!(
                formatter,
                "facet entity-link repair has already completed: {}",
                completion_marker.display()
            ),
            Self::TrustLock(error) => error.fmt(formatter),
            Self::MarkerPath(error) => error.fmt(formatter),
            Self::IdentityGroupRead { facet, source, .. } => {
                write!(formatter, "cannot read identity groups for facet {facet}: {source}")
            }
            Self::DirectoryScan { facet, source, .. } => {
                write!(formatter, "cannot scan facet entities for {facet}: {source}")
            }
            Self::EntityLinkPath {
                facet,
                facet_entity_dir,
                source,
                ..
            } => write!(
                formatter,
                "cannot inspect facet entity link {facet}/{facet_entity_dir}: {source}"
            ),
            Self::EntityLinkRead {
                facet,
                facet_entity_dir,
                source,
                ..
            } => write!(
                formatter,
                "cannot read facet entity link {facet}/{facet_entity_dir}: {source}"
            ),
            Self::PreparedHistoryRead {
                facet,
                facet_entity_dir,
                journal_entity_dir,
                source,
                ..
            } => write!(
                formatter,
                "cannot inspect prepared history for {facet}/{facet_entity_dir} at journal entity {journal_entity_dir}: {source}"
            ),
            Self::EntityLinkWrite {
                facet,
                facet_entity_dir,
                source,
                ..
            } => write!(
                formatter,
                "cannot write facet entity link {facet}/{facet_entity_dir}: {source}"
            ),
            Self::Incomplete { .. } => formatter.write_str(
                "facet entity-link repair is incomplete; resolve refused links before re-running",
            ),
            Self::CompletionMarkerWrite { source, .. } => {
                write!(formatter, "cannot record facet entity-link repair completion: {source}")
            }
            Self::JournalFacetDirectoryScan { source, .. } => {
                write!(formatter, "cannot scan facets for entity-link repair: {source}")
            }
            Self::FacetDeclarationRead { facet, source, .. } => {
                write!(formatter, "cannot read facet declaration {facet}: {source}")
            }
            Self::CachedMarkerRead { facet, source, .. } => {
                write!(formatter, "cannot read cached facet repair marker {facet}: {source}")
            }
            Self::JournalIdentityGroupRead { source, .. } => {
                write!(formatter, "cannot read identity groups for journal repair: {source}")
            }
            Self::JournalFacetRepair { facet, source, .. } => {
                write!(formatter, "facet entity-link repair aborted for {facet}: {source}")
            }
            Self::JournalWideIncomplete { .. } => formatter.write_str(
                "journal-wide facet entity-link repair is incomplete; resolve refused links before re-running",
            ),
            Self::JournalCompletionMarkerWrite { source, .. } => write!(
                formatter,
                "cannot record journal-wide facet entity-link repair completion: {source}"
            ),
        }
    }
}

impl Error for FacetEntityLinkRepairError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::TrustLock(error) => Some(error),
            Self::MarkerPath(error) => Some(error),
            Self::IdentityGroupRead { source, .. }
            | Self::PreparedHistoryRead { source, .. }
            | Self::JournalIdentityGroupRead { source, .. } => Some(source.as_ref()),
            Self::DirectoryScan { source, .. }
            | Self::EntityLinkPath { source, .. }
            | Self::JournalFacetDirectoryScan { source, .. } => Some(source.as_ref()),
            Self::EntityLinkRead { source, .. }
            | Self::FacetDeclarationRead { source, .. }
            | Self::CachedMarkerRead { source, .. } => Some(source.as_ref()),
            Self::EntityLinkWrite { source, .. } => Some(source.as_ref()),
            Self::CompletionMarkerWrite { source, .. }
            | Self::JournalCompletionMarkerWrite { source, .. } => Some(source.as_ref()),
            Self::JournalFacetRepair { source, .. } => Some(source.as_ref()),
            Self::AlreadyCompleted { .. }
            | Self::Incomplete { .. }
            | Self::JournalWideIncomplete { .. } => None,
        }
    }
}

/// Link every relationship directory in one facet exactly once after a clean scan.
pub fn repair_facet_entity_links(
    journal_root: &Path,
    facet: &str,
) -> Result<FacetEntityLinkReport, FacetEntityLinkRepairError> {
    let completion_marker = facet_entity_link_repair_marker_path(journal_root, facet)
        .map_err(FacetEntityLinkRepairError::MarkerPath)?;
    if path_lexists(&completion_marker)
        .map_err(|source| FacetEntityLinkRepairError::MarkerPath(FacetStoreError::from(source)))?
    {
        return Err(FacetEntityLinkRepairError::AlreadyCompleted { completion_marker });
    }
    let _trust =
        hold_facet_trust_lock(journal_root).map_err(FacetEntityLinkRepairError::TrustLock)?;
    let report = new_facet_report(facet, completion_marker);
    let groups = read_identity_group_map(journal_root).map_err(|source| {
        FacetEntityLinkRepairError::IdentityGroupRead {
            facet: facet.to_owned(),
            report: Box::new(report.clone()),
            source: Box::new(source),
        }
    })?;
    repair_facet_entity_links_with_groups(journal_root, facet, &groups, report)
}

/// Link every declared facet, reusing completed per-facet reports when available.
pub fn repair_facet_entity_links_journal_wide(
    journal_root: &Path,
) -> Result<FacetEntityLinkRepairReport, FacetEntityLinkRepairError> {
    let completion_marker = facet_entity_link_journal_wide_repair_marker_path(journal_root)
        .map_err(FacetEntityLinkRepairError::MarkerPath)?;
    if path_lexists(&completion_marker)
        .map_err(|source| FacetEntityLinkRepairError::MarkerPath(FacetStoreError::from(source)))?
    {
        return Err(FacetEntityLinkRepairError::AlreadyCompleted { completion_marker });
    }
    let _trust =
        hold_facet_trust_lock(journal_root).map_err(FacetEntityLinkRepairError::TrustLock)?;
    let mut report = FacetEntityLinkRepairReport {
        facets: Vec::new(),
        completion_marker,
    };
    let directory = facets_dir(journal_root).map_err(FacetEntityLinkRepairError::MarkerPath)?;
    let entries = list_dir_entries(&directory).map_err(|source| {
        FacetEntityLinkRepairError::JournalFacetDirectoryScan {
            report: Box::new(report.clone()),
            source: Box::new(source),
        }
    })?;
    let mut groups = None;

    for entry in entries {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let facet = entry.name.to_string_lossy().into_owned();
        let declaration = read_facet_declaration(journal_root, &facet).map_err(|source| {
            FacetEntityLinkRepairError::FacetDeclarationRead {
                facet: facet.clone(),
                report: Box::new(report.clone()),
                source: Box::new(source),
            }
        })?;
        if declaration.is_none() {
            continue;
        }
        let facet_marker = facet_entity_link_repair_marker_path(journal_root, &facet)
            .map_err(FacetEntityLinkRepairError::MarkerPath)?;
        if path_lexists(&facet_marker).map_err(|source| {
            FacetEntityLinkRepairError::CachedMarkerRead {
                facet: facet.clone(),
                report: Box::new(report.clone()),
                source: Box::new(FacetStoreError::from(source)),
            }
        })? {
            let cached = read_facet_marker(&facet_marker).map_err(|source| {
                FacetEntityLinkRepairError::CachedMarkerRead {
                    facet: facet.clone(),
                    report: Box::new(report.clone()),
                    source: Box::new(source),
                }
            })?;
            report.facets.push(cached);
            continue;
        }
        let groups = match &groups {
            Some(groups) => groups,
            None => {
                groups = Some(read_identity_group_map(journal_root).map_err(|source| {
                    FacetEntityLinkRepairError::JournalIdentityGroupRead {
                        report: Box::new(report.clone()),
                        source: Box::new(source),
                    }
                })?);
                groups
                    .as_ref()
                    .expect("identity groups were just initialized")
            }
        };
        let facet_report = new_facet_report(facet.as_str(), facet_marker);
        match repair_facet_entity_links_with_groups(journal_root, &facet, groups, facet_report) {
            Ok(facet_report) => report.facets.push(facet_report),
            Err(FacetEntityLinkRepairError::Incomplete {
                report: facet_report,
            }) => {
                report.facets.push(*facet_report);
            }
            Err(source) => {
                if let Some(partial) = source.partial_facet_report() {
                    report.facets.push(partial.clone());
                }
                return Err(FacetEntityLinkRepairError::JournalFacetRepair {
                    facet,
                    report: Box::new(report),
                    source: Box::new(source),
                });
            }
        }
    }

    if report
        .facets
        .iter()
        .any(FacetEntityLinkReport::has_refusals)
    {
        return Err(FacetEntityLinkRepairError::JournalWideIncomplete {
            report: Box::new(report),
        });
    }
    write_journal_wide_marker(&report).map_err(|source| {
        FacetEntityLinkRepairError::JournalCompletionMarkerWrite {
            report: Box::new(report.clone()),
            source: Box::new(source),
        }
    })?;
    Ok(report)
}

impl FacetEntityLinkRepairError {
    fn partial_facet_report(&self) -> Option<&FacetEntityLinkReport> {
        match self {
            Self::IdentityGroupRead { report, .. }
            | Self::DirectoryScan { report, .. }
            | Self::EntityLinkPath { report, .. }
            | Self::EntityLinkRead { report, .. }
            | Self::PreparedHistoryRead { report, .. }
            | Self::EntityLinkWrite { report, .. }
            | Self::Incomplete { report }
            | Self::CompletionMarkerWrite { report, .. } => Some(report),
            Self::AlreadyCompleted { .. }
            | Self::TrustLock(_)
            | Self::MarkerPath(_)
            | Self::JournalFacetDirectoryScan { .. }
            | Self::FacetDeclarationRead { .. }
            | Self::CachedMarkerRead { .. }
            | Self::JournalIdentityGroupRead { .. }
            | Self::JournalFacetRepair { .. }
            | Self::JournalWideIncomplete { .. }
            | Self::JournalCompletionMarkerWrite { .. } => None,
        }
    }
}

impl FacetEntityLinkReport {
    fn has_refusals(&self) -> bool {
        self.branches.iter().any(|branch| {
            matches!(
                branch,
                FacetEntityLinkRepairBranch::RefusedUnparseable { .. }
                    | FacetEntityLinkRepairBranch::RefusedPending { .. }
            )
        })
    }
}

fn repair_facet_entity_links_with_groups(
    journal_root: &Path,
    facet: &str,
    groups: &EntityIdentityGroupMap,
    mut report: FacetEntityLinkReport,
) -> Result<FacetEntityLinkReport, FacetEntityLinkRepairError> {
    let entities_dir = facet_entities_dir(journal_root, facet).map_err(|source| {
        FacetEntityLinkRepairError::DirectoryScan {
            facet: facet.to_owned(),
            report: Box::new(report.clone()),
            source: Box::new(path_error(source)),
        }
    })?;
    let entries = list_dir_entries(&entities_dir).map_err(|source| {
        FacetEntityLinkRepairError::DirectoryScan {
            facet: facet.to_owned(),
            report: Box::new(report.clone()),
            source: Box::new(source),
        }
    })?;

    for entry in entries {
        let facet_entity_dir = entry.name.to_string_lossy().into_owned();
        if entry.kind != DirEntryKind::Directory {
            report
                .branches
                .push(FacetEntityLinkRepairBranch::SkippedNotAnEntity { facet_entity_dir });
            continue;
        }
        let relationship_path = facet_entity_link_path(journal_root, facet, &facet_entity_dir)
            .map_err(|source| FacetEntityLinkRepairError::EntityLinkPath {
                facet: facet.to_owned(),
                facet_entity_dir: facet_entity_dir.clone(),
                report: Box::new(report.clone()),
                source: Box::new(path_error(source)),
            })?;
        if !path_lexists(&relationship_path).map_err(|source| {
            FacetEntityLinkRepairError::EntityLinkPath {
                facet: facet.to_owned(),
                facet_entity_dir: facet_entity_dir.clone(),
                report: Box::new(report.clone()),
                source: Box::new(source),
            }
        })? {
            report
                .branches
                .push(FacetEntityLinkRepairBranch::SkippedNotAnEntity { facet_entity_dir });
            continue;
        }
        let snapshot = match read_facet_entity_link(journal_root, facet, &facet_entity_dir) {
            Ok(snapshot) => snapshot,
            Err(source) if is_unparseable_link(&source) => {
                report
                    .branches
                    .push(FacetEntityLinkRepairBranch::RefusedUnparseable {
                        facet_entity_dir,
                        path: relationship_path,
                        detail: source.to_string(),
                    });
                continue;
            }
            Err(source) => {
                return Err(FacetEntityLinkRepairError::EntityLinkRead {
                    facet: facet.to_owned(),
                    facet_entity_dir,
                    report: Box::new(report),
                    source: Box::new(source),
                });
            }
        };
        let Some(snapshot) = snapshot else {
            report
                .branches
                .push(FacetEntityLinkRepairBranch::SkippedNotAnEntity { facet_entity_dir });
            continue;
        };
        let Some(journal_entity_dirs) = groups.groups.get(&facet_entity_dir) else {
            report
                .branches
                .push(FacetEntityLinkRepairBranch::Unmatched { facet_entity_dir });
            continue;
        };
        if journal_entity_dirs.len() != 1 {
            report
                .branches
                .push(FacetEntityLinkRepairBranch::MultiMatched {
                    facet_entity_dir,
                    journal_entity_dirs: journal_entity_dirs.clone(),
                });
            continue;
        }
        let journal_entity_dir = journal_entity_dirs[0].clone();
        let prepared =
            read_prepared_history(journal_root, &journal_entity_dir).map_err(|source| {
                FacetEntityLinkRepairError::PreparedHistoryRead {
                    facet: facet.to_owned(),
                    facet_entity_dir: facet_entity_dir.clone(),
                    journal_entity_dir: journal_entity_dir.clone(),
                    report: Box::new(report.clone()),
                    source: Box::new(source),
                }
            })?;
        if !prepared.is_empty() {
            report
                .branches
                .push(FacetEntityLinkRepairBranch::RefusedPending {
                    facet_entity_dir,
                    journal_entity_dir,
                });
            continue;
        }
        let mut relationship = snapshot
            .value()
            .as_object()
            .expect("facet entity link snapshots always contain objects")
            .clone();
        relationship.remove("entity_id");
        save_facet_entity_link(
            journal_root,
            facet,
            &facet_entity_dir,
            &facet_entity_dir,
            &relationship,
        )
        .map_err(|source| FacetEntityLinkRepairError::EntityLinkWrite {
            facet: facet.to_owned(),
            facet_entity_dir: facet_entity_dir.clone(),
            report: Box::new(report.clone()),
            source: Box::new(source),
        })?;
        report.branches.push(FacetEntityLinkRepairBranch::Linked {
            journal_entity_id: facet_entity_dir.clone(),
            facet_entity_dir,
        });
    }

    if report.has_refusals() {
        return Err(FacetEntityLinkRepairError::Incomplete {
            report: Box::new(report),
        });
    }
    write_facet_marker(&report).map_err(|source| {
        FacetEntityLinkRepairError::CompletionMarkerWrite {
            report: Box::new(report.clone()),
            source: Box::new(source),
        }
    })?;
    Ok(report)
}

fn new_facet_report(facet: &str, completion_marker: PathBuf) -> FacetEntityLinkReport {
    FacetEntityLinkReport {
        facet: facet.to_owned(),
        branches: Vec::new(),
        completion_marker,
    }
}

fn write_facet_marker(report: &FacetEntityLinkReport) -> Result<(), AtomicWriteError> {
    write_marker(&report.completion_marker, report)
}

fn write_journal_wide_marker(report: &FacetEntityLinkRepairReport) -> Result<(), AtomicWriteError> {
    write_marker(&report.completion_marker, report)
}

fn write_marker<T: Serialize>(path: &Path, report: &T) -> Result<(), AtomicWriteError> {
    write_json(
        path,
        &serde_json::json!({
            "completed_at": Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            "report": report,
        }),
        JsonWriteOptions {
            mode: Some(0o600),
            indent: Some(2),
            sort_keys: false,
        },
    )
}

fn read_facet_marker(path: &Path) -> Result<FacetEntityLinkReport, FacetStoreError> {
    let marker: FacetEntityLinkCompletionMarker = read_json(
        path,
        marker_default(),
        solstone_core_journal_io::MalformedPolicy::Raise,
    )?;
    let FacetEntityLinkCompletionMarker {
        completed_at,
        report,
    } = marker;
    let _ = completed_at;
    Ok(report)
}

fn marker_default() -> FacetEntityLinkCompletionMarker {
    FacetEntityLinkCompletionMarker {
        completed_at: String::new(),
        report: FacetEntityLinkReport {
            facet: String::new(),
            branches: Vec::new(),
            completion_marker: PathBuf::new(),
        },
    }
}

fn is_unparseable_link(error: &FacetStoreError) -> bool {
    matches!(
        error,
        FacetStoreError::EntityLinkNotObject { .. }
            | FacetStoreError::Read(ReadError::Malformed(_))
    )
}

fn path_error(error: FacetStoreError) -> PathError {
    match error {
        FacetStoreError::Path(error) => error,
        _ => unreachable!("facet path helpers only return path errors"),
    }
}

#[derive(Deserialize)]
struct FacetEntityLinkCompletionMarker {
    completed_at: String,
    report: FacetEntityLinkReport,
}
