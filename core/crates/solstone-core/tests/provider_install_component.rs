// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use solstone_core_local::install::{lease, status};

const HOLDER: &str = env!("CARGO_BIN_EXE_solstone-core-install-lease-holder");
const CORE: &str = env!("CARGO_BIN_EXE_solstone-core");
const EXPECTED_WESPEAKER: &str = "5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94";

fn wait_for_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.is_file() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn stage_in_flight(journal: &Path, provider: &str, target_sha: &str) {
    let mut current = status::idle_status(provider);
    current.target_fingerprint_sha256 = Some(target_sha.to_owned());
    let current = status::transition(current, "resolving", None, None).unwrap();
    status::write_status(journal, current).unwrap();
}

fn hold_lease(journal: &Path, provider: &str) -> (std::process::Child, PathBuf, PathBuf) {
    let holding = journal.join("holding");
    let go = journal.join("go");
    let child = Command::new(HOLDER)
        .arg(journal)
        .arg(provider)
        .arg(&holding)
        .arg(&go)
        .spawn()
        .expect("spawn lease holder");
    wait_for_file(&holding);
    (child, holding, go)
}

fn observe_until_installed(journal: &Path, provider: &str, target_sha: &str, go: &Path) {
    let outcome = status::observe_attempt(
        journal,
        provider,
        target_sha,
        Duration::from_millis(10),
        Duration::from_secs(5),
        Duration::ZERO,
        |_| {
            let _ = fs::write(go, "go");
        },
    )
    .expect("observe attempt");
    match outcome {
        status::ObserveAttempt::Terminal(status) => {
            assert_eq!(status.install_state, "installed");
        }
        other => panic!("expected terminal installed, got {other:?}"),
    }
}

#[test]
fn real_asset_gate_reports_wespeaker_digest_mismatch_before_installing() {
    let journal = tempfile::tempdir().unwrap();
    let assets = tempfile::tempdir().unwrap();
    fs::write(
        assets.path().join("wespeaker-resnet34-256.onnx"),
        b"wrong digest",
    )
    .unwrap();
    let output = Command::new(CORE)
        .args([
            "install-models",
            "--variant",
            if cfg!(target_os = "macos") {
                "coreml"
            } else {
                "cpu"
            },
        ])
        .env("SOLSTONE_JOURNAL", journal.path())
        .env("SOLSTONE_TRANSCRIBE_MODEL_ASSETS_DIR", assets.path())
        .output()
        .expect("install-models");
    assert_eq!(output.status.code(), Some(65));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("wespeaker-resnet34-256.onnx"), "{stderr}");
    assert!(stderr.contains("has sha256"), "{stderr}");
    assert!(stderr.contains(EXPECTED_WESPEAKER), "{stderr}");
    assert!(
        journal.path().read_dir().unwrap().next().is_none(),
        "asset gate must not write the journal"
    );
}

#[test]
fn same_target_held_lease_is_observed_to_ready() {
    let journal = tempfile::tempdir().unwrap();
    let target_sha = "parakeet-observe-target";
    stage_in_flight(journal.path(), "parakeet", target_sha);
    let (mut holder, _, go) = hold_lease(journal.path(), "parakeet");
    assert!(
        lease::acquire(journal.path(), "parakeet")
            .unwrap()
            .is_none()
    );
    observe_until_installed(journal.path(), "parakeet", target_sha, &go);
    let status = holder.wait().expect("holder exits");
    assert!(status.success());
}

#[test]
fn local_preheld_lease_observes_the_existing_local_attempt() {
    let journal = tempfile::tempdir().unwrap();
    let target_sha = "local-observe-target";
    stage_in_flight(journal.path(), "local", target_sha);
    let (mut holder, _, go) = hold_lease(journal.path(), "local");
    assert!(lease::is_held(journal.path(), "local").unwrap());
    observe_until_installed(journal.path(), "local", target_sha, &go);
    let status = holder.wait().expect("holder exits");
    assert!(status.success());
}

#[test]
fn ac3_preheld_lease_does_not_mint_a_new_attempt() {
    let journal = tempfile::tempdir().unwrap();
    let target_sha = "parakeet-preheld-target";
    let mut current = status::idle_status("parakeet");
    current.target_fingerprint_sha256 = Some(target_sha.to_owned());
    let current = status::transition(current, "failed", Some("done".to_owned()), None).unwrap();
    let attempt_id = current.attempt_id.clone();
    status::write_status(journal.path(), current).unwrap();
    let (mut holder, _, _) = hold_lease(journal.path(), "parakeet");
    assert!(
        lease::acquire(journal.path(), "parakeet")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        status::read_status(journal.path(), "parakeet")
            .unwrap()
            .attempt_id,
        attempt_id
    );
    holder.kill().expect("stop holder");
    holder.wait().expect("holder exits");
    assert_eq!(
        status::read_status(journal.path(), "parakeet")
            .unwrap()
            .attempt_id,
        attempt_id
    );
}
