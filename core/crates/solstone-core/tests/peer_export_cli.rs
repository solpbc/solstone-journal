// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[path = "support/stub_peer.rs"]
mod stub_peer;

use std::process::{Command, Output};

use serde_json::{Value, json};
use stub_peer::{Fixture, PeerPlan, RequestRoute, ResponseAction, StubPeer};

fn run(fixture: &Fixture, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["export", "--to", "office"])
        .args(extra)
        .arg("--journal")
        .arg(fixture.path())
        .output()
        .expect("run journal export")
}

fn config_plan() -> PeerPlan {
    PeerPlan::new([
        (
            RequestRoute::get("/app/import/journal/remote-i/manifest/config"),
            vec![ResponseAction::manifest_empty()],
        ),
        (
            RequestRoute::post("/app/import/journal/remote-i/ingest/config"),
            vec![ResponseAction::status(
                200,
                json!({"staged": true}).to_string(),
            )],
        ),
    ])
}

#[test]
fn config_export_strips_secrets_on_the_wire() {
    let peer = StubPeer::new(config_plan());
    let fixture = peer.fixture();
    fixture.set_config(
        json!({"convey": {"password_hash": "never", "secret": "never", "other_field": "keep"}}),
    );
    // Exercise the realistic fixture builders used by the broader export suite.
    fixture.add_entity("kept", json!({"id": "kept"}));
    fixture.add_facet(
        "work",
        &[("facet.json", br#"{}"#), ("ignored.txt", b"ignored")],
    );
    fixture.add_import(
        "20260203_120000",
        json!({"source": "one"}),
        json!({"ok": true}),
        Some(&[json!({"item": 1})]),
    );
    let output = run(&fixture, &["--only", "config"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let uploads = peer.ingest_requests();
    assert_eq!(uploads.len(), 1);
    let body: Value = serde_json::from_slice(&uploads[0].body).expect("config JSON");
    assert_eq!(body["config"]["convey"]["other_field"], "keep");
    assert!(body.pointer("/config/convey/password_hash").is_none());
    assert!(body.pointer("/config/convey/secret").is_none());
}

#[test]
fn only_and_dry_run_limit_requests_and_never_prompt_on_non_tty() {
    let peer = StubPeer::new(PeerPlan::new([(
        RequestRoute::get("/app/import/journal/remote-i/manifest/segments"),
        vec![ResponseAction::manifest_empty()],
    )]));
    let fixture = peer.fixture();
    fixture.add_segment("audio", "120000_30", &[("payload.json", b"payload")]);
    let output = run(&fixture, &["--only", "segments", "--dry-run"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("--- Export Summary ---"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Unpair \"office\" now?"));
    assert!(peer.ingest_requests().is_empty());
    assert_eq!(peer.requests().len(), 1);
    assert!(peer.handshake_errors().is_empty());
    let _ = ResponseAction::Drop;
}

#[test]
fn four_area_only_never_prompts_or_unpairs_on_non_tty() {
    let peer = StubPeer::new(PeerPlan::new([
        (
            RequestRoute::get("/app/import/journal/remote-i/manifest/segments"),
            vec![ResponseAction::manifest_empty()],
        ),
        (
            RequestRoute::get("/app/import/journal/remote-i/manifest/imports"),
            vec![ResponseAction::manifest_empty()],
        ),
        (
            RequestRoute::get("/app/import/journal/remote-i/manifest/entities"),
            vec![ResponseAction::manifest_empty()],
        ),
        (
            RequestRoute::get("/app/import/journal/remote-i/manifest/facets"),
            vec![ResponseAction::manifest_empty()],
        ),
    ]));
    let fixture = peer.fixture();
    let output = run(&fixture, &["--only", "segments,imports,entities,facets"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Unpair"));
    assert!(
        peer.requests()
            .iter()
            .all(|request| request.path != "/app/network/unpair")
    );
}

#[test]
fn manifest_failure_marks_one_area_failed_and_continues_others() {
    let peer = StubPeer::new(PeerPlan::new([
        (
            RequestRoute::get("/app/import/journal/remote-i/manifest/segments"),
            vec![ResponseAction::status(500, b"unavailable")],
        ),
        (
            RequestRoute::get("/app/import/journal/remote-i/manifest/imports"),
            vec![ResponseAction::manifest_empty()],
        ),
        (
            RequestRoute::get("/app/import/journal/remote-i/manifest/entities"),
            vec![ResponseAction::manifest_empty()],
        ),
        (
            RequestRoute::get("/app/import/journal/remote-i/manifest/facets"),
            vec![ResponseAction::manifest_empty()],
        ),
    ]));
    let fixture = peer.fixture();
    let output = run(&fixture, &["--only", "segments,imports,entities,facets"]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("segments: FAILED (Manifest query failed: 500 unavailable)"),
        "{stdout}"
    );
    let paths = peer
        .requests()
        .into_iter()
        .map(|request| request.path)
        .collect::<Vec<_>>();
    assert!(paths.contains(&"/app/import/journal/remote-i/manifest/imports".to_string()));
    assert!(paths.contains(&"/app/import/journal/remote-i/manifest/entities".to_string()));
    assert!(paths.contains(&"/app/import/journal/remote-i/manifest/facets".to_string()));
}

#[test]
fn retired_options_and_bad_area_use_argparse_style_errors() {
    let journal = tempfile::tempdir().expect("journal");
    for (args, expected) in [
        (
            ["--to", "https://example.com", "--dry-run"].as_slice(),
            "Sending to a URL with a key is retired. Use '--to <label>' to send to a paired peer.",
        ),
        (
            ["--to", "office", "--key", "old"].as_slice(),
            "'--key' is retired; use '--to <label>' without '--key'",
        ),
        (
            ["--to", "office", "--only", "bogus"].as_slice(),
            "--only must contain one or more of: config, entities, facets, imports, segments",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
            .arg("export")
            .args(args)
            .arg("--journal")
            .arg(journal.path())
            .output()
            .expect("run export");
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(
            String::from_utf8_lossy(&output.stderr).trim(),
            format!("journal export: error: {expected}")
        );
    }
}
