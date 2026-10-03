// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Confidential hosted-STT routing, attestation, and one-request transport.

use std::path::Path;
use std::time::{Duration, SystemTime};

use getrandom::fill as fill_random;
use serde_json::{Map, Value};
use solstone_core_journal_config::JournalConfigRead;
use solstone_core_local::{ByoEndpoint, LocalEndpointResolution, resolve_local_endpoint};
use solstone_core_observe_audio::{SAMPLE_RATE, audio_to_wav_bytes};
use solstone_core_spp_ratls::{
    AttestationFailureKind, AttestationState, AttestationStateStore, AttestedIo,
    NvattestEnsureStatus, ensure_nvattest_installed, establish_fresh_production_channel,
};

use crate::TranscribeError;
use crate::backend::parakeet_cpp::{
    ModelInfo, TranscriptionResponse, WordContractError, parse_verbose_json,
};

/// The confidential ASR shim bounds a request by BYTES, not by duration.
///
/// `asr_shim.py` on the engine: `MAX_REQUEST_BYTES = 11 * 1024 * 1024`, commented
/// "canonical 300s WAV is ~9.6 MB; allow validator tolerance + multipart framing".
/// ⚠ There is **no** server-side duration limit. This client used to mirror that
/// budget as a flat 300-second cap, which is a proxy rather than the contract -- and
/// it refused 301-305 s recordings the server would have accepted, because a 302 s
/// PCM16 WAV is about 9.7 MB with ~1.3 MB to spare.
///
/// Measured on an owner's journal 2026-09-01: 60 `confidential_audio_too_long`
/// refusals, all just over the proxy and all comfortably inside the real budget.
pub(crate) const CONFIDENTIAL_STT_MAX_REQUEST_BYTES: usize = 11 * 1024 * 1024;

/// Headroom for the multipart envelope wrapped around the WAV payload.
const CONFIDENTIAL_STT_ENVELOPE_BYTES: usize = 64 * 1024;

/// The request size a PCM16 WAV of `samples` will occupy, envelope included.
pub(crate) fn confidential_request_bytes(samples: usize) -> usize {
    solstone_core_observe_audio::wav_bytes_for_samples(samples)
        .saturating_add(CONFIDENTIAL_STT_ENVELOPE_BYTES)
}

/// Whether that request fits the shim's budget.
pub(crate) fn confidential_request_fits(samples: usize) -> bool {
    confidential_request_bytes(samples) <= CONFIDENTIAL_STT_MAX_REQUEST_BYTES
}

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

