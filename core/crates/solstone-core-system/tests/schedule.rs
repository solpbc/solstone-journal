// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{Local, NaiveDate, TimeZone};
use serde_json::{Value, json};
use solstone_core_system::cap::{CapResolver, DefaultCapResolver};
use solstone_core_system::partition::partition_for;
use solstone_core_system::request::{
    BusTaskRequest, ExecutionRequest, ScheduledArgv, ScheduledRequest, WireTaskRequest,
};
use solstone_core_system::schedule::{
    ScheduleConfig, ScheduleEngine, ScheduleEntry, ScheduleError, ScheduleMutation, ScheduleNow,
    ScheduleSubmissionSink, add_missing_schedule_entries, baseline_cap_contributions, daily_mark,
    hour_mark, initialize_schedule_config, is_due, mutate_schedule_entries, remove_schedule_entry,
    weekly_mark,
};

struct Bed {
    root: PathBuf,
}

impl Bed {
    fn new(name: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("solstone-schedule-{name}-{stamp}"));
        fs::create_dir_all(root.join("config")).expect("config");
        fs::create_dir_all(root.join("health")).expect("health");
        Self { root }
    }

    fn config(&self) -> PathBuf {
        self.root.join("config/schedules.json")
    }

    fn state(&self) -> PathBuf {
        self.root.join("health/scheduler.json")
    }

    fn write_config(&self, value: Value) {
        fs::write(self.config(), serde_json::to_vec(&value).expect("json")).expect("config");
    }
}

impl Drop for Bed {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Default)]
struct Sink {
    requests: Mutex<Vec<ScheduledRequest>>,
    accepted: bool,
}

impl Sink {
    fn names(&self) -> Vec<String> {
        self.requests
            .lock()
            .expect("sink")
            .iter()
            .map(|request| request.scheduler_name.clone().expect("clock schedule name"))
            .collect()
    }
}

impl ScheduleSubmissionSink for Sink {
    fn submit(&self, request: ScheduledRequest) -> bool {
        self.requests.lock().expect("sink").push(request);
        self.accepted
    }
}

