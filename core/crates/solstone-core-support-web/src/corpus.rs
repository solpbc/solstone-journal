// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Frozen Support corpus anchors.  W2c extends replay with its write probes.

use axum::{
    body::{Body, to_bytes},
    http::Request,
    response::Response,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_support_drafts::resolve_draft_outcome;
use solstone_core_support_portal::test_support::{RoutePortal, RouteReply};
use solstone_core_support_portal::{Ledger, PortalClient};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tower::ServiceExt;

const CORPUS: &str = include_str!("../../../fixtures/convey_support_corpus.json");

#[derive(Clone, Copy)]
struct CorpusProbe {
    name: &'static str,
    method: &'static str,
    path: &'static str,
    kind: &'static str,
    body: Option<&'static str>,
    key: Option<&'static str>,
    file: Option<(&'static str, &'static [u8], &'static str)>,
}

const KEY: Option<&str> = Some("sact1_corpusfixedparentaction");
const BASE_PROBES: &[CorpusProbe] = &[
    CorpusProbe {
        name: "page_index",
        method: "GET",
        path: "/app/support/",
        kind: "probe",
        body: None,
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "page_workspace",
        method: "GET",
        path: "/app/support/workspace",
        kind: "probe",
        body: None,
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "page_background",
        method: "GET",
        path: "/app/support/background",
        kind: "probe",
        body: None,
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "static_support_js",
        method: "GET",
        path: "/app/support/static/support.js",
        kind: "probe",
        body: None,
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "api_config",
        method: "GET",
        path: "/app/support/api/config",
        kind: "probe",
        body: None,
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "api_tickets_list",
        method: "GET",
        path: "/app/support/api/tickets",
        kind: "probe",
        body: None,
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "api_tickets_list_status",
        method: "GET",
        path: "/app/support/api/tickets?status=open",
        kind: "probe",
        body: None,
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "api_ticket_get",
        method: "GET",
        path: "/app/support/api/tickets/7",
        kind: "probe",
        body: None,
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "api_tickets_closed",
        method: "GET",
        path: "/app/support/api/tickets/closed",
        kind: "probe",
        body: None,
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "api_articles",
        method: "GET",
        path: "/app/support/api/articles?q=backup",
        kind: "probe",
        body: None,
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "api_article",
        method: "GET",
        path: "/app/support/api/articles/getting-started",
        kind: "probe",
        body: None,
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "api_announcements",
        method: "GET",
        path: "/app/support/api/announcements",
        kind: "probe",
        body: None,
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "api_diagnostics",
        method: "GET",
        path: "/app/support/api/diagnostics",
        kind: "probe",
        body: None,
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "api_badge_count",
        method: "GET",
        path: "/app/support/api/badge-count",
        kind: "probe",
        body: None,
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "routing_404_non_integer_ticket",
        method: "GET",
        path: "/app/support/api/tickets/notanint",
        kind: "probe",
        body: None,
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "api_draft",
        method: "POST",
        path: "/app/support/api/draft",
        kind: "probe_repeat",
        body: Some(r#"{"verb":"create","payload":{"subject":"s","description":"d"}}"#),
        key: None,
        file: None,
    },
    CorpusProbe {
        name: "api_register",
        method: "POST",
        path: "/app/support/api/register",
        kind: "probe_repeat",
        body: None,
        key: None,
        file: None,
    },
    CorpusProbe {
        name: "api_ticket_create",
        method: "POST",
        path: "/app/support/api/tickets",
        kind: "probe_repeat",
        body: Some(
            r#"{"subject":"corpus subject","description":"corpus description","auto_context":false}"#,
        ),
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "api_ticket_reply",
        method: "POST",
        path: "/app/support/api/tickets/7/reply",
        kind: "probe_repeat",
        body: Some(r#"{"content":"corpus reply"}"#),
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "api_ticket_attachment",
        method: "POST",
        path: "/app/support/api/tickets/7/attachments",
        kind: "probe_repeat",
        body: Some(r#"{"index":"0"}"#),
        key: KEY,
        file: Some(("corpus.txt", b"corpus attachment bytes", "text/plain")),
    },
    CorpusProbe {
        name: "api_ticket_close",
        method: "POST",
        path: "/app/support/api/tickets/7/close",
        kind: "probe_repeat",
        body: Some("{}"),
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "api_resolution_confirm",
        method: "POST",
        path: "/app/support/api/tickets/7/resolution/confirm",
        kind: "probe_repeat",
        body: Some("{}"),
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "api_resolution_still_need_help",
        method: "POST",
        path: "/app/support/api/tickets/7/resolution/still-need-help",
        kind: "probe_repeat",
        body: Some("{}"),
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "api_feedback",
        method: "POST",
        path: "/app/support/api/feedback",
        kind: "probe_repeat",
        body: Some(r#"{"body":"corpus feedback"}"#),
        key: KEY,
        file: None,
    },
];
const VALIDATION_PROBES: &[CorpusProbe] = &[
    CorpusProbe {
        name: "api_draft_unknown_verb",
        method: "POST",
        path: "/app/support/api/draft",
        kind: "probe",
        body: Some(r#"{"verb":"not-a-verb","payload":{}}"#),
        key: None,
        file: None,
    },
    CorpusProbe {
        name: "api_draft_missing_payload",
        method: "POST",
        path: "/app/support/api/draft",
        kind: "probe",
        body: Some(r#"{"verb":"create"}"#),
        key: None,
        file: None,
    },
    CorpusProbe {
        name: "api_ticket_create_missing_subject",
        method: "POST",
        path: "/app/support/api/tickets",
        kind: "probe",
        body: Some(r#"{"description":"d"}"#),
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "api_feedback_missing_body",
        method: "POST",
        path: "/app/support/api/feedback",
        kind: "probe",
        body: Some("{}"),
        key: KEY,
        file: None,
    },
    CorpusProbe {
        name: "api_ticket_attachment_bad_suffix",
        method: "POST",
        path: "/app/support/api/tickets/7/attachments",
        kind: "probe",
        body: Some(r#"{"index":"0"}"#),
        key: KEY,
        file: Some(("corpus.exe", b"nope", "application/octet-stream")),
    },
    CorpusProbe {
        name: "api_ticket_create_malformed_key",
        method: "POST",
        path: "/app/support/api/tickets",
        kind: "probe",
        body: Some(r#"{"subject":"s","description":"d","auto_context":false}"#),
        key: Some("not-a-valid-action-id"),
        file: None,
    },
];

const DRAIN_PROBES: &[CorpusProbe] = &[
    CorpusProbe {
        name: "drain_on_missing_key_refusal",
        method: "POST",
        path: "/app/support/api/tickets",
        kind: "drain",
        body: Some(r#"{"subject":"s","description":"d"}"#),
        key: None,
        file: None,
    },
    CorpusProbe {
        name: "drain_on_page_request",
        method: "GET",
        path: "/app/support/workspace",
        kind: "drain",
        body: None,
        key: None,
        file: None,
    },
];

const DISABLED_DRAIN_PROBES: &[CorpusProbe] = &[
    CorpusProbe {
        name: "drain_on_feature_unavailable_refusal",
        method: "GET",
        path: "/app/support/api/badge-count",
        kind: "drain",
        body: None,
        key: None,
        file: None,
    },
    CorpusProbe {
        name: "drain_on_local_read",
        method: "GET",
        path: "/app/support/api/config",
        kind: "drain",
        body: None,
        key: None,
        file: None,
    },
];

#[derive(Clone)]
struct ResolvedProbe {
    name: String,
    probe: CorpusProbe,
}

fn probes_for(phase: &str) -> Vec<ResolvedProbe> {
    let mut probes = BASE_PROBES
        .iter()
        .copied()
        .map(|probe| ResolvedProbe {
            name: probe.name.to_owned(),
            probe,
        })
        .collect::<Vec<_>>();
    if phase == "established" {
        probes.extend(
            BASE_PROBES
                .iter()
                .copied()
                .filter(|probe| probe.method == "POST" && probe.key.is_some())
                .map(|probe| ResolvedProbe {
                    name: format!("{}_no_key", probe.name),
                    probe: CorpusProbe {
                        name: probe.name,
                        kind: "probe",
                        key: None,
                        ..probe
                    },
                }),
        );
        probes.extend(
            VALIDATION_PROBES
                .iter()
                .chain(DRAIN_PROBES)
                .copied()
                .map(|probe| ResolvedProbe {
                    name: probe.name.to_owned(),
                    probe,
                }),
        );
    }
    if phase == "disabled" {
        probes.extend(
            DISABLED_DRAIN_PROBES
                .iter()
                .copied()
                .map(|probe| ResolvedProbe {
                    name: probe.name.to_owned(),
                    probe,
                }),
        );
    }
    probes
}

/// Explicit native/reference differences; never a broad normalization exemption.
const NATIVE_DIAGNOSTICS_CORPUS_DIVERGENCES: [(&str, &str, &str); 6] = [
    (
        "established",
        "api_diagnostics",
        "response.body.platform.python",
    ),
    (
        "established",
        "api_diagnostics",
        "response.body.recent_errors.*.time",
    ),
    (
        "disabled",
        "api_diagnostics",
        "response.body.platform.python",
    ),
    (
        "disabled",
        "api_diagnostics",
        "response.body.recent_errors.*.time",
    ),
    (
        "unregistered",
        "api_diagnostics",
        "response.body.platform.python",
    ),
    (
        "unregistered",
        "api_diagnostics",
        "response.body.recent_errors.*.time",
    ),
];

/// Test roots need a loopback URL in config; the Python capture supplied it by env instead.
const LOOPBACK_PORTAL_URL_SEED_NORMALIZATION: [(&str, &str); 3] = [
    ("established", "api_diagnostics"),
    ("disabled", "api_diagnostics"),
    ("unregistered", "api_diagnostics"),
];

/// Flask gates before route matching; the native shell gates after it.
const NATIVE_GATE_PHASE_ROUTING_DIVERGENCES: [(&str, &str, u16, u16); 2] = [
    ("unestablished", "routing_404_non_integer_ticket", 404, 302),
    ("corrupt", "routing_404_non_integer_ticket", 404, 500),
];

const HEADER_ALLOWLIST: [&str; 2] = ["Content-Type", "Location"];

fn compare_json(actual: &Value, expected: &Value) -> Result<(), String> {
    (actual == expected)
        .then_some(())
        .ok_or_else(|| "json body differs".to_owned())
}

fn compare_text(actual: &str, expected: &str) -> Result<(), String> {
    (actual == expected)
        .then_some(())
        .ok_or_else(|| "text differs".to_owned())
}

fn compare_text_sha256(actual: &[u8], expected: &str) -> Result<(), String> {
    (format!("{:x}", Sha256::digest(actual)) == expected)
        .then_some(())
        .ok_or_else(|| "text sha256 differs".to_owned())
}

fn compare_text_bytes(actual: &[u8], expected: u64) -> Result<(), String> {
    (actual.len() as u64 == expected)
        .then_some(())
        .ok_or_else(|| "text bytes differs".to_owned())
}

fn compare_text_prefix(actual: &str, expected: &str) -> Result<(), String> {
    actual
        .starts_with(expected)
        .then_some(())
        .ok_or_else(|| "text prefix differs".to_owned())
}

fn compare_headers(actual: &[(&str, &str)], expected: &[(&str, &str)]) -> Result<(), String> {
    let filter = |headers: &[(&str, &str)]| {
        let mut filtered = headers
            .iter()
            .filter_map(|(name, value)| {
                HEADER_ALLOWLIST
                    .iter()
                    .find(|allowed| name.eq_ignore_ascii_case(allowed))
                    .map(|allowed| ((*allowed).to_owned(), (*value).to_owned()))
            })
            .collect::<Vec<_>>();
        filtered.sort_unstable();
        filtered
    };
    (filter(actual) == filter(expected))
        .then_some(())
        .ok_or_else(|| "allowed headers differ".to_owned())
}

fn compare_portal_requests(actual: &[&str], expected: &[&str]) -> Result<(), String> {
    (actual == expected)
        .then_some(())
        .ok_or_else(|| "portal requests differ".to_owned())
}

fn compare_repeat_response(actual: &Value, expected: &Value) -> Result<(), String> {
    (actual == expected)
        .then_some(())
        .ok_or_else(|| "repeat response differs".to_owned())
}

fn same_json_type(left: &Value, right: &Value) -> bool {
    matches!(
        (left, right),
        (Value::Null, Value::Null)
            | (Value::Bool(_), Value::Bool(_))
            | (Value::Number(_), Value::Number(_))
            | (Value::String(_), Value::String(_))
            | (Value::Array(_), Value::Array(_))
            | (Value::Object(_), Value::Object(_))
    )
}

fn normalize_bare_pointer(actual: &mut Value, expected: &mut Value, pointer: &str) {
    let parts = pointer
        .trim_start_matches("response.body.")
        .trim_start_matches("repeat_response.body.")
        .split('.')
        .collect::<Vec<_>>();
    normalize_bare_parts(actual, expected, &parts, pointer);
}

fn remove_bare_pointer(value: &mut Value, pointer: &str) {
    let parts = pointer
        .trim_start_matches("response.body.")
        .trim_start_matches("repeat_response.body.")
        .split('.')
        .collect::<Vec<_>>();
    remove_bare_parts(value, &parts, pointer);
}

fn remove_bare_parts(value: &mut Value, parts: &[&str], pointer: &str) {
    if parts.len() == 1 {
        value
            .as_object_mut()
            .and_then(|object| object.remove(parts[0]))
            .unwrap_or_else(|| panic!("missing expected divergence pointer {pointer}"));
        return;
    }
    if parts[0] == "*" {
        for value in value
            .as_array_mut()
            .unwrap_or_else(|| panic!("expected divergence wildcard is not an array: {pointer}"))
        {
            remove_bare_parts(value, &parts[1..], pointer);
        }
        return;
    }
    let child = value
        .as_object_mut()
        .and_then(|object| object.get_mut(parts[0]))
        .unwrap_or_else(|| panic!("missing expected divergence pointer {pointer}"));
    remove_bare_parts(child, &parts[1..], pointer);
}

fn is_named_diagnostics_divergence(phase: &str, case: &str, pointer: &str) -> bool {
    NATIVE_DIAGNOSTICS_CORPUS_DIVERGENCES.iter().any(
        |(allowed_phase, allowed_case, allowed_pointer)| {
            *allowed_phase == phase && *allowed_case == case && *allowed_pointer == pointer
        },
    )
}

fn uses_loopback_portal_url_seed(phase: &str, case: &str) -> bool {
    LOOPBACK_PORTAL_URL_SEED_NORMALIZATION
        .iter()
        .any(|(allowed_phase, allowed_case)| *allowed_phase == phase && *allowed_case == case)
}

fn normalize_bare_parts(actual: &mut Value, expected: &mut Value, parts: &[&str], pointer: &str) {
    if parts.len() == 1 {
        let key = parts[0];
        let actual = actual
            .as_object_mut()
            .and_then(|value| value.remove(key))
            .unwrap_or_else(|| panic!("missing bare pointer {pointer}"));
        let expected = expected
            .as_object_mut()
            .and_then(|value| value.remove(key))
            .unwrap_or_else(|| panic!("missing expected bare pointer {pointer}"));
        assert!(
            same_json_type(&actual, &expected),
            "bare pointer type differs: {pointer}"
        );
        return;
    }
    if parts[0] == "*" {
        let actual = actual
            .as_array_mut()
            .unwrap_or_else(|| panic!("bare wildcard is not an array: {pointer}"));
        let expected = expected
            .as_array_mut()
            .unwrap_or_else(|| panic!("expected wildcard is not an array: {pointer}"));
        assert_eq!(
            actual.len(),
            expected.len(),
            "bare wildcard length: {pointer}"
        );
        for (actual, expected) in actual.iter_mut().zip(expected) {
            normalize_bare_parts(actual, expected, &parts[1..], pointer);
        }
    } else {
        let actual = actual
            .as_object_mut()
            .and_then(|value| value.get_mut(parts[0]))
            .unwrap_or_else(|| panic!("missing bare pointer {pointer}"));
        let expected = expected
            .as_object_mut()
            .and_then(|value| value.get_mut(parts[0]))
            .unwrap_or_else(|| panic!("missing expected bare pointer {pointer}"));
        normalize_bare_parts(actual, expected, &parts[1..], pointer);
    }
}

const STUB_PORTAL_URL: &str = "https://portal.example";

fn corpus_route_portal() -> RoutePortal {
    let pinned = &serde_json::from_str::<Value>(CORPUS).expect("corpus parses")["pinned"];
    let tos = pinned["stub_tos"].as_str().expect("pinned tos").to_owned();
    let token = pinned["stub_access_token"]
        .as_str()
        .expect("pinned token")
        .to_owned();
    let handle = pinned["handle"].as_str().expect("pinned handle").to_owned();
    let ticket = pinned["seeded_ticket_id"].as_i64().expect("pinned ticket");
    RoutePortal::new(fixed_routes(tos, token, handle, ticket))
}

fn install_route_portal(portal: &RoutePortal) -> super::TestClientGuard {
    let shared = portal.share();
    super::install_test_client_factory(move |root, anonymous| {
        shared.client(root.join("apps/support/portal"), None, anonymous)
    })
}

fn fixed_routes(
    tos: String,
    token: String,
    handle: String,
    ticket: i64,
) -> BTreeMap<(String, String), RouteReply> {
    let json_reply = |value: Value| RouteReply {
        status: 200,
        body: value.to_string(),
        content_type: "application/json".to_owned(),
    };
    let open = serde_json::json!({"ticket_id":ticket,"status":"open","subject":"a seeded open ticket","created_at":"2026-02-01T00:00:00Z","updated_at":"2026-02-02T00:00:00Z","body":"a field an active ticket keeps"});
    let closed = serde_json::json!({"ticket_id":8,"status":"closed","closed_at":"2026-02-03T00:00:00Z","close_scheduled_at":"2026-02-10T00:00:00Z","reason_code":"resolved","subject":"a field a tombstone must drop","thread":[{"body":"a message a tombstone must drop"}]});
    let mut routes = BTreeMap::new();
    routes.insert(
        ("GET".into(), "/tos".into()),
        RouteReply {
            status: 200,
            body: tos,
            content_type: "text/plain; charset=utf-8".into(),
        },
    );
    routes.insert(
        ("POST".into(), "/api/signup".into()),
        json_reply(serde_json::json!({"access_token":token,"handle":handle})),
    );
    for (method, path, value) in [
        (
            "POST",
            "/api/idempotency/ack",
            serde_json::json!({"acknowledged":true}),
        ),
        (
            "GET",
            "/api/tickets",
            Value::Array(vec![open.clone(), closed.clone()]),
        ),
        (
            "GET",
            "/api/tickets/closed",
            serde_json::json!({"tickets":[closed.clone()],"next_cursor":"corpus-cursor"}),
        ),
        (
            "GET",
            "/api/articles",
            serde_json::json!([{"slug":"getting-started","title":"Getting started"}]),
        ),
        (
            "GET",
            "/api/articles/getting-started",
            serde_json::json!({"slug":"getting-started","body":"an article"}),
        ),
        (
            "GET",
            "/api/announcements",
            serde_json::json!([{"id":1,"title":"an announcement"}]),
        ),
        (
            "POST",
            "/api/tickets",
            serde_json::json!({"ticket_id":101,"status":"open","subject":"corpus subject"}),
        ),
        (
            "POST",
            "/api/tickets/7/messages",
            serde_json::json!({"message_id":202,"status":"open"}),
        ),
        (
            "POST",
            "/api/tickets/7/attachments",
            serde_json::json!({"attachment_id":303,"status":"open","filename":"corpus.txt"}),
        ),
        ("POST", "/api/tickets/7/close", closed.clone()),
        ("POST", "/api/tickets/7/resolution/confirm", closed.clone()),
        (
            "POST",
            "/api/tickets/7/resolution/still-need-help",
            open.clone(),
        ),
    ] {
        routes.insert((method.into(), path.into()), json_reply(value));
    }
    routes.insert(
        ("GET".into(), format!("/api/tickets/{ticket}")),
        json_reply(open),
    );
    routes
}

fn phase_root(phase: &str, portal: Option<&RoutePortal>) -> TempDir {
    let root = TempDir::new().expect("phase root");
    let config = root.path().join("config");
    std::fs::create_dir_all(&config).expect("config directory");
    match phase {
        "unestablished" => return root,
        "corrupt" => std::fs::write(
            config.join("journal.json"),
            br#"{"setup":{"completed_at":17672256"#,
        )
        .expect("corrupt config"),
        "established" | "disabled" | "unregistered" => {
            let pinned: Value = serde_json::from_str(CORPUS).expect("corpus parses");
            std::fs::write(
                config.join("journal.json"),
                serde_json::to_vec(
                    &serde_json::json!({"setup":{"completed_at":pinned["pinned"]["completed_at"]}}),
                )
                .expect("session config"),
            )
            .expect("write session config");
            std::fs::write(
                config.join("config.json"),
                serde_json::to_vec(&serde_json::json!({
                    "support":{"enabled":phase != "disabled","portal_url":STUB_PORTAL_URL},
                    "provider":{"api_key":"corpus-not-a-real-key-authored-for-this-fixture"},
                    "observe":{"enabled":true},
                }))
                .expect("app config"),
            )
            .expect("write app config");
            let health = root.path().join("health");
            std::fs::create_dir_all(&health).expect("health directory");
            std::fs::write(
                health.join("observer.pid"),
                format!("{}\n", std::process::id()),
            )
            .expect("observer pid");
            std::fs::write(health.join("cortex.pid"), "not-a-pid\n").expect("cortex pid");
            let now = chrono::Local::now();
            let stamp = (now - chrono::Duration::hours(1)).format("%Y-%m-%dT%H:%M:%S");
            let stale = (now - chrono::Duration::hours(200)).format("%Y-%m-%dT%H:%M:%S");
            std::fs::write(
                health.join("supervisor.log"),
                format!(
                    "{stamp} ERROR provider rejected the call: api_key=corpus-authored-secret-value\n{stamp} ERROR reading /corpus/authored/path/file.txt failed\nERROR Traceback (most recent call last):\n{stale} ERROR this one is older than the recency window\n{stamp} INFO the operator asked about an ERROR yesterday\n{stamp} INFO an ordinary informational line\n"
                ),
            )
            .expect("supervisor log");
            if phase != "unregistered" {
                let mut client = portal
                    .expect("established and disabled phases need a RoutePortal")
                    .client(root.path().join("apps/support/portal"), None, false)
                    .expect("client");
                client.register().expect("registered fixture identity");
                let snapshot = root.path().join(".registered-support-state");
                std::fs::create_dir(&snapshot).expect("snapshot directory");
                for name in ["keypair.pem", "token.json", "tos.txt"] {
                    std::fs::copy(
                        root.path().join("apps/support/portal").join(name),
                        snapshot.join(name),
                    )
                    .expect("snapshot artifact");
                }
            }
        }
        _ => panic!("unknown phase {phase}"),
    }
    root
}

fn reset_portal_storage(root: &std::path::Path, phase: &str) {
    let storage = root.join("apps/support/portal");
    let _ = std::fs::remove_dir_all(&storage);
    if !matches!(phase, "established" | "disabled") {
        return;
    }
    let snapshot = root.join(".registered-support-state");
    std::fs::create_dir_all(&storage).expect("restore storage");
    for name in ["keypair.pem", "token.json", "tos.txt"] {
        let target = storage.join(name);
        std::fs::copy(snapshot.join(name), &target).expect("restore artifact");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))
                .expect("artifact permissions");
        }
    }
}

fn ledger_contents(root: &Path) -> Vec<(String, Vec<u8>)> {
    let operations = root.join("apps/support/portal/operations");
    let Ok(entries) = std::fs::read_dir(operations) else {
        return Vec::new();
    };
    let mut records = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            path.is_file().then(|| {
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    std::fs::read(path).expect("ledger record contents"),
                )
            })
        })
        .collect::<Vec<_>>();
    records.sort_by(|left, right| left.0.cmp(&right.0));
    records
}

fn corpus_request(probe: &CorpusProbe) -> Request<Body> {
    let mut request = Request::builder()
        .method(probe.method)
        .uri(probe.path)
        .header("Host", "127.0.0.1");
    if let Some(key) = probe.key {
        request = request.header("Idempotency-Key", key);
    }
    if let Some((filename, bytes, content_type)) = probe.file {
        const BOUNDARY: &str = "support-corpus-boundary";
        let mut body = Vec::new();
        if let Some(fields) = probe.body {
            for (name, value) in serde_json::from_str::<Value>(fields)
                .expect("multipart fields")
                .as_object()
                .expect("multipart object")
            {
                let value = value
                    .as_str()
                    .map_or_else(|| value.to_string(), str::to_owned);
                body.extend_from_slice(
                    format!(
                        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
                    )
                    .as_bytes(),
                );
            }
        }
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
        request
            .header(
                "Content-Type",
                format!("multipart/form-data; boundary={BOUNDARY}"),
            )
            .body(Body::from(body))
            .expect("multipart request")
    } else if let Some(body) = probe.body {
        request
            .header("Content-Type", "application/json")
            .body(Body::from(body))
            .expect("json request")
    } else {
        request.body(Body::empty()).expect("empty request")
    }
}

fn multipart_request(
    path: &str,
    key: Option<&str>,
    fields: &[(&str, &str)],
    file: Option<(Option<&str>, &[u8], &str)>,
) -> Request<Body> {
    const BOUNDARY: &str = "support-derivation-boundary";
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    if let Some((filename, bytes, content_type)) = file {
        body.extend_from_slice(
            format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\nContent-Type: {content_type}\r\n\r\n", filename.unwrap_or_default()).as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    let mut request = Request::post(path).header("Host", "127.0.0.1").header(
        "Content-Type",
        format!("multipart/form-data; boundary={BOUNDARY}"),
    );
    if let Some(key) = key {
        request = request.header("Idempotency-Key", key);
    }
    request.body(Body::from(body)).expect("multipart request")
}

fn json_request(path: &str, body: Value) -> Request<Body> {
    Request::post(path)
        .header("Host", "127.0.0.1")
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("json request")
}

fn keyed_json_request(path: &str, body: Value) -> Request<Body> {
    let mut request = json_request(path, body);
    request.headers_mut().insert(
        "Idempotency-Key",
        KEY.expect("corpus action key")
            .parse()
            .expect("action header"),
    );
    request
}

async fn shell_response(root: &Path, request: Request<Body>) -> (u16, Value) {
    let response = super::routes(root.to_path_buf())
        .oneshot(request)
        .await
        .expect("shell response");
    let status = response.status().as_u16();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    (
        status,
        serde_json::from_slice(&body).expect("json response"),
    )
}

fn draft_event(root: &Path, draft_id: &str) -> Value {
    let locator = root
        .join("chronicle/health/support-drafts")
        .join(format!("{draft_id}.json"));
    let day = serde_json::from_slice::<Value>(&std::fs::read(locator).expect("draft locator"))
        .expect("draft locator json")["captured_day"]
        .as_str()
        .expect("captured day")
        .to_owned();
    let drafts = root.join("chronicle").join(day).join("support-drafts");
    for segment in std::fs::read_dir(drafts)
        .expect("support-draft segments")
        .flatten()
    {
        let path = segment.path().join("support-drafts.jsonl");
        let Ok(contents) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in contents.lines() {
            let event: Value = serde_json::from_str(line).expect("draft event json");
            if event["kind"] == "support_draft" && event["draft_id"] == draft_id {
                return event;
            }
        }
    }
    panic!("support draft event for {draft_id}");
}

async fn assert_recorded_response(
    response: Response,
    expected_response: &Value,
    normalized: &[Value],
    normalization_prefix: &str,
    phase: &str,
    name: &str,
) {
    assert_eq!(
        response.status().as_u16(),
        expected_response["status"].as_u64().unwrap() as u16,
        "{name}"
    );
    let actual_headers = HEADER_ALLOWLIST
        .iter()
        .filter_map(|header| {
            response
                .headers()
                .get(*header)
                .and_then(|value| value.to_str().ok())
                .map(|value| (*header, value))
        })
        .collect::<Vec<_>>();
    let expected_headers = HEADER_ALLOWLIST
        .iter()
        .filter_map(|header| {
            expected_response["headers"][*header]
                .as_str()
                .map(|value| (*header, value))
        })
        .collect::<Vec<_>>();
    compare_headers(&actual_headers, &expected_headers)
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    if let Some(expected) = expected_response.get("body") {
        let mut actual: Value = serde_json::from_slice(&body).expect("json response");
        let mut expected = expected.clone();
        if uses_loopback_portal_url_seed(phase, name) {
            actual
                .pointer_mut("/config/support")
                .and_then(Value::as_object_mut)
                .and_then(|support| support.remove("portal_url"))
                .expect("loopback portal URL seed");
        }
        for pointer in normalized {
            let pointer = pointer.as_str().unwrap();
            if !pointer.starts_with(normalization_prefix) {
                continue;
            }
            if pointer.ends_with("#portal_url") {
                actual["portal_url"] = Value::String("<STUB_PORTAL>".to_owned());
            } else if !pointer.contains('#') {
                if is_named_diagnostics_divergence(phase, name, pointer) {
                    if pointer == "response.body.platform.python" {
                        remove_bare_pointer(&mut expected, pointer);
                    } else if actual
                        .get("recent_errors")
                        .and_then(Value::as_array)
                        .is_none_or(Vec::is_empty)
                    {
                        actual
                            .as_object_mut()
                            .and_then(|object| object.remove("recent_errors"))
                            .expect("native recent_errors divergence");
                        expected
                            .as_object_mut()
                            .and_then(|object| object.remove("recent_errors"))
                            .expect("expected recent_errors divergence");
                    } else {
                        normalize_bare_pointer(&mut actual, &mut expected, pointer);
                    }
                } else {
                    normalize_bare_pointer(&mut actual, &mut expected, pointer);
                }
            }
        }
        compare_json(&actual, &expected).unwrap_or_else(|error| panic!("{name}: {error}"));
    }
    if let Some(hash) = expected_response.get("text_sha256") {
        assert_eq!(
            format!("{:x}", Sha256::digest(&body)),
            hash.as_str().unwrap(),
            "{name}"
        );
    }
    if let Some(bytes) = expected_response.get("text_bytes") {
        compare_text_bytes(&body, bytes.as_u64().unwrap())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
    }
    if let Some(prefix) = expected_response.get("text_prefix") {
        compare_text_prefix(
            std::str::from_utf8(&body).unwrap(),
            prefix.as_str().unwrap(),
        )
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    }
    if let Some(text) = expected_response.get("text") {
        compare_text(std::str::from_utf8(&body).unwrap(), text.as_str().unwrap())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
    }
}

async fn replay_case(
    phase: &str,
    case: &Value,
    probe: &CorpusProbe,
    portal: &RoutePortal,
    root: &Path,
) {
    reset_portal_storage(root, phase);
    portal.clear_log();
    let _guard = install_route_portal(portal);
    if probe.kind == "drain" {
        seed_pending_acknowledgement(root, "corpus-drain");
    }
    let name = case["name"].as_str().expect("case name");
    let normalized = case["normalized"].as_array().expect("normalized pointers");
    let response = super::routes(root.to_path_buf())
        .oneshot(corpus_request(probe))
        .await
        .expect("router response");
    assert_recorded_response(
        response,
        &case["response"],
        normalized,
        "response.",
        phase,
        name,
    )
    .await;
    if let Some(repeat) = case.get("repeat_response") {
        let response = super::routes(root.to_path_buf())
            .oneshot(corpus_request(probe))
            .await
            .expect("repeat router response");
        assert_recorded_response(
            response,
            repeat,
            normalized,
            "repeat_response.",
            phase,
            name,
        )
        .await;
    }
    let actual_requests = portal
        .log()
        .into_iter()
        .map(|request| {
            serde_json::json!({"method":request.method,"path":request.path,"had_idempotency_key":request.idempotency_key.is_some(),"had_authorization":request.had_authorization,"had_dpop":request.had_dpop})
        })
        .collect::<Vec<_>>();
    assert_eq!(
        actual_requests,
        case["portal_requests"].as_array().unwrap().clone(),
        "{name} portal requests"
    );
    let records_ticket_list = case["portal_requests"]
        .as_array()
        .expect("portal requests")
        .iter()
        .any(|request| request["method"] == "GET" && request["path"] == "/api/tickets");
    if name == "api_tickets_list_status" && records_ticket_list {
        let ticket_list = portal
            .log()
            .into_iter()
            .find(|request| request.method == "GET" && request.path == "/api/tickets")
            .expect("ticket list portal request");
        assert_eq!(ticket_list.query.as_deref(), Some("status=open"));
    }
    if name == "api_tickets_list" && records_ticket_list {
        let ticket_list = portal
            .log()
            .into_iter()
            .find(|request| request.method == "GET" && request.path == "/api/tickets")
            .expect("ticket list portal request");
        assert_eq!(ticket_list.query, None);
    }
}

#[test]
fn corpus_has_the_frozen_phase_counts() {
    let corpus: serde_json::Value = serde_json::from_str(CORPUS).expect("support corpus parses");
    let expected = &corpus["expected_case_counts"];
    assert_eq!(expected["unestablished"], 24);
    assert_eq!(expected["corrupt"], 24);
    assert_eq!(expected["established"], 39);
    assert_eq!(expected["disabled"], 26);
    assert_eq!(expected["unregistered"], 24);
    assert_eq!(
        expected
            .as_object()
            .expect("counts object")
            .values()
            .map(|count| count.as_u64().expect("count"))
            .sum::<u64>(),
        137
    );
}

#[test]
fn every_fixture_case_resolves_to_the_transcribed_phase_probe() {
    let corpus: Value = serde_json::from_str(CORPUS).expect("support corpus parses");
    let mut total = 0;
    for phase in [
        "established",
        "disabled",
        "unestablished",
        "corrupt",
        "unregistered",
    ] {
        let probes = probes_for(phase);
        let cases = corpus["phases"][phase].as_array().expect("phase cases");
        assert_eq!(
            probes.len(),
            corpus["expected_case_counts"][phase].as_u64().unwrap() as usize,
            "{phase} probe count"
        );
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let resolved = probes
                .iter()
                .find(|probe| probe.name == name)
                .unwrap_or_else(|| panic!("unresolved {phase} case {name}"));
            assert!(
                !resolved.probe.method.is_empty() && !resolved.probe.path.is_empty(),
                "{phase} case {name} resolved to an incomplete typed probe"
            );
        }
        total += probes.len();
    }
    assert_eq!(total, 137);
}

#[test]
fn diagnostics_divergences_are_named_and_corpus_referenced() {
    assert_eq!(NATIVE_DIAGNOSTICS_CORPUS_DIVERGENCES.len(), 6);
    for (phase, case, _) in NATIVE_DIAGNOSTICS_CORPUS_DIVERGENCES {
        assert!(CORPUS.contains(&format!("\"{phase}\"")));
        assert!(CORPUS.contains(case));
    }
}

#[test]
fn workspace_and_support_js_use_draft_routes_not_direct_mutation() {
    let workspace = std::str::from_utf8(super::WORKSPACE).expect("workspace is utf-8");
    let js = std::str::from_utf8(super::SUPPORT_JS).expect("support js is utf-8");
    for hay in [workspace, js] {
        assert!(
            !hay.contains("/app/support/api/feedback"),
            "direct feedback mutation remains"
        );
        assert!(!hay.contains("/reply"), "direct reply mutation remains");
        assert!(
            !hay.contains("/attachments"),
            "direct attachment mutation remains"
        );
    }
    let combined = format!("{workspace}{js}");
    assert!(
        combined.contains("/app/support/api/draft"),
        "draft capture route missing"
    );
    assert!(
        combined.contains("/draft/confirm"),
        "draft confirm route missing"
    );
    assert!(
        combined.contains("/draft/cancel"),
        "draft cancel route missing"
    );
}

#[test]
fn comparator_rejects_changed_json_body() {
    assert!(compare_json(&serde_json::json!({"a": 1}), &serde_json::json!({"a": 2})).is_err());
}

#[test]
fn comparator_rejects_changed_inline_text() {
    assert!(compare_text("actual", "expected").is_err());
}

#[test]
fn comparator_rejects_changed_text_sha256() {
    assert!(compare_text_sha256(b"actual", "00").is_err());
}

#[test]
fn comparator_rejects_changed_text_bytes() {
    assert!(compare_text_bytes(b"actual", 1).is_err());
}

#[test]
fn comparator_rejects_changed_text_prefix() {
    assert!(compare_text_prefix("actual", "expected").is_err());
}

#[test]
fn comparator_rejects_changed_allowed_header() {
    assert!(
        compare_headers(
            &[("Content-Type", "application/json")],
            &[("Content-Type", "text/plain")]
        )
        .is_err()
    );
}

#[test]
fn comparator_rejects_changed_portal_request_sequence() {
    assert!(compare_portal_requests(&["GET /tos"], &["GET /api/tickets"]).is_err());
}

#[test]
fn comparator_rejects_changed_repeat_response() {
    assert!(
        compare_repeat_response(
            &serde_json::json!({"count": 1}),
            &serde_json::json!({"count": 2})
        )
        .is_err()
    );
}

#[test]
fn established_phase_starts_registered_and_reads_without_signup() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    portal.clear_log();
    let mut client = portal
        .client(root.path().join("apps/support/portal"), None, false)
        .expect("client");
    assert!(client.is_registered());
    client.list_tickets(None, None, None).expect("ticket read");
    assert_eq!(
        portal
            .log()
            .iter()
            .map(|request| request.path.as_str())
            .collect::<Vec<_>>(),
        vec!["/api/tickets"]
    );
}

#[test]
fn unregistered_phase_reregisters_after_each_storage_reset() {
    let portal = corpus_route_portal();
    let root = phase_root("unregistered", Some(&portal));
    for _ in 0..2 {
        reset_portal_storage(root.path(), "unregistered");
        portal
            .client(root.path().join("apps/support/portal"), None, false)
            .expect("client")
            .list_tickets(None, None, None)
            .expect("ticket read");
    }
    assert_eq!(
        portal
            .log()
            .iter()
            .map(|request| request.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "/tos",
            "/api/signup",
            "/api/tickets",
            "/tos",
            "/api/signup",
            "/api/tickets"
        ]
    );
}

#[test]
fn established_reset_clears_ledger_and_restores_private_identity() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let storage = root.path().join("apps/support/portal");
    let fields = serde_json::Map::from_iter([("ticket_id".to_owned(), serde_json::json!(7))]);
    Ledger::new(&storage)
        .begin_operation(
            "sact1_corpusfixedparentaction",
            "close",
            &fields,
            "anonymous",
            0,
            chrono::Utc::now(),
        )
        .expect("seed ledger record");
    assert!(storage.join("operations").is_dir());
    reset_portal_storage(root.path(), "established");
    assert!(!storage.join("operations").exists());
    for name in ["keypair.pem", "token.json"] {
        let path = storage.join(name);
        assert!(path.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}

#[tokio::test]
async fn established_read_and_page_cases_drive_all_fourteen_named_probes() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let _guard = install_route_portal(&portal);
    let corpus: Value = serde_json::from_str(CORPUS).unwrap();
    let names = [
        "page_index",
        "page_workspace",
        "page_background",
        "static_support_js",
        "api_config",
        "api_tickets_list",
        "api_tickets_list_status",
        "api_ticket_get",
        "api_tickets_closed",
        "api_articles",
        "api_article",
        "api_announcements",
        "api_diagnostics",
        "api_badge_count",
    ];
    assert_eq!(names.len(), 14);
    let probes = probes_for("established");
    for name in names {
        let case = corpus["phases"]["established"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == name)
            .unwrap_or_else(|| panic!("no corpus case for {name}"));
        let probe = probes
            .iter()
            .find(|probe| probe.name == name)
            .unwrap_or_else(|| panic!("no typed probe for {name}"));
        replay_case("established", case, &probe.probe, &portal, root.path()).await;
    }
    assert_eq!(
        corpus["phases"]["established"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| names.contains(&case["name"].as_str().unwrap()))
            .filter(|case| case.get("repeat_response").is_some())
            .count(),
        0
    );
}

#[tokio::test]
async fn established_write_cases_match_both_recorded_drives() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let _guard = install_route_portal(&portal);
    let corpus: Value = serde_json::from_str(CORPUS).expect("support corpus parses");
    let names = [
        "api_draft",
        "api_register",
        "api_ticket_create",
        "api_ticket_reply",
        "api_ticket_attachment",
        "api_ticket_close",
        "api_resolution_confirm",
        "api_resolution_still_need_help",
        "api_feedback",
    ];
    let still_skipped = [
        "routing_404_non_integer_ticket",
        "api_ticket_create_no_key",
        "api_ticket_reply_no_key",
        "api_ticket_attachment_no_key",
        "api_ticket_close_no_key",
        "api_resolution_confirm_no_key",
        "api_resolution_still_need_help_no_key",
        "api_feedback_no_key",
        "api_draft_unknown_verb",
        "api_draft_missing_payload",
        "api_ticket_create_missing_subject",
        "api_feedback_missing_body",
        "api_ticket_attachment_bad_suffix",
        "api_ticket_create_malformed_key",
        "drain_on_missing_key_refusal",
        "drain_on_page_request",
    ];
    assert_eq!(names.len(), 9);
    assert_eq!(still_skipped.len(), 16);
    assert_eq!(
        corpus["expected_case_counts"]["established"].as_u64(),
        Some(39)
    );
    assert_eq!(
        corpus["expected_case_counts"]
            .as_object()
            .expect("case counts")
            .values()
            .map(|value| value.as_u64().expect("case count"))
            .sum::<u64>(),
        137
    );

    let cases = corpus["phases"]["established"]
        .as_array()
        .expect("established cases");
    let probes = probes_for("established");
    for name in names {
        let case = cases
            .iter()
            .find(|case| case["name"] == name)
            .unwrap_or_else(|| panic!("no established corpus case for {name}"));
        assert!(case.get("repeat_response").is_some(), "{name} repeats");
        let probe = probes
            .iter()
            .find(|probe| probe.name == name)
            .unwrap_or_else(|| panic!("no typed probe for {name}"));
        replay_case("established", case, &probe.probe, &portal, root.path()).await;
    }
}

#[tokio::test]
async fn drain_runs_after_a_create_and_acknowledges_the_completed_operation() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let _guard = install_route_portal(&portal);
    let corpus: Value = serde_json::from_str(CORPUS).expect("support corpus parses");
    let case = corpus["phases"]["established"]
        .as_array()
        .expect("established cases")
        .iter()
        .find(|case| case["name"] == "api_ticket_create")
        .expect("create case");
    let probe = probes_for("established")
        .into_iter()
        .find(|probe| probe.name == "api_ticket_create")
        .expect("create probe");
    reset_portal_storage(root.path(), "established");
    portal.clear_log();
    let response = super::routes(root.path().to_path_buf())
        .oneshot(corpus_request(&probe.probe))
        .await
        .expect("create response");
    assert_recorded_response(
        response,
        &case["response"],
        case["normalized"].as_array().expect("normalized pointers"),
        "response.",
        "established",
        "api_ticket_create",
    )
    .await;
    assert_eq!(
        portal
            .log()
            .iter()
            .map(|request| (request.method.as_str(), request.path.as_str()))
            .collect::<Vec<_>>(),
        vec![("POST", "/api/tickets"), ("POST", "/api/idempotency/ack"),]
    );
    assert!(
        Ledger::new(root.path().join("apps/support/portal"))
            .list_pending_acknowledgements()
            .expect("pending acknowledgements")
            .is_empty()
    );
}

#[tokio::test]
async fn established_key_refusals_precede_ledger_and_portal_work() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let _guard = install_route_portal(&portal);
    let corpus: Value = serde_json::from_str(CORPUS).expect("support corpus parses");
    let cases = corpus["phases"]["established"]
        .as_array()
        .expect("established cases");
    let probes = probes_for("established");
    for name in [
        "api_ticket_create_no_key",
        "api_ticket_reply_no_key",
        "api_ticket_attachment_no_key",
        "api_ticket_close_no_key",
        "api_resolution_confirm_no_key",
        "api_resolution_still_need_help_no_key",
        "api_feedback_no_key",
    ] {
        reset_portal_storage(root.path(), "established");
        let before = ledger_contents(root.path());
        let case = cases
            .iter()
            .find(|case| case["name"] == name)
            .unwrap_or_else(|| panic!("no established corpus case for {name}"));
        let probe = probes
            .iter()
            .find(|probe| probe.name == name)
            .unwrap_or_else(|| panic!("no typed probe for {name}"));
        replay_case("established", case, &probe.probe, &portal, root.path()).await;
        assert_eq!(
            ledger_contents(root.path()),
            before,
            "{name} ledger contents"
        );
        assert!(
            portal.log().is_empty(),
            "{name} must not contact the portal"
        );
    }

    for name in ["api_draft", "api_register"] {
        let case = cases
            .iter()
            .find(|case| case["name"] == name)
            .unwrap_or_else(|| panic!("no established corpus case for {name}"));
        let probe = probes
            .iter()
            .find(|probe| probe.name == name)
            .unwrap_or_else(|| panic!("no typed probe for {name}"));
        assert_eq!(probe.probe.key, None, "{name} does not require a key");
        replay_case("established", case, &probe.probe, &portal, root.path()).await;
    }

    let name = "api_ticket_create_malformed_key";
    let case = cases
        .iter()
        .find(|case| case["name"] == name)
        .expect("malformed key case");
    let probe = probes
        .iter()
        .find(|probe| probe.name == name)
        .expect("malformed key probe");
    replay_case("established", case, &probe.probe, &portal, root.path()).await;
    assert!(
        portal
            .log()
            .iter()
            .any(|request| request.method == "POST" && request.path == "/api/tickets"),
        "a malformed non-empty key still reaches the mutation"
    );
}

#[tokio::test]
async fn draft_json_attach_is_rejected_and_json_capture_stays_local() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let _guard = install_route_portal(&portal);

    portal.clear_log();
    let (status, body) = shell_response(
        root.path(),
        json_request(
            "/app/support/api/draft",
            serde_json::json!({"verb":"attach","payload":{}}),
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(body["reason_code"], "invalid_request_value");
    assert!(portal.log().is_empty(), "a draft never contacts the portal");

    portal.clear_log();
    let (status, body) = shell_response(
        root.path(),
        json_request(
            "/app/support/api/draft",
            serde_json::json!({
                "verb":"create",
                "payload":{"subject":"draft subject"},
                "diagnostics_snapshot":{"source":"derivation-test"},
            }),
        ),
    )
    .await;
    assert_eq!(status, 200);
    let draft_id = body["draft_id"].as_str().expect("draft id");
    let event = draft_event(root.path(), draft_id);
    assert_eq!(event["kind"], "support_draft");
    assert!(event["ts"].is_i64());
    assert_eq!(event["draft_id"], draft_id);
    assert!(event["captured_day"].as_str().is_some());
    assert_eq!(event["verb"], "create");
    assert_eq!(
        event["payload"],
        serde_json::json!({"subject":"draft subject"})
    );
    assert_eq!(
        event["diagnostics_snapshot"],
        serde_json::json!({"source":"derivation-test"})
    );
    assert!(portal.log().is_empty(), "a successful draft stays local");
}

#[tokio::test]
async fn multipart_draft_refusals_happy_path_and_json_fallthrough() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let _guard = install_route_portal(&portal);

    let oversized = vec![b'x'; PortalClient::MAX_ATTACHMENT_SIZE as usize + 1];
    let refusals = vec![
        (
            "wrong verb",
            multipart_request(
                "/app/support/api/draft",
                None,
                &[("verb", "create"), ("ticket_id", "7")],
                Some((Some("draft.txt"), b"draft", "text/plain")),
            ),
            "invalid_request_value",
        ),
        (
            "non-integer ticket",
            multipart_request(
                "/app/support/api/draft",
                None,
                &[("verb", "attach"), ("ticket_id", "not-a-ticket")],
                Some((Some("draft.txt"), b"draft", "text/plain")),
            ),
            "invalid_request_value",
        ),
        (
            "empty filename",
            multipart_request(
                "/app/support/api/draft",
                None,
                &[("verb", "attach"), ("ticket_id", "7")],
                Some((Some(""), b"draft", "text/plain")),
            ),
            "missing_required_field",
        ),
        (
            "unsupported suffix",
            multipart_request(
                "/app/support/api/draft",
                None,
                &[("verb", "attach"), ("ticket_id", "7")],
                Some((Some("draft.exe"), b"draft", "application/octet-stream")),
            ),
            "invalid_request_value",
        ),
        (
            "oversized body",
            multipart_request(
                "/app/support/api/draft",
                None,
                &[("verb", "attach"), ("ticket_id", "7")],
                Some((Some("draft.txt"), &oversized, "text/plain")),
            ),
            "invalid_request_value",
        ),
    ];
    for (name, request, reason) in refusals {
        portal.clear_log();
        let (status, body) = shell_response(root.path(), request).await;
        assert_eq!(status, 400, "{name}");
        assert_eq!(body["reason_code"], reason, "{name}");
        assert!(portal.log().is_empty(), "{name} must stay local");
    }

    portal.clear_log();
    let (status, body) = shell_response(
        root.path(),
        multipart_request(
            "/app/support/api/draft",
            None,
            &[("verb", "attach"), ("ticket_id", "7")],
            None,
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(body["reason_code"], "invalid_request_value");
    assert!(
        portal.log().is_empty(),
        "a no-file multipart body falls through locally"
    );

    portal.clear_log();
    let bytes = b"draft attachment bytes";
    let (status, body) = shell_response(
        root.path(),
        multipart_request(
            "/app/support/api/draft",
            None,
            &[("verb", "attach"), ("ticket_id", "7")],
            Some((Some("draft.txt"), bytes, "text/plain")),
        ),
    )
    .await;
    assert_eq!(status, 200);
    let event = draft_event(root.path(), body["draft_id"].as_str().expect("draft id"));
    assert_eq!(event["verb"], "attach");
    assert_eq!(event["payload"]["filename"], "draft.txt");
    assert_eq!(
        STANDARD
            .decode(
                event["payload"]["content_b64"]
                    .as_str()
                    .expect("base64 draft")
            )
            .expect("draft base64"),
        bytes
    );
    assert!(portal.log().is_empty(), "a multipart draft stays local");
}

#[tokio::test]
async fn attachment_refusals_precede_suffix_and_retry_resends_identical_bytes() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let _guard = install_route_portal(&portal);
    for (name, request, reason, detail) in [
        (
            "no file",
            multipart_request(
                "/app/support/api/tickets/7/attachments",
                KEY,
                &[("index", "0")],
                None,
            ),
            "missing_required_field",
            "No file provided",
        ),
        (
            "empty filename",
            multipart_request(
                "/app/support/api/tickets/7/attachments",
                KEY,
                &[("index", "0")],
                Some((Some(""), b"nope", "application/octet-stream")),
            ),
            "missing_required_field",
            "No filename",
        ),
        (
            "non-integer index before suffix",
            multipart_request(
                "/app/support/api/tickets/7/attachments",
                KEY,
                &[("index", "abc")],
                Some((Some("bad.exe"), b"nope", "application/octet-stream")),
            ),
            "invalid_request_value",
            "index must be an integer",
        ),
        (
            "negative index before suffix",
            multipart_request(
                "/app/support/api/tickets/7/attachments",
                KEY,
                &[("index", "-1")],
                Some((Some("bad.exe"), b"nope", "application/octet-stream")),
            ),
            "invalid_request_value",
            "index must be non-negative",
        ),
    ] {
        portal.clear_log();
        let (status, body) = shell_response(root.path(), request).await;
        assert_eq!(status, 400, "{name}");
        assert_eq!(body["reason_code"], reason, "{name}");
        assert_eq!(body["detail"], detail, "{name}");
        assert!(portal.log().is_empty(), "{name} must precede portal work");
    }

    portal.clear_log();
    portal.clear_bodies();
    let (status, _) = shell_response(
        root.path(),
        multipart_request(
            "/app/support/api/tickets/7/attachments",
            KEY,
            &[("index", "0")],
            Some((
                Some("suffix-proves-spool.txt"),
                b"attachment body",
                "text/plain",
            )),
        ),
    )
    .await;
    assert_eq!(status, 201, "the spooled suffix reaches attach_file");
    let filename = portal
        .log()
        .into_iter()
        .find(|request| request.method == "POST" && request.path == "/api/tickets/7/attachments")
        .and_then(|request| request.multipart)
        .and_then(|parts| parts.into_iter().next())
        .map(|part| part.filename)
        .expect("attachment part");
    assert_eq!(
        filename, "suffix-proves-spool.txt",
        "the attachment upload preserves the original filename"
    );

    let retry_portal = corpus_route_portal();
    let retry_root = phase_root("established", Some(&retry_portal));
    let _retry_guard = install_route_portal(&retry_portal);
    retry_portal.clear_log();
    retry_portal.clear_bodies();
    retry_portal.override_route(
        "POST",
        "/api/tickets/7/attachments",
        vec![
            RouteReply {
                status: 401,
                body: r#"{"error":"tos_changed"}"#.into(),
                content_type: "application/json".into(),
            },
            RouteReply {
                status: 200,
                body: r#"{"attachment_id":303,"status":"open","filename":"retry.txt"}"#.into(),
                content_type: "application/json".into(),
            },
        ],
    );
    let (status, _) = shell_response(
        retry_root.path(),
        multipart_request(
            "/app/support/api/tickets/7/attachments",
            KEY,
            &[("index", "0")],
            Some((Some("retry.txt"), b"same retry bytes", "text/plain")),
        ),
    )
    .await;
    assert_eq!(status, 201);
    let attachment_parts = retry_portal
        .log()
        .into_iter()
        .filter(|request| request.method == "POST" && request.path == "/api/tickets/7/attachments")
        .map(|request| request.multipart.expect("multipart capture"))
        .collect::<Vec<_>>();
    assert_eq!(attachment_parts.len(), 2);
    assert_eq!(attachment_parts[0], attachment_parts[1]);
}

#[tokio::test]
async fn write_mutations_project_tombstones_and_preserve_active_ticket_bodies() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let _guard = install_route_portal(&portal);
    let closed = serde_json::json!({
        "ticket_id": 7,
        "status": "closed",
        "closed_at": "2026-03-01T00:00:00Z",
        "close_scheduled_at": "2026-03-08T00:00:00Z",
        "reason_code": "resolved",
        "subject": "a tombstone must omit this",
        "messages": [{"body": "and this"}],
    });
    portal.override_route(
        "POST",
        "/api/tickets/7/close",
        vec![RouteReply {
            status: 200,
            body: closed.to_string(),
            content_type: "application/json".to_owned(),
        }],
    );
    let (status, body) = shell_response(
        root.path(),
        Request::post("/app/support/api/tickets/7/close")
            .header("Host", "127.0.0.1")
            .header("Idempotency-Key", KEY.expect("corpus action key"))
            .body(Body::empty())
            .expect("close request"),
    )
    .await;
    assert_eq!(status, 201);
    assert_eq!(
        body,
        serde_json::json!({
            "ticket_id": 7,
            "status": "closed",
            "closed_at": "2026-03-01T00:00:00Z",
            "close_scheduled_at": "2026-03-08T00:00:00Z",
            "reason_code": "resolved",
        })
    );

    let active = serde_json::json!({
        "ticket_id": 101,
        "status": "open",
        "subject": "an active ticket keeps every field",
        "created_at": "2026-03-02T00:00:00Z",
        "internal": {"kept": true},
    });
    portal.override_route(
        "POST",
        "/api/tickets",
        vec![RouteReply {
            status: 200,
            body: active.to_string(),
            content_type: "application/json".to_owned(),
        }],
    );
    let (status, body) = shell_response(
        root.path(),
        keyed_json_request(
            "/app/support/api/tickets",
            serde_json::json!({
                "subject": "new ticket",
                "description": "new description",
                "auto_context": false,
            }),
        ),
    )
    .await;
    assert_eq!(status, 201);
    assert_eq!(body, active);
}

#[tokio::test]
async fn ticket_creation_auto_context_prefers_caller_context() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let _guard = install_route_portal(&portal);
    portal.clear_log();
    portal.clear_bodies();
    portal.override_route(
        "POST",
        "/api/tickets",
        vec![RouteReply {
            status: 200,
            body: r#"{"ticket_id":101,"status":"open"}"#.to_owned(),
            content_type: "application/json".to_owned(),
        }],
    );
    let (status, _) = shell_response(
        root.path(),
        keyed_json_request(
            "/app/support/api/tickets",
            serde_json::json!({
                "subject": "context merge",
                "description": "caller context wins",
                "auto_context": true,
                "user_context": {"platform": "caller-supplied platform"},
            }),
        ),
    )
    .await;
    assert_eq!(status, 201);
    let request_index = portal
        .log()
        .iter()
        .enumerate()
        .find_map(|(index, request)| {
            (request.method == "POST" && request.path == "/api/tickets").then_some(index)
        })
        .expect("ticket creation reached portal");
    let sent: Value = serde_json::from_slice(&portal.bodies()[request_index])
        .expect("portal ticket creation body is JSON");
    assert_eq!(
        sent["user_context"]["platform"], "caller-supplied platform",
        "the caller replaces collect_all's always-present platform diagnostic"
    );
}

#[tokio::test]
async fn write_path_non_integer_ticket_id_returns_http_not_found() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let _guard = install_route_portal(&portal);
    portal.clear_log();
    let (status, body) = shell_response(
        root.path(),
        json_request(
            "/app/support/api/tickets/not-an-integer/reply",
            serde_json::json!({"content":"ignored"}),
        ),
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(body["reason_code"], "http_error");
    assert!(
        portal.log().is_empty(),
        "an invalid path id must not reach the portal"
    );
}

#[tokio::test]
async fn established_validation_refusals_and_drain_cases_match_the_corpus() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let _guard = install_route_portal(&portal);
    let corpus: Value = serde_json::from_str(CORPUS).expect("support corpus parses");
    let cases = corpus["phases"]["established"]
        .as_array()
        .expect("established cases");
    let probes = probes_for("established");
    for name in [
        "api_draft_unknown_verb",
        "api_draft_missing_payload",
        "api_ticket_create_missing_subject",
        "api_feedback_missing_body",
        "api_ticket_attachment_bad_suffix",
        "drain_on_missing_key_refusal",
        "drain_on_page_request",
        "routing_404_non_integer_ticket",
    ] {
        let case = cases
            .iter()
            .find(|case| case["name"] == name)
            .unwrap_or_else(|| panic!("no established corpus case for {name}"));
        let probe = probes
            .iter()
            .find(|probe| probe.name == name)
            .unwrap_or_else(|| panic!("no typed probe for {name}"));
        replay_case("established", case, &probe.probe, &portal, root.path()).await;
    }

    reset_portal_storage(root.path(), "established");
    portal.clear_log();
    let attach_json = CorpusProbe {
        name: "draft_json_attach",
        method: "POST",
        path: "/app/support/api/draft",
        kind: "probe",
        body: Some(r#"{"verb":"attach","payload":{}}"#),
        key: None,
        file: None,
    };
    let response = super::routes(root.path().to_path_buf())
        .oneshot(corpus_request(&attach_json))
        .await
        .expect("draft response");
    assert_eq!(response.status().as_u16(), 400);
    let body: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("draft body"),
    )
    .expect("draft error json");
    assert_eq!(body["reason_code"], "invalid_request_value");
    assert_eq!(
        body["detail"],
        "verb must be create|feedback|reply|close|resolved|still_need_help and payload must be an object"
    );
    assert!(portal.log().is_empty());

    let all_established = [
        "page_index",
        "page_workspace",
        "page_background",
        "static_support_js",
        "api_config",
        "api_tickets_list",
        "api_tickets_list_status",
        "api_ticket_get",
        "api_tickets_closed",
        "api_articles",
        "api_article",
        "api_announcements",
        "api_diagnostics",
        "api_badge_count",
        "routing_404_non_integer_ticket",
        "api_draft",
        "api_register",
        "api_ticket_create",
        "api_ticket_reply",
        "api_ticket_attachment",
        "api_ticket_close",
        "api_resolution_confirm",
        "api_resolution_still_need_help",
        "api_feedback",
        "api_ticket_create_no_key",
        "api_ticket_reply_no_key",
        "api_ticket_attachment_no_key",
        "api_ticket_close_no_key",
        "api_resolution_confirm_no_key",
        "api_resolution_still_need_help_no_key",
        "api_feedback_no_key",
        "api_draft_unknown_verb",
        "api_draft_missing_payload",
        "api_ticket_create_missing_subject",
        "api_feedback_missing_body",
        "api_ticket_attachment_bad_suffix",
        "api_ticket_create_malformed_key",
        "drain_on_missing_key_refusal",
        "drain_on_page_request",
    ];
    assert_eq!(all_established.len(), 39);
    assert_eq!(cases.len(), all_established.len());
    assert!(
        cases
            .iter()
            .all(|case| { all_established.contains(&case["name"].as_str().expect("case name")) })
    );
}

