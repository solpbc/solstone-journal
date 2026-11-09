// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A Strava tile is body data. Thinking never reads one again, and what it
//! read of one before ends without a model call.
use super::*;
use std::collections::BTreeMap;

use solstone_core_system_health::{
    FilesystemHealthLogSource, lookup_segment_progress, read_segment_progress,
};

const DAY: &str = "20260813";
const NOW: i64 = 1_786_615_200_000;
const STRAVA: &str = "import.strava";
const TILE: &str = "070000_300";

const SENSE: &str = "{\n\"type\": \"generate\", \"max_output_tokens\": 1024, \"schedule\": \"segment\", \"priority\": 1, \"output\": \"json\"\n}\n";
const PARTICIPATION: &str = "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"schedule\":\"activity\",\"priority\":1,\"output\":\"json\",\"activities\":[\"*\"]\n}";

fn journal(
    talents: &[(&str, &str)],
) -> (
    tempfile::TempDir,
    tempfile::TempDir,
    context::ThinkContext,
    Arc<Recorder>,
) {
    let journal = tempdir().unwrap();
    let roots = tempdir().unwrap();
    let (talent_root, apps_root) = talent_roots(roots.path(), talents);
    let (context, recorder) = recorder_context(journal.path(), DAY, NOW);
    solstone_core_facets::create_facet(journal.path(), "work", "Work", "", "", "", None).unwrap();
    let context = context.with_talent_roots(talent_root, apps_root);
    (journal, roots, context, recorder)
}

fn active(facet: &str, activity: &str) -> Value {
    serde_json::json!({"density":"active","content_type":"work","activity_summary":"A run along the river.",
        "facets":[{"facet":facet,"level":"high","activity":activity}]})
}

fn segment_path(journal: &Path, stream: &str, key: &str) -> std::path::PathBuf {
    journal.join("chronicle").join(DAY).join(stream).join(key)
}

/// A tile as the Strava import writes it, with what Sense once wrote beside
/// it when `sensed` is given.
fn strava_tile(journal: &Path, key: &str, sensed: Option<&Value>) -> std::path::PathBuf {
    let dir = segment_path(journal, STRAVA, key);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("workout.json"),
        serde_json::json!({
            "schema": "solstone.import.strava.tile.v1",
            "activity_id": 9,
            "tile": {"index": 0, "count": 1},
            "workout": {"name": "Riverside tempo", "type": "Run", "distance_m": 10200.0}
        })
        .to_string(),
    )
    .unwrap();
    fs::write(dir.join("stream.json"), "{}").unwrap();
    if let Some(sense) = sensed {
        write_sense(journal, STRAVA, key, sense);
    }
    dir
}

fn write_sense(journal: &Path, stream: &str, key: &str, sense: &Value) {
    let talents = segment_path(journal, stream, key).join("talents");
    fs::create_dir_all(&talents).unwrap();
    fs::write(
        talents.join("sense.json"),
        serde_json::to_vec(sense).unwrap(),
    )
    .unwrap();
    let facets = sense["facets"].clone();
    fs::write(
        talents.join("facets.json"),
        serde_json::to_vec(&facets).unwrap(),
    )
    .unwrap();
}

/// Every file under `dir`, with its bytes.
fn tree(dir: &Path) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        let Ok(entries) = fs::read_dir(&next) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.insert(path.clone(), fs::read(&path).unwrap());
            }
        }
    }
    files
}

fn pending_routing(context: &context::ThinkContext) -> Vec<String> {
    solstone_core_system_health::read_pending_facet_routing(
        &FilesystemHealthLogSource::new(&context.journal),
        &context.day,
    )
    .unwrap()
    .value
    .keys()
    .map(|key| format!("{}/{}", key.stream.as_deref().unwrap_or(""), key.segment))
    .collect()
}

fn hold_routing(context: &context::ThinkContext, stream: &str, key: &str) {
    let fingerprint =
        solstone_core_system::catchup::read_raw_input_fingerprint(&context.journal, &context.day)
            .unwrap();
    write_health_event(
        &context.journal,
        &context.day,
        &serde_json::json!({"event":"facet.routing_pending", "ts":context.now_ms - 60_000,
            "mode":"segment", "day":context.day, "stream":stream, "segment":key,
            "reason_code":"declaration_unreadable", "input_fingerprint":fingerprint})
        .to_string(),
    );
}

