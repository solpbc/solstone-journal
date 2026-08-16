// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Socket, interrupted-I/O, and child-process transport contracts.

use std::fs;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use socket2::{Domain, Protocol, Socket, Type};
use solstone_core_spp_ratls::AttestedIo;
use solstone_core_transcribe::test_hooks::{
    ConfidentialMultipart, ParakeetConnect, ParakeetHealth, ParakeetTranscribe, SpeakerInvoke,
    confidential_multipart_exchange, invoke_speakers_child, parakeet_connect,
    parakeet_probe_health, parakeet_transcribe, recorded_convey_is_up,
};

const VALID: &str =
    r#"{"words":[{"word":"hello","start":0.0,"end":1.0,"conf":0.9}],"text":"hello"}"#;

enum Reply {
    Http { status: u16, body: &'static str },
    Close,
    Sleep(Duration),
}

#[derive(Debug)]
struct CapturedRequest {
    method: String,
    path: String,
    body: String,
}

struct StubServer {
    base_url: String,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    handle: thread::JoinHandle<()>,
}

impl StubServer {
    fn start(replies: Vec<Reply>) -> Self {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let handle = thread::spawn(move || {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            ready_tx.send(address).unwrap();
            for reply in replies {
                let (stream, _) = listener.accept().unwrap();
                let request = read_parakeet_request(stream.try_clone().unwrap());
                recorded.lock().unwrap().push(request);
                match reply {
                    Reply::Http { status, body } => {
                        let mut stream = stream;
                        write_response(&mut stream, status, body);
                    }
                    Reply::Close => drop(stream),
                    Reply::Sleep(duration) => thread::sleep(duration),
                }
            }
        });
        let address = ready_rx.recv().unwrap();
        Self {
            base_url: format!("http://{address}"),
            requests,
            handle,
        }
    }

    fn finish(self) -> Vec<CapturedRequest> {
        let Self {
            requests, handle, ..
        } = self;
        handle.join().unwrap();
        Arc::try_unwrap(requests).unwrap().into_inner().unwrap()
    }
}

fn read_confidential_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    let header_end = loop {
        let count = stream.read(&mut buffer).unwrap();
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let length = headers
        .lines()
        .find_map(|line| {
            line.strip_prefix("Content-Length:")?
                .trim()
                .parse::<usize>()
                .ok()
        })
        .unwrap();
    while bytes.len() < header_end + length {
        let count = stream.read(&mut buffer).unwrap();
        bytes.extend_from_slice(&buffer[..count]);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn read_parakeet_request(mut stream: TcpStream) -> CapturedRequest {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    let header_end = loop {
        let count = stream.read(&mut buffer).unwrap();
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]).into_owned();
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let count = stream.read(&mut buffer).unwrap();
        bytes.extend_from_slice(&buffer[..count]);
    }
    let request_line = headers.lines().next().unwrap();
    let mut request_line = request_line.split_whitespace();
    CapturedRequest {
        method: request_line.next().unwrap().to_owned(),
        path: request_line.next().unwrap().to_owned(),
        body: String::from_utf8_lossy(&bytes[header_end..]).into_owned(),
    }
}

fn write_response(stream: &mut TcpStream, status: u16, body: &str) {
    let status_text = if status == 200 { "OK" } else { "Error" };
    let response = format!(
        "HTTP/1.1 {status} {status_text}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

fn write_port(journal_path: &Path, name: &str, port: u16) {
    let health = journal_path.join("health");
    fs::create_dir_all(&health).unwrap();
    fs::write(health.join(name), port.to_string()).unwrap();
}

fn stub_port(base_url: &str) -> u16 {
    base_url.rsplit(':').next().unwrap().parse().unwrap()
}

fn reserve_unlistening_loopback() -> (Socket, u16) {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).unwrap();
    socket
        .bind(&SocketAddr::from((Ipv4Addr::LOCALHOST, 0)).into())
        .unwrap();
    let port = socket
        .local_addr()
        .unwrap()
        .as_socket()
        .expect("ipv4 socket")
        .port();
    (socket, port)
}

struct InterruptWriteStream {
    stream: TcpStream,
    interrupt_write: bool,
    interrupt_flush: bool,
    interrupt_timeout: bool,
}

impl InterruptWriteStream {
    fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            interrupt_write: true,
            interrupt_flush: true,
            interrupt_timeout: true,
        }
    }
}