fn now(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> ScheduleNow {
    let local = NaiveDate::from_ymd_opt(year, month, day)
        .expect("date")
        .and_hms_opt(hour, minute, 0)
        .expect("time");
    let unix_millis = Local
        .from_local_datetime(&local)
        .earliest()
        .expect("representable local time")
        .timestamp_millis();
    ScheduleNow { local, unix_millis }
}

fn state(last_run: f64) -> Value {
    json!({"last_run": last_run})
}

fn local_epoch(value: ScheduleNow) -> f64 {
    value.unix_millis as f64 / 1_000.0
}

fn entry(every: &str) -> ScheduleEntry {
    ScheduleEntry {
        cmd: vec!["journal".to_owned(), "heartbeat".to_owned()],
        every: every.to_owned(),
        max_runtime: None,
    }
}

#[test]
fn schedule_entry_mutations_are_idempotent_and_preserve_raw_values() {
    let bed = Bed::new("entry-mutations");
    bed.write_config(json!({
        "daily_time": "04:00",
        "unrelated": {"cmd": ["journal", "other"], "every": "daily", "custom": [1, 2]},
        "retired": {"cmd": ["journal", "old"], "every": "weekly"}
    }));
    let additions = std::collections::BTreeMap::from([(
        "maintenance:backup:run".to_owned(),
        json!({"cmd": ["journal", "maintenance", "run", "backup:run"], "every": "hourly"}),
    )]);

    assert_eq!(
        add_missing_schedule_entries(&bed.config(), &additions).expect("add"),
        vec!["maintenance:backup:run"]
    );
    let after_add = fs::read(bed.config()).expect("after add");
    assert!(
        add_missing_schedule_entries(&bed.config(), &additions)
            .expect("idempotent add")
            .is_empty()
    );
    assert_eq!(fs::read(bed.config()).expect("no rewrite"), after_add);

    assert!(remove_schedule_entry(&bed.config(), "retired").expect("remove"));
    let after_remove = fs::read(bed.config()).expect("after remove");
    assert!(!remove_schedule_entry(&bed.config(), "retired").expect("idempotent remove"));
    assert_eq!(fs::read(bed.config()).expect("no rewrite"), after_remove);

    let raw: Value = serde_json::from_slice(&after_remove).expect("raw json");
    assert_eq!(raw["daily_time"], "04:00");
    assert_eq!(raw["unrelated"]["custom"], json!([1, 2]));
    assert!(raw.get("maintenance:backup:run").is_some());
}

#[test]
fn arbitrary_schedule_mutation_is_locked_and_skips_noop_rewrites() {
    let bed = Bed::new("arbitrary-mutation");
    bed.write_config(json!({
        "daily_time":"04:00",
        "legacy":{"cmd":["sol","dream","daily"],"every":"daily"},
        "unrelated":{"keep":true}
    }));
    let rewritten = mutate_schedule_entries(&bed.config(), |raw| {
        raw["legacy"]["cmd"][0] = json!("journal");
        raw["legacy"]["cmd"][1] = json!("think");
        ScheduleMutation {
            changed: true,
            value: "legacy",
        }
    })
    .unwrap();
    assert_eq!(rewritten, "legacy");
    let before = fs::read(bed.config()).unwrap();
    let changed = mutate_schedule_entries(&bed.config(), |_raw| ScheduleMutation {
        changed: false,
        value: false,
    })
    .unwrap();
    assert!(!changed);
    assert_eq!(fs::read(bed.config()).unwrap(), before);
    let raw: Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(raw["legacy"]["cmd"], json!(["journal", "think", "daily"]));
    assert_eq!(raw["unrelated"], json!({"keep":true}));
}

#[test]
fn schedule_entry_mutations_reject_malformed_json_with_path() {
    let bed = Bed::new("entry-mutations-malformed");
    fs::write(bed.config(), b"{").expect("malformed");
    let additions = std::collections::BTreeMap::new();

    let error = add_missing_schedule_entries(&bed.config(), &additions).expect_err("malformed");
    assert!(
        error
            .to_string()
            .contains(&bed.config().display().to_string())
    );
    assert!(error.to_string().contains("malformed"));
}

#[test]
fn schedule_entry_mutations_wait_for_the_existing_schedule_lock() {
    let bed = Bed::new("entry-mutations-contention");
    let lock =
        solstone_core_journal_io::hold_lock(bed.config(), Default::default()).expect("hold lock");
    let path = bed.config();
    let entries = std::collections::BTreeMap::from([(
        "maintenance:backup:run".to_owned(),
        json!({"cmd": ["journal", "maintenance", "run", "backup:run"], "every": "hourly"}),
    )]);
    let (sent, received) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        sent.send(add_missing_schedule_entries(&path, &entries))
            .expect("send result");
    });

    assert!(received.recv_timeout(Duration::from_millis(100)).is_err());
    drop(lock);
    let added = received
        .recv_timeout(Duration::from_secs(1))
        .expect("released")
        .expect("add after lock release");
    assert_eq!(added, vec!["maintenance:backup:run"]);
    worker.join().expect("worker");
}

#[test]
fn ac1_ac6_schedule_submission_is_explicitly_separate_from_bus_decode() {
    let bus = BusTaskRequest::decode(
        WireTaskRequest {
            cmd: Some(vec![
                "solstone".to_owned(),
                "call".to_owned(),
                "x".to_owned(),
            ]),
            ..WireTaskRequest::default()
        },
        "bus-ref",
    )
    .expect("bus decode");
    let _: BusTaskRequest = bus;
    let scheduled = ScheduledRequest::new(
        ScheduledArgv::from_wire(vec![
            "solstone".to_owned(),
            "call".to_owned(),
            "x".to_owned(),
        ])
        .expect("scheduled argv"),
        "sched:x:1",
        "x",
    );
    assert!(matches!(
        ExecutionRequest::Scheduled(scheduled),
        ExecutionRequest::Scheduled(_)
    ));
}

#[test]
fn ac7_ac10_mark_intervals_are_strict_and_minute_interval_is_inclusive() {
    let current = now(2026, 3, 22, 10, 0);
    let config = ScheduleConfig {
        daily_time: Some("03:00".to_owned()),
        weekly_day: Some("sunday".to_owned()),
        weekly_time: Some("03:00".to_owned()),
        ..ScheduleConfig::default()
    };
    assert!(is_due(
        &entry("hourly"),
        Some(&state(local_epoch(now(2026, 3, 22, 9, 59)))),
        &config,
        current
    ));
    assert!(!is_due(
        &entry("hourly"),
        Some(&state(local_epoch(now(2026, 3, 22, 10, 0)))),
        &config,
        current
    ));
    assert!(is_due(
        &entry("daily"),
        Some(&state(local_epoch(now(2026, 3, 21, 2, 59)))),
        &config,
        current
    ));
    assert!(!is_due(
        &entry("daily"),
        Some(&state(local_epoch(now(2026, 3, 22, 3, 0)))),
        &config,
        current
    ));
    assert!(is_due(
        &entry("weekly"),
        Some(&state(local_epoch(now(2026, 3, 15, 2, 59)))),
        &config,
        current
    ));
    assert!(!is_due(
        &entry("weekly"),
        Some(&state(local_epoch(now(2026, 3, 22, 3, 0)))),
        &config,
        current
    ));
    assert!(is_due(
        &entry("5m"),
        Some(&state(local_epoch(now(2026, 3, 22, 9, 55)))),
        &config,
        current
    ));
    assert!(!is_due(
        &entry("5m"),
        Some(&state(local_epoch(now(2026, 3, 22, 9, 56)))),
        &config,
        current
    ));
}

