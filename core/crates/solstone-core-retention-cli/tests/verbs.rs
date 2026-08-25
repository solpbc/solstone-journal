// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(
    clippy::disallowed_methods,
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "the real-binary bed owns its temporary journal"
)]

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};

fn mark_at(label: &str) -> DateTime<Utc> {
    let raw = match label {
        "first" => "2026-08-06T12:00:00Z",
        "second" => "2026-08-06T12:00:01Z",
        "third" => "2026-08-06T12:00:02Z",
        other => other,
    };
    DateTime::parse_from_rfc3339(raw)
        .unwrap()
        .with_timezone(&Utc)
}

use serde_json::{Value, json};
use solstone_core_retention::Target;
use solstone_core_retention::marks::{
    Failure, MarkId, Proposal, RemovalClass, load, reconcile, record_failure,
};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Bed {
    root: PathBuf,
}

impl Bed {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "retention-cli-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn proven_segment(&self, day: &str, stream: &str, dir: &str, stamp: &str) -> PathBuf {
        let segment = self.root.join("chronicle").join(day).join(stream).join(dir);
        fs::create_dir_all(&segment).unwrap();
        let raw = b"the owner's recording";
        fs::write(segment.join("audio.flac"), raw).unwrap();
        let header = json!({
            "segment": dir,
            "_solstone_processing": {
                "schema": "solstone.processing.v1",
                "state": "analyzed",
                "reason_code": "ok",
                "handler": "transcribe",
                "attempted_at": stamp,
                "input_size": raw.len(),
            }
        });
        fs::write(
            segment.join("audio.jsonl"),
            format!("{header}\n{{\"start\":0.0,\"text\":\"hello\"}}\n"),
        )
        .unwrap();
        segment
    }

    fn add_proven_raw(&self, segment: &Path, dir: &str, name: &str, stamp: &str) {
        let raw = b"another owner's recording";
        fs::write(segment.join(name), raw).unwrap();
        let stem = name.rsplit_once('.').unwrap().0;
        let header = json!({
            "segment": dir,
            "_solstone_processing": {
                "schema": "solstone.processing.v1",
                "state": "analyzed",
                "reason_code": "ok",
                "handler": "transcribe",
                "attempted_at": stamp,
                "input_size": raw.len(),
            }
        });
        fs::write(
            segment.join(format!("{stem}.jsonl")),
            format!("{header}\n{{\"start\":0.0,\"text\":\"hello\"}}\n"),
        )
        .unwrap();
    }

    fn run(&self, verb: &str, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-retention"));
        command.arg(verb);
        command.args(args);
        command.output().unwrap()
    }

    fn journal(&self) -> &Path {
        &self.root
    }
}

impl Drop for Bed {
    fn drop(&mut self) {
        match fs::remove_dir_all(&self.root) {
            Ok(()) | Err(_) => {}
        }
    }
}

