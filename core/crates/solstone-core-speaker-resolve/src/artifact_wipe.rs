// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What a speakers wipe takes: every file the journal keeps about voices.
//!
//! The wipe moves them to the trash as one item ([`crate::speakers_trash`]),
//! where they wait to be recovered or erased. A dry run reports the same set.
//!
//! Taken: each live recording's embeddings (its top-level `*.npz`) and its
//! speaker labels and corrections, in both segment layouts; each entity's
//! voiceprints and owner centroid; the owner candidate; and the candidate
//! pool, which holds voice centroids.
//!
//! Left alone, as the owner's own decisions or not about voice: the owner's
//! answers about who spoke a line (`talents/owner_line_labels.json`), the
//! rejections in `awareness/current.json`, everything under `speakers/`, and
//! `talents/speakers.json`. The discovery groups are a cache, cleared when the
//! wipe moves and again when it comes back.

use std::fs;
use std::io;
use std::path::Path;

use serde::Serialize;
use solstone_core_journal_io::{PathOrDay, StreamLocation, day_dirs, iter_segments};
use thiserror::Error;

/// The owner candidate's snapshot.
pub const OWNER_CANDIDATE: &str = "awareness/owner_candidate.npz";

/// The candidate pool: voice centroids gathered across recordings.
pub const SPEAKER_CANDIDATES: &str = "awareness/speaker_candidates.json";

/// One wipe category's deterministic receipt.
#[derive(Debug, Default, Serialize)]
pub struct WipeCategory {
    pub count: usize,
    pub bytes: u64,
    pub paths: Vec<String>,
}

/// What a wipe takes, or would take, by category.
#[derive(Debug, Default, Serialize)]
pub struct WipeReport {
    pub dry_run: bool,
    pub segment_embeddings: WipeCategory,
    pub speaker_labels: WipeCategory,
    pub speaker_corrections: WipeCategory,
    pub entity_voiceprints: WipeCategory,
    pub owner_centroids: WipeCategory,
    pub owner_candidate: WipeCategory,
    pub speaker_candidates: WipeCategory,
    pub total_files: usize,
    pub total_bytes: u64,
}

/// Failure while finding the files a wipe takes.
#[derive(Debug, Error)]
pub enum ArtifactWipeError {
    #[error("artifact path failed: {0}")]
    Path(#[from] solstone_core_journal_io::PathError),
    #[error("artifact directory read failed at {path}: {source}")]
    Read { path: String, source: io::Error },
    #[error("a name in your journal can't be read: {path}")]
    NotUtf8 { path: String },
}

/// Report what a wipe would take, moving nothing.
pub fn dry_run(journal_root: &Path) -> Result<WipeReport, ArtifactWipeError> {
    let paths = wipe_paths(journal_root)?;
    Ok(report(true, &paths, |path| {
        fs::symlink_metadata(journal_root.join(path)).map_or(0, |metadata| metadata.len())
    }))
}

/// Every file a wipe takes, journal-relative and sorted, so the files of one
/// recording sit together.
pub fn wipe_paths(journal_root: &Path) -> Result<Vec<String>, ArtifactWipeError> {
    let mut paths = Vec::new();
    segment_files(journal_root, &mut paths)?;
    for entity in entity_dirs(journal_root)? {
        for name in ["voiceprints.npz", "owner_centroid.npz"] {
            let path = format!("entities/{entity}/{name}");
            if is_file(journal_root, &path) {
                paths.push(path);
            }
        }
    }
    for path in [OWNER_CANDIDATE, SPEAKER_CANDIDATES] {
        if is_file(journal_root, path) {
            paths.push(path.to_owned());
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// The report for `paths`, each sized by `size`.
pub(crate) fn report(dry_run: bool, paths: &[String], size: impl Fn(&str) -> u64) -> WipeReport {
    let mut report = WipeReport {
        dry_run,
        ..WipeReport::default()
    };
    for path in paths {
        let category = if path.starts_with("chronicle/") {
            if path.ends_with(".npz") {
                &mut report.segment_embeddings
            } else if path.ends_with("/speaker_labels.json") {
                &mut report.speaker_labels
            } else {
                &mut report.speaker_corrections
            }
        } else if path.starts_with("entities/") {
            if path.ends_with("/voiceprints.npz") {
                &mut report.entity_voiceprints
            } else {
                &mut report.owner_centroids
            }
        } else if path == OWNER_CANDIDATE {
            &mut report.owner_candidate
        } else {
            &mut report.speaker_candidates
        };
        let bytes = size(path);
        category.count += 1;
        category.bytes += bytes;
        category.paths.push(path.clone());
        report.total_files += 1;
        report.total_bytes += bytes;
    }
    report
}

/// Each live recording's voice files, in both layouts.
fn segment_files(journal_root: &Path, paths: &mut Vec<String>) -> Result<(), ArtifactWipeError> {
    let mut days: Vec<_> = day_dirs(journal_root)?.into_iter().collect();
    days.sort();
    for (day, directory) in days {
        for segment in iter_segments(journal_root, PathOrDay::Directory(&directory))? {
            let name = utf8(segment.name(), segment.path())?;
            let rel = match segment.stream() {
                StreamLocation::Direct => format!("chronicle/{day}/{name}"),
                StreamLocation::Named(stream) => {
                    format!("chronicle/{day}/{}/{name}", utf8(stream, segment.path())?)
                }
            };
            // A recording on its way out, or only the marker one left, isn't
            // the wipe's to take.
            if !crate::speakers_trash::live_segment(journal_root, &rel) {
                continue;
            }
            for (child, file_type) in directory_children(segment.path())? {
                let child = utf8(&child, segment.path())?;
                if file_type.is_file() && child.ends_with(".npz") {
                    paths.push(format!("{rel}/{child}"));
                }
            }
            for directory in ["agents", "talents"] {
                for name in ["speaker_labels.json", "speaker_corrections.json"] {
                    let path = format!("{rel}/{directory}/{name}");
                    if is_file(journal_root, &path) {
                        paths.push(path);
                    }
                }
            }
        }
    }
    Ok(())
}

fn utf8<'a>(name: &'a std::ffi::OsStr, at: &Path) -> Result<&'a str, ArtifactWipeError> {
    name.to_str().ok_or_else(|| ArtifactWipeError::NotUtf8 {
        path: at.display().to_string(),
    })
}

/// Every folder under `entities/`, whether or not it holds a live entity.
fn entity_dirs(journal_root: &Path) -> Result<Vec<String>, ArtifactWipeError> {
    let mut dirs: Vec<String> = directory_children(&journal_root.join("entities"))?
        .into_iter()
        .filter(|(_, file_type)| file_type.is_dir())
        .filter_map(|(name, _)| name.to_str().map(str::to_owned))
        .collect();
    dirs.sort();
    Ok(dirs)
}

/// A regular file, not followed through a link.
fn is_file(journal_root: &Path, relative: &str) -> bool {
    fs::symlink_metadata(journal_root.join(relative)).is_ok_and(|metadata| metadata.is_file())
}

fn directory_children(
    directory: &Path,
) -> Result<Vec<(std::ffi::OsString, fs::FileType)>, ArtifactWipeError> {
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
    let mut children = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ArtifactWipeError::Read {
            path: directory.display().to_string(),
            source,
        })?;
        let file_type = entry
            .file_type()
            .map_err(|source| ArtifactWipeError::Read {
                path: entry.path().display().to_string(),
                source,
            })?;
        children.push((entry.file_name(), file_type));
    }
    children.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(children)
}
