// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use solstone_core_backup::archive_activity::PreparedArchiveOperation;
use solstone_core_backup::archive_admission::ArchiveBudget;
use solstone_core_journal_io::media::{MediaPrimitive, run_with_media_hook};
use solstone_core_journal_io::paths::SegmentLayout;
use solstone_core_retention::archive_offer::OffloadOfferRefusal;

fn runner() -> Script {
    Script {
        outputs: RefCell::new(
            (0..64)
                .map(|_| output(json!({"id":CURRENT_REPO}).to_string()))
                .collect(),
        ),
        calls: RefCell::new(Vec::new()),
    }
}

fn prepare<'a>(fixture: &CurrentFixture, clock: &'a TestClock) -> PreparedArchiveOperation<'a> {
    PreparedArchiveOperation::prepare(
        std::sync::Arc::clone(&fixture.root),
        ArchiveBudget::new(clock).unwrap(),
        clock,
    )
    .unwrap()
}

fn at() -> ZonedDateTime {
    solstone_core_time::at_offset(
        jiff::Timestamp::new(CURRENT_NOW, 0).unwrap(),
        jiff::tz::Offset::UTC,
    )
}

#[test]
fn owning_planner_current_relief_avoids_capture_and_payload_capacity() {
    use solstone_core_segment::output_attempt::OutputClaims;
    for layout in [SegmentLayout::Direct, SegmentLayout::Named] {
        let fixture = CurrentFixture::new(layout);
        fixture.change_config(|config| config["backup"]["offload"]["budget_bytes"] = json!(16));
        let _busy = OutputClaims::discover(
            &fixture.journal.path().join("chronicle/20260101/020000_001"),
            solstone_core_journal_io::LockOptions {
                timeout: std::time::Duration::ZERO,
                ..Default::default()
            },
        )
        .unwrap();
        let clock = TestClock { now: CURRENT_NOW };
        let operation = prepare(&fixture, &clock);
        let carrier = std::sync::Arc::clone(operation.carrier());
        let runner = runner();
        let services = services(&runner, &Http, &clock, &Maintenance);
        let (result, copied) = run_with_media_hook(
            MediaPrimitive::StageCreated,
            || panic!("satisfied budget must not create a media-copy stage"),
            || create_planned_offload_offers(operation, &services, at()),
        );
        assert!(!copied);
        let report = result.planning().unwrap();
        assert!(report.reconciliation().unwrap().is_ok());
        let projection = report.initial_projection().unwrap();
        assert_eq!(
            (
                projection.local.total_bytes,
                projection.eligible_bytes,
                projection.projected_local_bytes
            ),
            (23, 7, 16)
        );
        assert_eq!(report.decision().unwrap().eligible().count(), 3); // overlap earns no second P
        assert!(result.observed().is_none());
        assert_eq!(result.source_copies().count(), 0);
        assert!(result.archives().is_empty());
        assert_eq!(result.ledger_writes().count(), 0);
        // A scripted runner cannot qualify the installed read member. Keep the
        // actual failed handoff; this test claims planning and zero N work only.
        let OffloadCreationOwner::ReadAdmissionFailure(failure) = result.owner() else {
            panic!("actual installed-read handoff must remain refused")
        };
        assert!(std::sync::Arc::ptr_eq(failure.carrier(), &carrier));
        assert!(matches!(
            result.reason(),
            Some(OffloadCreationRefusal::Readback(_))
        ));
        assert_eq!(report.phase(), OffloadPlanningPhase::Held);
        assert!(
            runner
                .calls
                .borrow()
                .iter()
                .all(|argv| !argv.iter().any(|arg| arg == "backup" || arg == "dump"))
        );
        assert_eq!(
            solstone_core_retention::review_context::load_at_root(&fixture.root)
                .unwrap()
                .marks
                .len(),
            2
        );
    }
}

