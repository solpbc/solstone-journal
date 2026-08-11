// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Confidential generation over one freshly attested RA-TLS channel.

use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use serde_json::{Map, Value};
use solstone_core_generate::GenerateRequest;
use solstone_core_local::{ByoEndpoint, HttpResponse};
use solstone_core_spp_ratls::{
    AttestationSession, AttestedHttpError, AttestedIo, CompositeVerdict, NvattestEnsureStatus,
    RatlsEndpoint, check_nvattest_readiness, classify_channel_failure,
    classify_nvattest_prerequisite, establish_production_attested_channel, send_json_request,
};

use crate::endpoint::{
    EndpointConverseCall, EndpointConverseResult, EndpointFailure, EndpointGenerated,
    EndpointResult, EndpointRuntime, EndpointTransport, EndpointTransportError, converse_failure,
    endpoint_converse_with, endpoint_generate_with,
};
use crate::{ConverseMessage, ConverseToolSpec};

const ATTESTED_CHANNEL_TIMEOUT: Duration = Duration::from_secs(120);

pub enum ConfidentialResult {
    Generated(EndpointGenerated),
    Failed(EndpointFailure),
    AttestationNotVerified,
    AttestationFailed,
    AttestationStale,
}

/// Performs one confidential generation attempt using a newly attested channel.
pub fn confidential_generate(
    request: &GenerateRequest,
    journal_path: &Path,
    endpoint: &ByoEndpoint,
    config: &Map<String, Value>,
    runtime: &EndpointRuntime,
) -> ConfidentialResult {
    confidential_generate_with(
        ConfidentialCall {
            request,
            journal_path,
            endpoint,
            config,
            runtime,
            now: SystemTime::now(),
        },
        check_nvattest_readiness,
        |ratls_endpoint, nvattest_dir| {
            establish_production_attested_channel(
                ratls_endpoint,
                nvattest_dir,
                ATTESTED_CHANNEL_TIMEOUT,
            )
            .map(|channel| EstablishedChannel {
                verdict: channel.verified.verdict.clone(),
                stream: Box::new(channel),
            })
            .map_err(|error| error.reason_code)
        },
    )
}

/// Performs one confidential tool-conversation turn using a newly attested channel.
pub fn confidential_converse(
    request: &GenerateRequest,
    messages: &[ConverseMessage],
    tools: &[ConverseToolSpec],
    journal_path: &Path,
    endpoint: &ByoEndpoint,
    config: &Map<String, Value>,
    runtime: &EndpointRuntime,
) -> EndpointConverseResult {
    confidential_converse_with(
        ConfidentialConverseCall {
            request,
            messages,
            tools,
            journal_path,
            endpoint,
            config,
            runtime,
            now: SystemTime::now(),
        },
        check_nvattest_readiness,
        |ratls_endpoint, nvattest_dir| {
            establish_production_attested_channel(
                ratls_endpoint,
                nvattest_dir,
                ATTESTED_CHANNEL_TIMEOUT,
            )
            .map(|channel| EstablishedChannel {
                verdict: channel.verified.verdict.clone(),
                stream: Box::new(channel),
            })
            .map_err(|error| error.reason_code)
        },
    )
}

struct EstablishedChannel {
    verdict: CompositeVerdict,
    stream: Box<dyn AttestedIo>,
}

/// The call's context, grouped so the injected seams stay visible in the signature.
struct ConfidentialCall<'a> {
    request: &'a GenerateRequest,
    journal_path: &'a Path,
    endpoint: &'a ByoEndpoint,
    config: &'a Map<String, Value>,
    runtime: &'a EndpointRuntime,
    now: SystemTime,
}

/// The converse call's context, grouped so the injected seams stay visible in the signature.
struct ConfidentialConverseCall<'a> {
    request: &'a GenerateRequest,
    messages: &'a [ConverseMessage],
    tools: &'a [ConverseToolSpec],
    journal_path: &'a Path,
    endpoint: &'a ByoEndpoint,
    config: &'a Map<String, Value>,
    runtime: &'a EndpointRuntime,
    now: SystemTime,
}