#[test]
fn ac11_ac12_missing_and_corrupt_last_run_follow_python_due_fallback() {
    let current = now(2026, 3, 22, 10, 0);
    let config = ScheduleConfig::default();
    // Python `_is_due` makes an absent last_run immediately due.
    assert!(is_due(&entry("hourly"), None, &config, current));
    // Python's OSError/ValueError from datetime.fromtimestamp also makes it due.
    assert!(is_due(
        &entry("hourly"),
        Some(&state(f64::MAX)),
        &config,
        current
    ));
    let unknown = entry("nonsense");
    assert!(!is_due(
        &unknown,
        Some(&state(local_epoch(current))),
        &config,
        current
    ));
}

#[test]
fn ac12_dst_marks_are_recomputed_from_naive_local_fields() {
    // The invariant is field-wise local comparison: repeated wall-clock hours
    // are not made DST-safe by a monotonic or UTC scheduler. NaiveDateTime
    // cannot encode which DST occurrence a wall-clock time belongs to.
    let first_occurrence = NaiveDate::from_ymd_opt(2026, 11, 1)
        .expect("date")
        .and_hms_opt(1, 15, 0)
        .expect("time");
    let second_occurrence = NaiveDate::from_ymd_opt(2026, 11, 1)
        .expect("date")
        .and_hms_opt(1, 45, 0)
        .expect("time");
    assert_eq!(hour_mark(first_occurrence), hour_mark(second_occurrence));
    assert_eq!(
        daily_mark(first_occurrence, None),
        daily_mark(second_occurrence, None)
    );
    assert_eq!(
        weekly_mark(first_occurrence, 6, None),
        weekly_mark(second_occurrence, 6, None)
    );
}

#[test]
fn ac13_ac16_defaults_are_idempotent_and_preserve_disabled_raw_entries() {
    let bed = Bed::new("defaults");
    bed.write_config(json!({
        "brain": {"cmd": ["journal", "brain", "refresh"], "every": "daily", "enabled": false},
        "daily_time": "04:00", "weekly_day": "monday", "weekly_time": "05:00"
    }));
    let (mut engine, diagnostics) =
        ScheduleEngine::init(bed.config(), bed.state(), now(2026, 3, 22, 10, 0)).expect("init");
    assert!(diagnostics.is_empty());
    let added = engine.register_defaults().expect("defaults");
    assert_eq!(added.len(), 5, "brain is preserved disabled: {added:?}");
    assert!(!added.iter().any(|name| name == "brain"));
    assert!(engine.register_defaults().expect("idempotent").is_empty());
    let raw: Value =
        serde_json::from_slice(&fs::read(bed.config()).expect("config")).expect("json");
    assert_eq!(raw["brain"]["enabled"], false);
    let status = engine.collect_status(now(2026, 3, 22, 10, 0));
    assert!(status.iter().all(|item| item.name != "brain"));
    assert!(status.iter().any(|item| item.name == "heartbeat"));
}

#[test]
fn fresh_schedule_defaults_are_staggered_without_backfilling_existing_configs() {
    let fresh = Bed::new("fresh-staggered-defaults");
    assert!(initialize_schedule_config(&fresh.config()).expect("fresh defaults"));
    let fresh_raw: Value =
        serde_json::from_slice(&fs::read(fresh.config()).expect("fresh config")).expect("json");
    assert_eq!(fresh_raw.as_object().expect("schedule object").len(), 2);
    assert_eq!(fresh_raw["daily_time"], "00:15");
    assert_eq!(fresh_raw["weekly_time"], "03:15");

    let existing = Bed::new("existing-defaults-without-metadata");
    let mut legacy_raw = fresh_raw;
    let legacy_object = legacy_raw.as_object_mut().expect("schedule object");
    legacy_object.remove("daily_time");
    legacy_object.remove("weekly_time");
    existing.write_config(legacy_raw);
    let before = fs::read(existing.config()).expect("legacy config");
    assert!(!initialize_schedule_config(&existing.config()).expect("legacy defaults"));
    assert_eq!(
        fs::read(existing.config()).expect("unchanged legacy config"),
        before
    );
}

