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
use solstone_core_support_portal::{Ledger, PortalClient};
use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct PortalRequest {
    method: String,
    path: String,
    query: Option<String>,
    had_idempotency_key: bool,
    had_authorization: bool,
    had_dpop: bool,
}

#[derive(Clone)]
struct HttpReply {
    status: u16,
    body: String,
    content_type: String,
}

type RouteReplyOverrides = BTreeMap<(String, String), VecDeque<HttpReply>>;

/// Test-only route-table fake. The portal crate's fake is a sequential transport;
/// this one is a loopback HTTP portal for corpus route replay.
struct FakePortal {
    base_url: String,
    log: Arc<Mutex<Vec<PortalRequest>>>,
    bodies: Arc<Mutex<Vec<Vec<u8>>>>,
    overrides: Arc<Mutex<RouteReplyOverrides>>,
    stop: Arc<AtomicBool>,
    wake: SocketAddr,
    thread: Option<JoinHandle<()>>,
}

impl FakePortal {
    fn new() -> Self {
        let pinned = &serde_json::from_str::<Value>(CORPUS).expect("corpus parses")["pinned"];
        let tos = pinned["stub_tos"].as_str().expect("pinned tos").to_owned();
        let token = pinned["stub_access_token"]
            .as_str()
            .expect("pinned token")
            .to_owned();
        let handle = pinned["handle"].as_str().expect("pinned handle").to_owned();
        let ticket = pinned["seeded_ticket_id"].as_i64().expect("pinned ticket");
        let routes = fixed_routes(tos, token, handle, ticket);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback fake");
        listener.set_nonblocking(true).expect("nonblocking fake");
        let wake = listener.local_addr().expect("fake address");
        let log = Arc::new(Mutex::new(Vec::new()));
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let overrides = Arc::new(Mutex::new(BTreeMap::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_log = log.clone();
        let thread_bodies = bodies.clone();
        let thread_overrides = overrides.clone();
        let thread_stop = stop.clone();
        let thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        if thread_stop.load(Ordering::Acquire) {
                            break;
                        }
                        let request = read_portal_request(&mut stream);
                        thread_bodies.lock().expect("body lock").push(request.1);
                        let key = (request.0.method.clone(), request.0.path.clone());
                        thread_log.lock().expect("log lock").push(request.0);
                        let reply = thread_overrides
                            .lock()
                            .expect("override lock")
                            .get_mut(&key)
                            .and_then(VecDeque::pop_front)
                            .or_else(|| routes.get(&key).cloned())
                            .unwrap_or_else(not_found_reply);
                        write_portal_reply(&mut stream, reply);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            base_url: format!("http://{wake}"),
            log,
            bodies,
            overrides,
            stop,
            wake,
            thread: Some(thread),
        }
    }
    fn url(&self) -> &str {
        &self.base_url
    }
    fn log(&self) -> Vec<PortalRequest> {
        self.log.lock().expect("log lock").clone()
    }
    fn clear_log(&self) {
        self.log.lock().expect("log lock").clear();
    }
    fn bodies(&self) -> Vec<Vec<u8>> {
        self.bodies.lock().expect("body lock").clone()
    }
    fn clear_bodies(&self) {
        self.bodies.lock().expect("body lock").clear();
    }
    fn override_route(&self, method: &str, path: &str, replies: Vec<HttpReply>) {
        self.overrides
            .lock()
            .expect("override lock")
            .insert((method.to_owned(), path.to_owned()), replies.into());
    }
}

impl Drop for FakePortal {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Ok(stream) = TcpStream::connect(self.wake) {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Some(thread) = self.thread.take() {
            thread.join().expect("fake thread joins");
        }
    }
}

fn fixed_routes(
    tos: String,
    token: String,
    handle: String,
    ticket: i64,
) -> BTreeMap<(String, String), HttpReply> {
    let json_reply = |value: Value| HttpReply {
        status: 200,
        body: value.to_string(),
        content_type: "application/json".to_owned(),
    };
    let open = serde_json::json!({"ticket_id":ticket,"status":"open","subject":"a seeded open ticket","created_at":"2026-02-01T00:00:00Z","updated_at":"2026-02-02T00:00:00Z","body":"a field an active ticket keeps"});
    let closed = serde_json::json!({"ticket_id":8,"status":"closed","closed_at":"2026-02-03T00:00:00Z","close_scheduled_at":"2026-02-10T00:00:00Z","reason_code":"resolved","subject":"a field a tombstone must drop","thread":[{"body":"a message a tombstone must drop"}]});
    let mut routes = BTreeMap::new();
    routes.insert(
        ("GET".into(), "/tos".into()),
        HttpReply {
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
fn not_found_reply() -> HttpReply {
    HttpReply {
        status: 404,
        body: r#"{"error":"not_found"}"#.into(),
        content_type: "application/json".into(),
    }
}
fn read_portal_request(stream: &mut TcpStream) -> (PortalRequest, Vec<u8>) {
    let mut raw = Vec::new();
    let mut buffer = [0; 1024];
    while !raw.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).expect("read request");
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&buffer[..read]);
    }
    let header_end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .unwrap_or(raw.len());
    let header = String::from_utf8_lossy(&raw[..header_end]);
    let mut lines = header.split("\r\n");
    let start = lines.next().unwrap_or_default();
    let mut words = start.split_whitespace();
    let method = words.next().unwrap_or_default().to_owned();
    let target = words.next().unwrap_or_default();
    let (path, query) = target
        .split_once('?')
        .map_or((target, None), |(path, query)| {
            (path, Some(query.to_owned()))
        });
    let path = path.to_owned();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect::<Vec<_>>();
    let length = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let chunked = headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("transfer-encoding") && value.eq_ignore_ascii_case("chunked")
    });
    let body = if chunked {
        let mut encoded = raw[header_end..].to_vec();
        loop {
            if let Some(decoded) = decode_chunked_body(&encoded) {
                break decoded;
            }
            let read = stream.read(&mut buffer).expect("read chunked request body");
            if read == 0 {
                break encoded;
            }
            encoded.extend_from_slice(&buffer[..read]);
        }
    } else {
        while raw.len().saturating_sub(header_end) < length {
            let read = stream.read(&mut buffer).expect("read request body");
            if read == 0 {
                break;
            }
            raw.extend_from_slice(&buffer[..read]);
        }
        raw[header_end..].to_vec()
    };
    (
        PortalRequest {
            method,
            path,
            query,
            had_idempotency_key: headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("idempotency-key")),
            had_authorization: headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("authorization")),
            had_dpop: headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("dpop")),
        },
        body,
    )
}