/// A Sense run queued before tiles counted as body data, on a tile Sense read
/// and on one it never reached: no model call, nothing under `talents/`, and a
/// routing retry pending for the tile ends so its day can read complete.
#[test]
fn a_queued_sense_run_on_a_strava_tile_writes_nothing_and_ends_its_pending_routing() {
    let (_journal, _roots, context, recorder) = journal(&[("sense", SENSE)]);
    let sensed = strava_tile(&context.journal, TILE, Some(&active("work", "exercise")));
    let unsensed = strava_tile(&context.journal, "080000_300", None);
    hold_routing(&context, STRAVA, TILE);
    assert_eq!(pending_routing(&context), vec![format!("{STRAVA}/{TILE}")]);
    // Its earlier reading covers its input, so only the pending routing kept it
    // from reading as thought, and from its outputs being published.
    write_health_event(
        &context.journal,
        &context.day,
        &serde_json::json!({"event":"sense.complete","mode":"segment","day":context.day,
            "stream":STRAVA,"segment":TILE,"density":"idle",
            "ts":chrono::Utc::now().timestamp_millis() + 3_600_000})
        .to_string(),
    );
    let before = (tree(&sensed), tree(&unsensed));
    for (key, live) in [(TILE, false), (TILE, true), ("080000_300", false)] {
        let mut log = test_log(&context, "segment");
        let result = segment::run(
            &context,
            &mut log,
            key,
            false,
            Some(STRAVA),
            1,
            Some(std::time::Duration::from_secs(610)),
            live,
            &[],
        )
        .unwrap();
        log.finish().unwrap();
        assert_eq!(result.failed, 0, "{result:?}");
    }
    assert!(recorder.requests.lock().unwrap().is_empty());
    assert_eq!((tree(&sensed), tree(&unsensed)), before);
    assert!(!unsensed.join("talents").exists());
    assert!(pending_routing(&context).is_empty());
    let progress = read_segment_progress(
        &FilesystemHealthLogSource::new(&context.journal),
        &context.day,
    )
    .unwrap();
    assert!(
        lookup_segment_progress(&progress.value, STRAVA, TILE)
            .unwrap()
            .completed
            .contains("facet_routing")
    );
}

fn oplog_events(context: &context::ThinkContext) -> Vec<Value> {
    let day = NaiveDate::parse_from_str(&context.day, "%Y%m%d").unwrap();
    catalog_oplogs(JournalRoot::open(&context.journal).unwrap(), &[day])
        .unwrap()
        .into_catalogued_entries()
        .into_iter()
        .flat_map(|(entry, mut file)| {
            file.seek(SeekFrom::Start(entry.payload_offset() as u64))
                .unwrap();
            BufReader::new(file)
                .lines()
                .map_while(Result::ok)
                .filter_map(|line| serde_json::from_str(&line).ok())
                .collect::<Vec<Value>>()
        })
        .collect()
}

fn records(context: &context::ThinkContext) -> Vec<Value> {
    fs::read_to_string(
        context
            .journal
            .join("facets/work/activities")
            .join(format!("{DAY}.jsonl")),
    )
    .unwrap_or_default()
    .lines()
    .map(|line| serde_json::from_str(line).unwrap())
    .collect()
}

fn live(context: &context::ThinkContext, stream: &str, key: &str) {
    let mut log = test_log(context, "live");
    segment::replay_activity_state(
        context,
        &mut log,
        &[(key.to_owned(), Some(stream.to_owned()))],
        false,
        1,
        false,
        true,
    )
    .unwrap();
    log.finish().unwrap();
}

fn parsed(args: &[&str]) -> args::ThinkArgs {
    let args = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    let args::ParseOutcome::Args(parsed) = args::parse(&args).unwrap() else {
        panic!("think args")
    };
    *parsed
}

fn command(context: &context::ThinkContext, args: &[&str]) -> CliRun {
    execute_parsed_command(
        context,
        &parsed(args),
        1,
        &solstone_core_system::process::ChildLaunchContext::default(),
    )
    .unwrap()
}

