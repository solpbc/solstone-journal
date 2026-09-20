// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use super::*;
use crate::activity_work::{due_activity_retries, seed_activity_retries};
fn fixture() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    context::ThinkContext,
    Arc<Recorder>,
) {
    let journal = tempdir().unwrap();
    let roots = tempdir().unwrap();
    let (talent_root, apps_root) = talent_roots(
        roots.path(),
        &[(
            "participation",
            "{\n\"type\":\"generate\",\"schedule\":\"activity\",\"priority\":1,\"output\":\"json\",\"activities\":[\"work\"]\n}",
        )],
    );
    let (context, recorder) = recorder_context(journal.path(), "20260813", 1_786_615_200_000);
    solstone_core_facets::create_facet(journal.path(), "work", "Work", "", "", "", None).unwrap();
    let context = context.with_talent_roots(talent_root, apps_root);
    for (key, sense) in [
        (
            "090000_300",
            serde_json::json!({"density":"active","content_type":"work","activity_summary":"test work","facets":[{"facet":"work","level":"high","activity":"work"}]}),
        ),
        (
            "090500_300",
            serde_json::json!({"density":"idle","content_type":"idle","facets":[]}),
        ),
        (
            "091000_300",
            serde_json::json!({"density":"idle","content_type":"idle","facets":[]}),
        ),
    ] {
        let path = segment_dir(journal.path(), "20260813", key).join("talents");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("sense.json"), serde_json::to_vec(&sense).unwrap()).unwrap();
    }
    (journal, roots, context, recorder)
}

fn replay(context: &context::ThinkContext, keys: &[&str], hydrate: bool) -> Result<(), String> {
    let mut log = test_log(context, "activity-investigation");
    let segments = keys
        .iter()
        .map(|key| ((*key).to_owned(), Some("default".to_owned())))
        .collect::<Vec<_>>();
    segment::replay_activity_state(context, &mut log, &segments, false, 1, false, hydrate)
}

fn fail_first(context: &context::ThinkContext, recorder: &Recorder) {
    recorder.end_states.lock().unwrap().insert(
        "use-1".to_owned(),
        solstone_core_cortex_client::UseEndState::Error,
    );
    let path = context.journal.join("talents/participation/use-1.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, concat!(
            "{\"event\":\"start\",\"use_id\":\"use-1\"}\n",
            "{\"event\":\"error\",\"terminal\":true,\"use_id\":\"use-1\",\"reason_code\":\"local_endpoint_unreachable\",\"retryable\":true,\"blocking\":true}\n"
        )).unwrap();
}

fn provenance(context: &context::ThinkContext) -> std::path::PathBuf {
    context
        .day_dir
        .join("health/talent-provenance/activity-inputs/work/work_090000_300.json")
}

fn later(context: &context::ThinkContext, delay: i64) -> context::ThinkContext {
    let mut next = context.clone();
    next.now_ms += delay;
    let now = next.now_ms;
    next.with_event_clock(Arc::new(move || now))
}

#[test]
fn failed_activity_is_reported_and_retried_without_replaying_segments() {
    let (_journal, _roots, context, recorder) = fixture();
    fail_first(&context, &recorder);
    assert!(replay(&context, &["090000_300", "090500_300"], true).is_err());
    assert_eq!(recorder.requests.lock().unwrap().len(), 1);
    assert!(
        due_activity_retries(&context.journal, context.now_ms)
            .unwrap()
            .is_empty()
    );
    assert!(!provenance(&context).exists());
    recorder.end_states.lock().unwrap().clear();
    let next = later(&context, 60_001);
    let due = due_activity_retries(&next.journal, next.now_ms).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(
        (&due[0].day, &due[0].facet, &due[0].activity),
        (
            &context.day,
            &"work".to_owned(),
            &"work_090000_300".to_owned()
        )
    );
    let mut log = test_log(&next, "activity-retry");
    let result = activity::run(
        &next,
        &mut log,
        &due[0].activity,
        &due[0].facet,
        false,
        false,
        1,
    )
    .unwrap();
    assert_eq!((result.success, result.failed), (1, 0));
    assert!(provenance(&next).exists());
    assert!(
        due_activity_retries(&next.journal, i64::MAX)
            .unwrap()
            .is_empty()
    );
    let states = solstone_core_system_health::read_terminal_states(
        &solstone_core_system_health::FilesystemHealthLogSource::new(&next.journal),
        &next.day,
        true,
    )
    .unwrap();
    let participation = states
        .value
        .iter()
        .find(|(unit, _)| unit.name == "participation")
        .unwrap()
        .1;
    assert_eq!(
        participation.latest_event,
        solstone_core_system_health::TerminalEvent::Complete
    );
    assert_eq!(participation.trailing_fail_count, 0);
    replay(&next, &["090000_300", "090500_300"], false).unwrap();
    assert_eq!(recorder.requests.lock().unwrap().len(), 2);
}

