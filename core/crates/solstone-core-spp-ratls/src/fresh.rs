// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One fresh production RA-TLS attestation attempt shared by confidential callers.

use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::{
    AttestationFailure, AttestationSession, AttestationStateStore, AttestedChannel,
    CompositeVerdict, NvattestEnsureStatus, RatlsEndpoint, classify_channel_failure,
    classify_nvattest_prerequisite, establish_production_attested_channel,
};

pub struct FreshAttestedChannel {
    pub host: String,
    pub stream: AttestedChannel,
    pub session: AttestationSession,
}

pub struct FreshAttestedChannelWith<S> {
    pub host: String,
    pub stream: S,
    pub session: AttestationSession,
}

#[doc(hidden)]
pub fn establish_fresh_production_channel(
    endpoint: &RatlsEndpoint,
    nvattest_dir: &Path,
    socket_timeout: Duration,
) -> Result<(CompositeVerdict, AttestedChannel), &'static str> {
    establish_production_attested_channel(endpoint, nvattest_dir, socket_timeout)
        .map(|channel| (channel.verified.verdict.clone(), channel))
        .map_err(|error| error.reason_code)
}

#[doc(hidden)]
pub fn perform_fresh_reattest_with<R, E, S>(
    state: &AttestationStateStore,
    endpoint_url: &str,
    nvattest_dir: &Path,
    socket_timeout: Duration,
    readiness: R,
    establish: E,
) -> Result<FreshAttestedChannelWith<S>, AttestationFailure>
where
    R: FnOnce(&Path) -> NvattestEnsureStatus,
    E: FnOnce(&RatlsEndpoint, &Path, Duration) -> Result<(CompositeVerdict, S), &'static str>,
{
    if let Some(failure) = classify_nvattest_prerequisite(readiness(nvattest_dir)) {
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
    let (verdict, stream) = match establish(&endpoint, nvattest_dir, socket_timeout) {
        Ok(established) => established,
        Err(reason_code) => {
            let failure = AttestationFailure {
                kind: classify_channel_failure(reason_code),
                reason_code,
            };
            state.record_attestation_failed(failure.kind, failure.reason_code);
            return Err(failure);
        }
    };
    let now = SystemTime::now();
    let session = AttestationSession {
        verdict,
        started_at: now,
        tpm_heartbeat_at: now,
        gpu_reattest_at: now,
    };
    state.record_attestation_verified(session.clone());
    Ok(FreshAttestedChannelWith {
        host,
        stream,
        session,
    })
}

pub fn perform_fresh_reattest<R>(
    state: &AttestationStateStore,
    endpoint_url: &str,
    nvattest_dir: &Path,
    socket_timeout: Duration,
    readiness: R,
) -> Result<FreshAttestedChannel, AttestationFailure>
where
    R: FnOnce(&Path) -> NvattestEnsureStatus,
{
    perform_fresh_reattest_with(
        state,
        endpoint_url,
        nvattest_dir,
        socket_timeout,
        readiness,
        establish_fresh_production_channel,
    )
    .map(|channel| FreshAttestedChannel {
        host: channel.host,
        stream: channel.stream,
        session: channel.session,
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
    use crate::{
        AttestationFailureKind, AttestationStateStore, NvattestEnsureStatus,
        check_nvattest_readiness, test_support::TempDir,
    };

    fn assert_prerequisite_failure(nvattest_dir: &Path, reason_code: &'static str) {
        let state = AttestationStateStore::new();
        let failure = match perform_fresh_reattest(
            &state,
            "not-a-channel-target",
            nvattest_dir,
            Duration::from_millis(1),
            check_nvattest_readiness,
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

    #[test]
    fn fresh_reattest_honors_an_injected_install_status() {
        let root = TempDir::new("fresh-inject");
        let state = AttestationStateStore::new();
        let failure = match perform_fresh_reattest(
            &state,
            "not-a-channel-target",
            root.path(),
            Duration::from_millis(1),
            |_| NvattestEnsureStatus::InstallInFlight,
        ) {
            Err(failure) => failure,
            Ok(_) => panic!("injected in-flight status must not establish a channel"),
        };
        assert_eq!(failure.kind, AttestationFailureKind::Unreachable);
        assert_eq!(failure.reason_code, "nvattest_install_in_progress");
        assert_eq!(state.get_attestation_state().failure, Some(failure));
    }
}