fn confidential_generate_with<R, E>(
    call: ConfidentialCall<'_>,
    readiness: R,
    establish: E,
) -> ConfidentialResult
where
    R: FnOnce(&Path) -> NvattestEnsureStatus,
    E: FnOnce(&RatlsEndpoint, &Path) -> Result<EstablishedChannel, &'static str>,
{
    let ConfidentialCall {
        request,
        journal_path,
        endpoint,
        config,
        runtime,
        now,
    } = call;
    if runtime
        .attestation_state()
        .get_attestation_state()
        .session
        .is_some_and(|session| session.status(now) == "stale")
    {
        return ConfidentialResult::AttestationStale;
    }

    let nvattest_dir = resolve_nvattest_dir(config, journal_path);
    if let Some(failure) = classify_nvattest_prerequisite(readiness(&nvattest_dir)) {
        runtime
            .attestation_state()
            .record_attestation_failed(failure.kind, failure.reason_code);
        return ConfidentialResult::AttestationNotVerified;
    }

    let target = match ratls_target(&endpoint.base_url) {
        Some(target) => target,
        None => {
            runtime.attestation_state().record_attestation_failed(
                classify_channel_failure("tls_handshake_failed"),
                "tls_handshake_failed",
            );
            return ConfidentialResult::AttestationFailed;
        }
    };
    let EstablishedChannel { verdict, stream } = match establish(&target.endpoint, &nvattest_dir) {
        Ok(channel) => channel,
        Err(reason_code) => {
            runtime
                .attestation_state()
                .record_attestation_failed(classify_channel_failure(reason_code), reason_code);
            return ConfidentialResult::AttestationFailed;
        }
    };
    runtime
        .attestation_state()
        .record_attestation_verified(AttestationSession {
            verdict,
            started_at: now,
            tpm_heartbeat_at: now,
            gpu_reattest_at: now,
        });

    let mut transport = AttestedEndpointTransport {
        stream,
        host: target.host,
    };
    match endpoint_generate_with(
        request,
        journal_path,
        endpoint,
        config,
        runtime,
        &mut transport,
        Instant::now(),
    ) {
        EndpointResult::Generated(generated) => ConfidentialResult::Generated(generated),
        EndpointResult::Failed(failure) => ConfidentialResult::Failed(failure),
    }
}

fn confidential_converse_with<R, E>(
    call: ConfidentialConverseCall<'_>,
    readiness: R,
    establish: E,
) -> EndpointConverseResult
where
    R: FnOnce(&Path) -> NvattestEnsureStatus,
    E: FnOnce(&RatlsEndpoint, &Path) -> Result<EstablishedChannel, &'static str>,
{
    let ConfidentialConverseCall {
        request,
        messages,
        tools,
        journal_path,
        endpoint,
        config,
        runtime,
        now,
    } = call;
    if runtime
        .attestation_state()
        .get_attestation_state()
        .session
        .is_some_and(|session| session.status(now) == "stale")
    {
        return converse_failure("attestation_stale");
    }

    let nvattest_dir = resolve_nvattest_dir(config, journal_path);
    if let Some(failure) = classify_nvattest_prerequisite(readiness(&nvattest_dir)) {
        runtime
            .attestation_state()
            .record_attestation_failed(failure.kind, failure.reason_code);
        return converse_failure("attestation_not_yet_verified");
    }

    let target = match ratls_target(&endpoint.base_url) {
        Some(target) => target,
        None => {
            runtime.attestation_state().record_attestation_failed(
                classify_channel_failure("tls_handshake_failed"),
                "tls_handshake_failed",
            );
            return converse_failure("attestation_failed");
        }
    };
    let EstablishedChannel { verdict, stream } = match establish(&target.endpoint, &nvattest_dir) {
        Ok(channel) => channel,
        Err(reason_code) => {
            runtime
                .attestation_state()
                .record_attestation_failed(classify_channel_failure(reason_code), reason_code);
            return converse_failure("attestation_failed");
        }
    };
    runtime
        .attestation_state()
        .record_attestation_verified(AttestationSession {
            verdict,
            started_at: now,
            tpm_heartbeat_at: now,
            gpu_reattest_at: now,
        });

    let mut transport = AttestedEndpointTransport {
        stream,
        host: target.host,
    };
    endpoint_converse_with(
        EndpointConverseCall {
            request,
            messages,
            tools,
            journal_path,
            endpoint,
            config,
            runtime,
        },
        &mut transport,
        Instant::now(),
    )
}

