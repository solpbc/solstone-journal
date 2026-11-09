// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::Cell;
use std::fs;
use std::io;

use chrono::TimeZone;
use serde_json::{Value, json};
use solstone_core_backup::archive_admission::{ArchiveBudget, Clock};
use solstone_core_backup_runtime::{
    HttpRequest, HttpResponse, HttpTransport, JournalMaintenance, JournalMaintenanceError,
    ToolOutput, ToolRequest, ToolRunner, hosted_runtime::HttpError,
};
use solstone_core_journal_io::media::{MediaPrimitive, run_with_media_hook};

use super::*;

struct FixedClock;
impl Clock for FixedClock {
    fn now_unix(&self) -> i64 {
        1_791_590_400
    }
    fn iso_week(&self) -> u8 {
        41
    }
}

struct NoWork(Cell<u64>);
impl ToolRunner for NoWork {
    fn run(&self, _: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        self.0.set(self.0.get() + 1);
        Err(io::Error::other("unexpected tool"))
    }
}
impl HttpTransport for NoWork {
    fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
        self.0.set(self.0.get() + 1);
        Err(HttpError::Other)
    }
}
impl JournalMaintenance for NoWork {
    fn full_scan(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
        unreachable!()
    }
}

fn services(work: &NoWork) -> BackupServices<'_> {
    BackupServices {
        runner: work,
        http: work,
        clock: &FixedClock,
        restic: None,
        rclone_path: None,
        version: "fixture",
        journal_maintenance: work,
    }
}

fn time() -> (NaiveDate, DateTime<Utc>) {
    (
        NaiveDate::from_ymd_opt(2026, 10, 10).unwrap(),
        Utc.with_ymd_and_hms(2026, 10, 10, 0, 0, 0).unwrap(),
    )
}

fn fixture(
    floor: Value,
    layout: SegmentLayout,
    names: &[&str],
) -> (tempfile::TempDir, Arc<JournalRoot>, OffloadOfferSelection) {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("config")).unwrap();
    fs::write(
        temp.path().join("config/journal.json"),
        serde_json::to_vec(&json!({"backup":{"offload":{"floor_bytes":floor}}})).unwrap(),
    )
    .unwrap();
    let selection = OffloadOfferSelection {
        target: Target {
            day: "20261009".into(),
            stream: if layout == SegmentLayout::Direct {
                "_default"
            } else {
                "camera"
            }
            .into(),
            dir: "120000_60_suffix".into(),
        },
        layout,
        names: names.iter().map(|name| (*name).into()).collect(),
    };
    let relative = match layout {
        SegmentLayout::Direct => format!(
            "chronicle/{}/{}",
            selection.target.day, selection.target.dir
        ),
        SegmentLayout::Named => format!(
            "chronicle/{}/{}/{}",
            selection.target.day, selection.target.stream, selection.target.dir
        ),
    };
    let directory = temp.path().join(relative);
    fs::create_dir_all(&directory).unwrap();
    for name in names {
        fs::write(directory.join(name), b"current").unwrap();
        let header = json!({"_solstone_processing": {"schema":"solstone.processing.v1", "handler":"transcribe", "state":"analyzed",
            "reason_code":"ok", "input_size":7, "attempted_at":"2026-10-09T00:00:00Z"}});
        fs::write(
            directory.join(name).with_extension("jsonl"),
            format!("{header}\n{{\"start\":0,\"text\":\"words\"}}\n"),
        )
        .unwrap();
    }
    let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
    (temp, root, selection)
}

fn prepare(root: &Arc<JournalRoot>) -> PreparedArchiveOperation<'static> {
    PreparedArchiveOperation::prepare(
        Arc::clone(root),
        ArchiveBudget::new(&FixedClock).unwrap(),
        &FixedClock,
    )
    .unwrap()
}

fn capture(
    operation: &PreparedArchiveOperation<'_>,
    selections: &[OffloadOfferSelection],
) -> ObservedOffloadOffers {
    let (today, now) = time();
    capture_offload_offers(operation, selections, today, now).unwrap()
}

fn proof(observed: &ObservedOffloadOffers, names: &[&str], label: &str) -> OffloadProof {
    let group = &observed.groups()[0];
    OffloadProof {
        journal_id: observed.journal_root().identity().observation_id(),
        manifest_id: format!("manifest-{label}"),
        repository_id: "a".repeat(64),
        snapshot_id: "b".repeat(64),
        target: group.selection().target.clone(),
        layout: group.selection().layout,
        files: group
            .files()
            .iter()
            .filter(|file| names.contains(&file.name()))
            .map(|file| ArchiveFileProof {
                witness_id: format!("witness-{label}-{}", file.name()),
                name: file.name().into(),
                bytes: file.observation().digest.size,
                sha256: file.observation().digest.sha256.clone(),
                archive_path: format!("/owned/{}", file.name()),
                processing: file.processing().clone(),
            })
            .collect(),
    }
}