#[tokio::test]
async fn unestablished_write_cases_are_session_gated_before_their_handlers() {
    let root = phase_root("unestablished", None);
    let corpus: Value = serde_json::from_str(CORPUS).expect("support corpus parses");
    let names = [
        "api_draft",
        "api_register",
        "api_ticket_create",
        "api_ticket_reply",
        "api_ticket_attachment",
        "api_ticket_close",
        "api_resolution_confirm",
        "api_resolution_still_need_help",
        "api_feedback",
    ];
    assert_eq!(names.len(), 9);
    let cases = corpus["phases"]["unestablished"]
        .as_array()
        .expect("unestablished cases");
    let probes = probes_for("unestablished");
    for name in names {
        let case = cases
            .iter()
            .find(|case| case["name"] == name)
            .unwrap_or_else(|| panic!("no unestablished corpus case for {name}"));
        assert_eq!(case["response"]["status"], 302, "{name}");
        assert_eq!(case["response"]["headers"]["Location"], "/init", "{name}");
        assert_eq!(case["response"]["text_bytes"], 197, "{name}");
        let probe = probes
            .iter()
            .find(|probe| probe.name == name)
            .unwrap_or_else(|| panic!("no typed probe for {name}"));
        reset_portal_storage(root.path(), "unestablished");
        let response = solstone_core_convey_shell::router(root.path().to_path_buf())
            .oneshot(corpus_request(&probe.probe))
            .await
            .expect("router response");
        assert_recorded_response(
            response,
            &case["response"],
            case["normalized"].as_array().expect("normalized pointers"),
            "response.",
            "unestablished",
            name,
        )
        .await;
        assert_eq!(
            case["portal_requests"].as_array().unwrap().len(),
            0,
            "{name} must not contact the portal"
        );
    }
}

