// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Real-socket claims for `UreqPortalTransport`. Do not run these bodies from
//! `--lib`; this target is listed and compiled only during routine validation.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use base64::Engine;
use socket2::{Domain, Socket, Type};
use solstone_core_support_portal::{PortalClient, PortalClientError};
use tempfile::TempDir;

const IO_TIMEOUT: Duration = Duration::from_secs(2);
const JOIN_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_REQUEST_BYTES: usize = 1024 * 1024;

struct CapturedRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

struct HttpReply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
    truncated: bool,
}

struct LoopbackPortal {
    base_url: String,
    log: Arc<Mutex<Vec<CapturedRequest>>>,
    stop: Arc<AtomicBool>,
    wake: SocketAddr,
    thread: Option<JoinHandle<()>>,
}

impl LoopbackPortal {
    fn new(replies: Vec<HttpReply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback fake");
        listener.set_nonblocking(true).expect("nonblocking fake");
        let address = listener.local_addr().expect("loopback address");
        let log = Arc::new(Mutex::new(Vec::new()));
        let thread_log = log.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = thread::spawn(move || {
            let mut replies: VecDeque<_> = replies.into();
            run_accept_loop(
                || listener.accept(),
                &thread_stop,
                |mut stream| {
                    let request = read_request(&mut stream);
                    thread_log.lock().expect("loopback log").push(request);
                    let reply = replies.pop_front().unwrap_or(HttpReply {
                        status: 500,
                        headers: Vec::new(),
                        body: "unexpected request".to_owned(),
                        truncated: false,
                    });
                    write_reply(&mut stream, reply);
                },
            );
        });
        Self {
            base_url: format!("http://{address}"),
            log,
            stop,
            wake: address,
            thread: Some(thread),
        }
    }

    fn url(&self) -> &str {
        &self.base_url
    }

    fn log(&self) -> Vec<CapturedRequest> {
        self.log.lock().expect("loopback log").clone()
    }
}

impl Drop for LoopbackPortal {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Ok(stream) = TcpStream::connect_timeout(&self.wake, IO_TIMEOUT) {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Some(thread) = self.thread.take() {
            join_bounded(thread, "loopback fake thread");
        }
    }
}

impl Clone for CapturedRequest {
    fn clone(&self) -> Self {
        Self {
            method: self.method.clone(),
            path: self.path.clone(),
            headers: self.headers.clone(),
            body: self.body.clone(),
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
                    .expect("blocking loopback client");
                stream
                    .set_read_timeout(Some(IO_TIMEOUT))
                    .expect("bound loopback read");
                stream
                    .set_write_timeout(Some(IO_TIMEOUT))
                    .expect("bound loopback write");
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
    let remaining = MAX_REQUEST_BYTES.saturating_sub(raw.len());
    if remaining == 0 {
        return 0;
    }
    let capacity = remaining.min(buffer.len());
    let read = stream.read(&mut buffer[..capacity]).unwrap_or(0);
    raw.extend_from_slice(&buffer[..read]);
    read
}

fn read_request(stream: &mut TcpStream) -> CapturedRequest {
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
    let path = words.next().unwrap_or_default().to_owned();
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
            let before = encoded.len();
            if read_more(stream, &mut encoded, &mut buffer) == 0 {
                break encoded;
            }
            debug_assert!(encoded.len() > before);
        }
    } else {
        while raw.len().saturating_sub(header_end) < length {
            if read_more(stream, &mut raw, &mut buffer) == 0 {
                break;
            }
        }
        raw.get(header_end..).unwrap_or_default().to_vec()
    };
    CapturedRequest {
        method,
        path,
        headers,
        body,
    }
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

fn write_reply(stream: &mut TcpStream, reply: HttpReply) {
    if reply.truncated {
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\nshort");
        return;
    }
    let reason = match reply.status {
        200 => "OK",
        302 => "Found",
        401 => "Unauthorized",
        409 => "Conflict",
        500 => "Internal Server Error",
        _ => "Response",
    };
    let mut response = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reply.body.len()
    );
    for (name, value) in reply.headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str("\r\n");
    response.push_str(&reply.body);
    let _ = stream.write_all(response.as_bytes());
}