#[test]
fn partial_success_survives_reload_and_does_not_repeat_a_sibling() {
    let (_journal, roots, context, recorder) = fixture();
    let config = "{\n\"type\":\"generate\",\"schedule\":\"activity\",\"priority\":1,\"output\":\"json\",\"activities\":[\"work\"]\n}";
    let (talent_root, apps_root) = talent_roots(
        roots.path(),
        &[("participation", config), ("sibling", config)],
    );
    let context = context.with_talent_roots(talent_root, apps_root);
    fail_first(&context, &recorder);
    assert!(replay(&context, &["090000_300", "090500_300"], true).is_err());
    assert_eq!(recorder.requests.lock().unwrap().len(), 2);
    recorder.end_states.lock().unwrap().clear();
    let next = later(&context, 120_001);
    let mut log = test_log(&next, "retry");
    let result =
        activity::run(&next, &mut log, "work_090000_300", "work", false, false, 1).unwrap();
    assert_eq!((result.success, result.failed), (1, 0));
    let names = recorder
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.name.clone())
        .collect::<Vec<_>>();
    assert_eq!(names, ["participation", "sibling", "participation"]);
}

#[test]
fn interrupted_wait_reattaches_the_running_use_instead_of_dispatching_again() {
    let (_journal, _roots, context, recorder) = fixture();
    *recorder.wait_error.lock().unwrap() = Some("connection lost".to_owned());
    assert!(replay(&context, &["090000_300", "090500_300"], true).is_err());
    let running = context
        .journal
        .join("talents/participation/use-1_active.jsonl");
    fs::create_dir_all(running.parent().unwrap()).unwrap();
    fs::write(&running, "{\"event\":\"start\",\"use_id\":\"use-1\"}\n").unwrap();
    assert_eq!(
        solstone_core_cortex_client::use_file_status(&context.journal.join("talents"), "use-1")
            .unwrap(),
        solstone_core_cortex_client::UseFileStatus::Running
    );
    *recorder.wait_error.lock().unwrap() = None;
    let next = later(&context, 60_001);
    let mut log = test_log(&next, "retry");
    let result =
        activity::run(&next, &mut log, "work_090000_300", "work", false, false, 1).unwrap();
    assert_eq!(result.failed, 0);
    assert_eq!(recorder.requests.lock().unwrap().len(), 1);
    assert_eq!(
        recorder.waits.lock().unwrap().as_slice(),
        &[vec!["use-1".to_owned()], vec!["use-1".to_owned()]]
    );
}