#[tokio::test]
async fn disabled_read_and_page_cases_match_the_corpus() {
    let portal = corpus_route_portal();
    let root = phase_root("disabled", Some(&portal));
    let corpus: Value = serde_json::from_str(CORPUS).expect("support corpus parses");
    let names = [
        "page_index",
        "page_workspace",
        "page_background",
        "static_support_js",
        "api_config",
        "api_tickets_list",
        "api_tickets_list_status",
        "api_ticket_get",
        "api_tickets_closed",
        "api_articles",
        "api_article",
        "api_announcements",
        "api_diagnostics",
        "api_badge_count",
        "routing_404_non_integer_ticket",
        "drain_on_feature_unavailable_refusal",
        "drain_on_local_read",
    ];
    let skipped_w2c_writes = [
        "api_draft",
        "api_register",
        "api_ticket_create",
        "api_ticket_reply",
        "api_ticket_attachment",
        "api_ticket_close",
        "api_resolution_confirm",
        "api_resolution_still_need_help",
        "api_feedback",
    ];
    let cases = corpus["phases"]["disabled"]
        .as_array()
        .expect("disabled cases");
    assert_eq!(names.len(), 17, "disabled cases driven");
    assert_eq!(skipped_w2c_writes.len(), 9, "W2c writes skipped");
    assert_eq!(cases.len(), names.len() + skipped_w2c_writes.len());

    let detail = |name: &str| {
        cases
            .iter()
            .find(|case| case["name"] == name)
            .unwrap_or_else(|| panic!("no disabled corpus case for {name}"))["response"]["body"]
                ["detail"]
                .as_str()
                .unwrap()
    };
    assert_eq!(detail("api_badge_count"), "Support is not enabled");
    assert_eq!(
        detail("drain_on_feature_unavailable_refusal"),
        "Support is not enabled"
    );
    for name in [
        "api_tickets_list",
        "api_tickets_list_status",
        "api_ticket_get",
        "api_tickets_closed",
        "api_articles",
        "api_article",
        "api_announcements",
    ] {
        assert_eq!(detail(name), "Support is disabled", "{name}");
    }

    let probes = probes_for("disabled");
    for name in names {
        let case = cases
            .iter()
            .find(|case| case["name"] == name)
            .unwrap_or_else(|| panic!("no disabled corpus case for {name}"));
        let probe = probes
            .iter()
            .find(|probe| probe.name == name)
            .unwrap_or_else(|| panic!("no typed probe for {name}"));
        replay_case("disabled", case, &probe.probe, &portal, root.path()).await;
    }
}

