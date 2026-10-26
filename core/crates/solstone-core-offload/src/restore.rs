// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Restore selection and owning prepared-media execution.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use solstone_core_backup_runtime::BackupServices;
use solstone_core_journal_io::media::MediaDirectory;
use solstone_core_journal_io::paths::{PathOrDay, iter_segments};
use solstone_core_journal_io::{JournalRoot, check_record_identities};

pub mod owned;

use crate::ledger;
use solstone_core_retention::archive_catalog::{SegmentCatalog, read_complete_catalog};

pub const RESTORE_RESERVE_BYTES: u64 = 1_000_000_000;
pub const OFFLOAD_RESTORE_TIMEOUT_SECONDS: u64 = 6 * 60 * 60;
pub const OFFLOAD_RESTORE_STATUSES: [&str; 5] = ["ok", "no_op", "refused", "degraded", "error"];
pub const OFFLOAD_RESTORE_REASONS: [&str; 24] = [
    "archive_unbound",
    "auth_failed",
    "backup_not_ready",
    "destination_admission_failed",
    "failed",
    "insufficient_free_space",
    "ledger_degraded",
    "legacy_read_failed",
    "legacy_review_held",
    "legacy_verification_failed",
    "locked",
    "missing_file_after_restore",
    "nothing_to_restore",
    "repo_missing",
    "restic_unavailable",
    "restore_conflict",
    "restore_held",
    "restore_not_published",
    "rclone_unavailable",
    "segment_identity",
    "segment_missing",
    "stream_marker_failed",
    "timeout",
    "verification_failed",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreSegmentResult {
    pub status: String,
    pub reason: Option<String>,
    pub day: String,
    pub stream: String,
    pub segment: String,
    pub snapshot_id: Option<String>,
    pub files_expected: u64,
    pub files_restored: u64,
    pub bytes_expected: u64,
    pub bytes_restored: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreResult {
    pub status: String,
    pub reason: Option<String>,
    pub scope: String,
    pub day: Option<String>,
    pub segments_selected: u64,
    pub segments_restored: u64,
    pub files_expected: u64,
    pub files_restored: u64,
    pub bytes_expected: u64,
    pub bytes_restored: u64,
    /// Actual admission, transaction or settlement causes. The reason token
    /// and independent effect counters retain their original meanings.
    pub reason_detail: Option<String>,
    pub details: Vec<RestoreSegmentResult>,
}
fn base(status: &str, reason: Option<&str>, scope: &str, day: Option<&str>) -> RestoreResult {
    RestoreResult {
        status: status.into(),
        reason: reason.map(str::to_owned),
        scope: scope.into(),
        day: day.map(str::to_owned),
        segments_selected: 0,
        segments_restored: 0,
        files_expected: 0,
        files_restored: 0,
        bytes_expected: 0,
        bytes_restored: 0,
        reason_detail: None,
        details: vec![],
    }
}
impl RestoreResult {
    fn with_reason_detail(mut self, detail: String) -> Self {
        self.reason_detail = Some(detail);
        self
    }
}
fn refused_selection(
    scope: &str,
    day: Option<&str>,
    selected: Vec<SegmentCatalog>,
) -> Result<RestoreResult, &'static str> {
    if selected.is_empty() {
        return Ok(base("no_op", Some("nothing_to_restore"), scope, day));
    }
    let mut result = base("refused", None, scope, day);
    result.segments_selected = u64::try_from(selected.len()).map_err(|_| "selection overflow")?;
    for segment in selected {
        let files = u64::try_from(segment.active.len()).map_err(|_| "selection overflow")?;
        let bytes = segment.active.iter().try_fold(0_u64, |total, witness| {
            total
                .checked_add(witness.file.bytes)
                .ok_or("selection overflow")
        })?;
        result.files_expected = result
            .files_expected
            .checked_add(files)
            .ok_or("selection overflow")?;
        result.bytes_expected = result
            .bytes_expected
            .checked_add(bytes)
            .ok_or("selection overflow")?;
        let reason = if segment
            .active
            .iter()
            .any(|witness| witness.repository_id.is_none())
        {
            "archive_unbound"
        } else {
            "restore_held"
        };
        // A single snapshot label is valid only when every selected file shares it.
        let snapshot = &segment.active[0].snapshot_id;
        let snapshot_id = segment
            .active
            .iter()
            .all(|witness| witness.snapshot_id == *snapshot)
            .then(|| snapshot.clone());
        if result.reason.is_none() {
            result.reason = Some(reason.into());
        }
        result.details.push(RestoreSegmentResult {
            status: "refused".into(),
            reason: Some(reason.into()),
            day: segment.day,
            stream: segment.stream,
            segment: segment.segment,
            snapshot_id,
            files_expected: files,
            files_restored: 0,
            bytes_expected: bytes,
            bytes_restored: 0,
        });
    }
    Ok(result)
}

fn run(journal: &Path, scope: &str, day: Option<&str>) -> RestoreResult {
    if day.is_some_and(|day| ledger::ledger_path_for_day(journal, day).is_err()) {
        return base("error", Some("ledger_degraded"), scope, day);
    }
    let root = match JournalRoot::open(journal) {
        Ok(root) => Arc::new(root),
        Err(error) => {
            return base("refused", Some("segment_identity"), scope, day)
                .with_reason_detail(error.to_string());
        }
    };
    let catalog = match read_complete_catalog(&root) {
        Ok(catalog) if !catalog.degraded() => catalog,
        Ok(_) => return base("error", Some("ledger_degraded"), scope, day),
        Err(error) => {
            return base("error", Some("ledger_degraded"), scope, day).with_reason_detail(error);
        }
    };
    let selected: Vec<_> = catalog
        .days
        .into_iter()
        .filter(|candidate| day.is_none_or(|day| candidate.day == day))
        .flat_map(|day| day.segments)
        .filter(|segment| !segment.active.is_empty())
        .collect();
    let days: BTreeSet<_> = selected.iter().map(|segment| &segment.day).collect();
    for selected_day in days {
        let directory = match MediaDirectory::open(
            Arc::clone(&root),
            &Path::new("chronicle").join(selected_day),
        ) {
            Ok(directory) => directory,
            Err(error) => {
                return base("refused", Some("segment_identity"), scope, day)
                    .with_reason_detail(format!("day {selected_day}: {error}"));
            }
        };
        let listed = match iter_segments(root.canonical_path(), PathOrDay::Day(selected_day)) {
            Ok(listed) => listed,
            Err(error) => {
                return base("refused", Some("segment_identity"), scope, day)
                    .with_reason_detail(format!("day {selected_day}: {error}"));
            }
        };
        if let Err(error) = check_record_identities(&listed) {
            return base("refused", Some("segment_identity"), scope, day)
                .with_reason_detail(format!("day {selected_day}: {error}"));
        }
        if let Err(error) = directory.revalidate() {
            return base("refused", Some("segment_identity"), scope, day)
                .with_reason_detail(format!("day {selected_day}: {error}"));
        }
    }
    let result = match refused_selection(scope, day, selected) {
        Ok(result) => result,
        Err(error) => {
            return base("error", Some("failed"), scope, day).with_reason_detail(error.into());
        }
    };
    if let Err(error) = root.revalidate_canonical_binding() {
        return base("refused", Some("segment_identity"), scope, day)
            .with_reason_detail(error.to_string());
    }
    result
}

/// The repository the selected bound files name, or `None` when only historical
/// files without one are selected; those are reviewed against the configured
/// repository at admission.
pub fn selected_repository(
    root: &Arc<JournalRoot>,
    day: Option<&str>,
) -> Result<Option<String>, &'static str> {
    let catalog = read_complete_catalog(root).map_err(|_| "ledger_degraded")?;
    let mut selected = None;
    let mut unbound = false;
    for witness in catalog
        .days
        .iter()
        .filter(|entry| day.is_none_or(|day| entry.day == day))
        .flat_map(|entry| &entry.segments)
        .flat_map(|segment| &segment.active)
    {
        let Some(candidate) = witness.repository_id.as_deref() else {
            unbound = true;
            continue;
        };
        if candidate.is_empty() {
            return Err("archive_unbound");
        }
        if selected.is_some_and(|id| id != candidate) {
            return Err("repository_mismatch");
        }
        selected = Some(candidate);
    }
    match selected {
        Some(id) => Ok(Some(id.to_owned())),
        None if unbound => Ok(None),
        None => Err("nothing_to_restore"),
    }
}