#[test]
fn owning_planner_partial_credit_preserves_whole_existing_mark_capture() {
    let mut fixture = CurrentFixture::new(SegmentLayout::Direct);
    fixture.marks.truncate(1);
    fixture.save(&fixture.marks);
    fixture.change_config(|config| config["backup"]["offload"]["budget_bytes"] = json!(16));
    // Same size and accepted processing, but old second-file SHA no longer matches.
    fs::write(fixture.segment.join("second.webm"), b"else").unwrap();
    let clock = TestClock { now: CURRENT_NOW };
    let operation = prepare(&fixture, &clock);
    let runner = runner();
    let services = services(&runner, &Http, &clock, &Maintenance);
    let result = create_planned_offload_offers(operation, &services, at());
    let report = result.planning().unwrap();
    let projection = report.initial_projection().unwrap();
    assert_eq!(
        (projection.eligible_bytes, projection.projected_local_bytes),
        (3, 20)
    );
    let observed = result.observed().unwrap();
    assert_eq!(observed.groups().len(), 1);
    let group = &observed.groups()[0];
    assert_eq!(group.selection().names, fixture.marks[0].proposal.names);
    assert_eq!(group.expected_mark(), Some(&fixture.marks[0]));
    assert_eq!(
        group
            .files()
            .iter()
            .map(|file| file.observation().digest.size)
            .sum::<u64>(),
        7
    );
    assert_eq!(result.source_copies().count(), 2); // N includes its credited sibling
    assert!(matches!(result.owner(), OffloadCreationOwner::Admitted(_)));
    assert!(result.reason().is_some()); // scripted archive cannot mint proof
    assert_eq!(result.ledger_writes().count(), 0);
    assert_eq!(
        solstone_core_retention::review_context::load_at_root(&fixture.root)
            .unwrap()
            .marks
            .get(&fixture.marks[0].id),
        Some(&fixture.marks[0])
    );
}

#[test]
fn owning_planner_withdraws_legacy_unbound_mark_and_offers_its_files_again() {
    let mut fixture = CurrentFixture::new(SegmentLayout::Direct);
    // An offload mark from before archive offers: same files, no offer, version 1.
    let mut legacy = fixture.marks[0].clone();
    legacy.offload = None;
    fixture.marks = vec![legacy.clone()];
    fixture.save(&fixture.marks);
    fixture.change_config(|config| config["backup"]["offload"]["budget_bytes"] = json!(16));
    let media =
        ["first.webm", "second.webm"].map(|name| fs::read(fixture.segment.join(name)).unwrap());
    let clock = TestClock { now: CURRENT_NOW };
    let operation = prepare(&fixture, &clock);
    let runner = runner();
    let services = services(&runner, &Http, &clock, &Maintenance);
    let result = create_planned_offload_offers(operation, &services, at());
    let report = result.planning().unwrap();
    let reconciled = report
        .reconciliation()
        .unwrap()
        .as_ref()
        .unwrap_or_else(|failure| panic!("{:?}", failure.reason()));
    assert!(reconciled.register().marks.is_empty());
    assert_eq!(reconciled.register().version, 1);
    // The withdrawn row earned no credit, so its files are offered afresh,
    // as a new archive offer rather than an in-place replacement.
    let observed = result.observed().unwrap();
    assert_eq!(observed.groups().len(), 1);
    let group = &observed.groups()[0];
    assert_eq!(group.selection().target, legacy.target);
    assert_eq!(group.selection().names, legacy.proposal.names);
    assert_eq!(group.expected_mark(), None);
    for (name, bytes) in ["first.webm", "second.webm"].iter().zip(&media) {
        assert_eq!(&fs::read(fixture.segment.join(name)).unwrap(), bytes);
    }
}

