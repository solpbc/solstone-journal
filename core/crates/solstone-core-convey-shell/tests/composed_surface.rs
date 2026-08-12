// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{self, ErrorKind, Read};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use serde_json::{Value, json};
use socket2::{Domain, SockAddr, Socket, Type};
use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceDid};
use solstone_core_convey_shell::{RouterMergeSet, merged_router};
use tower::ServiceExt;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
const DID: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "solstone-convey-shell-composed-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary journal creates");
        Self(path)
    }

    fn establish(&self) {
        fs::create_dir_all(self.0.join("config")).expect("config directory creates");
        fs::write(
            self.0.join("config/journal.json"),
            br#"{"setup":{"completed_at":1}}"#,
        )
        .expect("config writes");
    }

    fn unestablished_config(&self) {
        fs::create_dir_all(self.0.join("config")).expect("config directory creates");
        fs::write(self.0.join("config/journal.json"), b"{}").expect("config writes");
    }

    fn corrupt(&self) {
        fs::create_dir_all(self.0.join("config")).expect("config directory creates");
        fs::write(self.0.join("config/journal.json"), b"{").expect("config writes");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn linked_device() -> AccessBasis {
    AccessBasis::LinkedDevice {
        carrier: Carrier::Direct,
        did: LinkedDeviceDid::try_from(DID).expect("fixture DID"),
    }
}

fn app(root: &TempDir) -> axum::Router {
    merged_router(root.0.clone(), RouterMergeSet::production(root.0.clone()))
}

async fn request(
    app: axum::Router,
    method: Method,
    path: &str,
    body: Body,
    basis: AccessBasis,
    content_type: Option<String>,
) -> (StatusCode, String, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .body(body)
        .expect("request builds");
    request.extensions_mut().insert(basis);
    request.headers_mut().insert(
        "X-Solstone-Protocol-Version",
        "3".parse().expect("protocol header"),
    );
    if let Some(content_type) = content_type {
        request.headers_mut().insert(
            header::CONTENT_TYPE,
            content_type.parse().expect("content type"),
        );
    }
    let response = app.oneshot(request).await.expect("router responds");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response reads");
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, content_type, body)
}

fn multipart_upload_for(segment: &str, name: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = "composed-ingest-boundary";
    let envelope = json!({
        "day": "20260811",
        "segment": segment,
        "files": [{"submitted": name}]
    })
    .to_string();
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"envelope\"\r\n\r\n{envelope}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"{name}\"\r\nX-Test: value\r\n\r\n{}\r\n--{boundary}--\r\n",
        String::from_utf8_lossy(bytes),
    )
    .into_bytes();
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn multipart_upload() -> (String, Vec<u8>) {
    multipart_upload_for("120000_1", "audio.flac", b"sound")
}

fn saturated_listener(socket_path: &Path) -> (UnixListener, Vec<Socket>) {
    let address = SockAddr::unix(socket_path).expect("socket address builds");
    let socket = Socket::new(Domain::UNIX, Type::STREAM, None).expect("listener socket creates");
    socket.bind(&address).expect("listener socket binds");
    socket.listen(1).expect("small backlog listens");
    let listener: UnixListener = socket.into();
    let mut connections = Vec::new();
    for _ in 0..128 {
        let connection = Socket::new(Domain::UNIX, Type::STREAM, None).expect("client creates");
        connection
            .set_nonblocking(true)
            .expect("client becomes nonblocking");
        match connection.connect(&address) {
            Ok(()) => connections.push(connection),
            Err(error) if error.kind() == ErrorKind::WouldBlock => return (listener, connections),
            Err(error) => panic!("expected EAGAIN while saturating Callosum backlog, got {error}"),
        }
    }
    panic!("Callosum backlog did not return EAGAIN after 128 connection attempts");
}

fn drain_listener(listener: UnixListener) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        loop {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_millis(25)));
            let mut line = String::new();
            match stream.read_to_string(&mut line) {
                Ok(_) if line.contains("\"tract\":\"observe\"") => {
                    let _ = sender.send(line);
                    return;
                }
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => {}
            }
        }
    });
    receiver
}

