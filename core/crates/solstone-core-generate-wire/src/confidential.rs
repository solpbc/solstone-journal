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
    RatlsEndpoint, classify_channel_failure, classify_nvattest_prerequisite,
    ensure_nvattest_installed, establish_production_attested_channel, send_json_request,
};

use crate::endpoint::{
    EndpointFailure, EndpointGenerated, EndpointResult, EndpointRuntime, EndpointTransport,
    EndpointTransportError, endpoint_generate_with,
};

const ATTESTED_CHANNEL_TIMEOUT: Duration = Duration::from_secs(120);

pub enum ConfidentialResult {
    Generated(EndpointGenerated),
    Failed(EndpointFailure),
    AttestationNotVerified,
    AttestationFailed(&'static str),
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
        ensure_nvattest_installed,
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

fn attestation_refusal_is_health_probe(request: &GenerateRequest) -> bool {
    request.context == solstone_core_generate::HEALTH_BRAIN_GENERATE_CONTEXT
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
    let nvattest_dir = resolve_nvattest_dir(config, journal_path);
    if let Some(failure) = classify_nvattest_prerequisite(readiness(&nvattest_dir)) {
        if !attestation_refusal_is_health_probe(request) {
            solstone_core_brain::record_confidential_attestation_refusal(
                journal_path,
                config,
                failure.reason_code,
            );
        }
        runtime
            .attestation_state()
            .record_attestation_failed(failure.kind, failure.reason_code);
        return ConfidentialResult::AttestationNotVerified;
    }

    let target = match ratls_target(&endpoint.base_url) {
        Some(target) => target,
        None => {
            if !attestation_refusal_is_health_probe(request) {
                solstone_core_brain::record_confidential_attestation_refusal(
                    journal_path,
                    config,
                    "tls_handshake_failed",
                );
            }
            runtime.attestation_state().record_attestation_failed(
                classify_channel_failure("tls_handshake_failed"),
                "tls_handshake_failed",
            );
            return ConfidentialResult::AttestationFailed("tls_handshake_failed");
        }
    };
    let EstablishedChannel { verdict, stream } = match establish(&target.endpoint, &nvattest_dir) {
        Ok(channel) => channel,
        Err(reason_code) => {
            if !attestation_refusal_is_health_probe(request) {
                solstone_core_brain::record_confidential_attestation_refusal(
                    journal_path,
                    config,
                    reason_code,
                );
            }
            runtime
                .attestation_state()
                .record_attestation_failed(classify_channel_failure(reason_code), reason_code);
            return ConfidentialResult::AttestationFailed(reason_code);
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
        host: target.host.clone(),
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

fn authority(base_url: &str) -> Option<&str> {
    let auth = base_url
        .strip_prefix("https://")
        .or_else(|| base_url.strip_prefix("http://"))?
        .split('/')
        .next()?;
    if auth.is_empty() { None } else { Some(auth) }
}

fn ratls_target(base_url: &str) -> Option<RatlsTarget> {
    let auth = authority(base_url)?;
    let (host, port) = auth
        .rsplit_once(':')
        .and_then(|(host, port)| port.parse::<u16>().ok().map(|port| (host, port)))
        .unwrap_or((auth, 443));
    if host.is_empty() {
        return None;
    }
    Some(RatlsTarget {
        endpoint: RatlsEndpoint::new(host, port),
        host: auth.to_owned(),
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
        // Model discovery is optional in endpoint_generate_with;
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
        host: target.host.clone(),
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

/// Drives the confidential transport adapter directly over a caller-supplied channel,
/// bypassing readiness/establish so integration tests can exercise a real socket without
/// real attestation. Exposes only the post-establishment adapter call — no `ConfidentialCall`,
/// `EstablishedChannel`, or attestation-state bookkeeping.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub mod test_support {
    use super::*;

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

    pub(crate) fn request() -> GenerateRequest {
        GenerateRequest {
            id: None,
            context: "test.confidential".into(),
            contents: vec![ContentPart::Text {
                text: "Hello".into(),
            }],
            system_instruction: None,
            temperature: 0.2,
            max_output_tokens: 64,
            timeout_s: None,
            json_output: false,
            json_schema: None,
            enforce_responsiveness: false,
            attempt_index: 0,
            exclusive_admission: false,
            transport_retries: None,
        }
    }

    pub(crate) fn endpoint(port: u16) -> ByoEndpoint {
        ByoEndpoint {
            base_url: format!("http://127.0.0.1:{port}"),
            served_model_id: "served".into(),
            credential: Some("token".into()),
            parallel_slots: None,
            is_confidential: true,
            is_bundled: false,
        }
    }

    pub(crate) fn verdict() -> CompositeVerdict {
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

    pub(crate) fn journal(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "solstone-confidential-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create journal");
        path
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

    pub(crate) struct RecordingChannel {
        written: Rc<RefCell<Vec<u8>>>,
        response: io::Cursor<Vec<u8>>,
    }

    impl RecordingChannel {
        fn new(written: Rc<RefCell<Vec<u8>>>, response_body: &str) -> Self {
            Self::with_status(written, 200, "OK", response_body)
        }

        pub(crate) fn with_status(
            written: Rc<RefCell<Vec<u8>>>,
            status: u16,
            reason: &str,
            response_body: &str,
        ) -> Self {
            let framed = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\n\r\n{response_body}",
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
        for cause in ["tls_handshake_failed", "gpu_nonce_mismatch"] {
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
                    Err(cause)
                },
            );
            assert!(matches!(
                result,
                ConfidentialResult::AttestationFailed(detail) if detail == cause
            ));
            assert_eq!(attempts.load(Ordering::SeqCst), 1);
            let _ = std::fs::remove_dir_all(path);
        }
    }

    /// A session older than the attestation cadence is replaced, never a reason to stop.
    ///
    /// Every call attests its own channel before any request is written, so a stale
    /// record from an earlier call must lead to a fresh attestation. Refusing on it
    /// instead left a long-lived generate session unable to reach the lane again
    /// after one idle stretch.
    #[test]
    fn stale_session_reattests_and_generates_on_the_fresh_channel() {
        let runtime = EndpointRuntime::default();
        runtime
            .attestation_state()
            .record_attestation_verified(AttestationSession {
                verdict: verdict(),
                started_at: UNIX_EPOCH,
                tpm_heartbeat_at: UNIX_EPOCH,
                gpu_reattest_at: UNIX_EPOCH,
            });
        let now = UNIX_EPOCH + Duration::from_secs(10 * 60);
        let readiness = AtomicUsize::new(0);
        let establish = AtomicUsize::new(0);
        let written = Rc::new(RefCell::new(Vec::new()));
        let written_for_channel = written.clone();
        let path = journal("stale");
        let response_body = r#"{"choices":[{"message":{"content":"OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}"#;
        let result = confidential_generate_with(
            ConfidentialCall {
                request: &request(),
                journal_path: &path,
                endpoint: &endpoint(1),
                config: &Map::new(),
                runtime: &runtime,
                now,
            },
            |_| {
                readiness.fetch_add(1, Ordering::SeqCst);
                NvattestEnsureStatus::AlreadyInstalled
            },
            |_, _| {
                establish.fetch_add(1, Ordering::SeqCst);
                Ok(EstablishedChannel {
                    verdict: verdict(),
                    stream: Box::new(RecordingChannel::new(written_for_channel, response_body)),
                })
            },
        );
        assert!(matches!(result, ConfidentialResult::Generated(_)));
        assert_eq!(readiness.load(Ordering::SeqCst), 1);
        assert_eq!(establish.load(Ordering::SeqCst), 1);
        assert!(!written.borrow().is_empty());
        let session = runtime
            .attestation_state()
            .get_attestation_state()
            .session
            .expect("fresh session recorded");
        assert_eq!(session.started_at, now);
        assert_eq!(session.status(now), "verified");
        let _ = std::fs::remove_dir_all(path);
    }

    /// A stale session whose fresh attestation fails refuses the call, writes nothing
    /// and drops the stale session, so the next call attests again rather than reusing it.
    #[test]
    fn stale_session_refuses_and_writes_nothing_when_its_fresh_attestation_fails() {
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
        let path = journal("stale-failed");
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
        assert!(matches!(
            result,
            ConfidentialResult::AttestationFailed(detail) if detail == "tls_handshake_failed"
        ));
        assert_eq!(readiness.load(Ordering::SeqCst), 1);
        assert_eq!(establish.load(Ordering::SeqCst), 1);
        assert!(
            runtime
                .attestation_state()
                .get_attestation_state()
                .session
                .is_none()
        );
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn generation_destination_host_and_header_are_explicit_or_default_port() {
        let cases = [
            (
                "https://127.0.0.1:8443/v1/",
                "127.0.0.1",
                8443,
                "Host: 127.0.0.1:8443",
            ),
            (
                "https://attested.example/v1",
                "attested.example",
                443,
                "Host: attested.example",
            ),
        ];

        for (url, expected_host, expected_port, expected_host_header) in cases {
            let config_json = json!({
                "providers": {
                    "active": {"provider": "local"},
                    "local": {
                        "endpoint_url": url,
                        "served_model_id": "served"
                    }
                },
                "services": {"confidential": {}}
            });
            let config_map = config_json.as_object().unwrap().clone();
            let empty_env = |_name: &str| -> Option<String> { None };
            let (_, lane) = crate::lane::resolve_lane_with(&config_map, empty_env);
            let crate::lane::LaneOutcome::ConfidentialEndpoint(endpoint) = lane else {
                panic!("expected confidential endpoint");
            };

            let written = Rc::new(RefCell::new(Vec::new()));
            let written_for_channel = written.clone();
            let runtime = EndpointRuntime::default();
            let path = journal("gen-destination");
            let response_body = r#"{"choices":[{"message":{"content":"OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}"#;
            let recorded_target = Rc::new(RefCell::new(None));
            let recorded_target_clone = recorded_target.clone();

            let result = confidential_generate_with(
                ConfidentialCall {
                    request: &request(),
                    journal_path: &path,
                    endpoint: &endpoint,
                    config: &config_map,
                    runtime: &runtime,
                    now: UNIX_EPOCH,
                },
                |_| NvattestEnsureStatus::AlreadyInstalled,
                |ratls_endpoint, _nvattest_dir| {
                    *recorded_target_clone.borrow_mut() =
                        Some((ratls_endpoint.host.clone(), ratls_endpoint.port));
                    Ok(EstablishedChannel {
                        verdict: verdict(),
                        stream: Box::new(RecordingChannel::new(
                            written_for_channel.clone(),
                            response_body,
                        )),
                    })
                },
            );

            assert!(matches!(result, ConfidentialResult::Generated(_)));
            assert_eq!(
                *recorded_target.borrow(),
                Some((expected_host.to_owned(), expected_port))
            );
            let (head, _) = parsed_request(&written);
            assert!(
                head.lines().any(|line| line == expected_host_header),
                "expected header {expected_host_header} in {head}"
            );
            let _ = std::fs::remove_dir_all(path);
        }
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

    fn active_spp_config() -> Map<String, Value> {
        json!({
            "services": {
                "confidential": {
                    "device": "abc",
                    "endpoint_url": "http://127.0.0.1:9099",
                    "served_model_id": "served",
                    "credential_fingerprint_sha256": "cca56da30e3c8a13a11277193fd3263961e2e3d6d9f98038a91dac05e8fde16a"
                }
            },
            "providers": {
                "active": {"provider": "local", "model": "served"},
                "local": {"endpoint_url": "http://127.0.0.1:9099", "served_model_id": "served", "credential": "endpoint-credential"}
            }
        })
        .as_object()
        .unwrap()
        .clone()
    }

    fn setup_journal(path: &Path, config: &Map<String, Value>) {
        std::fs::create_dir_all(path.join("config")).unwrap();
        std::fs::write(
            path.join("config/journal.json"),
            serde_json::to_vec(config).unwrap(),
        )
        .unwrap();
        solstone_core_brain::generate_fingerprint_key(path).unwrap();
    }

    fn spp_ready_evidence(now: chrono::DateTime<chrono::Utc>) -> Value {
        let observed = now.to_rfc3339();
        let expires = (now + chrono::Duration::hours(2)).to_rfc3339();
        json!({
            "configuration": {"status": "ok", "observed_at": observed, "expires_at": expires},
            "generate": {"status": "ok", "observed_at": observed, "expires_at": expires},
            "lane_prerequisites": {
                "status": "ok",
                "observed_at": observed,
                "expires_at": expires,
            }
        })
    }

    #[test]
    fn confidential_generate_attestation_failure_records_refusal_in_brain() {
        let runtime = EndpointRuntime::default();
        let path = journal("generate-attest-refusal");
        let config_map = active_spp_config();
        setup_journal(&path, &config_map);

        let endpoint = endpoint(9099);
        let req = request();

        // 1. Regular request records refusal
        let result = confidential_generate_with(
            ConfidentialCall {
                request: &req,
                journal_path: &path,
                endpoint: &endpoint,
                config: &config_map,
                runtime: &runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _| Err("gateway_unreachable"),
        );
        assert!(matches!(
            result,
            ConfidentialResult::AttestationFailed("gateway_unreachable")
        ));

        let now = chrono::Utc::now() + chrono::Duration::seconds(1);
        let inspection = solstone_core_brain::inspect_brain_state(&path, &config_map, now);
        assert_eq!(inspection.projection.aggregate_state, "blocked");
        assert_eq!(
            inspection.projection.reason_code.as_deref(),
            Some("attestation_not_verified")
        );
        let record = inspection.record.expect("record exists");
        assert_eq!(
            record["evidence"]["lane_prerequisites"]["reason_code"],
            "attestation_not_verified"
        );
        let rev1 = record["revision"].as_u64().unwrap();

        // 2. Health probe request skips recording refusal
        let mut probe_req = request();
        probe_req.context = solstone_core_generate::HEALTH_BRAIN_GENERATE_CONTEXT.to_owned();
        let probe_result = confidential_generate_with(
            ConfidentialCall {
                request: &probe_req,
                journal_path: &path,
                endpoint: &endpoint,
                config: &config_map,
                runtime: &runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _| Err("tls_handshake_failed"),
        );
        assert!(matches!(
            probe_result,
            ConfidentialResult::AttestationFailed("tls_handshake_failed")
        ));

        // State remains from prior refusal and revision did not bump
        let inspection2 = solstone_core_brain::inspect_brain_state(&path, &config_map, now);
        let record2 = inspection2.record.expect("record exists");
        assert_eq!(record2["revision"].as_u64().unwrap(), rev1);
        assert_eq!(
            record2["evidence"]["lane_prerequisites"]["reason_code"],
            "attestation_not_verified"
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn confidential_generate_readiness_failure_records_refusal() {
        let runtime = EndpointRuntime::default();
        let path = journal("generate-readiness-refusal");
        let config_map = active_spp_config();
        setup_journal(&path, &config_map);

        let endpoint = endpoint(9099);
        let req = request();

        let result = confidential_generate_with(
            ConfidentialCall {
                request: &req,
                journal_path: &path,
                endpoint: &endpoint,
                config: &config_map,
                runtime: &runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::InstallInFlight,
            |_, _| Err("gateway_unreachable"),
        );
        assert!(matches!(result, ConfidentialResult::AttestationNotVerified));

        let now = chrono::Utc::now() + chrono::Duration::seconds(1);
        let inspection = solstone_core_brain::inspect_brain_state(&path, &config_map, now);
        assert_eq!(inspection.projection.aggregate_state, "blocked");
        assert_eq!(
            inspection.projection.reason_code.as_deref(),
            Some("nvattest_install_in_progress")
        );
        let record = inspection.record.expect("record exists");
        assert_eq!(
            record["evidence"]["lane_prerequisites"]["reason_code"],
            "nvattest_install_in_progress"
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn confidential_generate_invalid_ratls_target_records_refusal() {
        let runtime = EndpointRuntime::default();
        let path = journal("generate-invalid-ratls");
        let config_map = active_spp_config();
        setup_journal(&path, &config_map);

        let mut invalid_endpoint = endpoint(9099);
        invalid_endpoint.base_url = "not a valid url".to_owned();
        let req = request();

        let result = confidential_generate_with(
            ConfidentialCall {
                request: &req,
                journal_path: &path,
                endpoint: &invalid_endpoint,
                config: &config_map,
                runtime: &runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _| Err("gateway_unreachable"),
        );
        assert!(matches!(
            result,
            ConfidentialResult::AttestationFailed("tls_handshake_failed")
        ));

        let now = chrono::Utc::now() + chrono::Duration::seconds(1);
        let inspection = solstone_core_brain::inspect_brain_state(&path, &config_map, now);
        assert_eq!(inspection.projection.aggregate_state, "unhealthy");
        assert_eq!(
            inspection.projection.reason_code.as_deref(),
            Some("attestation_rejected")
        );
        let record = inspection.record.expect("record exists");
        assert_eq!(
            record["evidence"]["lane_prerequisites"]["reason_code"],
            "attestation_rejected"
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn confidential_generate_health_probe_during_begin_refresh_preserves_permit() {
        let runtime = EndpointRuntime::default();
        let path = journal("generate-health-probe-refresh");
        let config_map = active_spp_config();
        setup_journal(&path, &config_map);

        let now = chrono::Utc::now();
        let permit = solstone_core_brain::begin_refresh(
            &path,
            now,
            Some("test-run".to_owned()),
            None,
            false,
            None,
        )
        .expect("begin_refresh succeeds");

        let brain_bytes_before = std::fs::read(path.join("health/brain.json")).unwrap();

        let endpoint = endpoint(9099);
        let mut probe_req = request();
        probe_req.context = solstone_core_generate::HEALTH_BRAIN_GENERATE_CONTEXT.to_owned();

        let result = confidential_generate_with(
            ConfidentialCall {
                request: &probe_req,
                journal_path: &path,
                endpoint: &endpoint,
                config: &config_map,
                runtime: &runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _| Err("certificate_invalid"),
        );
        assert!(matches!(
            result,
            ConfidentialResult::AttestationFailed("certificate_invalid")
        ));

        let brain_bytes_after = std::fs::read(path.join("health/brain.json")).unwrap();
        assert_eq!(brain_bytes_before, brain_bytes_after);

        let outcome = spp_ready_evidence(now);
        let finish_result =
            solstone_core_brain::finish_refresh(&path, permit.unwrap(), outcome, now, None);
        assert!(finish_result.is_ok());

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn confidential_generate_ordinary_request_during_begin_refresh_causes_conflict() {
        let runtime = EndpointRuntime::default();
        let path = journal("generate-ordinary-refresh-conflict");
        let config_map = active_spp_config();
        setup_journal(&path, &config_map);

        let now = chrono::Utc::now();
        let permit = solstone_core_brain::begin_refresh(
            &path,
            now,
            Some("test-run".to_owned()),
            None,
            false,
            None,
        )
        .expect("begin_refresh succeeds")
        .expect("permit exists");

        let endpoint = endpoint(9099);
        let ordinary_req = request();

        let result = confidential_generate_with(
            ConfidentialCall {
                request: &ordinary_req,
                journal_path: &path,
                endpoint: &endpoint,
                config: &config_map,
                runtime: &runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _| Err("certificate_invalid"),
        );
        assert!(matches!(
            result,
            ConfidentialResult::AttestationFailed("certificate_invalid")
        ));

        let check_now = chrono::Utc::now() + chrono::Duration::seconds(1);
        let inspection = solstone_core_brain::inspect_brain_state(&path, &config_map, check_now);
        assert_eq!(inspection.projection.aggregate_state, "unhealthy");
        assert_eq!(
            inspection.projection.reason_code.as_deref(),
            Some("attestation_rejected")
        );

        let outcome = spp_ready_evidence(now);
        let finish_result = solstone_core_brain::finish_refresh(&path, permit, outcome, now, None);
        assert!(matches!(
            finish_result,
            Err(solstone_core_brain::WriterError::Conflict(_))
        ));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn confidential_generate_http_400_does_not_write_attestation_refusal() {
        let written = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let written_for_channel = written.clone();
        let runtime = EndpointRuntime::default();
        let path = journal("generate-400-no-refusal");
        let config_map = active_spp_config();
        setup_journal(&path, &config_map);

        let endpoint = endpoint(9099);
        let result = confidential_generate_with(
            ConfidentialCall {
                request: &request(),
                journal_path: &path,
                endpoint: &endpoint,
                config: &config_map,
                runtime: &runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _| {
                Ok(EstablishedChannel {
                    verdict: verdict(),
                    stream: Box::new(RecordingChannel::with_status(
                        written_for_channel,
                        400,
                        "Bad Request",
                        "error",
                    )),
                })
            },
        );
        assert!(matches!(result, ConfidentialResult::Failed(_)));
        assert!(!path.join("health/brain.json").exists());

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn confidential_generate_config_changed_to_none_skips_recording() {
        let runtime = EndpointRuntime::default();
        let path = journal("generate-config-none");
        let config_map = active_spp_config();
        setup_journal(&path, &config_map);

        std::fs::write(
            path.join("config/journal.json"),
            serde_json::to_vec(&json!({"providers": {"active": {"provider": "none"}}})).unwrap(),
        )
        .unwrap();

        let endpoint = endpoint(9099);
        let req = request();

        let result = confidential_generate_with(
            ConfidentialCall {
                request: &req,
                journal_path: &path,
                endpoint: &endpoint,
                config: &config_map,
                runtime: &runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _| Err("gateway_unreachable"),
        );
        assert!(matches!(
            result,
            ConfidentialResult::AttestationFailed("gateway_unreachable")
        ));
        assert!(!path.join("health/brain.json").exists());

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn confidential_generate_missing_key_skips_recording() {
        let runtime = EndpointRuntime::default();
        let path = journal("generate-missing-key");
        let config_map = active_spp_config();
        std::fs::create_dir_all(path.join("config")).unwrap();
        std::fs::write(
            path.join("config/journal.json"),
            serde_json::to_vec(&config_map).unwrap(),
        )
        .unwrap();

        let endpoint = endpoint(9099);
        let req = request();

        let result = confidential_generate_with(
            ConfidentialCall {
                request: &req,
                journal_path: &path,
                endpoint: &endpoint,
                config: &config_map,
                runtime: &runtime,
                now: UNIX_EPOCH,
            },
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _| Err("gateway_unreachable"),
        );
        assert!(matches!(
            result,
            ConfidentialResult::AttestationFailed("gateway_unreachable")
        ));
        assert!(!path.join("secrets/fingerprint.key").exists());
        assert!(!path.join("health/brain.json").exists());

        let _ = std::fs::remove_dir_all(path);
    }
}

#[cfg(test)]
#[test]
fn confidential_generate_contract_400_is_provider_request_rejected() {
    let written = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let written_for_channel = written.clone();
    let runtime = EndpointRuntime::default();
    let path = tests::journal("confidential_400");
    let endpoint = tests::endpoint(1);
    let error_body = "unexpected contract error";
    let result = confidential_generate_with(
        ConfidentialCall {
            request: &tests::request(),
            journal_path: &path,
            endpoint: &endpoint,
            config: &serde_json::Map::new(),
            runtime: &runtime,
            now: std::time::UNIX_EPOCH,
        },
        |_| NvattestEnsureStatus::AlreadyInstalled,
        |_, _| {
            Ok(EstablishedChannel {
                verdict: tests::verdict(),
                stream: Box::new(tests::RecordingChannel::with_status(
                    written_for_channel,
                    400,
                    "Bad Request",
                    error_body,
                )),
            })
        },
    );
    let ConfidentialResult::Failed(failure) = result else {
        panic!("expected confidential failure");
    };
    assert_eq!(
        failure.reason_code,
        Some("provider_request_rejected".to_owned())
    );
    assert_eq!(
        failure.detail,
        Some("the local endpoint rejected the request".to_owned())
    );
    let _ = std::fs::remove_dir_all(path);
}
