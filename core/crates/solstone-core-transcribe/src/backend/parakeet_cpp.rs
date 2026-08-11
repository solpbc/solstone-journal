// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Supervised parakeet.cpp service client.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use ureq::unversioned::multipart::{Form, Part};

use crate::TranscribeError;

const HOST: &str = "127.0.0.1";
const SERVICE_NAME: &str = "parakeet-cpp";
const PORT_FILE: &str = "parakeet-cpp.port";
const PLACEMENT_FILE: &str = "parakeet-cpp.placement";
const MODEL_FILENAME: &str = "tdt-0.6b-v3-q8_0.gguf";
const COMPUTE_TYPE: &str = "q8_0";
const DEFAULT_DEVICE: &str = "auto";
const HEALTH_TIMEOUT: Duration = Duration::from_secs(1);
const TRANSCRIBE_TIMEOUT: Duration = Duration::from_secs(300);

/// A ready supervised parakeet.cpp service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParakeetServer {
    pub(crate) port: u16,
    pub(crate) base_url: String,
}

/// The health outcome needed by connection setup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HealthState {
    Ready,
    NotReady,
}

/// One normalized word timing from parakeet.cpp's verbose response.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TranscriptionWord {
    pub(crate) word: String,
    pub(crate) start: f64,
    pub(crate) end: f64,
    pub(crate) probability: f64,
}

/// Parsed parakeet.cpp transcription response.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TranscriptionResponse {
    pub(crate) words: Vec<TranscriptionWord>,
    pub(crate) text: String,
}

/// Violations of the shared OpenAI-compatible verbose JSON word contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WordContractError {
    InvalidJson(String),
    NotObject,
    MissingWords,
    TextWithoutTimings,
    WordNotObject,
    MissingKey(&'static str),
    InvalidNumber,
}

/// Metadata retained in transcript headers for this backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelInfo {
    pub(crate) model: String,
    pub(crate) device: String,
    pub(crate) compute_type: String,
}

/// Read the supervisor-published parakeet.cpp port.
pub(crate) fn read_port(journal_path: &Path) -> Option<u16> {
    fs::read_to_string(health_path(journal_path, PORT_FILE))
        .ok()
        .and_then(|value| value.trim().parse().ok())
}

/// Read the validated supervisor-published parakeet.cpp placement.
pub(crate) fn read_placement(journal_path: &Path) -> Option<&'static str> {
    match fs::read_to_string(health_path(journal_path, PLACEMENT_FILE))
        .ok()?
        .trim()
    {
        "cpu" => Some("cpu"),
        "gpu" => Some("gpu"),
        _ => None,
    }
}

/// Probe the health endpoint. Any response other than 200 is not ready.
pub(crate) fn probe_health(base_url: &str, timeout: Duration) -> HealthState {
    let agent = agent(timeout);
    let response = agent.get(&format!("{base_url}/health")).call();
    match response {
        Ok(response) if response.status().as_u16() == 200 => HealthState::Ready,
        Ok(_) | Err(_) => HealthState::NotReady,
    }
}

/// Connect to the service only after the supervisor has published a ready port.
pub(crate) fn connect(journal_path: &Path) -> Result<ParakeetServer, TranscribeError> {
    let Some(port) = read_port(journal_path) else {
        return Err(deferred("no_port", "Parakeet server is not ready yet."));
    };
    let base_url = base_url(port);
    if probe_health(&base_url, HEALTH_TIMEOUT) != HealthState::Ready {
        return Err(deferred(
            "server_not_ready",
            "Parakeet server is not ready yet.",
        ));
    }
    Ok(ParakeetServer { port, base_url })
}

/// Submit a WAV payload to parakeet.cpp's OpenAI-compatible endpoint.
pub(crate) fn transcribe(
    server: &ParakeetServer,
    wav_bytes: &[u8],
) -> Result<TranscriptionResponse, TranscribeError> {
    transcribe_with_timeout(server, wav_bytes, TRANSCRIBE_TIMEOUT)
}

/// Build transcript-header metadata, with supervisor placement overriding config.
pub(crate) fn get_model_info(
    journal_path: &Path,
    configured_device: Option<&str>,
) -> Result<ModelInfo, TranscribeError> {
    require_linux()?;
    let configured_device = validated_device(configured_device)?;
    Ok(ModelInfo {
        model: MODEL_FILENAME.to_owned(),
        device: read_placement(journal_path)
            .unwrap_or(configured_device)
            .to_owned(),
        compute_type: COMPUTE_TYPE.to_owned(),
    })
}

