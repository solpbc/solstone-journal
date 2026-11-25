// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only local media, device space and proposed relief.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use nix::sys::statvfs::statvfs;
use solstone_core_format::segment::{is_date_key, segment_key};
use solstone_core_journal_io::paths::SegmentLayout;
use solstone_core_retention::content::{ContentName, JournalMedia, MediaClassifier};

use crate::run::{CurrentOffloadAvailability, CurrentOffloadDecision};

pub const MIN_FLOOR_BYTES: u64 = solstone_core_journal_io::JOURNAL_DISK_MIN_FLOOR_BYTES;
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RawMediaDayUsage {
    pub day: String,
    pub bytes: u64,
    pub files: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RawMediaUsage {
    pub total_bytes: u64,
    pub total_files: u64,
    pub per_day: Vec<RawMediaDayUsage>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SuggestedOffloadDefaults {
    pub budget_bytes: u64,
    pub floor_bytes: u64,
}

/// Exact local locator; the parsed segment key is grouping metadata only.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalMediaKey {
    pub day: String,
    pub stream: String,
    pub segment: String,
    pub segment_dir: String,
    pub layout: SegmentLayout,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalMediaFile {
    pub key: LocalMediaKey,
    pub bytes: u64,
}

/// Metadata-only local population. Neither membership nor size grants release authority.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawMediaPopulation {
    pub files: Vec<LocalMediaFile>,
    pub unknown_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy)]
pub enum MediaTotalOverflow {
    Bytes,
    Files,
}

impl std::fmt::Display for MediaTotalOverflow {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Bytes => "media byte total overflow",
            Self::Files => "media file total overflow",
        })
    }
}

impl RawMediaPopulation {
    pub fn complete(&self) -> bool {
        self.unknown_paths.is_empty()
    }

    /// Sum only the observed plain media; incomplete coverage remains explicit on this population.
    pub fn known_usage(&self) -> Result<RawMediaUsage, String> {
        self.checked_known_usage()
            .map_err(|error| error.to_string())
    }

    pub(crate) fn checked_known_usage(&self) -> Result<RawMediaUsage, MediaTotalOverflow> {
        let mut days = BTreeMap::<String, (u64, u64)>::new();
        for file in &self.files {
            let day = days.entry(file.key.day.clone()).or_default();
            day.0 = day
                .0
                .checked_add(file.bytes)
                .ok_or(MediaTotalOverflow::Bytes)?;
            day.1 = day.1.checked_add(1).ok_or(MediaTotalOverflow::Files)?;
        }
        let mut usage = RawMediaUsage {
            total_bytes: 0,
            total_files: 0,
            per_day: Vec::new(),
        };
        for (day, (bytes, files)) in days {
            usage.total_bytes = usage
                .total_bytes
                .checked_add(bytes)
                .ok_or(MediaTotalOverflow::Bytes)?;
            usage.total_files = usage
                .total_files
                .checked_add(files)
                .ok_or(MediaTotalOverflow::Files)?;
            usage.per_day.push(RawMediaDayUsage { day, bytes, files });
        }
        Ok(usage)
    }

    /// A complete local total cannot be inferred from a partial traversal.
    pub fn usage(&self) -> Result<RawMediaUsage, String> {
        if !self.complete() {
            return Err("local media coverage is incomplete".into());
        }
        self.known_usage()
    }
}

/// Numeric pending-relief projection retaining actual local usage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposedRelief {
    pub local: RawMediaUsage,
    pub eligible_bytes: u64,
    pub eligible_files: u64,
    pub projected_local_bytes: u64,
}