/// A Strava activity left open from before is closed with no record, by the
/// day's segment run and by a flush: both exit 0, nothing fails to persist and
/// no second record appears. On the same day an open import activity and a
/// capture activity still close, are written and get their talent work.
#[test]
fn a_strava_tile_opens_no_activity_and_its_neighbours_are_written() {
    let (_journal, _roots, context, recorder) = journal(&[("participation", PARTICIPATION)]);
    strava_tile(&context.journal, TILE, Some(&active("work", "exercise")));
    let archive = segment_path(&context.journal, "import.archive", "080000_300");
    fs::create_dir_all(&archive).unwrap();
    fs::write(archive.join("audio.jsonl"), "{}\n").unwrap();
    write_sense(
        &context.journal,
        "import.archive",
        "080000_300",
        &active("work", "listening"),
    );
    let capture = segment_path(&context.journal, "default", "090000_300");
    fs::create_dir_all(&capture).unwrap();
    fs::write(capture.join("audio.jsonl"), "{}\n").unwrap();
    write_sense(
        &context.journal,
        "default",
        "090000_300",
        &active("work", "meeting"),
    );
    // A record the import already holds for the workout.
    let held = "{\"id\":\"body_0123456789abcdef\",\"source\":\"body\",\"activity\":\"workout\",\"account\":{\"key\":\"import.strava\\u001f9\"}}\n";
    let file = context
        .journal
        .join("facets/work/activities")
        .join(format!("{DAY}.jsonl"));
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, held).unwrap();
    for (stream, key) in [
        (STRAVA, TILE),
        ("import.archive", "080000_300"),
        ("default", "090000_300"),
    ] {
        live(&context, stream, key);
    }
    let state =
        solstone_core_system::activity_state::activity_state_path(&context.journal, Some(STRAVA));
    assert!(!state.exists(), "no activity state follows Strava's tiles");
    let later = context
        .clone()
        .with_event_clock(Arc::new(|| NOW + 2 * 86_400_000));
    let segments = command(&later, &["--segments", "--day", DAY]);
    assert_eq!(segments.exit_code, 0, "{}", segments.stderr);
    for (stream, key) in [
        (STRAVA, TILE),
        ("import.archive", "080000_300"),
        ("default", "090000_300"),
    ] {
        let flushed = command(
            &later,
            &[
                "--flush",
                "--segment",
                key,
                "--stream",
                stream,
                "--day",
                DAY,
            ],
        );
        assert_eq!(flushed.exit_code, 0, "{stream}: {}", flushed.stderr);
    }
    let events = oplog_events(&context);
    assert!(
        !events
            .iter()
            .any(|event| event["event"] == "activity.persist_failed"),
        "{events:?}"
    );
    let rows = records(&context);
    assert!(
        !rows.iter().any(|row| row["stream"] == STRAVA),
        "no second record for the workout: {rows:?}"
    );
    assert!(fs::read_to_string(&file).unwrap().starts_with(held));
    for stream in ["import.archive", "default"] {
        let written = rows
            .iter()
            .filter(|row| {
                row["stream"]
                    .as_str()
                    .unwrap_or(solstone_core_journal_io::DEFAULT_STREAM)
                    == stream
            })
            .collect::<Vec<_>>();
        assert_eq!(written.len(), 1, "{stream}: {rows:?}");
    }
    let asked = recorder
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|request| request.name.clone())
        .collect::<Vec<_>>();
    assert_eq!(asked, vec!["participation", "participation"]);
}

const DAY2: &str = "20260812";

/// A row Sense wrote for an activity it read, in `stream`, naming `key`.
fn sensed_record(id: &str, stream: &str, key: &str, kind: &str) -> String {
    format!(
        "{}\n",
        serde_json::json!({"id":id,"facet":"work","activity":kind,"segments":[key],
            "level_avg":0.9,"description":"A run along the river.","active_entities":[],
            "created_at":NOW - 86_400_000,"stream":stream})
    )
}

fn day_context(context: &context::ThinkContext, day: &str) -> context::ThinkContext {
    segment::activity_context(context, day).unwrap()
}

/// Record `activity`'s participation work, then leave it as `state` says.
fn work(context: &context::ThinkContext, activity: &str, state: &str) {
    let record =
        solstone_core_facets::get_activity_record(&context.journal, "work", &context.day, activity)
            .unwrap()
            .unwrap();
    let hash = segment::compute_activity_input_hash(context, &context.day, &record).unwrap();
    let mut work = crate::activity_work::ActivityWork::begin(
        context,
        "work",
        activity,
        hash,
        BTreeSet::from(["participation".to_owned()]),
        false,
    )
    .unwrap();
    match state {
        "unfinished" => {}
        "parked" => work
            .park("participation", "activity_failed", context)
            .unwrap(),
        "blocked" => work
            .block("owner_field_held", "activity output is held by owner")
            .unwrap(),
        "muted" => work.set_muted_pause(true).unwrap(),
        other => panic!("{other}"),
    }
}