fn transcribe_with_timeout(
    server: &ParakeetServer,
    wav_bytes: &[u8],
    timeout: Duration,
) -> Result<TranscriptionResponse, TranscribeError> {
    require_linux()?;
    let form = Form::new()
        .text("response_format", "verbose_json")
        .text("timestamp_granularities[]", "word")
        .part(
            "file",
            Part::bytes(wav_bytes)
                .file_name("audio.wav")
                .mime_str("audio/wav")
                .map_err(|error| failure("local_protocol_error", error.to_string()))?,
        );
    let response = agent(timeout)
        .post(&format!("{}/v1/audio/transcriptions", server.base_url))
        .send(form)
        .map_err(map_ureq_error)?;
    if response.status().as_u16() != 200 {
        return Err(failure(
            "transcription_http_error",
            format!("HTTP {}", response.status().as_u16()),
        ));
    }
    let mut body = response.into_body();
    let mut text = String::new();
    body.as_reader()
        .read_to_string(&mut text)
        .map_err(map_response_read_error)?;
    parse_transcription_response(&text)
}

fn parse_transcription_response(text: &str) -> Result<TranscriptionResponse, TranscribeError> {
    parse_verbose_json(text).map_err(|error| match error {
        WordContractError::InvalidJson(error) => failure(
            "invalid_json",
            format!("response JSON was invalid: {error}"),
        ),
        WordContractError::NotObject => failure("invalid_json", "response JSON was not an object"),
        WordContractError::MissingWords => {
            failure("contract_violation", "response missing top-level words[]")
        }
        WordContractError::TextWithoutTimings => failure(
            "contract_violation",
            "response has text but no word timings",
        ),
        WordContractError::WordNotObject => {
            failure("contract_violation", "word timing item must be an object")
        }
        WordContractError::MissingKey(key) => failure(
            "contract_violation",
            format!("word timing missing key: {key}"),
        ),
        WordContractError::InvalidNumber => failure(
            "contract_violation",
            "word timing contains invalid numeric value",
        ),
    })
}

/// Parse the OpenAI-compatible verbose JSON word-timing response used by hosted STT.
pub(crate) fn parse_verbose_json(text: &str) -> Result<TranscriptionResponse, WordContractError> {
    let payload: Value = serde_json::from_str(text)
        .map_err(|error| WordContractError::InvalidJson(error.to_string()))?;
    let object = payload.as_object().ok_or(WordContractError::NotObject)?;
    let raw_words = object
        .get("words")
        .and_then(Value::as_array)
        .ok_or(WordContractError::MissingWords)?;
    let text = match object.get("text") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(value)) => value.trim().to_owned(),
        Some(value) => value.to_string().trim_matches('"').trim().to_owned(),
    };
    if raw_words.is_empty() {
        if text.is_empty() {
            return Ok(TranscriptionResponse {
                words: Vec::new(),
                text,
            });
        }
        return Err(WordContractError::TextWithoutTimings);
    }

    let words = raw_words
        .iter()
        .map(parse_word)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(TranscriptionResponse { words, text })
}

fn parse_word(value: &Value) -> Result<TranscriptionWord, WordContractError> {
    let object = value.as_object().ok_or(WordContractError::WordNotObject)?;
    let token = object
        .get("word")
        .ok_or(WordContractError::MissingKey("word"))?
        .to_string()
        .trim_matches('"')
        .trim()
        .to_owned();
    let start = finite_word_number(object, "start")?;
    let end = finite_word_number(object, "end")?;
    let probability = object
        .get("conf")
        .map(|value| {
            value
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or(WordContractError::InvalidNumber)
        })
        .transpose()?
        .unwrap_or(1.0);
    Ok(TranscriptionWord {
        word: format!(" {token}"),
        start,
        end,
        probability,
    })
}

