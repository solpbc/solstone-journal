// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use serde::Deserialize;
use solstone_core_backup_web::archive_worker::removal::{ApprovalState, status};
use solstone_core_retention::{
    Target,
    archive_catalog::codec,
    archive_proof::{AcceptedProcessingObservation, ArchiveFileProof, OffloadOffer, OffloadProof},
    content::{ClosedHandlerSet, JournalMedia},
    marks::{Mark, Proposal, Register, RemovalClass},
};
use std::{
    fs,
    time::{Duration, Instant},
};

type PublicStartHook = Box<dyn FnOnce(&str) + Send>;
thread_local! {
    static PUBLIC_START: std::cell::RefCell<Option<PublicStartHook>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn take_public_start_hook() -> Option<PublicStartHook> {
    PUBLIC_START.with(|hook| hook.borrow_mut().take())
}

#[derive(Deserialize)]
struct Input {
    mode: String,
    journal: PathBuf,
    restic_path: PathBuf,
    repository_id: String,
    snapshot_id: String,
    archive_path: String,
    bytes: u64,
    sha256: String,
}

fn seed(input: &Input, root: &Arc<JournalRoot>) -> (Vec<Mark>, Vec<PathBuf>) {
    let mut rows = Vec::new();
    let mut paths = Vec::new();
    let mut ledger = Vec::new();
    for (ordinal, class) in [
        RemovalClass::OffloadRawRelease,
        RemovalClass::PolicyRawRelease,
    ]
    .into_iter()
    .enumerate()
    {
        let target = Target {
            day: "20261009".into(),
            stream: "_default".into(),
            dir: format!("{}0000_60_fixture", 12 + ordinal),
        };
        let relative =
            solstone_core_retention::layout::segment_rel(&target.day, &target.stream, &target.dir);
        let segment = input.journal.join(&relative);
        fs::create_dir_all(&segment).unwrap();
        let raw = segment.join("audio.flac");
        fs::copy(&input.archive_path, &raw).unwrap();
        fs::write(segment.join("audio.jsonl"), format!(
            "{{\"_solstone_processing\":{{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\",\"reason_code\":\"ok\",\"handler\":\"transcribe\",\"input_size\":{},\"attempted_at\":\"2026-10-09T12:00:00Z\"}}}}\n{{\"start\":0,\"text\":\"synthetic transcript\"}}\n", input.bytes)).unwrap();
        let directory = solstone_core_journal_io::media::MediaDirectory::open(
            Arc::clone(root),
            Path::new(&relative),
        )
        .unwrap();
        let scan = solstone_core_retention::scan::scan_segment_bound(
            &directory,
            &ClosedHandlerSet,
            &JournalMedia,
            65_536,
        )
        .unwrap();
        let file = &scan.files()[0];
        let processing = AcceptedProcessingObservation::capture(
            &ClosedHandlerSet,
            &target,
            &file.name,
            file.size,
            &file.sidecar,
        )
        .unwrap();
        let observed = directory
            .observe_file(std::ffi::OsStr::new("audio.flac"))
            .unwrap()
            .unwrap();
        assert_eq!(observed.digest.size, input.bytes);
        assert_eq!(observed.digest.sha256, input.sha256);
        let offload = if ordinal == 0 {
            let proof = OffloadProof {
                journal_id: root.identity().observation_id(),
                manifest_id: "synthetic-home-manifest".into(),
                target: target.clone(),
                layout: solstone_core_journal_io::paths::SegmentLayout::Direct,
                repository_id: input.repository_id.clone(),
                snapshot_id: input.snapshot_id.clone(),
                files: vec![ArchiveFileProof {
                    witness_id: "synthetic-home-witness".into(),
                    name: "audio.flac".into(),
                    bytes: input.bytes,
                    sha256: input.sha256.clone(),
                    archive_path: input.archive_path.clone(),
                    processing,
                }],
            };
            ledger = serde_json::to_vec(&codec::Event::ArchiveV2 {
                schema_version: 2,
                time: 1,
                day: target.day.clone(),
                stream: target.stream.clone(),
                segment: "120000_60".into(),
                segment_dir: target.dir.clone(),
                layout: proof.layout,
                journal_id: proof.journal_id.clone(),
                manifest_id: proof.manifest_id.clone(),
                repository_id: proof.repository_id.clone(),
                snapshot_id: proof.snapshot_id.clone(),
                files: proof.files.clone(),
            })
            .unwrap();
            ledger.push(b'\n');
            Some(OffloadOffer::new(proof).unwrap())
        } else {
            None
        };
        let names = vec!["audio.flac".into()];
        rows.push(Mark {
            id: MarkId::derive(class, &target, &names),
            class,
            target,
            marked_at: "2026-10-09T12:00:00Z".into(),
            proposal: Proposal {
                names,
                bytes: input.bytes,
                reason: "synthetic review".into(),
            },
            state: MarkState::Marked,
            offload,
        });
        paths.push(raw);
    }
    fs::create_dir_all(input.journal.join("health/offload")).unwrap();
    fs::create_dir_all(input.journal.join("chronicle/20261009/health")).unwrap();
    fs::write(input.journal.join("health/offload/20261009.jsonl"), ledger).unwrap();
    save(input, &rows);
    (rows, paths)
}

fn save(input: &Input, rows: &[Mark]) {
    let register = Register {
        version: 2,
        marks: rows
            .iter()
            .map(|row| (row.id.clone(), row.clone()))
            .collect(),
    };
    fs::write(
        input.journal.join("health/retention-marks.json"),
        serde_json::to_vec(&register).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
#[ignore = "requires copied package and synthetic Home reviewed-removal fixture"]
async fn packaged_home_approval_join() {
    let input: Input = serde_json::from_slice(
        &fs::read(std::env::var_os("OWNED_APPROVAL_JSON").unwrap()).unwrap(),
    )
    .unwrap();
    assert!(matches!(input.mode.as_str(), "typed" | "mixed" | "reoffer"));
    let root = Arc::new(JournalRoot::open(&input.journal).unwrap());
    let (mut rows, paths) = seed(&input, &root);
    if input.mode == "typed" {
        rows[1].state = MarkState::Failed(solstone_core_retention::marks::Failure {
            at: "now".into(),
            reason: "unselected".into(),
            staged: None,
        });
        save(&input, &rows);
        rows.truncate(1);
    }
    let context = read_review_context(
        &root,
        &rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>(),
    )
    .unwrap();
    let body = json!({"review_context":serde_json::to_string(&context).unwrap(), "mark_ids":rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>()}).to_string();
    if input.mode == "reoffer" {
        let old = rows[0].offload.as_ref().unwrap();
        let replacement = OffloadOffer::new(old.proof.clone()).unwrap();
        assert_ne!(old.nonce, replacement.nonce);
        assert_ne!(old.revision, replacement.revision);
        rows[0].offload = Some(replacement);
        save(&input, &rows);
    }
    let before = fs::read(input.journal.join("health/retention-marks.json")).unwrap();
    let originals = paths
        .iter()
        .map(fs::read)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut deps = solstone_core_backup_web::BackupWebDeps::new(input.journal.clone());
    deps.executable = std::env::current_exe().unwrap();
    assert_eq!(input.restic_path.file_name().unwrap(), "restic");
    let now = jiff::tz::Offset::UTC
        .to_timestamp(jiff::civil::date(2026, 10, 11).at(12, 0, 0, 0))
        .unwrap();
    // Missing Extension never constructs another slot or touches admission.
    assert!(dispatch_approval(None, Ok(Bytes::from(body.clone())), now).is_err());
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let router = crate::routes_with_backup_deps(deps.clone(), crate::Clock::fixed(now), |_| false);
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/home/api/approve")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let started: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    if input.mode == "reoffer" {
        assert_eq!(started["state"], "approve.refused_before_start");
        assert_eq!(
            fs::read(input.journal.join("health/retention-marks.json")).unwrap(),
            before
        );
        for (path, expected) in paths.iter().zip(originals) {
            assert_eq!(fs::read(path).unwrap(), expected);
        }
        assert!(!input.journal.join("backup-activity.lock").exists());
        println!(
            "OWNED_APPROVAL_RESULT {}",
            json!({"mode":input.mode,"stale_refused":true,"unchanged":true})
        );
        return;
    }
    assert_eq!(started["state"], "approve.started");
    let generation = started["generation"].as_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let observed = loop {
        let observed = status(&deps.operations, generation).unwrap();
        if observed.worker_settled {
            break observed;
        }
        assert!(
            Instant::now() < deadline,
            "actual Home worker did not settle"
        );
        std::thread::park_timeout(Duration::from_millis(10));
    };
    assert_eq!(
        observed.state,
        if input.mode == "typed" {
            ApprovalState::Completed
        } else {
            ApprovalState::Held
        }
    );
    assert!(!paths[0].exists());
    assert_eq!(fs::read(&paths[1]).unwrap(), originals[1]);
    let register = load_at_root(&root).unwrap();
    assert!(!register.marks.contains_key(&rows[0].id));
    if input.mode == "mixed" {
        assert_eq!(register.marks.get(&rows[1].id), Some(&rows[1]));
    }
    let report = observed.report.as_ref().unwrap();
    assert_eq!(
        report["outcome"]["targets"].as_array().unwrap().len(),
        rows.len()
    );
    assert_eq!(report["attempts"].as_array().unwrap().len(), 1);
    let received = router
        .oneshot(
            Request::builder()
                .uri(format!("/app/home/api/approve/{generation}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(received.status().is_success());
    let received: Value =
        serde_json::from_slice(&to_bytes(received.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(received["generation"], generation);
    assert_eq!(received["report"], *report);
    let state = deps.operations.lock().unwrap();
    if input.mode == "mixed" {
        let returned = state.archive_returns().next().unwrap();
        let solstone_core_backup_web::archive_worker::ArchivePayload::ProofHeld { proof, .. } =
            returned.payload()
        else {
            panic!("whole partial Proof owner");
        };
        assert_eq!(proof.selected_marks(), rows);
        drop(state);
        assert!(
            solstone_core_backup_web::archive_operation::begin(
                &deps.operations,
                "backup",
                deps.clock.as_ref()
            )
            .is_err()
        );
    } else {
        assert!(state.archive_returns().next().is_none());
        drop(state);
        assert!(
            solstone_core_backup_web::archive_operation::begin(
                &deps.operations,
                "backup",
                deps.clock.as_ref()
            )
            .is_ok()
        );
    }
    println!(
        "OWNED_APPROVAL_RESULT {}",
        json!({"mode":input.mode,"status":observed,"typed_removed":true,"policy_preserved":true})
    );
}

#[tokio::test]
#[ignore = "uses the existing copied installed package; lease refusal precedes helper or WAN work"]
async fn packaged_public_approval_post_start_unknown_retains_original() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use solstone_core_backup_web::{BackupWebDeps, archive_operation};
    use solstone_core_journal_io::activity::ActivityLease;
    use tower::ServiceExt;
    let journal = tempfile::tempdir().unwrap();
    fs::create_dir_all(journal.path().join("config")).unwrap();
    fs::write(
        journal.path().join("config/journal.json"),
        br#"{"setup":{"completed_at":1700000000000}}"#,
    )
    .unwrap();
    let source = journal.path().join("audio.flac");
    fs::write(&source, b"synthetic local original").unwrap();
    let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
    let source_observation =
        solstone_core_journal_io::media::MediaDirectory::open(Arc::clone(&root), Path::new(""))
            .unwrap()
            .observe_file(std::ffi::OsStr::new("audio.flac"))
            .unwrap()
            .unwrap();
    let input = Input {
        mode: "receiving".into(),
        journal: journal.path().to_path_buf(),
        restic_path: PathBuf::new(),
        repository_id: "a".repeat(64),
        snapshot_id: "b".repeat(64),
        archive_path: source.to_string_lossy().into_owned(),
        bytes: source_observation.digest.size,
        sha256: source_observation.digest.sha256,
    };
    let (rows, paths) = seed(&input, &root);
    let context = read_review_context(
        &root,
        &rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>(),
    )
    .unwrap();
    let body = json!({"review_context": serde_json::to_string(&context).unwrap(), "mark_ids": rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>()});
    let external_lease = ActivityLease::try_acquire(Arc::clone(&root)).unwrap();
    let mut deps = BackupWebDeps::new(journal.path().to_path_buf());
    deps.executable = PathBuf::from(
        std::env::var_os("HOME_APPROVAL_RECEIVING_PACKAGE")
            .expect("existing copied package member"),
    );
    let now = jiff::tz::Offset::UTC
        .to_timestamp(jiff::civil::date(2026, 10, 11).at(12, 0, 0, 0))
        .unwrap();
    let router = crate::routes_with_backup_deps(deps.clone(), crate::Clock::fixed(now), |_| false);
    let (sent, received) = std::sync::mpsc::channel();
    PUBLIC_START.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |generation| {
            sent.send(generation.to_owned()).unwrap();
            panic!("actual start received before initial HTTP reply");
        }))
    });
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/home/api/approve")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let envelope: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(envelope["state"], OUTCOME_UNKNOWN);
    let generation = received.recv().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let observed = loop {
        let observed = status(&deps.operations, &generation).unwrap();
        if observed.worker_settled {
            break observed;
        }
        assert!(
            Instant::now() < deadline,
            "real preparation refusal did not settle"
        );
        std::thread::park_timeout(Duration::from_millis(10));
    };
    assert_eq!(observed.state, ApprovalState::Held);
    assert_eq!(
        observed.report.as_ref().unwrap()["preparation"]["state"],
        "refused"
    );
    assert!(matches!(
        deps.operations
            .lock()
            .unwrap()
            .archive_returns()
            .next()
            .unwrap()
            .payload(),
        solstone_core_backup_web::archive_worker::ArchivePayload::PreparationFailure(_)
    ));
    drop(external_lease);
    let mut observations = Vec::new();
    for (method, tail, expected) in [
        ("GET", "", false),
        ("POST", "/cancel", true),
        ("POST", "/cancel", true),
        ("GET", "", true),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/app/home/api/approve/{}{tail}", generation))
                    .header("content-type", "application/json")
                    .body(Body::from(if method == "POST" { "{}" } else { "" }))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_success());
        assert_eq!(response.headers()["cache-control"], "no-store");
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(value["generation"], generation);
        assert_eq!(value["state"], "held");
        assert_eq!(value["cancellation_requested"], expected);
        assert_eq!(value["report"], observed.report.as_ref().unwrap().clone());
        assert!(archive_operation::begin(&deps.operations, "backup", deps.clock.as_ref()).is_err());
        observations.push(value);
    }
    for path in paths {
        assert_eq!(fs::read(path).unwrap(), b"synthetic local original");
    }
    assert!(paths_unchanged(&root, &rows));
    println!(
        "HOME_APPROVAL_RECEIVING {}",
        json!({"started":envelope, "observations":observations, "busy":true, "helper_wan":"not admitted; actual activity capture refused"})
    );
}

fn paths_unchanged(root: &Arc<JournalRoot>, rows: &[Mark]) -> bool {
    let register = load_at_root(root).unwrap();
    rows.iter()
        .all(|row| register.marks.get(&row.id) == Some(row))
}