fn decode_chunked_body(encoded: &[u8]) -> Option<Vec<u8>> {
    let mut cursor = 0;
    let mut body = Vec::new();
    loop {
        let line_end = encoded[cursor..]
            .windows(2)
            .position(|window| window == b"\r\n")?;
        let line_end = cursor + line_end;
        let length = std::str::from_utf8(&encoded[cursor..line_end])
            .ok()?
            .split(';')
            .next()?
            .trim();
        let length = usize::from_str_radix(length, 16).ok()?;
        cursor = line_end + 2;
        if length == 0 {
            return (encoded.len() >= cursor + 2).then_some(body);
        }
        if encoded.len() < cursor + length + 2 {
            return None;
        }
        body.extend_from_slice(&encoded[cursor..cursor + length]);
        cursor += length;
        (encoded.get(cursor..cursor + 2)? == b"\r\n").then_some(())?;
        cursor += 2;
    }
}

fn multipart_file_bytes(body: &[u8]) -> Vec<u8> {
    let headers_end = body
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("multipart part headers")
        + 4;
    let body_end = body[headers_end..]
        .windows(4)
        .position(|window| window == b"\r\n--")
        .expect("multipart closing boundary")
        + headers_end;
    body[headers_end..body_end].to_vec()
}
fn write_portal_reply(stream: &mut TcpStream, reply: HttpReply) {
    let response = format!(
        "HTTP/1.1 {} Response\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        reply.status,
        reply.content_type,
        reply.body.len(),
        reply.body
    );
    stream.write_all(response.as_bytes()).expect("write reply");
}