impl Read for InterruptWriteStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.stream.read(buffer)
    }
}

impl Write for InterruptWriteStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.interrupt_write {
            self.interrupt_write = false;
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        self.stream.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.interrupt_flush {
            self.interrupt_flush = false;
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        self.stream.flush()
    }
}

impl AttestedIo for InterruptWriteStream {
    fn set_io_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        if self.interrupt_timeout {
            self.interrupt_timeout = false;
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        self.stream.set_read_timeout(timeout)?;
        self.stream.set_write_timeout(timeout)
    }
}

struct InterruptReadStream {
    stream: TcpStream,
    interrupt_read: bool,
}

impl InterruptReadStream {
    fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            interrupt_read: true,
        }
    }
}

impl Read for InterruptReadStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.interrupt_read {
            self.interrupt_read = false;
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        self.stream.read(buffer)
    }
}

impl Write for InterruptReadStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.stream.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

impl AttestedIo for InterruptReadStream {
    fn set_io_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        self.stream.set_read_timeout(timeout)?;
        self.stream.set_write_timeout(timeout)
    }
}

#[test]
fn multipart_request_uses_one_plain_tcp_channel_with_expected_fields() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_confidential_request(&mut stream);
        write_response(&mut stream, 200, VALID);
        request
    });
    let mut stream = TcpStream::connect(address).unwrap();

    let response = confidential_multipart_exchange(
        &mut stream,
        &address.to_string(),
        Some("credential"),
        b"WAV",
        Duration::from_secs(1),
    );
    let request = handle.join().unwrap();

    let ConfidentialMultipart::Received { status, .. } = response else {
        panic!("expected received multipart response");
    };
    assert_eq!(status, 200);
    assert!(request.starts_with("POST /v1/audio/transcriptions HTTP/1.1\r\n"));
    assert!(request.contains("Authorization: Bearer credential\r\n"));
    assert!(!request.contains("x-sol-device"));
    let boundary = request
        .lines()
        .find_map(|line| line.strip_prefix("Content-Type: multipart/form-data; boundary="))
        .unwrap();
    assert!(boundary.starts_with("solstone-confidential-stt-"));
    assert!(request.contains(&format!("--{boundary}\r\n")));
    let file = request
        .find("name=\"file\"; filename=\"audio.wav\"")
        .unwrap();
    let format = request.find("name=\"response_format\"").unwrap();
    let words = request
        .find("name=\"timestamp_granularities[]=word\"")
        .unwrap();
    assert!(file < format && format < words);
}

#[test]
fn multipart_transport_retries_interrupted_write() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = read_confidential_request(&mut stream);
        write_response(&mut stream, 200, VALID);
    });
    let stream = TcpStream::connect(address).unwrap();
    let mut stream = InterruptWriteStream::new(stream);

    let response = confidential_multipart_exchange(
        &mut stream,
        &address.to_string(),
        None,
        b"WAV",
        Duration::from_secs(1),
    );
    handle.join().unwrap();

    let ConfidentialMultipart::Received { status, .. } = response else {
        panic!("expected received multipart response");
    };
    assert_eq!(status, 200);
}

#[test]
fn multipart_transport_retries_interrupted_read() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = read_confidential_request(&mut stream);
        write_response(&mut stream, 200, VALID);
    });
    let stream = TcpStream::connect(address).unwrap();
    let mut stream = InterruptReadStream::new(stream);

    let response = confidential_multipart_exchange(
        &mut stream,
        &address.to_string(),
        None,
        b"WAV",
        Duration::from_secs(1),
    );
    handle.join().unwrap();

    let ConfidentialMultipart::Received { status, .. } = response else {
        panic!("expected received multipart response");
    };
    assert_eq!(status, 200);
}

#[test]
fn transport_failure_defers_without_a_second_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        drop(stream);
    });
    let mut stream = TcpStream::connect(address).unwrap();

    let response = confidential_multipart_exchange(
        &mut stream,
        &address.to_string(),
        None,
        b"WAV",
        Duration::from_secs(1),
    );
    handle.join().unwrap();

    let ConfidentialMultipart::Unreachable { reason } = response else {
        panic!("expected unreachable multipart exchange");
    };
    assert_eq!(reason, "hosted_transcribe_unreachable");
}

#[test]
fn recorded_convey_marks_solstone_up_when_listener_ready() {
    let temporary = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    write_port(
        temporary.path(),
        "convey.port",
        listener.local_addr().unwrap().port(),
    );
    assert!(recorded_convey_is_up(temporary.path()));
    drop(listener);
}

