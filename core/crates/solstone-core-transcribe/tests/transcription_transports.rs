// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Socket, interrupted-I/O, and child-process transport contracts.

use std::fs;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};
use solstone_core_spp_ratls::AttestedIo;
use solstone_core_transcribe::test_hooks::{
    ConfidentialMultipart, CoremlModelInfo, CoremlTranscribe, ParakeetConnect, ParakeetHealth,
    ParakeetTranscribe, SpeakerInvoke, confidential_multipart_exchange, coreml_get_model_info,
    coreml_transcribe_with_helper, invoke_speakers_program, parakeet_connect,
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

fn closed_loopback_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback port");
    let port = listener.local_addr().expect("listener address").port();
    drop(listener);
    port
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
    let port = closed_loopback_port();
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
fn blank_word_timing_is_a_hard_contract_error() {
    let stub = StubServer::start(vec![Reply::Http {
        status: 200,
        body: r#"{"words":[{"word":"","start":0.0,"end":1.0}],"text":"hello"}"#,
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

fn write_speaker_helper(directory: &Path, body: &str) -> std::path::PathBuf {
    let helper = directory.join("speakers-helper");
    fs::write(&helper, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mut permissions = fs::metadata(&helper).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&helper, permissions).unwrap();
    helper
}

fn invoke_timeout(program: &Path) -> SpeakerInvoke {
    #[cfg(target_os = "macos")]
    let timeout = Duration::from_millis(500);
    #[cfg(not(target_os = "macos"))]
    let timeout = Duration::from_millis(20);
    #[cfg(target_os = "macos")]
    let terminate_grace = Duration::from_millis(100);
    #[cfg(not(target_os = "macos"))]
    let terminate_grace = Duration::from_millis(20);
    invoke_speakers_program(
        program,
        b"{}",
        Path::new("input.wav"),
        timeout,
        1024,
        1024,
        terminate_grace,
        Duration::from_secs(1),
    )
}

#[cfg(unix)]
#[test]
fn responsive_speaker_child_observes_term_without_kill() {
    let temporary = tempfile::tempdir().unwrap();
    let receipt = temporary.path().join("started");
    let siglog = temporary.path().join("signals");
    let helper = write_speaker_helper(
        temporary.path(),
        &format!(
            "trap 'printf \"%s\\n\" TERM >> {}; exit 0' TERM\nprintf started > {}\nwhile :; do :; done",
            quote_path(&siglog),
            quote_path(&receipt)
        ),
    );
    let SpeakerInvoke::Failed {
        reason,
        native_exit_code,
    } = invoke_timeout(&helper)
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
    let helper = write_speaker_helper(
        temporary.path(),
        &format!(
            "trap 'printf \"%s\\n\" TERM >> {}' TERM\nprintf started > {}\nwhile :; do :; done",
            quote_path(&siglog),
            quote_path(&receipt)
        ),
    );
    let SpeakerInvoke::Failed {
        reason,
        native_exit_code,
    } = invoke_timeout(&helper)
    else {
        panic!("expected timeout failure");
    };
    assert_eq!(reason, "timeout");
    assert_eq!(native_exit_code, Some(-9));
    assert_eq!(fs::read_to_string(&siglog).unwrap(), "TERM\n");
}

#[cfg(unix)]
#[test]
fn speaker_root_exit_kills_descendant_that_holds_pipes() {
    let temporary = tempfile::tempdir().unwrap();
    let pid_receipt = temporary.path().join("descendant-pid");
    let helper = write_speaker_helper(
        temporary.path(),
        &format!(
            "cat >/dev/null\nsleep 5 &\nprintf '%s' \"$!\" > {}\nexit 0",
            quote_path(&pid_receipt)
        ),
    );
    let outcome = with_deadline(move || {
        invoke_speakers_program(
            &helper,
            b"{}",
            Path::new("input.wav"),
            Duration::from_secs(1),
            1024,
            1024,
            Duration::from_millis(20),
            Duration::from_secs(1),
        )
    });
    assert!(matches!(
        outcome,
        SpeakerInvoke::Completed { returncode: 0, .. }
    ));
    let raw_pid = fs::read_to_string(&pid_receipt)
        .unwrap()
        .trim()
        .parse::<i32>()
        .unwrap();
    let pid = rustix::process::Pid::from_raw(raw_pid).unwrap();
    let deadline = Instant::now() + IO_TIMEOUT;
    loop {
        match rustix::process::test_kill_process(pid) {
            Err(rustix::io::Errno::SRCH) => break,
            Ok(()) | Err(rustix::io::Errno::PERM) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(5));
            }
            outcome => panic!("speaker descendant survived cleanup: {outcome:?}"),
        }
    }
}

#[test]
fn speaker_program_receives_request_and_enforces_exact_capture_caps() {
    let temporary = tempfile::tempdir().unwrap();
    let request_receipt = temporary.path().join("request");
    let request_helper = write_speaker_helper(
        temporary.path(),
        &format!("cat > {}\nprintf done", quote_path(&request_receipt)),
    );
    let completed = invoke_speakers_program(
        &request_helper,
        b"request-bytes",
        Path::new("input.wav"),
        Duration::from_secs(1),
        4,
        4,
        Duration::from_millis(20),
        Duration::from_secs(1),
    );
    assert!(matches!(
        completed,
        SpeakerInvoke::Completed {
            returncode: 0,
            ref stdout,
            ref stderr,
        } if stdout == "done" && stderr.is_empty()
    ));
    assert_eq!(fs::read(&request_receipt).unwrap(), b"request-bytes");

    for (body, reason) in [
        ("printf 12345", "stdout-too-large"),
        ("printf 12345 >&2", "stderr-too-large"),
    ] {
        let helper = write_speaker_helper(temporary.path(), body);
        let SpeakerInvoke::Failed { reason: actual, .. } = invoke_speakers_program(
            &helper,
            b"",
            Path::new("input.wav"),
            Duration::from_secs(1),
            4,
            4,
            Duration::from_millis(20),
            Duration::from_secs(1),
        ) else {
            panic!("expected {reason}");
        };
        assert_eq!(actual, reason);
    }
}

const COREML_SUCCESS: &str = r#"{"transcript":"hello world!","audio_sec":1.0,"transcribe_ms":2,"rtfx":3.0,"token_timings":[{"token":"▁hel","token_id":1,"start":0.0,"end":0.1,"confidence":0.9},{"token":"lo","token_id":2,"start":0.1,"end":0.2,"confidence":0.6},{"token":"▁world","token_id":3,"start":0.2,"end":0.4,"confidence":0.8},{"token":"!","token_id":4,"start":0.4,"end":0.5,"confidence":0.7}]}"#;
const COREML_VERSION: &str = r#"{"fluidaudio_version":"0.14.0","model_version_default":"v3","swift_version":"Swift","hardware":"M4","macos_version":"26"}"#;
const LARGE_HELPER_BODY: &str = "printf '%s' '{\"transcript\":\"\",\"audio_sec\":1.0,\"transcribe_ms\":2,\"rtfx\":3.0,\"token_timings\":[],\"padding\":\"'\nhead -c 70000 /dev/zero | tr '\\0' x\nprintf '%s\\n' '\"}'";

fn write_coreml_helper(directory: &Path, body: &str) -> std::path::PathBuf {
    let helper = directory.join("parakeet-helper");
    fs::write(&helper, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mut permissions = fs::metadata(&helper).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&helper, permissions).unwrap();
    helper
}

fn quote_path(path: &Path) -> String {
    format!(
        "'{}'",
        path.display().to_string().replace('\'', "'\\\"'\\\"'")
    )
}

fn with_deadline<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = sender.send(work());
    });
    receiver
        .recv_timeout(IO_TIMEOUT)
        .expect("coreml helper finishes before the outer deadline")
}