#[tokio::test]
async fn unregistered_read_and_page_cases_register_for_every_portal_read() {
    let portal = corpus_route_portal();
    let root = phase_root("unregistered", Some(&portal));
    let corpus: Value = serde_json::from_str(CORPUS).expect("support corpus parses");
    let names = [
        "page_index",
        "page_workspace",
        "page_background",
        "static_support_js",
        "api_config",
        "api_tickets_list",
        "api_tickets_list_status",
        "api_ticket_get",
        "api_tickets_closed",
        "api_articles",
        "api_article",
        "api_announcements",
        "api_diagnostics",
        "api_badge_count",
        "routing_404_non_integer_ticket",
    ];
    let skipped_w2c_writes = [
        "api_draft",
        "api_register",
        "api_ticket_create",
        "api_ticket_reply",
        "api_ticket_attachment",
        "api_ticket_close",
        "api_resolution_confirm",
        "api_resolution_still_need_help",
        "api_feedback",
    ];
    let local_only = [
        "page_index",
        "page_workspace",
        "page_background",
        "static_support_js",
        "api_config",
        "api_diagnostics",
        "routing_404_non_integer_ticket",
    ];
    let cases = corpus["phases"]["unregistered"]
        .as_array()
        .expect("unregistered cases");
    let established_cases = corpus["phases"]["established"]
        .as_array()
        .expect("established cases");
    assert_eq!(names.len(), 15, "unregistered cases driven");
    assert_eq!(skipped_w2c_writes.len(), 9, "W2c writes skipped");
    assert_eq!(cases.len(), names.len() + skipped_w2c_writes.len());

    let probes = probes_for("unregistered");
    for name in names {
        let case = cases
            .iter()
            .find(|case| case["name"] == name)
            .unwrap_or_else(|| panic!("no unregistered corpus case for {name}"));
        let probe = probes
            .iter()
            .find(|probe| probe.name == name)
            .unwrap_or_else(|| panic!("no typed probe for {name}"));
        replay_case("unregistered", case, &probe.probe, &portal, root.path()).await;

        let actual = portal
            .log()
            .into_iter()
            .map(|request| {
                serde_json::json!({"method":request.method,"path":request.path,"had_idempotency_key":request.idempotency_key.is_some(),"had_authorization":request.had_authorization,"had_dpop":request.had_dpop})
            })
            .collect::<Vec<_>>();
        if local_only.contains(&name) {
            assert!(actual.is_empty(), "{name} must not contact the portal");
            continue;
        }

        let established = established_cases
            .iter()
            .find(|case| case["name"] == name)
            .unwrap_or_else(|| panic!("no established corpus case for {name}"));
        let expected = case["portal_requests"].as_array().unwrap();
        let established_request = established["portal_requests"].as_array().unwrap();
        assert_eq!(actual.len(), 3, "{name} must register before its read");
        assert_eq!(&actual[..2], &expected[..2], "{name} registration requests");
        assert_eq!(
            &actual[2..],
            established_request.as_slice(),
            "{name} final read must match established"
        );
    }
}