fn finite_word_number(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<f64, WordContractError> {
    object
        .get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .ok_or(WordContractError::InvalidNumber)
}

fn require_linux() -> Result<(), TranscribeError> {
    (std::env::consts::OS == "linux")
        .then_some(())
        .ok_or_else(|| {
            failure(
                "unsupported_platform",
                "parakeet-cpp is only supported on Linux",
            )
        })
}

fn validated_device(configured_device: Option<&str>) -> Result<&str, TranscribeError> {
    let device = configured_device.unwrap_or(DEFAULT_DEVICE);
    matches!(device, "auto" | "cpu")
        .then_some(device)
        .ok_or_else(|| failure("invalid_config", "device must be one of: auto, cpu"))
}

fn health_path(journal_path: &Path, name: &str) -> PathBuf {
    journal_path.join("health").join(name)
}

fn base_url(port: u16) -> String {
    format!("http://{HOST}:{port}")
}

fn agent(timeout: Duration) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(timeout))
        .timeout_recv_response(Some(timeout))
        .timeout_recv_body(Some(timeout))
        .timeout_global(Some(timeout))
        .build();
    ureq::Agent::new_with_config(config)
}

fn map_ureq_error(error: ureq::Error) -> TranscribeError {
    match error {
        ureq::Error::Timeout(_) => deferred("read_timeout", "transcription request timed out"),
        ureq::Error::Protocol(error) => deferred("server_disconnected", error.to_string()),
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed => {
            deferred("connect_error", error.to_string())
        }
        ureq::Error::Io(error) => map_io_error(error),
        ureq::Error::Http(error) => failure("local_protocol_error", error.to_string()),
        ureq::Error::BadUri(error) => failure("local_protocol_error", error),
        ureq::Error::RequireHttpsOnly(error) => failure("unsupported_protocol", error.to_string()),
        ureq::Error::TlsRequired => failure("unsupported_protocol", "TLS is required"),
        ureq::Error::StatusCode(status) => {
            failure("transcription_http_error", format!("HTTP {status}"))
        }
        error => deferred("transport_error", error.to_string()),
    }
}

fn map_response_read_error(error: io::Error) -> TranscribeError {
    map_io_error(error)
}

fn map_io_error(error: io::Error) -> TranscribeError {
    use io::ErrorKind;

    match error.kind() {
        ErrorKind::TimedOut => deferred("read_timeout", error.to_string()),
        ErrorKind::ConnectionRefused => deferred("connect_error", error.to_string()),
        ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset | ErrorKind::BrokenPipe => {
            deferred("server_disconnected", error.to_string())
        }
        _ => deferred("network_error", error.to_string()),
    }
}

fn deferred(reason: impl Into<String>, detail: impl Into<String>) -> TranscribeError {
    TranscribeError::ParakeetCppDeferred {
        reason: reason.into(),
        detail: detail.into(),
    }
}

