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
    let result = activity::run(&next, &mut log, &due[0].activity, &due[0].facet, false, 1).unwrap();
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
    let result = activity::run(&next, &mut log, "work_090000_300", "work", false, 1).unwrap();
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
    let result = activity::run(&next, &mut log, "work_090000_300", "work", false, 1).unwrap();
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
    assert!(activity::run(&context, &mut log, "work_090000_300", "work", true, 1).is_err());
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
        activity::run(&context, &mut log, "old_activity", "work", false, 1)
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
    let result = activity::run(&next, &mut log, "work_090000_300", "work", false, 1).unwrap();
    assert_eq!((result.success, result.failed), (1, 0));
    assert_eq!(recorder.requests.lock().unwrap().len(), 1);
    assert_eq!(recorder.waits.lock().unwrap().len(), 2);
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
        activity::run(&next, &mut log, "work_090000_300", "work", true, 1)
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
            activity::run(&next, &mut log, "work_090000_300", "work", false, 1)
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