/// Join a current decision with the same journal's complete local population.
/// None means the owning inspection was unavailable. The subtraction projects
/// proposed relief; it does not measure deletion or grant release authority.
pub fn project_current_offload(
    local: &RawMediaPopulation,
    decision: &CurrentOffloadDecision,
) -> Result<Option<ProposedRelief>, String> {
    let usage = local.usage()?;
    let mut files = BTreeMap::new();
    for file in &local.files {
        let key = (
            file.key.day.as_str(),
            file.key.stream.as_str(),
            file.key.segment_dir.as_str(),
            file.key.layout,
            file.key.name.as_str(),
        );
        if files.insert(key, file.bytes).is_some() {
            return Err("local media observation is duplicated".into());
        }
    }
    match decision.availability() {
        CurrentOffloadAvailability::Available => {}
        CurrentOffloadAvailability::Unavailable => return Ok(None),
    }
    let mut credited = BTreeSet::new();
    let (mut eligible_bytes, mut eligible_files) = (0u64, 0u64);
    for file in decision.eligible() {
        let target = file.target();
        let key = (
            target.day.as_str(),
            target.stream.as_str(),
            target.dir.as_str(),
            file.layout(),
            file.name(),
        );
        if files.get(&key).copied() != Some(file.bytes()) || !credited.insert(key) {
            continue;
        }
        eligible_bytes = eligible_bytes
            .checked_add(file.bytes())
            .ok_or("eligible media byte total overflow")?;
        eligible_files = eligible_files
            .checked_add(1)
            .ok_or("eligible media file total overflow")?;
    }
    let projected_local_bytes = usage
        .total_bytes
        .checked_sub(eligible_bytes)
        .ok_or("eligible relief exceeds local media total")?;
    Ok(Some(ProposedRelief {
        local: usage,
        eligible_bytes,
        eligible_files,
        projected_local_bytes,
    }))
}

fn children(path: &Path, population: &mut RawMediaPopulation) -> Vec<fs::DirEntry> {
    // Check every directory we enter. This scan is advisory, not filesystem admission for deletion.
    if !fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        population.unknown_paths.push(path.into());
        return Vec::new();
    }
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => {
            population.unknown_paths.push(path.into());
            return Vec::new();
        }
    };
    entries
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry),
            Err(_) => {
                population.unknown_paths.push(path.into());
                None
            }
        })
        .collect()
}

fn plain_directory(entry: &fs::DirEntry, population: &mut RawMediaPopulation) -> bool {
    match entry.file_type() {
        Ok(kind) => kind.is_dir(),
        Err(_) => {
            population.unknown_paths.push(entry.path());
            false
        }
    }
}

fn scan_segment(
    entry: &fs::DirEntry,
    day: &str,
    stream: &str,
    layout: SegmentLayout,
    population: &mut RawMediaPopulation,
) {
    let Some(segment_dir) = entry.file_name().to_str().map(str::to_owned) else {
        population.unknown_paths.push(entry.path());
        return;
    };
    let Some(segment) = segment_key(&segment_dir) else {
        return;
    };
    for file in children(&entry.path(), population) {
        let name = file.file_name();
        let Some(name) = name.to_str() else {
            population.unknown_paths.push(file.path());
            continue;
        };
        if !ContentName::new(name).is_some_and(|name| JournalMedia.is_owner_media(&name)) {
            continue;
        }
        match fs::symlink_metadata(file.path()) {
            Ok(metadata) if metadata.file_type().is_file() => {
                population.files.push(LocalMediaFile {
                    key: LocalMediaKey {
                        day: day.into(),
                        stream: stream.into(),
                        segment: segment.clone(),
                        segment_dir: segment_dir.clone(),
                        layout,
                        name: name.into(),
                    },
                    bytes: metadata.len(),
                })
            }
            Ok(_) => {} // Symlinks and nested metadata directories are outside the plain population.
            Err(_) => population.unknown_paths.push(file.path()),
        }
    }
}

