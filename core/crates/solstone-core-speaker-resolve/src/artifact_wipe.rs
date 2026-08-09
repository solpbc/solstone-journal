// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Mechanical cleanup of durable speaker-identity artifacts.

use std::fs;
use std::io;
use std::path::Path;

use serde::Serialize;
use solstone_core_journal_io::{
    LockError, LockOptions, Removed, hold_lock, path_lexists, remove_file,
};
use thiserror::Error;

/// One wipe category's deterministic receipt.
#[derive(Debug, Default, Serialize)]
pub struct WipeCategory {
    pub count: usize,
    pub bytes: u64,
    pub paths: Vec<String>,
}

/// Result of removing only the durable speaker-identity artifacts owned here.
#[derive(Debug, Default, Serialize)]
pub struct WipeReport {
    pub dry_run: bool,
    pub entity_voiceprints: WipeCategory,
    pub owner_centroids: WipeCategory,
    pub owner_candidate: WipeCategory,
    pub total_files: usize,
    pub total_bytes: u64,
}

/// Failure while finding or removing a speaker artifact.
#[derive(Debug, Error)]
pub enum ArtifactWipeError {
    #[error("artifact lock failed: {0}")]
    Lock(#[from] LockError),
    #[error("artifact path failed: {0}")]
    Path(#[from] solstone_core_journal_io::PathError),
    #[error("artifact directory read failed at {path}: {source}")]
    Read { path: String, source: io::Error },
}

/// Report, and optionally remove, all gated entity voiceprint/owner artifacts.
pub fn wipe_speaker_artifacts(
    journal_root: &Path,
    dry_run: bool,
) -> Result<WipeReport, ArtifactWipeError> {
    let mut report = WipeReport {
        dry_run,
        ..WipeReport::default()
    };
    for entity_id in entity_ids(journal_root)? {
        record_entry(
            journal_root,
            &format!("entities/{entity_id}/voiceprints.npz"),
            dry_run,
            &mut report.entity_voiceprints,
        )?;
        record_entry(
            journal_root,
            &format!("entities/{entity_id}/owner_centroid.npz"),
            dry_run,
            &mut report.owner_centroids,
        )?;
    }
    record_entry(
        journal_root,
        "awareness/owner_candidate.npz",
        dry_run,
        &mut report.owner_candidate,
    )?;
    report.total_files = report.entity_voiceprints.count
        + report.owner_centroids.count
        + report.owner_candidate.count;
    report.total_bytes = report.entity_voiceprints.bytes
        + report.owner_centroids.bytes
        + report.owner_candidate.bytes;
    Ok(report)
}

fn entity_ids(journal_root: &Path) -> Result<Vec<String>, ArtifactWipeError> {
    let directory = journal_root.join("entities");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(ArtifactWipeError::Read {
                path: directory.display().to_string(),
                source,
            });
        }
    };
    let mut entity_ids = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ArtifactWipeError::Read {
            path: directory.display().to_string(),
            source,
        })?;
        if entry
            .file_type()
            .map_err(|source| ArtifactWipeError::Read {
                path: entry.path().display().to_string(),
                source,
            })?
            .is_dir()
            && let Some(entity_id) = entry.file_name().to_str()
        {
            entity_ids.push(entity_id.to_owned());
        }
    }
    entity_ids.sort();
    Ok(entity_ids)
}

fn record_entry(
    journal_root: &Path,
    relative: &str,
    dry_run: bool,
    category: &mut WipeCategory,
) -> Result<(), ArtifactWipeError> {
    let path = journal_root.join(relative);
    if !path_lexists(&path)? || !path.is_file() {
        return Ok(());
    }
    let bytes = fs::metadata(&path)
        .map_err(|source| ArtifactWipeError::Read {
            path: path.display().to_string(),
            source,
        })?
        .len();
    category.count += 1;
    category.bytes += bytes;
    category.paths.push(relative.to_owned());
    if dry_run {
        return Ok(());
    }
    let _lock = hold_lock(&path, LockOptions::default())?;
    if matches!(remove_file(journal_root, relative)?, Removed::AlreadyAbsent) {
        category.count -= 1;
        category.bytes -= bytes;
        category.paths.pop();
    }
    Ok(())
}
