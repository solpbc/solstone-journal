// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration as Wait, Instant};

struct Server {
    url: String,
    entered: mpsc::Receiver<String>,
    release: mpsc::Sender<()>,
    block_next: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let (entered_tx, entered) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let block_next = Arc::new(AtomicBool::new(false));
        let server_block = Arc::clone(&block_next);
        let worker = thread::spawn(move || {
            while !server_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Windows hands back an accepted socket in the listener's
                        // non-blocking mode (Linux does not), so a read could
                        // find the request not yet arrived and fail WouldBlock.
                        stream.set_nonblocking(false).unwrap();
                        let body = read_request(&mut stream);
                        let text = body["messages"][0]["content"].as_str().unwrap().to_owned();
                        let blocked = text == "long" || server_block.swap(false, Ordering::AcqRel);
                        entered_tx.send(text).unwrap();
                        if blocked {
                            release_rx.recv_timeout(Wait::from_secs(10)).unwrap();
                        }
                        let body =
                            json!({"choices":[{"message":{"content":"OK"},"finish_reason":"stop"}],
                            "usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}})
                            .to_string();
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        // Timeout controls deliberately leave before the response.
                        let _ = stream.write_all(response.as_bytes());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Wait::from_millis(2));
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                }
            }
        });
        Self {
            url,
            entered,
            release,
            block_next,
            stop,
            worker: Some(worker),
        }
    }

    fn entered(&self) -> String {
        self.entered.recv_timeout(Wait::from_secs(10)).unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.release.send(());
        let worker = self.worker.take().unwrap().join();
        // A second panic while a failed test unwinds aborts the whole binary
        // and loses every captured message, so a worker panic fails the test
        // only when the test is not already failing.
        if !thread::panicking() {
            worker.unwrap();
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Value {
    stream.set_read_timeout(Some(Wait::from_secs(5))).unwrap();
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        assert!(header.len() < 8192);
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        header.push(byte[0]);
    }
    let header = std::str::from_utf8(&header).unwrap();
    assert!(header.starts_with("POST "), "{header}");
    let length: usize = header
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .unwrap();
    assert!(length < 16384);
    let mut body = vec![0; length];
    stream.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn config(url: &str) -> Map<String, Value> {
    json!({"providers":{"active":{"provider":"local"},"local":{
        "endpoint_url":url,"served_model_id":"model","served_context_window":32768}}})
    .as_object()
    .unwrap()
    .clone()
}

fn save_config(journal: &Path, config: &Map<String, Value>) {
    std::fs::create_dir_all(journal.join("config")).unwrap();
    std::fs::write(
        journal.join("config/journal.json"),
        serde_json::to_vec(config).unwrap(),
    )
    .unwrap();
}

fn request(text: &str, context: &str, timeout: f64) -> GenerateRequest {
    GenerateRequest {
        id: None,
        context: context.into(),
        contents: vec![ContentPart::Text { text: text.into() }],
        system_instruction: None,
        temperature: 0.0,
        max_output_tokens: 512,
        timeout_s: Some(timeout),
        json_output: false,
        json_schema: None,
        attempt_index: 0,
        exclusive_admission: false,
        overflow_text: None,
        transport_retries: Some(0),
    }
}

fn dispatch(
    request: &GenerateRequest,
    journal: &Path,
    config: &Map<String, Value>,
) -> GenerateResponse {
    solstone_core_generate_wire::respond_with_config(
        request,
        &solstone_core_generate_wire::EndpointRuntime::default(),
        journal,
        config,
        |_| None,
        &Cancellation::never(),
    )
    .unwrap()
}

struct ShortProbe;
impl OwnerProbe for ShortProbe {
    fn process_overrides(&self) -> bool {
        false
    }
    fn bundled_runtime_fingerprint(&self, _: &Path, _: &Map<String, Value>) -> Option<String> {
        None
    }
    fn assess_bundled_runtime(
        &self,
        _: &Path,
        _: Option<&str>,
    ) -> BundledRuntimePrerequisiteAssessment {
        panic!("own endpoint never acquires bundled inference")
    }
    fn execute_generate(
        &self,
        request: &GenerateRequest,
        journal: &Path,
        config: &Map<String, Value>,
        cancellation: &Cancellation,
    ) -> Result<GenerateResponse, ClientError> {
        let mut request = request.clone();
        request.timeout_s = Some(0.5);
        RespondWithConfigProbe {
            runtime: solstone_core_generate_wire::EndpointRuntime::default(),
        }
        .execute_generate(&request, journal, config, cancellation)
    }
}

/// `ShortProbe` on the probe's own deadline, for a check that must see a closed
/// port refused. Linux refuses a loopback connect at once, but Windows retries
/// the refused connect for longer than `ShortProbe`'s half second, so there the
/// short check ends as a timeout before the refusal arrives.
struct RefusalProbe;
impl OwnerProbe for RefusalProbe {
    fn process_overrides(&self) -> bool {
        ShortProbe.process_overrides()
    }
    fn bundled_runtime_fingerprint(
        &self,
        journal: &Path,
        config: &Map<String, Value>,
    ) -> Option<String> {
        ShortProbe.bundled_runtime_fingerprint(journal, config)
    }
    fn assess_bundled_runtime(
        &self,
        journal: &Path,
        expected: Option<&str>,
    ) -> BundledRuntimePrerequisiteAssessment {
        ShortProbe.assess_bundled_runtime(journal, expected)
    }
    fn execute_generate(
        &self,
        request: &GenerateRequest,
        journal: &Path,
        config: &Map<String, Value>,
        cancellation: &Cancellation,
    ) -> Result<GenerateResponse, ClientError> {
        RespondWithConfigProbe {
            runtime: solstone_core_generate_wire::EndpointRuntime::default(),
        }
        .execute_generate(request, journal, config, cancellation)
    }
}

struct NoSpp;
impl OwnerSppPrerequisite for NoSpp {
    fn assess(&self, _: &Path, _: &Map<String, Value>, _: DateTime<Utc>) -> Value {
        panic!("own endpoint never attests")
    }
}

fn options() -> OwnerRefreshOptions {
    OwnerRefreshOptions {
        expected_fingerprint: None,
        expected_active_fingerprint: false,
        expect_active_fingerprint_absent: false,
    }
}

fn await_tickets(journal: &Path, count: usize) {
    let line = solstone_core_local::admission::model_call_line_dir(journal);
    let deadline = Instant::now() + Wait::from_secs(5);
    loop {
        let actual = std::fs::read_dir(&line)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".ticket"))
            .count();
        if actual == count {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "wanted {count} waiters, got {actual}"
        );
        thread::sleep(Wait::from_millis(2));
    }
}