fn seed_pending_acknowledgement(root: &Path, parent_action_id: &str) {
    let ledger = Ledger::new(root.join("apps/support/portal"));
    let fields = serde_json::Map::from_iter([("ticket_id".to_owned(), serde_json::json!(7))]);
    let record = ledger
        .begin_operation(
            parent_action_id,
            "close",
            &fields,
            "anonymous",
            0,
            chrono::Utc::now(),
        )
        .expect("seed operation");
    let record = ledger
        .mark_in_progress(&record, chrono::Utc::now())
        .expect("seed in progress");
    ledger
        .mark_completed(&record, Some("corpus-remote-operation"), chrono::Utc::now())
        .expect("seed completed");
}

async fn support_route_response(root: &Path, path: &str) -> (u16, String, Vec<u8>) {
    let response = super::routes(root.to_path_buf())
        .oneshot(
            Request::get(path)
                .header("Host", "127.0.0.1")
                .body(Body::empty())
                .expect("route request"),
        )
        .await
        .expect("support route response");
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .expect("content type")
        .to_str()
        .expect("content type text")
        .to_owned();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body")
        .to_vec();
    (status, content_type, body)
}

#[tokio::test]
async fn drain_acknowledges_before_a_portal_backed_handler() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let _guard = install_route_portal(&portal);
    reset_portal_storage(root.path(), "established");
    seed_pending_acknowledgement(root.path(), "drain-before");
    portal.clear_log();

    let _ = support_route_response(root.path(), "/app/support/api/tickets").await;
    assert_eq!(
        portal
            .log()
            .iter()
            .map(|request| (request.method.as_str(), request.path.as_str()))
            .collect::<Vec<_>>(),
        vec![("POST", "/api/idempotency/ack"), ("GET", "/api/tickets"),]
    );
}