fn proof_payload(proof: &str) -> serde_json::Value {
    let payload = proof.split('.').nth(1).unwrap();
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn seeded_client(portal_url: &str, dir: &std::path::Path) -> PortalClient {
    std::fs::write(
        dir.join("keypair.pem"),
        include_bytes!("../../../fixtures/support_portal_golden_nonproduction/keypair.pem"),
    )
    .unwrap();
    std::fs::write(dir.join("token.json"), r#"{"access_token":"loop-token"}"#).unwrap();
    PortalClient::new(portal_url, dir, Some("loop".to_owned()), false).unwrap()
}

#[test]
fn real_transport_does_not_follow_redirects_and_sends_dpop_headers() {
    let target = LoopbackPortal::new(vec![]);
    let redirect = LoopbackPortal::new(vec![HttpReply {
        status: 302,
        headers: vec![("Location".to_owned(), format!("{}/reached", target.url()))],
        body: String::new(),
        truncated: false,
    }]);
    let dir = TempDir::new().unwrap();
    let mut client = seeded_client(redirect.url(), dir.path());
    let response =
        solstone_core_support_portal::test_support::authed_get(&mut client, "/read").unwrap();
    assert_eq!(response.status, 302);
    let request = redirect.log().pop().expect("redirect saw one request");
    assert_eq!(request.path, "/read");
    assert_eq!(
        request
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
            .map(|(_, value)| value.as_str()),
        Some("DPoP loop-token")
    );
    let proof = request
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("dpop"))
        .map(|(_, value)| value)
        .unwrap();
    assert_eq!(
        proof_payload(proof)["htu"],
        format!("{}/read", redirect.url())
    );
    assert!(target.log().is_empty());
}

#[test]
fn real_transport_sends_literal_multipart_form_bytes() {
    let portal = LoopbackPortal::new(vec![HttpReply {
        status: 200,
        headers: Vec::new(),
        body: r#"{"attachment_id":1,"status":"open"}"#.to_owned(),
        truncated: false,
    }]);
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("note.txt");
    std::fs::write(&file_path, b"form-payload-bytes").unwrap();
    let mut client = seeded_client(portal.url(), dir.path());
    client
        .attach_file(7, &file_path, "attach-1", 0, Some("note.txt"), None)
        .expect("attachment request");
    let request = portal
        .log()
        .into_iter()
        .find(|entry| entry.path == "/api/tickets/7/attachments")
        .expect("attachment request reached the portal");
    let content_type = request
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.as_str())
        .unwrap_or_default();
    assert!(
        content_type.starts_with("multipart/form-data;"),
        "{content_type}"
    );
    let boundary = content_type
        .split("boundary=")
        .nth(1)
        .expect("multipart boundary");
    let body = String::from_utf8_lossy(&request.body);
    assert!(
        body.contains(boundary),
        "body must include the Form boundary"
    );
    assert!(
        body.contains("Content-Disposition:") && body.contains("filename="),
        "body must include a multipart filename: {body}"
    );
    assert!(
        request
            .body
            .windows(b"form-payload-bytes".len())
            .any(|window| window == b"form-payload-bytes"),
        "body must include the file payload"
    );
}

#[test]
fn real_transport_maps_connection_refused_to_transport_error() {
    // Hold a bound but non-listening TCP socket so no other process can claim
    // the ephemeral port between address selection and the client connect.
    let socket = Socket::new(Domain::IPV4, Type::STREAM, None).expect("closed-port socket");
    socket
        .bind(&SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0).into())
        .expect("bind closed-port probe");
    let address = socket
        .local_addr()
        .expect("closed-port address")
        .as_socket()
        .expect("IP closed-port address");
    let dir = TempDir::new().unwrap();
    let mut client =
        PortalClient::new(format!("http://{address}"), dir.path(), None, false).unwrap();
    assert!(matches!(
        client.fetch_tos(),
        Err(PortalClientError::Transport { .. })
    ));
}

#[test]
fn real_transport_maps_truncated_response_to_transport_error() {
    let portal = LoopbackPortal::new(vec![HttpReply {
        status: 200,
        headers: Vec::new(),
        body: String::new(),
        truncated: true,
    }]);
    let dir = TempDir::new().unwrap();
    let mut client = PortalClient::new(portal.url(), dir.path(), None, false).unwrap();
    assert!(matches!(
        client.fetch_tos(),
        Err(PortalClientError::Transport { .. })
    ));
}

#[test]
fn loopback_helper_terminates_on_accept_failure() {
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
