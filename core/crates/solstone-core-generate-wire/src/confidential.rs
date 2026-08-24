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

#[allow(dead_code)]
fn endpoint_converse_now<T: EndpointTransport>(
    call: EndpointConverseCall<'_>,
    transport: &mut T,
) -> EndpointConverseResult {
    endpoint_converse_with(call, transport, Instant::now())
}

#[allow(dead_code)]
fn confidential_transport_generate(
    request: &GenerateRequest,
    journal_path: &Path,
    endpoint: &ByoEndpoint,
    config: &Map<String, Value>,
    runtime: &EndpointRuntime,
    stream: Box<dyn AttestedIo>,
) -> EndpointResult {
    let target = ratls_target(&endpoint.base_url).expect("test endpoint parses");
    let mut transport = AttestedEndpointTransport {
        stream,
        host: target.host,
    };
    endpoint_generate_with(
        request,
        journal_path,
        endpoint,
        config,
        runtime,
        &mut transport,
        Instant::now(),
    )
}

#[allow(dead_code, clippy::too_many_arguments)]
fn confidential_transport_converse(
    request: &GenerateRequest,
    messages: &[ConverseMessage],
    tools: &[ConverseToolSpec],
    journal_path: &Path,
    endpoint: &ByoEndpoint,
    config: &Map<String, Value>,
    runtime: &EndpointRuntime,
    stream: Box<dyn AttestedIo>,
) -> EndpointConverseResult {
    let target = ratls_target(&endpoint.base_url).expect("test endpoint parses");
    let mut transport = AttestedEndpointTransport {
        stream,
        host: target.host,
    };
    endpoint_converse_now(
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
    )
}

