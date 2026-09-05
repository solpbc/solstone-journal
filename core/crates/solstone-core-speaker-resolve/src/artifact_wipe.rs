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
    pub segment_embeddings: WipeCategory,
    pub speaker_labels: WipeCategory,
    pub speaker_corrections: WipeCategory,
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

/// Report, and optionally remove, all legacy speaker artifacts.
pub fn wipe_speaker_artifacts(
    journal_root: &Path,
    dry_run: bool,
) -> Result<WipeReport, ArtifactWipeError> {
    let _trust = (!dry_run)
        .then(|| solstone_core_entity::hold_entity_trust_lock(journal_root))
        .transpose()
        .map_err(|error| match error {
            solstone_core_entity::EntityTrustLockError::Path(error) => {
                ArtifactWipeError::Path(error)
            }
            solstone_core_entity::EntityTrustLockError::Lock(error) => {
                ArtifactWipeError::Lock(error)
            }
        })?;
    let mut report = WipeReport {
        dry_run,
        ..WipeReport::default()
    };
    for segment in chronicle_segments(journal_root)? {
        for entry in directory_files(&segment)? {
            if !entry
                .extension()
                .is_some_and(|extension| extension == "npz")
            {
                continue;
            }
            record_path(
                journal_root,
                &entry,
                dry_run,
                &mut report.segment_embeddings,
            )?;
        }
        for directory in ["agents", "talents"] {
            let root = segment.join(directory);
            record_path(
                journal_root,
                &root.join("speaker_labels.json"),
                dry_run,
                &mut report.speaker_labels,
            )?;
            record_path(
                journal_root,
                &root.join("speaker_corrections.json"),
                dry_run,
                &mut report.speaker_corrections,
            )?;
        }
    }
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
    report.total_files = report.segment_embeddings.count
        + report.speaker_labels.count
        + report.speaker_corrections.count
        + report.entity_voiceprints.count
        + report.owner_centroids.count
        + report.owner_candidate.count;
    report.total_bytes = report.segment_embeddings.bytes
        + report.speaker_labels.bytes
        + report.speaker_corrections.bytes
        + report.entity_voiceprints.bytes
        + report.owner_centroids.bytes
        + report.owner_candidate.bytes;
    Ok(report)
}

fn entity_ids(journal_root: &Path) -> Result<Vec<String>, ArtifactWipeError> {
    let directory = journal_root.join("entities");
    let mut entity_ids = Vec::new();
    for entry in directory_entries(&directory)? {
        if let Some(entity_id) = entry.file_name().and_then(|value| value.to_str()) {
            entity_ids.push(entity_id.to_owned());
        }
    }
    entity_ids.sort();
    Ok(entity_ids)
}

fn chronicle_segments(journal_root: &Path) -> Result<Vec<std::path::PathBuf>, ArtifactWipeError> {
    let mut segments = Vec::new();
    for day in directory_entries(&journal_root.join("chronicle"))? {
        for stream in directory_entries(&day)? {
            segments.extend(directory_entries(&stream)?);
        }
    }
    segments.sort();
    Ok(segments)
}

fn directory_entries(directory: &Path) -> Result<Vec<std::path::PathBuf>, ArtifactWipeError> {
    Ok(directory_children(directory)?
        .into_iter()
        .filter_map(|(path, file_type)| file_type.is_dir().then_some(path))
        .collect())
}

fn directory_files(directory: &Path) -> Result<Vec<std::path::PathBuf>, ArtifactWipeError> {
    Ok(directory_children(directory)?
        .into_iter()
        .filter_map(|(path, file_type)| file_type.is_file().then_some(path))
        .collect())
}

fn directory_children(
    directory: &Path,
) -> Result<Vec<(std::path::PathBuf, fs::FileType)>, ArtifactWipeError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(ArtifactWipeError::Read {
                path: directory.display().to_string(),
                source,
            });
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ArtifactWipeError::Read {
            path: directory.display().to_string(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|source| ArtifactWipeError::Read {
                path: path.display().to_string(),
                source,
            })?;
        paths.push((path, file_type));
    }
    paths.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(paths)
}

fn record_path(
    journal_root: &Path,
    path: &Path,
    dry_run: bool,
    category: &mut WipeCategory,
) -> Result<(), ArtifactWipeError> {
    let relative = path
        .strip_prefix(journal_root)
        .expect("wipe paths are constructed under the journal root")
        .to_string_lossy();
    record_entry(journal_root, &relative, dry_run, category)
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn test_root() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "solstone-core-speaker-wipe-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ))
    }

    fn write(root: &Path, relative: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("fixture path has parent"))
            .expect("create fixture parent");
        fs::write(path, b"fixture").expect("write fixture");
    }

    #[test]
    fn wipe_covers_all_legacy_speaker_artifact_categories() {
        let root = test_root();
        let paths = [
            "chronicle/20260809/main/120000_1/audio.npz",
            "chronicle/20260809/main/120000_1/agents/speaker_labels.json",
            "chronicle/20260809/main/120000_1/talents/speaker_labels.json",
            "chronicle/20260809/main/120000_1/agents/speaker_corrections.json",
            "chronicle/20260809/main/120000_1/talents/speaker_corrections.json",
            "entities/person/voiceprints.npz",
            "entities/person/owner_centroid.npz",
            "awareness/owner_candidate.npz",
        ];
        for path in paths {
            write(&root, path);
        }

        let dry_run = wipe_speaker_artifacts(&root, true).expect("dry-run wipe");
        assert!(dry_run.dry_run);
        assert_eq!(dry_run.segment_embeddings.count, 1);
        assert_eq!(dry_run.speaker_labels.count, 2);
        assert_eq!(dry_run.speaker_corrections.count, 2);
        assert_eq!(dry_run.entity_voiceprints.count, 1);
        assert_eq!(dry_run.owner_centroids.count, 1);
        assert_eq!(dry_run.owner_candidate.count, 1);
        assert_eq!(dry_run.total_files, paths.len());
        assert!(paths.iter().all(|path| root.join(path).is_file()));

        let committed = wipe_speaker_artifacts(&root, false).expect("commit wipe");
        assert!(!committed.dry_run);
        assert_eq!(committed.segment_embeddings.count, 1);
        assert_eq!(committed.speaker_labels.count, 2);
        assert_eq!(committed.speaker_corrections.count, 2);
        assert_eq!(committed.entity_voiceprints.count, 1);
        assert_eq!(committed.owner_centroids.count, 1);
        assert_eq!(committed.owner_candidate.count, 1);
        assert_eq!(committed.total_files, paths.len());
        assert!(paths.iter().all(|path| !root.join(path).exists()));

        fs::remove_dir_all(root).expect("remove fixture root");
    }
}
