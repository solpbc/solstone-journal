// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Work discovery for the owner's asks to think a corrected day again.

use super::*;
use solstone_core_system::owner_ask::{self, AskedActivity};
use solstone_core_system::queue::{ProcessState, ProcessStateProbe, TaskQueueOptions};

struct NoProcesses;
impl ProcessStateProbe for NoProcesses {
    fn state(&self, _: u32) -> ProcessState {
        panic!("unready fixture must not launch")
    }
}

fn queue(journal: &Path) -> TaskQueue {
    TaskQueue::new(TaskQueueOptions {
        #[cfg(windows)]
        read_file_grants: Vec::new(),
        journal_root: journal.into(),
        cap_resolver: Arc::new(solstone_core_system::cap::DefaultCapResolver::default()),
        process_state_probe: Arc::new(NoProcesses),
        queue_sink: None,
        process_sink: None,
        ready: false,
        before_deadline_commit: None,
        child_environment: BTreeMap::new(),
        task_binary: None,
    })
}

fn journal() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("config")).unwrap();
    std::fs::write(
        root.path().join("config/journal.json"),
        serde_json::json!({
            "identity": {"timezone": "UTC"},
            "providers": {"active": {"provider": "local", "model": "fixture"}}
        })
        .to_string(),
    )
    .unwrap();
    root
}

fn days_ago(days: i64) -> String {
    (chrono::Utc::now().date_naive() - chrono::Duration::days(days))
        .format("%Y%m%d")
        .to_string()
}

/// A recorded, sensed segment and a reading activity over it.
fn activity(root: &Path, day: &str, id: &str, key: &str) -> AskedActivity {
    let segment = root.join("chronicle").join(day).join("notes").join(key);
    std::fs::create_dir_all(segment.join("talents")).unwrap();
    std::fs::write(
        segment.join("audio.jsonl"),
        "{\"start\":\"00:00:01\",\"text\":\"Read about the garden.\"}\n",
    )
    .unwrap();
    std::fs::write(
        segment.join("talents/sense.json"),
        r#"{"density":"active","content_type":"reading","facets":[]}"#,
    )
    .unwrap();
    let _ = solstone_core_facets::create_facet(root, "work", "Work", "", "", "", None);
    let _ = solstone_core_facets::append_activity_record(
        root,
        "work",
        day,
        serde_json::json!({"id": id, "activity": "reading", "facet": "work", "stream": "notes", "segments": [key]})
            .as_object()
            .unwrap()
            .clone(),
    )
    .unwrap();
    AskedActivity {
        facet: "work".into(),
        activity: id.into(),
    }
}

/// Segments sensed and thought on their current input, as a closed day's are.
fn thought(root: &Path, day: &str, keys: &[&str]) {
    use solstone_core_journal_io::{
        JournalRoot,
        operational_log::{OplogFormat, create_oplog_at},
    };
    use std::io::Write;
    let mut log = create_oplog_at(
        JournalRoot::open(root).unwrap(),
        "think",
        "segment",
        OplogFormat::Jsonl,
        chrono::NaiveDate::parse_from_str(day, "%Y%m%d")
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .fixed_offset(),
    )
    .unwrap();
    let sensed = chrono::Utc::now().timestamp_millis() + 3_600_000;
    for key in keys {
        let segment = root.join("chronicle").join(day).join("notes").join(key);
        std::fs::create_dir_all(&segment).unwrap();
        std::fs::write(
            segment.join("audio.jsonl"),
            "{\"start\":\"00:00:01\",\"text\":\"A meeting about a garden.\"}\n",
        )
        .unwrap();
        let row = serde_json::json!({"event":"sense.complete","ts":sensed,"mode":"segment","stream":"notes","segment":key,"density":"idle"});
        writeln!(log, "{row}").unwrap();
    }
}

/// Each thought segment's saved outputs published, so the day is ready.
fn published(root: &Path, day: &str, keys: &[&str]) {
    for key in keys {
        let segment = root.join("chronicle").join(day).join("notes").join(key);
        let newest = solstone_core_system_health::newest_segment_input_ms(&segment).unwrap();
        solstone_core_system::segment_publication::publish(root, &segment, newest, |_, _| Ok(()))
            .unwrap();
    }
}

fn work_record(root: &Path, day: &str, asked: &AskedActivity) -> PathBuf {
    use sha2::{Digest, Sha256};
    let identity = crate::ActivityRetry {
        day: day.into(),
        facet: asked.facet.clone(),
        activity: asked.activity.clone(),
    };
    root.join("health/activity-work").join(format!(
        "{:x}.json",
        Sha256::digest(serde_json::to_vec(&identity).unwrap())
    ))
}

fn edit_record(path: &Path, edit: impl FnOnce(&mut serde_json::Value)) {
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    edit(&mut record);
    std::fs::write(path, record.to_string()).unwrap();
}

fn briefing(root: &Path, day: &str) -> daily_coverage::UnitCoverage {
    daily_coverage::read_daily_coverage(root, day)
        .unwrap()
        .units
        .into_iter()
        .find(|unit| unit.identity.name == "morning_briefing")
        .expect("the briefing is a daily unit")
}