#[test]
fn endpoint_health_and_save_check_wait_outside_the_probe_timeout_and_keep_ready_evidence() {
    let server = Server::new();
    let journal = tempfile::tempdir().unwrap();
    let config = config(&server.url);
    save_config(journal.path(), &config);
    assert!(matches!(
        refresh(journal.path(), &options(), &ShortProbe, &NoSpp, &|| false),
        OwnerRefreshOutcome::PersistedReady { .. }
    ));
    server.entered();
    let before = std::fs::read(brain_state_path(journal.path())).unwrap();
    thread::scope(|scope| {
        let long =
            scope.spawn(|| dispatch(&request("long", "test.work", 5.0), journal.path(), &config));
        assert_eq!(server.entered(), "long");
        let health =
            scope.spawn(|| refresh(journal.path(), &options(), &ShortProbe, &NoSpp, &|| false));
        let save = scope.spawn(|| {
            dispatch(
                &request(
                    "Reply with the single word OK.",
                    "settings.cloud.validate_key",
                    0.5,
                ),
                journal.path(),
                &config,
            )
        });
        await_tickets(journal.path(), 2);
        thread::sleep(Wait::from_millis(750));
        assert_eq!(
            std::fs::read(brain_state_path(journal.path())).unwrap(),
            before
        );
        assert!(!probe_file_lease_held(&brain_refresh_lease_path(journal.path())).unwrap());
        assert!(server.entered.try_recv().is_err());
        server.release.send(()).unwrap();
        assert!(matches!(
            long.join().unwrap(),
            GenerateResponse::Generated(_)
        ));
        assert!(matches!(
            save.join().unwrap(),
            GenerateResponse::Generated(_)
        ));
        assert!(matches!(
            health.join().unwrap(),
            OwnerRefreshOutcome::PersistedReady { .. }
        ));
    });
    let port = TcpListener::bind("127.0.0.1:0").unwrap();
    let down = self::config(&format!("http://{}", port.local_addr().unwrap()));
    drop(port);
    save_config(journal.path(), &down);
    let outcome = refresh(journal.path(), &options(), &RefusalProbe, &NoSpp, &|| false);
    assert!(
        matches!(&outcome,
        OwnerRefreshOutcome::PersistedNonReady { view, .. }
            if view.reason_code.as_deref() == Some("endpoint_unreachable")),
        "{outcome:?}"
    );
}

#[test]
fn endpoint_renewal_fallback_cancellation_never_opens_a_checking_lease() {
    let journal = tempfile::tempdir().unwrap();
    save_config(journal.path(), &config("http://127.0.0.1:9"));
    let _turn = solstone_core_local::admission::wait_for_model_call(
        journal.path(),
        Default::default(),
        &|| false,
    )
    .unwrap();
    let cancelled = AtomicBool::new(false);
    thread::scope(|scope| {
        let check = scope.spawn(|| {
            renew_prerequisites(journal.path(), None, &ShortProbe, &NoSpp, &|| {
                cancelled.load(Ordering::Acquire)
            })
        });
        await_tickets(journal.path(), 1);
        assert!(!brain_state_path(journal.path()).exists());
        cancelled.store(true, Ordering::Release);
        assert!(matches!(
            check.join().unwrap(),
            OwnerRefreshOutcome::Cancelled
        ));
    });
    assert!(!brain_state_path(journal.path()).exists());
}

#[test]
fn endpoint_active_check_returns_busy_and_real_response_timeout_stays_nonready() {
    let server = Server::new();
    let journal = tempfile::tempdir().unwrap();
    save_config(journal.path(), &config(&server.url));
    server.block_next.store(true, Ordering::Release);
    thread::scope(|scope| {
        let check =
            scope.spawn(|| refresh(journal.path(), &options(), &ShortProbe, &NoSpp, &|| false));
        server.entered();
        assert!(matches!(
            refresh(journal.path(), &options(), &ShortProbe, &NoSpp, &|| false),
            OwnerRefreshOutcome::Busy { .. }
        ));
        assert!(matches!(
            check.join().unwrap(),
            OwnerRefreshOutcome::PersistedNonReady { .. }
        ));
        server.release.send(()).unwrap();
    });
}