/// Drives the confidential transport adapter directly over a caller-supplied channel,
/// bypassing readiness/establish so integration tests can exercise a real socket without
/// real attestation. Exposes only the post-establishment adapter call — no `ConfidentialCall`,
/// `EstablishedChannel`, or attestation-state bookkeeping.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub mod test_support {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    pub fn confidential_converse_with_controls<R, E>(
        request: &GenerateRequest,
        messages: &[ConverseMessage],
        tools: &[ConverseToolSpec],
        journal_path: &Path,
        endpoint: &ByoEndpoint,
        config: &Map<String, Value>,
        runtime: &EndpointRuntime,
        now: SystemTime,
        readiness: R,
        establish: E,
    ) -> EndpointConverseResult
    where
        R: FnOnce(&Path) -> NvattestEnsureStatus,
        E: FnOnce(
            &RatlsEndpoint,
            &Path,
        ) -> Result<(CompositeVerdict, Box<dyn AttestedIo>), &'static str>,
    {
        confidential_converse_with(
            ConfidentialConverseCall {
                request,
                messages,
                tools,
                journal_path,
                endpoint,
                config,
                runtime,
                now,
            },
            readiness,
            |ratls_endpoint, nvattest_dir| {
                establish(ratls_endpoint, nvattest_dir)
                    .map(|(verdict, stream)| EstablishedChannel { verdict, stream })
            },
        )
    }

    pub fn confidential_generate_over_channel(
        request: &GenerateRequest,
        journal_path: &Path,
        endpoint: &ByoEndpoint,
        config: &Map<String, Value>,
        runtime: &EndpointRuntime,
        stream: Box<dyn AttestedIo>,
    ) -> EndpointResult {
        confidential_transport_generate(request, journal_path, endpoint, config, runtime, stream)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn confidential_converse_over_channel(
        request: &GenerateRequest,
        messages: &[ConverseMessage],
        tools: &[ConverseToolSpec],
        journal_path: &Path,
        endpoint: &ByoEndpoint,
        config: &Map<String, Value>,
        runtime: &EndpointRuntime,
        stream: Box<dyn AttestedIo>,
    ) -> EndpointConverseResult {
        confidential_transport_converse(
            request,
            messages,
            tools,
            journal_path,
            endpoint,
            config,
            runtime,
            stream,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        io::{self, Read, Write},
        rc::Rc,
        sync::atomic::{AtomicUsize, Ordering},
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

    fn parsed_request(written: &Rc<RefCell<Vec<u8>>>) -> (String, Value) {
        let bytes = written.borrow();
        let text = std::str::from_utf8(&bytes).expect("request UTF-8");
        let (head, body) = text.split_once("\r\n\r\n").expect("header/body split");
        (
            head.to_owned(),
            serde_json::from_str(body).expect("JSON body"),
        )
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

    struct RecordingChannel {
        written: Rc<RefCell<Vec<u8>>>,
        response: io::Cursor<Vec<u8>>,
    }

    impl RecordingChannel {
        fn new(written: Rc<RefCell<Vec<u8>>>, response_body: &str) -> Self {
            let framed = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{response_body}",
                response_body.len()
            );
            Self {
                written,
                response: io::Cursor::new(framed.into_bytes()),
            }
        }
    }

    impl Read for RecordingChannel {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.response.read(buf)
        }
    }

    impl Write for RecordingChannel {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl AttestedIo for RecordingChannel {
        fn set_io_timeout(&mut self, _timeout: Option<Duration>) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn fresh_attestation_uses_one_channel_request_with_confidential_qwen_controls() {
        let written = Rc::new(RefCell::new(Vec::new()));
        let written_for_channel = written.clone();
        let runtime = EndpointRuntime::default();
        let path = journal("success");
        let endpoint = endpoint(1);
        let response_body = r#"{"choices":[{"message":{"content":"OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}"#;
        let result = confidential_generate_with(
            ConfidentialCall {
                request: &request(),
                journal_path: &path,
                endpoint: &endpoint,
                config: &Map::new(),
                runtime: &runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _| {
                Ok(EstablishedChannel {
                    verdict: verdict(),
                    stream: Box::new(RecordingChannel::new(written_for_channel, response_body)),
                })
            },
        );
        assert!(matches!(result, ConfidentialResult::Generated(_)));
        let (head, body) = parsed_request(&written);
        for field in [
            "chat_template_kwargs",
            "top_p",
            "top_k",
            "min_p",
            "presence_penalty",
        ] {
            assert!(body.get(field).is_some(), "missing {field}");
        }
        assert!(
            head.lines()
                .any(|line| line == "Authorization: Bearer token"),
            "missing bearer: {head}"
        );
        assert!(
            head.lines().any(|line| line == "Host: 127.0.0.1:1"),
            "missing host: {head}"
        );
        assert!(
            head.lines()
                .any(|line| line == "Content-Type: application/json"),
            "missing content-type: {head}"
        );
        let raw = written.borrow();
        let text = std::str::from_utf8(&raw).expect("request UTF-8");
        let (_, raw_body) = text.split_once("\r\n\r\n").expect("header/body split");
        let declared = head
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .expect("content length")
            .parse::<usize>()
            .expect("numeric content length");
        assert_eq!(declared, raw_body.len());
        drop(raw);

        let unauthed_written = Rc::new(RefCell::new(Vec::new()));
        let unauthed_written_for_channel = unauthed_written.clone();
        let mut unauthed = endpoint.clone();
        unauthed.credential = None;
        let unauthed_runtime = EndpointRuntime::default();
        let unauthed_result = confidential_generate_with(
            ConfidentialCall {
                request: &request(),
                journal_path: &path,
                endpoint: &unauthed,
                config: &Map::new(),
                runtime: &unauthed_runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _| {
                Ok(EstablishedChannel {
                    verdict: verdict(),
                    stream: Box::new(RecordingChannel::new(
                        unauthed_written_for_channel,
                        response_body,
                    )),
                })
            },
        );
        assert!(matches!(unauthed_result, ConfidentialResult::Generated(_)));
        let (unauthed_head, _) = parsed_request(&unauthed_written);
        assert!(
            !unauthed_head
                .lines()
                .any(|line| line.starts_with("Authorization:")),
            "unexpected authorization: {unauthed_head}"
        );
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn endpoint_and_confidential_converse_parse_the_same_tool_turn() {
        let response_body = converse_response_body();
        let path = journal("converse-equivalence");
        let request = request();
        let messages = converse_messages();
        let tools = converse_tools();
        let config = served_window_config();
        let endpoint = endpoint(1);
        let endpoint_runtime = EndpointRuntime::default();
        let mut endpoint_transport = ResponseTransport {
            response: HttpResponse {
                status: 200,
                body: response_body.clone(),
            },
        };
        let endpoint_turn = endpoint_converse_now(
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
                    stream: Box::new(RecordingChannel::new(
                        Rc::new(RefCell::new(Vec::new())),
                        &response_body,
                    )),
                })
            },
        )
        .expect("confidential converse turn");
        assert_eq!(endpoint_turn, confidential_turn);

        let malformed = json!({
            "choices": [{
                "message": {
                    "content": "before",
                    "tool_calls": [{
                        "id": "call-1",
                        "type": "function",
                        "function": {"name": "weather", "arguments": "{not json"},
                    }],
                },
                "finish_reason": "stop",
            }],
            "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5},
        })
        .to_string();
        let mut malformed_endpoint_transport = ResponseTransport {
            response: HttpResponse {
                status: 200,
                body: malformed.clone(),
            },
        };
        let malformed_endpoint = endpoint_converse_now(
            EndpointConverseCall {
                request: &request,
                messages: &messages,
                tools: &tools,
                journal_path: &path,
                endpoint: &endpoint,
                config: &config,
                runtime: &EndpointRuntime::default(),
            },
            &mut malformed_endpoint_transport,
        );
        let malformed_confidential = confidential_converse_with(
            ConfidentialConverseCall {
                request: &request,
                messages: &messages,
                tools: &tools,
                journal_path: &path,
                endpoint: &endpoint,
                config: &config,
                runtime: &EndpointRuntime::default(),
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _| {
                Ok(EstablishedChannel {
                    verdict: verdict(),
                    stream: Box::new(RecordingChannel::new(
                        Rc::new(RefCell::new(Vec::new())),
                        &malformed,
                    )),
                })
            },
        );
        assert!(malformed_endpoint.is_err());
        assert!(malformed_confidential.is_err());
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn fresh_attestation_uses_one_channel_converse_request_with_qwen_controls() {
        let written = Rc::new(RefCell::new(Vec::new()));
        let written_for_channel = written.clone();
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
                endpoint: &endpoint(1),
                config: &served_window_config(),
                runtime: &runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _| {
                Ok(EstablishedChannel {
                    verdict: verdict(),
                    stream: Box::new(RecordingChannel::new(
                        written_for_channel,
                        &converse_response_body(),
                    )),
                })
            },
        );
        assert!(result.is_ok());
        let (_, body) = parsed_request(&written);
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
    fn readiness_failure_refuses_without_attempting_a_channel() {
        let runtime = EndpointRuntime::default();
        let attempts = AtomicUsize::new(0);
        let path = journal("not-verified");
        let result = confidential_generate_with(
            ConfidentialCall {
                request: &request(),
                journal_path: &path,
                endpoint: &endpoint(1),
                config: &Map::new(),
                runtime: &runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::Unavailable,
            |_, _| {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err("tls_handshake_failed")
            },
        );
        assert!(matches!(result, ConfidentialResult::AttestationNotVerified));
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
        assert_eq!(
            runtime
                .attestation_state()
                .get_attestation_state()
                .failure
                .as_ref()
                .map(|failure| failure.reason_code),
            Some("nvattest_unavailable")
        );
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn channel_failure_refuses_without_an_endpoint_request() {
        let runtime = EndpointRuntime::default();
        let attempts = AtomicUsize::new(0);
        let path = journal("failed");
        let result = confidential_generate_with(
            ConfidentialCall {
                request: &request(),
                journal_path: &path,
                endpoint: &endpoint(1),
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
        let path = journal("stale");
        let result = confidential_generate_with(
            ConfidentialCall {
                request: &request(),
                journal_path: &path,
                endpoint: &endpoint(1),
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
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn converse_readiness_failure_refuses_without_attempting_a_channel() {
        let runtime = EndpointRuntime::default();
        let attempts = AtomicUsize::new(0);
        let path = journal("converse-not-verified");
        let messages = converse_messages();
        let tools = converse_tools();
        let failure = confidential_converse_with(
            ConfidentialConverseCall {
                request: &request(),
                messages: &messages,
                tools: &tools,
                journal_path: &path,
                endpoint: &endpoint(1),
                config: &Map::new(),
                runtime: &runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::IntegrityFailed,
            |_, _| {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err("tls_handshake_failed")
            },
        )
        .expect_err("attestation prerequisite failure");
        assert_eq!(failure.reason_code, "attestation_not_yet_verified");
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
        assert_eq!(
            runtime
                .attestation_state()
                .get_attestation_state()
                .failure
                .as_ref()
                .map(|failure| failure.reason_code),
            Some("nvattest_integrity_failed")
        );
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn converse_channel_failure_refuses_without_an_endpoint_request() {
        let runtime = EndpointRuntime::default();
        let attempts = AtomicUsize::new(0);
        let path = journal("converse-failed");
        let messages = converse_messages();
        let tools = converse_tools();
        let failure = confidential_converse_with(
            ConfidentialConverseCall {
                request: &request(),
                messages: &messages,
                tools: &tools,
                journal_path: &path,
                endpoint: &endpoint(1),
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
        let path = journal("converse-stale");
        let messages = converse_messages();
        let tools = converse_tools();
        let failure = confidential_converse_with(
            ConfidentialConverseCall {
                request: &request(),
                messages: &messages,
                tools: &tools,
                journal_path: &path,
                endpoint: &endpoint(1),
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
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn confidential_converse_without_a_served_window_posts_nothing_after_attestation() {
        let runtime = EndpointRuntime::default();
        let establish = AtomicUsize::new(0);
        let written = Rc::new(RefCell::new(Vec::new()));
        let written_for_channel = written.clone();
        let path = journal("converse-no-window");
        let messages = converse_messages();
        let tools = converse_tools();
        let failure = confidential_converse_with(
            ConfidentialConverseCall {
                request: &request(),
                messages: &messages,
                tools: &tools,
                journal_path: &path,
                endpoint: &endpoint(1),
                config: &Map::new(),
                runtime: &runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _| {
                establish.fetch_add(1, Ordering::SeqCst);
                Ok(EstablishedChannel {
                    verdict: verdict(),
                    stream: Box::new(RecordingChannel::new(written_for_channel, "")),
                })
            },
        )
        .expect_err("missing served window");
        assert_eq!(failure.reason_code, "context_budget_exceeded");
        assert_eq!(establish.load(Ordering::SeqCst), 1);
        assert!(written.borrow().is_empty());
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