/// A briefing accepted at `revision`, its last attempt left as `status`.
fn accept_briefing(
    root: &Path,
    day: &str,
    revision: &str,
    committed_at_ms: i64,
    status: solstone_core_journal_io::DailyUnitStatus,
) {
    let unit = briefing(root, day);
    let mut record = solstone_core_journal_io::DailyUnitRecord::new(
        unit.identity.clone(),
        revision,
        &unit.contract_digest,
    );
    record.status = status;
    record.accepted = Some(solstone_core_journal_io::AcceptedDailyResult {
        evidence_revision: revision.into(),
        contract_digest: unit.contract_digest.clone(),
        status: solstone_core_journal_io::DailyUnitStatus::CommittedNoOutput,
        packet_digest: Some("a".repeat(64)),
        generated_result: Some(serde_json::json!({"response": "done", "output": "done"})),
        receipts: Vec::new(),
        committed_at_ms,
    });
    solstone_core_journal_io::save_daily_unit_record(root, &record).unwrap();
}

fn asks(root: &Path) -> Vec<owner_ask::OwnerAsk> {
    owner_ask::read(root).unwrap().into_values().collect()
}

fn inventory(root: &Path) -> WorkInventory {
    solstone_core_system::work_inventory::read(root)
        .unwrap()
        .unwrap()
}

fn daily_for(plan: &[(String, WorkBand)], day: &str) -> bool {
    plan.iter().any(|(reference, _)| {
        reference == &format!("supervisor-work-{day}-daily")
            || reference.starts_with(&format!("supervisor-retry-daily-{day}-"))
    })
}

/// Every task one pass would hand the queue, in rank order, with its band.
fn plan(root: &Path, queue: &TaskQueue) -> Vec<(String, WorkBand)> {
    let backlog = reconcile_pass(root, queue, &AtomicBool::new(false), false).unwrap();
    backlog
        .admitted
        .iter()
        .chain(backlog.ranked.iter())
        .map(|task| {
            (
                task.request.reference.clone(),
                task.request.priority.unwrap().band,
            )
        })
        .collect()
}

fn without(plan: &[(String, WorkBand)], reference: &str) -> Vec<(String, WorkBand)> {
    plan.iter()
        .filter(|(r, _)| r != reference)
        .cloned()
        .collect()
}

fn activity_reference(day: &str, asked: &AskedActivity) -> String {
    let unit = Unit::Activity {
        day: day.into(),
        facet: asked.facet.clone(),
        activity: asked.activity.clone(),
        talent: "activity".into(),
    };
    format!("supervisor-activity-{day}-{}", unit.id().unwrap())
}

#[test]
fn an_ask_runs_exactly_its_activity_in_band_two_as_a_full_refresh() {
    let root = journal();
    let day = days_ago(90);
    let asked = activity(root.path(), &day, "reading_090000_300", "090000_300");
    let sibling = activity(root.path(), &day, "reading_100000_300", "100000_300");
    let elsewhere = activity(
        root.path(),
        &days_ago(91),
        "reading_090000_300",
        "090000_300",
    );
    let queue = queue(root.path());
    let before = plan(root.path(), &queue);
    owner_ask::record(root.path(), &day, std::slice::from_ref(&asked), false, 1).unwrap();
    let after = plan(root.path(), &queue);
    let reference = activity_reference(&day, &asked);
    assert!(
        after.contains(&(reference.clone(), WorkBand::Waiting)),
        "{after:?}"
    );
    assert_eq!(without(&after, &reference), before);
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(work_record(root.path(), &day, &asked)).unwrap())
            .unwrap();
    assert_eq!(record["refresh"], true);
    assert!(!work_record(root.path(), &day, &sibling).exists());
    assert!(!work_record(root.path(), &days_ago(91), &elsewhere).exists());
    assert_eq!(asks(root.path())[0].seeded, BTreeSet::from([asked]));
}

#[test]
fn an_asked_day_with_its_activity_work_queued_is_owner_work_until_it_leaves() {
    let root = journal();
    let day = days_ago(90);
    let asked = activity(root.path(), &day, "reading_090000_300", "090000_300");
    let queue = queue(root.path());
    plan(root.path(), &queue);
    assert!(owner_days_in_queue(root.path(), &queue).is_empty());
    owner_ask::record(root.path(), &day, std::slice::from_ref(&asked), true, 1).unwrap();
    plan(root.path(), &queue);
    assert!(queue.contains_reference(&activity_reference(&day, &asked)));
    assert_eq!(
        owner_days_in_queue(root.path(), &queue)
            .keys()
            .collect::<Vec<_>>(),
        [&day]
    );
    queue.replace_discovered(Vec::new());
    assert!(owner_days_in_queue(root.path(), &queue).is_empty());
}

#[test]
fn an_ask_on_today_keeps_today_in_band_one_and_adds_no_daily_work() {
    let root = journal();
    let day = days_ago(0);
    let asked = activity(root.path(), &day, "reading_090000_300", "090000_300");
    let queue = queue(root.path());
    let before = plan(root.path(), &queue);
    owner_ask::record(root.path(), &day, std::slice::from_ref(&asked), true, 1).unwrap();
    let after = plan(root.path(), &queue);
    let reference = activity_reference(&day, &asked);
    assert!(
        after.contains(&(reference.clone(), WorkBand::Now)),
        "{after:?}"
    );
    assert_eq!(without(&after, &reference), before);
    assert!(
        before.iter().all(|(_, band)| *band == WorkBand::Now),
        "{before:?}"
    );
}