pub fn restore_offload_day(
    journal: &Path,
    _services: &BackupServices<'_>,
    day: &str,
) -> RestoreResult {
    run(journal, "day", Some(day))
}

pub fn restore_all_offload(journal: &Path, _services: &BackupServices<'_>) -> RestoreResult {
    run(journal, "all", None)
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::fs;
    use std::io;
    use std::path::PathBuf;

    use crate::ledger::append_offload_event;
    use serde_json::json;
    use solstone_core_backup_runtime::hosted_runtime::HttpError;
    use solstone_core_backup_runtime::{
        Clock, HttpRequest, HttpResponse, HttpTransport, JournalMaintenance,
        JournalMaintenanceError, ToolOutput, ToolRequest, ToolRunner,
    };
    use solstone_core_retention::archive_catalog::OffloadFile;
    use solstone_core_retention::{Target, upsert_offload};

    #[derive(Default)]
    struct Harness {
        calls: Cell<u64>,
    }
    impl ToolRunner for Harness {
        fn run(&self, _: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            self.calls.set(self.calls.get() + 1);
            Err(io::Error::other("held restore must never launch a helper"))
        }
    }
    impl HttpTransport for Harness {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            panic!("held restore must not contact a repository")
        }
    }
    impl Clock for Harness {
        fn now_unix(&self) -> i64 {
            panic!("held restore does not record an outcome")
        }
        fn iso_week(&self) -> u8 {
            panic!("held restore does not record an outcome")
        }
    }
    impl JournalMaintenance for Harness {
        fn rebuild_body_history(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            panic!("held restore does not write journal state")
        }
        fn full_scan(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            panic!("held restore does not write journal state")
        }
    }
    impl Harness {
        fn services(&self) -> BackupServices<'_> {
            BackupServices {
                runner: self,
                http: self,
                clock: self,
                restic: Some((Path::new("/unused/restic")).into()),
                rclone_path: None,
                version: "test",
                journal_maintenance: self,
            }
        }
    }

    fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
        fn walk(root: &Path, relative: &Path, out: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
            for entry in fs::read_dir(root.join(relative)).unwrap() {
                let entry = entry.unwrap();
                let path = relative.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    out.insert(path.clone(), None);
                    walk(root, &path, out);
                } else {
                    out.insert(path, Some(fs::read(entry.path()).unwrap()));
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(root, Path::new(""), &mut out);
        out
    }
    fn legacy(journal: &Path, day: &str, dir: &str) -> PathBuf {
        let segment = journal.join("chronicle").join(day).join(dir);
        fs::create_dir_all(&segment).unwrap();
        append_offload_event(
            journal,
            day,
            "_default",
            dir,
            "legacy-snapshot",
            &[OffloadFile {
                name: "raw.webm".into(),
                bytes: 8,
                sha256: "a".repeat(64),
            }],
            1,
        )
        .unwrap();
        upsert_offload(
            journal,
            &Target {
                day: day.into(),
                stream: "_default".into(),
                dir: dir.into(),
            },
            vec!["raw.webm".into()],
            8,
            "legacy archive".into(),
            chrono::DateTime::<chrono::Utc>::from_timestamp(1, 0).unwrap(),
        )
        .unwrap();
        segment
    }
    fn bound(journal: &Path, intent: &solstone_core_retention::restore_intent::RestoreIntent) {
        let witness = &intent.witness;
        fs::create_dir_all(journal.join(intent.locator.relative_segment())).unwrap();
        let event = json!({
            "event_kind":"offload_archive_v2", "schema_version":2, "time":1,
            "day":intent.locator.target.day, "stream":intent.locator.target.stream,
            "segment":intent.locator.segment, "segment_dir":intent.locator.target.dir,
            "layout":intent.locator.layout, "journal_id":witness.journal_id,
            "manifest_id":witness.manifest_id, "repository_id":witness.repository_id,
            "snapshot_id":witness.snapshot_id,
            "files":[{"witness_id":witness.witness_id, "name":witness.file.name,
                "bytes":witness.file.bytes,"sha256":witness.file.sha256,
                "archive_path":witness.archive_path,"processing":witness.processing}]
        });
        let path = ledger::ledger_path_for_day(journal, &intent.locator.target.day).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(file, "{event}").unwrap();
    }
    fn assert_zero(result: &RestoreResult, harness: &Harness) {
        assert_eq!(harness.calls.get(), 0);
        assert_eq!(
            (
                result.segments_restored,
                result.files_restored,
                result.bytes_restored
            ),
            (0, 0, 0)
        );
        assert!(
            result
                .details
                .iter()
                .all(|detail| detail.files_restored == 0 && detail.bytes_restored == 0)
        );
    }

    #[test]
    fn empty_catalog_is_no_op_without_tools_or_creating_metadata() {
        let journal = tempfile::tempdir().unwrap();
        let harness = Harness::default();
        for result in [
            restore_all_offload(journal.path(), &harness.services()),
            restore_offload_day(journal.path(), &harness.services(), "20261009"),
        ] {
            assert_eq!(result.status, "no_op", "{result:?}");
            assert_eq!(result.reason.as_deref(), Some("nothing_to_restore"));
            assert_zero(&result, &harness);
        }
        assert!(snapshot(journal.path()).is_empty());
        assert_eq!(
            restore_offload_day(journal.path(), &harness.services(), "invalid")
                .reason
                .as_deref(),
            Some("ledger_degraded")
        );
    }

    #[test]
    fn unbound_missing_and_present_media_preserve_every_owner_file_and_obligation() {
        let journal = tempfile::tempdir().unwrap();
        let absent = legacy(journal.path(), "20260112", "120000_012");
        let present = legacy(journal.path(), "20260113", "120000_012");
        fs::write(present.join("raw.webm"), b"existing competitor").unwrap();
        fs::write(absent.join("other.flac"), b"unselected owner file").unwrap();
        fs::create_dir_all(journal.path().join("chronicle/20260112/health")).unwrap();
        fs::write(
            journal
                .path()
                .join("chronicle/20260112/health/stream.updated"),
            b"existing marker",
        )
        .unwrap();
        fs::create_dir_all(journal.path().join("config")).unwrap();
        fs::write(
            journal.path().join("config/journal.json"),
            b"existing backup result state",
        )
        .unwrap();
        let before = snapshot(journal.path());
        let harness = Harness::default();
        for result in [
            restore_offload_day(journal.path(), &harness.services(), "20260112"),
            restore_all_offload(journal.path(), &harness.services()),
        ] {
            assert_eq!(result.status, "refused", "{result:?}");
            assert_eq!(result.reason.as_deref(), Some("archive_unbound"));
            assert_ne!(
                result.reason.as_deref(),
                Some("segment_identity"),
                "same key on different days is valid"
            );
            assert_zero(&result, &harness);
        }
        assert_eq!(snapshot(journal.path()), before);
        assert!(!absent.join("raw.webm").exists());
    }

    #[test]
    fn bound_multi_generation_selection_is_held_without_arbitrary_snapshot() {
        let journal = tempfile::tempdir().unwrap();
        let first = crate::test_restore_intent::fixture(false);
        bound(journal.path(), &first);
        let mut second = first.clone();
        second.witness.manifest_id = "manifest-2".into();
        second.witness.witness_id = "witness-b".into();
        second.witness.snapshot_id = "e".repeat(64);
        second.witness.file.name = "b.flac".into();
        second.witness.file.bytes = 7;
        second.witness.archive_path = Some("/stage/b.flac".into());
        bound(journal.path(), &second);
        let before = snapshot(journal.path());
        let harness = Harness::default();
        let result = restore_all_offload(journal.path(), &harness.services());
        assert_eq!(result.status, "refused", "{result:?}");
        assert_eq!(result.reason.as_deref(), Some("restore_held"));
        assert_eq!(
            (
                result.segments_selected,
                result.files_expected,
                result.bytes_expected
            ),
            (1, 2, 10)
        );
        assert_eq!(result.details[0].snapshot_id, None);
        assert_zero(&result, &harness);
        assert_eq!(snapshot(journal.path()), before);
    }

    #[test]
    fn mixed_day_and_all_selection_keeps_each_segments_reason() {
        let journal = tempfile::tempdir().unwrap();
        bound(journal.path(), &crate::test_restore_intent::fixture(false));
        legacy(journal.path(), "20261010", "120000_012");
        let harness = Harness::default();
        let day = restore_offload_day(journal.path(), &harness.services(), "20261009");
        let all = restore_all_offload(journal.path(), &harness.services());
        assert_eq!((day.segments_selected, day.files_expected), (1, 1));
        assert_eq!(day.reason.as_deref(), Some("restore_held"));
        assert_eq!((all.segments_selected, all.files_expected), (2, 2));
        assert_eq!(
            all.details
                .iter()
                .map(|detail| detail.reason.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("restore_held"), Some("archive_unbound")]
        );
        assert_zero(&day, &harness);
        assert_zero(&all, &harness);
    }

    #[test]
    fn torn_or_non_file_catalog_never_becomes_empty_success() {
        let journal = tempfile::tempdir().unwrap();
        let harness = Harness::default();
        let path = ledger::ledger_path_for_day(journal.path(), "20261009").unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{torn ledger").unwrap();
        let before = snapshot(journal.path());
        let result = restore_all_offload(journal.path(), &harness.services());
        assert_eq!(result.reason.as_deref(), Some("ledger_degraded"));
        assert_zero(&result, &harness);
        assert_eq!(snapshot(journal.path()), before);
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let result = restore_all_offload(journal.path(), &harness.services());
        assert_eq!(result.reason.as_deref(), Some("ledger_degraded"));
        assert_zero(&result, &harness);
        assert!(path.is_dir());
    }

    #[test]
    fn duplicate_segment_identity_refuses_before_launch_or_state_recording() {
        let journal = tempfile::tempdir().unwrap();
        legacy(journal.path(), "20260112", "120000_012");
        legacy(journal.path(), "20260112", "120000_012_copy");
        let before = snapshot(journal.path());
        let harness = Harness::default();
        let result = restore_all_offload(journal.path(), &harness.services());
        assert_eq!(
            result.reason.as_deref(),
            Some("segment_identity"),
            "{result:?}"
        );
        assert!(result.reason_detail.is_some());
        assert_zero(&result, &harness);
        assert_eq!(snapshot(journal.path()), before);
    }

    #[test]
    fn directory_read_error_preserves_selection_uncertainty() {
        let journal = tempfile::tempdir().unwrap();
        let intent = crate::test_restore_intent::fixture(false);
        bound(journal.path(), &intent);
        let day = journal.path().join("chronicle/20261009");
        fs::rename(&day, journal.path().join("displaced-day")).unwrap();
        fs::write(&day, b"occupied day path").unwrap();
        let before = snapshot(journal.path());
        let harness = Harness::default();
        let result = restore_all_offload(journal.path(), &harness.services());
        assert_eq!(result.status, "refused", "{result:?}");
        assert_eq!(result.reason.as_deref(), Some("segment_identity"));
        assert_zero(&result, &harness);
        assert_eq!(snapshot(journal.path()), before);
    }

    #[test]
    fn unrepresentable_selection_totals_refuse_without_wrapping() {
        let journal = tempfile::tempdir().unwrap();
        let mut first = crate::test_restore_intent::fixture(false);
        first.witness.file.bytes = u64::MAX;
        bound(journal.path(), &first);
        let mut second = first.clone();
        second.witness.manifest_id = "manifest-2".into();
        second.witness.witness_id = "witness-b".into();
        second.witness.file.name = "b.flac".into();
        second.witness.file.bytes = 1;
        second.witness.archive_path = Some("/stage/b.flac".into());
        bound(journal.path(), &second);
        let before = snapshot(journal.path());
        let harness = Harness::default();
        let result = restore_all_offload(journal.path(), &harness.services());
        assert_eq!(result.status, "error", "{result:?}");
        assert_eq!(result.reason.as_deref(), Some("failed"));
        assert_zero(&result, &harness);
        assert_eq!(snapshot(journal.path()), before);
    }
}