/// Enumerate plain owner media without hashes or following symlink entries.
pub fn measure_local_media(journal: &Path) -> RawMediaPopulation {
    let mut population = RawMediaPopulation::default();
    if !fs::symlink_metadata(journal).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        population.unknown_paths.push(journal.into());
        return population;
    }
    let chronicle = journal.join("chronicle");
    match fs::symlink_metadata(&chronicle) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return population,
        _ => {}
    }
    for day in children(&chronicle, &mut population) {
        let name = day.file_name();
        let Some(day_name) = name.to_str() else {
            population.unknown_paths.push(day.path());
            continue;
        };
        if !is_date_key(day_name) || !plain_directory(&day, &mut population) {
            continue;
        }
        for child in children(&day.path(), &mut population) {
            let name = child.file_name();
            let Some(name) = name.to_str() else {
                population.unknown_paths.push(child.path());
                continue;
            };
            // Owned stages are siblings of live segments, including names containing a segment key.
            if name.starts_with(".tmp-offload-") || !plain_directory(&child, &mut population) {
                continue;
            }
            if segment_key(name).is_some() {
                scan_segment(
                    &child,
                    day_name,
                    "_default",
                    SegmentLayout::Direct,
                    &mut population,
                );
            } else {
                for segment in children(&child.path(), &mut population) {
                    if segment
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.starts_with(".tmp-offload-"))
                        || !plain_directory(&segment, &mut population)
                    {
                        continue;
                    }
                    scan_segment(
                        &segment,
                        day_name,
                        name,
                        SegmentLayout::Named,
                        &mut population,
                    );
                }
            }
        }
    }
    population.files.sort_by(|a, b| a.key.cmp(&b.key));
    population.unknown_paths.sort();
    population.unknown_paths.dedup();
    population
}
// `statvfs`'s block counts are u64 on Linux and u32 on Darwin, so both of these
// only compile on Linux unconverted. See the note in
// `solstone-core-backup-web::measurement` for why the lint has to be silenced
// rather than the cast dropped.
#[cfg(unix)]
pub fn device_free_bytes(journal: &Path) -> Result<u64, String> {
    let stat = statvfs(journal).map_err(|error| error.to_string())?;
    #[allow(clippy::unnecessary_cast)]
    let blocks = stat.blocks_available() as u64;
    Ok(blocks.saturating_mul(stat.fragment_size()))
}
#[cfg(unix)]
pub fn device_total_bytes(journal: &Path) -> Result<u64, String> {
    let stat = statvfs(journal).map_err(|error| error.to_string())?;
    #[allow(clippy::unnecessary_cast)]
    let blocks = stat.blocks() as u64;
    Ok(blocks.saturating_mul(stat.fragment_size()))
}

#[cfg(windows)]
pub fn device_free_bytes(journal: &Path) -> Result<u64, String> {
    solstone_core_journal_io::windows_disk_space(journal)
        .map(|space| space.available_bytes)
        .map_err(|error| error.to_string())
}

#[cfg(windows)]
pub fn device_total_bytes(journal: &Path) -> Result<u64, String> {
    solstone_core_journal_io::windows_disk_space(journal)
        .map(|space| space.total_bytes)
        .map_err(|error| error.to_string())
}

#[cfg(not(any(unix, windows)))]
pub fn device_free_bytes(_journal: &Path) -> Result<u64, String> {
    Err("device capacity measurement is unsupported on this platform".into())
}