#[test]
fn a_journal_without_asks_plans_exactly_as_with_the_ask_reader_disabled() {
    let root = journal();
    let day = days_ago(90);
    thought(root.path(), &day, &["090000_300", "100000_300"]);
    activity(
        root.path(),
        &days_ago(3),
        "reading_090000_300",
        "090000_300",
    );
    let queue = queue(root.path());
    let without_file = plan(root.path(), &queue);
    let report = inventory(root.path());
    assert!(report.owner_asks.is_none());
    assert!(
        !report
            .unreadable
            .iter()
            .any(|e| e.starts_with("owner asks:"))
    );
    crate::owner_asks::READER_DISABLED.with(|disabled| disabled.set(true));
    let disabled = plan(root.path(), &queue);
    crate::owner_asks::READER_DISABLED.with(|disabled| disabled.set(false));
    assert_eq!(without_file, disabled);
    assert!(daily_for(&without_file, &day));
}

#[test]
fn a_damaged_or_newer_store_plans_as_none_and_is_reported() {
    let root = journal();
    let day = days_ago(90);
    thought(root.path(), &day, &["090000_300"]);
    let asked = activity(root.path(), &day, "reading_090000_300", "090000_300");
    let queue = queue(root.path());
    let none = plan(root.path(), &queue);
    for bytes in [
        "{".to_owned(),
        serde_json::json!({"version": owner_ask::STORE_VERSION + 1, "asks": {}}).to_string(),
    ] {
        std::fs::write(root.path().join("health/owner-asks.json"), &bytes).unwrap();
        assert_eq!(plan(root.path(), &queue), none);
        let report = inventory(root.path());
        assert!(report.owner_asks.unwrap().unreadable);
        assert!(
            report
                .unreadable
                .iter()
                .any(|e| e.starts_with("owner asks:"))
        );
        assert!(!owner_ask::thawed(root.path(), &day));
        assert!(matches!(
            owner_ask::record(root.path(), &day, std::slice::from_ref(&asked), true, 1),
            Err(owner_ask::AskError::Unreadable(_))
        ));
        assert!(!work_record(root.path(), &day, &asked).exists());
        assert_eq!(
            std::fs::read_to_string(root.path().join("health/owner-asks.json")).unwrap(),
            bytes
        );
    }
}

