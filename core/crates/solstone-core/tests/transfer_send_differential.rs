// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[path = "support/stub_peer.rs"]
mod stub_peer;

use std::path::Path;
use std::process::{Command, Output};

use sha2::{Digest, Sha256};
use stub_peer::{CapturedRequest, Fixture, PeerPlan, ResponseAction, StubPeer};

fn fixture(peer: &StubPeer) -> Fixture {
    let fixture = peer.fixture();
    fixture.add_segment(
        "audio",
        "120000_30",
        &[("stream.json", b"control"), ("payload.json", b"payload")],
    );
    fixture
}

fn native(fixture: &Fixture, dry_run: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
    command.args(["transfer", "send", "--to", "office", "--day", "20260203"]);
    if dry_run {
        command.arg("--dry-run");
    }
    command
        .arg("--journal")
        .arg(fixture.path())
        .output()
        .expect("run native transfer send")
}

fn python(fixture: &Fixture, dry_run: bool) -> Output {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root");
    let script = if dry_run {
        "import solstone.observe.transfer as t; t.relay_url=lambda:None; p=t.resolve_peer('office'); t.send_segments_pl(p,['20260203'],True)"
    } else {
        "import solstone.observe.transfer as t; t.relay_url=lambda:None; p=t.resolve_peer('office'); t.send_segments_pl(p,['20260203'],False)"
    };
    Command::new(repository.join(".venv/bin/python"))
        .arg("-c")
        .arg(script)
        .env("PYTHONPATH", repository)
        .env("SOLSTONE_JOURNAL", fixture.path())
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .output()
        .expect("run Python transfer send")
}

fn assert_same_child_result(native: &Output, python: &Output) {
    assert_eq!(
        native.status.code(),
        python.status.code(),
        "native stdout: {}\nnative stderr: {}\nPython stdout: {}\nPython stderr: {}",
        String::from_utf8_lossy(&native.stdout),
        String::from_utf8_lossy(&native.stderr),
        String::from_utf8_lossy(&python.stdout),
        String::from_utf8_lossy(&python.stderr),
    );
    assert_eq!(
        native.stdout,
        python.stdout,
        "native stderr: {}\nPython stderr: {}",
        String::from_utf8_lossy(&native.stderr),
        String::from_utf8_lossy(&python.stderr),
    );
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );
    assert!(
        python.status.success(),
        "Python stderr: {}",
        String::from_utf8_lossy(&python.stderr)
    );
}

fn assert_equivalent_requests(left: &[CapturedRequest], right: &[CapturedRequest]) {
    assert_eq!(left.len(), right.len());
    for (left, right) in left.iter().zip(right) {
        assert_eq!(left.method, right.method);
        assert_eq!(left.path, right.path);
        assert_eq!(left.body.is_empty(), right.body.is_empty());
    }
}

#[test]
fn native_and_python_send_match_normal_dry_run_and_skip() {
    for (dry_run, manifest) in [
        (false, ResponseAction::manifest_empty()),
        (true, ResponseAction::manifest_empty()),
        (
            false,
            ResponseAction::status(
                200,
                format!(
                    r#"{{"20260203":{{"audio/120000_30":{{"files":[{{"name":"payload.json","sha256":"{:x}","size":7}}]}}}}}}"#,
                    Sha256::digest(b"payload")
                ),
            ),
        ),
    ] {
        let peer = StubPeer::new(PeerPlan::new(
            vec![manifest.clone(), manifest],
            vec![
                ResponseAction::status(200, Vec::new()),
                ResponseAction::status(200, Vec::new()),
            ],
        ));
        let fixture = fixture(&peer);
        let native = native(&fixture, dry_run);
        let native_requests = peer.requests();
        let python = python(&fixture, dry_run);
        let all_requests = peer.requests();
        let python_requests = &all_requests[native_requests.len()..];
        assert_same_child_result(&native, &python);
        assert_equivalent_requests(&native_requests, python_requests);
    }
}