#[test]
fn ac15_malformed_runtime_config_is_loud_while_missing_config_is_quiet() {
    let bed = Bed::new("malformed");
    let (_, diagnostics) =
        ScheduleEngine::init(bed.config(), bed.state(), now(2026, 3, 22, 10, 0)).expect("missing");
    assert!(diagnostics.is_empty());
    fs::write(bed.config(), b"{").expect("malformed");
    let (mut engine, diagnostics) =
        ScheduleEngine::init(bed.config(), bed.state(), now(2026, 3, 22, 10, 0))
            .expect("runtime degrade");
    assert_eq!(diagnostics.len(), 1);
    assert!(engine.register_defaults().is_err());
}

#[test]
fn ac16_non_ascii_max_runtime_is_a_diagnostic_not_a_panic() {
    let bed = Bed::new("non-ascii-cap");
    bed.write_config(json!({
        "heartbeat": {"cmd": ["journal", "heartbeat"], "every": "daily", "max_runtime": "5m🎉"}
    }));
    let (engine, diagnostics) =
        ScheduleEngine::init(bed.config(), bed.state(), now(2026, 3, 22, 10, 0)).expect("init");
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].message.contains("invalid max_runtime"));
    assert!(
        engine
            .collect_status(now(2026, 3, 22, 10, 0))
            .iter()
            .any(|item| item.name == "heartbeat")
    );
}

#[test]
fn ac17_ac19_edge_checks_retry_coarse_only_and_catch_up_is_bounded() {
    let bed = Bed::new("triggers");
    bed.write_config(json!({
        "hour": {"cmd": ["journal", "heartbeat"], "every": "hourly"},
        "minute": {"cmd": ["journal", "heartbeat"], "every": "2m"}
    }));
    fs::write(
        bed.state(),
        serde_json::to_vec(&json!({
            "hour": state(local_epoch(now(2026, 3, 22, 9, 0))),
            "minute": state(local_epoch(now(2026, 3, 22, 9, 55)))
        }))
        .expect("state"),
    )
    .expect("state write");
    let (mut engine, _) =
        ScheduleEngine::init(bed.config(), bed.state(), now(2026, 3, 22, 10, 0)).expect("init");
    let sink = Sink {
        accepted: false,
        ..Sink::default()
    };
    let report = engine.check(now(2026, 3, 22, 10, 1), &sink).expect("check");
    assert!(report.submitted.is_empty());
    assert_eq!(sink.names(), vec!["minute"]);
    let report = engine
        .check(now(2026, 3, 22, 11, 0), &sink)
        .expect("hour boundary");
    assert!(report.submitted.is_empty());
    assert!(sink.names().contains(&"hour".to_owned()));
    let sink = Sink {
        accepted: true,
        ..Sink::default()
    };
    let caught = engine.catch_up(now(2026, 3, 22, 11, 0), &sink, &BTreeSet::new());
    assert!(caught.submitted.len() <= 2);
}

#[test]
fn catch_up_leaves_fresh_entries_to_their_next_mark() {
    // A never-run entry is due, so catch-up would submit it at boot. An entry
    // that did not exist before this boot was not missed: named as fresh, it
    // waits for its cadence boundary like an entry added to a running scheduler.
    let bed = Bed::new("fresh");
    bed.write_config(json!({
        "missed": {"cmd": ["journal", "heartbeat"], "every": "daily"},
        "maintenance:new": {"cmd": ["journal", "maintenance", "run", "new"], "every": "daily"}
    }));
    let (mut engine, _) =
        ScheduleEngine::init(bed.config(), bed.state(), now(2026, 3, 22, 10, 0)).expect("init");
    let sink = Sink {
        accepted: true,
        ..Sink::default()
    };
    let fresh = BTreeSet::from(["maintenance:new".to_owned()]);
    let caught = engine.catch_up(now(2026, 3, 22, 10, 0), &sink, &fresh);
    assert_eq!(caught.submitted, vec!["missed".to_owned()]);
    assert_eq!(sink.names(), vec!["missed"]);

    let sink = Sink {
        accepted: true,
        ..Sink::default()
    };
    engine
        .check(now(2026, 3, 23, 0, 16), &sink)
        .expect("daily boundary");
    assert!(
        sink.names().contains(&"maintenance:new".to_owned()),
        "a fresh entry runs at its first daily mark: {:?}",
        sink.names()
    );
}

