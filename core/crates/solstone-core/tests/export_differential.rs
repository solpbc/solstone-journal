// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[allow(dead_code)]
#[path = "support/stub_peer.rs"]
mod stub_peer;

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{Value, json};
use stub_peer::{CapturedRequest, Fixture, PeerPlan, RequestRoute, ResponseAction, StubPeer};

const KEY_PREFIX: &str = "remote-i";

fn route(area: &str, method: fn(String) -> RequestRoute) -> RequestRoute {
    method(format!("/app/import/journal/{KEY_PREFIX}/{area}"))
}

fn plan() -> PeerPlan {
    PeerPlan::new(
        ["segments", "imports", "entities", "facets", "config"]
            .into_iter()
            .flat_map(|area| {
                [
                    (
                        route(&format!("manifest/{area}"), RequestRoute::get),
                        vec![
                            ResponseAction::manifest_empty(),
                            ResponseAction::manifest_empty(),
                        ],
                    ),
                    (
                        route(&format!("ingest/{area}"), RequestRoute::post),
                        vec![
                            ResponseAction::status(200, b"{}".to_vec()),
                            ResponseAction::status(200, b"{}".to_vec()),
                        ],
                    ),
                ]
            }),
    )
}

fn fixture(peer: &StubPeer) -> Fixture {
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
    fixture
}

fn native(fixture: &Fixture) -> Output {
    Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["export", "--to", "office", "--journal"])
        .arg(fixture.path())
        .output()
        .expect("run native export")
}

fn python(fixture: &Fixture) -> Output {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root");
    let script = r#"
import contextlib
import io
import solstone.observe.export as e
from solstone.observe.pl_http import PlHttpSession
from solstone.think.link.bundle import load_client_identity
from solstone.think.link.dialer import TunnelClient

e.relay_url = lambda: None
peer = e.resolve_peer("office")
identity = load_client_identity(peer.dir)
with TunnelClient(identity, relay_url=e.relay_url()) as tunnel:
    session = PlHttpSession(tunnel)
    with contextlib.redirect_stdout(io.StringIO()):
        results = e._run_export_areas(
            "https://pl.peer", peer.instance_id, ["20260203"], False, session, e.FULL_EXPORT_SET
        )
e._print_export_summary(results)
"#;
    Command::new(repository.join(".venv/bin/python"))
        .arg("-c")
        .arg(script)
        .env("PYTHONPATH", repository)
        .env("SOLSTONE_JOURNAL", fixture.path())
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .output()
        .expect("run Python export")
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

fn config_body(requests: &[CapturedRequest]) -> Value {
    let request = requests
        .iter()
        .find(|request| request.path.ends_with("/ingest/config"))
        .expect("config upload request");
    serde_json::from_slice(&request.body).expect("config JSON body")
}

fn contains_key(value: &Value, target: &str) -> bool {
    match value {
        Value::Object(entries) => {
            entries.contains_key(target)
                || entries.values().any(|value| contains_key(value, target))
        }
        Value::Array(values) => values.iter().any(|value| contains_key(value, target)),
        _ => false,
    }
}

#[test]
fn native_and_python_export_match_all_areas() {
    let peer = StubPeer::new(plan());
    let fixture = fixture(&peer);

    let native = native(&fixture);
    let native_requests = peer.requests();
    let python = python(&fixture);
    let all_requests = peer.requests();
    let python_requests = &all_requests[native_requests.len()..];

    assert_same_child_result(&native, &python);
    assert_equivalent_requests(&native_requests, python_requests);
    for requests in [&native_requests, python_requests] {
        let config = config_body(requests);
        assert!(!contains_key(&config, "password_hash"));
        assert!(!contains_key(&config, "secret"));
        assert_eq!(
            config.pointer("/config/convey/other_field"),
            Some(&json!("send-this"))
        );
    }
}
