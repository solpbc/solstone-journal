// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Confidential hosted-STT routing, attestation, and one-request transport.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use getrandom::fill as fill_random;
use serde_json::{Map, Value};
use solstone_core_journal_config::JournalConfigRead;
use solstone_core_local::{ByoEndpoint, LocalEndpointResolution, resolve_local_endpoint};
use solstone_core_observe_audio::{SAMPLE_RATE, audio_to_wav_bytes};
use solstone_core_spp_ratls::{
    AttestationFailureKind, AttestationSession, AttestationState, AttestationStateStore,
    AttestedIo, CompositeVerdict, NvattestEnsureStatus, RatlsEndpoint, classify_channel_failure,
    classify_nvattest_prerequisite, ensure_nvattest_installed, perform_fresh_reattest,
};

use crate::TranscribeError;
use crate::backend::parakeet_cpp::{ModelInfo, TranscriptionResponse, parse_verbose_json};

pub(crate) const CONFIDENTIAL_STT_MAX_AUDIO_SECONDS: f64 = 300.0;

const ATTESTED_CHANNEL_TIMEOUT: Duration = Duration::from_secs(120);
const TRANSCRIBE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESPONSE_HEADERS: usize = 16 * 1024;
const MAX_RESPONSE_BODY: usize = 8 * 1024 * 1024;
const MULTIPART_BOUNDARY_PREFIX: &str = "solstone-confidential-stt-";

/// Return the `services.confidential` object only when both levels are objects.
pub(crate) fn confidential_provenance(config: &JournalConfigRead) -> Option<Map<String, Value>> {
    config
        .config
        .as_ref()
        .and_then(|root| root.get("services"))
        .and_then(Value::as_object)
        .and_then(|services| services.get("confidential"))
        .and_then(Value::as_object)
        .cloned()
}

/// Whether configuration resolves to a credentialed confidential BYO endpoint.
pub(crate) fn confidential_channel_plausible(config: &JournalConfigRead) -> bool {
    matches!(
        config.config.as_ref().map(resolve_local_endpoint),
        Some(LocalEndpointResolution::Byo(ByoEndpoint {
            is_confidential: true,
            credential: Some(_),
            ..
        }))
    )
}

/// Whether a registered STT backend keeps raw audio on this machine.
pub(crate) fn is_local_backend(name: &str) -> bool {
    matches!(name, "parakeet" | "parakeet-cpp")
}

/// Refuse remote STT before raw audio can leave an active confidential lane.
pub(crate) fn refuse_confidential_egress(
    config: &JournalConfigRead,
    backend: &str,
    confidential_audio_enabled: bool,
) -> Result<(), TranscribeError> {
    if confidential_provenance(config).is_none() {
        return if backend == "confidential" {
            Err(deferred(
                "confidential_lane_inactive",
                "the confidential lane is no longer active",
            ))
        } else {
            Ok(())
        };
    }

    if is_local_backend(backend) {
        return Ok(());
    }
    if backend == "confidential" {
        return if confidential_audio_enabled {
            Ok(())
        } else {
            Err(deferred(
                "confidential_audio_disabled",
                "confidential audio handling is disabled",
            ))
        };
    }

    Err(deferred(
        "confidential_egress_blocked",
        format!("confidential lane blocks STT backend {backend:?}; raw audio must stay local"),
    ))
}