#[tokio::test]
async fn empty_drain_makes_no_portal_request_or_keypair() {
    let portal = corpus_route_portal();
    let root = phase_root("unregistered", Some(&portal));
    let _guard = install_route_portal(&portal);
    reset_portal_storage(root.path(), "unregistered");
    portal.clear_log();

    let _ = support_route_response(root.path(), "/app/support/api/config").await;
    assert!(portal.log().is_empty());
    assert!(
        !root.path().join("apps/support/portal/keypair.pem").exists(),
        "an empty drain must not generate an identity"
    );
}

#[tokio::test]
async fn drain_failure_does_not_change_any_registered_route_response() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let _guard = install_route_portal(&portal);
    let paths = [
        "/app/support/",
        "/app/support/workspace",
        "/app/support/background",
        "/app/support/static/support.js",
        "/app/support/api/config",
        "/app/support/api/tickets",
        "/app/support/api/tickets/closed",
        "/app/support/api/tickets/7",
        "/app/support/api/articles",
        "/app/support/api/articles/getting-started",
        "/app/support/api/announcements",
        "/app/support/api/diagnostics",
        "/app/support/api/badge-count",
    ];
    assert_eq!(paths.len(), 13);

    for (index, path) in paths.into_iter().enumerate() {
        reset_portal_storage(root.path(), "established");
        portal.clear_log();
        let baseline = support_route_response(root.path(), path).await;

        reset_portal_storage(root.path(), "established");
        portal.clear_log();
        portal.override_route(
            "POST",
            "/api/idempotency/ack",
            vec![
                RouteReply {
                    status: 500,
                    body: "drain failed".into(),
                    content_type: "text/plain; charset=utf-8".into(),
                },
                RouteReply {
                    status: 500,
                    body: "drain failed".into(),
                    content_type: "text/plain; charset=utf-8".into(),
                },
            ],
        );
        seed_pending_acknowledgement(root.path(), &format!("drain-failure-{index}"));
        let with_failed_drain = support_route_response(root.path(), path).await;
        assert_eq!(with_failed_drain, baseline, "{path}");
    }
}