fn failure(reason: impl Into<String>, detail: impl Into<String>) -> TranscribeError {
    TranscribeError::ParakeetCppFailure {
        reason: reason.into(),
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use serde_json::json;

    use super::{
        COMPUTE_TYPE, HealthState, MODEL_FILENAME, ParakeetServer, WordContractError, connect,
        get_model_info, map_ureq_error, parse_transcription_response, parse_verbose_json,
        probe_health, read_placement, read_port, transcribe_with_timeout,
    };
    use crate::TranscribeError;
    use crate::config::{parakeet_cpp_device, read_transcribe_config};

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
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let recorded = Arc::clone(&requests);
            let handle = thread::spawn(move || {
                for reply in replies {
                    let (stream, _) = listener.accept().unwrap();
                    let request = read_request(stream.try_clone().unwrap());
                    recorded.lock().unwrap().push(request);
                    match reply {
                        Reply::Http { status, body } => {
                            let mut stream = stream;
                            let status_text = if status == 200 { "OK" } else { "Error" };
                            let response = format!(
                                "HTTP/1.1 {status} {status_text}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            );
                            let _ = stream.write_all(response.as_bytes());
                        }
                        Reply::Close => drop(stream),
                        Reply::Sleep(duration) => thread::sleep(duration),
                    }
                }
            });
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
        write_port(temporary.path(), stub_port(&stub.base_url));

        let server = connect(temporary.path()).unwrap();
        let response = transcribe_with_timeout(&server, b"wav", Duration::from_secs(1)).unwrap();
        assert!(response.words.is_empty());
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
    fn missing_port_defers_with_no_port() {
        let temporary = tempfile::tempdir().unwrap();

        let error = connect(temporary.path()).unwrap_err();

        assert_deferred_reason(error, "no_port");
    }

    #[test]
    fn non_200_health_defers_with_server_not_ready() {
        let temporary = tempfile::tempdir().unwrap();
        let stub = StubServer::start(vec![Reply::Http {
            status: 503,
            body: "loading",
        }]);
        write_port(temporary.path(), stub_port(&stub.base_url));

        let error = connect(temporary.path()).unwrap_err();
        stub.finish();

        assert_deferred_reason(error, "server_not_ready");
    }

    #[test]
    fn refused_health_defers_with_server_not_ready() {
        let temporary = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        write_port(temporary.path(), port);

        let error = connect(temporary.path()).unwrap_err();

        assert_deferred_reason(error, "server_not_ready");
    }

    #[test]
    fn disconnected_transcription_defers_with_server_disconnected() {
        let stub = StubServer::start(vec![Reply::Close]);
        let server = server_for(&stub);

        let error = transcribe_with_timeout(&server, b"wav", Duration::from_secs(1)).unwrap_err();
        stub.finish();

        assert_deferred_reason(error, "server_disconnected");
    }

    #[test]
    fn timed_out_transcription_defers_with_read_timeout() {
        let stub = StubServer::start(vec![Reply::Sleep(Duration::from_millis(150))]);
        let server = server_for(&stub);

        let error =
            transcribe_with_timeout(&server, b"wav", Duration::from_millis(25)).unwrap_err();
        stub.finish();

        assert_deferred_reason(error, "read_timeout");
    }

    #[test]
    fn refused_transcription_defers_with_connect_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let server = ParakeetServer {
            port,
            base_url: format!("http://127.0.0.1:{port}"),
        };

        let error =
            transcribe_with_timeout(&server, b"wav", Duration::from_millis(50)).unwrap_err();

        assert_deferred_reason(error, "connect_error");
    }

    #[test]
    fn catch_all_ureq_error_defers_with_transport_error() {
        let error = map_ureq_error(ureq::Error::RedirectFailed);

        assert_deferred_reason(error, "transport_error");
    }

    #[test]
    fn live_http_500_is_a_hard_transcription_error() {
        let stub = StubServer::start(vec![Reply::Http {
            status: 500,
            body: "broken",
        }]);
        let server = server_for(&stub);

        let error = transcribe_with_timeout(&server, b"wav", Duration::from_secs(1)).unwrap_err();
        stub.finish();

        assert_failure_reason(error, "transcription_http_error");
    }

    #[test]
    fn malformed_live_json_is_a_hard_error() {
        let stub = StubServer::start(vec![Reply::Http {
            status: 200,
            body: "not-json",
        }]);
        let server = server_for(&stub);

        let error = transcribe_with_timeout(&server, b"wav", Duration::from_secs(1)).unwrap_err();
        stub.finish();

        assert_failure_reason(error, "invalid_json");
    }

    #[test]
    fn text_without_word_timings_is_a_hard_contract_error() {
        let stub = StubServer::start(vec![Reply::Http {
            status: 200,
            body: r#"{"words":[],"text":"hello"}"#,
        }]);
        let server = server_for(&stub);

        let error = transcribe_with_timeout(&server, b"wav", Duration::from_secs(1)).unwrap_err();
        stub.finish();

        assert_failure_reason(error, "contract_violation");
    }

    #[test]
    fn placement_overrides_configured_device() {
        let temporary = tempfile::tempdir().unwrap();
        write_placement(temporary.path(), "gpu");

        let info = get_model_info(temporary.path(), Some("cpu")).unwrap();

        assert_eq!(info.model, MODEL_FILENAME);
        assert_eq!(info.device, "gpu");
        assert_eq!(info.compute_type, COMPUTE_TYPE);
    }

    #[test]
    fn configured_device_is_used_without_placement() {
        let temporary = tempfile::tempdir().unwrap();

        let info = get_model_info(temporary.path(), Some("cpu")).unwrap();

        assert_eq!(info.device, "cpu");
        assert_eq!(read_placement(temporary.path()), None);
    }

    #[test]
    fn empty_words_and_empty_text_are_a_valid_empty_result() {
        let response = parse_transcription_response(r#"{"words":[],"text":""}"#).unwrap();

        assert!(response.words.is_empty());
        assert_eq!(response.text, "");
    }

    #[test]
    fn shared_verbose_parser_marks_each_contract_failure_and_cpp_maps_it_hard() {
        let cases = [
            ("{", "invalid_json"),
            ("[]", "invalid_json"),
            ("{}", "contract_violation"),
            (r#"{"words":[],"text":"hello"}"#, "contract_violation"),
            (r#"{"words":[1]}"#, "contract_violation"),
            (r#"{"words":[{}]}"#, "contract_violation"),
            (
                r#"{"words":[{"word":"hello","start":"bad","end":1.0}]}"#,
                "contract_violation",
            ),
        ];

        for (index, (payload, expected_reason)) in cases.into_iter().enumerate() {
            let marker = parse_verbose_json(payload).unwrap_err();
            match (index, marker) {
                (0, WordContractError::InvalidJson(_))
                | (1, WordContractError::NotObject)
                | (2, WordContractError::MissingWords)
                | (3, WordContractError::TextWithoutTimings)
                | (4, WordContractError::WordNotObject)
                | (5, WordContractError::MissingKey("word"))
                | (6, WordContractError::InvalidNumber) => {}
                (_, marker) => panic!("unexpected contract marker: {marker:?}"),
            }
            assert_failure_reason(
                parse_transcription_response(payload).unwrap_err(),
                expected_reason,
            );
        }
    }

    #[test]
    fn parakeet_coreml_timeout_config_does_not_set_cpp_device() {
        let temporary = tempfile::tempdir().unwrap();
        let config_directory = temporary.path().join("config");
        fs::create_dir_all(&config_directory).unwrap();
        fs::write(
            config_directory.join("journal.json"),
            json!({"transcribe":{"parakeet":{"timeout_sec":10}}}).to_string(),
        )
        .unwrap();
        let config = read_transcribe_config(temporary.path()).unwrap();

        assert_eq!(parakeet_cpp_device(&config), None);
    }

    #[test]
    fn health_probe_accepts_only_http_200() {
        let stub = StubServer::start(vec![Reply::Http {
            status: 204,
            body: "",
        }]);

        assert_eq!(
            probe_health(&stub.base_url, Duration::from_secs(1)),
            HealthState::NotReady
        );
        stub.finish();
    }

    fn read_request(mut stream: TcpStream) -> CapturedRequest {
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

    fn write_port(journal_path: &std::path::Path, port: u16) {
        let health = journal_path.join("health");
        fs::create_dir_all(&health).unwrap();
        fs::write(health.join("parakeet-cpp.port"), port.to_string()).unwrap();
    }

    fn write_placement(journal_path: &std::path::Path, device: &str) {
        let health = journal_path.join("health");
        fs::create_dir_all(&health).unwrap();
        fs::write(health.join("parakeet-cpp.placement"), device).unwrap();
    }

    fn stub_port(base_url: &str) -> u16 {
        base_url.rsplit(':').next().unwrap().parse().unwrap()
    }

    fn server_for(stub: &StubServer) -> ParakeetServer {
        ParakeetServer {
            port: stub_port(&stub.base_url),
            base_url: stub.base_url.clone(),
        }
    }

    fn assert_deferred_reason(error: TranscribeError, expected_reason: &str) {
        assert_eq!(error.exit_code(), 69);
        let TranscribeError::ParakeetCppDeferred { reason, .. } = error else {
            panic!("expected deferred parakeet.cpp error");
        };
        assert_eq!(reason, expected_reason);
    }

    fn assert_failure_reason(error: TranscribeError, expected_reason: &str) {
        assert_eq!(error.exit_code(), 1);
        let TranscribeError::ParakeetCppFailure { reason, .. } = error else {
            panic!("expected hard parakeet.cpp error");
        };
        assert_eq!(reason, expected_reason);
    }

    #[test]
    fn port_file_is_trimmed_and_invalid_placement_is_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        write_port(temporary.path(), 4321);
        fs::write(
            temporary.path().join("health/parakeet-cpp.placement"),
            "invalid\n",
        )
        .unwrap();

        assert_eq!(read_port(temporary.path()), Some(4321));
        assert_eq!(read_placement(temporary.path()), None);
    }
}