fn transcribe_script(body: &str, timeout: Duration) -> CoremlTranscribe {
    let temporary = tempfile::tempdir().unwrap();
    transcribe_helper(
        &write_coreml_helper(temporary.path(), body),
        temporary.path(),
        "v3",
        timeout,
    )
}

fn transcribe_helper(
    helper: &Path,
    cache_dir: &Path,
    model_version: &str,
    timeout: Duration,
) -> CoremlTranscribe {
    let helper = helper.to_path_buf();
    let cache_dir = cache_dir.to_path_buf();
    let model_version = model_version.to_owned();
    with_deadline(move || {
        coreml_transcribe_with_helper(&[0.0], &helper, &cache_dir, &model_version, timeout)
    })
}

fn version_script(body: &str, model_version: &str) -> CoremlModelInfo {
    let temporary = tempfile::tempdir().unwrap();
    let helper = write_coreml_helper(temporary.path(), body).to_path_buf();
    let model_version = model_version.to_owned();
    with_deadline(move || coreml_get_model_info(&helper, &model_version, Duration::from_secs(1)))
}

#[cfg(unix)]
fn wait_until_dead(pid: u32) {
    let deadline = Instant::now() + IO_TIMEOUT;
    while process_is_live(pid) {
        assert!(Instant::now() < deadline, "grandchild {pid} should die");
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(target_os = "linux")]
fn process_is_live(pid: u32) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some((_, fields)) = stat.rsplit_once(") ") else {
        return false;
    };
    fields.bytes().next() != Some(b'Z')
}