#[tokio::test]
async fn shell_router_applies_the_session_gate_to_support_config() {
    let root = phase_root("unestablished", None);
    let corpus: Value = serde_json::from_str(CORPUS).expect("support corpus parses");
    let case = corpus["phases"]["unestablished"]
        .as_array()
        .expect("unestablished cases")
        .iter()
        .find(|case| case["name"] == "api_config")
        .expect("unestablished api config case");
    let response = solstone_core_convey_shell::router(root.path().to_path_buf())
        .oneshot(
            Request::get("/app/support/api/config")
                .header("Host", "127.0.0.1")
                .body(Body::empty())
                .expect("config request"),
        )
        .await
        .expect("shell response");
    assert_eq!(response.status().as_u16(), 302);
    assert_eq!(response.headers().get("location").unwrap(), "/init");
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/html; charset=utf-8"
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("redirect body");
    assert_eq!(body.len(), 197);
    assert_eq!(
        format!("{:x}", Sha256::digest(&body)),
        case["response"]["text_sha256"].as_str().unwrap()
    );
    assert_eq!(
        std::str::from_utf8(&body).unwrap(),
        case["response"]["text"].as_str().unwrap()
    );
}

#[tokio::test]
async fn support_static_and_closed_routes_take_precedence() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let _guard = install_route_portal(&portal);

    portal.clear_log();
    let (status, content_type, body) =
        support_route_response(root.path(), "/app/support/api/tickets/closed").await;
    assert_eq!(status, 200);
    assert_eq!(content_type, "application/json");
    assert_eq!(
        portal
            .log()
            .iter()
            .map(|request| (request.method.as_str(), request.path.as_str()))
            .collect::<Vec<_>>(),
        vec![("GET", "/api/tickets/closed")]
    );
    assert!(
        serde_json::from_slice::<Value>(&body)
            .unwrap()
            .get("next_cursor")
            .is_some()
    );

    let response = super::routes(root.path().to_path_buf())
        .oneshot(
            Request::get("/app/support/static/support.js")
                .header("Host", "127.0.0.1")
                .body(Body::empty())
                .expect("static request"),
        )
        .await
        .expect("static response");
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/javascript; charset=utf-8"
    );
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("static body")
            .as_ref(),
        super::SUPPORT_JS
    );
}

#[tokio::test]
async fn support_bare_path_permanently_redirects_to_the_trailing_slash() {
    let portal = corpus_route_portal();
    let _guard = install_route_portal(&portal);
    let root = TempDir::new().expect("redirect root");
    let response = super::routes(root.path().to_path_buf())
        .oneshot(
            Request::get("/app/support")
                .header("Host", "127.0.0.1")
                .body(Body::empty())
                .expect("redirect request"),
        )
        .await
        .expect("redirect response");
    assert_eq!(response.status().as_u16(), 308);
    assert_eq!(response.headers().get("location").unwrap(), "/app/support/");
}

#[tokio::test]
async fn gate_phase_ticket_id_divergences_are_named_and_recorded() {
    let corpus: Value = serde_json::from_str(CORPUS).expect("support corpus parses");
    for (phase, case_name, fixture_status, native_status) in NATIVE_GATE_PHASE_ROUTING_DIVERGENCES {
        let case = corpus["phases"][phase]
            .as_array()
            .expect("phase cases")
            .iter()
            .find(|case| case["name"] == case_name)
            .unwrap_or_else(|| panic!("missing {phase} {case_name}"));
        assert_eq!(
            case["response"]["status"].as_u64().unwrap() as u16,
            fixture_status,
            "fixture {phase} {case_name}"
        );

        let root = phase_root(phase, None);
        let response = solstone_core_convey_shell::router(root.path().to_path_buf())
            .oneshot(
                Request::get("/app/support/api/tickets/notanint")
                    .header("Host", "127.0.0.1")
                    .body(Body::empty())
                    .expect("ticket request"),
            )
            .await
            .expect("shell response");
        assert_ne!(
            fixture_status, native_status,
            "named divergence must be real"
        );
        assert_eq!(
            response.status().as_u16(),
            native_status,
            "{phase} {case_name}: fixture {fixture_status}, native {native_status}"
        );
    }
}

fn confirm_request(draft_id: &str) -> Request<Body> {
    json_request(
        "/app/support/api/draft/confirm",
        serde_json::json!({"draft_id": draft_id}),
    )
}

fn cancel_request(draft_id: &str) -> Request<Body> {
    json_request(
        "/app/support/api/draft/cancel",
        serde_json::json!({"draft_id": draft_id}),
    )
}

fn write_locator(root: &Path, draft_id: &str, day: &str) {
    let path = root
        .join("chronicle/health/support-drafts")
        .join(format!("{draft_id}.json"));
    std::fs::create_dir_all(path.parent().expect("locator parent")).expect("locator parent");
    std::fs::write(path, format!("{{\"captured_day\":\"{day}\"}}\n")).expect("write locator");
}

fn write_draft_event(root: &Path, day: &str, segment: &str, event: &Value) {
    let path = root
        .join("chronicle")
        .join(day)
        .join("support-drafts")
        .join(segment)
        .join("support-drafts.jsonl");
    std::fs::create_dir_all(path.parent().expect("draft parent")).expect("draft parent");
    let mut contents = std::fs::read_to_string(&path).unwrap_or_default();
    contents.push_str(&event.to_string());
    contents.push('\n');
    std::fs::write(path, contents).expect("write draft");
}

fn support_draft_line(draft_id: &str, day: &str, verb: &str, payload: Value) -> Value {
    serde_json::json!({
        "kind": "support_draft",
        "ts": 1,
        "draft_id": draft_id,
        "captured_day": day,
        "verb": verb,
        "payload": payload,
        "diagnostics_snapshot": null,
    })
}

fn outcome_mark_path(root: &Path, draft_id: &str) -> PathBuf {
    root.join("chronicle/health/support-drafts")
        .join(format!("{draft_id}.outcome.json"))
}

fn draft_file_for_draft(root: &Path, draft_id: &str) -> PathBuf {
    let event = draft_event(root, draft_id);
    let day = event["captured_day"].as_str().expect("captured day");
    let drafts = root.join("chronicle").join(day).join("support-drafts");
    for segment in std::fs::read_dir(drafts)
        .expect("support-draft segments")
        .flatten()
    {
        let path = segment.path().join("support-drafts.jsonl");
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        if contents.lines().any(|line| {
            serde_json::from_str::<Value>(line)
                .ok()
                .is_some_and(|event| event["draft_id"] == draft_id)
        }) {
            return path;
        }
    }
    panic!("draft file for {draft_id}");
}

fn mutation_keys(portal: &RoutePortal) -> Vec<String> {
    portal
        .log()
        .into_iter()
        .filter(|request| request.method == "POST" && request.path != "/api/idempotency/ack")
        .filter_map(|request| request.idempotency_key)
        .collect()
}

async fn capture_create(root: &Path, payload: Value) -> String {
    let (status, body) = shell_response(
        root,
        json_request(
            "/app/support/api/draft",
            serde_json::json!({
                "verb": "create",
                "payload": payload,
            }),
        ),
    )
    .await;
    assert_eq!(status, 200);
    body["draft_id"].as_str().expect("draft id").to_owned()
}

#[tokio::test]
async fn confirm_create_submits_and_old_chat_paths_are_gone() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let _guard = install_route_portal(&portal);
    portal.clear_log();
    let draft_id = capture_create(
        root.path(),
        serde_json::json!({"subject":"draft subject","description":"draft body"}),
    )
    .await;
    assert!(
        portal
            .log()
            .iter()
            .all(|request| request.method != "POST" || request.path == "/api/idempotency/ack"),
        "capture stays local"
    );

    portal.clear_log();
    portal.clear_bodies();
    let (status, body) = shell_response(root.path(), confirm_request(&draft_id)).await;
    assert_eq!(status, 200);
    assert_eq!(body["ok"], true);
    assert_eq!(body["outcome"], "submitted");
    assert_eq!(body["ticket_id"], 101);
    let tickets = portal
        .log()
        .into_iter()
        .filter(|request| request.method == "POST" && request.path == "/api/tickets")
        .collect::<Vec<_>>();
    assert_eq!(tickets.len(), 1);
    assert!(tickets[0].idempotency_key.is_some());

    for path in [
        "/api/chat/support/draft/confirm",
        "/api/chat/support/draft/cancel",
    ] {
        let response = super::routes(root.path().to_path_buf())
            .oneshot(json_request(path, serde_json::json!({})))
            .await
            .expect("old path response");
        assert_eq!(response.status().as_u16(), 404, "{path}");
    }
}