pub(crate) fn transcribe_with<R, E, S>(
    audio: &[f32],
    journal_path: &Path,
    config: &JournalConfigRead,
    state: &AttestationStateStore,
    readiness: R,
    establish: E,
) -> Result<(TranscriptionResponse, ModelInfo), TranscribeError>
where
    R: FnOnce(&Path) -> NvattestEnsureStatus,
    S: AttestedIo,
    E: FnOnce(
        &solstone_core_spp_ratls::RatlsEndpoint,
        &Path,
        Duration,
    ) -> Result<(solstone_core_spp_ratls::CompositeVerdict, S), &'static str>,
{
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
    let nvattest_dir =
        solstone_core_spp_ratls::resolve_nvattest_dir(config.config.as_ref(), journal_path);
    let mut channel = solstone_core_spp_ratls::perform_fresh_reattest_with(
        state,
        &endpoint.base_url,
        &nvattest_dir,
        ATTESTED_CHANNEL_TIMEOUT,
        readiness,
        establish,
    )
    .map_err(|failure| {
        if let Some(held_config) = config.config.as_ref() {
            solstone_core_brain::record_confidential_attestation_refusal(
                journal_path,
                held_config,
                failure.reason_code,
            );
        }
        if let Err(error) = solstone_core_brain::record_transcription_verification(
            journal_path,
            failure.reason_code,
            &endpoint.base_url,
        ) {
            log::warn!(
                "confidential transcription verification was not recorded for {}: {error}",
                journal_path.display()
            );
        }
        deferred_from_attestation(state, now)
    })?;
    if let Err(error) = solstone_core_brain::clear_transcription_verification(journal_path) {
        log::warn!(
            "confidential transcription verification was not cleared for {}: {error}",
            journal_path.display()
        );
    }
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

/// Send one hosted transcription over a freshly attested channel.
pub(crate) fn transcribe(
    audio: &[f32],
    journal_path: &Path,
    config: &JournalConfigRead,
    state: &AttestationStateStore,
) -> Result<(TranscriptionResponse, ModelInfo), TranscribeError> {
    transcribe_with(
        audio,
        journal_path,
        config,
        state,
        ensure_nvattest_installed,
        establish_fresh_production_channel,
    )
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
    let response = solstone_core_spp_ratls::ratls::http::recv_bounded_http_response(
        stream,
        MAX_RESPONSE_HEADERS,
        MAX_RESPONSE_BODY,
    )
    .map_err(|err| match err {
        solstone_core_spp_ratls::ratls::http::BoundedHttpError::Transport(e) => {
            HttpError::Transport(e)
        }
        solstone_core_spp_ratls::ratls::http::BoundedHttpError::Protocol(p) => {
            HttpError::Protocol(p)
        }
    })?;
    let status = solstone_core_spp_ratls::ratls::http::response_status(&response.status_line)
        .map_err(HttpError::Protocol)?;
    Ok(HttpResponse {
        status,
        body: response.body,
    })
}

fn write_all_retry_interrupted(stream: &mut dyn AttestedIo, bytes: &[u8]) -> std::io::Result<()> {
    solstone_core_spp_ratls::ratls::http::write_all_retry_interrupted(stream, bytes)
}

fn retry_interrupted<T>(operation: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    solstone_core_spp_ratls::ratls::http::retry_interrupted(operation)
}

/// The structural name of a verbose-JSON contract violation.
///
/// ⛔ Never includes `InvalidJson`'s payload: that is the raw response body, which in
/// real use carries the owner's speech.
fn word_contract_detail(error: &WordContractError) -> String {
    match error {
        WordContractError::InvalidNumber { key, found } => {
            format!("word `{key}` was not a finite number (found {found})")
        }
        other => word_contract_kind(other).to_owned(),
    }
}

fn word_contract_kind(error: &WordContractError) -> &'static str {
    match error {
        WordContractError::InvalidJson(_) => "response was not JSON",
        WordContractError::NotObject => "response was not an object",
        WordContractError::MissingWords => "response had no words array",
        WordContractError::TextWithoutTimings => "response had text but no word timings",
        WordContractError::WordNotObject => "a word entry was not an object",
        WordContractError::MissingKey(key) => match *key {
            "word" => "a word entry was missing `word`",
            "start" => "a word entry was missing `start`",
            "end" => "a word entry was missing `end`",
            _ => "a word entry was missing a required key",
        },
        WordContractError::BlankWord => "a word entry was blank",
        // ⚠ Rendered by the caller with the key and type, so it is not folded here.
        WordContractError::InvalidNumber { .. } => "a word number was not finite",
    }
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
            let transcription = parse_verbose_json(&body).map_err(|error| {
                // ⚠ Name WHICH clause of the contract failed. This used to discard the
                // error, so 20 refusals on an owner's journal said only "violated
                // the verbose JSON contract" -- true, unactionable, and indistinguishable
                // from a transport problem. The variant is structural; ⛔ the InvalidJson
                // payload is deliberately not included, because it is the response body
                // and in real use that is owner speech.
                deferred(
                    "hosted_transcribe_contract_failed",
                    format!(
                        "hosted STT response violated the verbose JSON contract: {}",
                        word_contract_detail(&error)
                    ),
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
    use solstone_core_spp_attest::{
        nvgpu::claims::GpuAppraisal,
        snp::{CpuAppraisal, CpuTcb, TcbVersion},
    };
    use solstone_core_spp_ratls::{
        AttestationFailureKind, AttestationSession, AttestationState, AttestationStateStore,
        AttestedIo, CompositeVerdict, GPU_REATTEST_INTERVAL, NvattestEnsureStatus, SESSION_CAP,
        TPM_HEARTBEAT_INTERVAL,
    };

    use super::{
        CONFIDENTIAL_STT_MAX_REQUEST_BYTES, HttpResponse, attestation_reason,
        confidential_channel_plausible, confidential_provenance, hosted_response,
        multipart_boundary, refuse_confidential_egress,
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
        assert_deferred_reason(
            refuse_confidential_egress(&active_config(), "remote", true).unwrap_err(),
            "confidential_egress_blocked",
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
            (302, "", "hosted_transcribe_unexpected_status"),
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
        let active = active_config();
        let audio = [0.0_f32; 160];
        let error = super::transcribe_with(
            &audio,
            Path::new("/journal"),
            &active,
            &store,
            |_| {
                readiness_attempts.fetch_add(1, Ordering::SeqCst);
                NvattestEnsureStatus::InstallInFlight
            },
            |_, _, _| -> Result<(CompositeVerdict, MockAttestedStream), &'static str> {
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
        let active = active_config();
        let audio = [0.0_f32; 160];
        let error = super::transcribe_with(
            &audio,
            Path::new("/journal"),
            &active,
            &store,
            |_| NvattestEnsureStatus::Unavailable,
            |_, _, _| -> Result<(CompositeVerdict, MockAttestedStream), &'static str> {
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
        let invalid_config = config(json!({
            "services":{"confidential":{"device":"abc"}},
            "providers":{"local":{"endpoint_url":"not-a-url","served_model_id":"served","credential":"secret"}}
        }));
        let audio = [0.0_f32; 160];
        let error = super::transcribe_with(
            &audio,
            Path::new("/journal"),
            &invalid_config,
            &store,
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _, _| -> Result<(CompositeVerdict, MockAttestedStream), &'static str> {
                channel_attempts.fetch_add(1, Ordering::SeqCst);
                panic!("invalid target must not establish a channel")
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
            Some("tls_handshake_failed")
        );
    }

    #[test]
    fn establishment_failure_records_cause_and_defers() {
        let store = AttestationStateStore::new();
        let channel_attempts = AtomicUsize::new(0);
        let active = active_config();
        let audio = [0.0_f32; 160];
        let journal_dir = tempfile::tempdir_in("/var/tmp").unwrap();
        let journal_path = journal_dir.path();
        std::fs::create_dir_all(journal_path.join("config")).unwrap();
        std::fs::write(
            journal_path.join("config/journal.json"),
            serde_json::to_vec(active.config.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        solstone_core_brain::generate_fingerprint_key(journal_path).unwrap();

        let error = super::transcribe_with(
            &audio,
            journal_path,
            &active,
            &store,
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _, _| -> Result<(CompositeVerdict, MockAttestedStream), &'static str> {
                channel_attempts.fetch_add(1, Ordering::SeqCst);
                Err("gateway_unreachable")
            },
        )
        .unwrap_err();
        assert_deferred_reason(error, "attestation_unreachable");
        assert_eq!(channel_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(
            store
                .get_attestation_state()
                .failure
                .as_ref()
                .map(|failure| failure.reason_code),
            Some("gateway_unreachable")
        );

        let now = chrono::Utc::now() + chrono::Duration::seconds(1);
        let config_map = active.config.as_ref().unwrap();
        let inspection = solstone_core_brain::inspect_brain_state(journal_path, config_map, now);
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
    }

    #[test]
    fn attestation_refusal_certificate_invalid_records_rejected_and_unhealthy() {
        let store = AttestationStateStore::new();
        let active = active_config();
        let audio = [0.0_f32; 160];
        let journal_dir = tempfile::tempdir_in("/var/tmp").unwrap();
        let journal_path = journal_dir.path();
        std::fs::create_dir_all(journal_path.join("config")).unwrap();
        std::fs::write(
            journal_path.join("config/journal.json"),
            serde_json::to_vec(active.config.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        solstone_core_brain::generate_fingerprint_key(journal_path).unwrap();

        let error = super::transcribe_with(
            &audio,
            journal_path,
            &active,
            &store,
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _, _| -> Result<(CompositeVerdict, MockAttestedStream), &'static str> {
                Err("certificate_invalid")
            },
        )
        .unwrap_err();
        assert_deferred_reason(error, "attestation_failed");

        let now = chrono::Utc::now() + chrono::Duration::seconds(1);
        let config_map = active.config.as_ref().unwrap();
        let inspection = solstone_core_brain::inspect_brain_state(journal_path, config_map, now);
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
    }

    #[test]
    fn attestation_refusal_readiness_install_failed_records_install_failed() {
        let store = AttestationStateStore::new();
        let active = active_config();
        let audio = [0.0_f32; 160];
        let journal_dir = tempfile::tempdir_in("/var/tmp").unwrap();
        let journal_path = journal_dir.path();
        std::fs::create_dir_all(journal_path.join("config")).unwrap();
        std::fs::write(
            journal_path.join("config/journal.json"),
            serde_json::to_vec(active.config.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        solstone_core_brain::generate_fingerprint_key(journal_path).unwrap();

        let error = super::transcribe_with(
            &audio,
            journal_path,
            &active,
            &store,
            |_| NvattestEnsureStatus::InstallFailed,
            |_, _, _| -> Result<(CompositeVerdict, MockAttestedStream), &'static str> {
                Ok((
                    verdict(),
                    MockAttestedStream {
                        response: Vec::new(),
                        read_offset: 0,
                        written: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
                        established_flag: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                            false,
                        )),
                        saw_established: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                            false,
                        )),
                    },
                ))
            },
        )
        .unwrap_err();
        assert_deferred_reason(error, "attestation_failed");

        let now = chrono::Utc::now() + chrono::Duration::seconds(1);
        let config_map = active.config.as_ref().unwrap();
        let inspection = solstone_core_brain::inspect_brain_state(journal_path, config_map, now);
        assert_eq!(inspection.projection.aggregate_state, "unhealthy");
        assert_eq!(
            inspection.projection.reason_code.as_deref(),
            Some("nvattest_install_failed")
        );
        let record = inspection.record.expect("record exists");
        assert_eq!(
            record["evidence"]["lane_prerequisites"]["reason_code"],
            "nvattest_install_failed"
        );
    }

    #[test]
    fn attestation_refusal_non_spp_provider_skips_recording() {
        let store = AttestationStateStore::new();
        let openai_cfg = config(json!({
            "services":{
                "confidential":{
                    "device":"abc",
                    "endpoint_url":"https://endpoint",
                    "served_model_id":"served",
                    "credential_fingerprint_sha256":"cca56da30e3c8a13a11277193fd3263961e2e3d6d9f98038a91dac05e8fde16a"
                }
            },
            "providers":{
                "active":{"provider":"openai","model":"gpt-4o"},
                "local":{"endpoint_url":"https://endpoint","served_model_id":"served","credential":"endpoint-credential"}
            }
        }));
        let audio = [0.0_f32; 160];
        let journal_dir = tempfile::tempdir_in("/var/tmp").unwrap();
        let journal_path = journal_dir.path();
        std::fs::create_dir_all(journal_path.join("config")).unwrap();
        std::fs::write(
            journal_path.join("config/journal.json"),
            serde_json::to_vec(openai_cfg.config.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        solstone_core_brain::generate_fingerprint_key(journal_path).unwrap();

        let error = super::transcribe_with(
            &audio,
            journal_path,
            &openai_cfg,
            &store,
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _, _| -> Result<(CompositeVerdict, MockAttestedStream), &'static str> {
                Err("gateway_unreachable")
            },
        )
        .unwrap_err();
        assert_deferred_reason(error, "attestation_unreachable");

        assert!(!journal_path.join("health/brain.json").exists());
    }

    #[test]
    fn attestation_refusal_missing_fingerprint_key_skips_recording() {
        let store = AttestationStateStore::new();
        let active = active_config();
        let audio = [0.0_f32; 160];
        let journal_dir = tempfile::tempdir_in("/var/tmp").unwrap();
        let journal_path = journal_dir.path();
        std::fs::create_dir_all(journal_path.join("config")).unwrap();
        std::fs::write(
            journal_path.join("config/journal.json"),
            serde_json::to_vec(active.config.as_ref().unwrap()).unwrap(),
        )
        .unwrap();

        let error = super::transcribe_with(
            &audio,
            journal_path,
            &active,
            &store,
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _, _| -> Result<(CompositeVerdict, MockAttestedStream), &'static str> {
                Err("gateway_unreachable")
            },
        )
        .unwrap_err();
        assert_deferred_reason(error, "attestation_unreachable");

        assert!(!journal_path.join("secrets/fingerprint.key").exists());
        assert!(!journal_path.join("health/brain.json").exists());
    }

    #[test]
    fn attestation_refusal_config_changed_to_none_skips_recording() {
        let store = AttestationStateStore::new();
        let active = active_config();
        let audio = [0.0_f32; 160];
        let journal_dir = tempfile::tempdir_in("/var/tmp").unwrap();
        let journal_path = journal_dir.path();
        std::fs::create_dir_all(journal_path.join("config")).unwrap();
        std::fs::write(
            journal_path.join("config/journal.json"),
            serde_json::to_vec(active.config.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        solstone_core_brain::generate_fingerprint_key(journal_path).unwrap();

        std::fs::write(
            journal_path.join("config/journal.json"),
            serde_json::to_vec(&json!({"providers": {"active": {"provider": "none"}}})).unwrap(),
        )
        .unwrap();

        let error = super::transcribe_with(
            &audio,
            journal_path,
            &active,
            &store,
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _, _| -> Result<(CompositeVerdict, MockAttestedStream), &'static str> {
                Err("gateway_unreachable")
            },
        )
        .unwrap_err();
        assert_deferred_reason(error, "attestation_unreachable");

        assert!(!journal_path.join("health/brain.json").exists());
    }

    #[test]
    fn attestation_refusal_missing_credential_hosted_unreachable() {
        let store = AttestationStateStore::new();
        let no_cred_cfg = config(json!({
            "services":{
                "confidential":{
                    "device":"abc",
                    "endpoint_url":"https://endpoint",
                    "served_model_id":"served",
                    "credential_fingerprint_sha256":"cca56da30e3c8a13a11277193fd3263961e2e3d6d9f98038a91dac05e8fde16a"
                }
            },
            "providers":{
                "active":{"provider":"local","model":"served"},
                "local":{"endpoint_url":"https://endpoint","served_model_id":"served","credential":""}
            }
        }));
        let audio = [0.0_f32; 160];
        let journal_dir = tempfile::tempdir_in("/var/tmp").unwrap();
        let journal_path = journal_dir.path();
        std::fs::create_dir_all(journal_path.join("config")).unwrap();
        std::fs::write(
            journal_path.join("config/journal.json"),
            serde_json::to_vec(no_cred_cfg.config.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        solstone_core_brain::generate_fingerprint_key(journal_path).unwrap();

        let error = super::transcribe_with(
            &audio,
            journal_path,
            &no_cred_cfg,
            &store,
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _, _| -> Result<(CompositeVerdict, FailingWriteStream), &'static str> {
                Ok((verdict(), FailingWriteStream))
            },
        )
        .unwrap_err();
        assert_deferred_reason(error, "hosted_transcribe_unreachable");
        assert!(!journal_path.join("health/brain.json").exists());
    }

    struct FailingWriteStream;

    impl std::io::Read for FailingWriteStream {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Ok(0)
        }
    }

    impl std::io::Write for FailingWriteStream {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "write pipe broken",
            ))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl AttestedIo for FailingWriteStream {
        fn set_io_timeout(&mut self, _timeout: Option<Duration>) -> std::io::Result<()> {
            Ok(())
        }

        fn trailing_after_body(&mut self) -> std::io::Result<solstone_core_spp_ratls::Trailing> {
            Ok(solstone_core_spp_ratls::Trailing::None)
        }
    }

    #[test]
    fn attestation_refusal_established_stream_write_failure_does_not_write_brain_record() {
        let store = AttestationStateStore::new();
        let active = active_config();
        let audio = [0.0_f32; 160];
        let journal_dir = tempfile::tempdir_in("/var/tmp").unwrap();
        let journal_path = journal_dir.path();
        std::fs::create_dir_all(journal_path.join("config")).unwrap();
        std::fs::write(
            journal_path.join("config/journal.json"),
            serde_json::to_vec(active.config.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        solstone_core_brain::generate_fingerprint_key(journal_path).unwrap();

        let error = super::transcribe_with(
            &audio,
            journal_path,
            &active,
            &store,
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _, _| -> Result<(CompositeVerdict, FailingWriteStream), &'static str> {
                Ok((verdict(), FailingWriteStream))
            },
        )
        .unwrap_err();
        assert_deferred_reason(error, "hosted_transcribe_unreachable");
        assert!(!journal_path.join("health/brain.json").exists());
    }

    struct MockAttestedStream {
        response: Vec<u8>,
        read_offset: usize,
        written: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
        established_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
        saw_established: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl std::io::Read for MockAttestedStream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let available = &self.response[self.read_offset..];
            let len = available.len().min(buf.len());
            buf[..len].copy_from_slice(&available[..len]);
            self.read_offset += len;
            Ok(len)
        }
    }

    impl std::io::Write for MockAttestedStream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.established_flag.load(Ordering::SeqCst) {
                self.saw_established.store(true, Ordering::SeqCst);
            }
            self.written.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl AttestedIo for MockAttestedStream {
        fn set_io_timeout(&mut self, _timeout: Option<Duration>) -> std::io::Result<()> {
            Ok(())
        }

        fn trailing_after_body(&mut self) -> std::io::Result<solstone_core_spp_ratls::Trailing> {
            if self.read_offset < self.response.len() {
                Ok(solstone_core_spp_ratls::Trailing::Surplus)
            } else {
                Ok(solstone_core_spp_ratls::Trailing::None)
            }
        }
    }

    #[test]
    fn stale_session_does_not_reuse_and_reattests() {
        let store = AttestationStateStore::new();
        store.record_attestation_verified(AttestationSession {
            verdict: verdict(),
            started_at: UNIX_EPOCH,
            tpm_heartbeat_at: UNIX_EPOCH,
            gpu_reattest_at: UNIX_EPOCH,
        });
        let channel_attempts = std::sync::Arc::new(AtomicUsize::new(0));
        let active = active_config();
        let audio = [0.0_f32; 160];
        let established = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let saw_established = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let written = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let channel_attempts_clone = channel_attempts.clone();
        let established_clone = established.clone();
        let saw_established_clone = saw_established.clone();
        let written_clone = written.clone();

        let response_bytes = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{VALID}",
            VALID.len()
        )
        .into_bytes();

        let result = super::transcribe_with(
            &audio,
            Path::new("/journal"),
            &active,
            &store,
            |_| NvattestEnsureStatus::AlreadyInstalled,
            move |_, _, _| {
                channel_attempts_clone.fetch_add(1, Ordering::SeqCst);
                let stream = MockAttestedStream {
                    response: response_bytes,
                    read_offset: 0,
                    written: written_clone,
                    established_flag: established_clone.clone(),
                    saw_established: saw_established_clone,
                };
                established_clone.store(true, Ordering::SeqCst);
                Ok((verdict(), stream))
            },
        );
        assert!(result.is_ok());
        assert_eq!(channel_attempts.load(Ordering::SeqCst), 1);
        assert!(saw_established.load(Ordering::SeqCst));
    }

    #[test]
    fn fresh_session_in_store_does_not_skip_reattest() {
        let store = AttestationStateStore::new();
        store.record_attestation_verified(verified_session(std::time::SystemTime::now()));
        let channel_attempts = std::sync::Arc::new(AtomicUsize::new(0));
        let active = active_config();
        let audio = [0.0_f32; 160];
        let established = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let saw_established = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let written = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let channel_attempts_clone = channel_attempts.clone();
        let established_clone = established.clone();
        let saw_established_clone = saw_established.clone();
        let written_clone = written.clone();

        let response_bytes = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{VALID}",
            VALID.len()
        )
        .into_bytes();

        let result = super::transcribe_with(
            &audio,
            Path::new("/journal"),
            &active,
            &store,
            |_| NvattestEnsureStatus::AlreadyInstalled,
            move |_, _, _| {
                channel_attempts_clone.fetch_add(1, Ordering::SeqCst);
                let stream = MockAttestedStream {
                    response: response_bytes,
                    read_offset: 0,
                    written: written_clone,
                    established_flag: established_clone.clone(),
                    saw_established: saw_established_clone,
                };
                established_clone.store(true, Ordering::SeqCst);
                Ok((verdict(), stream))
            },
        );
        assert!(result.is_ok());
        assert_eq!(channel_attempts.load(Ordering::SeqCst), 1);
        assert!(saw_established.load(Ordering::SeqCst));
    }

    #[test]
    fn success_path_establishes_channel_and_transcribes() {
        let store = AttestationStateStore::new();
        let channel_attempts = std::sync::Arc::new(AtomicUsize::new(0));
        let active = active_config();
        let audio = [0.0_f32; 160];
        let established = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let saw_established = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let written = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let channel_attempts_clone = channel_attempts.clone();
        let established_clone = established.clone();
        let saw_established_clone = saw_established.clone();
        let written_clone = written.clone();

        let response_bytes = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{VALID}",
            VALID.len()
        )
        .into_bytes();

        let (response, metadata) = super::transcribe_with(
            &audio,
            Path::new("/journal"),
            &active,
            &store,
            |_| NvattestEnsureStatus::AlreadyInstalled,
            move |_, _, _| {
                channel_attempts_clone.fetch_add(1, Ordering::SeqCst);
                let stream = MockAttestedStream {
                    response: response_bytes,
                    read_offset: 0,
                    written: written_clone,
                    established_flag: established_clone.clone(),
                    saw_established: saw_established_clone,
                };
                established_clone.store(true, Ordering::SeqCst);
                Ok((verdict(), stream))
            },
        )
        .unwrap();

        assert_eq!(channel_attempts.load(Ordering::SeqCst), 1);
        assert!(saw_established.load(Ordering::SeqCst));
        assert_eq!(metadata.model, "confidential");
        assert_eq!(metadata.device, "confidential");
        assert_eq!(response.text, "hello");

        let written_bytes = written.lock().unwrap().clone();
        let request_line = b"POST /v1/audio/transcriptions HTTP/1.1\r\n";
        assert_eq!(
            written_bytes
                .windows(request_line.len())
                .filter(|window| *window == request_line)
                .count(),
            1
        );
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
        assert_eq!(CONFIDENTIAL_STT_MAX_REQUEST_BYTES, 11 * 1024 * 1024);
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
            "services":{
                "confidential":{
                    "device":"abc",
                    "endpoint_url":"https://endpoint",
                    "served_model_id":"served",
                    "credential_fingerprint_sha256":"cca56da30e3c8a13a11277193fd3263961e2e3d6d9f98038a91dac05e8fde16a"
                }
            },
            "providers":{
                "active":{"provider":"local","model":"served"},
                "local":{"endpoint_url":"https://endpoint","served_model_id":"served","credential":"endpoint-credential"}
            }
        }))
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
                status: solstone_core_spp_attest::nvgpu::GpuStatusAuthorization::OnlineNonce,
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

    use log::{Level, LevelFilter, Log, Metadata, Record};
    use std::sync::{Mutex, Once, OnceLock};

    struct TestLogger;
    static LOGGER: TestLogger = TestLogger;
    static LOGGER_INIT: Once = Once::new();
    static LOGS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

    impl Log for TestLogger {
        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            metadata.level() <= Level::Warn
        }
        fn log(&self, record: &Record<'_>) {
            if self.enabled(record.metadata()) {
                LOGS.get_or_init(|| Mutex::new(Vec::new()))
                    .lock()
                    .unwrap()
                    .push(record.args().to_string());
            }
        }
        fn flush(&self) {}
    }

    fn init_test_logger() {
        LOGGER_INIT.call_once(|| {
            log::set_logger(&LOGGER).unwrap();
            log::set_max_level(LevelFilter::Warn);
        });
    }

    fn captured_logs() -> Vec<String> {
        LOGS.get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap()
            .clone()
    }

    fn mock_stream(response: Vec<u8>) -> MockAttestedStream {
        MockAttestedStream {
            response,
            read_offset: 0,
            written: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            established_flag: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            saw_established: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    #[test]
    fn transcription_verification_nvattest_readiness_unavailable() {
        let store = AttestationStateStore::new();
        let active = active_config();
        let audio = [0.0_f32; 160];
        let journal_dir = tempfile::tempdir_in("/var/tmp").unwrap();
        let journal_path = journal_dir.path();
        std::fs::create_dir_all(journal_path.join("config")).unwrap();
        std::fs::write(
            journal_path.join("config/journal.json"),
            serde_json::to_vec(active.config.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        solstone_core_brain::generate_fingerprint_key(journal_path).unwrap();

        let mut establish_called = false;
        let error = super::transcribe_with(
            &audio,
            journal_path,
            &active,
            &store,
            |_| NvattestEnsureStatus::Unavailable,
            |_, _, _| -> Result<(CompositeVerdict, MockAttestedStream), &'static str> {
                establish_called = true;
                Ok((verdict(), mock_stream(vec![])))
            },
        )
        .unwrap_err();
        assert!(!establish_called);
        assert_deferred_reason(error, "attestation_failed");

        let status = solstone_core_brain::read_transcription_verification(journal_path)
            .expect("status written");
        assert_eq!(status.reason, "nvattest_unavailable");
        assert_eq!(status.endpoint, "https://endpoint");
        assert!(chrono::DateTime::parse_from_rfc3339(status.observed_at.as_ref().unwrap()).is_ok());
    }

    #[test]
    fn transcription_verification_invalid_target_refuses() {
        let store = AttestationStateStore::new();
        let active = config(json!({
            "services":{
                "confidential":{
                    "device":"dev-1",
                    "endpoint_url":"not-a-url",
                    "served_model_id":"served",
                    "credential_fingerprint_sha256":"cca56da30e3c8a13a11277193fd3263961e2e3d6d9f98038a91dac05e8fde16a"
                }
            },
            "providers":{
                "active":{"provider":"local","model":"served"},
                "local":{"endpoint_url":"not-a-url","served_model_id":"served","credential":"endpoint-credential"}
            }
        }));
        let audio = [0.0_f32; 160];
        let journal_dir = tempfile::tempdir_in("/var/tmp").unwrap();
        let journal_path = journal_dir.path();
        std::fs::create_dir_all(journal_path.join("config")).unwrap();
        std::fs::write(
            journal_path.join("config/journal.json"),
            serde_json::to_vec(active.config.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        solstone_core_brain::generate_fingerprint_key(journal_path).unwrap();

        let error = super::transcribe_with(
            &audio,
            journal_path,
            &active,
            &store,
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _, _| -> Result<(CompositeVerdict, MockAttestedStream), &'static str> {
                panic!("establish should not be called for invalid target");
            },
        )
        .unwrap_err();
        assert_deferred_reason(error, "attestation_failed");

        let status = solstone_core_brain::read_transcription_verification(journal_path)
            .expect("status written");
        assert_eq!(status.reason, "attestation_rejected");
        assert_eq!(status.endpoint, "not-a-url");
        assert!(chrono::DateTime::parse_from_rfc3339(status.observed_at.as_ref().unwrap()).is_ok());
    }

    #[test]
    fn transcription_verification_establishment_refusal_and_active_lane() {
        let store = AttestationStateStore::new();
        let active = active_config();
        let audio = [0.0_f32; 160];
        let journal_dir = tempfile::tempdir_in("/var/tmp").unwrap();
        let journal_path = journal_dir.path();
        std::fs::create_dir_all(journal_path.join("config")).unwrap();
        std::fs::write(
            journal_path.join("config/journal.json"),
            serde_json::to_vec(active.config.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        solstone_core_brain::generate_fingerprint_key(journal_path).unwrap();

        let error = super::transcribe_with(
            &audio,
            journal_path,
            &active,
            &store,
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _, _| -> Result<(CompositeVerdict, MockAttestedStream), &'static str> {
                Err("gateway_unreachable")
            },
        )
        .unwrap_err();
        assert_deferred_reason(error, "attestation_unreachable");

        let status = solstone_core_brain::read_transcription_verification(journal_path)
            .expect("status written");
        assert_eq!(status.reason, "attestation_not_verified");
        assert_eq!(status.endpoint, "https://endpoint");
        assert!(chrono::DateTime::parse_from_rfc3339(status.observed_at.as_ref().unwrap()).is_ok());

        // Active lane (spp): health/brain.json also exists
        assert!(journal_path.join("health/brain.json").exists());
    }

    #[test]
    fn transcription_verification_cleared_on_established_channel() {
        let store = AttestationStateStore::new();
        let active = active_config();
        let audio = [0.0_f32; 160];
        let journal_dir = tempfile::tempdir_in("/var/tmp").unwrap();
        let journal_path = journal_dir.path();
        std::fs::create_dir_all(journal_path.join("config")).unwrap();
        std::fs::write(
            journal_path.join("config/journal.json"),
            serde_json::to_vec(active.config.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        solstone_core_brain::generate_fingerprint_key(journal_path).unwrap();

        // 1. Refusal creates status
        let _ = super::transcribe_with(
            &audio,
            journal_path,
            &active,
            &store,
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _, _| -> Result<(CompositeVerdict, MockAttestedStream), &'static str> {
                Err("gateway_unreachable")
            },
        );
        assert!(solstone_core_brain::read_transcription_verification(journal_path).is_some());
        assert!(journal_path.join("health/brain.json").exists());

        // 2. Established channel with FailingWriteStream clears status but write fails later
        let error2 = super::transcribe_with(
            &audio,
            journal_path,
            &active,
            &store,
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _, _| -> Result<(CompositeVerdict, FailingWriteStream), &'static str> {
                Ok((verdict(), FailingWriteStream))
            },
        )
        .unwrap_err();
        assert_deferred_reason(error2, "hosted_transcribe_unreachable");

        // Status is cleared, but brain.json from first call remains
        assert_eq!(
            solstone_core_brain::read_transcription_verification(journal_path),
            None
        );
        assert!(journal_path.join("health/brain.json").exists());
    }

    #[test]
    fn transcription_verification_other_lane_records_status_skips_brain_record() {
        let store = AttestationStateStore::new();
        let openai_cfg = config(json!({
            "services":{
                "confidential":{
                    "device":"dev-1",
                    "endpoint_url":"https://endpoint",
                    "served_model_id":"served",
                    "credential_fingerprint_sha256":"cca56da30e3c8a13a11277193fd3263961e2e3d6d9f98038a91dac05e8fde16a"
                }
            },
            "providers":{
                "active":{"provider":"openai","model":"gpt-4o"},
                "local":{"endpoint_url":"https://endpoint","served_model_id":"served","credential":"endpoint-credential"}
            }
        }));
        let audio = [0.0_f32; 160];
        let journal_dir = tempfile::tempdir_in("/var/tmp").unwrap();
        let journal_path = journal_dir.path();
        std::fs::create_dir_all(journal_path.join("config")).unwrap();
        std::fs::write(
            journal_path.join("config/journal.json"),
            serde_json::to_vec(openai_cfg.config.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        solstone_core_brain::generate_fingerprint_key(journal_path).unwrap();

        let error = super::transcribe_with(
            &audio,
            journal_path,
            &openai_cfg,
            &store,
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _, _| -> Result<(CompositeVerdict, MockAttestedStream), &'static str> {
                Err("gateway_unreachable")
            },
        )
        .unwrap_err();
        assert_deferred_reason(error, "attestation_unreachable");

        let status = solstone_core_brain::read_transcription_verification(journal_path)
            .expect("status written");
        assert_eq!(status.reason, "attestation_not_verified");
        assert_eq!(status.endpoint, "https://endpoint");
        assert!(!journal_path.join("health/brain.json").exists());
    }

    #[test]
    fn early_returns_do_not_create_status_file() {
        let store = AttestationStateStore::new();
        let audio = [0.0_f32; 160];
        let journal_dir = tempfile::tempdir_in("/var/tmp").unwrap();
        let journal_path = journal_dir.path();
        std::fs::create_dir_all(journal_path.join("config")).unwrap();

        // The egress gate's own refusals never reach the send path, so only a
        // refusal on the send path itself can write the status.
        // A missing credential is refused before any attestation attempt.
        let no_cred_cfg = config(json!({
            "services": {
                "confidential": {
                    "device": "dev-1",
                    "endpoint_url": "https://endpoint",
                    "served_model_id": "served",
                    "credential_fingerprint_sha256": "cca56da30e3c8a13a11277193fd3263961e2e3d6d9f98038a91dac05e8fde16a"
                }
            },
            "providers": {
                "active": {"provider": "local", "model": "served"},
                "local": {"endpoint_url": "https://endpoint", "served_model_id": "served"}
            }
        }));
        let mut establish_called = false;
        let error3 = super::transcribe_with(
            &audio,
            journal_path,
            &no_cred_cfg,
            &store,
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _, _| -> Result<(CompositeVerdict, MockAttestedStream), &'static str> {
                establish_called = true;
                Ok((verdict(), mock_stream(vec![])))
            },
        )
        .unwrap_err();
        assert!(!establish_called);
        assert_deferred_reason(error3, "hosted_transcribe_unreachable");
        assert!(!solstone_core_brain::transcription_verification_path(journal_path).exists());
    }

    #[test]
    fn failed_status_write_logs_warning_and_defers() {
        init_test_logger();
        let store = AttestationStateStore::new();
        let active = active_config();
        let audio = [0.0_f32; 160];
        let journal_dir = tempfile::tempdir_in("/var/tmp").unwrap();
        let journal_path = journal_dir.path();
        std::fs::create_dir_all(journal_path.join("config")).unwrap();
        std::fs::write(
            journal_path.join("config/journal.json"),
            serde_json::to_vec(active.config.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        solstone_core_brain::generate_fingerprint_key(journal_path).unwrap();

        // Create confidential-transcription.json as a directory so atomic_replace fails
        std::fs::create_dir_all(journal_path.join("health/confidential-transcription.json"))
            .unwrap();

        let error = super::transcribe_with(
            &audio,
            journal_path,
            &active,
            &store,
            |_| NvattestEnsureStatus::AlreadyInstalled,
            |_, _, _| -> Result<(CompositeVerdict, MockAttestedStream), &'static str> {
                Err("gateway_unreachable")
            },
        )
        .unwrap_err();
        assert_deferred_reason(error, "attestation_unreachable");
        assert!(solstone_core_brain::read_transcription_verification(journal_path).is_none());

        let logs = captured_logs();
        let journal_str = journal_path.display().to_string();
        assert!(
            logs.iter().any(|msg| msg.contains(&journal_str)),
            "expected warning in logs: {logs:?}"
        );
    }
}
