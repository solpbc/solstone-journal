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
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use solstone_core_retention::Target;
use solstone_core_retention::marks::{
    Failure, Proposal, RemovalClass, load, reconcile, record_failure,
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
        "20260701",
        "field.audio",
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
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
    let marks = first_body["marks"]["marks"].as_object().unwrap();
    let mark = marks.values().next().unwrap();
    let marked_at = mark["marked_at"].clone();
    assert_eq!(first.status.code(), Some(0));
    assert!(
        mark["proposal"]["reason"]
            .as_str()
            .unwrap()
            .contains("age_days")
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
    let listed = bed.run("marks", &["--journal", bed.journal().to_str().unwrap()]);
    assert_eq!(receipt(&listed)["marks"]["marks"], json!({}));
}

#[test]
fn remove_marked_refuses_not_required_and_failed_marks_before_touching_disk() {
    let bed = Bed::new("preflight");
    let owner = target();
    let owner_register = reconcile(
        bed.journal(),
        RemovalClass::OwnerSegmentRemoval,
        &[(owner, proposal(Vec::new()))],
        "first",
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
        "first",
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
    assert!(
        receipt(&policy_refused)["outcome"]["targets"][0]["not_removed"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("KeptForever")
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
        "2026-08-06T00:00:00Z",
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
    assert!(
        body["outcome"]["targets"][0]["not_removed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["reason"]
                .as_str()
                .unwrap()
                .contains("not named in this mark's proposal"))
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
        "first",
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
                == "this file was named in the proposal but is no longer present")
    );
    assert_eq!(load(bed.journal()).unwrap().marks, Default::default());
}

#[test]
fn mark_does_not_change_marks_owned_by_other_classes() {
    let bed = Bed::new("other-classes");
    fs::create_dir(bed.journal().join("chronicle")).unwrap();
    reconcile(
        bed.journal(),
        RemovalClass::OffloadRawRelease,
        &[(target(), proposal(vec!["audio.flac".to_owned()]))],
        "first",
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
        "first",
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
            "--execute",
            "true",
        ],
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(!segment.join("audio.flac").exists());
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
    assert_eq!(keys(&mark_body), BTreeSet::from(["marks", "ok", "verb"]));
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