/// Send one hosted transcription over a freshly attested channel.
pub(crate) fn transcribe(
    audio: &[f32],
    journal_path: &Path,
    config: &JournalConfigRead,
    state: &AttestationStateStore,
) -> Result<(TranscriptionResponse, ModelInfo), TranscribeError> {
    let endpoint = confidential_endpoint(config)?;
    if endpoint.credential.is_none() {
        return Err(deferred(
            "hosted_transcribe_unreachable",
            "the confidential endpoint has no credential",
        ));
    }
    let wav = audio_to_wav_bytes(audio, SAMPLE_RATE)
        .map_err(|error| deferred("confidential_audio_encode_failed", error.to_string()))?;
    let now = SystemTime::now();
    if attestation_reason(&state.get_attestation_state(), now) == Some("attestation_stale") {
        return Err(deferred(
            "attestation_stale",
            "the previous attestation session is stale",
        ));
    }
    let nvattest_dir = resolve_nvattest_dir(
        config.config.as_ref().expect("endpoint requires config"),
        journal_path,
    );
    let mut channel = perform_fresh_reattest(
        state,
        &endpoint.base_url,
        &nvattest_dir,
        ATTESTED_CHANNEL_TIMEOUT,
        ensure_nvattest_installed,
    )
    .map_err(|_| deferred_from_attestation(state, now))?;
    let response = send_multipart_request(
        &mut channel.stream,
        &channel.host,
        endpoint.credential.as_deref(),
        &wav,
        TRANSCRIBE_TIMEOUT,
    )
    .map_err(hosted_transcribe_transport_error)?;
    hosted_response(response)
}

fn confidential_endpoint(config: &JournalConfigRead) -> Result<ByoEndpoint, TranscribeError> {
    match config.config.as_ref().map(resolve_local_endpoint) {
        Some(LocalEndpointResolution::Byo(endpoint)) if endpoint.is_confidential => Ok(endpoint),
        _ => Err(deferred(
            "confidential_lane_inactive",
            "the confidential lane has no confidential BYO endpoint",
        )),
    }
}

struct ConfidentialCall<'a> {
    wav: &'a [u8],
    journal_path: &'a Path,
    endpoint: &'a ByoEndpoint,
    config: &'a Map<String, Value>,
    state: &'a AttestationStateStore,
    now: SystemTime,
    timeout: Duration,
}

struct EstablishedChannel {
    verdict: CompositeVerdict,
    stream: Box<dyn AttestedIo>,
}

fn confidential_transcribe_with<R, E>(
    call: ConfidentialCall<'_>,
    readiness: R,
    establish: E,
) -> Result<(TranscriptionResponse, ModelInfo), TranscribeError>
where
    R: FnOnce(&Path) -> NvattestEnsureStatus,
    E: FnOnce(&RatlsEndpoint, &Path) -> Result<EstablishedChannel, &'static str>,
{
    let ConfidentialCall {
        wav,
        journal_path,
        endpoint,
        config,
        state,
        now,
        timeout,
    } = call;
    if attestation_reason(&state.get_attestation_state(), now) == Some("attestation_stale") {
        return Err(deferred(
            "attestation_stale",
            "the previous attestation session is stale",
        ));
    }

    let nvattest_dir = resolve_nvattest_dir(config, journal_path);
    if let Some(failure) = classify_nvattest_prerequisite(readiness(&nvattest_dir)) {
        state.record_attestation_failed(failure.kind, failure.reason_code);
        return Err(deferred_from_attestation(state, now));
    }

    let target = match ratls_target(&endpoint.base_url) {
        Some(target) => target,
        None => {
            state.record_attestation_failed(
                classify_channel_failure("tls_handshake_failed"),
                "tls_handshake_failed",
            );
            return Err(deferred_from_attestation(state, now));
        }
    };
    let EstablishedChannel {
        verdict,
        mut stream,
    } = match establish(&target.endpoint, &nvattest_dir) {
        Ok(channel) => channel,
        Err(reason_code) => {
            state.record_attestation_failed(classify_channel_failure(reason_code), reason_code);
            return Err(deferred_from_attestation(state, now));
        }
    };
    state.record_attestation_verified(AttestationSession {
        verdict,
        started_at: now,
        tpm_heartbeat_at: now,
        gpu_reattest_at: now,
    });

    let response = send_multipart_request(
        &mut *stream,
        &target.host,
        endpoint.credential.as_deref(),
        wav,
        timeout,
    )
    .map_err(hosted_transcribe_transport_error)?;
    hosted_response(response)
}

fn deferred_from_attestation(state: &AttestationStateStore, now: SystemTime) -> TranscribeError {
    let reason = attestation_reason(&state.get_attestation_state(), now)
        .unwrap_or("attestation_not_yet_verified");
    deferred(reason, "the confidential attestation channel is not ready")
}

