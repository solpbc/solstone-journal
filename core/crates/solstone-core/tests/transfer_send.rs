// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[path = "support/stub_peer.rs"]
mod stub_peer;

use std::process::{Command, Output};

use stub_peer::{Fixture, PeerPlan, ResponseAction, StubPeer};

fn fixture(peer: &StubPeer) -> Fixture {
    let fixture = peer.fixture();
    fixture.add_segment(
        "audio",
        "120000_30",
        &[("stream.json", b"control"), ("payload.json", b"payload")],
    );
    fixture
}

fn run(fixture: &Fixture, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["transfer", "send", "--to", "office", "--day", "20260203"])
        .args(extra)
        .arg("--journal")
        .arg(fixture.path())
        .output()
        .expect("run solstone-core transfer send")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn live_binary_uploads_over_framed_mtls() {
    let peer = StubPeer::new(PeerPlan::new(
        vec![ResponseAction::manifest_empty()],
        vec![ResponseAction::status(200, Vec::new())],
    ));
    let fixture = fixture(&peer);
    let output = run(&fixture, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout(&output).contains("Transfer complete: 1 sent, 0 skipped, 0 failed"),
        "stdout: {}\nstderr: {}\nrequests: {:?}\nhandshake errors: {:?}",
        stdout(&output),
        String::from_utf8_lossy(&output.stderr),
        peer.requests(),
        peer.handshake_errors()
    );
    let uploads = peer.ingest_requests();
    assert_eq!(uploads.len(), 1);
    assert!(!uploads[0].body.is_empty());
    assert!(
        uploads[0]
            .body
            .windows(b"payload.json".len())
            .any(|part| part == b"payload.json")
    );
}

#[test]
fn reserved_files_are_not_uploaded_and_reserved_only_segments_skip() {
    let peer = StubPeer::new(PeerPlan::new(
        vec![ResponseAction::manifest_empty()],
        vec![ResponseAction::status(200, Vec::new())],
    ));
    let fixture = fixture(&peer);
    fixture.add_segment(
        "audio",
        "130000_30",
        &[("stream.json", b"control"), ("ingest.json", b"control")],
    );
    let output = run(&fixture, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout(&output).contains("1 sent, 1 skipped, 0 failed"));
    let uploads = peer.ingest_requests();
    assert_eq!(uploads.len(), 1);
    assert!(
        !uploads[0]
            .body
            .windows(b"stream.json".len())
            .any(|part| part == b"stream.json")
    );
    assert!(
        !uploads[0]
            .body
            .windows(b"ingest.json".len())
            .any(|part| part == b"ingest.json")
    );
}

#[test]
fn manifest_failure_degrades_to_empty_and_uploads() {
    let peer = StubPeer::new(PeerPlan::new(
        vec![ResponseAction::status(500, Vec::new())],
        vec![ResponseAction::status(200, Vec::new())],
    ));
    let fixture = fixture(&peer);
    let output = run(&fixture, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(peer.ingest_requests().len(), 1);
}

#[test]
fn upload_statuses_have_the_expected_retry_and_terminal_behavior() {
    for (action, uploads, expected, stops_later_segments) in [
        (
            ResponseAction::status(200, Vec::new()),
            1,
            "1 sent, 0 skipped, 0 failed",
            false,
        ),
        (
            ResponseAction::status(401, Vec::new()),
            1,
            "Authentication failed: invalid or missing paired-link identity",
            true,
        ),
        (
            ResponseAction::status(403, Vec::new()),
            1,
            "Authentication failed: paired-link identity revoked or disabled",
            true,
        ),
        (
            ResponseAction::status(418, Vec::new()),
            1,
            "0 sent, 0 skipped, 1 failed",
            false,
        ),
    ] {
        let peer = StubPeer::new(PeerPlan::new(
            vec![ResponseAction::manifest_empty()],
            vec![action],
        ));
        let fixture = fixture(&peer);
        if stops_later_segments {
            fixture.add_segment("audio", "130000_30", &[("later.json", b"later")]);
        }
        let output = run(&fixture, &[]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(stdout(&output).contains(expected), "{}", stdout(&output));
        let requests = peer.ingest_requests();
        assert_eq!(requests.len(), uploads);
        if stops_later_segments {
            assert!(
                !requests[0]
                    .body
                    .windows(b"later.json".len())
                    .any(|part| part == b"later.json")
            );
        }
    }

    let peer = StubPeer::new(PeerPlan::new(
        vec![ResponseAction::manifest_empty()],
        vec![
            ResponseAction::status(500, Vec::new()),
            ResponseAction::status(500, Vec::new()),
            ResponseAction::status(500, Vec::new()),
        ],
    ));
    let fixture = fixture(&peer);
    let output = run(&fixture, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout(&output).contains("0 sent, 0 skipped, 1 failed"));
    assert_eq!(peer.ingest_requests().len(), 3);
}

#[test]
fn connection_drops_retry_uploads() {
    let peer = StubPeer::new(PeerPlan::new(
        vec![ResponseAction::manifest_empty()],
        vec![
            ResponseAction::Drop,
            ResponseAction::Drop,
            ResponseAction::Drop,
        ],
    ));
    let fixture = fixture(&peer);
    let output = run(&fixture, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout(&output).contains("0 sent, 0 skipped, 1 failed"));
    assert_eq!(peer.ingest_requests().len(), 3);
}

#[test]
fn dry_run_only_queries_the_manifest() {
    let peer = StubPeer::new(PeerPlan::new(
        vec![ResponseAction::manifest_empty()],
        vec![ResponseAction::status(200, Vec::new())],
    ));
    let fixture = fixture(&peer);
    let output = run(&fixture, &["--dry-run"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout(&output).contains("Dry run: would send 1, skip 0"));
    assert!(peer.ingest_requests().is_empty());
    assert_eq!(peer.requests().len(), 1);
}
