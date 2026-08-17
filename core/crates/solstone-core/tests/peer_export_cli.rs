// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::stub_peer;

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

fn all_areas_plan() -> PeerPlan {
    PeerPlan::new(
        ["segments", "imports", "entities", "facets", "config"]
            .into_iter()
            .flat_map(|area| {
                [
                    (
                        RequestRoute::get(format!("/app/import/journal/remote-i/manifest/{area}")),
                        vec![ResponseAction::manifest_empty()],
                    ),
                    (
                        RequestRoute::post(format!("/app/import/journal/remote-i/ingest/{area}")),
                        vec![ResponseAction::status(
                            200,
                            json!({"staged": true}).to_string(),
                        )],
                    ),
                ]
            }),
    )
}

#[test]
fn export_all_five_areas_in_one_run() {
    let peer = StubPeer::new(all_areas_plan());
    let fixture = peer.fixture();
    fixture.add_segment("audio", "120000_30", &[("stream.json", b"segment")]);
    fixture.add_entity("alice", json!({"id": "alice", "name": "Alice"}));
    fixture.add_facet("work", &[("facet.json", b"{\"name\":\"work\"}")]);
    fixture.add_import(
        "20260203_120000",
        json!({"source": "calendar"}),
        json!({"status": "imported"}),
        None,
    );
    fixture.set_config(json!({
        "convey": {
            "password_hash": "do-not-send",
            "secret": "also-do-not-send",
            "other_field": "send-this"
        }
    }));

    let output = run(&fixture, &[]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "\n--- Export Summary ---\n  segments: 1 skipped\n  imports: nothing to send\n  entities: nothing to send\n  facets: 1 sent\n  config: 1 staged\n"
    );

    let requests = peer.requests();
    assert_eq!(
        requests
            .iter()
            .map(|request| (
                request.method.as_str(),
                request.path.as_str(),
                request.body.is_empty()
            ))
            .collect::<Vec<_>>(),
        [
            (
                "GET",
                "/app/import/journal/remote-i/manifest/segments",
                true
            ),
            ("GET", "/app/import/journal/remote-i/manifest/imports", true),
            ("POST", "/app/import/journal/remote-i/ingest/imports", false),
            (
                "GET",
                "/app/import/journal/remote-i/manifest/entities",
                true
            ),
            (
                "POST",
                "/app/import/journal/remote-i/ingest/entities",
                false
            ),
            ("GET", "/app/import/journal/remote-i/manifest/facets", true),
            ("POST", "/app/import/journal/remote-i/ingest/facets", false),
            ("GET", "/app/import/journal/remote-i/manifest/config", true),
            ("POST", "/app/import/journal/remote-i/ingest/config", false),
        ]
    );

    let config = requests
        .iter()
        .find(|request| request.path.ends_with("/ingest/config"))
        .expect("config upload");
    let body: Value = serde_json::from_slice(&config.body).expect("config JSON");
    assert_eq!(body["config"]["convey"]["other_field"], "send-this");
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
fn persistent_entity_server_error_reports_retry_exhaustion() {
    let peer = StubPeer::new(PeerPlan::new([
        (
            RequestRoute::get("/app/import/journal/remote-i/manifest/entities"),
            vec![ResponseAction::manifest_empty()],
        ),
        (
            RequestRoute::post("/app/import/journal/remote-i/ingest/entities"),
            vec![
                ResponseAction::status(500, b"unavailable"),
                ResponseAction::status(500, b"unavailable"),
                ResponseAction::status(500, b"unavailable"),
            ],
        ),
    ]));
    let fixture = peer.fixture();
    fixture.add_entity("alice", json!({"id": "alice", "name": "Alice"}));

    let output = run(&fixture, &["--only", "entities"]);

    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("entities: FAILED (Entity upload failed after all retries)"),
        "{stdout}"
    );
    assert!(!stdout.contains("Entity upload failed: 500"), "{stdout}");
    assert_eq!(peer.ingest_requests().len(), 3);
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