#[test]
fn recorded_convey_is_down_when_port_is_bound_but_not_listening() {
    let temporary = tempfile::tempdir().unwrap();
    let (_reserved, port) = reserve_unlistening_loopback();
    write_port(temporary.path(), "convey.port", port);
    assert!(!recorded_convey_is_up(temporary.path()));
}

#[cfg(target_os = "linux")]
#[test]
fn health_and_transcription_use_the_expected_wire_paths_and_form_fields() {
    let temporary = tempfile::tempdir().unwrap();
    let stub = StubServer::start(vec![
        Reply::Http {
            status: 200,
            body: "{}",
        },
        Reply::Http {
            status: 200,
            body: r#"{"words":[],"text":""}"#,
        },
    ]);
    write_port(
        temporary.path(),
        "parakeet-cpp.port",
        stub_port(&stub.base_url),
    );

    assert!(matches!(
        parakeet_connect(temporary.path()),
        ParakeetConnect::Ready
    ));
    let ParakeetTranscribe::Ok { words, .. } =
        parakeet_transcribe(&stub.base_url, b"wav", Duration::from_secs(1))
    else {
        panic!("expected empty transcription");
    };
    assert!(words.is_empty());
    let requests = stub.finish();

    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/health");
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].path, "/v1/audio/transcriptions");
    assert!(requests[1].body.contains("response_format"));
    assert!(requests[1].body.contains("verbose_json"));
    assert!(requests[1].body.contains("timestamp_granularities[]"));
    assert!(requests[1].body.contains("word"));
}

#[test]
fn non_200_health_defers_with_server_not_ready() {
    let temporary = tempfile::tempdir().unwrap();
    let stub = StubServer::start(vec![Reply::Http {
        status: 503,
        body: "loading",
    }]);
    write_port(
        temporary.path(),
        "parakeet-cpp.port",
        stub_port(&stub.base_url),
    );

    let ParakeetConnect::Deferred { reason } = parakeet_connect(temporary.path()) else {
        panic!("expected deferred connect");
    };
    stub.finish();
    assert_eq!(reason, "server_not_ready");
}

#[test]
fn refused_health_defers_with_server_not_ready() {
    let temporary = tempfile::tempdir().unwrap();
    let (_reserved, port) = reserve_unlistening_loopback();
    write_port(temporary.path(), "parakeet-cpp.port", port);

    let ParakeetConnect::Deferred { reason } = parakeet_connect(temporary.path()) else {
        panic!("expected deferred connect");
    };
    assert_eq!(reason, "server_not_ready");
}

#[cfg(target_os = "linux")]
#[test]
fn disconnected_transcription_defers_with_server_disconnected() {
    let stub = StubServer::start(vec![Reply::Close]);

    let ParakeetTranscribe::Deferred { reason } =
        parakeet_transcribe(&stub.base_url, b"wav", Duration::from_secs(1))
    else {
        panic!("expected deferred transcription");
    };
    stub.finish();
    assert_eq!(reason, "server_disconnected");
}

#[cfg(target_os = "linux")]
#[test]
fn timed_out_transcription_defers_with_read_timeout() {
    let stub = StubServer::start(vec![Reply::Sleep(Duration::from_millis(150))]);

    let ParakeetTranscribe::Deferred { reason } =
        parakeet_transcribe(&stub.base_url, b"wav", Duration::from_millis(25))
    else {
        panic!("expected deferred transcription");
    };
    stub.finish();
    assert_eq!(reason, "read_timeout");
}

#[cfg(target_os = "linux")]
#[test]
fn refused_transcription_defers_with_connect_error() {
    let (_reserved, port) = reserve_unlistening_loopback();
    let base_url = format!("http://127.0.0.1:{port}");

    let ParakeetTranscribe::Deferred { reason } =
        parakeet_transcribe(&base_url, b"wav", Duration::from_millis(50))
    else {
        panic!("expected deferred transcription");
    };
    assert_eq!(reason, "connect_error");
}

#[cfg(target_os = "linux")]
#[test]
fn live_http_500_is_a_hard_transcription_error() {
    let stub = StubServer::start(vec![Reply::Http {
        status: 500,
        body: "broken",
    }]);

    let ParakeetTranscribe::Failed { reason } =
        parakeet_transcribe(&stub.base_url, b"wav", Duration::from_secs(1))
    else {
        panic!("expected hard transcription failure");
    };
    stub.finish();
    assert_eq!(reason, "transcription_http_error");
}