#[tokio::test]
async fn merged_surface_keeps_session_scopes_access_and_shell_fallback() {
    let unestablished = TempDir::new();
    assert_eq!(
        request(
            app(&unestablished),
            Method::GET,
            "/app/observer/ingest/manifest",
            Body::empty(),
            linked_device(),
            None,
        )
        .await
        .0,
        StatusCode::FOUND
    );
    assert_eq!(
        request(
            app(&unestablished),
            Method::GET,
            "/init",
            Body::empty(),
            AccessBasis::Localhost,
            None,
        )
        .await
        .0,
        StatusCode::OK
    );

    let corrupt = TempDir::new();
    corrupt.corrupt();
    assert_eq!(
        request(
            app(&corrupt),
            Method::GET,
            "/app/observer/ingest/manifest",
            Body::empty(),
            linked_device(),
            None,
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            app(&corrupt),
            Method::GET,
            "/init",
            Body::empty(),
            AccessBasis::Localhost,
            None,
        )
        .await
        .0,
        StatusCode::INTERNAL_SERVER_ERROR
    );

    let established = TempDir::new();
    established.establish();
    let (linked_status, _, _) = request(
        app(&established),
        Method::GET,
        "/app/observer/ingest/manifest",
        Body::empty(),
        linked_device(),
        None,
    )
    .await;
    assert_eq!(linked_status, StatusCode::OK);
    let (localhost_status, _, localhost_body) = request(
        app(&established),
        Method::GET,
        "/app/observer/ingest/manifest",
        Body::empty(),
        AccessBasis::Localhost,
        None,
    )
    .await;
    assert_eq!(localhost_status, StatusCode::FORBIDDEN);
    assert_eq!(localhost_body["reason_code"], "linked_device_required");
    assert_eq!(
        request(
            app(&established),
            Method::GET,
            "/app/observer/ingest",
            Body::empty(),
            linked_device(),
            None,
        )
        .await
        .0,
        StatusCode::METHOD_NOT_ALLOWED
    );
    let (fallback_status, fallback_type, _) = request(
        app(&established),
        Method::GET,
        "/does-not-exist",
        Body::empty(),
        AccessBasis::Localhost,
        None,
    )
    .await;
    assert_eq!(fallback_status, StatusCode::NOT_FOUND);
    assert!(fallback_type.starts_with("text/html"));
}

#[tokio::test]
async fn merged_link_identity_aliases_match() {
    let journal = TempDir::new();
    journal.establish();
    let network = request(
        app(&journal),
        Method::GET,
        "/app/network/api/identity",
        Body::empty(),
        AccessBasis::Localhost,
        None,
    )
    .await;
    let link = request(
        app(&journal),
        Method::GET,
        "/app/link/api/identity",
        Body::empty(),
        AccessBasis::Localhost,
        None,
    )
    .await;
    assert_eq!(network.0, StatusCode::OK);
    assert_eq!(link.0, StatusCode::OK);
    assert_eq!(network.2, link.2);
}