#[test]
fn owning_planner_reconciles_kept_empty_before_unset_budget() {
    let mut fixture = CurrentFixture::new(SegmentLayout::Direct);
    let directory = fixture.journal.path().join("chronicle/20260101/030000_001");
    fs::create_dir(&directory).unwrap();
    let raw = directory.join("empty.flac");
    fs::write(&raw, b"empty").unwrap();
    write_empty_audio_sidecar(&raw);
    let proof = fixture.observe_offer(
        SegmentLayout::Direct,
        "030000_001",
        &["empty.flac"],
        "kept-empty",
    );
    fixture.add_offer(proof);
    let empty_id = fixture.marks.last().unwrap().id.clone();
    fixture.change_config(|config| {
        config["retention"] = json!({"empty_audio":"keep"});
        config["backup"]["offload"]["budget_bytes"] = Value::Null;
    });
    let clock = TestClock { now: CURRENT_NOW };
    let operation = prepare(&fixture, &clock);
    let runner = runner();
    let services = services(&runner, &Http, &clock, &Maintenance);
    let result = create_planned_offload_offers(operation, &services, at());
    let report = result.planning().unwrap();
    let reconciled = report
        .reconciliation()
        .unwrap()
        .as_ref()
        .unwrap_or_else(|failure| panic!("{:?}", failure.reason()));
    assert!(!reconciled.register().marks.contains_key(&empty_id));
    for ordinary in &fixture.marks[..2] {
        assert_eq!(
            reconciled.register().marks.get(&ordinary.id),
            Some(ordinary)
        );
    }
    assert_eq!(report.budget_bytes(), None);
    assert!(report.initial_projection().is_some());
    assert!(result.observed().is_none());
    assert_eq!(result.source_copies().count(), 0);
    assert!(raw.exists());
}

#[test]
fn owning_planner_invalid_settings_and_incomplete_local_stop_before_wan() {
    for case in ["budget", "floor", "local", "drift"] {
        // APFS refuses the non-UTF-8 name that makes local media incomplete here.
        if case == "local" && cfg!(target_os = "macos") {
            continue;
        }
        let fixture = CurrentFixture::new(SegmentLayout::Direct);
        match case {
            "budget" => fixture
                .change_config(|config| config["backup"]["offload"]["budget_bytes"] = json!("16")),
            "floor" => fixture
                .change_config(|config| config["backup"]["offload"]["floor_bytes"] = Value::Null),
            _ => {}
        }
        let clock = TestClock { now: CURRENT_NOW };
        let operation = prepare(&fixture, &clock);
        let carrier = std::sync::Arc::clone(operation.carrier());
        if case == "local" {
            use std::os::unix::ffi::OsStringExt;
            fs::write(
                fixture
                    .journal
                    .path()
                    .join("chronicle/20260101/020000_001")
                    .join(std::ffi::OsString::from_vec(vec![0xff])),
                b"unknown",
            )
            .unwrap();
        }
        if case == "drift" {
            fixture.change_config(|config| config["backup"]["offload"]["budget_bytes"] = json!(16));
        }
        let runner = runner();
        let services = services(&runner, &Http, &clock, &Maintenance);
        let result = create_planned_offload_offers(operation, &services, at());
        assert!(result.reason().is_some());
        assert!(
            runner.calls.borrow().is_empty(),
            "case={case}, reason={:?}, calls={:?}",
            result.reason(),
            runner.calls.borrow()
        );
        let OffloadCreationOwner::Prepared(original) = result.owner() else {
            panic!("prelaunch refusal must retain original preparation")
        };
        assert!(std::sync::Arc::ptr_eq(original.carrier(), &carrier));
        assert!(result.observed().is_none());
        assert_eq!(result.source_copies().count(), 0);
        assert!(result.archives().is_empty());
        if case == "budget" {
            assert!(result.planning().unwrap().reconciliation().unwrap().is_ok());
        }
        if case == "local" {
            assert!(matches!(
                result.reason(),
                Some(OffloadCreationRefusal::Planning(
                    OffloadPlanningRefusal::Local(_)
                ))
            ));
        }
        if case == "floor" || case == "drift" {
            assert!(matches!(
                result.reason(),
                Some(OffloadCreationRefusal::Inputs(
                    OffloadOfferRefusal::Admission(_)
                ))
            ));
        }
    }
}