#[tokio::test]
async fn confirm_lookup_walks_only_the_locator_day() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let _guard = install_route_portal(&portal);

    let (status, body) = shell_response(root.path(), confirm_request("missing")).await;
    assert_eq!(status, 200);
    assert_eq!(body["outcome"], "not_found");

    write_locator(root.path(), "locator-only", "20260815");
    let (status, body) = shell_response(root.path(), confirm_request("locator-only")).await;
    assert_eq!(status, 200);
    assert_eq!(body["outcome"], "not_found");

    write_locator(root.path(), "no-match", "20260815");
    write_draft_event(
        root.path(),
        "20260815",
        "100000_300",
        &support_draft_line(
            "other-id",
            "20260815",
            "create",
            serde_json::json!({"subject":"other","description":"other"}),
        ),
    );
    let (status, body) = shell_response(root.path(), confirm_request("no-match")).await;
    assert_eq!(status, 200);
    assert_eq!(body["outcome"], "not_found");

    write_locator(root.path(), "first-id", "20260816");
    write_locator(root.path(), "second-id", "20260816");
    write_draft_event(
        root.path(),
        "20260816",
        "100000_300",
        &support_draft_line(
            "first-id",
            "20260816",
            "create",
            serde_json::json!({"subject":"first subject","description":"first"}),
        ),
    );
    write_draft_event(
        root.path(),
        "20260816",
        "100000_300",
        &support_draft_line(
            "second-id",
            "20260816",
            "create",
            serde_json::json!({"subject":"second subject","description":"second"}),
        ),
    );
    portal.clear_log();
    portal.clear_bodies();
    let (status, body) = shell_response(root.path(), confirm_request("second-id")).await;
    assert_eq!(status, 200);
    assert_eq!(body["outcome"], "submitted");
    let create = portal
        .log()
        .into_iter()
        .position(|request| request.method == "POST" && request.path == "/api/tickets")
        .expect("second draft reached portal");
    let sent: Value = serde_json::from_slice(&portal.bodies()[create]).expect("create body");
    assert_eq!(sent["subject"], "second subject");
}

#[tokio::test]
async fn confirm_and_cancel_marks_are_durable_and_replay_the_same_key() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let _guard = install_route_portal(&portal);

    let submitted = capture_create(
        root.path(),
        serde_json::json!({"subject":"once","description":"once"}),
    )
    .await;
    portal.clear_log();
    let (status, first) = shell_response(root.path(), confirm_request(&submitted)).await;
    assert_eq!(status, 200);
    assert_eq!(first["outcome"], "submitted");
    let first_keys = mutation_keys(&portal);
    assert_eq!(first_keys.len(), 1);

    let (status, again) = shell_response(root.path(), confirm_request(&submitted)).await;
    assert_eq!(status, 200);
    assert_eq!(again["outcome"], "already_submitted");
    assert_eq!(mutation_keys(&portal), first_keys);

    std::fs::remove_file(outcome_mark_path(root.path(), &submitted)).expect("delete mark");
    let (status, replay) = shell_response(root.path(), confirm_request(&submitted)).await;
    assert_eq!(status, 200);
    assert_eq!(replay["outcome"], "submitted");
    assert_eq!(replay["ticket_id"], first["ticket_id"]);
    let replay_keys = mutation_keys(&portal);
    assert_eq!(replay_keys.len(), 2);
    assert_eq!(replay_keys[0], replay_keys[1]);

    let other = capture_create(
        root.path(),
        serde_json::json!({"subject":"other","description":"other"}),
    )
    .await;
    let (status, _) = shell_response(root.path(), confirm_request(&other)).await;
    assert_eq!(status, 200);
    let keys = mutation_keys(&portal);
    assert_ne!(keys[0], keys[keys.len() - 1]);

    let cancelled = capture_create(
        root.path(),
        serde_json::json!({"subject":"cancel","description":"cancel"}),
    )
    .await;
    portal.clear_log();
    let (status, body) = shell_response(root.path(), cancel_request(&cancelled)).await;
    assert_eq!(status, 200);
    assert_eq!(body["outcome"], "cancelled");
    assert!(mutation_keys(&portal).is_empty());
    let (status, body) = shell_response(root.path(), cancel_request(&cancelled)).await;
    assert_eq!(status, 200);
    assert_eq!(body["outcome"], "cancelled");
    assert!(mutation_keys(&portal).is_empty());

    portal.clear_log();
    let (status, body) = shell_response(root.path(), cancel_request(&submitted)).await;
    assert_eq!(status, 200);
    assert_eq!(body["outcome"], "already_submitted");
    assert!(mutation_keys(&portal).is_empty());
}

#[tokio::test]
async fn confirm_dispatches_each_verb_from_the_stored_payload() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let _guard = install_route_portal(&portal);

    portal.clear_log();
    portal.clear_bodies();
    let first = capture_create(
        root.path(),
        serde_json::json!({
            "subject": "alpha",
            "description": "one",
            "auto_context": false,
            "user_context": {"only": "alpha"},
        }),
    )
    .await;
    let second = capture_create(
        root.path(),
        serde_json::json!({
            "subject": "beta",
            "description": "two",
            "auto_context": false,
            "user_context": {"only": "beta"},
            "anonymous": true,
        }),
    )
    .await;
    let (status, _) = shell_response(root.path(), confirm_request(&first)).await;
    assert_eq!(status, 200);
    let (status, _) = shell_response(root.path(), confirm_request(&second)).await;
    assert_eq!(status, 200);
    let creates = portal
        .log()
        .into_iter()
        .enumerate()
        .filter(|(_, request)| request.method == "POST" && request.path == "/api/tickets")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(creates.len(), 2);
    let alpha: Value = serde_json::from_slice(&portal.bodies()[creates[0]]).expect("alpha");
    let beta: Value = serde_json::from_slice(&portal.bodies()[creates[1]]).expect("beta");
    assert_eq!(alpha["user_context"], serde_json::json!({"only":"alpha"}));
    assert_eq!(beta["user_context"], serde_json::json!({"only":"beta"}));
    assert!(alpha.get("platform").is_none());
    assert!(alpha.get("recent_errors").is_none());
    assert!(beta.get("version").is_none());
    assert_ne!(
        mutation_keys(&portal)[0],
        mutation_keys(&portal)[mutation_keys(&portal).len() - 1]
    );

    let (status, body) = shell_response(
        root.path(),
        json_request(
            "/app/support/api/draft",
            serde_json::json!({"verb":"feedback","payload":{"body":"the flow"}}),
        ),
    )
    .await;
    assert_eq!(status, 200);
    portal.clear_log();
    let (status, _) = shell_response(
        root.path(),
        confirm_request(body["draft_id"].as_str().expect("feedback id")),
    )
    .await;
    assert_eq!(status, 200);
    assert!(
        portal
            .log()
            .iter()
            .any(|request| request.method == "POST" && request.path == "/api/tickets")
    );

    for (verb, path) in [
        ("reply", "/api/tickets/7/messages"),
        ("close", "/api/tickets/7/close"),
        ("resolved", "/api/tickets/7/resolution/confirm"),
        (
            "still_need_help",
            "/api/tickets/7/resolution/still-need-help",
        ),
    ] {
        let payload = if verb == "reply" {
            serde_json::json!({"ticket_id":7,"content":format!("reply {verb}")})
        } else {
            serde_json::json!({"ticket_id":7})
        };
        let (status, body) = shell_response(
            root.path(),
            json_request(
                "/app/support/api/draft",
                serde_json::json!({"verb":verb,"payload":payload}),
            ),
        )
        .await;
        assert_eq!(status, 200, "{verb}");
        portal.clear_log();
        let (status, _) = shell_response(
            root.path(),
            confirm_request(body["draft_id"].as_str().expect("id")),
        )
        .await;
        assert_eq!(status, 200, "{verb}");
        assert!(
            portal
                .log()
                .iter()
                .any(|request| request.method == "POST" && request.path == path),
            "{verb} {path}"
        );
    }

    let bytes_a = b"attach-a";
    let bytes_b = b"attach-b-different";
    for bytes in [bytes_a.as_slice(), bytes_b.as_slice()] {
        let (status, body) = shell_response(
            root.path(),
            multipart_request(
                "/app/support/api/draft",
                None,
                &[("verb", "attach"), ("ticket_id", "7")],
                Some((Some("draft.txt"), bytes, "text/plain")),
            ),
        )
        .await;
        assert_eq!(status, 200);
        portal.clear_log();
        let (status, _) = shell_response(
            root.path(),
            confirm_request(body["draft_id"].as_str().expect("attach id")),
        )
        .await;
        assert_eq!(status, 200);
        assert!(portal.log().iter().any(
            |request| request.method == "POST" && request.path == "/api/tickets/7/attachments"
        ));
    }
}

#[tokio::test]
async fn confirm_portal_failure_leaves_the_draft_open() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let _guard = install_route_portal(&portal);
    let draft_id = capture_create(
        root.path(),
        serde_json::json!({"subject":"fail","description":"fail"}),
    )
    .await;
    portal.override_route(
        "POST",
        "/api/tickets",
        vec![RouteReply {
            status: 500,
            body: "broken".to_owned(),
            content_type: "text/plain".to_owned(),
        }],
    );
    let (status, body) = shell_response(root.path(), confirm_request(&draft_id)).await;
    assert_eq!(status, 500);
    assert_ne!(body["outcome"], "submitted");
    assert_eq!(
        resolve_draft_outcome(root.path(), &draft_id).expect("outcome"),
        None
    );
    assert!(!outcome_mark_path(root.path(), &draft_id).exists());
}

#[tokio::test]
async fn confirm_and_cancel_validate_without_an_idempotency_header() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let _guard = install_route_portal(&portal);
    for path in [
        "/app/support/api/draft/confirm",
        "/app/support/api/draft/cancel",
    ] {
        let (status, body) =
            shell_response(root.path(), json_request(path, serde_json::json!({}))).await;
        assert_eq!(status, 400, "{path}");
        assert_eq!(body["reason_code"], "missing_required_field", "{path}");
    }
    let draft_id = capture_create(
        root.path(),
        serde_json::json!({"subject":"headerless","description":"headerless"}),
    )
    .await;
    let (status, body) = shell_response(root.path(), confirm_request(&draft_id)).await;
    assert_eq!(status, 200);
    assert_eq!(body["outcome"], "submitted");
}

#[tokio::test]
async fn confirm_and_cancel_do_not_append_draft_events() {
    let portal = corpus_route_portal();
    let root = phase_root("established", Some(&portal));
    let _guard = install_route_portal(&portal);
    let confirm_id = capture_create(
        root.path(),
        serde_json::json!({"subject":"keep","description":"keep"}),
    )
    .await;
    let confirm_path = draft_file_for_draft(root.path(), &confirm_id);
    let before = std::fs::read(&confirm_path).expect("snapshot");
    let (status, _) = shell_response(root.path(), confirm_request(&confirm_id)).await;
    assert_eq!(status, 200);
    assert_eq!(std::fs::read(&confirm_path).expect("after confirm"), before);

    let cancel_id = capture_create(
        root.path(),
        serde_json::json!({"subject":"drop","description":"drop"}),
    )
    .await;
    let cancel_path = draft_file_for_draft(root.path(), &cancel_id);
    let before = std::fs::read(&cancel_path).expect("cancel snapshot");
    let (status, _) = shell_response(root.path(), cancel_request(&cancel_id)).await;
    assert_eq!(status, 200);
    assert_eq!(std::fs::read(cancel_path).expect("after cancel"), before);
}

#[tokio::test]
async fn disabled_and_unestablished_reject_confirm_and_cancel() {
    let portal = corpus_route_portal();
    let root = phase_root("disabled", Some(&portal));
    let _guard = install_route_portal(&portal);
    portal.clear_log();
    for request in [confirm_request("any"), cancel_request("any")] {
        let (status, body) = shell_response(root.path(), request).await;
        assert_eq!(status, 403);
        assert_eq!(body["reason_code"], "feature_unavailable");
    }
    assert!(portal.log().is_empty());

    let unestablished = phase_root("unestablished", None);
    for path in [
        "/app/support/api/draft/confirm",
        "/app/support/api/draft/cancel",
    ] {
        let response = solstone_core_convey_shell::router(unestablished.path().to_path_buf())
            .oneshot(json_request(path, serde_json::json!({"draft_id":"any"})))
            .await
            .expect("shell response");
        assert_eq!(response.status().as_u16(), 302, "{path}");
        assert_eq!(
            response.headers().get("location").unwrap(),
            "/init",
            "{path}"
        );
    }
}