#[test]
fn absent_or_overflow_floor_stops_before_copy_or_admission() {
    for floor in [Value::Null, json!(0), json!("1"), json!(u64::MAX)] {
        let (_temp, root, selection) = fixture(floor, SegmentLayout::Direct, &["audio.flac"]);
        let operation = prepare(&root);
        let carrier = Arc::clone(operation.carrier());
        let work = NoWork(Cell::new(0));
        let (today, now) = time();
        let result = create_offload_offers(operation, &services(&work), &[selection], today, now);
        assert!(matches!(
            result.reason(),
            Some(OffloadCreationRefusal::Inputs(
                OffloadOfferRefusal::Admission(_)
            ))
        ));
        let OffloadCreationOwner::Prepared(original) = result.owner() else {
            panic!("original prepared owner required")
        };
        assert!(Arc::ptr_eq(original.carrier(), &carrier));
        assert_eq!(result.source_copies().count(), 0);
        assert!(result.partial_copy().is_none());
        assert!(result.archives().is_empty());
        assert_eq!(result.ledger_writes().count(), 0);
        assert_eq!(work.0.get(), 0);
        assert!(
            !root
                .canonical_path()
                .join("health/retention-marks.json")
                .exists()
        );
    }
}

#[test]
fn owned_input_refusal_keeps_original_prepared_before_admission() {
    use solstone_core_backup::archive_activity::OwnedPreparedArchiveOperation;
    for floor in [Value::Null, json!(u64::MAX)] {
        let (_temp, root, selection) = fixture(floor, SegmentLayout::Named, &["audio.flac"]);
        let original = OwnedPreparedArchiveOperation::prepare(
            Arc::clone(&root),
            ArchiveBudget::new(&FixedClock).unwrap(),
            Arc::new(FixedClock),
        )
        .unwrap_or_else(|failure| panic!("preparation: {:?}", failure.reason()));
        let (today, now) = time();
        let failure = prepare_owned_offload_inputs(&original, &[selection], today, now)
            .err()
            .expect("invalid floor refused before admission");
        assert!(matches!(
            failure.reason(),
            OffloadCreationRefusal::Inputs(OffloadOfferRefusal::Admission(_))
        ));
        assert!(Arc::ptr_eq(failure.carrier(), original.carrier()));
        assert_eq!(
            failure.observed().unwrap().groups()[0].files()[0]
                .observation()
                .digest
                .size,
            7
        );
        let original = original.into_owned_hold();
        assert!(Arc::ptr_eq(failure.carrier(), original.carrier()));
        let coverage = solstone_core_journal_io::activity_obligations::observe_with_carrier(
            &root,
            original.carrier().observation(),
        )
        .unwrap();
        assert!(coverage.unresolved_entries().next().is_none());
        assert!(
            !root
                .canonical_path()
                .join("health/retention-marks.json")
                .exists()
        );
        assert!(read_complete_catalog(&root).unwrap().days.is_empty());
    }
}

#[test]
fn actual_capacity_reserves_n_plus_m_and_keeps_named_copies_on_admission_refusal() {
    for layout in [SegmentLayout::Direct, SegmentLayout::Named] {
        let (_temp, root, selection) = fixture(json!(7), layout, &["a.flac", "b.flac"]);
        let operation = prepare(&root);
        let carrier = Arc::clone(operation.carrier());
        let observed = capture(&operation, std::slice::from_ref(&selection));
        let metadata = (2 * MAX_REGISTER_BYTES + 1024 * 1024) as u64;
        assert_eq!(
            operation
                .configuration()
                .offload_required_bytes(21, metadata)
                .unwrap(),
            28 + metadata
        );
        capacity(&operation, &observed).unwrap();
        drop(observed);
        let work = NoWork(Cell::new(0));
        let (today, now) = time();
        let result = create_offload_offers(operation, &services(&work), &[selection], today, now);
        assert!(matches!(
            result.reason(),
            Some(OffloadCreationRefusal::Archive(ArchiveError::NotConfigured))
        ));
        let OffloadCreationOwner::AdmissionFailure(failure) = result.owner() else {
            panic!("original admission failure required")
        };
        assert!(Arc::ptr_eq(failure.carrier().unwrap(), &carrier));
        assert_eq!(result.source_copies().count(), 2);
        assert!(result.source_copies().all(|copy| copy.digest().size == 7));
        assert_eq!(work.0.get(), 0);
        assert!(
            !root
                .canonical_path()
                .join("health/retention-marks.json")
                .exists()
        );
    }
}

