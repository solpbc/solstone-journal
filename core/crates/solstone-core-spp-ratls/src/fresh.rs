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

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, time::Duration};

    use super::perform_fresh_reattest;
    use crate::{AttestationFailureKind, AttestationStateStore, test_support::TempDir};

    fn assert_prerequisite_failure(nvattest_dir: &Path, reason_code: &'static str) {
        let state = AttestationStateStore::new();
        let failure = match perform_fresh_reattest(
            &state,
            "not-a-channel-target",
            nvattest_dir,
            Duration::from_millis(1),
        ) {
            Err(failure) => failure,
            Ok(_) => panic!("readiness refusal must not establish a channel"),
        };
        assert_eq!(failure.kind, AttestationFailureKind::Failed);
        assert_eq!(failure.reason_code, reason_code);
        assert_eq!(state.get_attestation_state().failure, Some(failure));
    }

    #[test]
    fn fresh_reattest_records_the_locator_cause_before_channel_establishment() {
        let root = TempDir::new("fresh");
        assert_prerequisite_failure(&root.path().join("missing"), "nvattest_unavailable");
        assert_prerequisite_failure(root.path(), "nvattest_unavailable");

        fs::create_dir_all(root.path().join("bin")).expect("create binary directory");
        fs::create_dir_all(root.path().join("lib")).expect("create library directory");
        fs::write(root.path().join("bin/nvattest"), "placeholder").expect("write binary");
        assert_prerequisite_failure(root.path(), "nvattest_integrity_failed");
    }
}