#[test]
#[cfg(feature = "full-tests")]
fn owning_planner_post_p_capacity_error_retains_same_admission() {
    let fixture = CurrentFixture::new(SegmentLayout::Direct);
    fixture.save(&[]);
    fixture.change_config(|config| config["backup"]["offload"]["budget_bytes"] = json!(16));
    let clock = TestClock { now: CURRENT_NOW };
    let operation = prepare(&fixture, &clock);
    let carrier = std::sync::Arc::clone(operation.carrier());
    let runner = runner();
    let services = services(&runner, &Http, &clock, &Maintenance);
    let (result, consumed) = crate::run::with_payload_capacity_fault(|| {
        create_planned_offload_offers(operation, &services, at())
    });
    assert!(consumed);
    let OffloadCreationOwner::Admitted(original) = result.owner() else {
        panic!("actual post-P capacity error must retain admission")
    };
    assert!(std::sync::Arc::ptr_eq(original.carrier(), &carrier));
    let Some(OffloadCreationRefusal::Inputs(OffloadOfferRefusal::Io(error))) = result.reason()
    else {
        panic!("original native capacity error required")
    };
    assert_eq!(error.raw_os_error(), Some(13));
    let projection = result.planning().unwrap().initial_projection().unwrap();
    assert_eq!(
        (projection.local.total_bytes, projection.eligible_bytes),
        (23, 0)
    );
    assert_eq!(result.observed().unwrap().groups().len(), 1);
    assert_eq!(result.source_copies().count(), 0);
    assert!(result.partial_copy().is_none());
    assert!(result.archives().is_empty());
    assert_eq!(result.ledger_writes().count(), 0);
    assert!(
        runner
            .calls
            .borrow()
            .iter()
            .all(|argv| !argv.iter().any(|arg| arg == "backup" || arg == "dump"))
    );
    assert!(
        solstone_core_retention::review_context::load_at_root(&fixture.root)
            .unwrap()
            .marks
            .is_empty()
    );
}

#[test]
fn owned_planner_preflight_refusals_retain_original_before_admission() {
    use solstone_core_backup::archive_activity::OwnedPreparedArchiveOperation;
    use std::sync::Arc;
    for case in ["budget", "floor", "local", "drift", "disabled"] {
        // APFS refuses the non-UTF-8 name that makes local media incomplete here.
        if case == "local" && cfg!(target_os = "macos") {
            continue;
        }
        let fixture = CurrentFixture::new(SegmentLayout::Direct);
        match case {
            "budget" => fixture
                .change_config(|config| config["backup"]["offload"]["budget_bytes"] = json!("16")),
            "floor" => fixture
                .change_config(|config| config["backup"]["offload"]["floor_bytes"] = Value::Null),
            "disabled" => fixture
                .change_config(|config| config["backup"]["offload"]["enabled"] = json!(false)),
            _ => {}
        }
        let clock = Arc::new(TestClock { now: CURRENT_NOW });
        let original = OwnedPreparedArchiveOperation::prepare(
            Arc::clone(&fixture.root),
            ArchiveBudget::new(clock.as_ref()).unwrap(),
            clock,
        )
        .unwrap_or_else(|failure| panic!("preparation: {:?}", failure.reason()));
        if case == "local" {
            use std::os::unix::ffi::OsStringExt;
            fs::write(
                fixture
                    .segment
                    .join(std::ffi::OsString::from_vec(vec![0xff])),
                b"unknown",
            )
            .unwrap();
        }
        if case == "drift" {
            fixture.change_config(|config| config["backup"]["offload"]["budget_bytes"] = json!(16));
        }
        let (failure, copied) = run_with_media_hook(
            MediaPrimitive::StageCreated,
            || panic!("pre-admission refusal cannot copy media"),
            || {
                prepare_owned_offload_plan(&original, at())
                    .err()
                    .expect("preflight refusal")
            },
        );
        assert!(!copied);
        assert!(Arc::ptr_eq(failure.carrier(), original.carrier()));
        if case == "budget" || case == "floor" {
            assert!(failure.planning().reconciliation().unwrap().is_ok());
        }
        assert!(failure.planning().decision().is_none());
        assert!(failure.planning().initial_projection().is_none());
        assert_eq!(
            solstone_core_retention::review_context::load_at_root(&fixture.root)
                .unwrap()
                .marks
                .len(),
            2
        );
        let original = original.into_owned_hold();
        assert!(Arc::ptr_eq(failure.carrier(), original.carrier()));
        let coverage = solstone_core_journal_io::activity_obligations::observe_with_carrier(
            &fixture.root,
            original.carrier().observation(),
        );
        if case == "local" {
            assert!(
                coverage.is_err(),
                "the same uncertain name must also refuse census"
            );
        } else {
            assert!(coverage.unwrap().unresolved_entries().next().is_none());
        }
    }
}