fn unit(day: &str, activity: &str) -> solstone_core_system::work_retry::Unit {
    solstone_core_system::work_retry::Unit::Activity {
        day: day.into(),
        facet: "work".into(),
        activity: activity.into(),
        talent: "participation".into(),
    }
}

fn held(journal: &Path, day: &str, activity: &str) -> bool {
    solstone_core_system::work_retry::read(journal, &unit(day, activity).id().unwrap())
        .unwrap()
        .is_some_and(|record| record.held)
}

fn work_files(journal: &Path) -> usize {
    fs::read_dir(journal.join("health/activity-work"))
        .map(|entries| entries.count())
        .unwrap_or(0)
}

fn receipt(context: &context::ThinkContext, day: &str, activity: &str) -> bool {
    segment::activity_provenance_path(context, day, "work", activity).is_file()
}

const STRAVA_IDS: [&str; 4] = [
    "exercise_070000_300",
    "exercise_071000_300",
    "exercise_072000_300",
    "exercise_073000_300",
];
const STRAVA_STATES: [&str; 4] = ["unfinished", "parked", "blocked", "muted"];

/// Unfinished, parked, blocked and muted work on Strava Sense records ends at
/// discovery through each talent's own completion: no call, no work file, no
/// held retry record, a receipt, and every record byte for byte. The day's
/// rebuild then finds nothing left to run. Parked and blocked capture work
/// beside it, on another day, stays as it was.
#[test]
fn activity_work_on_a_strava_reading_is_settled_at_discovery() {
    let (_journal, _roots, context, recorder) = journal(&[("participation", PARTICIPATION)]);
    let file = |day: &str| {
        context
            .journal
            .join("facets/work/activities")
            .join(format!("{day}.jsonl"))
    };
    fs::create_dir_all(file(DAY).parent().unwrap()).unwrap();
    let mut rows = String::new();
    for id in STRAVA_IDS {
        let key = id.trim_start_matches("exercise_");
        strava_tile(&context.journal, key, Some(&active("work", "exercise")));
        rows.push_str(&sensed_record(id, STRAVA, key, "exercise"));
    }
    fs::write(file(DAY), &rows).unwrap();
    let mut capture = String::new();
    for key in ["090000_300", "091000_300"] {
        let dir = context
            .journal
            .join("chronicle")
            .join(DAY2)
            .join("default")
            .join(key);
        fs::create_dir_all(dir.join("talents")).unwrap();
        fs::write(dir.join("audio.jsonl"), "{}\n").unwrap();
        fs::write(
            dir.join("talents/sense.json"),
            active("work", "meeting").to_string(),
        )
        .unwrap();
        capture.push_str(&sensed_record(
            &format!("meeting_{key}"),
            "default",
            key,
            "meeting",
        ));
    }
    fs::write(file(DAY2), &capture).unwrap();
    for (id, state) in STRAVA_IDS.into_iter().zip(STRAVA_STATES) {
        work(&context, id, state);
    }
    let other = day_context(&context, DAY2);
    work(&other, "meeting_090000_300", "parked");
    work(&other, "meeting_091000_300", "blocked");
    assert!(held(&context.journal, DAY, STRAVA_IDS[1]));
    assert_eq!(work_files(&context.journal), 6);

    // What one planner pass reads first, in either order.
    let (held_units, errors) = crate::discover_held_activities(&context.journal);
    assert_eq!(held_units, vec![unit(DAY2, "meeting_090000_300")]);
    let snapshot = crate::activity_work::snapshot_activity_work(&context.journal, NOW);
    let (held_units, more) = crate::discover_held_activities(&context.journal);
    let errors = [errors, more].concat();
    assert!(
        snapshot.errors.is_empty() && errors.is_empty(),
        "{errors:?}"
    );
    assert!(snapshot.due.iter().all(|work| work.day != DAY));
    assert!(snapshot.unfinished.iter().all(|work| work.day != DAY));
    assert_eq!(held_units, vec![unit(DAY2, "meeting_090000_300")]);
    assert!(recorder.requests.lock().unwrap().is_empty());
    assert_eq!(
        work_files(&context.journal),
        2,
        "only the capture work stays"
    );
    for id in STRAVA_IDS {
        assert!(!held(&context.journal, DAY, id), "{id}");
        assert!(receipt(&context, DAY, id), "{id}");
    }
    assert!(held(&context.journal, DAY2, "meeting_090000_300"));
    assert!(!receipt(&other, DAY2, "meeting_090000_300"));
    assert_eq!(fs::read_to_string(file(DAY)).unwrap(), rows);
    assert_eq!(fs::read_to_string(file(DAY2)).unwrap(), capture);

    // The day's rebuild: its activity steps finish with no talent run.
    let mut log = test_log(&context, "rebuild");
    crate::activity_work::reconcile_retained(&context, &mut log, 1).unwrap();
    let proof = crate::activity_work::rebuild_receipts(&context, DAY).unwrap();
    assert_eq!(proof.len(), STRAVA_IDS.len());
    // Capture work left on its day still holds that day's rebuild.
    assert!(crate::activity_work::rebuild_receipts(&other, DAY2).is_err());
    assert!(recorder.requests.lock().unwrap().is_empty());
    assert_eq!(fs::read_to_string(file(DAY)).unwrap(), rows);
}

