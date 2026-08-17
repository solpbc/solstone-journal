// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Socket, interrupted-I/O, and child-process transport contracts.

use std::fs;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::Child;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};
use solstone_core_spp_ratls::AttestedIo;
use solstone_core_transcribe::test_hooks::{
    ConfidentialMultipart, ParakeetConnect, ParakeetHealth, ParakeetTranscribe, SpeakerInvoke,
    confidential_multipart_exchange, invoke_speakers_child, parakeet_connect,
    parakeet_probe_health, parakeet_transcribe, recorded_convey_is_up,
};

const VALID: &str =
    r#"{"words":[{"word":"hello","start":0.0,"end":1.0,"conf":0.9}],"text":"hello"}"#;
const IO_TIMEOUT: Duration = Duration::from_secs(2);

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
                let stream = accept_with_deadline(&listener);
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
        let address = ready_rx
            .recv_timeout(IO_TIMEOUT)
            .expect("stub listener becomes ready before the deadline");
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

fn accept_with_deadline(listener: &TcpListener) -> TcpStream {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + IO_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
                stream.set_write_timeout(Some(IO_TIMEOUT)).unwrap();
                return stream;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "stub accepts a connection before the deadline"
                );
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("stub accept failed: {error}"),
        }
    }
}

fn read_confidential_request(stream: &mut TcpStream) -> Vec<u8> {
    stream.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    stream.set_write_timeout(Some(IO_TIMEOUT)).unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    let header_end = loop {
        let count = stream.read(&mut buffer).unwrap();
        assert_ne!(count, 0, "confidential request ended before its headers");
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap();
    while bytes.len() < header_end + length {
        let count = stream.read(&mut buffer).unwrap();
        assert_ne!(count, 0, "confidential request ended before its body");
        bytes.extend_from_slice(&buffer[..count]);
    }
    bytes.truncate(header_end + length);
    bytes
}

fn read_parakeet_request(mut stream: TcpStream) -> CapturedRequest {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream.set_write_timeout(Some(IO_TIMEOUT)).unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    let header_end = loop {
        let count = stream.read(&mut buffer).unwrap();
        assert_ne!(count, 0, "Parakeet request ended before its headers");
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
        assert_ne!(count, 0, "Parakeet request ended before its body");
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
        let mut stream = accept_with_deadline(&listener);
        let request = read_confidential_request(&mut stream);
        write_response(&mut stream, 200, VALID);
        request
    });
    let mut stream = TcpStream::connect(address).unwrap();
    const WAV: &[u8] = b"\x00WAV\xff";

    let response = confidential_multipart_exchange(
        &mut stream,
        &address.to_string(),
        Some("credential"),
        WAV,
        Duration::from_secs(1),
    );
    let request = handle.join().unwrap();

    let ConfidentialMultipart::Received { status, .. } = response else {
        panic!("expected received multipart response");
    };
    assert_eq!(status, 200);
    let header_end = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap()
        + 4;
    let headers = std::str::from_utf8(&request[..header_end]).unwrap();
    let boundary = headers
        .lines()
        .find_map(|line| line.strip_prefix("Content-Type: multipart/form-data; boundary="))
        .unwrap();
    assert!(boundary.starts_with("solstone-confidential-stt-"));
    let mut expected_body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
    )
    .into_bytes();
    expected_body.extend_from_slice(WAV);
    expected_body.extend_from_slice(
        format!(
            "\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"response_format\"\r\n\r\nverbose_json\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"timestamp_granularities[]=word\"\r\n\r\nword\r\n--{boundary}--\r\n"
        )
        .as_bytes(),
    );
    let expected_headers = format!(
        "POST /v1/audio/transcriptions HTTP/1.1\r\nHost: {address}\r\nContent-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {}\r\nAuthorization: Bearer credential\r\n\r\n",
        expected_body.len()
    );
    assert_eq!(&request[..header_end], expected_headers.as_bytes());
    assert_eq!(&request[header_end..], expected_body);
}

#[test]
fn multipart_transport_retries_interrupted_write() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let mut stream = accept_with_deadline(&listener);
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
        let mut stream = accept_with_deadline(&listener);
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
        let stream = accept_with_deadline(&listener);
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

struct SpeakerChildGuard(Option<Child>);

impl SpeakerChildGuard {
    fn child(&mut self) -> &mut Child {
        self.0.as_mut().expect("speaker child remains owned")
    }
}

impl Drop for SpeakerChildGuard {
    fn drop(&mut self) {
        let Some(child) = self.0.as_mut() else {
            return;
        };
        if !matches!(child.try_wait(), Ok(Some(_))) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn spawn_speaker_child(script: &str, receipt: &Path, siglog: &Path) -> SpeakerChildGuard {
    fs::write(receipt, []).unwrap();
    fs::write(siglog, []).unwrap();
    SpeakerChildGuard(Some(
        std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .env("RECEIPT", receipt)
            .env("SIGLOG", siglog)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    ))
}

fn wait_for_started(child: &mut SpeakerChildGuard, receipt: &Path) {
    let deadline = Instant::now() + IO_TIMEOUT;
    loop {
        if fs::read_to_string(receipt).unwrap() == "started" {
            return;
        }
        if let Some(status) = child.child().try_wait().unwrap() {
            panic!("speaker child exited before its ready receipt: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "speaker child writes its ready receipt before the deadline"
        );
        thread::sleep(Duration::from_millis(5));
    }
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
while :; do :; done
"#,
        &receipt,
        &siglog,
    );
    wait_for_started(&mut child, &receipt);
    let SpeakerInvoke::Failed {
        reason,
        native_exit_code,
    } = invoke_timeout(child.child())
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
while :; do :; done
"#,
        &receipt,
        &siglog,
    );
    wait_for_started(&mut child, &receipt);
    let SpeakerInvoke::Failed {
        reason,
        native_exit_code,
    } = invoke_timeout(child.child())
    else {
        panic!("expected timeout failure");
    };
    assert_eq!(reason, "timeout");
    assert_eq!(native_exit_code, Some(-9));
    assert_eq!(fs::read_to_string(&siglog).unwrap(), "TERM\n");
}
