// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Real-socket claims for the Support web surface and its loopback helper.

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use axum::body::{Body, to_bytes};
use axum::http::Request;
use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use serde_json::Value;
use solstone_core_convey_http::listener::bind_loopback;
use solstone_core_convey_http::serve::{STANDARD_BODY_LIMIT, serve_connection, tcp_builder};
use solstone_core_support_portal::PortalClient;
use solstone_core_support_web::routes;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream as TokioTcpStream;
use tower::ServiceExt;

const CORPUS: &str = include_str!("../../../fixtures/convey_support_corpus.json");
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const JOIN_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_WIRE_BYTES: usize = 1024 * 1024;
const CONFIG_DRIVER: &str = "SOLSTONE_SUPPORT_CONFIG_DRIVER";
const CONFIG_CHILD_CASE: &str = "SOLSTONE_SUPPORT_CONFIG_CHILD_CASE";
const CONFIG_ROOT: &str = "SOLSTONE_SUPPORT_CONFIG_ROOT";
const CONFIG_RECEIPT: &str = "SOLSTONE_SUPPORT_CONFIG_RECEIPT";
const CONFIG_POISON: &str = "SOLSTONE_SUPPORT_CONFIG_UNRELATED_POISON";
const CONFIG_CASES: &[&str] = &[
    "default-absent",
    "default-unreadable",
    "default-malformed",
    "journal",
    "empty-environment",
    "environment",
];

struct ReapedChild {
    child: Option<Child>,
}

impl ReapedChild {
    fn spawn(mut command: Command) -> Self {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        Self {
            child: Some(command.spawn().expect("spawn support config child")),
        }
    }