#[test]
fn owned_planner_keeps_actual_withdrawal_when_later_floor_refuses() {
    use solstone_core_backup::archive_activity::OwnedPreparedArchiveOperation;
    use solstone_core_retention::archive_offer::OffloadReconciliationDisposition;
    use std::sync::Arc;
    let mut fixture = CurrentFixture::new(SegmentLayout::Named);
    let directory = fixture.journal.path().join("chronicle/20260101/030000_001");
    fs::create_dir(&directory).unwrap();
    let raw = directory.join("empty.flac");
    fs::write(&raw, b"empty").unwrap();
    write_empty_audio_sidecar(&raw);
    let proof = fixture.observe_offer(
        SegmentLayout::Direct,
        "030000_001",
        &["empty.flac"],
        "kept-empty",
    );
    fixture.add_offer(proof);
    let empty_id = fixture.marks.last().unwrap().id.clone();
    fixture.change_config(|config| {
        config["retention"] = json!({"empty_audio":"keep"});
        config["backup"]["offload"]["floor_bytes"] = Value::Null;
    });
    let clock = Arc::new(TestClock { now: CURRENT_NOW });
    let original = OwnedPreparedArchiveOperation::prepare(
        Arc::clone(&fixture.root),
        ArchiveBudget::new(clock.as_ref()).unwrap(),
        clock,
    )
    .unwrap_or_else(|failure| panic!("preparation: {:?}", failure.reason()));
    let failure = prepare_owned_offload_plan(&original, at())
        .err()
        .expect("floor refusal");
    assert!(matches!(
        failure.reason(),
        OffloadCreationRefusal::Inputs(OffloadOfferRefusal::Admission(_))
    ));
    let reconciled = failure
        .planning()
        .reconciliation()
        .unwrap()
        .as_ref()
        .unwrap_or_else(|failure| panic!("reconciliation: {:?}", failure.reason()));
    assert!(!reconciled.register().marks.contains_key(&empty_id));
    assert!(reconciled.groups().iter().any(|group| matches!(
        group.disposition(),
        OffloadReconciliationDisposition::Publication { .. }
    )));
    for mark in &fixture.marks[..2] {
        assert_eq!(reconciled.register().marks.get(&mark.id), Some(mark));
    }
    assert!(raw.exists());
    assert!(failure.planning().decision().is_none());
    assert!(Arc::ptr_eq(failure.carrier(), original.carrier()));
}

#[test]
fn owned_planner_closed_and_settled_fields_are_send_static() {
    fn require<T: Send + 'static>() {}
    require::<OwnedOffloadPlanInputFailure>();
    require::<OwnedOffloadPlanHold>();
    require::<OwnedOffloadPlanFinish>();
    require::<SettledOffloadPlan>();
    require::<SettledOffloadPlanReport>();
    require::<OffloadPlanningReport>();
    require::<CurrentOffloadReport>();
}