#[tokio::test]
async fn merged_init_mark_routes_pass_unestablished_and_stop_on_corruption() {
    let journal = TempDir::new();
    journal.unestablished_config();

    let (regenerated, _, _) = request(
        app(&journal),
        Method::POST,
        "/init/mark/regenerate",
        Body::empty(),
        AccessBasis::Localhost,
        None,
    )
    .await;
    assert_eq!(regenerated, StatusCode::OK);
    let (locked, _, _) = request(
        app(&journal),
        Method::POST,
        "/init/mark/lock",
        Body::empty(),
        AccessBasis::Localhost,
        None,
    )
    .await;
    assert_eq!(locked, StatusCode::OK);

    journal.corrupt();
    let (status, _, _) = request(
        app(&journal),
        Method::POST,
        "/init/mark/regenerate",
        Body::empty(),
        AccessBasis::Localhost,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn merged_ingest_emits_the_filtered_callosum_payload() {
    let journal = TempDir::new();
    journal.establish();
    let health = journal.0.join("health");
    fs::create_dir(&health).expect("health directory creates");
    let socket_path = health.join("callosum.sock");
    let listener = UnixListener::bind(&socket_path).expect("Callosum listener binds");
    let receiver = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("notifier connects");
        let mut line = String::new();
        stream.read_to_string(&mut line).expect("notifier writes");
        line
    });

    let (content_type, body) = multipart_upload();
    let (status, _, body_json) = request(
        app(&journal),
        Method::POST,
        "/app/observer/ingest",
        Body::from(body),
        linked_device(),
        Some(content_type),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body_json["status"], "ok");

    let line = receiver.join().expect("listener thread completes");
    assert_eq!(
        serde_json::from_str::<Value>(&line).expect("Callosum JSON line"),
        json!({
            "tract": "observe",
            "event": "observing",
            "day": "20260811",
            "segment": "120000_1",
            "stream": "device",
            "observer": DID,
            "files": ["audio.flac"]
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac11c_saturated_callosum_backlog_blocks_until_a_listener_drains() {
    let journal = TempDir::new();
    journal.establish();
    let health = journal.0.join("health");
    fs::create_dir(&health).expect("health directory creates");
    let (listener, _connections) = saturated_listener(&health.join("callosum.sock"));
    let (content_type, body) = multipart_upload();
    let upload = tokio::spawn(async move {
        request(
            app(&journal),
            Method::POST,
            "/app/observer/ingest",
            Body::from(body),
            linked_device(),
            Some(content_type),
        )
        .await
    });

    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        !upload.is_finished(),
        "the inline notifier must still be waiting for a saturated Callosum backlog"
    );

    let observed = drain_listener(listener);
    let (status, _, body) = upload.await.expect("upload task joins");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    let line = observed
        .recv_timeout(Duration::from_secs(2))
        .expect("drained listener receives observe.observing");
    assert_eq!(
        serde_json::from_str::<Value>(&line).expect("Callosum JSON line")["event"],
        "observing"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac12_two_blocked_ingests_leave_a_worker_for_an_exempt_shell_route() {
    let journal = TempDir::new();
    journal.establish();
    let health = journal.0.join("health");
    fs::create_dir(&health).expect("health directory creates");
    let (_listener, _connections) = saturated_listener(&health.join("callosum.sock"));

    let (first_content_type, first_body) = multipart_upload_for("120000_1", "first.flac", b"one");
    let first_app = app(&journal);
    let first = tokio::spawn(async move {
        request(
            first_app,
            Method::POST,
            "/app/observer/ingest",
            Body::from(first_body),
            linked_device(),
            Some(first_content_type),
        )
        .await
    });
    let (second_content_type, second_body) =
        multipart_upload_for("120100_1", "second.flac", b"two");
    let second_root = journal.0.clone();
    let second_app = merged_router(second_root.clone(), RouterMergeSet::production(second_root));
    let second = tokio::spawn(async move {
        request(
            second_app,
            Method::POST,
            "/app/observer/ingest",
            Body::from(second_body),
            linked_device(),
            Some(second_content_type),
        )
        .await
    });
    let shell_root = journal.0.clone();
    let shell_app = merged_router(shell_root.clone(), RouterMergeSet::production(shell_root));
    let shell = tokio::spawn(async move {
        request(
            shell_app,
            Method::GET,
            "/favicon.ico",
            Body::empty(),
            AccessBasis::Localhost,
            None,
        )
        .await
    });

    let (shell_status, _, _) = tokio::time::timeout(Duration::from_secs(2), shell)
        .await
        .expect("shell request completes within two notifier budgets")
        .expect("shell task joins");
    assert_eq!(shell_status, StatusCode::OK);
    for upload in [first, second] {
        let (status, _, body) = upload.await.expect("upload task joins");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
    }
    assert!(
        journal
            .0
            .join("chronicle/20260811/device/120000_1/events.jsonl")
            .exists()
    );
    assert!(
        journal
            .0
            .join("chronicle/20260811/device/120100_1/events.jsonl")
            .exists()
    );
}

#[tokio::test]
async fn ac12b_missing_callosum_socket_keeps_ingest_durable() {
    let journal = TempDir::new();
    journal.establish();
    let (content_type, body) = multipart_upload();
    let (status, _, response) = request(
        app(&journal),
        Method::POST,
        "/app/observer/ingest",
        Body::from(body),
        linked_device(),
        Some(content_type),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["status"], "ok");
    assert_eq!(
        fs::read(
            journal
                .0
                .join("chronicle/20260811/device/120000_1/audio.flac")
        )
        .expect("upload remains durable"),
        b"sound"
    );
}
