// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use solstone_core_journal_io::{PathOrDay, day_dirs, iter_segments};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentsToTalentsMigrationReport {
    pub discovered: usize,
    pub moved: usize,
    pub skipped: usize,
    pub errors: usize,
    pub collisions: usize,
}
#[derive(Debug)]
pub struct TalentStorageError(pub String);
impl std::fmt::Display for TalentStorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for TalentStorageError {}

pub fn rename_agents_to_talents(
    journal: &Path,
    dry_run: bool,
) -> Result<AgentsToTalentsMigrationReport, TalentStorageError> {
    let mut planned = Vec::new();
    let mut skipped = 0;
    add_pair(
        &mut planned,
        &mut skipped,
        journal.join("agents"),
        journal.join("talents"),
    );
    add_pair(
        &mut planned,
        &mut skipped,
        journal.join("health/agents.json"),
        journal.join("health/talents.json"),
    );
    let mut days = day_dirs(journal)
        .map_err(|error| TalentStorageError(error.to_string()))?
        .into_iter()
        .collect::<Vec<_>>();
    days.sort();
    for (_, day_dir) in &days {
        add_pair(
            &mut planned,
            &mut skipped,
            day_dir.join("agents"),
            day_dir.join("talents"),
        );
        let segments = iter_segments(journal, PathOrDay::Directory(day_dir))
            .map_err(|error| TalentStorageError(error.to_string()))?;
        for segment in segments {
            add_pair(
                &mut planned,
                &mut skipped,
                segment.path.join("agents"),
                segment.path.join("talents"),
            );
        }
    }
    let collisions = planned.iter().filter(|(_, dst)| dst.exists()).count();
    let mut report = AgentsToTalentsMigrationReport {
        discovered: planned.len(),
        skipped,
        collisions,
        ..Default::default()
    };
    if collisions > 0 {
        return Ok(report);
    }
    for (source, destination) in planned {
        if dry_run {
            report.moved += 1;
            continue;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(io)?;
        }
        match fs::rename(&source, &destination) {
            Ok(()) => report.moved += 1,
            Err(error) => {
                report.errors += 1;
                let _ = error;
            }
        }
    }
    Ok(report)
}
fn add_pair(
    planned: &mut Vec<(PathBuf, PathBuf)>,
    skipped: &mut usize,
    source: PathBuf,
    destination: PathBuf,
) {
    if source.exists() {
        planned.push((source, destination))
    } else if destination.exists() {
        *skipped += 1
    }
}
fn io(error: std::io::Error) -> TalentStorageError {
    TalentStorageError(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[test]
    fn collision_aborts_all_moves() {
        let temp = tempdir().unwrap();
        fs::create_dir_all(temp.path().join("agents")).unwrap();
        fs::create_dir_all(temp.path().join("talents")).unwrap();
        let report = rename_agents_to_talents(temp.path(), false).unwrap();
        assert_eq!(report.collisions, 1);
        assert_eq!(report.errors, 0);
        assert!(temp.path().join("agents").exists());
    }
    #[test]
    fn moves_root_day_segment_and_health_paths() {
        let temp = tempdir().unwrap();
        for path in [
            "agents",
            "chronicle/20260101/agents",
            "chronicle/20260101/080000_300/agents",
            "health",
        ] {
            fs::create_dir_all(temp.path().join(path)).unwrap();
        }
        fs::write(temp.path().join("health/agents.json"), b"{}").unwrap();
        let report = rename_agents_to_talents(temp.path(), false).unwrap();
        assert_eq!(report.moved, 4);
        assert_eq!(report.errors, 0);
        assert!(temp.path().join("talents").exists());
        assert!(temp.path().join("chronicle/20260101/talents").exists());
        assert!(
            temp.path()
                .join("chronicle/20260101/080000_300/talents")
                .exists()
        );
    }
}
