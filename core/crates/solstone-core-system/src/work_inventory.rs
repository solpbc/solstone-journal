// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A reporting snapshot. Never consulted as evidence that work is complete.

use serde::{Deserialize, Serialize};
use solstone_core_journal_config::work_priority::WorkBand;
use solstone_core_journal_io::durability::{ArtifactId, artifact};
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BandProgress {
    pub owner_requests: u64,
    pub segments_complete: u64,
    pub segments_owed: u64,
    pub daily_units_complete: u64,
    pub daily_units_owed: u64,
    #[serde(default)]
    pub activities_owed: u64,
    pub eligible: u64,
    pub permission_held: u64,
    pub prerequisite_held: u64,
}

impl BandProgress {
    pub fn add(&mut self, other: &Self) {
        self.owner_requests += other.owner_requests;
        self.segments_complete += other.segments_complete;
        self.segments_owed += other.segments_owed;
        self.daily_units_complete += other.daily_units_complete;
        self.daily_units_owed += other.daily_units_owed;
        self.activities_owed += other.activities_owed;
        self.eligible += other.eligible;
        self.permission_held += other.permission_held;
        self.prerequisite_held += other.prerequisite_held;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkHold {
    #[serde(default)]
    pub retry_unit: Option<String>,
    pub day: String,
    pub segment: Option<String>,
    pub stream: Option<String>,
    pub band: WorkBand,
    pub reason: String,
}

/// A completed discovery pass's history view, never a completion authority.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryProgress {
    pub caught_up_since: Option<String>,
    pub through_day: String,
    pub segments_owed: u64,
    pub daily_units_owed: u64,
    pub set_aside_units: u64,
    pub unreadable_days: usize,
    pub sample: Option<HistorySample>,
    pub net_per_hour: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistorySample {
    pub local_day: String,
    pub journal_version: String,
    pub as_of_ms: i64,
    pub owed: u64,
    pub set_aside: u64,
    pub unreadable: Vec<String>,
}

impl HistorySample {
    pub fn net_per_hour(&self, previous: &Self) -> Option<f64> {
        let elapsed = self.as_of_ms.checked_sub(previous.as_of_ms)?;
        let finished = previous.owed.checked_sub(self.owed)?;
        (self.local_day == previous.local_day
            && self.journal_version == previous.journal_version
            && self.set_aside == previous.set_aside
            && self.unreadable == previous.unreadable
            && elapsed > 0
            && finished > 0)
            .then(|| finished as f64 * 3_600_000.0 / elapsed as f64)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkInventory {
    pub version: u32,
    pub as_of_ms: i64,
    pub complete: bool,
    pub days_total: usize,
    pub days_inspected: usize,
    pub oldest_day: Option<String>,
    pub newest_day: Option<String>,
    pub bands: [BandProgress; 6],
    pub holds: Vec<WorkHold>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<HistoryProgress>,
    pub unreadable: Vec<String>,
    /// When discovery will look again. A finished pass stays current until then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_pass_ms: Option<i64>,
}

/// How long a snapshot stays current past its pass or its promised next one.
pub const CURRENT_FOR_MS: i64 = 180_000;

impl Default for WorkInventory {
    fn default() -> Self {
        Self {
            version: 1,
            as_of_ms: chrono::Utc::now().timestamp_millis(),
            complete: false,
            days_total: 0,
            days_inspected: 0,
            oldest_day: None,
            newest_day: None,
            bands: std::array::from_fn(|_| BandProgress::default()),
            holds: Vec::new(),
            history: None,
            unreadable: Vec::new(),
            next_pass_ms: None,
        }
    }
}

impl WorkInventory {
    pub fn band(&mut self, band: WorkBand) -> &mut BandProgress {
        &mut self.bands[band as usize - 1]
    }
    /// Whether a reader may treat this snapshot as discovery's current view.
    pub fn current_at(&self, now_ms: i64) -> bool {
        let until = self
            .next_pass_ms
            .unwrap_or(self.as_of_ms)
            .max(self.as_of_ms);
        now_ms >= self.as_of_ms && now_ms - until < CURRENT_FOR_MS
    }
    pub fn publish(&self, journal: &Path) -> Result<(), String> {
        self.publish_checked(journal, &|| Ok(()))
    }
    pub fn publish_checked(
        &self,
        journal: &Path,
        checkpoint: &dyn Fn() -> Result<(), String>,
    ) -> Result<(), String> {
        // Capture the publication root before invoking caller code. A retained
        // mobile checkpoint also rejects a root lost before this entry.
        let root = std::sync::Arc::new(
            solstone_core_journal_io::JournalRoot::open(journal)
                .map_err(|error| error.to_string())?,
        );
        let namespace = solstone_core_journal_io::journal_directory::JournalNamespace::new(root);
        let check = || {
            checkpoint()?;
            namespace.revalidate().map_err(|error| error.to_string())
        };
        check()?;
        let path = Path::new(artifact(ArtifactId::WorkInventory).path);
        let bytes = solstone_core_journal_io::atomic::serialize_json_bytes(
            self,
            solstone_core_journal_io::JsonWriteOptions::default(),
        )
        .map_err(|error| error.to_string())?;
        let parent = namespace
            .directory(path.parent().expect("fixed inventory parent"), true)
            .map_err(|error| error.to_string())?
            .expect("created inventory parent");
        let name = path.file_name().expect("fixed inventory filename");
        #[cfg(unix)]
        match solstone_core_journal_io::atomic_replace_bound_checked(
            parent.as_ref(),
            name,
            &bytes,
            0o600,
            &check,
        )
        .map_err(|error| error.to_string())?
        {
            solstone_core_journal_io::BoundAtomicOutcome::Published { .. } => {}
            outcome => return Err(format!("work inventory publication uncertain: {outcome:?}")),
        }
        #[cfg(windows)]
        {
            let lock = solstone_core_journal_io::acquire_existing_parent_lock_bound_checked(
                parent.as_ref(),
                std::ffi::OsStr::new(".work-inventory-publication.lock"),
                solstone_core_journal_io::DEFAULT_LOCK_TIMEOUT,
                solstone_core_journal_io::DEFAULT_LOCK_POLL_INTERVAL,
                &check,
            )
            .map_err(|error| error.to_string())?;
            match solstone_core_journal_io::atomic_replace_detailed_bound_checked(
                parent.as_ref(),
                &lock,
                name,
                &bytes,
                0o600,
                &check,
            )
            .map_err(|error| error.to_string())?
            {
                solstone_core_journal_io::DetailedAtomicOutcome::Published => {}
                outcome => {
                    return Err(format!("work inventory publication uncertain: {outcome:?}"));
                }
            }
        }
        namespace.revalidate().map_err(|error| error.to_string())
    }
}

pub fn read(journal: &Path) -> Result<Option<WorkInventory>, String> {
    let bytes = match std::fs::read(journal.join(artifact(ArtifactId::WorkInventory).path)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let report: WorkInventory =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if report.version != 1 {
        return Err("unrecognized work inventory".into());
    }
    Ok(Some(report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_refuses_root_replacement_after_caller_checkpoint() {
        let directory = tempfile::tempdir().unwrap();
        let journal = directory.path().join("journal");
        std::fs::create_dir(&journal).unwrap();
        let original = solstone_core_journal_io::JournalRoot::open(&journal).unwrap();
        let retained = directory.path().join("retained");
        let first = std::cell::Cell::new(true);
        let report = WorkInventory::default();
        let result = report.publish_checked(&journal, &|| {
            original
                .revalidate_canonical_binding()
                .map_err(|e| e.to_string())?;
            if first.replace(false) {
                std::fs::rename(&journal, &retained).unwrap();
                std::fs::create_dir(&journal).unwrap();
            }
            Ok(())
        });
        let replacement_version = read(&journal).unwrap().map(|report| report.version);
        assert!(
            result.is_err(),
            "publication must refuse authority lost after the caller check; actual replacement_snapshot_version={replacement_version:?}"
        );
        assert!(
            std::fs::read_dir(&journal).unwrap().next().is_none(),
            "replacement root must remain an empty tree"
        );
        assert!(!retained.join("health/work-inventory.json").exists());
    }

    #[test]
    fn checked_publication_preserves_normal_snapshot_and_stop_refusal() {
        let journal = tempfile::tempdir().unwrap();
        let report = WorkInventory {
            oldest_day: Some("20000101".into()),
            ..WorkInventory::default()
        };
        report.publish_checked(journal.path(), &|| Ok(())).unwrap();
        let path = journal.path().join("health/work-inventory.json");
        let original = std::fs::read(&path).unwrap();
        assert_eq!(
            read(journal.path()).unwrap().unwrap().oldest_day.as_deref(),
            Some("20000101")
        );
        let changed = WorkInventory::default();
        assert!(
            changed
                .publish_checked(journal.path(), &|| Err("stopped".into()))
                .is_err()
        );
        assert_eq!(std::fs::read(path).unwrap(), original);
    }

    #[test]
    fn history_rate_requires_two_comparable_decreasing_full_passes() {
        let mut previous = HistorySample {
            local_day: "20261013".into(),
            journal_version: "test-version".into(),
            as_of_ms: 1_000,
            owed: 1_000,
            set_aside: 0,
            unreadable: Vec::new(),
        };
        let mut current = previous.clone();
        current.as_of_ms += 3_600_000;
        current.owed -= 300;
        assert_eq!(current.net_per_hour(&previous), Some(300.0));
        previous.local_day = "20261012".into();
        assert_eq!(current.net_per_hour(&previous), None);
        previous.local_day = current.local_day.clone();
        previous.journal_version = "another-version".into();
        assert_eq!(current.net_per_hour(&previous), None);
        previous.journal_version = current.journal_version.clone();
        previous.unreadable.push("unreadable day".into());
        assert_eq!(current.net_per_hour(&previous), None);
        current.unreadable = previous.unreadable.clone();
        assert_eq!(current.net_per_hour(&previous), Some(300.0));
        previous.set_aside = 1;
        assert_eq!(current.net_per_hour(&previous), None);
        previous.set_aside = 0;
        previous.owed = current.owed;
        assert_eq!(current.net_per_hour(&previous), None);
        previous.owed -= 1;
        assert_eq!(current.net_per_hour(&previous), None);
        previous.owed = 1_000;
        previous.as_of_ms = current.as_of_ms;
        assert_eq!(current.net_per_hour(&previous), None);
    }

    #[test]
    fn a_finished_pass_stays_current_until_the_next_one_is_due() {
        let mut report = WorkInventory {
            as_of_ms: 1_000_000,
            ..WorkInventory::default()
        };
        assert!(report.current_at(1_000_000 + CURRENT_FOR_MS - 1));
        assert!(!report.current_at(1_000_000 + CURRENT_FOR_MS));
        assert!(!report.current_at(999_999));
        report.next_pass_ms = Some(1_000_000 + 3_600_000);
        assert!(report.current_at(1_000_000 + 3_600_000 + CURRENT_FOR_MS - 1));
        assert!(!report.current_at(1_000_000 + 3_600_000 + CURRENT_FOR_MS));
    }
}