#[test]
fn the_day_waits_for_its_asked_activity_however_its_work_stands() {
    let root = journal();
    let day = days_ago(90);
    thought(root.path(), &day, &["090000_300", "100000_300"]);
    let asked = activity(root.path(), &day, "reading_090000_300", "090000_300");
    let queue = queue(root.path());
    assert!(daily_for(&plan(root.path(), &queue), &day));
    owner_ask::record(root.path(), &day, std::slice::from_ref(&asked), true, 1).unwrap();
    let record = work_record(root.path(), &day, &asked);
    let activity_task = activity_reference(&day, &asked);
    // The seeding pass, and the next one before the queue has run anything.
    for _ in 0..2 {
        let plan = plan(root.path(), &queue);
        assert!(plan.iter().any(|(r, _)| *r == activity_task), "{plan:?}");
        assert!(!daily_for(&plan, &day), "{plan:?}");
    }
    assert!(queue.contains_reference(&activity_task));
    assert!(
        inventory(root.path())
            .holds
            .iter()
            .any(|hold| hold.day == day && hold.reason == "owner_ask_activities")
    );
    // Its talents dispatched and running.
    edit_record(&record, |r| {
        r["uses"] = serde_json::json!({"participation": "1"})
    });
    assert!(!daily_for(&plan(root.path(), &queue), &day));
    // A failure that reached no endpoint waits out its backoff, so the
    // activity is not even offered; the day still waits.
    edit_record(&record, |r| {
        r["uses"] = serde_json::json!({});
        r["attempts"] = serde_json::json!(1);
        r["next_attempt_ms"] = serde_json::json!(i64::MAX);
    });
    let waiting = plan(root.path(), &queue);
    assert!(
        !waiting.iter().any(|(r, _)| *r == activity_task),
        "{waiting:?}"
    );
    assert!(!daily_for(&waiting, &day));
    // Parked work no longer holds the day.
    edit_record(&record, |r| r["parked"] = r["remaining"].clone());
    assert!(daily_for(&plan(root.path(), &queue), &day));
    // Nor does work that finished; its daily work then passes admission.
    std::fs::remove_file(&record).unwrap();
    let backlog = reconcile_pass(root.path(), &queue, &AtomicBool::new(false), false).unwrap();
    assert!(
        backlog
            .admitted
            .iter()
            .any(|task| task.request.reference == format!("supervisor-work-{day}-daily")),
        "{:?}",
        backlog
            .admitted
            .iter()
            .map(|t| &t.request.reference)
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_frozen_briefing_regenerates_while_thawed_and_freezes_again_once_current() {
    let root = journal();
    let day = days_ago(90);
    thought(root.path(), &day, &["090000_300"]);
    accept_briefing(
        root.path(),
        &day,
        "earlier evidence",
        1,
        solstone_core_journal_io::DailyUnitStatus::CommittedNoOutput,
    );
    let frozen = briefing(root.path(), &day);
    assert!(frozen.state.is_current());
    assert_eq!(
        frozen.earlier_version,
        Some(daily_coverage::AcceptedReuse::FrozenBriefing)
    );
    // An ask without a thaw leaves it frozen.
    let plain = activity(root.path(), &day, "reading_090000_300", "090000_300");
    owner_ask::record(root.path(), &day, &[plain], false, 1).unwrap();
    assert!(briefing(root.path(), &day).state.is_current());
    // A thaw makes it owed, and it stays owed through a pass.
    owner_ask::record(root.path(), &day, &[], true, 2).unwrap();
    assert!(briefing(root.path(), &day).state.is_owed());
    let queue = queue(root.path());
    plan(root.path(), &queue);
    assert!(asks(root.path()).iter().any(|a| a.thaw && !a.thaw_done));
    // A briefing committed after the ask from an older packet is still owed.
    accept_briefing(
        root.path(),
        &day,
        "a packet prepared before the ask",
        chrono::Utc::now().timestamp_millis(),
        solstone_core_journal_io::DailyUnitStatus::CommittedNoOutput,
    );
    plan(root.path(), &queue);
    assert!(briefing(root.path(), &day).state.is_owed());
    assert!(asks(root.path()).iter().any(|a| a.thaw && !a.thaw_done));
    // Accepted at the current evidence: the thaw ends and the day freezes again.
    let current = briefing(root.path(), &day).evidence_revision;
    accept_briefing(
        root.path(),
        &day,
        &current,
        chrono::Utc::now().timestamp_millis(),
        solstone_core_journal_io::DailyUnitStatus::CommittedNoOutput,
    );
    std::fs::remove_dir_all(root.path().join("health/activity-work")).unwrap();
    plan(root.path(), &queue);
    assert!(asks(root.path()).iter().all(|a| !a.thaw || a.thaw_done));
    assert!(!owner_ask::thawed(root.path(), &day));
    thought(root.path(), &day, &["100000_300"]);
    let later = briefing(root.path(), &day);
    assert_ne!(later.evidence_revision, current);
    assert_eq!(
        later.earlier_version,
        Some(daily_coverage::AcceptedReuse::FrozenBriefing)
    );
}

#[test]
fn an_unchanged_or_capped_briefing_ends_the_thaw() {
    for capped in [false, true] {
        let root = journal();
        let day = days_ago(90);
        thought(root.path(), &day, &["090000_300"]);
        let current = briefing(root.path(), &day).evidence_revision;
        if capped {
            accept_briefing(
                root.path(),
                &day,
                "earlier evidence",
                1,
                solstone_core_journal_io::DailyUnitStatus::CommittedNoOutput,
            );
            let unit = briefing(root.path(), &day);
            let mut record =
                solstone_core_journal_io::load_daily_unit_record(root.path(), &unit.identity)
                    .unwrap()
                    .unwrap();
            record.evidence_revision = current.clone();
            record.status = solstone_core_journal_io::DailyUnitStatus::Capped;
            record.reason_code = Some("model_not_found".into());
            record.failure_count = 1;
            solstone_core_journal_io::save_daily_unit_record(root.path(), &record).unwrap();
        } else {
            accept_briefing(
                root.path(),
                &day,
                &current,
                1,
                solstone_core_journal_io::DailyUnitStatus::CommittedNoOutput,
            );
        }
        let ask = owner_ask::record(root.path(), &day, &[], true, 2).unwrap();
        let queue = queue(root.path());
        let plan = plan(root.path(), &queue);
        assert!(!daily_for(&plan, &day), "{plan:?}");
        // The week page for that long-ended week is absent, so the step ends
        // and the finished ask is removed.
        assert!(!owner_ask::read(root.path()).unwrap().contains_key(&ask.id));
        let progress = inventory(root.path()).owner_asks.unwrap();
        assert_eq!(progress.thaw_capped, u64::from(capped));
        assert_eq!(progress.week_unfinished.get("page_absent"), Some(&1));
        assert!(!owner_ask::thawed(root.path(), &day));
    }
}

#[test]
fn a_later_ask_still_holding_activities_keeps_an_earlier_thaw_open() {
    let root = journal();
    let day = days_ago(90);
    thought(root.path(), &day, &["090000_300"]);
    let current = briefing(root.path(), &day).evidence_revision;
    accept_briefing(
        root.path(),
        &day,
        &current,
        1,
        solstone_core_journal_io::DailyUnitStatus::CommittedNoOutput,
    );
    let a = owner_ask::record(root.path(), &day, &[], true, 2).unwrap();
    let asked = activity(root.path(), &day, "reading_090000_300", "090000_300");
    owner_ask::record(root.path(), &day, std::slice::from_ref(&asked), false, 3).unwrap();
    let queue = queue(root.path());
    plan(root.path(), &queue);
    assert!(!owner_ask::read(root.path()).unwrap()[&a.id].thaw_done);
    // The asked activity's work finishes; then the briefing is accepted.
    std::fs::remove_file(work_record(root.path(), &day, &asked)).unwrap();
    let current = briefing(root.path(), &day).evidence_revision;
    accept_briefing(
        root.path(),
        &day,
        &current,
        chrono::Utc::now().timestamp_millis(),
        solstone_core_journal_io::DailyUnitStatus::CommittedNoOutput,
    );
    plan(root.path(), &queue);
    assert!(!owner_ask::read(root.path()).unwrap().contains_key(&a.id));
}

#[test]
fn a_thawed_briefing_after_a_capped_attempt_retries_once_through_its_retry() {
    let root = journal();
    let day = days_ago(90);
    thought(root.path(), &day, &["090000_300"]);
    published(root.path(), &day, &["090000_300"]);
    accept_briefing(
        root.path(),
        &day,
        "evidence zero",
        1,
        solstone_core_journal_io::DailyUnitStatus::CommittedNoOutput,
    );
    let unit = briefing(root.path(), &day);
    let mut record = solstone_core_journal_io::load_daily_unit_record(root.path(), &unit.identity)
        .unwrap()
        .unwrap();
    record.evidence_revision = "evidence one".into();
    record.status = solstone_core_journal_io::DailyUnitStatus::Capped;
    record.reason_code = Some("schema_invalid".into());
    record.failure_count = 3;
    solstone_core_journal_io::save_daily_unit_record(root.path(), &record).unwrap();
    let retry = |plan: &[(String, WorkBand)]| {
        plan.iter()
            .filter(|(r, _)| r.starts_with(&format!("supervisor-retry-daily-{day}-")))
            .count()
    };
    let queue = queue(root.path());
    assert_eq!(retry(&plan(root.path(), &queue)), 0);
    owner_ask::record(root.path(), &day, &[], true, 2).unwrap();
    let thawed = plan(root.path(), &queue);
    assert_eq!(retry(&thawed), 1, "{thawed:?}");
    assert!(thawed.iter().any(|(r, band)| r.starts_with("supervisor-retry-daily-") && *band == WorkBand::Waiting));
    let after = solstone_core_journal_io::load_daily_unit_record(root.path(), &unit.identity)
        .unwrap()
        .unwrap();
    assert_eq!(after.failure_count, 3);
    assert_eq!(after.evidence_revision, "evidence one");
    assert_eq!(
        after.status,
        solstone_core_journal_io::DailyUnitStatus::Capped
    );
}

#[test]
fn a_catchup_backoff_holds_the_day_only_when_earned_after_the_ask() {
    let root = journal();
    let day = days_ago(90);
    thought(root.path(), &day, &["090000_300"]);
    let fingerprint =
        solstone_core_system::catchup::read_raw_input_fingerprint(root.path(), &day).unwrap();
    let backoff = |last_attempt_at: f64| {
        std::fs::create_dir_all(root.path().join("health")).unwrap();
        std::fs::write(
            root.path().join("health/catchup-state.json"),
            serde_json::json!({"version": 1, "entries": {format!("{day}:daily-catchup"): {
                "day": day, "command_kind": "daily-catchup", "last_attempt_at": last_attempt_at,
                "next_retry_at": 1e12, "fingerprint": fingerprint, "reason_code": "daily_failed"
            }}})
            .to_string(),
        )
        .unwrap();
    };
    let queue = queue(root.path());
    backoff(1000.0);
    assert!(!daily_for(&plan(root.path(), &queue), &day));
    owner_ask::record(root.path(), &day, &[], true, 1_000_001).unwrap();
    assert!(daily_for(&plan(root.path(), &queue), &day));
    backoff(1001.0);
    assert!(!daily_for(&plan(root.path(), &queue), &day));
}

#[test]
fn owner_intents_leave_asks_alone_and_hold_them_until_they_end() {
    let root = journal();
    let day = days_ago(90);
    let asked = activity(root.path(), &day, "reading_090000_300", "090000_300");
    let ask = owner_ask::record(root.path(), &day, std::slice::from_ref(&asked), true, 1).unwrap();
    let store = || std::fs::read(root.path().join("health/owner-asks.json")).unwrap();
    let saved = store();
    work_intent::register(root.path(), &day, false).unwrap();
    assert_eq!(store(), saved);
    let rebuild = work_intent::register(root.path(), &day, true).unwrap();
    assert_eq!(store(), saved);
    let queue = queue(root.path());
    plan(root.path(), &queue);
    assert_eq!(owner_ask::read(root.path()).unwrap()[&ask.id], ask);
    assert!(!work_record(root.path(), &day, &asked).exists());
    let waiting = |reason: &str| {
        inventory(root.path())
            .owner_asks
            .unwrap()
            .pending
            .iter()
            .any(|p| p.id == ask.id && p.waiting_on == reason)
    };
    assert!(waiting("from_scratch"));
    let lease = work_intent::claim_from_scratch(root.path(), &day, &rebuild.id).unwrap();
    plan(root.path(), &queue);
    assert!(waiting("from_scratch"), "a live rebuild holds the ask");
    work_intent::stop_rebuilds(root.path()).unwrap();
    drop(lease);
    assert_eq!(store(), saved);
    plan(root.path(), &queue);
    assert!(waiting("owner_stopped"));
    assert!(!work_record(root.path(), &day, &asked).exists());
    let next = work_intent::register(root.path(), &day, true).unwrap();
    let lease = work_intent::claim_from_scratch(root.path(), &day, &next.id).unwrap();
    // An unsuccessful rebuild becomes the ordinary ask, which holds no ask.
    work_intent::finish_from_scratch(root.path(), &day, &next.id, false).unwrap();
    drop(lease);
    assert_eq!(store(), saved);
    plan(root.path(), &queue);
    assert!(work_record(root.path(), &day, &asked).exists());
    assert!(
        owner_ask::read(root.path()).unwrap()[&ask.id]
            .to_seed
            .is_empty()
    );
}

#[test]
fn an_asked_activity_whose_work_is_set_aside_leaves_the_ask_as_dropped() {
    let root = journal();
    let day = days_ago(90);
    let asked = activity(root.path(), &day, "reading_090000_300", "090000_300");
    let ask = owner_ask::record(root.path(), &day, std::slice::from_ref(&asked), false, 1).unwrap();
    let queue = queue(root.path());
    plan(root.path(), &queue);
    let seeded = owner_ask::read(root.path()).unwrap()[&ask.id].clone();
    assert_eq!(seeded.seeded, BTreeSet::from([asked.clone()]));
    // Still running, it stays seeded.
    plan(root.path(), &queue);
    assert_eq!(owner_ask::read(root.path()).unwrap()[&ask.id], seeded);

    edit_record(&work_record(root.path(), &day, &asked), |record| {
        record["blocked"] = serde_json::json!({"reason_code": "fixture", "message": "fixture"});
    });
    plan(root.path(), &queue);
    let dropped = owner_ask::read(root.path()).unwrap()[&ask.id].clone();
    assert!(dropped.seeded.is_empty(), "{dropped:?}");
    assert_eq!(
        dropped.dropped,
        BTreeMap::from([(crate::owner_asks::ACTIVITY_SET_ASIDE.to_owned(), 1)])
    );
}

#[test]
fn an_activity_that_cannot_run_again_is_dropped_and_counted() {
    let root = journal();
    let day = days_ago(90);
    let real = activity(root.path(), &day, "reading_090000_300", "090000_300");
    let missing = AskedActivity {
        facet: "work".into(),
        activity: "reading_110000_300".into(),
    };
    let broken = activity(root.path(), &day, "reading_100000_300", "100000_300");
    std::fs::create_dir_all(root.path().join("health/activity-work")).unwrap();
    std::fs::write(work_record(root.path(), &day, &broken), "{").unwrap();
    let ask = owner_ask::record(
        root.path(),
        &day,
        &[real.clone(), missing, broken],
        false,
        1,
    )
    .unwrap();
    // A held claim is a running activity: its name stays for a later pass.
    let context = crate::context::ThinkContext::new(
        root.path(),
        day.clone(),
        root.path().join("chronicle").join(&day),
        1,
    )
    .unwrap();
    let claim_path =
        crate::segment::activity_provenance_path(&context, &day, &real.facet, &real.activity);
    std::fs::create_dir_all(claim_path.parent().unwrap()).unwrap();
    let claim = solstone_core_journal_io::hold_lock(&claim_path, Default::default()).unwrap();
    let queue = queue(root.path());
    plan(root.path(), &queue);
    let held = owner_ask::read(root.path()).unwrap()[&ask.id].clone();
    assert_eq!(held.to_seed, BTreeSet::from([real.clone()]));
    assert_eq!(held.dropped.get("activity_missing"), Some(&1));
    assert_eq!(held.dropped.get("work_record_unreadable"), Some(&1));
    drop(claim);
    plan(root.path(), &queue);
    let seeded = owner_ask::read(root.path()).unwrap()[&ask.id].clone();
    assert!(seeded.to_seed.is_empty());
    assert_eq!(seeded.seeded, BTreeSet::from([real]));
    assert_eq!(
        inventory(root.path())
            .owner_asks
            .unwrap()
            .dropped
            .values()
            .sum::<u64>(),
        2
    );
}

#[test]
fn an_asked_activity_offered_again_through_its_retry_takes_band_two() {
    let root = journal();
    let day = days_ago(90);
    let asked = activity(root.path(), &day, "reading_090000_300", "090000_300");
    let ask = owner_ask::record(root.path(), &day, std::slice::from_ref(&asked), false, 1).unwrap();
    let queue = queue(root.path());
    plan(root.path(), &queue);
    // Every talent failed again and again and was parked on an older input.
    edit_record(&work_record(root.path(), &day, &asked), |r| {
        r["parked"] = r["remaining"].clone();
        r["input_hash"] = serde_json::json!("an older input");
    });
    let retries = |plan: &[(String, WorkBand)]| {
        plan.iter()
            .filter(|(r, _)| r.starts_with(&format!("supervisor-retry-talent-{day}-")))
            .map(|(_, band)| *band)
            .collect::<Vec<_>>()
    };
    let asked_plan = plan(root.path(), &queue);
    let bands = retries(&asked_plan);
    assert!(!bands.is_empty(), "{asked_plan:?}");
    assert!(bands.iter().all(|band| *band == WorkBand::Waiting));
    owner_ask::update(root.path(), &ask.id, |_| Some(None)).unwrap();
    let bands = retries(&plan(root.path(), &queue));
    assert!(!bands.is_empty());
    assert!(bands.iter().all(|band| *band > WorkBand::Waiting));
}

const WEEK: [&str; 3] = ["20260809", "20260812", "20260815"];
const WEEK_TASK: &str = "supervisor-owner-week-20260809";

fn page(root: &Path, generated_at_ms: Option<i64>) {
    let days = (9..=15)
        .map(|d| serde_json::json!({"day": format!("202608{d:02}"), "state": "nothing_shared"}))
        .collect::<Vec<_>>();
    let mut body = serde_json::json!({
        "version": 1,
        "week": {"start": "20260809", "end": "20260815"},
        "selection": {"status": "none", "model": null},
        "intro": [], "memories": [], "days": days, "said": []
    });
    if let Some(at) = generated_at_ms {
        body["generated_at"] = serde_json::json!(
            chrono::DateTime::from_timestamp_millis(at)
                .unwrap()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        );
    }
    let path = root.join("reflections/weekly/20260809.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body.to_string()).unwrap();
}

/// Thought days whose briefings are current, each with a thaw ask.
fn settled_week(root: &Path, committed_at_ms: i64, asked_at_ms: i64) {
    // Facet declarations are part of every briefing's contract.
    let _ = solstone_core_facets::create_facet(root, "work", "Work", "", "", "", None);
    for day in WEEK {
        thought(root, day, &["090000_300"]);
        let current = briefing(root, day).evidence_revision;
        accept_briefing(
            root,
            day,
            &current,
            committed_at_ms,
            solstone_core_journal_io::DailyUnitStatus::CommittedNoOutput,
        );
        owner_ask::record(root, day, &[], true, asked_at_ms).unwrap();
    }
}

fn week_task(plan: &[(String, WorkBand)]) -> Vec<WorkBand> {
    plan.iter()
        .filter(|(r, _)| r == WEEK_TASK)
        .map(|(_, band)| *band)
        .collect()
}

fn week_steps(root: &Path) -> Vec<owner_ask::WeekStep> {
    asks(root).into_iter().map(|ask| ask.week).collect()
}

#[test]
fn asks_across_an_ended_week_remake_its_page_once_every_day_is_settled() {
    let root = journal();
    let now = chrono::Utc::now().timestamp_millis();
    // One day's facet makes daily outputs other than its briefing owed there.
    thought(root.path(), "20260812", &["090000_300"]);
    std::fs::create_dir_all(
        root.path()
            .join("chronicle/20260812/notes/090000_300/talents"),
    )
    .unwrap();
    std::fs::write(
        root.path()
            .join("chronicle/20260812/notes/090000_300/talents/facets.json"),
        r#"[{"facet":"work"}]"#,
    )
    .unwrap();
    settled_week(root.path(), 1, now - 60_000);
    page(root.path(), Some(now - 120_000));
    // The last day still waits for an asked activity.
    let asked = activity(root.path(), "20260815", "reading_100000_300", "100000_300");
    owner_ask::record(
        root.path(),
        "20260815",
        std::slice::from_ref(&asked),
        false,
        now - 60_000,
    )
    .unwrap();
    let queue = queue(root.path());
    assert!(week_task(&plan(root.path(), &queue)).is_empty());
    // Other daily outputs of the day stay owed; only briefings decide the week.
    assert!(
        daily_coverage::read_daily_coverage(root.path(), "20260812")
            .unwrap()
            .state
            .is_owed()
    );
    // Its work finishes and the day's briefing is made again from it.
    std::fs::remove_file(work_record(root.path(), "20260815", &asked)).unwrap();
    assert!(week_task(&plan(root.path(), &queue)).is_empty());
    let current = briefing(root.path(), "20260815").evidence_revision;
    accept_briefing(
        root.path(),
        "20260815",
        &current,
        now - 30_000,
        solstone_core_journal_io::DailyUnitStatus::CommittedNoOutput,
    );
    let backlog = reconcile_pass(root.path(), &queue, &AtomicBool::new(false), false).unwrap();
    let tasks = backlog
        .admitted
        .iter()
        .chain(&backlog.ranked)
        .filter(|task| task.request.reference == WEEK_TASK)
        .collect::<Vec<_>>();
    assert_eq!(tasks.len(), 1);
    assert_eq!(
        tasks[0].request.cmd.as_wire()[tasks[0].request.cmd.as_wire().len() - 4..],
        ["think", "--day", "20260815", "--weekly"]
    );
    assert_eq!(tasks[0].request.priority.unwrap().band, WorkBand::Waiting);
    assert!(queue.contains_reference(WEEK_TASK));
    // The page run is the owner's work on every asked day of the week, so
    // its end starts the pass that sees the page.
    let owner_days = owner_days_in_queue(root.path(), &queue);
    assert!(owner_days.contains_key("20260815"), "{owner_days:?}");
    assert!(
        owner_days
            .values()
            .all(|week| week.as_deref() == Some(WEEK_TASK))
    );
    assert!(
        week_steps(root.path())
            .iter()
            .all(|step| step.attempts == 1)
    );
    // The page is made after the asks: every step ends and the asks are done.
    page(root.path(), Some(chrono::Utc::now().timestamp_millis() + 1));
    assert!(week_task(&plan(root.path(), &queue)).is_empty());
    assert!(asks(root.path()).is_empty());
}

#[test]
fn a_page_made_from_a_briefing_older_than_the_last_one_is_made_again() {
    let root = journal();
    let now = chrono::Utc::now().timestamp_millis();
    // The ask came first; the page was built before the latest briefing commit.
    settled_week(root.path(), now - 10_000, now - 30_000);
    page(root.path(), Some(now - 20_000));
    let queue = queue(root.path());
    assert_eq!(
        week_task(&plan(root.path(), &queue)),
        vec![WorkBand::Waiting]
    );
    // Twin: a page built after both ends the step.
    page(root.path(), Some(now - 5_000));
    assert!(week_task(&plan(root.path(), &queue)).is_empty());
    assert!(asks(root.path()).is_empty());
}

#[test]
fn a_page_that_cannot_be_trusted_or_read_ends_the_step_without_a_run() {
    for (case, reason) in [
        ("absent", "page_absent"),
        ("unshowable", "page_unshowable"),
        ("untrusted", "page_untrusted"),
        ("unreadable", "page_unreadable"),
    ] {
        let root = journal();
        let now = chrono::Utc::now().timestamp_millis();
        settled_week(root.path(), 1, now - 60_000);
        let weekly = root.path().join("reflections/weekly");
        std::fs::create_dir_all(&weekly).unwrap();
        match case {
            "absent" => {}
            "unshowable" => std::fs::write(weekly.join("20260809.md"), "a page").unwrap(),
            "untrusted" => {
                std::fs::write(weekly.join("20260809.json"), r#"{"version":2}"#).unwrap()
            }
            _ => std::fs::create_dir_all(weekly.join("20260809.json")).unwrap(),
        }
        let queue = queue(root.path());
        assert!(week_task(&plan(root.path(), &queue)).is_empty(), "{case}");
        assert!(asks(root.path()).is_empty(), "{case}");
        let progress = inventory(root.path()).owner_asks.unwrap();
        assert_eq!(progress.week_unfinished.get(reason), Some(&3), "{case}");
    }
}

#[test]
fn a_page_with_malformed_content_is_made_again_and_counted() {
    let root = journal();
    let now = chrono::Utc::now().timestamp_millis();
    settled_week(root.path(), 1, now - 60_000);
    let weekly = root.path().join("reflections/weekly");
    std::fs::create_dir_all(&weekly).unwrap();
    std::fs::write(weekly.join("20260809.json"), "{").unwrap();
    let queue = queue(root.path());
    assert_eq!(
        week_task(&plan(root.path(), &queue)),
        vec![WorkBand::Waiting]
    );
    assert!(
        week_steps(root.path())
            .iter()
            .all(|step| step.attempts == 1 && step.ended.is_none())
    );
}

#[test]
fn an_ask_in_a_week_that_has_not_ended_queues_no_page_run() {
    let root = journal();
    let day = days_ago(0);
    thought(root.path(), &day, &["090000_300"]);
    let queue = queue(root.path());
    // A first pass settles today's daily inputs; then every other step is
    // done, so only the week holds the ask.
    plan(root.path(), &queue);
    let current = briefing(root.path(), &day).evidence_revision;
    accept_briefing(
        root.path(),
        &day,
        &current,
        1,
        solstone_core_journal_io::DailyUnitStatus::CommittedNoOutput,
    );
    let ask = owner_ask::record(root.path(), &day, &[], true, 1).unwrap();
    let plan = plan(root.path(), &queue);
    assert!(
        !plan
            .iter()
            .any(|(r, _)| r.starts_with("supervisor-owner-week-"))
    );
    let after = owner_ask::read(root.path()).unwrap()[&ask.id].clone();
    assert!(after.thaw_done);
    assert_eq!(after.week.ended, None);
}

#[test]
fn a_page_run_is_offered_every_pass_and_counted_at_most_once_an_hour_up_to_three() {
    let root = journal();
    let now = chrono::Utc::now().timestamp_millis();
    settled_week(root.path(), 1, now - 60_000);
    page(root.path(), Some(now - 120_000));
    let queue = queue(root.path());
    // Queued and not started, across three passes.
    for _ in 0..3 {
        assert_eq!(
            week_task(&plan(root.path(), &queue)),
            vec![WorkBand::Waiting]
        );
        assert!(queue.contains_reference(WEEK_TASK));
    }
    assert!(
        week_steps(root.path())
            .iter()
            .all(|step| step.attempts == 1)
    );
    // One run finished without making the page newer; three more passes.
    queue.replace_discovered(Vec::new());
    page(root.path(), Some(now - 110_000));
    for _ in 0..3 {
        assert_eq!(
            week_task(&plan(root.path(), &queue)),
            vec![WorkBand::Waiting]
        );
    }
    assert!(
        week_steps(root.path())
            .iter()
            .all(|step| step.attempts == 1)
    );
    // An hour on, a second and third attempt are counted; then the bound ends it.
    for expected in [2, 3] {
        for ask in asks(root.path()) {
            owner_ask::update(root.path(), &ask.id, |current| {
                let mut next = current.clone();
                next.week.counted_at_ms = Some(now - crate::owner_asks::WEEK_ATTEMPT_SPACING_MS);
                Some(Some(next))
            })
            .unwrap();
        }
        assert_eq!(
            week_task(&plan(root.path(), &queue)),
            vec![WorkBand::Waiting]
        );
        assert!(
            week_steps(root.path())
                .iter()
                .all(|step| step.attempts == expected)
        );
    }
    for ask in asks(root.path()) {
        owner_ask::update(root.path(), &ask.id, |current| {
            let mut next = current.clone();
            next.week.counted_at_ms = Some(now - crate::owner_asks::WEEK_ATTEMPT_SPACING_MS);
            Some(Some(next))
        })
        .unwrap();
    }
    assert!(week_task(&plan(root.path(), &queue)).is_empty());
    assert!(asks(root.path()).is_empty());
    let progress = inventory(root.path()).owner_asks.unwrap();
    assert_eq!(progress.week_unfinished.get("attempts_exhausted"), Some(&3));
}