fn resolve_nvattest_dir(config: &Map<String, Value>, journal_path: &Path) -> PathBuf {
    config
        .get("services")
        .and_then(Value::as_object)
        .and_then(|services| services.get("confidential"))
        .and_then(Value::as_object)
        .and_then(|confidential| confidential.get("nvattest_dir"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("SPP_NVATTEST_DIR").map(PathBuf::from))
        .unwrap_or_else(|| journal_path.join("cache/providers/nvattest"))
}

struct RatlsTarget {
    endpoint: RatlsEndpoint,
    host: String,
}

fn ratls_target(base_url: &str) -> Option<RatlsTarget> {
    let authority = base_url
        .strip_prefix("https://")
        .or_else(|| base_url.strip_prefix("http://"))?
        .split('/')
        .next()?;
    if authority.is_empty() {
        return None;
    }
    let (host, port) = authority
        .rsplit_once(':')
        .and_then(|(host, port)| port.parse::<u16>().ok().map(|port| (host, port)))
        .unwrap_or((authority, 443));
    if host.is_empty() {
        return None;
    }
    Some(RatlsTarget {
        endpoint: RatlsEndpoint::new(host, port),
        host: authority.to_owned(),
    })
}

struct AttestedEndpointTransport {
    stream: Box<dyn AttestedIo>,
    host: String,
}

impl EndpointTransport for AttestedEndpointTransport {
    fn get(
        &mut self,
        _base_url: &str,
        _path: &str,
        _credential: Option<&str>,
        _timeout: Duration,
    ) -> Result<HttpResponse, EndpointTransportError> {
        // Model discovery is optional in endpoint_generate_with/endpoint_converse_with;
        // it must not issue an unaudited second request over the one-shot channel.
        Err(EndpointTransportError::Other)
    }

    fn post_json(
        &mut self,
        _base_url: &str,
        path: &str,
        body: &Value,
        credential: Option<&str>,
        timeout: Duration,
    ) -> Result<HttpResponse, EndpointTransportError> {
        let body = serde_json::to_vec(body).map_err(|_| EndpointTransportError::Other)?;
        self.stream
            .set_io_timeout(Some(timeout))
            .map_err(|_| EndpointTransportError::Other)?;
        send_json_request(&mut *self.stream, &self.host, path, credential, &body)
            .map(|response| HttpResponse {
                status: response.status,
                body: String::from_utf8_lossy(&response.body).into_owned(),
            })
            .map_err(|error| match error {
                AttestedHttpError::Transport(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    ) =>
                {
                    EndpointTransportError::Capacity
                }
                AttestedHttpError::Transport(_) => EndpointTransportError::Connection,
                AttestedHttpError::Protocol(_) => EndpointTransportError::Other,
            })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{self, Read, Write},
        net::{TcpListener, TcpStream},
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::{Duration, UNIX_EPOCH},
    };

    use serde_json::json;
    use solstone_core_generate::ContentPart;
    use solstone_core_spp_attest::{
        nvgpu::claims::GpuAppraisal,
        snp::{CpuAppraisal, CpuTcb, TcbVersion},
    };
    use solstone_core_spp_ratls::AttestationSession;

    use super::*;

    fn request() -> GenerateRequest {
        GenerateRequest {
            id: None,
            context: "test.confidential".into(),
            contents: vec![ContentPart::Text {
                text: "Hello".into(),
            }],
            system_instruction: None,
            temperature: 0.2,
            max_output_tokens: 64,
            thinking_budget: None,
            timeout_s: None,
            json_output: false,
            json_schema: None,
            enforce_responsiveness: false,
            attempt_index: 0,
            exclusive_admission: false,
            transport_retries: None,
        }
    }

    fn endpoint(port: u16) -> ByoEndpoint {
        ByoEndpoint {
            base_url: format!("http://127.0.0.1:{port}"),
            served_model_id: "served".into(),
            credential: Some("token".into()),
            parallel_slots: None,
            is_confidential: true,
            is_bundled: false,
        }
    }

    fn verdict() -> CompositeVerdict {
        let tcb = TcbVersion {
            boot_loader: None,
            tee: None,
            snp: None,
            microcode: None,
            fmc: None,
        };
        CompositeVerdict {
            verified: true,
            legs: ["cpu", "gpu"],
            substrate: "test".into(),
            checked_at: UNIX_EPOCH,
            cpu: CpuAppraisal {
                steps: Vec::new(),
                hcla_version: 0,
                report_version: 0,
                cpuid_family: None,
                cpuid_model: None,
                cpuid_step: None,
                tcb: CpuTcb {
                    current: tcb.clone(),
                    reported: tcb.clone(),
                    committed: tcb.clone(),
                    launch: tcb,
                },
                pcr_sha256: String::new(),
                host_data_hex: String::new(),
                measurement_hex: String::new(),
                chip_id_hex: String::new(),
            },
            gpu: GpuAppraisal {
                steps: Vec::new(),
                driver_version: String::new(),
                vbios_version: String::new(),
                hwmodel: "H100".into(),
                ueid: String::new(),
                oemid: String::new(),
                eat_nonce: String::new(),
                claims_version: String::new(),
                arch: String::new(),
                envelope_gpu_uuid: String::new(),
            },
        }
    }

    fn journal(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "solstone-confidential-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create journal");
        path
    }

    fn assert_no_endpoint_request(listener: &TcpListener) {
        listener.set_nonblocking(true).expect("set nonblocking");
        assert!(matches!(
            listener.accept(),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock
        ));
    }

    fn converse_messages() -> Vec<ConverseMessage> {
        vec![ConverseMessage::User { text: "ask".into() }]
    }

    fn converse_tools() -> Vec<ConverseToolSpec> {
        vec![ConverseToolSpec {
            name: "weather".into(),
            description: "weather".into(),
            parameters: json!({"type": "object"}),
        }]
    }

    fn served_window_config() -> Map<String, Value> {
        json!({"providers": {"local": {"served_context_window": 2048}}})
            .as_object()
            .expect("config object")
            .clone()
    }

    fn converse_response_body() -> String {
        json!({
            "choices": [{
                "message": {
                    "content": "before",
                    "tool_calls": [{
                        "id": "call-1",
                        "type": "function",
                        "function": {"name": "weather", "arguments": "{\"city\":\"Denver\"}"},
                    }],
                },
                "finish_reason": "stop",
            }],
            "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5},
        })
        .to_string()
    }

    fn read_request_body(stream: &mut TcpStream) -> Value {
        let mut bytes = Vec::new();
        let mut content_length = None;
        loop {
            let mut buffer = [0_u8; 4096];
            let read = stream.read(&mut buffer).expect("read");
            bytes.extend_from_slice(&buffer[..read]);
            if content_length.is_none()
                && let Some(head_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n")
            {
                let head = std::str::from_utf8(&bytes[..head_end]).expect("headers UTF-8");
                content_length = Some(
                    head.lines()
                        .find_map(|line| line.strip_prefix("Content-Length: "))
                        .expect("content length")
                        .parse::<usize>()
                        .expect("numeric content length"),
                );
            }
            if let Some(length) = content_length
                && let Some(head_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n")
                && bytes.len() >= head_end + 4 + length
            {
                let body = &bytes[head_end + 4..head_end + 4 + length];
                return serde_json::from_slice(body).expect("JSON body");
            }
        }
    }

    fn write_json_response(stream: &mut TcpStream, response_body: &str) {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{response_body}",
            response_body.len()
        );
        stream
            .write_all(response.as_bytes())
            .expect("write response");
    }

    struct ResponseTransport {
        response: HttpResponse,
    }

    impl EndpointTransport for ResponseTransport {
        fn get(
            &mut self,
            _base_url: &str,
            _path: &str,
            _credential: Option<&str>,
            _timeout: Duration,
        ) -> Result<HttpResponse, EndpointTransportError> {
            Err(EndpointTransportError::Other)
        }

        fn post_json(
            &mut self,
            _base_url: &str,
            _path: &str,
            _body: &Value,
            _credential: Option<&str>,
            _timeout: Duration,
        ) -> Result<HttpResponse, EndpointTransportError> {
            Ok(self.response.clone())
        }
    }

    struct NoRequestStream {
        writes: Arc<AtomicUsize>,
    }

    impl Read for NoRequestStream {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::WouldBlock, "no request"))
        }
    }

    impl Write for NoRequestStream {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl AttestedIo for NoRequestStream {
        fn set_io_timeout(&mut self, _timeout: Option<Duration>) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn fresh_attestation_uses_one_channel_request_with_confidential_qwen_controls() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("address").port();
        let requests = Arc::new(AtomicUsize::new(0));
        let recorded = Arc::new(std::sync::Mutex::new(None));
        let recorded_for_server = recorded.clone();
        let requests_for_server = requests.clone();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut bytes = Vec::new();
            let mut content_length = None;
            loop {
                let mut buffer = [0_u8; 4096];
                let read = stream.read(&mut buffer).expect("read");
                bytes.extend_from_slice(&buffer[..read]);
                if content_length.is_none()
                    && let Some(head_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n")
                {
                    let head = std::str::from_utf8(&bytes[..head_end]).expect("headers UTF-8");
                    content_length = Some(
                        head.lines()
                            .find_map(|line| line.strip_prefix("Content-Length: "))
                            .expect("content length")
                            .parse::<usize>()
                            .expect("numeric content length"),
                    );
                }
                if let Some(length) = content_length
                    && let Some(head_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n")
                    && bytes.len() >= head_end + 4 + length
                {
                    break;
                }
            }
            requests_for_server.fetch_add(1, Ordering::SeqCst);
            let text = String::from_utf8(bytes).expect("request UTF-8");
            let body = text.split("\r\n\r\n").nth(1).expect("body");
            *recorded_for_server.lock().expect("record") =
                Some(serde_json::from_str::<Value>(body).expect("JSON body"));
            let response_body = r#"{"choices":[{"message":{"content":"OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{response_body}",
                response_body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });
        let runtime = EndpointRuntime::default();
        let path = journal("success");
        let result = confidential_generate_with(
            ConfidentialCall {
                request: &request(),
                journal_path: &path,
                endpoint: &endpoint(port),
                config: &Map::new(),
                runtime: &runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _| {
                Ok(EstablishedChannel {
                    verdict: verdict(),
                    stream: Box::new(TcpStream::connect(("127.0.0.1", port)).expect("connect")),
                })
            },
        );
        assert!(matches!(result, ConfidentialResult::Generated(_)));
        server.join().expect("join");
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        let body = recorded.lock().expect("record").take().expect("one body");
        for field in [
            "chat_template_kwargs",
            "top_p",
            "top_k",
            "min_p",
            "presence_penalty",
        ] {
            assert!(body.get(field).is_some(), "missing {field}");
        }
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn endpoint_and_confidential_converse_parse_the_same_tool_turn() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("address").port();
        let response_body = converse_response_body();
        let response_for_server = response_body.clone();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let _ = read_request_body(&mut stream);
            write_json_response(&mut stream, &response_for_server);
        });
        let path = journal("converse-equivalence");
        let request = request();
        let messages = converse_messages();
        let tools = converse_tools();
        let config = served_window_config();
        let endpoint = endpoint(port);
        let endpoint_runtime = EndpointRuntime::default();
        let mut endpoint_transport = ResponseTransport {
            response: HttpResponse {
                status: 200,
                body: response_body,
            },
        };
        let endpoint_turn = endpoint_converse_with(
            EndpointConverseCall {
                request: &request,
                messages: &messages,
                tools: &tools,
                journal_path: &path,
                endpoint: &endpoint,
                config: &config,
                runtime: &endpoint_runtime,
            },
            &mut endpoint_transport,
            Instant::now(),
        )
        .expect("endpoint converse turn");

        let confidential_runtime = EndpointRuntime::default();
        let confidential_turn = confidential_converse_with(
            ConfidentialConverseCall {
                request: &request,
                messages: &messages,
                tools: &tools,
                journal_path: &path,
                endpoint: &endpoint,
                config: &config,
                runtime: &confidential_runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _| {
                Ok(EstablishedChannel {
                    verdict: verdict(),
                    stream: Box::new(TcpStream::connect(("127.0.0.1", port)).expect("connect")),
                })
            },
        )
        .expect("confidential converse turn");
        assert_eq!(endpoint_turn, confidential_turn);
        server.join().expect("join");
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn fresh_attestation_uses_one_channel_converse_request_with_qwen_controls() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("address").port();
        let requests = Arc::new(AtomicUsize::new(0));
        let recorded = Arc::new(std::sync::Mutex::new(None));
        let requests_for_server = requests.clone();
        let recorded_for_server = recorded.clone();
        let response_body = converse_response_body();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let body = read_request_body(&mut stream);
            requests_for_server.fetch_add(1, Ordering::SeqCst);
            *recorded_for_server.lock().expect("record") = Some(body);
            write_json_response(&mut stream, &response_body);
        });
        let runtime = EndpointRuntime::default();
        let path = journal("converse-success");
        let messages = converse_messages();
        let tools = converse_tools();
        let result = confidential_converse_with(
            ConfidentialConverseCall {
                request: &request(),
                messages: &messages,
                tools: &tools,
                journal_path: &path,
                endpoint: &endpoint(port),
                config: &served_window_config(),
                runtime: &runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _| {
                Ok(EstablishedChannel {
                    verdict: verdict(),
                    stream: Box::new(TcpStream::connect(("127.0.0.1", port)).expect("connect")),
                })
            },
        );
        assert!(result.is_ok());
        server.join().expect("join");
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        let body = recorded.lock().expect("record").take().expect("one body");
        assert!(body.get("tools").is_some());
        for field in [
            "chat_template_kwargs",
            "top_p",
            "top_k",
            "min_p",
            "presence_penalty",
        ] {
            assert!(body.get(field).is_some(), "missing {field}");
        }
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn attested_transport_applies_the_endpoint_request_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("address").port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            thread::sleep(Duration::from_millis(150));
        });
        let mut transport = AttestedEndpointTransport {
            stream: Box::new(TcpStream::connect(("127.0.0.1", port)).expect("connect")),
            host: format!("127.0.0.1:{port}"),
        };

        assert!(matches!(
            transport.post_json(
                "",
                "/v1/chat/completions",
                &json!({}),
                None,
                Duration::from_millis(50),
            ),
            Err(EndpointTransportError::Capacity)
        ));
        server.join().expect("join");
    }

    #[test]
    fn readiness_failure_refuses_without_attempting_a_channel() {
        let runtime = EndpointRuntime::default();
        let attempts = AtomicUsize::new(0);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind endpoint");
        let path = journal("not-verified");
        let result = confidential_generate_with(
            ConfidentialCall {
                request: &request(),
                journal_path: &path,
                endpoint: &endpoint(listener.local_addr().expect("address").port()),
                config: &Map::new(),
                runtime: &runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::InstallFailed,
            |_, _| {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err("tls_handshake_failed")
            },
        );
        assert!(matches!(result, ConfidentialResult::AttestationNotVerified));
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
        assert_no_endpoint_request(&listener);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn channel_failure_refuses_without_an_endpoint_request() {
        let runtime = EndpointRuntime::default();
        let attempts = AtomicUsize::new(0);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind endpoint");
        let path = journal("failed");
        let result = confidential_generate_with(
            ConfidentialCall {
                request: &request(),
                journal_path: &path,
                endpoint: &endpoint(listener.local_addr().expect("address").port()),
                config: &Map::new(),
                runtime: &runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _| {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err("tls_handshake_failed")
            },
        );
        assert!(matches!(result, ConfidentialResult::AttestationFailed));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_no_endpoint_request(&listener);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn stale_session_refuses_before_readiness_or_channel_establishment() {
        let runtime = EndpointRuntime::default();
        runtime
            .attestation_state()
            .record_attestation_verified(AttestationSession {
                verdict: verdict(),
                started_at: UNIX_EPOCH,
                tpm_heartbeat_at: UNIX_EPOCH,
                gpu_reattest_at: UNIX_EPOCH,
            });
        let readiness = AtomicUsize::new(0);
        let establish = AtomicUsize::new(0);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind endpoint");
        let path = journal("stale");
        let result = confidential_generate_with(
            ConfidentialCall {
                request: &request(),
                journal_path: &path,
                endpoint: &endpoint(listener.local_addr().expect("address").port()),
                config: &Map::new(),
                runtime: &runtime,
                now: UNIX_EPOCH + Duration::from_secs(10 * 60),
            },
            |_| {
                readiness.fetch_add(1, Ordering::SeqCst);
                NvattestEnsureStatus::AlreadyInstalled
            },
            |_, _| {
                establish.fetch_add(1, Ordering::SeqCst);
                Err("tls_handshake_failed")
            },
        );
        assert!(matches!(result, ConfidentialResult::AttestationStale));
        assert_eq!(readiness.load(Ordering::SeqCst), 0);
        assert_eq!(establish.load(Ordering::SeqCst), 0);
        assert_no_endpoint_request(&listener);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn converse_readiness_failure_refuses_without_attempting_a_channel() {
        let runtime = EndpointRuntime::default();
        let attempts = AtomicUsize::new(0);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind endpoint");
        let path = journal("converse-not-verified");
        let messages = converse_messages();
        let tools = converse_tools();
        let failure = confidential_converse_with(
            ConfidentialConverseCall {
                request: &request(),
                messages: &messages,
                tools: &tools,
                journal_path: &path,
                endpoint: &endpoint(listener.local_addr().expect("address").port()),
                config: &Map::new(),
                runtime: &runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::InstallFailed,
            |_, _| {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err("tls_handshake_failed")
            },
        )
        .expect_err("attestation prerequisite failure");
        assert_eq!(failure.reason_code, "attestation_not_yet_verified");
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
        assert_no_endpoint_request(&listener);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn converse_channel_failure_refuses_without_an_endpoint_request() {
        let runtime = EndpointRuntime::default();
        let attempts = AtomicUsize::new(0);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind endpoint");
        let path = journal("converse-failed");
        let messages = converse_messages();
        let tools = converse_tools();
        let failure = confidential_converse_with(
            ConfidentialConverseCall {
                request: &request(),
                messages: &messages,
                tools: &tools,
                journal_path: &path,
                endpoint: &endpoint(listener.local_addr().expect("address").port()),
                config: &Map::new(),
                runtime: &runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _| {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err("tls_handshake_failed")
            },
        )
        .expect_err("channel failure");
        assert_eq!(failure.reason_code, "attestation_failed");
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_no_endpoint_request(&listener);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn converse_stale_session_refuses_before_readiness_or_channel_establishment() {
        let runtime = EndpointRuntime::default();
        runtime
            .attestation_state()
            .record_attestation_verified(AttestationSession {
                verdict: verdict(),
                started_at: UNIX_EPOCH,
                tpm_heartbeat_at: UNIX_EPOCH,
                gpu_reattest_at: UNIX_EPOCH,
            });
        let readiness = AtomicUsize::new(0);
        let establish = AtomicUsize::new(0);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind endpoint");
        let path = journal("converse-stale");
        let messages = converse_messages();
        let tools = converse_tools();
        let failure = confidential_converse_with(
            ConfidentialConverseCall {
                request: &request(),
                messages: &messages,
                tools: &tools,
                journal_path: &path,
                endpoint: &endpoint(listener.local_addr().expect("address").port()),
                config: &Map::new(),
                runtime: &runtime,
                now: UNIX_EPOCH + Duration::from_secs(10 * 60),
            },
            |_| {
                readiness.fetch_add(1, Ordering::SeqCst);
                NvattestEnsureStatus::AlreadyInstalled
            },
            |_, _| {
                establish.fetch_add(1, Ordering::SeqCst);
                Err("tls_handshake_failed")
            },
        )
        .expect_err("stale attestation");
        assert_eq!(failure.reason_code, "attestation_stale");
        assert_eq!(readiness.load(Ordering::SeqCst), 0);
        assert_eq!(establish.load(Ordering::SeqCst), 0);
        assert_no_endpoint_request(&listener);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn confidential_converse_without_a_served_window_posts_nothing_after_attestation() {
        let runtime = EndpointRuntime::default();
        let establish = AtomicUsize::new(0);
        let writes = Arc::new(AtomicUsize::new(0));
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind endpoint");
        let path = journal("converse-no-window");
        let messages = converse_messages();
        let tools = converse_tools();
        let writes_for_channel = writes.clone();
        let failure = confidential_converse_with(
            ConfidentialConverseCall {
                request: &request(),
                messages: &messages,
                tools: &tools,
                journal_path: &path,
                endpoint: &endpoint(listener.local_addr().expect("address").port()),
                config: &Map::new(),
                runtime: &runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _| {
                establish.fetch_add(1, Ordering::SeqCst);
                Ok(EstablishedChannel {
                    verdict: verdict(),
                    stream: Box::new(NoRequestStream {
                        writes: writes_for_channel,
                    }),
                })
            },
        )
        .expect_err("missing served window");
        assert_eq!(failure.reason_code, "context_budget_exceeded");
        assert_eq!(establish.load(Ordering::SeqCst), 1);
        assert_eq!(writes.load(Ordering::SeqCst), 0);
        assert_no_endpoint_request(&listener);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn nvattest_directory_uses_explicit_confidential_config() {
        let journal = Path::new("/journal");
        let explicit = json!({"services": {"confidential": {"nvattest_dir": "/explicit"}}})
            .as_object()
            .expect("config")
            .clone();
        assert_eq!(
            resolve_nvattest_dir(&explicit, journal),
            PathBuf::from("/explicit")
        );
    }
}
