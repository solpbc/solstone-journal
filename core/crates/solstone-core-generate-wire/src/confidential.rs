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
    EndpointFailure, EndpointGenerated, EndpointResult, EndpointRuntime, EndpointTransport,
    EndpointTransportError, endpoint_generate_with,
};

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
        request,
        journal_path,
        endpoint,
        config,
        runtime,
        SystemTime::now(),
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

fn confidential_generate_with<R, E>(
    request: &GenerateRequest,
    journal_path: &Path,
    endpoint: &ByoEndpoint,
    config: &Map<String, Value>,
    runtime: &EndpointRuntime,
    now: SystemTime,
    readiness: R,
    establish: E,
) -> ConfidentialResult
where
    R: FnOnce(&Path) -> NvattestEnsureStatus,
    E: FnOnce(&RatlsEndpoint, &Path) -> Result<EstablishedChannel, &'static str>,
{
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
        // Model discovery is optional in endpoint_generate_with; it must not
        // issue an unaudited second request over the one-shot channel.
        Err(EndpointTransportError::Other)
    }

    fn post_json(
        &mut self,
        _base_url: &str,
        path: &str,
        body: &Value,
        credential: Option<&str>,
        _timeout: Duration,
    ) -> Result<HttpResponse, EndpointTransportError> {
        let body = serde_json::to_vec(body).map_err(|_| EndpointTransportError::Other)?;
        send_json_request(&mut *self.stream, &self.host, path, credential, &body)
            .map(|response| HttpResponse {
                status: response.status,
                body: String::from_utf8_lossy(&response.body).into_owned(),
            })
            .map_err(|error| match error {
                AttestedHttpError::Transport(error) if error.kind() == io::ErrorKind::TimedOut => {
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
            &request(),
            &path,
            &endpoint(port),
            &Map::new(),
            &runtime,
            UNIX_EPOCH,
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
    fn readiness_failure_refuses_without_attempting_a_channel() {
        let runtime = EndpointRuntime::default();
        let attempts = AtomicUsize::new(0);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind endpoint");
        let path = journal("not-verified");
        let result = confidential_generate_with(
            &request(),
            &path,
            &endpoint(listener.local_addr().expect("address").port()),
            &Map::new(),
            &runtime,
            UNIX_EPOCH,
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
            &request(),
            &path,
            &endpoint(listener.local_addr().expect("address").port()),
            &Map::new(),
            &runtime,
            UNIX_EPOCH,
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
            &request(),
            &path,
            &endpoint(listener.local_addr().expect("address").port()),
            &Map::new(),
            &runtime,
            UNIX_EPOCH + Duration::from_secs(10 * 60),
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
