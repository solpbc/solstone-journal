// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Real-socket claims for the Support web surface and its loopback helper.

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use axum::body::{Body, to_bytes};
use axum::http::Request;
use serde_json::Value;
use solstone_core_support_portal::PortalClient;
use solstone_core_support_web::routes;
use tempfile::TempDir;
use tower::ServiceExt;

const CORPUS: &str = include_str!("../../../fixtures/convey_support_corpus.json");
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const JOIN_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_WIRE_BYTES: usize = 1024 * 1024;

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
            run_accept_loop(
                || listener.accept(),
                &thread_stop,
                |mut stream| {
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
                },
            );
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

    fn bodies(&self) -> Vec<Vec<u8>> {
        self.bodies.lock().expect("body lock").clone()
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
        if let Ok(stream) = TcpStream::connect_timeout(&self.wake, IO_TIMEOUT) {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Some(thread) = self.thread.take() {
            join_bounded(thread, "fake thread");
        }
    }
}

fn run_accept_loop<A, H>(mut accept: A, stop: &AtomicBool, mut on_accept: H)
where
    A: FnMut() -> io::Result<(TcpStream, SocketAddr)>,
    H: FnMut(TcpStream),
{
    while !stop.load(Ordering::Acquire) {
        match accept() {
            Ok((stream, _)) => {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                stream
                    .set_read_timeout(Some(IO_TIMEOUT))
                    .expect("bound fake read");
                stream
                    .set_write_timeout(Some(IO_TIMEOUT))
                    .expect("bound fake write");
                on_accept(stream);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(_) => break,
        }
    }
}

fn join_bounded(thread: JoinHandle<()>, label: &str) {
    let deadline = Instant::now() + JOIN_TIMEOUT;
    while !thread.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(2));
    }
    assert!(thread.is_finished(), "{label} exceeded shutdown deadline");
    thread.join().unwrap_or_else(|_| panic!("{label} panicked"));
}

fn read_more(stream: &mut TcpStream, raw: &mut Vec<u8>, buffer: &mut [u8]) -> usize {
    let remaining = MAX_WIRE_BYTES.saturating_sub(raw.len());
    if remaining == 0 {
        return 0;
    }
    let capacity = remaining.min(buffer.len());
    let read = stream.read(&mut buffer[..capacity]).unwrap_or(0);
    raw.extend_from_slice(&buffer[..read]);
    read
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
    let open = serde_json::json!({"ticket_id":ticket,"status":"open"});
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
    routes.insert(
        ("GET".into(), "/api/tickets".into()),
        json_reply(Value::Array(vec![open.clone()])),
    );
    routes.insert(
        ("GET".into(), format!("/api/tickets/{ticket}")),
        json_reply(open),
    );
    routes.insert(
        ("POST".into(), "/api/idempotency/ack".into()),
        json_reply(serde_json::json!({"acknowledged":true})),
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
        if read_more(stream, &mut raw, &mut buffer) == 0 {
            break;
        }
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
        .map_or((target.to_owned(), None), |(path, query)| {
            (path.to_owned(), Some(query.to_owned()))
        });
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
            if read_more(stream, &mut encoded, &mut buffer) == 0 {
                break encoded;
            }
        }
    } else {
        while raw.len().saturating_sub(header_end) < length {
            if read_more(stream, &mut raw, &mut buffer) == 0 {
                break;
            }
        }
        raw.get(header_end..).unwrap_or_default().to_vec()
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

fn write_portal_reply(stream: &mut TcpStream, reply: HttpReply) {
    let response = format!(
        "HTTP/1.1 {} Response\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        reply.status,
        reply.content_type,
        reply.body.len(),
        reply.body
    );
    let _ = stream.write_all(response.as_bytes());
}

fn fake_request(fake: &FakePortal, method: &str, path: &str, headers: &[(&str, &str)]) -> String {
    let mut stream =
        TcpStream::connect(fake.url().trim_start_matches("http://")).expect("connect fake");
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .expect("bound fake response read");
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .expect("bound fake request write");
    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).expect("write request");
    let mut response = String::new();
    stream
        .take(MAX_WIRE_BYTES as u64)
        .read_to_string(&mut response)
        .expect("read response");
    response
}

fn established_root(portal_url: &str) -> TempDir {
    let root = TempDir::new().expect("phase root");
    let config = root.path().join("config");
    std::fs::create_dir_all(&config).expect("config directory");
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
            "support":{"enabled":true,"portal_url":portal_url},
        }))
        .expect("app config"),
    )
    .expect("write app config");
    let portal = root.path().join("apps/support/portal");
    std::fs::create_dir_all(&portal).expect("portal state directory");
    std::fs::write(
        portal.join("keypair.pem"),
        include_bytes!("../../../fixtures/support_portal_golden_nonproduction/keypair.pem"),
    )
    .expect("seed non-production key fixture");
    root
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

#[tokio::test]
async fn routes_complete_a_live_loopback_portal_read() {
    let fake = FakePortal::new();
    let root = established_root(fake.url());
    let mut client = PortalClient::from_journal_settings(root.path(), None, false).expect("client");
    client.register().expect("register against live fake");
    let response = routes(root.path().to_path_buf())
        .oneshot(
            Request::get("/app/support/api/tickets")
                .header("Host", "127.0.0.1")
                .body(Body::empty())
                .expect("tickets request"),
        )
        .await
        .expect("routes response");
    assert_eq!(response.status().as_u16(), 200);
    let body = to_bytes(response.into_body(), MAX_WIRE_BYTES)
        .await
        .expect("tickets body");
    let tickets: Value = serde_json::from_slice(&body).expect("tickets json");
    assert!(tickets.as_array().is_some());
    assert!(
        fake.log()
            .iter()
            .any(|request| request.method == "GET" && request.path == "/api/tickets")
    );
}

#[test]
fn fake_portal_terminates_on_accept_failure() {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread = thread::spawn(move || {
        run_accept_loop(
            || Err(io::Error::other("injected accept failure")),
            &thread_stop,
            |_| panic!("accept failure must not yield a stream"),
        );
    });
    join_bounded(thread, "accept-failure loop");
}