#[cfg(target_os = "linux")]
#[test]
fn malformed_live_json_is_a_hard_error() {
    let stub = StubServer::start(vec![Reply::Http {
        status: 200,
        body: "not-json",
    }]);

    let ParakeetTranscribe::Failed { reason } =
        parakeet_transcribe(&stub.base_url, b"wav", Duration::from_secs(1))
    else {
        panic!("expected hard transcription failure");
    };
    stub.finish();
    assert_eq!(reason, "invalid_json");
}

#[cfg(target_os = "linux")]
#[test]
fn text_without_word_timings_is_a_hard_contract_error() {
    let stub = StubServer::start(vec![Reply::Http {
        status: 200,
        body: r#"{"words":[],"text":"hello"}"#,
    }]);

    let ParakeetTranscribe::Failed { reason } =
        parakeet_transcribe(&stub.base_url, b"wav", Duration::from_secs(1))
    else {
        panic!("expected hard transcription failure");
    };
    stub.finish();
    assert_eq!(reason, "contract_violation");
}

#[test]
fn health_probe_accepts_only_http_200() {
    let stub = StubServer::start(vec![Reply::Http {
        status: 204,
        body: "",
    }]);

    assert!(matches!(
        parakeet_probe_health(&stub.base_url, Duration::from_secs(1)),
        ParakeetHealth::NotReady
    ));
    stub.finish();
}

fn spawn_speaker_child(script: &str, receipt: &Path, siglog: &Path) -> std::process::Child {
    let status = std::process::Command::new("mkfifo")
        .arg(receipt)
        .status()
        .unwrap();
    assert!(status.success(), "mkfifo");
    fs::write(siglog, []).unwrap();
    std::process::Command::new("sh")
        .arg("-c")
        .arg(script)
        .env("RECEIPT", receipt)
        .env("SIGLOG", siglog)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap()
}

fn wait_for_started(receipt: &Path) {
    let mut started = fs::File::open(receipt).unwrap();
    let mut buf = String::new();
    started.read_to_string(&mut buf).unwrap();
    assert_eq!(buf, "started");
}

fn invoke_timeout(child: &mut std::process::Child) -> SpeakerInvoke {
    invoke_speakers_child(
        child,
        b"{}",
        Path::new("input.wav"),
        Duration::from_millis(20),
        1024,
        1024,
        Duration::from_millis(20),
        Duration::from_secs(1),
    )
}

#[cfg(unix)]
#[test]
fn responsive_speaker_child_observes_term_without_kill() {
    let temporary = tempfile::tempdir().unwrap();
    let receipt = temporary.path().join("started");
    let siglog = temporary.path().join("signals");
    let mut child = spawn_speaker_child(
        r#"trap 'printf "%s\n" TERM >> "$SIGLOG"; exit 0' TERM
printf started > "$RECEIPT"
sleep 5 & wait
"#,
        &receipt,
        &siglog,
    );
    wait_for_started(&receipt);
    let SpeakerInvoke::Failed {
        reason,
        native_exit_code,
    } = invoke_timeout(&mut child)
    else {
        panic!("expected timeout failure");
    };
    assert_eq!(reason, "timeout");
    assert_eq!(native_exit_code, Some(0));
    assert_eq!(fs::read_to_string(&siglog).unwrap(), "TERM\n");
}

#[cfg(unix)]
#[test]
fn ignoring_speaker_child_observes_term_before_kill() {
    let temporary = tempfile::tempdir().unwrap();
    let receipt = temporary.path().join("started");
    let siglog = temporary.path().join("signals");
    let mut child = spawn_speaker_child(
        r#"trap 'printf "%s\n" TERM >> "$SIGLOG"' TERM
printf started > "$RECEIPT"
sleep 5 & wait
"#,
        &receipt,
        &siglog,
    );
    wait_for_started(&receipt);
    let SpeakerInvoke::Failed {
        reason,
        native_exit_code,
    } = invoke_timeout(&mut child)
    else {
        panic!("expected timeout failure");
    };
    assert_eq!(reason, "timeout");
    assert_eq!(native_exit_code, Some(-9));
    assert_eq!(fs::read_to_string(&siglog).unwrap(), "TERM\n");
}