fn receipt(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

fn armed_policy() -> String {
    json!({
        "default_rule": {"anchor": "captured", "period": 1, "priority": 0},
        "enabled": true,
    })
    .to_string()
}

fn target() -> Target {
    Target {
        day: "20260701".to_owned(),
        stream: "field.audio".to_owned(),
        dir: "070000_17".to_owned(),
    }
}

fn proposal(names: Vec<String>) -> Proposal {
    Proposal {
        bytes: 1,
        reason: "test approval".to_owned(),
        names,
    }
}

fn keys(body: &Value) -> BTreeSet<&str> {
    body.as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

fn register_json(bed: &Bed) -> Value {
    serde_json::to_value(load(bed.journal()).unwrap()).unwrap()
}

struct HeldLock {
    child: Child,
    ready: PathBuf,
}

impl Drop for HeldLock {
    fn drop(&mut self) {
        match self.child.kill() {
            Ok(()) | Err(_) => {}
        }
        match self.child.wait() {
            Ok(_) | Err(_) => {}
        }
        match fs::remove_file(&self.ready) {
            Ok(()) | Err(_) => {}
        }
    }
}

#[cfg(target_os = "macos")]
fn start_lock_holder(sidecar: &Path, ready: &Path) -> Child {
    Command::new("python3")
        .args([
            "-c",
            "import fcntl, pathlib, sys, time; lock = open(sys.argv[1], 'a'); \
             fcntl.flock(lock, fcntl.LOCK_EX); pathlib.Path(sys.argv[2]).touch(); time.sleep(60)",
        ])
        .arg(&sidecar)
        .arg(&ready)
        .spawn()
        .unwrap()
}

#[cfg(not(target_os = "macos"))]
fn start_lock_holder(sidecar: &Path, ready: &Path) -> Child {
    Command::new("flock")
        .args(["--exclusive", "--no-fork"])
        .arg(sidecar)
        .args(["sh", "-c", "touch \"$1\"; exec sleep 60", "sh"])
        .arg(ready)
        .spawn()
        .unwrap()
}

fn hold_lock(path: &Path) -> HeldLock {
    let file_name = path.file_name().unwrap().to_string_lossy();
    let sidecar = path.with_file_name(format!("{file_name}.lock"));
    let ready = sidecar.with_extension("held");
    let mut child = start_lock_holder(&sidecar, &ready);
    for _attempt in 0..100 {
        if ready.exists() {
            return HeldLock { child, ready };
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    match child.kill() {
        Ok(()) | Err(_) => {}
    }
    match child.wait() {
        Ok(_) | Err(_) => {}
    }
    panic!("the test lock was not acquired");
}

fn make_day_unreadable(day: &Path) {
    fs::set_permissions(day, fs::Permissions::from_mode(0o000)).unwrap();
}

fn restore_day_permissions(day: &Path) {
    fs::set_permissions(day, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn marks_is_read_only_and_reports_an_empty_register() {
    let bed = Bed::new("marks-empty");
    let output = bed.run("marks", &["--journal", bed.journal().to_str().unwrap()]);
    let body = receipt(&output);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(body["verb"], "marks");
    assert_eq!(body["marks"]["marks"], json!({}));
    assert!(!bed.journal().join("health").exists());
}

#[test]
fn mark_refuses_an_unavailable_chronicle_without_reconciling_the_register() {
    let bed = Bed::new("chronicle-file");
    fs::write(bed.journal().join("chronicle"), b"not a directory").unwrap();
    let output = bed.run(
        "mark",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
        ],
    );
    let body = receipt(&output);
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(body["verb"], "mark");
    assert!(body["error"].as_str().unwrap().contains("chronicle"));
    assert!(!bed.journal().join("health").exists());
}

#[test]
fn mark_is_non_destructive_and_keeps_marked_at_for_the_same_proposal() {
    let bed = Bed::new("mark-idempotent");
    let segment = bed.proven_segment(
        "20260706",
        "field.audio",
        "070000_17",
        "2026-07-06T00:00:00Z",
    );
    let policy = json!({
        "default_rule": {"anchor": "captured", "period": 7, "priority": 0},
        "enabled": true,
    })
    .to_string();
    let first = bed.run(
        "mark",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
        ],
    );
    let first_body = receipt(&first);
    let marks = first_body["marks"]["marks"].as_object().unwrap();
    let mark = marks.values().next().unwrap();
    let marked_at = mark["marked_at"].clone();
    assert_eq!(first.status.code(), Some(0));
    assert_eq!(
        mark["proposal"]["reason"],
        "policy eligibility: Eligible { anchor: Captured, age_days: 30, period: Days(7) }"
    );
    assert_eq!(
        fs::read(segment.join("audio.flac")).unwrap(),
        b"the owner's recording"
    );

    let second = bed.run(
        "mark",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-07",
            "--now",
            "2026-08-07T00:00:00Z",
            "--policy",
            &policy,
        ],
    );
    let second_body = receipt(&second);
    let second_mark = second_body["marks"]["marks"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap();
    assert_eq!(second.status.code(), Some(0));
    assert_eq!(second_mark["marked_at"], marked_at);
    assert_eq!(
        fs::read(segment.join("audio.flac")).unwrap(),
        b"the owner's recording"
    );
}

#[test]
fn mark_receipt_discloses_each_skipped_segment_reason() {
    let bed = Bed::new("mark-skipped-segments");
    let no_media = bed
        .journal()
        .join("chronicle/20260701/field.audio/010000_17");
    fs::create_dir_all(&no_media).unwrap();
    bed.proven_segment(
        "20260806",
        "field.audio",
        "020000_17",
        "2026-08-06T00:00:00Z",
    );
    let held = bed
        .journal()
        .join("chronicle/20260701/field.audio/030000_17");
    fs::create_dir_all(&held).unwrap();
    fs::write(held.join("audio.flac"), b"not yet proven").unwrap();
    let policy = armed_policy();

    let output = bed.run(
        "mark",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
        ],
    );
    let body = receipt(&output);
    let skipped = body["plan"]["skipped_segments"].as_array().unwrap();
    let by_dir = skipped
        .iter()
        .map(|entry| (entry["dir"].as_str().unwrap(), entry))
        .collect::<std::collections::BTreeMap<_, _>>();

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(body["plan"]["skipped"], 3);
    assert_eq!(by_dir["010000_17"]["reason"], "no_media");
    assert_eq!(by_dir["010000_17"]["day"], "20260701");
    assert_eq!(by_dir["010000_17"]["stream"], "field.audio");
    assert_eq!(by_dir["020000_17"]["reason"], "policy");
    assert!(by_dir["020000_17"]["eligibility"].is_object());
    assert_eq!(by_dir["020000_17"]["day"], "20260806");
    assert_eq!(by_dir["020000_17"]["stream"], "field.audio");
    assert_eq!(by_dir["030000_17"]["reason"], "held");
    assert!(
        !by_dir["030000_17"]["blockers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(by_dir["030000_17"]["day"], "20260701");
    assert_eq!(by_dir["030000_17"]["stream"], "field.audio");
}

#[test]
fn mark_continues_past_an_unreadable_day() {
    let bed = Bed::new("mark-unreadable-day");
    bed.proven_segment(
        "20260701",
        "field.audio",
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    let unreadable = bed.journal().join("chronicle/20260702");
    fs::create_dir_all(&unreadable).unwrap();
    make_day_unreadable(&unreadable);
    let policy = armed_policy();

    let output = bed.run(
        "mark",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
        ],
    );
    restore_day_permissions(&unreadable);
    let body = receipt(&output);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(body["ok"], true);
    assert_eq!(body["plan"]["unreadable_days"], json!(["20260702"]));
    assert_eq!(body["marks"]["marks"].as_object().unwrap().len(), 1);
}

#[test]
fn mark_preserves_existing_marks_under_an_unreadable_day() {
    let bed = Bed::new("mark-preserves-unreadable-day");
    let day = "20260701";
    bed.proven_segment(day, "field.audio", "070000_17", "2026-07-01T00:00:00Z");
    let policy = armed_policy();
    let first = bed.run(
        "mark",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
        ],
    );
    let first_body = receipt(&first);
    let (mark_id, original_mark) = first_body["marks"]["marks"]
        .as_object()
        .unwrap()
        .iter()
        .next()
        .map(|(id, mark)| (id.clone(), mark.clone()))
        .unwrap();
    let unreadable = bed.journal().join(format!("chronicle/{day}"));
    make_day_unreadable(&unreadable);

    let second = bed.run(
        "mark",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-07",
            "--now",
            "2026-08-07T00:00:00Z",
            "--policy",
            &policy,
        ],
    );
    restore_day_permissions(&unreadable);
    let second_body = receipt(&second);

    assert_eq!(second.status.code(), Some(0));
    assert_eq!(second_body["plan"]["unreadable_days"], json!([day]));
    assert_eq!(
        second_body["marks"]["marks"][mark_id.as_str()],
        original_mark
    );
}

#[test]
fn mark_offload_is_idempotent_and_refuses_a_missing_file() {
    let bed = Bed::new("mark-offload");
    bed.proven_segment(
        "20260701",
        "field.audio",
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    let journal = bed.journal().to_str().unwrap();
    let first = bed.run(
        "mark-offload",
        &[
            "--journal",
            journal,
            "--day",
            "20260701",
            "--stream",
            "field.audio",
            "--dir",
            "070000_17",
            "--file",
            "audio.flac",
            "--reason",
            "archive://audio",
            "--now",
            "2026-08-06T00:00:00Z",
        ],
    );
    let first_body = receipt(&first);
    let marked_at = first_body["marks"]["marks"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap()["marked_at"]
        .clone();
    assert_eq!(first.status.code(), Some(0));
    let second = bed.run(
        "mark-offload",
        &[
            "--journal",
            journal,
            "--day",
            "20260701",
            "--stream",
            "field.audio",
            "--dir",
            "070000_17",
            "--file",
            "audio.flac",
            "--reason",
            "archive://audio",
            "--now",
            "2026-08-07T00:00:00Z",
        ],
    );
    let second_body = receipt(&second);
    assert_eq!(second.status.code(), Some(0));
    assert_eq!(
        second_body["marks"]["marks"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap()["marked_at"],
        marked_at
    );

    let refused = bed.run(
        "mark-offload",
        &[
            "--journal",
            journal,
            "--day",
            "20260701",
            "--stream",
            "field.audio",
            "--dir",
            "070000_17",
            "--file",
            "missing.flac",
            "--reason",
            "archive://audio",
            "--now",
            "2026-08-07T00:00:00Z",
        ],
    );
    let refused_body = receipt(&refused);
    assert_eq!(refused.status.code(), Some(3));
    assert_eq!(refused_body["verb"], "mark-offload");
}

#[test]
fn mark_offload_preserves_marks_for_other_segments() {
    let bed = Bed::new("mark-offload-preserves");
    bed.proven_segment(
        "20260701",
        "field.audio",
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    bed.proven_segment(
        "20260702",
        "field.audio",
        "070100_17",
        "2026-07-02T00:00:00Z",
    );
    let journal = bed.journal().to_str().unwrap();

    let first = bed.run(
        "mark-offload",
        &[
            "--journal",
            journal,
            "--day",
            "20260701",
            "--stream",
            "field.audio",
            "--dir",
            "070000_17",
            "--file",
            "audio.flac",
            "--reason",
            "archive://first",
            "--now",
            "2026-08-06T00:00:00Z",
        ],
    );
    assert_eq!(first.status.code(), Some(0));
    let second = bed.run(
        "mark-offload",
        &[
            "--journal",
            journal,
            "--day",
            "20260702",
            "--stream",
            "field.audio",
            "--dir",
            "070100_17",
            "--file",
            "audio.flac",
            "--reason",
            "archive://second",
            "--now",
            "2026-08-06T00:01:00Z",
        ],
    );
    let second_receipt = receipt(&second);
    let marks = second_receipt["marks"]["marks"].as_object().unwrap();

    assert_eq!(second.status.code(), Some(0));
    assert_eq!(marks.len(), 2);
    assert!(marks.values().any(|mark| {
        mark["target"]["day"] == "20260701" && mark["target"]["dir"] == "070000_17"
    }));
    assert!(marks.values().any(|mark| {
        mark["target"]["day"] == "20260702" && mark["target"]["dir"] == "070100_17"
    }));
}

#[test]
fn resolve_offload_removes_a_mark_and_is_idempotent() {
    let bed = Bed::new("resolve-offload");
    bed.proven_segment(
        "20260701",
        "field.audio",
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    let journal = bed.journal().to_str().unwrap();
    let marked = bed.run(
        "mark-offload",
        &[
            "--journal",
            journal,
            "--day",
            "20260701",
            "--stream",
            "field.audio",
            "--dir",
            "070000_17",
            "--file",
            "audio.flac",
            "--reason",
            "archive://audio",
            "--now",
            "2026-08-06T00:00:00Z",
        ],
    );
    assert_eq!(marked.status.code(), Some(0));

    let args = [
        "--journal",
        journal,
        "--day",
        "20260701",
        "--stream",
        "field.audio",
        "--dir",
        "070000_17",
        "--file",
        "audio.flac",
    ];
    let resolved = bed.run("resolve-offload", &args);
    let resolved_body = receipt(&resolved);
    assert_eq!(resolved.status.code(), Some(0));
    assert_eq!(resolved_body["verb"], "resolve-offload");
    assert_eq!(resolved_body["marks"]["marks"], json!({}));

    let repeated = bed.run("resolve-offload", &args);
    let repeated_body = receipt(&repeated);
    assert_eq!(repeated.status.code(), Some(0));
    assert_eq!(repeated_body["marks"]["marks"], json!({}));
}

#[test]
fn remove_marked_reproves_then_releases_the_named_policy_mark() {
    let bed = Bed::new("remove-marked");
    let segment = bed.proven_segment(
        "20260701",
        "field.audio",
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    let untouched = b"leave this file unchanged";
    fs::write(segment.join("notes.txt"), untouched).unwrap();
    let policy = armed_policy();
    let marked = bed.run(
        "mark",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
        ],
    );
    let id = receipt(&marked)["marks"]["marks"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    let removed = bed.run(
        "remove-marked",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
            "--mark",
            &id,
        ],
    );
    let body = receipt(&removed);
    assert_eq!(removed.status.code(), Some(0));
    assert_eq!(body["verb"], "remove-marked");
    assert_eq!(
        keys(&body),
        BTreeSet::from(["detail", "index", "ok", "outcome", "verb"])
    );
    assert!(!segment.join("audio.flac").exists());
    assert_eq!(fs::read(segment.join("notes.txt")).unwrap(), untouched);
    let listed = bed.run("marks", &["--journal", bed.journal().to_str().unwrap()]);
    assert_eq!(receipt(&listed)["marks"]["marks"], json!({}));
}

#[test]
fn remove_marked_refuses_unknown_flags_without_releasing() {
    let bed = Bed::new("remove-marked-unknown-flag");
    let segment = bed.proven_segment(
        "20260701",
        "field.audio",
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    let policy = armed_policy();
    let marked = bed.run(
        "mark",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
        ],
    );
    let id = receipt(&marked)["marks"]["marks"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    let output = bed.run(
        "remove-marked",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
            "--mark",
            &id,
            "--typo",
            "value",
        ],
    );
    let body = receipt(&output);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(keys(&body), BTreeSet::from(["error", "ok", "verb"]));
    assert_eq!(body["verb"], "remove-marked");
    assert!(segment.join("audio.flac").exists());
    assert!(
        load(bed.journal())
            .unwrap()
            .marks
            .contains_key(&MarkId::parse(&id).unwrap())
    );
}

#[test]
fn remove_marked_stops_at_a_locked_later_mark_without_losing_completed_rows() {
    let bed = Bed::new("remove-marked-halt-later");
    let first = Target {
        day: "20260701".to_owned(),
        ..target()
    };
    let second = Target {
        day: "20260702".to_owned(),
        ..target()
    };
    let first_segment = bed.proven_segment(
        &first.day,
        &first.stream,
        &first.dir,
        "2026-07-01T00:00:00Z",
    );
    let second_segment = bed.proven_segment(
        &second.day,
        &second.stream,
        &second.dir,
        "2026-07-01T00:00:00Z",
    );
    let first_proposal = proposal(vec!["audio.flac".to_owned()]);
    let second_proposal = proposal(vec!["audio.flac".to_owned()]);
    reconcile(
        bed.journal(),
        RemovalClass::PolicyRawRelease,
        &[
            (first.clone(), first_proposal.clone()),
            (second.clone(), second_proposal.clone()),
        ],
        mark_at("first"),
    )
    .unwrap();
    let first_id = MarkId::derive(
        RemovalClass::PolicyRawRelease,
        &first,
        &first_proposal.names,
    );
    let second_id = MarkId::derive(
        RemovalClass::PolicyRawRelease,
        &second,
        &second_proposal.names,
    );
    let _held = hold_lock(&second_segment);
    let policy = armed_policy();
    let output = bed.run(
        "remove-marked",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
            "--mark",
            first_id.as_str(),
            "--mark",
            second_id.as_str(),
        ],
    );
    let body = receipt(&output);
    assert_eq!(output.status.code(), Some(4));
    assert!(!first_segment.join("audio.flac").exists());
    assert!(second_segment.join("audio.flac").exists());
    assert_eq!(body["outcome"]["targets"].as_array().unwrap().len(), 1);
    assert_eq!(body["outcome"]["targets"][0]["target"]["day"], first.day);
    assert_eq!(
        body["outcome"]["halted"]["reason"],
        format!(
            "i couldn't start on the originals for {} because something else is using them. the rest of the removal list wasn't attempted (1 remaining).",
            second_id.as_str()
        )
    );
}

#[test]
fn remove_marked_reports_halted_before_start_when_the_first_mark_is_locked() {
    let bed = Bed::new("remove-marked-halt-first");
    let segment = bed.proven_segment(
        "20260701",
        "field.audio",
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    let proposed = proposal(vec!["audio.flac".to_owned()]);
    let marked = reconcile(
        bed.journal(),
        RemovalClass::PolicyRawRelease,
        &[(target(), proposed.clone())],
        mark_at("first"),
    )
    .unwrap();
    let id = marked.marks.keys().next().unwrap().as_str().to_owned();
    let _held = hold_lock(&segment);
    let policy = armed_policy();
    let output = bed.run(
        "remove-marked",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
            "--mark",
            &id,
        ],
    );
    let body = receipt(&output);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(body["outcome"]["targets"], json!([]));
    assert_eq!(
        body["outcome"]["halted"]["reason"],
        format!(
            "i couldn't start on the originals for {id} because something else is using them. the rest of the removal list wasn't attempted (1 remaining)."
        )
    );
    assert!(segment.join("audio.flac").exists());
}

#[test]
fn remove_marked_keeps_its_receipt_when_resolving_the_mark_fails() {
    let bed = Bed::new("remove-marked-register-error");
    let segment = bed.proven_segment(
        "20260701",
        "field.audio",
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    let policy = armed_policy();
    let marked = bed.run(
        "mark",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
        ],
    );
    let id = receipt(&marked)["marks"]["marks"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    let output = {
        let _held = hold_lock(&bed.journal().join("health/retention-marks.json"));
        bed.run(
            "remove-marked",
            &[
                "--journal",
                bed.journal().to_str().unwrap(),
                "--today",
                "2026-08-06",
                "--now",
                "2026-08-06T00:00:00Z",
                "--policy",
                &policy,
                "--mark",
                &id,
            ],
        )
    };
    let body = receipt(&output);
    assert_eq!(output.status.code(), Some(0));
    assert!(!segment.join("audio.flac").exists());
    assert_eq!(body["outcome"]["targets"].as_array().unwrap().len(), 1);
    assert!(
        body["detail"]["register_error"]
            .as_str()
            .unwrap()
            .contains("removal register is busy")
    );
    assert!(
        load(bed.journal())
            .unwrap()
            .marks
            .contains_key(&MarkId::parse(&id).unwrap())
    );
}

#[test]
fn remove_marked_repeated_approval_leaves_the_first_removal_intact() {
    let bed = Bed::new("remove-marked-repeat");
    let segment = bed.proven_segment(
        "20260701",
        "field.audio",
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    let policy = armed_policy();
    let marked = bed.run(
        "mark",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
        ],
    );
    let id = receipt(&marked)["marks"]["marks"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    let args = [
        "--journal",
        bed.journal().to_str().unwrap(),
        "--today",
        "2026-08-06",
        "--now",
        "2026-08-06T00:00:00Z",
        "--policy",
        &policy,
        "--mark",
        &id,
    ];
    let first = bed.run("remove-marked", &args);
    let second = bed.run("remove-marked", &args);
    assert_eq!(first.status.code(), Some(0));
    assert_eq!(second.status.code(), Some(3));
    assert!(!segment.join("audio.flac").exists());
    assert_eq!(receipt(&second)["outcome"], Value::Null);
}

#[test]
fn remove_marked_refuses_not_required_and_failed_marks_before_touching_disk() {
    let bed = Bed::new("preflight");
    let owner = target();
    let owner_register = reconcile(
        bed.journal(),
        RemovalClass::OwnerSegmentRemoval,
        &[(owner, proposal(Vec::new()))],
        mark_at("first"),
    )
    .unwrap();
    let owner_id = owner_register
        .marks
        .keys()
        .next()
        .unwrap()
        .as_str()
        .to_owned();
    let failed_target = Target {
        day: "20260702".to_owned(),
        ..target()
    };
    let failed_register = record_failure(
        bed.journal(),
        RemovalClass::PolicyRawRelease,
        &failed_target,
        &["audio.flac".to_owned()],
        Failure {
            at: "first".to_owned(),
            reason: "needs recovery".to_owned(),
            staged: Some("set-aside/segment".to_owned()),
        },
        mark_at("first"),
    )
    .unwrap();
    let failed_id = failed_register
        .marks
        .keys()
        .find(|id| id.as_str() != owner_id)
        .unwrap()
        .as_str()
        .to_owned();
    for id in [&owner_id, &failed_id] {
        let output = bed.run(
            "remove-marked",
            &[
                "--journal",
                bed.journal().to_str().unwrap(),
                "--today",
                "2026-08-06",
                "--now",
                "2026-08-06T00:00:00Z",
                "--mark",
                id,
            ],
        );
        assert_eq!(output.status.code(), Some(3));
        assert_eq!(receipt(&output)["verb"], "remove-marked");
    }
}

#[test]
fn remove_marked_preflight_rejects_a_missing_id_before_a_valid_mark_runs() {
    let bed = Bed::new("missing-id");
    let segment = bed.proven_segment(
        "20260701",
        "field.audio",
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    let policy = armed_policy();
    let marked = bed.run(
        "mark",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
        ],
    );
    let valid = receipt(&marked)["marks"]["marks"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    let missing = "0000000000000000000000000000000000000000000000000000000000000000";
    let output = bed.run(
        "remove-marked",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
            "--mark",
            missing,
            "--mark",
            &valid,
        ],
    );
    assert_eq!(output.status.code(), Some(3));
    assert!(
        receipt(&output)["error"]
            .as_str()
            .unwrap()
            .contains(missing)
    );
    assert!(segment.join("audio.flac").exists());
}

#[test]
fn remove_marked_reports_current_policy_and_processing_proof_refusals() {
    let policy = armed_policy();
    let bed = Bed::new("policy-refusal");
    bed.proven_segment(
        "20260701",
        "field.audio",
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    let marked = bed.run(
        "mark",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
        ],
    );
    let id = receipt(&marked)["marks"]["marks"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    let disabled =
        json!({"default_rule":{"anchor":"captured","period":1,"priority":0},"enabled":false})
            .to_string();
    let policy_refused = bed.run(
        "remove-marked",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &disabled,
            "--mark",
            &id,
        ],
    );
    assert_eq!(policy_refused.status.code(), Some(3));
    assert_eq!(
        receipt(&policy_refused)["outcome"]["targets"][0]["not_removed"][0]["reason"],
        "this one is kept indefinitely."
    );

    let too_young =
        json!({"default_rule":{"anchor":"captured","period":90,"priority":0},"enabled":true})
            .to_string();
    let too_young_refused = bed.run(
        "remove-marked",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &too_young,
            "--mark",
            &id,
        ],
    );
    assert_eq!(too_young_refused.status.code(), Some(3));
    assert_eq!(
        receipt(&too_young_refused)["outcome"]["targets"][0]["not_removed"][0]["reason"],
        "this one isn't old enough to delete yet."
    );

    let proof_bed = Bed::new("proof-refusal");
    let segment = proof_bed.proven_segment(
        "20260701",
        "field.audio",
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    let marked = proof_bed.run(
        "mark",
        &[
            "--journal",
            proof_bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
        ],
    );
    let id = receipt(&marked)["marks"]["marks"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    fs::write(segment.join("audio.jsonl"), "{}\n").unwrap();
    let proof_refused = proof_bed.run(
        "remove-marked",
        &[
            "--journal",
            proof_bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
            "--mark",
            &id,
        ],
    );
    assert_eq!(proof_refused.status.code(), Some(3));
    assert!(
        receipt(&proof_refused)["outcome"]["targets"][0]["not_removed"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("audio.flac")
    );

    let missing_anchor_bed = Bed::new("anchor-missing");
    let missing_target = Target {
        day: "not-a-day".to_owned(),
        ..target()
    };
    missing_anchor_bed.proven_segment(
        &missing_target.day,
        &missing_target.stream,
        &missing_target.dir,
        "2026-07-01T00:00:00Z",
    );
    let register = reconcile(
        missing_anchor_bed.journal(),
        RemovalClass::PolicyRawRelease,
        &[(missing_target, proposal(vec!["audio.flac".to_owned()]))],
        mark_at("first"),
    )
    .unwrap();
    let missing_id = register.marks.keys().next().unwrap().as_str().to_owned();
    let missing_anchor = missing_anchor_bed.run(
        "remove-marked",
        &[
            "--journal",
            missing_anchor_bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
            "--mark",
            &missing_id,
        ],
    );
    assert_eq!(missing_anchor.status.code(), Some(3));
    assert_eq!(
        receipt(&missing_anchor)["outcome"]["targets"][0]["not_removed"][0]["reason"],
        "there's no date on this one, so it can't be deleted."
    );
}

#[test]
fn remove_marked_reports_freshly_proven_files_outside_the_approval() {
    let bed = Bed::new("not-approved");
    let segment = bed.proven_segment(
        "20260701",
        "field.audio",
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    bed.add_proven_raw(&segment, "070000_17", "other.wav", "2026-07-01T00:00:00Z");
    let register = reconcile(
        bed.journal(),
        RemovalClass::PolicyRawRelease,
        &[(
            Target {
                day: "20260701".to_owned(),
                stream: "field.audio".to_owned(),
                dir: "070000_17".to_owned(),
            },
            Proposal {
                bytes: b"the owner's recording".len() as u64,
                reason: "approval for audio.flac".to_owned(),
                names: vec!["audio.flac".to_owned()],
            },
        )],
        mark_at("2026-08-06T00:00:00Z"),
    )
    .unwrap();
    let id = register.marks.keys().next().unwrap().as_str().to_owned();
    let policy = armed_policy();
    let output = bed.run(
        "remove-marked",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
            "--mark",
            &id,
        ],
    );
    let body = receipt(&output);
    assert_eq!(output.status.code(), Some(3));
    assert!(!segment.join("audio.flac").exists());
    assert!(segment.join("other.wav").exists());
    assert_eq!(
        body["outcome"]["targets"][0]["removed"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        body["outcome"]["targets"][0]["not_removed"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        body["outcome"]["targets"][0]["not_removed"]
            .as_array()
            .unwrap()
            .iter()
                .any(|item| item["reason"]
                .as_str()
                .unwrap()
                == "this file was proven releasable but is not on the removal list, so it is left in place")
    );
}

#[test]
fn remove_marked_accounts_for_a_proposal_file_that_is_already_gone() {
    let bed = Bed::new("gone-approved");
    let segment = bed.proven_segment(
        "20260701",
        "field.audio",
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    bed.add_proven_raw(&segment, "070000_17", "other.wav", "2026-07-01T00:00:00Z");
    let register = reconcile(
        bed.journal(),
        RemovalClass::PolicyRawRelease,
        &[(
            target(),
            proposal(vec!["audio.flac".to_owned(), "other.wav".to_owned()]),
        )],
        mark_at("first"),
    )
    .unwrap();
    let id = register.marks.keys().next().unwrap().as_str().to_owned();
    fs::remove_file(segment.join("other.wav")).unwrap();
    let policy = armed_policy();
    let output = bed.run(
        "remove-marked",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
            "--mark",
            &id,
        ],
    );
    let body = receipt(&output);
    assert_eq!(output.status.code(), Some(3));
    assert!(!segment.join("audio.flac").exists());
    assert!(
        body["outcome"]["targets"][0]["not_removed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["reason"]
                == "this file was on the removal list but is no longer present")
    );
    assert_eq!(load(bed.journal()).unwrap().marks, Default::default());
}

#[test]
fn decline_drops_only_the_named_offload_mark_without_touching_media() {
    let bed = Bed::new("decline");
    let policy_segment = bed.proven_segment(
        "20260701",
        "field.audio",
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    let offload_target = Target {
        day: "20260702".to_owned(),
        ..target()
    };
    let offload_segment = bed.proven_segment(
        &offload_target.day,
        &offload_target.stream,
        &offload_target.dir,
        "2026-07-01T00:00:00Z",
    );
    let policy_proposal = proposal(vec!["audio.flac".to_owned()]);
    let offload_proposal = proposal(vec!["audio.flac".to_owned()]);
    reconcile(
        bed.journal(),
        RemovalClass::PolicyRawRelease,
        &[(target(), policy_proposal.clone())],
        mark_at("first"),
    )
    .unwrap();
    reconcile(
        bed.journal(),
        RemovalClass::OffloadRawRelease,
        &[(offload_target.clone(), offload_proposal.clone())],
        mark_at("first"),
    )
    .unwrap();
    let policy_id = MarkId::derive(
        RemovalClass::PolicyRawRelease,
        &target(),
        &policy_proposal.names,
    );
    let offload_id = MarkId::derive(
        RemovalClass::OffloadRawRelease,
        &offload_target,
        &offload_proposal.names,
    );
    let policy_bytes = fs::read(policy_segment.join("audio.flac")).unwrap();
    let offload_bytes = fs::read(offload_segment.join("audio.flac")).unwrap();
    let output = bed.run(
        "decline",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--mark",
            offload_id.as_str(),
        ],
    );
    let body = receipt(&output);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(keys(&body), BTreeSet::from(["marks", "ok", "verb"]));
    assert!(body["marks"]["marks"].get(policy_id.as_str()).is_some());
    assert!(body["marks"]["marks"].get(offload_id.as_str()).is_none());
    assert_eq!(
        fs::read(policy_segment.join("audio.flac")).unwrap(),
        policy_bytes
    );
    assert_eq!(
        fs::read(offload_segment.join("audio.flac")).unwrap(),
        offload_bytes
    );
}

#[test]
fn decline_refusals_leave_the_register_unchanged() {
    let missing = Bed::new("decline-missing");
    let missing_register = reconcile(
        missing.journal(),
        RemovalClass::PolicyRawRelease,
        &[(target(), proposal(vec!["audio.flac".to_owned()]))],
        mark_at("first"),
    )
    .unwrap();
    let missing_before = register_json(&missing);
    let absent = "0000000000000000000000000000000000000000000000000000000000000000";
    let missing_output = missing.run(
        "decline",
        &[
            "--journal",
            missing.journal().to_str().unwrap(),
            "--mark",
            absent,
        ],
    );
    assert_eq!(missing_output.status.code(), Some(3));
    assert_eq!(
        keys(&receipt(&missing_output)),
        BTreeSet::from(["error", "ok", "verb"])
    );
    assert_eq!(register_json(&missing), missing_before);

    let not_required = Bed::new("decline-not-required");
    let not_required_register = reconcile(
        not_required.journal(),
        RemovalClass::OwnerRawRelease,
        &[(target(), proposal(vec!["audio.flac".to_owned()]))],
        mark_at("first"),
    )
    .unwrap();
    let not_required_id = not_required_register
        .marks
        .keys()
        .next()
        .unwrap()
        .as_str()
        .to_owned();
    let not_required_before = register_json(&not_required);
    let not_required_output = not_required.run(
        "decline",
        &[
            "--journal",
            not_required.journal().to_str().unwrap(),
            "--mark",
            &not_required_id,
        ],
    );
    assert_eq!(not_required_output.status.code(), Some(3));
    assert_eq!(
        keys(&receipt(&not_required_output)),
        BTreeSet::from(["error", "ok", "verb"])
    );
    assert_eq!(register_json(&not_required), not_required_before);

    let failed = Bed::new("decline-failed");
    let failed_register = record_failure(
        failed.journal(),
        RemovalClass::PolicyRawRelease,
        &target(),
        &["audio.flac".to_owned()],
        Failure {
            at: "first".to_owned(),
            reason: "needs recovery".to_owned(),
            staged: Some("set-aside/segment".to_owned()),
        },
        mark_at("first"),
    )
    .unwrap();
    let failed_id = failed_register
        .marks
        .keys()
        .next()
        .unwrap()
        .as_str()
        .to_owned();
    let failed_before = register_json(&failed);
    let failed_output = failed.run(
        "decline",
        &[
            "--journal",
            failed.journal().to_str().unwrap(),
            "--mark",
            &failed_id,
        ],
    );
    assert_eq!(failed_output.status.code(), Some(3));
    assert_eq!(
        keys(&receipt(&failed_output)),
        BTreeSet::from(["error", "ok", "verb"])
    );
    assert_eq!(register_json(&failed), failed_before);

    let duplicate = Bed::new("decline-duplicate");
    let duplicate_register = reconcile(
        duplicate.journal(),
        RemovalClass::PolicyRawRelease,
        &[(target(), proposal(vec!["audio.flac".to_owned()]))],
        mark_at("first"),
    )
    .unwrap();
    let duplicate_id = duplicate_register
        .marks
        .keys()
        .next()
        .unwrap()
        .as_str()
        .to_owned();
    let duplicate_before = register_json(&duplicate);
    let duplicate_output = duplicate.run(
        "decline",
        &[
            "--journal",
            duplicate.journal().to_str().unwrap(),
            "--mark",
            &duplicate_id,
            "--mark",
            &duplicate_id,
        ],
    );
    assert_eq!(duplicate_output.status.code(), Some(2));
    assert_eq!(
        keys(&receipt(&duplicate_output)),
        BTreeSet::from(["error", "ok", "verb"])
    );
    assert_eq!(register_json(&duplicate), duplicate_before);
    assert_eq!(missing_register.marks.len(), 1);
}

#[test]
fn mark_does_not_change_marks_owned_by_other_classes() {
    let bed = Bed::new("other-classes");
    fs::create_dir(bed.journal().join("chronicle")).unwrap();
    reconcile(
        bed.journal(),
        RemovalClass::OffloadRawRelease,
        &[(target(), proposal(vec!["audio.flac".to_owned()]))],
        mark_at("first"),
    )
    .unwrap();
    let failed_target = Target {
        day: "20260702".to_owned(),
        ..target()
    };
    record_failure(
        bed.journal(),
        RemovalClass::OwnerSegmentRemoval,
        &failed_target,
        &[],
        Failure {
            at: "first".to_owned(),
            reason: "staged".to_owned(),
            staged: Some("set-aside".to_owned()),
        },
        mark_at("first"),
    )
    .unwrap();
    let before = serde_json::to_value(load(bed.journal()).unwrap()).unwrap();
    let output = bed.run(
        "mark",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
        ],
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        serde_json::to_value(load(bed.journal()).unwrap()).unwrap(),
        before
    );
}

#[test]
fn marks_refuses_unsupported_and_unknown_register_shapes() {
    for (name, contents, expected) in [
        ("version", r#"{"version":2,"marks":{}}"#, "version"),
        (
            "unknown",
            r#"{"version":1,"marks":{},"extra":true}"#,
            "valid JSON",
        ),
    ] {
        let bed = Bed::new(name);
        let register = bed.journal().join("health/retention-marks.json");
        fs::create_dir_all(register.parent().unwrap()).unwrap();
        fs::write(register, contents).unwrap();
        let output = bed.run("marks", &["--journal", bed.journal().to_str().unwrap()]);
        assert_eq!(output.status.code(), Some(3));
        assert!(
            receipt(&output)["error"]
                .as_str()
                .unwrap()
                .contains(expected)
        );
    }
}

#[test]
fn sweep_execute_still_releases_an_armed_proven_segment() {
    let bed = Bed::new("sweep-smoke");
    let segment = bed.proven_segment(
        "20260701",
        "field.audio",
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    let policy = armed_policy();
    let output = bed.run(
        "sweep",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
            "--force",
            "true",
        ],
    );
    let body = receipt(&output);
    assert_eq!(output.status.code(), Some(0));
    assert!(!segment.join("audio.flac").exists());
    assert_eq!(body["detail"]["executed"], true);
    assert!(body.get("outcome").is_some());
}

#[test]
fn sweep_without_force_plans_only() {
    let bed = Bed::new("sweep-plan-only");
    let segment = bed.proven_segment(
        "20260701",
        "field.audio",
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    let policy = armed_policy();
    let output = bed.run(
        "sweep",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
        ],
    );
    let body = receipt(&output);
    assert_eq!(output.status.code(), Some(0));
    assert!(segment.join("audio.flac").exists());
    assert_eq!(body["executed"], false);
    assert_eq!(
        keys(&body),
        BTreeSet::from(["executed", "ok", "plan", "verb"])
    );
    assert_eq!(body["ok"], true);
    assert_eq!(body["verb"], "sweep");
}

#[test]
fn sweep_refuses_execute() {
    for (name, value, with_force) in [
        ("true", "true", false),
        ("1", "1", false),
        ("yes", "yes", false),
        ("TRUE", "TRUE", false),
        ("true-with-force", "true", true),
    ] {
        let bed = Bed::new(name);
        let segment = bed.proven_segment(
            "20260701",
            "field.audio",
            "070000_17",
            "2026-07-01T00:00:00Z",
        );
        let policy = armed_policy();
        let journal = bed.journal().to_str().unwrap();
        let mut args = vec![
            "--journal",
            journal,
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            &policy,
            "--execute",
            value,
        ];
        if with_force {
            args.extend(["--force", "true"]);
        }
        let output = bed.run("sweep", &args);
        let body = receipt(&output);
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(body["ok"], false);
        assert!(body["error"].as_str().unwrap().contains("--force"));
        assert!(segment.join("audio.flac").exists());
    }
}

#[test]
fn sweep_force_false_and_zero_plan_only() {
    for (name, value) in [("false", "false"), ("zero", "0")] {
        let bed = Bed::new(name);
        let segment = bed.proven_segment(
            "20260701",
            "field.audio",
            "070000_17",
            "2026-07-01T00:00:00Z",
        );
        let policy = armed_policy();
        let output = bed.run(
            "sweep",
            &[
                "--journal",
                bed.journal().to_str().unwrap(),
                "--today",
                "2026-08-06",
                "--now",
                "2026-08-06T00:00:00Z",
                "--policy",
                &policy,
                "--force",
                value,
            ],
        );
        let body = receipt(&output);
        assert_eq!(output.status.code(), Some(0));
        assert!(segment.join("audio.flac").exists());
        assert_ne!(body["detail"]["executed"], true);
        assert_eq!(body["executed"], false);
    }
}

#[test]
fn sweep_help_names_force_and_consent() {
    let bed = Bed::new("sweep-help");
    let output = bed.run("--help", &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout
            .contains("--force executes the sweep. Use it only with the owner's express consent.")
    );
    let sweep_line = stdout
        .lines()
        .find(|line| line.contains("sweep") && line.contains("--journal P"))
        .expect("sweep usage line");
    assert!(sweep_line.contains("--force"));
    assert!(!sweep_line.contains("--execute"));
}

#[test]
fn staged_segment_refusal_is_registered_and_recovery_reconciles_it() {
    let bed = Bed::new("segment-register");
    let segment = bed.proven_segment(
        "20260701",
        "field.audio",
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    let staged = segment.parent().unwrap().join(".removing_070000_17");
    fs::create_dir(&staged).unwrap();
    let refused = bed.run(
        "remove-segments",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--at",
            "2026-08-06T00:00:00Z",
            "--did",
            "owner",
            "--segment",
            "20260701/field.audio/070000_17",
        ],
    );
    assert_eq!(refused.status.code(), Some(3));
    let listed = bed.run("marks", &["--journal", bed.journal().to_str().unwrap()]);
    let marks = receipt(&listed)["marks"]["marks"]
        .as_object()
        .unwrap()
        .clone();
    assert_eq!(marks.len(), 1);
    let failed = marks.values().next().unwrap();
    assert_eq!(failed["class"], "owner_segment_removal");
    assert_eq!(
        failed["state"]["failed"]["staged"],
        "chronicle/20260701/field.audio/.removing_070000_17"
    );

    fs::remove_dir(&staged).unwrap();
    let recovered = bed.run(
        "recover",
        &[
            "--journal",
            bed.journal().to_str().unwrap(),
            "--at",
            "2026-08-06T00:01:00Z",
            "--did",
            "owner",
        ],
    );
    assert_eq!(recovered.status.code(), Some(0));
    let after = bed.run("marks", &["--journal", bed.journal().to_str().unwrap()]);
    assert_eq!(receipt(&after)["marks"]["marks"], json!({}));
}

#[test]
fn every_new_verb_and_unknown_verb_returns_json_with_its_identifier() {
    let bed = Bed::new("receipt-shapes");
    fs::create_dir(bed.journal().join("chronicle")).unwrap();
    let journal = bed.journal().to_str().unwrap();
    let mark = bed.run(
        "mark",
        &[
            "--journal",
            journal,
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
        ],
    );
    let mark_body = receipt(&mark);
    assert_eq!(mark_body["verb"], "mark");
    assert_eq!(
        keys(&mark_body),
        BTreeSet::from(["marks", "ok", "plan", "verb"])
    );
    let marks = bed.run("marks", &["--journal", journal]);
    let marks_body = receipt(&marks);
    assert_eq!(marks_body["verb"], "marks");
    assert_eq!(keys(&marks_body), BTreeSet::from(["marks", "ok", "verb"]));
    let offload = bed.run(
        "mark-offload",
        &[
            "--journal",
            journal,
            "--day",
            "20260806",
            "--dir",
            "070000_17",
            "--file",
            "missing.flac",
            "--reason",
            "archive://audio",
            "--now",
            "2026-08-06T00:00:00Z",
        ],
    );
    let offload_body = receipt(&offload);
    assert_eq!(offload.status.code(), Some(3));
    assert_eq!(offload_body["verb"], "mark-offload");
    assert_eq!(keys(&offload_body), BTreeSet::from(["error", "ok", "verb"]));
    let remove_marked = bed.run(
        "remove-marked",
        &[
            "--journal",
            journal,
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--mark",
            "0000000000000000000000000000000000000000000000000000000000000000",
        ],
    );
    let remove_marked_body = receipt(&remove_marked);
    assert_eq!(remove_marked.status.code(), Some(3));
    assert_eq!(remove_marked_body["verb"], "remove-marked");
    assert_eq!(
        keys(&remove_marked_body),
        BTreeSet::from(["error", "ok", "verb"])
    );
    let unknown = bed.run("unknown", &[]);
    assert_eq!(unknown.status.code(), Some(2));
    assert!(
        receipt(&unknown)["error"]
            .as_str()
            .unwrap()
            .contains("unknown verb")
    );
}

#[test]
fn rfc3339_flag_refusals_leave_the_register_unchanged() {
    fn refuse(name: &str, verb: &str, extra: &[&str], needle: &str, expected_verb: Option<&str>) {
        let bed = Bed::new(name);
        reconcile(
            bed.journal(),
            RemovalClass::PolicyRawRelease,
            &[(target(), proposal(vec!["audio.flac".to_owned()]))],
            mark_at("first"),
        )
        .unwrap();
        let journal = bed.journal().to_str().unwrap().to_owned();
        let mut args = vec!["--journal", journal.as_str()];
        args.extend_from_slice(extra);
        let before = register_json(&bed);
        let output = bed.run(verb, &args);
        let body = receipt(&output);
        assert_eq!(output.status.code(), Some(2), "{name}: {body}");
        assert_eq!(body["ok"], false, "{name}");
        assert!(
            body["error"].as_str().unwrap().contains(needle),
            "{name}: {}",
            body["error"]
        );
        if let Some(expected_verb) = expected_verb {
            assert_eq!(body["verb"], expected_verb, "{name}");
        }
        assert_eq!(register_json(&bed), before, "{name}");
    }

    refuse(
        "mark-now-missing",
        "mark",
        &["--today", "2026-08-06"],
        "--now is required",
        Some("mark"),
    );
    refuse(
        "mark-now-bad",
        "mark",
        &["--today", "2026-08-06", "--now", "not-an-instant"],
        "--now must be an RFC 3339 instant, not `not-an-instant`",
        Some("mark"),
    );
    refuse(
        "mark-offload-now-missing",
        "mark-offload",
        &[
            "--day",
            "20260701",
            "--dir",
            "070000_17",
            "--file",
            "audio.flac",
            "--reason",
            "archive://audio",
        ],
        "--now is required",
        Some("mark-offload"),
    );
    refuse(
        "mark-offload-now-bad",
        "mark-offload",
        &[
            "--day",
            "20260701",
            "--dir",
            "070000_17",
            "--file",
            "audio.flac",
            "--reason",
            "archive://audio",
            "--now",
            "not-an-instant",
        ],
        "--now must be an RFC 3339 instant, not `not-an-instant`",
        Some("mark-offload"),
    );
    refuse(
        "recover-at-missing",
        "recover",
        &["--did", "owner"],
        "--at is required",
        None,
    );
    refuse(
        "recover-at-bad",
        "recover",
        &["--at", "not-an-instant", "--did", "owner"],
        "--at must be an RFC 3339 instant, not `not-an-instant`",
        None,
    );
}
