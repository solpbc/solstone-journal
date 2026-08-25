// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::import_ingest_door::door;
use crate::stub_peer;

use std::process::{Command, Output};

use sha2::{Digest, Sha256};
use stub_peer::{Fixture, PeerPlan, RequestRoute, ResponseAction, StubPeer};

fn plan(manifest: Vec<ResponseAction>, ingest: Vec<ResponseAction>) -> PeerPlan {
    PeerPlan::new([
        (
            RequestRoute::get(door("GET", "remote-i", "manifest", "segments")),
            manifest,
        ),
        (
            RequestRoute::post(door("POST", "remote-i", "ingest", "segments")),
            ingest,
        ),
    ])
}

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
        .args([
            "transfer", "send", "--to", "office", "--only", "segments", "--day", "20260203",
        ])
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
    let peer = StubPeer::new(plan(
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
        stdout(&output).contains("segments: 1 sent"),
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
    let peer = StubPeer::new(plan(
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
    assert!(stdout(&output).contains("segments: 1 sent, 1 skipped"));
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
fn manifest_failure_marks_the_segments_area_failed_without_uploading() {
    let peer = StubPeer::new(plan(
        vec![ResponseAction::status(500, Vec::new())],
        vec![ResponseAction::status(200, Vec::new())],
    ));
    let fixture = fixture(&peer);
    let output = run(&fixture, &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stdout(&output).contains("segments: FAILED (Manifest query failed: 500"),
        "{}",
        stdout(&output)
    );
    assert!(peer.ingest_requests().is_empty());
}

#[test]
fn upload_statuses_have_the_expected_retry_and_terminal_behavior() {
    for (action, uploads, expected, exit, stops_later_segments) in [
        (
            ResponseAction::status(200, Vec::new()),
            1,
            "segments: 1 sent",
            0,
            false,
        ),
        (
            ResponseAction::status(401, Vec::new()),
            1,
            "segments: FAILED (Authentication failed: invalid or missing API key)",
            1,
            true,
        ),
        (
            ResponseAction::status(403, Vec::new()),
            1,
            "segments: FAILED (Authentication failed: journal source revoked or disabled)",
            1,
            true,
        ),
        (
            ResponseAction::status(418, Vec::new()),
            1,
            "segments: 1 failed",
            1,
            false,
        ),
    ] {
        let peer = StubPeer::new(plan(vec![ResponseAction::manifest_empty()], vec![action]));
        let fixture = fixture(&peer);
        if stops_later_segments {
            fixture.add_segment("audio", "130000_30", &[("later.json", b"later")]);
        }
        let output = run(&fixture, &[]);
        assert_eq!(output.status.code(), Some(exit));
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

    let peer = StubPeer::new(plan(
        vec![ResponseAction::manifest_empty()],
        vec![
            ResponseAction::status(500, Vec::new()),
            ResponseAction::status(500, Vec::new()),
            ResponseAction::status(500, Vec::new()),
        ],
    ));
    let fixture = fixture(&peer);
    let output = run(&fixture, &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).contains("segments: 1 failed"));
    assert_eq!(peer.ingest_requests().len(), 3);
}

#[test]
fn connection_drops_retry_uploads() {
    let peer = StubPeer::new(plan(
        vec![ResponseAction::manifest_empty()],
        vec![
            ResponseAction::Drop,
            ResponseAction::Drop,
            ResponseAction::Drop,
        ],
    ));
    let fixture = fixture(&peer);
    let output = run(&fixture, &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).contains("segments: 1 failed"));
    assert_eq!(peer.ingest_requests().len(), 3);
}

#[test]
fn dry_run_only_queries_the_manifest() {
    let peer = StubPeer::new(plan(
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
    assert!(stdout(&output).contains("segments: 1 sent"));
    assert!(peer.ingest_requests().is_empty());
    assert_eq!(peer.requests().len(), 1);
}

#[test]
fn day_range_limits_transfer_send_to_the_selected_segments_area() {
    let peer = StubPeer::new(plan(
        vec![ResponseAction::manifest_empty()],
        vec![
            ResponseAction::status(200, Vec::new()),
            ResponseAction::status(200, Vec::new()),
        ],
    ));
    let fixture = fixture(&peer);
    fixture.add_segment_for_day(
        "20260204",
        "audio",
        "120100_30",
        &[("payload.json", b"next day")],
    );
    let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args([
            "transfer",
            "send",
            "--to",
            "office",
            "--only",
            "segments",
            "--day",
            "20260203-20260204",
        ])
        .arg("--journal")
        .arg(fixture.path())
        .output()
        .expect("run transfer send with range");
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        stdout(&output),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout(&output).contains("segments: 2 sent"));
    assert_eq!(peer.ingest_requests().len(), 2);
    assert!(peer.requests().iter().all(|request| {
        request.path == door("GET", "remote-i", "manifest", "segments")
            || request.path == door("POST", "remote-i", "ingest", "segments")
    }));
}

#[test]
fn matching_manifest_skips_the_segment_without_uploading() {
    let hash = format!("{:x}", Sha256::digest(b"payload"));
    let peer = StubPeer::new(plan(
        vec![ResponseAction::status(
            200,
            format!(
                r#"{{"20260203":{{"audio/120000_30":{{"files":[{{"name":"payload.json","sha256":"{hash}","size":7}}]}}}}}}"#
            ),
        )],
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
        stdout(&output).contains("segments: 1 skipped"),
        "stdout: {}\nstderr: {}",
        stdout(&output),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(peer.ingest_requests().is_empty());
}