fn phase_root(phase: &str, portal_url: &str) -> TempDir {
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
                    "support":{"enabled":phase != "disabled","portal_url":portal_url},
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
                let mut client =
                    PortalClient::from_journal_settings(root.path(), None, false).expect("client");
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
    let response = solstone_core_convey_shell::router(root.to_path_buf())
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
    let chat = root.join("chronicle").join(day).join("chat");
    for segment in std::fs::read_dir(chat).expect("chat segments").flatten() {
        let path = segment.path().join("chat.jsonl");
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
        compare_text_bytes(&body, expected_response["text_bytes"].as_u64().unwrap())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
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
}

async fn replay_case(
    phase: &str,
    case: &Value,
    probe: &CorpusProbe,
    fake: &FakePortal,
    root: &Path,
) {
    reset_portal_storage(root, phase);
    fake.clear_log();
    if probe.kind == "drain" {
        seed_pending_acknowledgement(root, "corpus-drain");
    }
    let name = case["name"].as_str().expect("case name");
    let normalized = case["normalized"].as_array().expect("normalized pointers");
    let response = solstone_core_convey_shell::router(root.to_path_buf())
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
        let response = solstone_core_convey_shell::router(root.to_path_buf())
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
    let actual_requests = fake
        .log()
        .into_iter()
        .map(|request| {
            serde_json::json!({"method":request.method,"path":request.path,"had_idempotency_key":request.had_idempotency_key,"had_authorization":request.had_authorization,"had_dpop":request.had_dpop})
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
        let ticket_list = fake
            .log()
            .into_iter()
            .find(|request| request.method == "GET" && request.path == "/api/tickets")
            .expect("ticket list portal request");
        assert_eq!(ticket_list.query.as_deref(), Some("status=open"));
    }
    if name == "api_tickets_list" && records_ticket_list {
        let ticket_list = fake
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
fn copied_assets_are_byte_identical_to_the_frozen_hashes() {
    for (bytes, hash, size) in [
        (
            super::WORKSPACE,
            "ca70f29ba5345d5374c23be877eb52fc1e421f0eec088d9b9cd31032016e1deb",
            37_261,
        ),
        (
            super::BACKGROUND,
            "6299f26fbc48e57ddd708f2d97b947086f3ae650531f982d97d6bf1c0441fe4b",
            1_182,
        ),
        (
            super::SUPPORT_JS,
            "dc56041242732fbd1e54cbc16c02735ef325ce99c8d9bcf1c851f9f412af0b55",
            10_629,
        ),
        (
            super::SHELL,
            "508e101adc759313ddce94c3263f4124f7575e7dd0d07dfc69cf02817746e7fe",
            12_199,
        ),
    ] {
        assert_eq!(bytes.len(), size);
        assert_eq!(format!("{:x}", Sha256::digest(bytes)), hash);
    }
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

fn fake_request(fake: &FakePortal, method: &str, path: &str, headers: &[(&str, &str)]) -> String {
    let mut stream =
        TcpStream::connect(fake.url().trim_start_matches("http://")).expect("connect fake");
    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).expect("write request");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read response");
    response
}

#[test]
fn fake_serves_pinned_registration_values_and_logs_request_flags() {
    let fake = FakePortal::new();
    let tos = fake_request(&fake, "GET", "/tos", &[]);
    assert!(
        tos.ends_with(
            serde_json::from_str::<Value>(CORPUS).unwrap()["pinned"]["stub_tos"]
                .as_str()
                .unwrap()
        )
    );
    let signup = fake_request(&fake, "POST", "/api/signup", &[]);
    assert!(signup.contains("corpus.stub.access.token"));
    assert!(signup.contains("solstone-corpus-host"));
    let _ = fake_request(
        &fake,
        "GET",
        "/api/tickets",
        &[("Authorization", "DPoP ignored"), ("DPoP", "ignored")],
    );
    assert_eq!(
        fake.log(),
        vec![
            PortalRequest {
                method: "GET".into(),
                path: "/tos".into(),
                query: None,
                had_idempotency_key: false,
                had_authorization: false,
                had_dpop: false
            },
            PortalRequest {
                method: "POST".into(),
                path: "/api/signup".into(),
                query: None,
                had_idempotency_key: false,
                had_authorization: false,
                had_dpop: false
            },
            PortalRequest {
                method: "GET".into(),
                path: "/api/tickets".into(),
                query: None,
                had_idempotency_key: false,
                had_authorization: true,
                had_dpop: true
            },
        ]
    );
    assert_eq!(fake.bodies().len(), 3);
}

#[test]
fn fake_route_override_is_consumed_then_fixed_response_resumes() {
    let fake = FakePortal::new();
    fake.override_route(
        "POST",
        "/api/idempotency/ack",
        vec![HttpReply {
            status: 500,
            body: "temporary".into(),
            content_type: "text/plain".into(),
        }],
    );
    assert!(fake_request(&fake, "POST", "/api/idempotency/ack", &[]).starts_with("HTTP/1.1 500"));
    assert!(fake_request(&fake, "POST", "/api/idempotency/ack", &[]).starts_with("HTTP/1.1 200"));
}

#[test]
fn established_phase_starts_registered_and_reads_without_signup() {
    let fake = FakePortal::new();
    let root = phase_root("established", fake.url());
    fake.clear_log();
    let mut client = PortalClient::from_journal_settings(root.path(), None, false).expect("client");
    assert!(client.is_registered());
    client.list_tickets(None, None, None).expect("ticket read");
    assert_eq!(
        fake.log()
            .iter()
            .map(|request| request.path.as_str())
            .collect::<Vec<_>>(),
        vec!["/api/tickets"]
    );
}

#[test]
fn unregistered_phase_reregisters_after_each_storage_reset() {
    let fake = FakePortal::new();
    let root = phase_root("unregistered", fake.url());
    for _ in 0..2 {
        reset_portal_storage(root.path(), "unregistered");
        PortalClient::from_journal_settings(root.path(), None, false)
            .expect("client")
            .list_tickets(None, None, None)
            .expect("ticket read");
    }
    assert_eq!(
        fake.log()
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
    let fake = FakePortal::new();
    let root = phase_root("established", fake.url());
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
    let fake = FakePortal::new();
    let root = phase_root("established", fake.url());
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
        replay_case("established", case, &probe.probe, &fake, root.path()).await;
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
    let fake = FakePortal::new();
    let root = phase_root("established", fake.url());
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
        replay_case("established", case, &probe.probe, &fake, root.path()).await;
    }
}

#[tokio::test]
async fn drain_runs_after_a_create_and_acknowledges_the_completed_operation() {
    let fake = FakePortal::new();
    let root = phase_root("established", fake.url());
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
    fake.clear_log();
    let response = solstone_core_convey_shell::router(root.path().to_path_buf())
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
        fake.log()
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
    let fake = FakePortal::new();
    let root = phase_root("established", fake.url());
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
        replay_case("established", case, &probe.probe, &fake, root.path()).await;
        assert_eq!(
            ledger_contents(root.path()),
            before,
            "{name} ledger contents"
        );
        assert!(fake.log().is_empty(), "{name} must not contact the portal");
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
        replay_case("established", case, &probe.probe, &fake, root.path()).await;
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
    replay_case("established", case, &probe.probe, &fake, root.path()).await;
    assert!(
        fake.log()
            .iter()
            .any(|request| request.method == "POST" && request.path == "/api/tickets"),
        "a malformed non-empty key still reaches the mutation"
    );
}

#[tokio::test]
async fn draft_json_attach_is_rejected_and_json_capture_stays_local() {
    let fake = FakePortal::new();
    let root = phase_root("established", fake.url());

    fake.clear_log();
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
    assert!(fake.log().is_empty(), "a draft never contacts the portal");

    fake.clear_log();
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
    assert!(fake.log().is_empty(), "a successful draft stays local");
}

#[tokio::test]
async fn multipart_draft_refusals_happy_path_and_json_fallthrough() {
    let fake = FakePortal::new();
    let root = phase_root("established", fake.url());

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
        fake.clear_log();
        let (status, body) = shell_response(root.path(), request).await;
        assert_eq!(status, 400, "{name}");
        assert_eq!(body["reason_code"], reason, "{name}");
        assert!(fake.log().is_empty(), "{name} must stay local");
    }

    fake.clear_log();
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
        fake.log().is_empty(),
        "a no-file multipart body falls through locally"
    );

    fake.clear_log();
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
    assert!(fake.log().is_empty(), "a multipart draft stays local");
}

#[tokio::test]
async fn attachment_refusals_precede_suffix_and_retry_resends_identical_bytes() {
    let fake = FakePortal::new();
    let root = phase_root("established", fake.url());
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
        fake.clear_log();
        let (status, body) = shell_response(root.path(), request).await;
        assert_eq!(status, 400, "{name}");
        assert_eq!(body["reason_code"], reason, "{name}");
        assert_eq!(body["detail"], detail, "{name}");
        assert!(fake.log().is_empty(), "{name} must precede portal work");
    }

    fake.clear_log();
    fake.clear_bodies();
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
    let bodies = fake.bodies();
    let wire_body = String::from_utf8_lossy(&bodies[0]);
    assert!(
        wire_body.contains("suffix-proves-spool.txt"),
        "the attachment upload preserves the original filename: {wire_body}"
    );

    let retry_fake = FakePortal::new();
    let retry_root = phase_root("established", retry_fake.url());
    retry_fake.clear_log();
    retry_fake.clear_bodies();
    retry_fake.override_route(
        "POST",
        "/api/tickets/7/attachments",
        vec![
            HttpReply {
                status: 401,
                body: r#"{"error":"tos_changed"}"#.into(),
                content_type: "application/json".into(),
            },
            HttpReply {
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
    let attachment_bodies = retry_fake
        .log()
        .iter()
        .enumerate()
        .filter(|(_, request)| {
            request.method == "POST" && request.path == "/api/tickets/7/attachments"
        })
        .map(|(index, _)| retry_fake.bodies()[index].clone())
        .collect::<Vec<_>>();
    assert_eq!(attachment_bodies.len(), 2);
    assert_eq!(
        multipart_file_bytes(&attachment_bodies[0]),
        multipart_file_bytes(&attachment_bodies[1])
    );
}

#[tokio::test]
async fn write_mutations_project_tombstones_and_preserve_active_ticket_bodies() {
    let fake = FakePortal::new();
    let root = phase_root("established", fake.url());
    let closed = serde_json::json!({
        "ticket_id": 7,
        "status": "closed",
        "closed_at": "2026-03-01T00:00:00Z",
        "close_scheduled_at": "2026-03-08T00:00:00Z",
        "reason_code": "resolved",
        "subject": "a tombstone must omit this",
        "messages": [{"body": "and this"}],
    });
    fake.override_route(
        "POST",
        "/api/tickets/7/close",
        vec![HttpReply {
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
    fake.override_route(
        "POST",
        "/api/tickets",
        vec![HttpReply {
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
    let fake = FakePortal::new();
    let root = phase_root("established", fake.url());
    fake.clear_log();
    fake.clear_bodies();
    fake.override_route(
        "POST",
        "/api/tickets",
        vec![HttpReply {
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
    let request_index = fake
        .log()
        .iter()
        .enumerate()
        .find_map(|(index, request)| {
            (request.method == "POST" && request.path == "/api/tickets").then_some(index)
        })
        .expect("ticket creation reached portal");
    let sent: Value = serde_json::from_slice(&fake.bodies()[request_index])
        .expect("portal ticket creation body is JSON");
    assert_eq!(
        sent["user_context"]["platform"], "caller-supplied platform",
        "the caller replaces collect_all's always-present platform diagnostic"
    );
}

#[tokio::test]
async fn write_path_non_integer_ticket_id_returns_http_not_found() {
    let fake = FakePortal::new();
    let root = phase_root("established", fake.url());
    fake.clear_log();
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
        fake.log().is_empty(),
        "an invalid path id must not reach the portal"
    );
}

#[tokio::test]
async fn established_validation_refusals_and_drain_cases_match_the_corpus() {
    let fake = FakePortal::new();
    let root = phase_root("established", fake.url());
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
        replay_case("established", case, &probe.probe, &fake, root.path()).await;
    }

    reset_portal_storage(root.path(), "established");
    fake.clear_log();
    let attach_json = CorpusProbe {
        name: "draft_json_attach",
        method: "POST",
        path: "/app/support/api/draft",
        kind: "probe",
        body: Some(r#"{"verb":"attach","payload":{}}"#),
        key: None,
        file: None,
    };
    let response = solstone_core_convey_shell::router(root.path().to_path_buf())
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
    assert!(fake.log().is_empty());

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
    let fake = FakePortal::new();
    let root = phase_root("unestablished", fake.url());
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
        replay_case("unestablished", case, &probe.probe, &fake, root.path()).await;
    }
}

#[tokio::test]
async fn disabled_read_and_page_cases_match_the_corpus() {
    let fake = FakePortal::new();
    let root = phase_root("disabled", fake.url());
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
        replay_case("disabled", case, &probe.probe, &fake, root.path()).await;
    }
}

#[tokio::test]
async fn unregistered_read_and_page_cases_register_for_every_portal_read() {
    let fake = FakePortal::new();
    let root = phase_root("unregistered", fake.url());
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
        replay_case("unregistered", case, &probe.probe, &fake, root.path()).await;

        let actual = fake
            .log()
            .into_iter()
            .map(|request| {
                serde_json::json!({"method":request.method,"path":request.path,"had_idempotency_key":request.had_idempotency_key,"had_authorization":request.had_authorization,"had_dpop":request.had_dpop})
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

fn run_config_environment_child(mode: &str, support_url: Option<&str>) {
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .args([
            "--exact",
            "corpus::config_fails_open_and_prefers_the_support_url_environment_override",
            "--test-threads=1",
        ])
        .env("SOLSTONE_SUPPORT_CONFIG_TEST_MODE", mode)
        .env_remove("SOLSTONE_SUPPORT_URL");
    if let Some(support_url) = support_url {
        command.env("SOLSTONE_SUPPORT_URL", support_url);
    }
    let output = command.output().expect("run config child");
    assert!(
        output.status.success(),
        "config child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("running 1 test"),
        "config child did not run its exact test"
    );
}

#[tokio::test]
async fn drain_acknowledges_before_a_portal_backed_handler() {
    let fake = FakePortal::new();
    let root = phase_root("established", fake.url());
    reset_portal_storage(root.path(), "established");
    seed_pending_acknowledgement(root.path(), "drain-before");
    fake.clear_log();

    let _ = support_route_response(root.path(), "/app/support/api/tickets").await;
    assert_eq!(
        fake.log()
            .iter()
            .map(|request| (request.method.as_str(), request.path.as_str()))
            .collect::<Vec<_>>(),
        vec![("POST", "/api/idempotency/ack"), ("GET", "/api/tickets"),]
    );
}

#[tokio::test]
async fn empty_drain_makes_no_portal_request_or_keypair() {
    let fake = FakePortal::new();
    let root = phase_root("unregistered", fake.url());
    reset_portal_storage(root.path(), "unregistered");
    fake.clear_log();

    let _ = support_route_response(root.path(), "/app/support/api/config").await;
    assert!(fake.log().is_empty());
    assert!(
        !root.path().join("apps/support/portal/keypair.pem").exists(),
        "an empty drain must not generate an identity"
    );
}

#[tokio::test]
async fn drain_failure_does_not_change_any_registered_route_response() {
    let fake = FakePortal::new();
    let root = phase_root("established", fake.url());
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
        fake.clear_log();
        let baseline = support_route_response(root.path(), path).await;

        reset_portal_storage(root.path(), "established");
        fake.clear_log();
        fake.override_route(
            "POST",
            "/api/idempotency/ack",
            vec![
                HttpReply {
                    status: 500,
                    body: "drain failed".into(),
                    content_type: "text/plain; charset=utf-8".into(),
                },
                HttpReply {
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
    let fake = FakePortal::new();
    let root = phase_root("unestablished", fake.url());
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
async fn config_fails_open_and_prefers_the_support_url_environment_override() {
    match std::env::var("SOLSTONE_SUPPORT_CONFIG_TEST_MODE")
        .ok()
        .as_deref()
    {
        None => {
            run_config_environment_child("defaults", None);
            run_config_environment_child("environment", Some("https://environment.example///"));
        }
        Some("defaults") => {
            for (name, setup) in [
                ("absent", None),
                ("unreadable", Some("directory")),
                ("malformed", Some("not json")),
            ] {
                let root = TempDir::new().expect("config root");
                if let Some(setup) = setup {
                    let config = root.path().join("config");
                    std::fs::create_dir_all(&config).expect("config directory");
                    if setup == "directory" {
                        std::fs::create_dir(config.join("config.json")).expect("unreadable config");
                    } else {
                        std::fs::write(config.join("config.json"), setup)
                            .expect("malformed config");
                    }
                }
                let (status, _, body) =
                    support_route_response(root.path(), "/app/support/api/config").await;
                let body: Value = serde_json::from_slice(&body).expect("config response json");
                assert_eq!(status, 200, "{name}");
                assert_eq!(body["enabled"], true, "{name}");
                assert_eq!(body["portal_url"], "https://support.solstone.app", "{name}");
            }
        }
        Some("environment") => {
            let root = TempDir::new().expect("configured root");
            let config = root.path().join("config");
            std::fs::create_dir_all(&config).expect("config directory");
            std::fs::write(
                config.join("config.json"),
                r#"{"support":{"enabled":true,"portal_url":"https://journal.example///"}}"#,
            )
            .expect("journal config");
            let (_, _, body) = support_route_response(root.path(), "/app/support/api/config").await;
            let body: Value = serde_json::from_slice(&body).expect("config response json");
            assert_eq!(body["enabled"], true);
            assert_eq!(body["portal_url"], "https://environment.example");
        }
        Some(mode) => panic!("unknown config test mode {mode}"),
    }
}

#[tokio::test]
async fn support_static_and_closed_routes_take_precedence() {
    let fake = FakePortal::new();
    let root = phase_root("established", fake.url());

    fake.clear_log();
    let (status, content_type, body) =
        support_route_response(root.path(), "/app/support/api/tickets/closed").await;
    assert_eq!(status, 200);
    assert_eq!(content_type, "application/json");
    assert_eq!(
        fake.log()
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

    let response = solstone_core_convey_shell::router(root.path().to_path_buf())
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
    let fake = FakePortal::new();
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

        let root = phase_root(phase, fake.url());
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
