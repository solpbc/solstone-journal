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

#[doc(hidden)]
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
    establish_production_attested_channel(endpoint, nvattest_dir, socket_timeout, 0)
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
    let Some((endpoint, host)) = resolve_ratls_target(endpoint_url) else {
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

pub fn resolve_ratls_target(base_url: &str) -> Option<(RatlsEndpoint, String)> {
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

#[cfg(windows)]
pub fn resolve_nvattest_dir(
    config: Option<&serde_json::Map<String, serde_json::Value>>,
    journal_path: &Path,
) -> std::path::PathBuf {
    let _ = (config, journal_path);
    // The verifier belongs to the installed signed program.
    // Neither journal state nor environment can select an executable.
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            let bin = exe.parent()?;
            (bin.file_name()? == "bin").then(|| bin.parent().map(Path::to_path_buf))?
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, time::Duration};

    use super::perform_fresh_reattest;
    use crate::{
        AttestationFailureKind, AttestationStateStore, NvattestEnsureStatus,
        check_nvattest_readiness, test_support::TempDir,
    };

    #[test]
    fn target_refuses_an_empty_host_and_defaults_to_port_443() {
        for refused in ["https://:443", "https://", "https:///v1", "not-a-url"] {
            assert!(super::resolve_ratls_target(refused).is_none(), "{refused}");
        }
        let (endpoint, host) =
            super::resolve_ratls_target("https://confidential.example/v1").unwrap();
        assert_eq!(
            endpoint,
            crate::RatlsEndpoint::new("confidential.example", 443)
        );
        assert_eq!(host, "confidential.example");
        let (endpoint, host) =
            super::resolve_ratls_target("https://confidential.example:8443/v1").unwrap();
        assert_eq!(
            endpoint,
            crate::RatlsEndpoint::new("confidential.example", 8443)
        );
        assert_eq!(host, "confidential.example:8443");
    }

    fn assert_prerequisite_failure(
        nvattest_dir: &Path,
        reason_code: &'static str,
        expected_kind: AttestationFailureKind,
    ) {
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
        assert_eq!(failure.kind, expected_kind);
        assert_eq!(failure.reason_code, reason_code);
        assert_eq!(state.get_attestation_state().failure, Some(failure));
    }

    #[test]
    fn fresh_reattest_records_the_locator_cause_before_channel_establishment() {
        let root = TempDir::new("fresh");
        assert_prerequisite_failure(
            &root.path().join("missing"),
            "nvattest_unavailable",
            AttestationFailureKind::Unreachable,
        );
        assert_prerequisite_failure(
            root.path(),
            "nvattest_unavailable",
            AttestationFailureKind::Unreachable,
        );

        fs::create_dir_all(root.path().join("bin")).expect("create binary directory");
        fs::create_dir_all(root.path().join("lib")).expect("create library directory");
        fs::write(root.path().join("bin/nvattest"), "placeholder").expect("write binary");
        assert_prerequisite_failure(
            root.path(),
            "nvattest_integrity_failed",
            AttestationFailureKind::Failed,
        );
    }

    #[test]
    fn fresh_reattest_honors_an_injected_readiness_status() {
        let root = TempDir::new("fresh-inject");
        let state = AttestationStateStore::new();
        let failure = match perform_fresh_reattest(
            &state,
            "not-a-channel-target",
            root.path(),
            Duration::from_millis(1),
            |_| NvattestEnsureStatus::PlatformUnsupported,
        ) {
            Err(failure) => failure,
            Ok(_) => panic!("injected status must not establish a channel"),
        };
        assert_eq!(failure.kind, AttestationFailureKind::Failed);
        assert_eq!(failure.reason_code, "nvattest_platform_unsupported");
        assert_eq!(state.get_attestation_state().failure, Some(failure));
    }
}