#[cfg(target_os = "macos")]
fn process_is_live(pid: u32) -> bool {
    std::process::Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .is_ok_and(|output| output.status.success() && !output.stdout.is_empty())
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn process_is_live(pid: u32) -> bool {
    let Some(pid) = rustix::process::Pid::from_raw(pid as i32) else {
        return false;
    };
    matches!(
        rustix::process::test_kill_process(pid),
        Ok(()) | Err(rustix::io::Errno::PERM)
    )
}

fn grandchild_pid(path: &Path) -> u32 {
    fs::read_to_string(path)
        .unwrap()
        .trim()
        .parse()
        .expect("helper wrote a grandchild pid")
}

#[cfg(unix)]
#[test]
fn coreml_timed_out_helper_defers() {
    let CoremlTranscribe::Deferred { reason } =
        transcribe_script("sleep 1", Duration::from_millis(25))
    else {
        panic!("expected deferred timeout");
    };
    assert_eq!(reason, "coreml_helper_timeout");
}

#[cfg(unix)]
#[test]
fn coreml_nonzero_helper_fails() {
    let CoremlTranscribe::Failed { reason } =
        transcribe_script("echo helper-error >&2\nexit 5", Duration::from_secs(1))
    else {
        panic!("expected hard helper failure");
    };
    assert_eq!(reason, "coreml_helper_exit_failed");
}

#[cfg(unix)]
#[test]
fn coreml_helper_launch_failure_fails_with_its_own_reason() {
    let temporary = tempfile::tempdir().unwrap();
    let helper = temporary.path().join("parakeet-helper");
    fs::write(&helper, "#!/definitely/missing/sh\n").unwrap();
    let mut permissions = fs::metadata(&helper).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&helper, permissions).unwrap();
    let CoremlTranscribe::Failed { reason } =
        transcribe_helper(&helper, temporary.path(), "v3", Duration::from_secs(1))
    else {
        panic!("expected launch failure");
    };
    assert_eq!(reason, "coreml_helper_launch_failed");
}

#[cfg(unix)]
#[test]
fn coreml_malformed_helper_json_fails() {
    let CoremlTranscribe::Failed { reason } =
        transcribe_script("printf '%s\\n' not-json", Duration::from_secs(1))
    else {
        panic!("expected invalid JSON");
    };
    assert_eq!(reason, "coreml_invalid_json");
}

#[cfg(unix)]
#[test]
fn coreml_contract_violating_helper_json_fails() {
    let CoremlTranscribe::Failed { reason } = transcribe_script(
        "printf '%s\\n' '{\"transcript\":\"hello\",\"audio_sec\":1.0,\"transcribe_ms\":2,\"rtfx\":3.0,\"token_timings\":[]}'",
        Duration::from_secs(1),
    ) else {
        panic!("expected contract violation");
    };
    assert_eq!(reason, "coreml_contract_violation");
}

#[cfg(unix)]
#[test]
fn coreml_successful_helper_collapses_subwords_and_punctuation() {
    let CoremlTranscribe::Ok { words, text } = transcribe_script(
        &format!("printf '%s\\n' '{COREML_SUCCESS}'"),
        Duration::from_secs(1),
    ) else {
        panic!("expected successful transcription");
    };
    assert_eq!(text, "hello world!");
    assert_eq!(words.len(), 2);
    assert_eq!(words[0].word, " hello");
    assert_eq!(words[0].probability, 0.6);
    assert_eq!(words[1].word, " world!");
    assert_eq!(words[1].probability, 0.7);
}