    fn wait_bounded(&mut self, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self
                .child
                .as_mut()
                .expect("owned support config child")
                .try_wait()
                .expect("poll support config child")
            {
                self.child.take();
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "support config child exceeded {timeout:?}"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for ReapedChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct ReapedProcessGroup {
    child: Option<Child>,
    group: Pid,
}

impl ReapedProcessGroup {
    fn spawn(mut command: Command) -> Self {
        command
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let child = command.spawn().expect("spawn support config driver");
        let group = Pid::from_raw(i32::try_from(child.id()).expect("driver PID fits group ID"));
        Self {
            child: Some(child),
            group,
        }
    }

    fn wait_bounded(&mut self, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self
                .child
                .as_mut()
                .expect("owned support config driver")
                .try_wait()
                .expect("poll support config driver")
            {
                self.child.take();
                self.terminate_group();
                self.assert_group_gone();
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "support config driver exceeded {timeout:?}"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn terminate_group(&self) {
        match killpg(self.group, Signal::SIGKILL) {
            Ok(()) | Err(Errno::ESRCH) => {}
            Err(error) => panic!("terminate support config process group: {error}"),
        }
    }

    fn assert_group_gone(&self) {
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            match killpg(self.group, None::<Signal>) {
                Err(Errno::ESRCH) => return,
                Ok(()) | Err(Errno::EPERM) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(5));
                }
                Ok(()) | Err(Errno::EPERM) => {
                    panic!("support config process group survived cleanup")
                }
                Err(error) => panic!("inspect support config process group: {error}"),
            }
        }
    }
}

impl Drop for ReapedProcessGroup {
    fn drop(&mut self) {
        if self.child.is_some() {
            self.terminate_group();
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.assert_group_gone();
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
                    .set_nonblocking(false)
                    .expect("make accepted fake connection blocking");
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

fn prepare_config_case(root: &Path, case: &str) {
    assert!(
        CONFIG_CASES.contains(&case),
        "unregistered config case {case}"
    );
    let config_file = root.join("config/config.json");
    match case {
        "default-absent" => {}
        "default-unreadable" => {
            std::fs::create_dir_all(&config_file).expect("create unreadable config fixture");
        }
        "default-malformed" => {
            std::fs::create_dir_all(config_file.parent().expect("config parent"))
                .expect("create config directory");
            std::fs::write(config_file, "not json").expect("write malformed config fixture");
        }
        "journal" | "empty-environment" | "environment" => {
            std::fs::create_dir_all(config_file.parent().expect("config parent"))
                .expect("create config directory");
            std::fs::write(
                config_file,
                r#"{"support":{"enabled":true,"portal_url":"https://journal.example/path///"}}"#,
            )
            .expect("write journal config fixture");
        }
        _ => unreachable!("case allowlist checked"),
    }
}

fn expected_config_url(case: &str) -> &'static str {
    match case {
        "default-absent" | "default-unreadable" | "default-malformed" => {
            "https://support.solstone.app"
        }
        "journal" | "empty-environment" => "https://journal.example/path",
        "environment" => "https://environment.example/path",
        _ => panic!("unregistered config case {case}"),
    }
}

fn config_case_environment(case: &str, root: &Path, receipt: &Path) -> BTreeMap<String, String> {
    let mut environment = BTreeMap::from([
        (CONFIG_CHILD_CASE.to_owned(), case.to_owned()),
        (CONFIG_ROOT.to_owned(), root.to_string_lossy().into_owned()),
        (
            CONFIG_RECEIPT.to_owned(),
            receipt.to_string_lossy().into_owned(),
        ),
    ]);
    match case {
        "empty-environment" => {
            environment.insert("SOLSTONE_SUPPORT_URL".to_owned(), String::new());
        }
        "environment" => {
            environment.insert(
                "SOLSTONE_SUPPORT_URL".to_owned(),
                "https://environment.example/path///".to_owned(),
            );
        }
        _ => {}
    }
    environment
}

fn run_config_case_driver(root: &Path) {
    for case in CONFIG_CASES {
        let case_root = root.join(case);
        std::fs::create_dir_all(&case_root).expect("create config case root");
        prepare_config_case(&case_root, case);
        let receipt = root.join(format!("{case}.receipt.json"));
        assert!(!receipt.exists(), "config case receipt must be create-once");
        let environment = config_case_environment(case, &case_root, &receipt);
        let mut command = Command::new(std::env::current_exe().expect("support test executable"));
        command
            .args([
                "--exact",
                "config_environment_case_child",
                "--test-threads=1",
            ])
            .env_clear()
            .envs(&environment);
        let mut child = ReapedChild::spawn(command);
        let status = child.wait_bounded(IO_TIMEOUT);
        assert!(
            status.success(),
            "support config case {case} failed: {status}"
        );
        assert!(
            receipt.is_file(),
            "support config case {case} emitted no receipt"
        );
    }
}

#[test]
fn config_environment_case_driver() {
    let Some(driver) = std::env::var(CONFIG_DRIVER).ok() else {
        return;
    };
    assert_eq!(driver, "1", "invalid support config driver mode");
    assert_eq!(
        std::env::var(CONFIG_POISON).as_deref(),
        Ok("must-not-leak"),
        "driver positive control lost its unrelated poison"
    );
    let root = PathBuf::from(std::env::var(CONFIG_ROOT).expect("config driver root"));
    run_config_case_driver(&root);
}

#[test]
fn config_environment_case_child() {
    let Some(case) = std::env::var(CONFIG_CHILD_CASE).ok() else {
        return;
    };
    assert!(
        CONFIG_CASES.contains(&case.as_str()),
        "unregistered config case {case}"
    );
    let root = PathBuf::from(std::env::var(CONFIG_ROOT).expect("config child root"));
    let receipt = PathBuf::from(std::env::var(CONFIG_RECEIPT).expect("config child receipt"));
    let environment = {
        #[cfg(target_os = "macos")]
        {
            // macOS injects this process metadata after `env_clear`; it is not
            // inherited from the driver and is outside this fixture's allowlist.
            std::env::vars()
                .filter(|(name, _)| name != "__CF_USER_TEXT_ENCODING")
                .collect::<BTreeMap<_, _>>()
        }
        #[cfg(not(target_os = "macos"))]
        {
            std::env::vars().collect::<BTreeMap<_, _>>()
        }
    };
    let expected_environment = config_case_environment(&case, &root, &receipt);
    assert_eq!(
        environment, expected_environment,
        "config child effective environment must be the exact case allowlist"
    );
    assert!(
        !environment.contains_key(CONFIG_POISON),
        "unrelated driver poison leaked into config child"
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("support config child runtime");
    let response = runtime.block_on(async {
        routes(root.clone())
            .oneshot(
                Request::get("/app/support/api/config")
                    .header("Host", "127.0.0.1")
                    .body(Body::empty())
                    .expect("config request"),
            )
            .await
            .expect("config response")
    });
    assert_eq!(response.status().as_u16(), 200, "{case}");
    let body = runtime
        .block_on(to_bytes(response.into_body(), MAX_WIRE_BYTES))
        .expect("config response body");
    let body: Value = serde_json::from_slice(&body).expect("config response json");
    assert_eq!(body["enabled"], true, "{case}");
    assert_eq!(body["portal_url"], expected_config_url(&case), "{case}");

    let receipt_value = serde_json::json!({
        "case": case,
        "effective_environment": environment,
        "enabled": body["enabled"],
        "portal_url": body["portal_url"],
    });
    std::fs::write(
        &receipt,
        serde_json::to_vec(&receipt_value).expect("serialize config case receipt"),
    )
    .expect("write config case receipt");
}

#[test]
fn config_fails_open_and_prefers_the_support_url_environment_override() {
    let root = TempDir::new().expect("support config process root");
    let mut command = Command::new(std::env::current_exe().expect("support test executable"));
    command
        .args([
            "--exact",
            "config_environment_case_driver",
            "--test-threads=1",
        ])
        .env_clear()
        .env(CONFIG_DRIVER, "1")
        .env(CONFIG_ROOT, root.path())
        .env(CONFIG_POISON, "must-not-leak");
    let mut driver = ReapedProcessGroup::spawn(command);
    let status = driver.wait_bounded(JOIN_TIMEOUT);
    assert!(status.success(), "support config driver failed: {status}");

    let mut observed = Vec::new();
    for case in CONFIG_CASES {
        let case_root = root.path().join(case);
        let receipt_path = root.path().join(format!("{case}.receipt.json"));
        let receipt: Value = serde_json::from_slice(
            &std::fs::read(&receipt_path)
                .unwrap_or_else(|error| panic!("read {case} receipt: {error}")),
        )
        .unwrap_or_else(|error| panic!("parse {case} receipt: {error}"));
        assert_eq!(receipt["case"], *case);
        assert_eq!(receipt["enabled"], true);
        assert_eq!(receipt["portal_url"], expected_config_url(case));
        assert_eq!(
            serde_json::from_value::<BTreeMap<String, String>>(
                receipt["effective_environment"].clone()
            )
            .expect("receipt effective environment"),
            config_case_environment(case, &case_root, &receipt_path),
            "{case} receipt must carry the child's complete effective environment"
        );
        observed.push(receipt["case"].as_str().expect("receipt case").to_owned());
    }
    assert_eq!(observed, CONFIG_CASES);
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

async fn with_served_connection<F, Fut>(root: PathBuf, write: F) -> (u16, String, Vec<u8>)
where
    F: FnOnce(TokioTcpStream) -> Fut,
    Fut: std::future::Future<Output = TokioTcpStream>,
{
    let listeners = bind_loopback(0).await.expect("loopback binds");
    let address = listeners.ipv4_addr().expect("IPv4 address");
    let task = tokio::spawn(async move {
        let (stream, identity) = listeners.accept().await.expect("connection accepts");
        let builder = tcp_builder();
        serve_connection(stream, routes(root), identity, &builder)
            .await
            .expect("connection serves");
    });
    let client = TokioTcpStream::connect(address)
        .await
        .expect("client connects");
    let mut client = write(client).await;
    let mut bytes = Vec::new();
    client
        .read_to_end(&mut bytes)
        .await
        .expect("response reads");
    task.await.expect("server task joins");

    let marker = b"\r\n\r\n";
    let header_end = bytes
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("response has headers");
    let headers = String::from_utf8(bytes[..header_end].to_vec()).expect("headers are text");
    let status = headers
        .split_whitespace()
        .nth(1)
        .expect("status exists")
        .parse()
        .expect("status parses");
    (status, headers, bytes[header_end + marker.len()..].to_vec())
}

#[tokio::test]
async fn support_still_rejects_one_byte_over_the_standard_cap() {
    let journal = TempDir::new_in("/var/tmp").expect("journal");
    let payload_len = STANDARD_BODY_LIMIT + 1;
    let (status, _, body) = with_served_connection(journal.path().to_path_buf(), move |mut client| {
        async move {
            let header = format!(
                "POST /app/support/api/feedback HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {payload_len}\r\n\r\n"
            );
            client
                .write_all(header.as_bytes())
                .await
                .expect("headers write");
            let chunk = [b'x'; 65_536];
            let mut remaining = payload_len;
            while remaining > 0 {
                let take = remaining.min(chunk.len());
                client
                    .write_all(&chunk[..take])
                    .await
                    .expect("payload writes");
                remaining -= take;
            }
            client
        }
    })
    .await;
    assert_eq!(status, 413, "{}", String::from_utf8_lossy(&body));
}

#[tokio::test]
async fn support_draft_rejects_one_byte_over_the_standard_cap() {
    let journal = TempDir::new_in("/var/tmp").expect("journal");
    let payload_len = STANDARD_BODY_LIMIT + 1;
    let (status, _, body) = with_served_connection(journal.path().to_path_buf(), move |mut client| {
        async move {
            let header = format!(
                "POST /app/support/api/draft HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {payload_len}\r\n\r\n"
            );
            client
                .write_all(header.as_bytes())
                .await
                .expect("headers write");
            let chunk = [b'x'; 65_536];
            let mut remaining = payload_len;
            while remaining > 0 {
                let take = remaining.min(chunk.len());
                client
                    .write_all(&chunk[..take])
                    .await
                    .expect("payload writes");
                remaining -= take;
            }
            client
        }
    })
    .await;
    assert_eq!(status, 413, "{}", String::from_utf8_lossy(&body));
}
