// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Peer-export behavior exposed through `journal transfer send`.

use crate::{import_ingest_door::door, stub_peer};

use std::process::{Command, Output};

use serde_json::{Value, json};
use stub_peer::{Fixture, PeerPlan, RequestRoute, ResponseAction, StubPeer};

fn run(fixture: &Fixture, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["transfer", "send", "--to", "office"])
        .args(extra)
        .arg("--journal")
        .arg(fixture.path())
        .output()
        .expect("run journal transfer send")
}

fn config_plan() -> PeerPlan {
    PeerPlan::new([
        (
            RequestRoute::get(door("GET", "remote-i", "manifest", "config")),
            vec![ResponseAction::manifest_empty()],
        ),
        (
            RequestRoute::post(door("POST", "remote-i", "ingest", "config")),
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
    fixture.set_config(json!({
        "env": {
            "OPENAI_API_KEY": "never",
            "ANTHROPIC_API_KEY": "never",
            "GOOGLE_API_KEY": "never",
            "PLAUD_ACCESS_TOKEN": "never",
            "other_field": "keep"
        },
        "convey": {
            "password_hash": "never",
            "secret": "never",
            "bind": "127.0.0.1",
            "other_field": "keep"
        },
        "backup": {
            "destination": {"credentials": {"token": "never"}, "url": "keep"},
            "daily_key": "never",
            "recovery_key": "never"
        },
        "voice": {"openai_api_key": "never", "provider": "keep"},
        "pairing": {"home_address": "never", "label": "keep"},
        "identity": {"name": "Keep"}
    }));

    let output = run(&fixture, &["--only", "config"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let uploads = peer.ingest_requests();
    assert_eq!(uploads.len(), 1, "config must be posted for inspection");
    let body: Value = serde_json::from_slice(&uploads[0].body).expect("config JSON");
    for path in [
        "/config/env/OPENAI_API_KEY",
        "/config/env/ANTHROPIC_API_KEY",
        "/config/env/GOOGLE_API_KEY",
        "/config/env/PLAUD_ACCESS_TOKEN",
        "/config/convey/password_hash",
        "/config/convey/secret",
        "/config/backup/destination/credentials",
        "/config/backup/daily_key",
        "/config/backup/recovery_key",
        "/config/voice/openai_api_key",
        "/config/pairing/home_address",
    ] {
        assert!(body.pointer(path).is_none(), "must omit {path}: {body}");
    }
    assert_eq!(
        body.pointer("/config/convey/bind"),
        Some(&json!("127.0.0.1"))
    );
    assert_eq!(body.pointer("/config/identity/name"), Some(&json!("Keep")));
}

fn all_areas_plan() -> PeerPlan {
    PeerPlan::new(
        ["segments", "imports", "entities", "facets", "config"]
            .into_iter()
            .flat_map(|area| {
                [
                    (
                        RequestRoute::get(door("GET", "remote-i", "manifest", area)),
                        vec![ResponseAction::manifest_empty()],
                    ),
                    (
                        RequestRoute::post(door("POST", "remote-i", "ingest", area)),
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
    fixture.add_segment("audio", "120000_30", &[("payload.json", b"segment")]);
    fixture.add_entity("alice", json!({"id": "alice", "name": "Alice"}));
    fixture.add_facet("work", &[("facet.json", b"{\"name\":\"work\"}")]);
    fixture.add_import(
        "20260203_120000",
        json!({"source": "calendar"}),
        json!({"status": "imported"}),
        None,
    );
    fixture.set_config(json!({"convey": {"bind": "127.0.0.1"}}));

    let output = run(&fixture, &[]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--- Export Summary ---"), "{stdout}");
    for area in ["segments", "imports", "entities", "facets", "config"] {
        assert!(stdout.contains(&format!("  {area}:")), "{stdout}");
    }

    let posts = peer
        .ingest_requests()
        .into_iter()
        .map(|request| request.path)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        posts,
        ["segments", "imports", "entities", "facets", "config"]
            .into_iter()
            .map(|area| door("POST", "remote-i", "ingest", area))
            .collect()
    );
    assert!(
        peer.requests()
            .iter()
            .all(|request| request.path != "/app/network/unpair")
    );
}

#[test]
fn authentication_failure_in_a_default_send_exits_nonzero() {
    let peer = StubPeer::new(PeerPlan::new(
        ["segments", "imports", "entities", "facets", "config"]
            .into_iter()
            .map(|area| {
                (
                    RequestRoute::get(door("GET", "remote-i", "manifest", area)),
                    vec![ResponseAction::manifest_empty()],
                )
            })
            .chain([(
                RequestRoute::post(door("POST", "remote-i", "ingest", "config")),
                vec![ResponseAction::status(401, b"invalid")],
            )]),
    ));
    let fixture = peer.fixture();
    fixture.set_config(json!({"convey": {"bind": "127.0.0.1"}}));

    let output = run(&fixture, &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("config: FAILED (Authentication failed: invalid or missing API key)")
    );
    assert_eq!(peer.ingest_requests().len(), 1);
}

#[test]
fn only_and_dry_run_limit_requests_without_unpairing() {
    let peer = StubPeer::new(PeerPlan::new([(
        RequestRoute::get(door("GET", "remote-i", "manifest", "segments")),
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
    assert!(String::from_utf8_lossy(&output.stdout).contains("segments: 1 sent"));
    assert!(peer.ingest_requests().is_empty());
    assert_eq!(peer.requests().len(), 1);
    assert!(peer.handshake_errors().is_empty());
    assert!(
        peer.requests()
            .iter()
            .all(|request| request.path != "/app/network/unpair")
    );
}

#[test]
fn dry_run_without_only_lists_all_areas_and_posts_nothing() {
    let peer = StubPeer::new(PeerPlan::new(
        ["segments", "imports", "entities", "facets", "config"]
            .into_iter()
            .map(|area| {
                (
                    RequestRoute::get(door("GET", "remote-i", "manifest", area)),
                    vec![ResponseAction::manifest_empty()],
                )
            }),
    ));
    let fixture = peer.fixture();
    fixture.add_segment("audio", "120000_30", &[("payload.json", b"payload")]);
    fixture.set_config(json!({"convey": {"bind": "127.0.0.1"}}));

    let output = run(&fixture, &["--dry-run"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    for area in ["segments", "imports", "entities", "facets", "config"] {
        assert!(stdout.contains(&format!("  {area}:")), "{stdout}");
    }
    assert!(peer.ingest_requests().is_empty());
    assert_eq!(peer.requests().len(), 5);
}

#[test]
fn four_area_only_never_unpairs() {
    let peer = StubPeer::new(PeerPlan::new(
        ["segments", "imports", "entities", "facets"]
            .into_iter()
            .map(|area| {
                (
                    RequestRoute::get(door("GET", "remote-i", "manifest", area)),
                    vec![ResponseAction::manifest_empty()],
                )
            }),
    ));
    let fixture = peer.fixture();
    let output = run(&fixture, &["--only", "segments,imports,entities,facets"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
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
            RequestRoute::get(door("GET", "remote-i", "manifest", "segments")),
            vec![ResponseAction::status(500, b"unavailable")],
        ),
        (
            RequestRoute::get(door("GET", "remote-i", "manifest", "imports")),
            vec![ResponseAction::manifest_empty()],
        ),
        (
            RequestRoute::get(door("GET", "remote-i", "manifest", "entities")),
            vec![ResponseAction::manifest_empty()],
        ),
        (
            RequestRoute::get(door("GET", "remote-i", "manifest", "facets")),
            vec![ResponseAction::manifest_empty()],
        ),
        (
            RequestRoute::get(door("GET", "remote-i", "manifest", "config")),
            vec![ResponseAction::manifest_empty()],
        ),
        (
            RequestRoute::post(door("POST", "remote-i", "ingest", "config")),
            vec![ResponseAction::status(
                200,
                json!({"staged": true}).to_string(),
            )],
        ),
    ]));
    let fixture = peer.fixture();
    fixture.set_config(json!({"convey": {"bind": "127.0.0.1"}}));
    let output = run(&fixture, &[]);
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
    for area in ["imports", "entities", "facets"] {
        assert!(
            paths.contains(&door("GET", "remote-i", "manifest", area)),
            "missing {area} manifest"
        );
    }
    assert!(paths.contains(&door("POST", "remote-i", "ingest", "config")));
}

#[test]
fn persistent_entity_server_error_reports_retry_exhaustion() {
    let peer = StubPeer::new(PeerPlan::new([
        (
            RequestRoute::get(door("GET", "remote-i", "manifest", "entities")),
            vec![ResponseAction::manifest_empty()],
        ),
        (
            RequestRoute::post(door("POST", "remote-i", "ingest", "entities")),
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
fn refused_entity_upload_marks_area_failed_and_returns_nonzero() {
    let peer = StubPeer::new(PeerPlan::new([
        (
            RequestRoute::get(door("GET", "remote-i", "manifest", "entities")),
            vec![ResponseAction::manifest_empty()],
        ),
        (
            RequestRoute::post(door("POST", "remote-i", "ingest", "entities")),
            vec![ResponseAction::status(403, b"revoked")],
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
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(
            "entities: FAILED (Authentication failed: journal source revoked or disabled)"
        ),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(peer.ingest_requests().len(), 1);
}

#[test]
fn bad_area_fails_closed_with_an_argparse_style_error() {
    let journal = tempfile::tempdir().expect("journal");
    let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["transfer", "send", "--to", "office", "--only", "bogus"])
        .arg("--journal")
        .arg(journal.path())
        .output()
        .expect("run transfer send");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "journal transfer send: error: --only must contain one or more of: config, entities, facets, imports, segments"
    );
}

#[test]
fn transfer_send_help_documents_the_five_area_default() {
    let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["transfer", "send", "--help"])
        .output()
        .expect("run transfer send help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for option in ["--to TO", "--day DAY", "--only AREAS", "--dry-run"] {
        assert!(stdout.contains(option), "{stdout}");
    }
    assert!(stdout.contains("default: all five areas"), "{stdout}");
    assert!(stdout.contains("segments, imports, entities, facets, config"));
}
