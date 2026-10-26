// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use solstone_core_journal_io::paths::SegmentLayout;

fn selected() -> SegmentCatalog {
    let intent = crate::test_restore_intent::fixture(false);
    SegmentCatalog {
        day: intent.locator.target.day,
        stream: intent.locator.target.stream,
        segment: intent.locator.segment,
        active: vec![intent.witness],
        ..SegmentCatalog::default()
    }
}

fn records(segment: &SegmentCatalog) -> Vec<FileRecord> {
    segment
        .active
        .iter()
        .map(|witness| FileRecord {
            detail: 0,
            day: segment.day.clone(),
            stream: segment.stream.clone(),
            segment: segment.segment.clone(),
            witness: witness.clone(),
            disposition: RestoreDisposition::Unattempted,
            reason: Some("restore_held"),
            files: 0,
            bytes: 0,
            owner: FileOwner::None,
        })
        .collect()
}

#[test]
fn exact_directory_and_layout_select_each_witness_without_snapshot_union() {
    let temp = tempfile::tempdir().unwrap();
    let mut segment = selected();
    let exact = segment.active[0].segment_dir.clone().unwrap();
    std::fs::create_dir_all(
        temp.path()
            .join("chronicle")
            .join(&segment.day)
            .join(&segment.stream)
            .join(&exact),
    )
    .unwrap();
    let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
    let discovered = discover(&root, &segment.day).unwrap();
    let original = records(&segment).remove(0);
    assert!(destination(&original, &discovered).is_ok());
    segment.active[0].layout = Some(SegmentLayout::Direct);
    assert_eq!(
        destination(&records(&segment)[0], &discovered),
        Err("segment_missing")
    );
    segment.active[0].layout = Some(SegmentLayout::Named);
    segment.active[0].segment_dir = Some(format!("{exact}_other"));
    assert_eq!(
        destination(&records(&segment)[0], &discovered),
        Err("segment_missing")
    );
    segment.active[0].repository_id = None;
    assert_eq!(
        destination(&records(&segment)[0], &[]),
        Err("archive_unbound")
    );
}

#[test]
fn selected_totals_refuse_overflow_before_any_effect() {
    let mut segment = selected();
    segment.active[0].file.bytes = u64::MAX;
    let mut other = segment.active[0].clone();
    other.file.name = "b.flac".into();
    other.file.bytes = 1;
    segment.active.push(other);
    assert_eq!(
        refused_selection("all", None, vec![segment]).unwrap_err(),
        "selection overflow"
    );
}

#[test]
fn unattempted_tail_and_already_present_do_not_earn_bytes() {
    let segment = selected();
    let mut report = refused_selection("all", None, vec![segment.clone()]).unwrap();
    let mut files = records(&segment);
    files[0].disposition = RestoreDisposition::AlreadyPresent;
    summarize(&mut report, &files);
    assert_eq!((report.files_restored, report.bytes_restored), (0, 0));
    assert_eq!(report.status, "refused");
    assert_eq!(report.reason.as_deref(), Some("restore_held"));
}

pub(crate) fn check_actual_accounting(
    outcomes: Vec<DetachedTransaction>,
    expected_files: u64,
    expected_bytes: u64,
    held: bool,
    settled_conflict: bool,
) {
    let mut segment = selected();
    let mut second = segment.active[0].clone();
    second.file.name = "b.flac".into();
    segment.active.push(second);
    let mut files = records(&segment);
    let mut stopped = false;
    for (file, outcome) in files.iter_mut().zip(outcomes) {
        assert!(!stopped, "a held outcome must stop before the next read");
        stopped = file.accept(outcome);
    }
    assert_eq!(stopped, held);
    let mut report = refused_selection("all", None, vec![segment]).unwrap();
    if held {
        report.reason_detail = Some("earlier discovery failure".into());
    }
    summarize(&mut report, &files);
    assert_eq!(
        (report.files_restored, report.bytes_restored),
        (expected_files, expected_bytes)
    );
    assert_eq!(
        report.status,
        if held || settled_conflict {
            "degraded"
        } else {
            "ok"
        }
    );
    if settled_conflict {
        assert_eq!(files[0].disposition, RestoreDisposition::Conflict);
        assert_eq!((files[0].files, files[0].bytes), (0, 0));
        assert_eq!(report.reason.as_deref(), Some("restore_conflict"));
        assert_eq!(report.segments_restored, 0);
    }
    if held {
        assert_eq!(files[1].disposition, RestoreDisposition::Unattempted);
        let FileOwner::Transaction(transaction) = &files[0].owner else {
            panic!("actual held transaction required");
        };
        let actual_hold = transaction.facts().hold.unwrap();
        let detail = report.reason_detail.as_deref().unwrap();
        assert!(detail.starts_with("earlier discovery failure"));
        assert!(detail.ends_with(&actual_hold));
    }
}

#[test]
fn legacy_only_refusal_precedes_capacity_and_mixed_legacy_does_not_inflate_stage_requirement() {
    let segment = selected();
    let mut files = records(&segment);
    files[0].witness.repository_id = None;
    files[0].witness.file.bytes = u64::MAX;
    assert_eq!(bound_maximum(&mut files), None);
    assert_eq!(files[0].reason, Some("archive_unbound"));
    files.extend(records(&segment));
    assert_eq!(bound_maximum(&mut files), Some(3));
    assert_eq!(files[0].disposition, RestoreDisposition::Unattempted);
    assert!(matches!(files[0].owner, FileOwner::None));
}