/// The owner reactivating blocked work on a Strava reading settles it the same
/// way, and the command succeeds saying so.
#[test]
fn reactivating_a_strava_reading_settles_its_work_and_succeeds() {
    let (_journal, _roots, context, recorder) = journal(&[("participation", PARTICIPATION)]);
    strava_tile(&context.journal, TILE, Some(&active("work", "exercise")));
    let file = context
        .journal
        .join("facets/work/activities")
        .join(format!("{DAY}.jsonl"));
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let row = sensed_record(STRAVA_IDS[0], STRAVA, TILE, "exercise");
    fs::write(&file, &row).unwrap();
    work(&context, STRAVA_IDS[0], "blocked");
    let run = command(
        &context,
        &[
            "--activity",
            STRAVA_IDS[0],
            "--facet",
            "work",
            "--reactivate",
            "--day",
            DAY,
        ],
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    assert!(run.stderr.contains(STRAVA_IDS[0]), "{}", run.stderr);
    assert_eq!(work_files(&context.journal), 0);
    assert!(!held(&context.journal, DAY, STRAVA_IDS[0]));
    assert!(receipt(&context, DAY, STRAVA_IDS[0]));
    assert!(recorder.requests.lock().unwrap().is_empty());
    assert_eq!(fs::read_to_string(&file).unwrap(), row);
    // Nothing records new work on it either.
    assert!(
        crate::activity::enqueue(&context, STRAVA_IDS[0], "work", true)
            .unwrap()
            .is_none()
    );
    assert_eq!(work_files(&context.journal), 0);
}

/// A connection failure is never adopted as work on a Strava reading; the same
/// failure on a capture activity is.
#[test]
fn a_failed_send_on_a_strava_reading_seeds_no_work() {
    let (_journal, _roots, context, _recorder) = journal(&[("participation", PARTICIPATION)]);
    strava_tile(&context.journal, TILE, Some(&active("work", "exercise")));
    let capture = segment_path(&context.journal, "default", "090000_300");
    fs::create_dir_all(capture.join("talents")).unwrap();
    fs::write(capture.join("audio.jsonl"), "{}\n").unwrap();
    let file = context
        .journal
        .join("facets/work/activities")
        .join(format!("{DAY}.jsonl"));
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(
        &file,
        sensed_record(STRAVA_IDS[0], STRAVA, TILE, "exercise")
            + &sensed_record("meeting_090000_300", "default", "090000_300", "meeting"),
    )
    .unwrap();
    for activity in [STRAVA_IDS[0], "meeting_090000_300"] {
        write_health_event(
            &context.journal,
            DAY,
            &serde_json::json!({"event":"talent.fail","ts":NOW,"day":DAY,"mode":"activity",
                "facet":"work","activity":activity,"name":"participation","use_id":"old-use",
                "reason_code":"local_endpoint_unreachable"})
            .to_string(),
        );
    }
    crate::activity_work::seed_activity_retries(&context.journal, DAY, NOW).unwrap();
    assert_eq!(
        work_files(&context.journal),
        1,
        "no work is recorded for it"
    );
    let due = crate::activity_work::due_activity_retries(&context.journal, NOW).unwrap();
    assert_eq!(
        due.iter()
            .map(|work| work.activity.as_str())
            .collect::<Vec<_>>(),
        vec!["meeting_090000_300"]
    );
    assert_eq!(work_files(&context.journal), 1);
}

/// The owner deletes a workout Sense read, through the retention door as the
/// Strava delete does, and its day is rebuilt. The receipt names an input the
/// delete took away; the rebuild still finishes, with no talent run.
#[test]
fn a_deleted_strava_workouts_day_still_rebuilds() {
    let (_journal, _roots, context, recorder) = journal(&[("participation", PARTICIPATION)]);
    strava_tile(&context.journal, TILE, Some(&active("work", "exercise")));
    let file = context
        .journal
        .join("facets/work/activities")
        .join(format!("{DAY}.jsonl"));
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let row = sensed_record(STRAVA_IDS[0], STRAVA, TILE, "exercise");
    fs::write(&file, &row).unwrap();
    // Its talents finished before the delete, and left their receipt.
    let record =
        solstone_core_facets::get_activity_record(&context.journal, "work", DAY, STRAVA_IDS[0])
            .unwrap()
            .unwrap();
    let hash = segment::compute_activity_input_hash(&context, DAY, &record).unwrap();
    segment::write_activity_provenance(&context, DAY, "work", STRAVA_IDS[0], &hash).unwrap();
    let outcome = solstone_core_retention::door::remove_segments(
        &context.journal,
        &[solstone_core_retention::Target {
            day: DAY.into(),
            stream: STRAVA.into(),
            dir: TILE.into(),
        }],
        "2026-08-14T12:00:00Z",
        solstone_core_retention::RemovalReason::OwnerSegmentDelete,
        "strava-delete:test",
    );
    assert!(outcome.halted.is_none(), "{outcome:?}");
    let tile = segment_path(&context.journal, STRAVA, TILE);
    assert!(tile.join("tombstone.json").is_file());
    assert!(!tile.join("workout.json").exists());

    let mut log = test_log(&context, "rebuild");
    crate::activity_work::reconcile_retained(&context, &mut log, 1).unwrap();
    crate::activity_work::rebuild_receipts(&context, DAY).unwrap();
    assert!(recorder.requests.lock().unwrap().is_empty());
    assert_eq!(fs::read_to_string(&file).unwrap(), row);

    // The whole day's rebuild, as the owner asks for it.
    let request = solstone_core_system::work_intent::register(&context.journal, DAY, true).unwrap();
    let binding = JournalRoot::open(&context.journal)
        .unwrap()
        .identity()
        .observation_id();
    let mut rebuild = context.clone();
    rebuild.rebuild = Some(Arc::new(
        solstone_core_system::work_intent::begin_rebuild(
            &context.journal,
            DAY,
            &request.id,
            &binding,
        )
        .unwrap(),
    ));
    let args = args::ThinkArgs {
        day: Some(DAY.into()),
        refresh: true,
        ..args::ThinkArgs::default()
    };
    let outcome = daily_lifecycle::run(
        &rebuild,
        &mut log,
        &args,
        1,
        Some(std::time::Duration::from_secs(90)),
        &solstone_core_system::process::ChildLaunchContext::default(),
    );
    log.finish().unwrap();
    let intent = solstone_core_system::work_intent::read(&context.journal)
        .unwrap()
        .remove(DAY)
        .unwrap();
    assert!(
        intent.rebuild.as_ref().unwrap().segments.is_some(),
        "the rebuild seals: {outcome:?}"
    );
    assert!(recorder.requests.lock().unwrap().is_empty());
    assert_eq!(fs::read_to_string(&file).unwrap(), row);
}

/// A capture activity whose type and first segment make the same identity as
/// a Strava reading's still gets its own record.
#[test]
fn a_capture_activity_colliding_with_a_strava_reading_still_gets_its_record() {
    let (_journal, _roots, context, _recorder) = journal(&[("participation", PARTICIPATION)]);
    strava_tile(&context.journal, TILE, Some(&active("work", "exercise")));
    let file = context
        .journal
        .join("facets/work/activities")
        .join(format!("{DAY}.jsonl"));
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    // Sense named an activity by what the segment was, here `work`.
    let held = sensed_record("work_070000_300", STRAVA, TILE, "work");
    fs::write(&file, &held).unwrap();
    let capture = segment_path(&context.journal, "default", TILE);
    fs::create_dir_all(&capture).unwrap();
    fs::write(capture.join("audio.jsonl"), "{}\n").unwrap();
    write_sense(
        &context.journal,
        "default",
        TILE,
        &active("work", "exercise"),
    );
    live(&context, "default", TILE);
    let flushed = command(
        &context,
        &[
            "--flush",
            "--segment",
            TILE,
            "--stream",
            "default",
            "--day",
            DAY,
        ],
    );
    assert_eq!(flushed.exit_code, 0, "{}", flushed.stderr);
    let rows = records(&context);
    assert!(fs::read_to_string(&file).unwrap().starts_with(&held));
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[1]["id"], "work_default_070000_300", "{rows:?}");
}

