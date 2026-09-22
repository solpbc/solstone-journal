// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use solstone_core_spp_attest::snp::check_pcr_fingerprint;
use solstone_core_spp_attest::{PcrMode, Policy};

use crate::ratls::channel::{
    AttestedIo, RatlsEndpoint, establish_attested_channel, send_json_request,
    send_transcription_request,
};
use crate::ratls::contract::CompositeEvidence;
use crate::ratls::verify::CompositeVerifier;

const QUALIFICATION_WAV: &[u8] = &[
    b'R', b'I', b'F', b'F', 0x44, 0x00, 0x00, 0x00, b'W', b'A', b'V', b'E', b'f', b'm', b't', b' ',
    0x10, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x80, 0x3e, 0x00, 0x00, 0x00, 0x7d, 0x00, 0x00,
    0x02, 0x00, 0x10, 0x00, b'd', b'a', b't', b'a', 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

#[derive(Debug, Clone)]
pub struct QualificationRequest {
    pub host: String,
    pub port: u16,
    pub policy: Policy,
    pub output_dir: PathBuf,
    pub model: Option<String>,
    pub content: bool,
    pub owner_nonce: [u8; 32],
    pub now: SystemTime,
    pub socket_timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct QualificationSuccess {
    pub evidence: CompositeEvidence,
    pub pcr_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualificationError {
    pub reason_code: &'static str,
}

impl fmt::Display for QualificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "confidential attestation rejected ({})",
            self.reason_code
        )
    }
}

impl std::error::Error for QualificationError {}

pub fn qualification_policy(mode: PcrMode, pins: BTreeSet<String>) -> Policy {
    Policy {
        pcr_mode: mode,
        pcr_pins: pins,
        ..Policy::default()
    }
}

pub fn run_qualification(
    request: &QualificationRequest,
    nvattest_dir: &Path,
    composite_verifier: &dyn CompositeVerifier,
) -> Result<QualificationSuccess, QualificationError> {
    if request.content && request.model.is_none() {
        return Err(QualificationError {
            reason_code: "model_missing",
        });
    }

    let endpoint = RatlsEndpoint::new(&request.host, request.port);
    let mut channel = establish_attested_channel(
        &endpoint,
        &request.owner_nonce,
        nvattest_dir,
        request.now,
        None,
        Some(&request.policy),
        None,
        composite_verifier,
        request.socket_timeout,
        0,
    )
    .map_err(|error| QualificationError {
        reason_code: error.reason_code,
    })?;

    write_evidence_files(
        &request.output_dir,
        &request.owner_nonce,
        &channel.verified.evidence,
    )?;

    let pcr_sha256 = match request.policy.pcr_mode {
        PcrMode::Record => Some(
            check_pcr_fingerprint(&channel.verified.evidence.quote_pcrs, &request.policy).map_err(
                |_| QualificationError {
                    reason_code: "pcr_fingerprint_failed",
                },
            )?,
        ),
        _ => None,
    };

    if !request.content {
        return Ok(QualificationSuccess {
            evidence: channel.verified.evidence.clone(),
            pcr_sha256,
        });
    }

    let model = request.model.as_ref().expect("model checked");
    let host_header = format!("{}:{}", request.host, request.port);
    let chat_body = serde_json::to_vec(&serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": "qualification"}]
    }))
    .map_err(|_| QualificationError {
        reason_code: "chat_failed",
    })?;

    channel
        .set_io_timeout(Some(request.socket_timeout))
        .map_err(|_| QualificationError {
            reason_code: "chat_failed",
        })?;

    let chat_response = send_json_request(
        &mut channel,
        &host_header,
        "/v1/chat/completions",
        None,
        &chat_body,
    )
    .map_err(|_| QualificationError {
        reason_code: "chat_failed",
    })?;

    if !(200..=299).contains(&chat_response.status) {
        return Err(QualificationError {
            reason_code: "chat_failed",
        });
    }

    channel
        .set_io_timeout(Some(request.socket_timeout))
        .map_err(|_| QualificationError {
            reason_code: "transcription_failed",
        })?;

    let transcribe_response =
        send_transcription_request(&mut channel, &host_header, QUALIFICATION_WAV).map_err(
            |_| QualificationError {
                reason_code: "transcription_failed",
            },
        )?;

    if !(200..=299).contains(&transcribe_response.status) {
        return Err(QualificationError {
            reason_code: "transcription_failed",
        });
    }

    Ok(QualificationSuccess {
        evidence: channel.verified.evidence.clone(),
        pcr_sha256,
    })
}

fn write_evidence_files(
    output_dir: &Path,
    owner_nonce: &[u8; 32],
    evidence: &CompositeEvidence,
) -> Result<(), QualificationError> {
    fs::create_dir_all(output_dir).map_err(|_| QualificationError {
        reason_code: "evidence_write_failed",
    })?;

    let mut nonce_hex = String::with_capacity(64);
    for byte in owner_nonce {
        use std::fmt::Write;
        let _ = write!(&mut nonce_hex, "{byte:02x}");
    }

    fs::write(output_dir.join("nonce.hex"), nonce_hex.as_bytes()).map_err(|_| {
        QualificationError {
            reason_code: "evidence_write_failed",
        }
    })?;
    fs::write(output_dir.join("akpub.pem"), &evidence.ak_public_key_pem).map_err(|_| {
        QualificationError {
            reason_code: "evidence_write_failed",
        }
    })?;
    fs::write(output_dir.join("quote.msg"), &evidence.quote_message).map_err(|_| {
        QualificationError {
            reason_code: "evidence_write_failed",
        }
    })?;
    fs::write(output_dir.join("quote.sig"), &evidence.quote_signature).map_err(|_| {
        QualificationError {
            reason_code: "evidence_write_failed",
        }
    })?;
    fs::write(output_dir.join("quote.pcrs"), &evidence.quote_pcrs).map_err(|_| {
        QualificationError {
            reason_code: "evidence_write_failed",
        }
    })?;
    fs::write(output_dir.join("hcl_report.bin"), &evidence.hcl_report).map_err(|_| {
        QualificationError {
            reason_code: "evidence_write_failed",
        }
    })?;

    Ok(())
}
