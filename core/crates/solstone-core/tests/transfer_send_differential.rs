// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[allow(dead_code)]
use crate::stub_peer;

use std::path::Path;
use std::process::{Command, Output};

use sha2::{Digest, Sha256};
use stub_peer::{CapturedRequest, Fixture, PeerPlan, RequestRoute, ResponseAction, StubPeer};

/// What the server actually registers, and what the native client now posts to.
const NATIVE_INGEST_PATH: &str = "/app/import/journal/remote-i/ingest/segments";
/// What the dead Python reference posts to. ⛔ Not a route the server has.
const LEGACY_PYTHON_INGEST_PATH: &str = "/app/import/journal/remote-i/ingest/segments/20260203";

fn plan(manifest: Vec<ResponseAction>, ingest: Vec<ResponseAction>) -> PeerPlan {
    PeerPlan::new([
        (
            RequestRoute::get("/app/import/journal/remote-i/manifest/segments"),
            manifest,
        ),
        (RequestRoute::post(NATIVE_INGEST_PATH), ingest.clone()),
        // ⚠ DECLARED DIVERGENCE, 2026-08-13. The Python reference posts a
        // day-suffixed path the server has NEVER registered, so every upload
        // through it answered 404. The native client was ported from it and
        // inherited the bug; this stub registered only the suffixed path, so the
        // fake matched the bug and no test could fail on it.
        // The native client is fixed. The Python is a dead reference retained
        // solely as this differential's oracle and is deliberately not changed,
        // per the standing rule that the conversion is Rust-only. This route
        // exists only so that oracle still runs; ⛔ it is not a server contract.
        (RequestRoute::post(LEGACY_PYTHON_INGEST_PATH), ingest),
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
    for (native, python) in left.iter().zip(right) {
        assert_eq!(native.method, python.method);
        assert_eq!(native.body.is_empty(), python.body.is_empty());
        if native.path == python.path {
            continue;
        }
        // The ONE declared divergence, keyed to these two exact paths rather than
        // to a shape, so any other path difference still fails.
        assert_eq!(
            (native.path.as_str(), python.path.as_str()),
            (NATIVE_INGEST_PATH, LEGACY_PYTHON_INGEST_PATH),
            "the only permitted path divergence is the segment-ingest door, where \
             the Python reference builds a day-suffixed path the server never \
             registered; anything else is an unintended divergence"
        );
    }
}

/// The native client posts to the path the server actually serves.
///
/// This is the assertion the parity check could not make: `assert_equivalent_requests`
/// compared native and Python paths to each other, so pinning them together pinned
/// the bug. Assert the boundary directly instead.
fn assert_native_uses_the_served_route(requests: &[CapturedRequest]) {
    let posts: Vec<&CapturedRequest> = requests
        .iter()
        .filter(|request| request.method == "POST")
        .collect();
    for request in &posts {
        assert_eq!(
            request.path, NATIVE_INGEST_PATH,
            "native segment upload must target the server's registered rule"
        );
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
        let peer = StubPeer::new(plan(
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
        assert_native_uses_the_served_route(&native_requests);
        assert_equivalent_requests(&native_requests, python_requests);
    }
}