const DOCUMENTS: &str = "{\n\"type\": \"generate\", \"max_output_tokens\": 1024, \"schedule\": \"segment\", \"priority\": 2, \"output\": \"md\"\n}\n";

fn segment_unit(stream: &str, key: &str, talent: &str) -> solstone_core_system::work_retry::Unit {
    solstone_core_system::work_retry::Unit::Segment {
        day: DAY.into(),
        stream: stream.into(),
        segment: key.into(),
        talent: talent.into(),
        direct: false,
    }
}

/// Hold `unit` as set aside after failures, then grant it the owner's retry.
fn held_with_owner_ask(journal: &Path, unit: &solstone_core_system::work_retry::Unit) -> String {
    use solstone_core_system::work_retry;
    let input = work_retry::input::current(journal, unit).unwrap();
    work_retry::observe_held(journal, unit, "send_failed", &input).unwrap();
    work_retry::request(journal, unit).unwrap();
    let record = work_retry::read(journal, &unit.id().unwrap())
        .unwrap()
        .unwrap();
    record
        .permission(&input, work_retry::DECLARED_FIXES)
        .unwrap()
        .cause
}

/// A segment talent held on a Strava tile, retried by the owner's ask, makes
/// no model call and ends accepted, its hold released. The same retry on a
/// capture segment still dispatches.
#[test]
fn a_held_segment_talent_on_a_strava_tile_ends_without_a_call() {
    let (_journal, _roots, context, recorder) =
        journal(&[("sense", SENSE), ("documents", DOCUMENTS)]);
    strava_tile(&context.journal, TILE, Some(&active("work", "exercise")));
    let capture = segment_path(&context.journal, "default", "090000_300");
    fs::create_dir_all(&capture).unwrap();
    fs::write(capture.join("audio.jsonl"), "{}\n").unwrap();
    write_sense(
        &context.journal,
        "default",
        "090000_300",
        &active("work", "meeting"),
    );
    let strava = segment_unit(STRAVA, TILE, "documents");
    let other = segment_unit("default", "090000_300", "documents");
    let strava_cause = held_with_owner_ask(&context.journal, &strava);
    let other_cause = held_with_owner_ask(&context.journal, &other);
    let before = tree(&segment_path(&context.journal, STRAVA, TILE));

    let mut log = test_log(&context, "retry");
    let ended = crate::retry::run(
        &context,
        &mut log,
        &strava.id().unwrap(),
        &strava_cause,
        &solstone_core_system::process::ChildLaunchContext::default(),
    )
    .unwrap();
    assert_eq!(ended.failed, 0, "{ended:?}");
    assert!(recorder.requests.lock().unwrap().is_empty());
    assert!(
        !solstone_core_system::work_retry::read(&context.journal, &strava.id().unwrap())
            .unwrap()
            .unwrap()
            .held
    );
    assert_eq!(tree(&segment_path(&context.journal, STRAVA, TILE)), before);

    let _ = crate::retry::run(
        &context,
        &mut log,
        &other.id().unwrap(),
        &other_cause,
        &solstone_core_system::process::ChildLaunchContext::default(),
    );
    log.finish().unwrap();
    let asked = recorder
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|request| request.name.clone())
        .collect::<Vec<_>>();
    assert_eq!(asked, vec!["documents"]);
}
