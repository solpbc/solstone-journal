// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One fresh production RA-TLS attestation attempt shared by confidential callers.

use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::{
    AttestationFailure, AttestationSession, AttestationStateStore, AttestedChannel, RatlsEndpoint,
    check_nvattest_readiness, classify_channel_failure, classify_nvattest_prerequisite,
    establish_production_attested_channel,
};

pub struct FreshAttestedChannel {
    pub host: String,
    pub stream: AttestedChannel,
    pub session: AttestationSession,
}

pub fn perform_fresh_reattest(
    state: &AttestationStateStore,
    endpoint_url: &str,
    nvattest_dir: &Path,
    socket_timeout: Duration,
) -> Result<FreshAttestedChannel, AttestationFailure> {
    if let Some(failure) = classify_nvattest_prerequisite(check_nvattest_readiness(nvattest_dir)) {
        state.record_attestation_failed(failure.kind, failure.reason_code);
        return Err(failure);
    }
    let Some((endpoint, host)) = target(endpoint_url) else {
        let failure = AttestationFailure {
            kind: classify_channel_failure("tls_handshake_failed"),
            reason_code: "tls_handshake_failed",
        };
        state.record_attestation_failed(failure.kind, failure.reason_code);
        return Err(failure);
    };
    let stream =
        match establish_production_attested_channel(&endpoint, nvattest_dir, socket_timeout) {
            Ok(stream) => stream,
            Err(error) => {
                let failure = AttestationFailure {
                    kind: classify_channel_failure(error.reason_code),
                    reason_code: error.reason_code,
                };
                state.record_attestation_failed(failure.kind, failure.reason_code);
                return Err(failure);
            }
        };
    let now = SystemTime::now();
    let session = AttestationSession {
        verdict: stream.verified.verdict.clone(),
        started_at: now,
        tpm_heartbeat_at: now,
        gpu_reattest_at: now,
    };
    state.record_attestation_verified(session.clone());
    Ok(FreshAttestedChannel {
        host,
        stream,
        session,
    })
}

fn target(base_url: &str) -> Option<(RatlsEndpoint, String)> {
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
    (!host.is_empty()).then(|| (RatlsEndpoint::new(host, port), authority.to_owned()))
}