fn attestation_reason(state: &AttestationState, now: SystemTime) -> Option<&'static str> {
    match state.failure.as_ref().map(|failure| failure.kind) {
        Some(AttestationFailureKind::Unreachable) => Some("attestation_unreachable"),
        Some(AttestationFailureKind::Failed) => Some("attestation_failed"),
        None => match state.session.as_ref() {
            None => Some("attestation_not_yet_verified"),
            Some(session) if session.status(now) == "stale" => Some("attestation_stale"),
            Some(_) => None,
        },
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

#[derive(Debug)]
pub(crate) struct HttpResponse {
    pub(crate) status: u16,
    pub(crate) body: Vec<u8>,
}

#[derive(Debug)]
pub(crate) enum HttpError {
    Entropy(getrandom::Error),
    Transport(std::io::Error),
    Protocol(&'static str),
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Entropy(error) => error.fmt(formatter),
            Self::Transport(error) => error.fmt(formatter),
            Self::Protocol(reason) => formatter.write_str(reason),
        }
    }
}

pub(crate) fn send_multipart_request(
    stream: &mut dyn AttestedIo,
    host: &str,
    bearer: Option<&str>,
    wav: &[u8],
    timeout: Duration,
) -> Result<HttpResponse, HttpError> {
    let (boundary, body) = multipart_body(wav)?;
    let mut request = format!(
        "POST /v1/audio/transcriptions HTTP/1.1\r\nHost: {host}\r\nContent-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(bearer) = bearer {
        request.push_str("Authorization: Bearer ");
        request.push_str(bearer);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    retry_interrupted(|| stream.set_io_timeout(Some(timeout))).map_err(HttpError::Transport)?;
    write_all_retry_interrupted(stream, request.as_bytes()).map_err(HttpError::Transport)?;
    write_all_retry_interrupted(stream, &body).map_err(HttpError::Transport)?;
    retry_interrupted(|| stream.flush()).map_err(HttpError::Transport)?;
    recv_bounded_http_response(stream)
}

fn multipart_body(wav: &[u8]) -> Result<(String, Vec<u8>), HttpError> {
    let boundary = multipart_boundary()?;
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: audio/wav\r\n\r\n");
    body.extend_from_slice(wav);
    body.extend_from_slice(b"\r\n");
    push_text_part_header(&mut body, &boundary, "response_format");
    body.extend_from_slice(b"verbose_json\r\n");
    push_text_part_header(&mut body, &boundary, "timestamp_granularities[]=word");
    body.extend_from_slice(b"word\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok((boundary, body))
}

fn multipart_boundary() -> Result<String, HttpError> {
    let mut bytes = [0_u8; 16];
    fill_random(&mut bytes).map_err(HttpError::Entropy)?;
    Ok(format!(
        "{MULTIPART_BOUNDARY_PREFIX}{}",
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

fn push_text_part_header(body: &mut Vec<u8>, boundary: &str, name: &str) {
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"");
    body.extend_from_slice(name.as_bytes());
    body.extend_from_slice(b"\"\r\n\r\n");
}

fn recv_bounded_http_response(stream: &mut dyn AttestedIo) -> Result<HttpResponse, HttpError> {
    let marker = b"\r\n\r\n";
    let mut data = Vec::new();
    let header_end = loop {
        if let Some(position) = data
            .windows(marker.len())
            .position(|window| window == marker)
        {
            break position;
        }
        if data.len() >= MAX_RESPONSE_HEADERS {
            return Err(HttpError::Protocol("response_headers_too_large"));
        }
        let mut buffer = [0_u8; 4096];
        let read_len = buffer.len().min(MAX_RESPONSE_HEADERS - data.len());
        let count = retry_interrupted(|| stream.read(&mut buffer[..read_len]))
            .map_err(HttpError::Transport)?;
        if count == 0 {
            return Err(HttpError::Protocol("response_eof"));
        }
        data.extend_from_slice(&buffer[..count]);
    };
    let lines = data[..header_end]
        .split(|byte| *byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .collect::<Vec<_>>();
    let status = lines
        .first()
        .and_then(|line| parse_status(line).ok())
        .ok_or(HttpError::Protocol("response_status_invalid"))?;
    let mut content_length = None;
    for line in lines.iter().skip(1) {
        let Some(colon) = line.iter().position(|byte| *byte == b':') else {
            return Err(HttpError::Protocol("response_header_invalid"));
        };
        if line[..colon].eq_ignore_ascii_case(b"content-length") {
            if content_length.is_some() {
                return Err(HttpError::Protocol("response_content_length_duplicate"));
            }
            content_length = std::str::from_utf8(&line[colon + 1..])
                .ok()
                .and_then(|value| value.trim().parse::<usize>().ok());
        }
    }
    let length = content_length
        .filter(|length| *length <= MAX_RESPONSE_BODY)
        .ok_or(HttpError::Protocol("response_content_length_invalid"))?;
    let mut body = data[header_end + marker.len()..].to_vec();
    while body.len() < length {
        let mut buffer = [0_u8; 65536];
        let remaining = (length - body.len()).min(buffer.len());
        let count = retry_interrupted(|| stream.read(&mut buffer[..remaining]))
            .map_err(HttpError::Transport)?;
        if count == 0 {
            return Err(HttpError::Protocol("response_body_eof"));
        }
        body.extend_from_slice(&buffer[..count]);
    }
    Ok(HttpResponse {
        status,
        body: body[..length].to_vec(),
    })
}

fn write_all_retry_interrupted(
    stream: &mut dyn AttestedIo,
    mut bytes: &[u8],
) -> std::io::Result<()> {
    while !bytes.is_empty() {
        let count = retry_interrupted(|| stream.write(bytes))?;
        if count == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::WriteZero));
        }
        bytes = &bytes[count..];
    }
    Ok(())
}

fn retry_interrupted<T>(mut operation: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    loop {
        match operation() {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
}

fn parse_status(line: &[u8]) -> Result<u16, ()> {
    let mut fields = line.split(|byte| *byte == b' ');
    let version = fields.next().ok_or(())?;
    let status = fields.next().ok_or(())?;
    version
        .starts_with(b"HTTP/")
        .then_some(())
        .and_then(|_| std::str::from_utf8(status).ok()?.parse::<u16>().ok())
        .ok_or(())
}

fn hosted_response(
    response: HttpResponse,
) -> Result<(TranscriptionResponse, ModelInfo), TranscribeError> {
    match response.status {
        400 | 413 => Err(deferred(
            "hosted_transcribe_rejected",
            format!("hosted STT returned HTTP {}", response.status),
        )),
        429 | 503 | 504 => Err(deferred(
            "hosted_transcribe_backpressure",
            format!("hosted STT returned HTTP {}", response.status),
        )),
        200 => {
            let body = String::from_utf8_lossy(&response.body);
            let transcription = parse_verbose_json(&body).map_err(|_| {
                deferred(
                    "hosted_transcribe_contract_failed",
                    "hosted STT response violated the verbose JSON contract",
                )
            })?;
            Ok((
                transcription,
                ModelInfo {
                    model: "confidential".to_owned(),
                    device: "confidential".to_owned(),
                    compute_type: "".to_owned(),
                },
            ))
        }
        status => Err(deferred(
            "hosted_transcribe_unexpected_status",
            format!("hosted STT returned HTTP {status}"),
        )),
    }
}

pub(crate) fn hosted_transcribe_transport_error(error: HttpError) -> TranscribeError {
    deferred("hosted_transcribe_unreachable", error.to_string())
}

fn deferred(reason: impl Into<String>, detail: impl Into<String>) -> TranscribeError {
    TranscribeError::ConfidentialDeferred {
        reason: reason.into(),
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, UNIX_EPOCH};

    use serde_json::{Value, json};
    use solstone_core_journal_config::JournalConfigRead;
    use solstone_core_local::ByoEndpoint;
    use solstone_core_spp_attest::{
        nvgpu::claims::GpuAppraisal,
        snp::{CpuAppraisal, CpuTcb, TcbVersion},
    };
    use solstone_core_spp_ratls::{
        AttestationFailureKind, AttestationSession, AttestationState, AttestationStateStore,
        CompositeVerdict, GPU_REATTEST_INTERVAL, NvattestEnsureStatus, SESSION_CAP,
        TPM_HEARTBEAT_INTERVAL,
    };

    use super::{
        CONFIDENTIAL_STT_MAX_AUDIO_SECONDS, ConfidentialCall, HttpResponse, attestation_reason,
        confidential_channel_plausible, confidential_provenance, confidential_transcribe_with,
        hosted_response, multipart_boundary, refuse_confidential_egress,
    };
    use crate::TranscribeError;

    const VALID: &str =
        r#"{"words":[{"word":"hello","start":0.0,"end":1.0,"conf":0.9}],"text":"hello"}"#;

    #[test]
    fn plausible_channel_requires_a_confidential_byo_endpoint_and_credential() {
        assert!(confidential_channel_plausible(&active_config()));
        assert!(!confidential_channel_plausible(&config(json!({
            "services":{"confidential":{}},
            "providers":{"local":{"credential":"secret"}}
        }))));
        assert!(!confidential_channel_plausible(&config(json!({
            "services":{"confidential":{}},
            "providers":{"local":{"endpoint_url":"https://endpoint","served_model_id":"served","credential":""}}
        }))));
    }

    #[test]
    fn active_lane_retains_local_and_enabled_confidential_egress() {
        assert!(refuse_confidential_egress(&active_config(), "parakeet", false).is_ok());
        assert!(refuse_confidential_egress(&active_config(), "parakeet-cpp", false).is_ok());
        assert!(refuse_confidential_egress(&active_config(), "confidential", true).is_ok());
    }

    #[test]
    fn provenance_requires_object_levels() {
        assert_eq!(
            confidential_provenance(&config(
                json!({"services":{"confidential":{"device":"abc"}}})
            )),
            Some(serde_json::from_value(json!({"device":"abc"})).unwrap())
        );
        assert_eq!(confidential_provenance(&config(json!({}))), None);
        assert_eq!(
            confidential_provenance(&config(json!({"services":{"confidential":true}}))),
            None
        );
    }

    #[test]
    fn egress_gate_refuses_inactive_and_disabled_lanes_before_dispatch() {
        assert_deferred_reason(
            refuse_confidential_egress(&config(json!({})), "confidential", true).unwrap_err(),
            "confidential_lane_inactive",
        );
        assert_deferred_reason(
            refuse_confidential_egress(&active_config(), "confidential", false).unwrap_err(),
            "confidential_audio_disabled",
        );
    }

    #[test]
    fn hosted_status_and_contract_failures_are_all_deferred() {
        for (status, body, expected) in [
            (400, "", "hosted_transcribe_rejected"),
            (413, "", "hosted_transcribe_rejected"),
            (429, "", "hosted_transcribe_backpressure"),
            (503, "", "hosted_transcribe_backpressure"),
            (504, "", "hosted_transcribe_backpressure"),
            (500, "", "hosted_transcribe_unexpected_status"),
            (200, "not-json", "hosted_transcribe_contract_failed"),
            (
                200,
                r#"{"words":[],"text":"hello"}"#,
                "hosted_transcribe_contract_failed",
            ),
        ] {
            assert_deferred_reason(
                hosted_response(HttpResponse {
                    status,
                    body: body.as_bytes().to_vec(),
                })
                .unwrap_err(),
                expected,
            );
        }
        let (_, metadata) = hosted_response(HttpResponse {
            status: 200,
            body: VALID.as_bytes().to_vec(),
        })
        .unwrap();
        assert_eq!(metadata.model, "confidential");
        assert_eq!(metadata.device, "confidential");
    }

    #[test]
    fn multipart_boundaries_are_fresh_per_request() {
        let first = multipart_boundary().unwrap();
        let second = multipart_boundary().unwrap();

        assert!(first.starts_with(super::MULTIPART_BOUNDARY_PREFIX));
        assert!(second.starts_with(super::MULTIPART_BOUNDARY_PREFIX));
        assert_ne!(first, second);
    }

    #[test]
    fn readiness_failure_refuses_before_an_endpoint_request() {
        let store = AttestationStateStore::new();
        let readiness_attempts = AtomicUsize::new(0);
        let channel_attempts = AtomicUsize::new(0);
        let unreachable_endpoint = endpoint("https://127.0.0.1:9");
        let config = active_config().config.unwrap();
        let error = confidential_transcribe_with(
            ConfidentialCall {
                wav: b"WAV",
                journal_path: Path::new("/journal"),
                endpoint: &unreachable_endpoint,
                config: &config,
                state: &store,
                now: UNIX_EPOCH,
                timeout: Duration::from_millis(10),
            },
            |_| {
                readiness_attempts.fetch_add(1, Ordering::SeqCst);
                NvattestEnsureStatus::InstallInFlight
            },
            |_, _| {
                channel_attempts.fetch_add(1, Ordering::SeqCst);
                Err("gateway_unreachable")
            },
        )
        .unwrap_err();
        assert_deferred_reason(error, "attestation_unreachable");
        assert_eq!(readiness_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(channel_attempts.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn unavailable_nvattest_records_its_cause_before_an_endpoint_request() {
        let store = AttestationStateStore::new();
        let channel_attempts = AtomicUsize::new(0);
        let endpoint = endpoint("https://127.0.0.1:9");
        let config = active_config().config.unwrap();
        let error = confidential_transcribe_with(
            ConfidentialCall {
                wav: b"WAV",
                journal_path: Path::new("/journal"),
                endpoint: &endpoint,
                config: &config,
                state: &store,
                now: UNIX_EPOCH,
                timeout: Duration::from_millis(10),
            },
            |_| NvattestEnsureStatus::Unavailable,
            |_, _| {
                channel_attempts.fetch_add(1, Ordering::SeqCst);
                Err("gateway_unreachable")
            },
        )
        .unwrap_err();
        assert_deferred_reason(error, "attestation_failed");
        assert_eq!(channel_attempts.load(Ordering::SeqCst), 0);
        assert_eq!(
            store
                .get_attestation_state()
                .failure
                .as_ref()
                .map(|failure| failure.reason_code),
            Some("nvattest_unavailable")
        );
    }

    #[test]
    fn invalid_target_failure_refuses_before_a_channel_attempt() {
        let store = AttestationStateStore::new();
        let channel_attempts = AtomicUsize::new(0);
        let invalid_target_endpoint = endpoint("not-a-url");
        let config = active_config().config.unwrap();
        let error = confidential_transcribe_with(
            ConfidentialCall {
                wav: b"WAV",
                journal_path: Path::new("/journal"),
                endpoint: &invalid_target_endpoint,
                config: &config,
                state: &store,
                now: UNIX_EPOCH,
                timeout: Duration::from_millis(10),
            },
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _| {
                channel_attempts.fetch_add(1, Ordering::SeqCst);
                panic!("invalid target must not establish a channel")
            },
        )
        .unwrap_err();
        assert_deferred_reason(error, "attestation_failed");
        assert_eq!(channel_attempts.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn stale_session_refuses_before_readiness_or_endpoint_request() {
        let store = AttestationStateStore::new();
        store.record_attestation_verified(AttestationSession {
            verdict: verdict(),
            started_at: UNIX_EPOCH,
            tpm_heartbeat_at: UNIX_EPOCH,
            gpu_reattest_at: UNIX_EPOCH,
        });
        let endpoint = endpoint("https://127.0.0.1:9");
        let config = active_config().config.unwrap();
        let readiness_attempts = AtomicUsize::new(0);
        let channel_attempts = AtomicUsize::new(0);

        let error = confidential_transcribe_with(
            ConfidentialCall {
                wav: b"WAV",
                journal_path: Path::new("/journal"),
                endpoint: &endpoint,
                config: &config,
                state: &store,
                now: UNIX_EPOCH + Duration::from_secs(600),
                timeout: Duration::from_millis(10),
            },
            |_| {
                readiness_attempts.fetch_add(1, Ordering::SeqCst);
                NvattestEnsureStatus::AlreadyInstalled
            },
            |_, _| {
                channel_attempts.fetch_add(1, Ordering::SeqCst);
                Err("gateway_unreachable")
            },
        )
        .unwrap_err();

        assert_deferred_reason(error, "attestation_stale");
        assert_eq!(readiness_attempts.load(Ordering::SeqCst), 0);
        assert_eq!(channel_attempts.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn attestation_reason_mapper_covers_the_initial_and_failure_states() {
        let now = UNIX_EPOCH + Duration::from_secs(10_000);
        assert_eq!(
            attestation_reason(&AttestationState::default(), now),
            Some("attestation_not_yet_verified")
        );
        let missing = AttestationState {
            session: None,
            failure: None,
            last_verified: Some(verified_session(now)),
        };
        assert_eq!(
            attestation_reason(&missing, now),
            Some("attestation_not_yet_verified")
        );
        assert_eq!(
            attestation_reason(
                &AttestationState {
                    session: Some(verified_session(now)),
                    ..Default::default()
                },
                now
            ),
            None
        );
        let unreachable = AttestationState {
            failure: Some(solstone_core_spp_ratls::AttestationFailure {
                kind: AttestationFailureKind::Unreachable,
                reason_code: "gateway_unreachable",
            }),
            ..Default::default()
        };
        assert_eq!(
            attestation_reason(&unreachable, now),
            Some("attestation_unreachable")
        );
        let failed = AttestationState {
            failure: Some(solstone_core_spp_ratls::AttestationFailure {
                kind: AttestationFailureKind::Failed,
                reason_code: "tls_handshake_failed",
            }),
            ..Default::default()
        };
        assert_eq!(attestation_reason(&failed, now), Some("attestation_failed"));

        let second = Duration::from_secs(1);
        let expired = [
            AttestationSession {
                tpm_heartbeat_at: now - TPM_HEARTBEAT_INTERVAL,
                ..verified_session(now)
            },
            AttestationSession {
                gpu_reattest_at: now - GPU_REATTEST_INTERVAL,
                ..verified_session(now)
            },
            AttestationSession {
                started_at: now - SESSION_CAP,
                ..verified_session(now)
            },
        ];
        for session in expired {
            let state = AttestationState {
                session: Some(session),
                ..Default::default()
            };
            assert_eq!(attestation_reason(&state, now - second), None);
            assert_eq!(attestation_reason(&state, now), Some("attestation_stale"));
            assert_eq!(
                attestation_reason(&state, now + second),
                Some("attestation_stale")
            );
        }
        assert_eq!(CONFIDENTIAL_STT_MAX_AUDIO_SECONDS, 300.0);
    }

    #[test]
    fn successful_hosted_response_uses_fixed_model_metadata() {
        let (_, metadata) = hosted_response(HttpResponse {
            status: 200,
            body: VALID.as_bytes().to_vec(),
        })
        .unwrap();

        assert_eq!(metadata.model, "confidential");
        assert_eq!(metadata.device, "confidential");
    }

    fn active_config() -> JournalConfigRead {
        config(json!({
            "services":{"confidential":{"device":"abc"}},
            "providers":{"local":{"endpoint_url":"https://endpoint","served_model_id":"served","credential":"secret"}}
        }))
    }

    fn endpoint(base_url: &str) -> ByoEndpoint {
        ByoEndpoint {
            base_url: base_url.to_owned(),
            served_model_id: "served".to_owned(),
            credential: Some("secret".to_owned()),
            parallel_slots: None,
            is_confidential: true,
            is_bundled: false,
        }
    }

    fn verified_session(now: std::time::SystemTime) -> AttestationSession {
        AttestationSession {
            verdict: verdict(),
            started_at: now,
            tpm_heartbeat_at: now,
            gpu_reattest_at: now,
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

    fn config(value: Value) -> JournalConfigRead {
        JournalConfigRead {
            present: true,
            sha256: None,
            config: Some(value.as_object().unwrap().clone()),
        }
    }

    fn assert_deferred_reason(error: TranscribeError, expected_reason: &str) {
        assert_eq!(error.exit_code(), 69);
        let TranscribeError::ConfidentialDeferred { reason, .. } = error else {
            panic!("expected confidential deferral");
        };
        assert_eq!(reason, expected_reason);
    }
}