#[test]
fn same_bytes_replacement_before_copy_refuses_before_wan_with_copy_retained() {
    let (temp, root, selection) = fixture(json!(1), SegmentLayout::Direct, &["audio.flac"]);
    let operation = prepare(&root);
    let raw = temp
        .path()
        .join("chronicle/20261009/120000_60_suffix/audio.flac");
    let before = fs::metadata(&raw).unwrap();
    let old = temp.path().join("old-raw");
    let replacement = raw.clone();
    let work = NoWork(Cell::new(0));
    let (today, now) = time();
    let (result, hit) = run_with_media_hook(
        MediaPrimitive::StageCreated,
        move || {
            fs::rename(&replacement, &old)?;
            fs::write(&replacement, b"current")
        },
        || create_offload_offers(operation, &services(&work), &[selection], today, now),
    );
    assert!(hit);
    use std::os::unix::fs::MetadataExt;
    assert_ne!(before.ino(), fs::metadata(&raw).unwrap().ino());
    assert!(matches!(
        result.reason(),
        Some(OffloadCreationRefusal::Inputs(OffloadOfferRefusal::Changed))
    ));
    assert_eq!(result.source_copies().count(), 1);
    assert_eq!(work.0.get(), 0);
    assert!(result.archives().is_empty());
    assert_eq!(result.ledger_writes().count(), 0);
    assert_eq!(fs::read(raw).unwrap(), b"current");
}

#[test]
fn copy_error_retains_actual_target_and_capacity_query_error_creates_no_copy() {
    for primitive in [
        MediaPrimitive::PreparationWrite,
        MediaPrimitive::CapacityQuery,
    ] {
        let (_temp, root, selection) = fixture(json!(1), SegmentLayout::Direct, &["audio.flac"]);
        let operation = prepare(&root);
        let work = NoWork(Cell::new(0));
        let (today, now) = time();
        let (result, hit) = run_with_media_hook(
            primitive,
            || Err(io::Error::other("injected native refusal")),
            || create_offload_offers(operation, &services(&work), &[selection], today, now),
        );
        assert!(hit);
        assert_eq!(work.0.get(), 0);
        assert_eq!(result.source_copies().count(), 0);
        if primitive == MediaPrimitive::PreparationWrite {
            assert!(matches!(
                result.partial_copy(),
                Some(OffloadCreationNative::Copy(_))
            ));
            assert_eq!(result.native_error().unwrap().kind(), io::ErrorKind::Other);
        } else {
            assert!(result.partial_copy().is_none());
            assert!(matches!(
                result.reason(),
                Some(OffloadCreationRefusal::Inputs(OffloadOfferRefusal::Io(_)))
            ));
        }
        assert!(result.archives().is_empty());
        assert_eq!(result.ledger_writes().count(), 0);
    }
}

#[test]
fn exact_manifest_partition_and_reuse_preserve_complete_writer_history() {
    let (_temp, root, selection) = fixture(
        json!(1),
        SegmentLayout::Direct,
        &["a.flac", "b.flac", "c.flac"],
    );
    let operation = prepare(&root);
    let observed = capture(&operation, std::slice::from_ref(&selection));
    let original = proof(&observed, &["a.flac", "b.flac"], "old");
    let newer = proof(&observed, &["b.flac"], "new");
    assert!(durable(
        &append_archive_witness(&root, &original, 1).unwrap(),
        false
    ));
    assert!(durable(
        &append_archive_witness(&root, &newer, 2).unwrap(),
        false
    ));
    let catalog = read_complete_catalog(&root).unwrap();
    let selected = partition(&catalog, &Register::empty(), &[selection]).unwrap();
    assert_eq!(selected.len(), 3);
    let observed = capture(&operation, &selected);
    for group in observed.groups() {
        let candidate = reusable(&catalog, group, &root, &"a".repeat(64)).unwrap();
        match group.files()[0].name() {
            "a.flac" => {
                let (subset, complete) = candidate.unwrap();
                assert_eq!(subset.files.len(), 1);
                assert_eq!(complete, original);
                assert!(durable(
                    &confirm_archive_witness(&root, &complete, 3).unwrap(),
                    true
                ));
            }
            "b.flac" => {
                assert_eq!(candidate.unwrap().0, newer);
            }
            _ => assert!(candidate.is_none()),
        }
        assert!(
            reusable(&catalog, group, &root, &"e".repeat(64))
                .unwrap()
                .is_none()
        );
    }
    assert!(
        partition(
            &catalog,
            &Register::empty(),
            &[selected[0].clone(), selected[0].clone()]
        )
        .is_err()
    );
    assert!(
        !root
            .canonical_path()
            .join("health/retention-marks.json")
            .exists()
    );
}