#[cfg(unix)]
#[test]
fn coreml_large_helper_stdout_is_drained_before_exit() {
    let CoremlTranscribe::Ok { words, text } =
        transcribe_script(LARGE_HELPER_BODY, Duration::from_secs(1))
    else {
        panic!("expected drained large payload");
    };
    assert!(words.is_empty());
    assert!(text.is_empty());
}

#[cfg(unix)]
#[test]
fn coreml_version_probe_succeeds_only_with_a_valid_version_envelope() {
    let CoremlModelInfo::Ok {
        model,
        device,
        compute_type,
    } = version_script(
        &format!(
            "if [ \"$1\" = \"--version\" ]; then printf '%s\\n' '{COREML_VERSION}'; else printf '%s\\n' '{COREML_SUCCESS}'; fi"
        ),
        "v2",
    )
    else {
        panic!("expected version envelope");
    };
    assert_eq!(model, "parakeet-tdt-0.6b-v2");
    assert_eq!(device, "ane");
    assert_eq!(compute_type, "coreml_fp16");
}

#[cfg(unix)]
#[test]
fn coreml_version_probe_failure_is_hard_not_deferred() {
    let CoremlModelInfo::Failed { reason } = version_script("exit 5", "v3") else {
        panic!("expected hard version-probe failure");
    };
    assert_eq!(reason, "coreml_version_probe_failed");
}

#[cfg(unix)]
#[test]
fn coreml_helper_receives_direct_coreml_argv() {
    let temporary = tempfile::tempdir().unwrap();
    let arguments = temporary.path().join("arguments");
    let helper = write_coreml_helper(
        temporary.path(),
        &format!(
            "printf '%s\\n' \"$@\" > {}\nprintf '%s\\n' '{COREML_SUCCESS}'",
            quote_path(&arguments)
        ),
    );
    let cache_dir = temporary.path().join("cache");
    assert!(matches!(
        transcribe_helper(&helper, &cache_dir, "v2", Duration::from_secs(1)),
        CoremlTranscribe::Ok { .. }
    ));
    let recorded = fs::read_to_string(arguments).unwrap();
    let recorded: Vec<_> = recorded.lines().collect();
    assert_eq!(
        recorded[..4],
        ["--cache-dir", cache_dir.to_str().unwrap(), "--model", "v2"]
    );
    assert!(recorded[4].ends_with(".wav"));
}

#[cfg(unix)]
#[test]
fn coreml_root_exit_kills_descendant_that_holds_pipes() {
    let temporary = tempfile::tempdir().unwrap();
    let pid_file = temporary.path().join("grandchild");
    let helper = write_coreml_helper(
        temporary.path(),
        &format!(
            "sleep 30 &\nprintf '%s\\n' $! > {}\nprintf '%s\\n' '{COREML_SUCCESS}'",
            quote_path(&pid_file)
        ),
    );
    assert!(matches!(
        transcribe_helper(&helper, temporary.path(), "v3", Duration::from_secs(1)),
        CoremlTranscribe::Ok { .. }
    ));
    wait_until_dead(grandchild_pid(&pid_file));
}

#[cfg(unix)]
#[test]
fn coreml_timeout_kills_helper_process_group() {
    let temporary = tempfile::tempdir().unwrap();
    let pid_file = temporary.path().join("grandchild");
    #[cfg(target_os = "macos")]
    let timeout = Duration::from_secs(1);
    #[cfg(not(target_os = "macos"))]
    let timeout = Duration::from_millis(200);
    let helper = write_coreml_helper(
        temporary.path(),
        &format!(
            "sleep 30 &\nprintf '%s\\n' $! > {}\nsleep 30",
            quote_path(&pid_file)
        ),
    );
    let CoremlTranscribe::Deferred { reason } =
        transcribe_helper(&helper, temporary.path(), "v3", timeout)
    else {
        panic!("expected group-wide timeout");
    };
    assert_eq!(reason, "coreml_helper_timeout");
    wait_until_dead(grandchild_pid(&pid_file));
}

#[cfg(unix)]
#[test]
fn coreml_large_valid_output_joins_before_deadline() {
    let started = Instant::now();
    assert!(matches!(
        transcribe_script(LARGE_HELPER_BODY, Duration::from_secs(1)),
        CoremlTranscribe::Ok { .. }
    ));
    assert!(started.elapsed() < IO_TIMEOUT);
}