#[cfg(not(any(unix, windows)))]
pub fn device_total_bytes(_journal: &Path) -> Result<u64, String> {
    Err("device capacity measurement is unsupported on this platform".into())
}
pub fn suggest_offload_defaults(total: u64) -> Result<SuggestedOffloadDefaults, String> {
    if total == 0 {
        return Err("total_bytes must be a positive integer".into());
    }
    Ok(SuggestedOffloadDefaults {
        budget_bytes: total / 2,
        floor_bytes: (total / 10).max(MIN_FLOOR_BYTES).min(total / 4),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file(day: &str, name: &str, bytes: u64) -> LocalMediaFile {
        LocalMediaFile {
            key: LocalMediaKey {
                day: day.into(),
                stream: "_default".into(),
                segment: "010000_001".into(),
                segment_dir: "010000_001".into(),
                layout: SegmentLayout::Direct,
                name: name.into(),
            },
            bytes,
        }
    }

    #[test]
    fn known_local_totals_remain_local_and_unknown_is_not_zero() {
        let mut population = RawMediaPopulation {
            files: vec![
                file("20260101", "ordinary.webm", 11),
                file("20260101", "pending.flac", 7),
                file("20260102", "kept.flac", 5),
            ],
            unknown_paths: Vec::new(),
        };
        let usage = population.usage().unwrap();
        assert_eq!((usage.total_bytes, usage.total_files), (23, 3));
        assert_eq!(
            usage
                .per_day
                .iter()
                .map(|day| day.bytes)
                .collect::<Vec<_>>(),
            vec![18, 5]
        );
        population.unknown_paths.push("unreadable-day".into());
        assert!(!population.complete());
        assert!(population.usage().is_err());
        assert_eq!(population.known_usage().unwrap(), usage);
        population.files = vec![
            file("20260101", "large.webm", u64::MAX),
            file("20260102", "extra.flac", 1),
        ];
        assert!(population.known_usage().is_err());
    }

    #[test]
    fn defaults_match_floor_rule() {
        assert_eq!(
            suggest_offload_defaults(100_000_000_000)
                .unwrap()
                .floor_bytes,
            20_000_000_000
        );
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod filesystem_tests {
    use super::*;

    #[test]
    fn distinct_multiday_usage_aggregates_real_tree() {
        let journal = tempfile::tempdir().unwrap();
        let first = journal.path().join("chronicle/20260101/010000_001");
        let second = journal.path().join("chronicle/20260102/020000_001");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(first.join("raw.webm"), b"one").unwrap();
        fs::write(second.join("raw.webm"), b"012345").unwrap();
        let usage = measure_local_media(journal.path()).usage().unwrap();
        assert_eq!(usage.total_bytes, 9);
        assert_eq!(
            usage
                .per_day
                .iter()
                .map(|day| day.bytes)
                .collect::<Vec<_>>(),
            vec![3, 6]
        );
    }
    #[test]
    fn population_excludes_metadata_and_owned_stages_but_counts_hidden_media() {
        let journal = tempfile::tempdir().unwrap();
        let direct = journal.path().join("chronicle/20260101/010000_001_full");
        let named = journal
            .path()
            .join("chronicle/20260101/_default/010000_001_full");
        let stage = journal
            .path()
            .join("chronicle/20260101/.tmp-offload-010000_001");
        let named_stage = journal
            .path()
            .join("chronicle/20260101/_default/.tmp-offload-010000_001");
        for path in [&direct, &named, &stage, &named_stage] {
            fs::create_dir_all(path).unwrap();
        }
        fs::write(direct.join("raw.FLAC"), [0; 11]).unwrap();
        fs::write(named.join(".hidden.flac"), [0; 7]).unwrap();
        fs::write(direct.join("photo.png"), [0; 5]).unwrap();
        for name in ["raw.jsonl", "tombstone.json", "notes.txt", ".flac"] {
            fs::write(direct.join(name), [0; 100]).unwrap();
        }
        for path in [&stage, &named_stage] {
            fs::write(path.join("staged.flac"), [0; 200]).unwrap();
        }
        fs::create_dir(direct.join("nested.flac")).unwrap();
        let population = measure_local_media(journal.path());
        assert!(population.complete());
        assert_eq!(
            (
                population.usage().unwrap().total_bytes,
                population.files.len()
            ),
            (23, 3)
        );
        assert!(
            population
                .files
                .iter()
                .all(|file| file.key.segment == "010000_001"
                    && file.key.segment_dir == "010000_001_full")
        );
        let hidden = population
            .files
            .iter()
            .find(|file| file.key.name == ".hidden.flac")
            .unwrap();
        assert_eq!(
            (hidden.key.stream.as_str(), hidden.key.layout),
            ("_default", SegmentLayout::Named)
        );
        assert!(
            population
                .files
                .iter()
                .any(|file| file.key.layout == SegmentLayout::Direct)
        );
    }

    #[test]
    fn missing_chronicle_is_empty_but_failed_directory_traversal_is_unknown() {
        let journal = tempfile::tempdir().unwrap();
        assert_eq!(
            measure_local_media(journal.path())
                .usage()
                .unwrap()
                .total_bytes,
            0
        );
        fs::write(journal.path().join("chronicle"), "blocked").unwrap();
        let population = measure_local_media(journal.path());
        assert_eq!(
            population.unknown_paths,
            vec![journal.path().join("chronicle")]
        );
        assert!(population.usage().is_err());
        assert!(
            measure_local_media(&journal.path().join("missing-root"))
                .usage()
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_files_and_directories_do_not_enter_local_population() {
        use std::os::unix::fs::symlink;
        let journal = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("raw.flac"), [0; 100]).unwrap();
        let segment = journal.path().join("chronicle/20260101/010000_001");
        fs::create_dir_all(&segment).unwrap();
        fs::write(segment.join("local.flac"), [0; 7]).unwrap();
        symlink(outside.path().join("raw.flac"), segment.join("linked.flac")).unwrap();
        symlink(
            outside.path(),
            journal.path().join("chronicle/20260101/020000_001"),
        )
        .unwrap();
        symlink(
            outside.path(),
            journal.path().join("chronicle/20260101/stream"),
        )
        .unwrap();
        symlink(outside.path(), journal.path().join("chronicle/20260102")).unwrap();
        let population = measure_local_media(journal.path());
        assert!(population.complete());
        assert_eq!(
            (
                population.usage().unwrap().total_bytes,
                population.files.len()
            ),
            (7, 1)
        );
    }
}