#[test]
fn changed_inputs_start_new_work_and_claim_blocks_a_concurrent_retry() {
    let (_journal, _roots, context, recorder) = fixture();
    replay(&context, &["090000_300", "090500_300"], true).unwrap();
    let sense =
        segment_dir(&context.journal, &context.day, "090000_300").join("talents/sense.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&sense).unwrap()).unwrap();
    value["activity_summary"] = serde_json::json!("changed input");
    fs::write(sense, serde_json::to_vec(&value).unwrap()).unwrap();
    replay(&context, &["090000_300", "090500_300"], false).unwrap();
    assert_eq!(recorder.requests.lock().unwrap().len(), 2);
    let record = solstone_core_facets::get_activity_record(
        &context.journal,
        "work",
        &context.day,
        "work_090000_300",
    )
    .unwrap()
    .unwrap();
    let hash = segment::compute_activity_input_hash(&context, &context.day, &record).unwrap();
    let _claim = crate::activity_work::ActivityWork::begin(
        &context,
        "work",
        "work_090000_300",
        hash,
        BTreeSet::from(["participation".to_owned()]),
        true,
    )
    .unwrap();
    let mut log = test_log(&context, "concurrent");
    assert!(
        activity::run(
            &context,
            &mut log,
            "work_090000_300",
            "work",
            true,
            false,
            1
        )
        .is_err()
    );
    assert_eq!(recorder.requests.lock().unwrap().len(), 2);
}

#[test]
fn earlier_connection_failures_are_adopted_by_source_day_without_resetting_backoff() {
    let (_journal, _roots, context, recorder) = fixture();
    write_activity_record(
        &context.journal,
        "work",
        &context.day,
        serde_json::json!({"id":"old_activity","activity":"work","segments":["090000_300"]}),
    );
    write_health_event(
        &context.journal,
        &context.day,
        r#"{"event":"talent.fail","ts":1786615200000,"day":"20260813","mode":"activity","facet":"work","activity":"old_activity","name":"participation","use_id":"old-use","reason_code":"local_endpoint_unreachable"}"#,
    );
    seed_activity_retries(&context.journal, &context.day, context.now_ms).unwrap();
    let due = due_activity_retries(&context.journal, context.now_ms).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].activity, "old_activity");
    fail_first(&context, &recorder);
    let mut log = test_log(&context, "retry");
    assert_eq!(
        activity::run(&context, &mut log, "old_activity", "work", false, false, 1)
            .unwrap()
            .failed,
        1
    );
    seed_activity_retries(&context.journal, &context.day, context.now_ms).unwrap();
    assert!(
        due_activity_retries(&context.journal, context.now_ms)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        due_activity_retries(&context.journal, context.now_ms + 60_001)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn durable_finish_after_lost_wait_is_folded_without_another_model_call() {
    let (_journal, _roots, context, recorder) = fixture();
    *recorder.wait_error.lock().unwrap() = Some("connection lost".to_owned());
    assert!(replay(&context, &["090000_300", "090500_300"], true).is_err());
    let path = context.journal.join("talents/participation/use-1.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        "{\"use_id\":\"use-1\"}\n{\"event\":\"finish\",\"terminal\":true}\n",
    )
    .unwrap();
    let next = later(&context, 86_400_001);
    assert_eq!(
        due_activity_retries(&context.journal, next.now_ms).unwrap()[0].day,
        context.day
    );
    let mut log = test_log(&next, "retry");
    let result =
        activity::run(&next, &mut log, "work_090000_300", "work", false, false, 1).unwrap();
    assert_eq!((result.success, result.failed), (1, 0));
    assert_eq!(recorder.requests.lock().unwrap().len(), 1);
    assert_eq!(recorder.waits.lock().unwrap().len(), 1); // Durable finish needs no new wait.
    assert!(
        due_activity_retries(&context.journal, i64::MAX)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn explicit_nonretryable_failure_parks_but_connection_failure_never_exhausts() {
    let (_journal, _roots, context, recorder) = fixture();
    fail_first(&context, &recorder);
    let path = context.journal.join("talents/participation/use-1.jsonl");
    fs::write(&path, "{\"use_id\":\"use-1\"}\n{\"event\":\"error\",\"terminal\":true,\"reason_code\":\"invalid_config\",\"retryable\":false}\n").unwrap();
    assert!(replay(&context, &["090000_300", "090500_300"], true).is_err());
    assert!(
        due_activity_retries(&context.journal, i64::MAX)
            .unwrap()
            .is_empty()
    );
    let next = later(&context, 86_400_001);
    recorder.end_states.lock().unwrap().clear();
    let mut log = test_log(&next, "explicit-repair");
    assert_eq!(
        activity::run(&next, &mut log, "work_090000_300", "work", true, false, 1)
            .unwrap()
            .failed,
        0
    );
    assert_eq!(recorder.requests.lock().unwrap().len(), 2);
}

#[test]
fn repeated_connection_interruptions_remain_due_after_many_attempts() {
    let (_journal, _roots, context, recorder) = fixture();
    fail_first(&context, &recorder);
    assert!(replay(&context, &["090000_300", "090500_300"], true).is_err());
    for attempt in 2..=12 {
        let next = later(&context, (attempt - 1) * 3_600_001);
        assert_eq!(
            due_activity_retries(&next.journal, next.now_ms)
                .unwrap()
                .len(),
            1
        );
        let id = format!("use-{attempt}");
        recorder
            .end_states
            .lock()
            .unwrap()
            .insert(id.clone(), solstone_core_cortex_client::UseEndState::Error);
        fs::write(context.journal.join("talents/participation").join(format!("{id}.jsonl")),
            format!("{{\"use_id\":\"{id}\"}}\n{{\"event\":\"error\",\"terminal\":true,\"reason_code\":\"local_endpoint_unreachable\",\"retryable\":true}}\n")).unwrap();
        let mut log = test_log(&next, "retry");
        assert_eq!(
            activity::run(&next, &mut log, "work_090000_300", "work", false, false, 1)
                .unwrap()
                .failed,
            1
        );
        assert!(
            due_activity_retries(&next.journal, next.now_ms)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            due_activity_retries(&next.journal, next.now_ms + 3_600_000)
                .unwrap()
                .len(),
            1
        );
    }
    assert!(!provenance(&context).exists());
}

#[test]
fn replaced_destination_blocks_activity_work_and_reactivate_refuses_replacement() {
    let (_journal, _roots, context, recorder) = fixture();
    let original_decl = fs::read_to_string(context.journal.join("facets/work/facet.json")).unwrap();
    fail_first(&context, &recorder);
    assert!(replay(&context, &["090000_300", "090500_300"], true).is_err());

    // Replace the facet declaration's identity on disk
    let facet_file = context.journal.join("facets/work/facet.json");
    fs::write(
        &facet_file,
        r#"{"id":"00000000-0000-4000-8000-000000000002","title":"Work Replacement"}"#,
    )
    .unwrap();

    let next = later(&context, 60_001);
    let mut log = test_log(&next, "retry");
    let result =
        activity::run(&next, &mut log, "work_090000_300", "work", false, false, 1).unwrap();
    // Failed due to replaced destination identity blocking the work
    assert_eq!(result.failed, 1);
    assert_eq!(
        result.failed_names,
        vec![
            "the destination facet was replaced. reclassify the source segment; an old result cannot be applied to the replacement."
        ]
    );

    // Blocked work is not due for retries
    assert!(
        due_activity_retries(&next.journal, i64::MAX)
            .unwrap()
            .is_empty()
    );

    // Reactivating while replaced STILL refuses with the exact error and stays blocked
    let mut reactivate_log = test_log(&next, "reactivate");
    let reactivate_result = activity::run(
        &next,
        &mut reactivate_log,
        "work_090000_300",
        "work",
        false,
        true,
        1,
    )
    .unwrap();
    assert_eq!(reactivate_result.failed, 1);
    assert_eq!(
        reactivate_result.failed_names,
        vec![
            "the destination facet was replaced. reclassify the source segment; an old result cannot be applied to the replacement."
        ]
    );

    // Restore original facet identity on disk
    fs::write(&facet_file, original_decl).unwrap();

    // Now reactivating succeeds!
    recorder.end_states.lock().unwrap().clear();
    let mut restore_log = test_log(&next, "restore");
    let restore_result = activity::run(
        &next,
        &mut restore_log,
        "work_090000_300",
        "work",
        false,
        true,
        1,
    )
    .unwrap();
    assert_eq!((restore_result.success, restore_result.failed), (1, 0));
    assert!(provenance(&next).exists());
}

#[test]
fn muted_destination_pauses_activity_work_and_unmuting_resumes_without_replaying_segment() {
    let (_journal, _roots, context, recorder) = fixture();
    fail_first(&context, &recorder);
    assert!(replay(&context, &["090000_300", "090500_300"], true).is_err());

    // Mute destination facet
    solstone_core_facets::set_facet_muted(&context.journal, "work", true).unwrap();

    let next = later(&context, 60_001);
    let mut log = test_log(&next, "retry");
    let result =
        activity::run(&next, &mut log, "work_090000_300", "work", false, false, 1).unwrap();
    assert_eq!((result.success, result.failed), (0, 1));

    // Muted work is paused and not due
    assert!(
        due_activity_retries(&next.journal, i64::MAX)
            .unwrap()
            .is_empty()
    );

    // Unmute destination facet
    solstone_core_facets::set_facet_muted(&context.journal, "work", false).unwrap();

    // Now due again without replaying segment
    let due = due_activity_retries(&next.journal, next.now_ms).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].activity, "work_090000_300");

    recorder.end_states.lock().unwrap().clear();
    let mut resume_log = test_log(&next, "resume");
    let resume_result = activity::run(
        &next,
        &mut resume_log,
        "work_090000_300",
        "work",
        false,
        false,
        1,
    )
    .unwrap();
    assert_eq!((resume_result.success, resume_result.failed), (1, 0));
}

#[test]
fn activity_run_without_record_returns_exact_correction_error() {
    let (_journal, _roots, context, _recorder) = fixture();
    let mut log = test_log(&context, "test");
    let result = activity::run(
        &context,
        &mut log,
        "nonexistent_activity",
        "work",
        false,
        false,
        1,
    )
    .unwrap();
    assert_eq!(result.failed, 1);
    assert_eq!(
        result.failed_names,
        vec![
            "no activity record exists for this classification. after correcting the declaration, reprocess the source segment."
        ]
    );
}

#[test]
fn raw_output_refuses_replaced_or_muted_destination_without_writing() {
    let journal = tempdir().unwrap();
    solstone_core_facets::create_facet(journal.path(), "work", "Work", "", "", "", None).unwrap();
    let original_id =
        solstone_core_facets::observe_facet_write_identity(journal.path(), "work").unwrap();

    let output_path = journal
        .path()
        .join("facets/work/activities/20260813/act_1/participation.json");
    let mut config = Map::new();
    config.insert(
        "output_path".to_owned(),
        Value::String(output_path.display().to_string()),
    );
    config.insert("facet".to_owned(), Value::String("work".to_owned()));
    config.insert("destination_id".to_owned(), Value::String(original_id));

    let prepared = solstone_core_talent_runtime::PreparedTalent {
        name: "participation".to_owned(),
        config: config.clone(),
    };

    // Valid output writes
    let wrote = solstone_core_talent_runtime::writers::write_output_if_configured(
        &prepared,
        &solstone_core_talent_runtime::ExecutionContext {
            journal: journal.path().to_path_buf(),
        },
        "{\"result\":1}",
    )
    .unwrap();
    assert!(wrote);
    assert!(output_path.exists());

    // Replace facet id
    let facet_file = journal.path().join("facets/work/facet.json");
    fs::write(
        &facet_file,
        r#"{"id":"00000000-0000-4000-8000-000000000099","title":"Replaced"}"#,
    )
    .unwrap();

    let output_path2 = journal
        .path()
        .join("facets/work/activities/20260813/act_2/participation.json");
    let mut config2 = config.clone();
    config2.insert(
        "output_path".to_owned(),
        Value::String(output_path2.display().to_string()),
    );
    let prepared2 = solstone_core_talent_runtime::PreparedTalent {
        name: "participation".to_owned(),
        config: config2,
    };

    let err = solstone_core_talent_runtime::writers::write_output_if_configured(
        &prepared2,
        &solstone_core_talent_runtime::ExecutionContext {
            journal: journal.path().to_path_buf(),
        },
        "{\"result\":2}",
    )
    .unwrap_err();
    assert!(err.contains("replaced"));
    assert!(!output_path2.exists());
}

#[test]
fn reactivate_with_missing_activity_row_returns_projection_only_even_if_facet_missing() {
    let (_journal, _roots, context, _recorder) = fixture();
    let mut log = test_log(&context, "test");
    let result = activity::run(
        &context,
        &mut log,
        "nonexistent_activity",
        "missing_facet",
        false,
        true,
        1,
    )
    .unwrap();
    assert_eq!(result.failed, 1);
    assert_eq!(
        result.failed_names,
        vec![
            "no activity record exists for this classification. after correcting the declaration, reprocess the source segment."
        ]
    );
}

#[test]
fn mixed_persist_ended_activities_batch_with_undeclared_and_muted_siblings() {
    let journal = tempdir().unwrap();
    let roots = tempdir().unwrap();
    let (talent_root, apps_root) = talent_roots(
        roots.path(),
        &[(
            "participation",
            "{\n\"type\":\"generate\",\"schedule\":\"activity\",\"priority\":1,\"output\":\"json\",\"activities\":[\"work\",\"personal\"]\n}",
        )],
    );
    let (context, _recorder) = recorder_context(journal.path(), "20260813", 1_786_615_200_000);
    solstone_core_facets::create_facet(journal.path(), "work", "Work", "", "", "", None).unwrap();
    solstone_core_facets::create_facet(journal.path(), "personal", "Personal", "", "", "", None)
        .unwrap();
    solstone_core_facets::set_facet_muted(journal.path(), "personal", true).unwrap();

    let context = context.with_talent_roots(talent_root, apps_root);

    // Segment 1: sense has undeclared "Work", declared "work", muted "personal", and undeclared "extra"
    let sense_active = serde_json::json!({
        "density": "active",
        "content_type": "work",
        "activity_summary": "mixed test",
        "facets": [
            {"facet": "Work", "level": "high", "activity": "work"},
            {"facet": "work", "level": "high", "activity": "work"},
            {"facet": "personal", "level": "high", "activity": "personal"},
            {"facet": "extra", "level": "high", "activity": "extra"}
        ]
    });
    let sense_idle = serde_json::json!({
        "density": "idle",
        "content_type": "idle",
        "facets": []
    });

    for (key, sense) in [("090000_300", &sense_active), ("090500_300", &sense_idle)] {
        let path = segment_dir(journal.path(), "20260813", key).join("talents");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("sense.json"), serde_json::to_vec(sense).unwrap()).unwrap();
    }

    // Persist batch succeeds without failing solely because muted personal paused
    let outcome = replay(&context, &["090000_300", "090500_300"], true);
    assert!(outcome.is_ok(), "replay failed: {:?}", outcome);

    // Valid declared sibling "work" gets an activity record
    let work_activities = journal.path().join("facets/work/activities/20260813.jsonl");
    assert!(work_activities.exists());
    let work_content = fs::read_to_string(&work_activities).unwrap();
    assert!(work_content.contains("\"facet\":\"work\""));

    // Muted declared sibling "personal" gets an activity record
    let personal_activities = journal
        .path()
        .join("facets/personal/activities/20260813.jsonl");
    assert!(personal_activities.exists());
    let personal_content = fs::read_to_string(&personal_activities).unwrap();
    assert!(personal_content.contains("\"facet\":\"personal\""));

    // Undeclared / case-variant siblings do NOT create facet directories
    assert!(!journal.path().join("facets/Work").exists());
    assert!(!journal.path().join("facets/extra").exists());
}

#[test]
fn write_sense_and_change_keeps_undeclared_in_sense_and_filters_facets() {
    let (journal, _roots, context, _recorder) = fixture();
    solstone_core_facets::create_facet(journal.path(), "personal", "Personal", "", "", "", None)
        .unwrap();
    solstone_core_facets::set_facet_muted(journal.path(), "personal", true).unwrap();

    let segment_path = segment_dir(journal.path(), "20260813", "100000_300");
    let sense_obj = serde_json::json!({
        "density": "active",
        "content_type": "mixed",
        "activity_summary": "summary test",
        "facets": [
            {"facet": "Work", "level": "high"},
            {"facet": "work", "level": "high"},
            {"facet": "personal", "level": "medium"},
            {"facet": "other_undeclared", "level": "low"}
        ]
    });
    let sense_map = sense_obj.as_object().unwrap();

    let mut log = test_log(&context, "projection");
    crate::segment::write_sense_and_change(
        &context,
        &mut log,
        "100000_300",
        Some("default"),
        &segment_path,
        sense_map,
    )
    .unwrap();

    let talents = segment_path.join("talents");

    // talents/sense.json retains all original entries (including undeclared and case variant)
    let written_sense: serde_json::Value =
        serde_json::from_slice(&fs::read(talents.join("sense.json")).unwrap()).unwrap();
    let sense_facets = written_sense["facets"].as_array().unwrap();
    let sense_slugs: Vec<&str> = sense_facets
        .iter()
        .filter_map(|f| f["facet"].as_str())
        .collect();
    assert_eq!(
        sense_slugs,
        vec!["Work", "work", "personal", "other_undeclared"]
    );

    // talents/facets.json contains only exact declared slugs (work and muted personal)
    let written_facets: serde_json::Value =
        serde_json::from_slice(&fs::read(talents.join("facets.json")).unwrap()).unwrap();
    let facets_arr = written_facets.as_array().unwrap();
    let declared_slugs: Vec<&str> = facets_arr
        .iter()
        .filter_map(|f| f["facet"].as_str())
        .collect();
    assert_eq!(declared_slugs, vec!["work", "personal"]);
}

#[test]
fn destination_failures_park_first_admission_and_preserve_source() {
    for malformed in [false, true] {
        let (_journal, _roots, context, recorder) = fixture();
        let row = serde_json::json!({"id":"orphan","activity":"work","facet":"work","segments":["090000_300"]});
        let path = context
            .journal
            .join("facets/work/activities/20260813.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, format!("{row}\n")).unwrap();
        let declaration = context.journal.join("facets/work/facet.json");
        if malformed {
            fs::write(&declaration, b"not json").unwrap();
        } else {
            fs::remove_file(&declaration).unwrap();
        }
        let before = fs::read(&path).unwrap();
        let mut log = test_log(&context, "bad-destination");
        assert_eq!(
            activity::run(&context, &mut log, "orphan", "work", false, false, 1)
                .unwrap()
                .failed,
            1
        );
        assert!(
            crate::activity_work::has_persisted_disposition(
                &context.journal,
                &context.day,
                "work",
                "orphan"
            )
            .unwrap()
        );
        assert!(
            due_activity_retries(&context.journal, i64::MAX)
                .unwrap()
                .is_empty()
        );
        assert!(recorder.requests.lock().unwrap().is_empty());
        assert_eq!(fs::read(&path).unwrap(), before);
        if malformed {
            assert_eq!(fs::read(declaration).unwrap(), b"not json");
        }
    }
}

#[test]
fn paused_work_reclassifies_missing_destination_then_folds_late_finish() {
    let (_journal, _roots, context, recorder) = fixture();
    fail_first(&context, &recorder);
    assert!(replay(&context, &["090000_300", "090500_300"], true).is_err());
    solstone_core_facets::set_facet_muted(&context.journal, "work", true).unwrap();
    let next = later(&context, 60_001);
    let mut log = test_log(&next, "mute");
    activity::run(&next, &mut log, "work_090000_300", "work", false, false, 1).unwrap();
    solstone_core_facets::delete_facet(&context.journal, "work").unwrap();
    assert_eq!(
        due_activity_retries(&context.journal, next.now_ms)
            .unwrap()
            .len(),
        1
    );
    let mut log = test_log(&next, "deleted");
    assert_eq!(
        activity::run(&next, &mut log, "work_090000_300", "work", false, false, 1)
            .unwrap()
            .failed,
        1
    );
    assert!(
        due_activity_retries(&context.journal, i64::MAX)
            .unwrap()
            .is_empty()
    );
    fs::write(
        context.journal.join("talents/participation/use-1.jsonl"),
        "{\"use_id\":\"use-1\"}\n{\"event\":\"finish\",\"terminal\":true}\n",
    )
    .unwrap();
    assert_eq!(
        due_activity_retries(&context.journal, next.now_ms)
            .unwrap()
            .len(),
        1
    );
    let mut log = test_log(&next, "late-finish");
    assert_eq!(
        activity::run(&next, &mut log, "work_090000_300", "work", false, false, 1)
            .unwrap()
            .success,
        1
    );
    assert_eq!(recorder.requests.lock().unwrap().len(), 1);
    assert!(!context.journal.join("facets/work").exists());
}

#[test]
fn corrupted_work_is_not_silently_replaced_and_legacy_declaration_adopts_once() {
    let (_journal, _roots, context, recorder) = fixture();
    let decl = context.journal.join("facets/work/facet.json");
    fs::write(&decl, r#"{"title":"Historical","extra":"preserve"}"#).unwrap();
    let row =
        serde_json::json!({"id":"old","activity":"work","facet":"work","segments":["090000_300"]});
    let path = context
        .journal
        .join("facets/work/activities/20260813.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, format!("{row}\n")).unwrap();
    fail_first(&context, &recorder);
    let mut log = test_log(&context, "historical");
    activity::run(&context, &mut log, "old", "work", false, false, 1).unwrap();
    let declaration: Value = serde_json::from_slice(&fs::read(&decl).unwrap()).unwrap();
    assert_eq!(declaration["extra"], "preserve");
    let bound = recorder.requests.lock().unwrap()[0].config["destination_id"].clone();
    assert_eq!(declaration["id"], bound);
    let work = fs::read_dir(context.journal.join("health/activity-work"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|x| x == "json"))
        .unwrap();
    fs::write(&work, b"corrupted").unwrap();
    let next = later(&context, 60_001);
    let mut log = test_log(&next, "corrupt");
    assert!(
        activity::run(&next, &mut log, "old", "work", false, false, 1)
            .unwrap_err()
            .contains("malformed activity work")
    );
    assert_eq!(fs::read(work).unwrap(), b"corrupted");
    assert_eq!(recorder.requests.lock().unwrap().len(), 1);
}

#[test]
fn unreadable_projection_retries_retained_source_and_keeps_valid_siblings() {
    use solstone_core_system_health::{
        FilesystemHealthLogSource, ThoughtVerdict, lookup_segment_progress, read_segment_progress,
        segment_fully_thought,
    };
    let (_journal, _roots, context, recorder) = fixture();
    solstone_core_facets::create_facet(&context.journal, "second", "Second", "", "", "", None)
        .unwrap();
    let declaration = context.journal.join("facets/second/facet.json");
    let saved = fs::read(&declaration).unwrap();
    fs::remove_file(&declaration).unwrap();
    fs::create_dir(&declaration).unwrap();
    // Retained context outside the selected repair must never acquire new activity work.
    for (key, value) in [
        (
            "120000_300",
            serde_json::json!({"density":"active","content_type":"work","facets":[{"facet":"work","level":"high","activity":"work"}]}),
        ),
        (
            "120500_300",
            serde_json::json!({"density":"idle","content_type":"idle","facets":[]}),
        ),
    ] {
        let path = segment_dir(&context.journal, &context.day, key).join("talents");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("sense.json"), serde_json::to_vec(&value).unwrap()).unwrap();
    }
    let source =
        segment_dir(&context.journal, &context.day, "090000_300").join("talents/sense.json");
    let bytes = serde_json::to_vec(
        &serde_json::json!({"density":"active","content_type":"work","facets":[
            {"facet":"second","level":"high","activity":"work"},
            {"facet":"work","level":"high","activity":"work"}
        ]}),
    )
    .unwrap();
    fs::write(&source, &bytes).unwrap();
    assert!(
        replay(&context, &["090000_300", "090500_300"], false)
            .unwrap_err()
            .contains("routing remains pending")
    );
    assert!(
        context
            .journal
            .join("facets/work/activities/20260813.jsonl")
            .exists()
    );
    assert!(
        !context
            .journal
            .join("facets/second/activities/20260813.jsonl")
            .exists()
    );
    assert_eq!(fs::read(&source).unwrap(), bytes);
    let generation =
        solstone_core_journal_io::bump_stream_marker(&context.journal, &context.day).unwrap();
    let fingerprint =
        solstone_core_system::catchup::read_raw_input_fingerprint(&context.journal, &context.day)
            .unwrap();
    solstone_core_journal_io::publish_daily_marker_if_current(
        &context.journal,
        &context.day,
        generation,
        &fingerprint,
        || Ok(fingerprint.clone()),
    )
    .unwrap();
    let coverage = solstone_core_system::daily_coverage::DailyCoverage {
        maintenance: None,
        day: context.day.clone(),
        as_of_ms: context.now_ms,
        state: solstone_core_system::daily_coverage::CoverageState::Current,
        units: vec![],
    };
    assert!(
        !solstone_core_system_health::day_is_complete_with(
            &context.journal,
            &context.day,
            Ok(&coverage)
        )
        .unwrap()
    );
    assert!(
        solstone_core_system::catchup::days_with_expired_retry(
            &context.journal,
            &std::collections::BTreeSet::new(),
            std::time::UNIX_EPOCH + std::time::Duration::from_millis(context.now_ms as u64)
        )
        .unwrap()
        .is_empty()
    );
    assert_eq!(
        solstone_core_system::catchup::days_with_expired_retry(
            &context.journal,
            &std::collections::BTreeSet::new(),
            std::time::UNIX_EPOCH
                + std::time::Duration::from_millis(context.now_ms as u64 + 600_001)
        )
        .unwrap(),
        vec![context.day.clone()]
    );
    let progress = read_segment_progress(
        &FilesystemHealthLogSource::new(&context.journal),
        &context.day,
    )
    .unwrap();
    let mut state = lookup_segment_progress(&progress.value, "default", "090000_300")
        .unwrap()
        .clone();
    state.sensed = true;
    state.density = Some("idle".to_owned());
    assert_eq!(
        segment_fully_thought(Some(&state)),
        ThoughtVerdict::Dispatched("facet_routing".to_owned())
    );
    let before = recorder.requests.lock().unwrap().len();
    let mut other_log = test_log(&context, "unselected-pending");
    other_log.log(
        "facet.routing_pending",
        context.now_ms + 1,
        serde_json::json!({"day":context.day,"segment":"120000_300","stream":"default"})
            .as_object()
            .unwrap()
            .clone(),
    );
    other_log.finish().unwrap();
    fs::remove_dir(&declaration).unwrap();
    fs::write(&declaration, saved).unwrap();
    fs::write(context.talent_root.join("sense.md"), "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":1,\"output\":\"json\"\n}\nfixture").unwrap();
    let pending = solstone_core_system_health::read_pending_facet_routing(
        &FilesystemHealthLogSource::new(&context.journal),
        &context.day,
    )
    .unwrap();
    let expected =
        solstone_core_system::catchup::read_raw_input_fingerprint(&context.journal, &context.day)
            .unwrap();
    assert_eq!(
        pending.value.values().next().unwrap().as_deref(),
        Some(expected.as_str()),
        "{pending:?}"
    );
    let next = later(&context, 600_001);
    let mut log = test_log(&next, "routing-retry");
    let result = segment::run_repair_batch_with_activity(
        &next,
        &mut log,
        vec![("090000_300".to_owned(), Some("default".to_owned()))],
        false,
        1,
        1,
        None,
        vec![],
        false,
    )
    .unwrap();
    assert_eq!(result.failed, 0, "{result:?}");
    assert!(
        context
            .journal
            .join("facets/second/activities/20260813.jsonl")
            .exists(),
        "source after repair: {}; pending: {pending:?}",
        String::from_utf8_lossy(&fs::read(&source).unwrap())
    );
    let requests = recorder.requests.lock().unwrap();
    assert_eq!(requests.len(), before + 1); // Only the newly eligible activity, never Sense or the completed sibling.
    assert!(requests.iter().all(|request| request.name != "sense"));
    let still_pending = solstone_core_system_health::read_pending_facet_routing(
        &FilesystemHealthLogSource::new(&context.journal),
        &context.day,
    )
    .unwrap();
    assert_eq!(still_pending.value.len(), 1);
    assert_eq!(
        still_pending.value.keys().next().unwrap().segment,
        "120000_300"
    );
    let progress = read_segment_progress(
        &FilesystemHealthLogSource::new(&context.journal),
        &context.day,
    )
    .unwrap();
    assert!(
        lookup_segment_progress(&progress.value, "default", "090000_300")
            .unwrap()
            .completed
            .contains("facet_routing")
    );
    assert_eq!(fs::read(&source).unwrap(), bytes);
}

#[test]
fn streamless_replay_uses_the_resolved_routing_coordinate_and_changed_source_regenerates() {
    let (_journal, _roots, context, recorder) = fixture();
    let declaration = context.journal.join("facets/work/facet.json");
    let saved = fs::read(&declaration).unwrap();
    fs::remove_file(&declaration).unwrap();
    fs::create_dir(&declaration).unwrap();
    let mut log = test_log(&context, "without-stream");
    assert!(
        segment::replay_activity_state(
            &context,
            &mut log,
            &[("090000_300".to_owned(), None)],
            false,
            1,
            false,
            true
        )
        .is_err()
    );
    let pending = solstone_core_system_health::read_pending_facet_routing(
        &solstone_core_system_health::FilesystemHealthLogSource::new(&context.journal),
        &context.day,
    )
    .unwrap();
    assert_eq!(pending.value.len(), 1);
    assert_eq!(
        pending.value.keys().next().unwrap().stream.as_deref(),
        Some("default")
    );
    fs::remove_dir(&declaration).unwrap();
    fs::write(&declaration, saved).unwrap();
    fs::write(
        segment_dir(&context.journal, &context.day, "090000_300").join("chat.jsonl"),
        b"{\"text\":\"changed source\"}\n",
    )
    .unwrap();
    fs::write(context.talent_root.join("sense.md"), "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":1,\"output\":\"json\"\n}\nfixture").unwrap();
    let next = later(&context, 600_001);
    let mut log = test_log(&next, "changed-source");
    segment::run(
        &next,
        &mut log,
        "090000_300",
        false,
        None,
        1,
        None,
        false,
        &[],
    )
    .unwrap();
    assert!(
        recorder
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.name == "sense")
    );
}

#[test]
fn unreadable_activity_declaration_retries_with_backoff_and_same_identity() {
    let (_journal, _roots, context, recorder) = fixture();
    fail_first(&context, &recorder);
    assert!(replay(&context, &["090000_300", "090500_300"], true).is_err());
    let declaration = context.journal.join("facets/work/facet.json");
    let saved = fs::read(&declaration).unwrap();
    fs::remove_file(&declaration).unwrap();
    fs::create_dir(&declaration).unwrap();
    let next = later(&context, 60_001);
    let mut log = test_log(&next, "unreadable");
    assert_eq!(
        activity::run(&next, &mut log, "work_090000_300", "work", false, false, 1)
            .unwrap()
            .failed,
        1
    );
    assert!(
        due_activity_retries(&context.journal, next.now_ms)
            .unwrap()
            .is_empty()
    );
    assert_eq!(recorder.requests.lock().unwrap().len(), 1);
    fs::remove_dir(&declaration).unwrap();
    fs::write(&declaration, saved).unwrap();
    recorder.end_states.lock().unwrap().clear();
    let next = later(&next, 120_001);
    assert_eq!(
        due_activity_retries(&context.journal, next.now_ms)
            .unwrap()
            .len(),
        1
    );
    let mut log = test_log(&next, "restored");
    assert_eq!(
        activity::run(&next, &mut log, "work_090000_300", "work", false, false, 1)
            .unwrap()
            .success,
        1
    );
    assert_eq!(recorder.requests.lock().unwrap().len(), 2);
}