#[test]
fn ac19_coarse_boundary_gate_prevents_duplicate_without_completion() {
    let bed = Bed::new("boundary");
    bed.write_config(json!({"hour": {"cmd": ["journal", "heartbeat"], "every": "hourly"}}));
    let (mut engine, _) =
        ScheduleEngine::init(bed.config(), bed.state(), now(2026, 3, 22, 10, 0)).expect("init");
    let sink = Sink {
        accepted: true,
        ..Sink::default()
    };
    engine.check(now(2026, 3, 22, 11, 0), &sink).expect("first");
    engine
        .check(now(2026, 3, 22, 11, 1), &sink)
        .expect("second");
    assert_eq!(sink.names(), vec!["hour"]);
}

#[test]
fn ac20_ac23_completion_recovers_and_is_mutex_serialized() {
    let bed = Bed::new("completion");
    fs::write(bed.state(), b"{").expect("corrupt state");
    let (engine, _) =
        ScheduleEngine::init(bed.config(), bed.state(), now(2026, 3, 22, 10, 0)).expect("init");
    // A failed run writes last_run exactly as a successful run does.
    engine
        .record_completion("failed", 100.0, "error", "ref-failed")
        .expect("failed write");
    let engine = Arc::new(engine);
    let mut workers = Vec::new();
    for index in 0..8 {
        let engine = Arc::clone(&engine);
        workers.push(std::thread::spawn(move || {
            engine
                .record_completion(&format!("n{index}"), index as f64, "ok", "ref")
                .expect("write");
        }));
    }
    for worker in workers {
        worker.join().expect("join");
    }
    let state: Value =
        serde_json::from_slice(&fs::read(bed.state()).expect("state")).expect("json");
    assert_eq!(state["failed"]["last_run"], 100.0);
    assert_eq!(state.as_object().expect("object").len(), 9);
}

#[test]
fn ac20_completion_preserves_other_state_and_recovers_missing_file() {
    let bed = Bed::new("completion-existing");
    let untouched = json!({
        "last_run": 42.0,
        "last_status": "old-status",
        "last_ref": "old-ref",
        "other": "preserved"
    });
    fs::write(
        bed.state(),
        serde_json::to_vec(&json!({
            "target": {"last_run": 1.0, "last_status": "old", "last_ref": "old-ref"},
            "untouched": untouched
        }))
        .expect("state"),
    )
    .expect("state write");
    let (engine, _) =
        ScheduleEngine::init(bed.config(), bed.state(), now(2026, 3, 22, 10, 0)).expect("init");
    engine
        .record_completion("target", 100.0, "ok", "target-ref")
        .expect("target write");
    let state: Value =
        serde_json::from_slice(&fs::read(bed.state()).expect("state")).expect("json");
    assert_eq!(state["untouched"], untouched);
    assert_eq!(state["target"]["last_run"], 100.0);

    let missing = Bed::new("completion-missing");
    let (engine, _) =
        ScheduleEngine::init(missing.config(), missing.state(), now(2026, 3, 22, 10, 0))
            .expect("init");
    engine
        .record_completion("fresh", 200.0, "ok", "fresh-ref")
        .expect("missing-state write");
    let state: Value =
        serde_json::from_slice(&fs::read(missing.state()).expect("state")).expect("json");
    assert_eq!(
        state,
        json!({
            "fresh": {"last_run": 200.0, "last_status": "ok", "last_ref": "fresh-ref"}
        })
    );
}

#[test]
fn ac20_non_object_runtime_state_is_loud_from_init_and_check() {
    let bed = Bed::new("state-shape");
    fs::write(bed.state(), b"[]").expect("state shape");
    assert!(matches!(
        ScheduleEngine::init(bed.config(), bed.state(), now(2026, 3, 22, 10, 0)),
        Err(ScheduleError::StateShape { .. })
    ));

    fs::write(bed.state(), b"{}").expect("valid state");
    let (mut engine, _) =
        ScheduleEngine::init(bed.config(), bed.state(), now(2026, 3, 22, 10, 0)).expect("init");
    fs::write(bed.state(), b"[]").expect("state shape");
    assert!(matches!(
        engine.check(
            now(2026, 3, 22, 10, 1),
            &Sink {
                accepted: true,
                ..Sink::default()
            }
        ),
        Err(ScheduleError::StateShape { .. })
    ));
}

#[test]
fn ac24_status_matches_metadata_shape_and_floors_every_on_demand() {
    let bed = Bed::new("status");
    bed.write_config(json!({
        "daily_time": "04:00", "weekly_day": "monday", "weekly_time": "05:00",
        "daily": {"cmd": ["journal", "heartbeat"], "every": "daily"},
        "weekly": {"cmd": ["journal", "heartbeat"], "every": "weekly"},
        "minute": {"cmd": ["journal", "heartbeat"], "every": "2m"}
    }));
    let (engine, _) =
        ScheduleEngine::init(bed.config(), bed.state(), now(2026, 3, 22, 10, 0)).expect("init");
    let status = engine.collect_status(now(2026, 3, 22, 10, 0));
    let daily = status
        .iter()
        .find(|item| item.name == "daily")
        .expect("daily");
    assert_eq!(daily.daily_time.as_deref(), Some("04:00"));
    assert!(daily.weekly_day.is_none());
    let weekly = status
        .iter()
        .find(|item| item.name == "weekly")
        .expect("weekly");
    assert_eq!(weekly.weekly_day.as_deref(), Some("monday"));
    assert_eq!(weekly.weekly_time.as_deref(), Some("05:00"));
    assert_eq!(
        status
            .iter()
            .find(|item| item.name == "minute")
            .expect("minute")
            .every,
        "5m"
    );
}

#[test]
fn ac24_status_omits_empty_time_metadata() {
    let bed = Bed::new("empty-status-time");
    bed.write_config(json!({
        "daily_time": "", "weekly_day": "monday", "weekly_time": "",
        "daily": {"cmd": ["journal", "heartbeat"], "every": "daily"},
        "weekly": {"cmd": ["journal", "heartbeat"], "every": "weekly"}
    }));
    let (engine, _) =
        ScheduleEngine::init(bed.config(), bed.state(), now(2026, 3, 22, 10, 0)).expect("init");
    let status = engine.collect_status(now(2026, 3, 22, 10, 0));
    assert!(
        status
            .iter()
            .find(|item| item.name == "daily")
            .expect("daily")
            .daily_time
            .is_none()
    );
    let weekly = status
        .iter()
        .find(|item| item.name == "weekly")
        .expect("weekly");
    assert_eq!(weekly.weekly_day.as_deref(), Some("monday"));
    assert!(weekly.weekly_time.is_none());
}

#[test]
fn scheduled_caps_follow_each_loaded_entry_without_changing_partition_baselines() {
    let bed = Bed::new("caps");
    bed.write_config(json!({
        "edges": {"cmd": ["journal", "indexer", "--rebuild-edges"], "every": "hourly", "max_runtime": "10m"},
        "rescan": {"cmd": ["journal", "indexer", "--rescan"], "every": "hourly", "max_runtime": "20m"}
    }));
    let (mut engine, _) =
        ScheduleEngine::init(bed.config(), bed.state(), now(2026, 3, 22, 10, 0)).expect("init");
    let sink = Sink {
        accepted: true,
        ..Sink::default()
    };
    engine
        .check(now(2026, 3, 22, 11, 0), &sink)
        .expect("first tick");
    let first = sink.requests.lock().expect("sink").clone();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].max_runtime, Some(Duration::from_secs(600)));
    assert_eq!(first[1].max_runtime, Some(Duration::from_secs(1200)));
    assert_eq!(first[0].cmd.partition(), first[1].cmd.partition());

    bed.write_config(json!({
        "edges": {"cmd": ["journal", "indexer", "--rebuild-edges"], "every": "hourly", "max_runtime": "25m"},
        "rescan": {"cmd": ["journal", "indexer", "--rescan"], "every": "hourly"}
    }));
    engine
        .check(now(2026, 3, 22, 12, 0), &sink)
        .expect("reload tick");
    let requests = sink.requests.lock().expect("sink");
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].max_runtime, Some(Duration::from_secs(600)));
    assert_eq!(requests[2].max_runtime, Some(Duration::from_secs(1500)));
    assert_eq!(requests[3].max_runtime, None);

    let mut resolver = DefaultCapResolver::default();
    for (partition, cap) in baseline_cap_contributions() {
        resolver.set_override(partition, cap);
    }
    let indexer = partition_for(&["journal".to_owned(), "indexer".to_owned()]);
    assert_eq!(resolver.cap_for(&indexer), Duration::from_secs(7200));
}
