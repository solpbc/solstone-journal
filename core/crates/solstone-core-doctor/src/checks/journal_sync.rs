// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
#[cfg(unix)]
use solstone_core_system::lifecycle::{
    SyncCheckResult, SyncPeerIdentity, SyncRescan, rescan_sync_read_only, sync_peer_diagnostic,
};
#[cfg(unix)]
use solstone_core_system::process::SystemProcessInstanceSource;
#[cfg(unix)]
use solstone_core_system_health::{SyncRescanDiagnosis, describe_sync_rescan};

#[cfg(unix)]
use super::service_status;

#[cfg(unix)]
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    if !context.journal_path.is_dir() {
        return Ok(make_result(
            check,
            Status::Skip,
            "no local journal",
            None::<String>,
        ));
    }
    let process_source = SystemProcessInstanceSource;
    let now = context.now.timestamp() as f64;
    let diagnosis =
        describe_sync_rescan(&context.journal_path, "doctor.check", now, &process_source);
    // `doctor` never writes its own heartbeat, so the exclusion filter inside
    // `describe_sync_rescan` (keyed on a caller's own heartbeat filename) can
    // never suppress the journal's own supervisor. That means a healthy,
    // currently-running supervisor's fresh heartbeat is otherwise
    // indistinguishable from a genuine foreign live writer and gets
    // classified `HeartbeatNeedsAttention`. Confirming that this journal's
    // own supervisor is reachable over its status socket (the same probe
    // `service_running` and `task_pace` already use) tells us our own
    // supervisor is healthy, but it does NOT tell us whether the live
    // heartbeat behind this classification was ours or a genuine second
    // writer -- the status payload carries no heartbeat filename, writer id,
    // or run id for us to exclude by (and adding one would be a
    // Callosum wire-contract change, out of scope here). So when the
    // supervisor is confirmed reachable, re-read the raw peer set
    // (read-only, no side effects) and report it exactly the way the Clean
    // branch below would: Ok, but naming the last observed peer's identity
    // when one is present, so a genuine foreign writer is still surfaced
    // instead of silently erased by an unconditional "this device only".
    if matches!(diagnosis, SyncRescanDiagnosis::HeartbeatNeedsAttention(_))
        && service_status::fetch(context).is_some()
        && let Ok(SyncRescan::Complete(result)) =
            rescan_sync_read_only(&context.journal_path, "doctor.check", None, now)
    {
        return Ok(make_result(
            check,
            Status::Ok,
            clean_detail(context, Some(&result)),
            None::<String>,
        ));
    }
    render_sync_rescan(context, check, diagnosis)
}

#[cfg(not(unix))]
pub fn run(_context: &CheckContext, check: Check) -> RunnerResult {
    Ok(make_result(
        check,
        Status::Skip,
        "not supported on windows",
        None::<String>,
    ))
}

/// Render the "this device only" detail, naming the most recently observed
/// peer's identity when one is present. Shared by the confirmed-running
/// downgrade above and the `Clean` diagnosis below so both report a genuine
/// foreign writer identically instead of one of them silently dropping it.
#[cfg(unix)]
fn clean_detail(context: &CheckContext, result: Option<&SyncCheckResult>) -> String {
    let clean = format!("this device only ({})", context.hostname);
    result
        .and_then(|result| result.peer_observations.last())
        .map(sync_peer_diagnostic)
        .filter(|writer| !writer.identity.is_unidentified())
        .map_or_else(
            || clean.clone(),
            |writer| {
                format!(
                    "{clean}\n  last foreign writer: {} ({})",
                    writer.hostname,
                    doctor_identity_label(&writer.identity)
                )
            },
        )
}

#[cfg(unix)]
fn render_sync_rescan(
    context: &CheckContext,
    check: Check,
    diagnosis: SyncRescanDiagnosis,
) -> RunnerResult {
    match diagnosis {
        SyncRescanDiagnosis::Waiting(message) => {
            Ok(make_result(check, Status::Warn, message, None::<String>))
        }
        SyncRescanDiagnosis::HeartbeatNeedsAttention(message)
        | SyncRescanDiagnosis::AdmissionWaitNeedsAttention(message) => {
            Ok(make_result(check, Status::Fail, message, None::<String>))
        }
        SyncRescanDiagnosis::Unsafe(message) => {
            Ok(make_result(check, Status::Fail, message, None::<String>))
        }
        SyncRescanDiagnosis::Clean(result) => Ok(make_result(
            check,
            Status::Ok,
            clean_detail(context, result.as_ref()),
            None::<String>,
        )),
    }
}

#[cfg(unix)]
fn doctor_identity_label(identity: &SyncPeerIdentity) -> String {
    match identity {
        SyncPeerIdentity::LegacyV1 {
            legacy_machine_id_prefix,
        }
        | SyncPeerIdentity::UnknownFuture {
            legacy_machine_id_prefix,
        } => format!("legacy machine {legacy_machine_id_prefix}..."),
        SyncPeerIdentity::V2 {
            writer_id_prefix,
            run_id,
        } => format!("writer {writer_id_prefix}... run {run_id}"),
        SyncPeerIdentity::Unidentified => "unidentified heartbeat".to_owned(),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use chrono::Utc;
    use solstone_core_system::lifecycle::{
        HeartbeatClassification, HeartbeatV2, RunId, SyncCheckResult, SyncPeerObservation,
        SyncSnapshot, WriterId,
    };
    use solstone_core_system_health::SyncRescanDiagnosis;

    use super::{Check, CheckContext, Status, render_sync_rescan};

    fn context() -> CheckContext {
        CheckContext {
            home_dir: PathBuf::new(),
            install_bin_dir: PathBuf::new(),
            journal_path: PathBuf::new(),
            callosum_socket_path: PathBuf::new(),
            platform: crate::vocabulary::Platform::Linux,
            now: Utc::now(),
            host_arch: String::new(),
            hostname: "host".to_owned(),
            checkout_root: None,
            payload_root: None,
            port: 0,
            service_status_timeout: std::time::Duration::ZERO,
            service_status_command_override: None,
            parakeet_server_probe_override: None,
            speakers_analyze_resolvers: None,
            vad_runtime_probe: None,
            free_space_bytes_override: None,
        }
    }

    fn check() -> Check {
        Check {
            name: "journal_sync",
            severity: crate::vocabulary::Severity::Blocker,
            platforms: &[],
        }
    }

    #[test]
    fn rescan_diagnosis_renders_waiting_and_needs_attention_branches() {
        let context = context();
        let clean =
            render_sync_rescan(&context, check(), SyncRescanDiagnosis::Clean(None)).unwrap();
        assert_eq!(clean.status, Status::Ok);
        assert!(clean.detail.starts_with("this device only"));

        let heartbeat_needs_attention = render_sync_rescan(
            &context,
            check(),
            SyncRescanDiagnosis::HeartbeatNeedsAttention("attention copy".to_owned()),
        )
        .unwrap();
        assert_eq!(heartbeat_needs_attention.status, Status::Fail);
        assert_eq!(heartbeat_needs_attention.detail, "attention copy");

        let waiting = render_sync_rescan(
            &context,
            check(),
            SyncRescanDiagnosis::Waiting("waiting copy".to_owned()),
        )
        .unwrap();
        assert_eq!(waiting.status, Status::Warn);
        assert_eq!(waiting.detail, "waiting copy");

        let needs_attention = render_sync_rescan(
            &context,
            check(),
            SyncRescanDiagnosis::AdmissionWaitNeedsAttention("attention copy".to_owned()),
        )
        .unwrap();
        assert_eq!(needs_attention.status, Status::Fail);
        assert_eq!(needs_attention.detail, "attention copy");

        let unsafe_entry = render_sync_rescan(
            &context,
            check(),
            SyncRescanDiagnosis::Unsafe("unsafe copy".to_owned()),
        )
        .unwrap();
        assert_eq!(unsafe_entry.status, Status::Fail);
        assert_eq!(unsafe_entry.detail, "unsafe copy");

        let complete = render_sync_rescan(
            &context,
            check(),
            SyncRescanDiagnosis::Clean(Some(SyncCheckResult {
                snapshot: SyncSnapshot::default(),
                peer_observations: Vec::new(),
                live_peer_observations: Vec::new(),
            })),
        )
        .unwrap();
        assert_eq!(complete.status, Status::Ok);
    }

    #[test]
    fn clean_rescan_labels_a_v2_peer_with_writer_and_run_identity() {
        let heartbeat = HeartbeatV2::new(
            WriterId::parse("0123456789abcdef0123456789abcdef").expect("writer ID"),
            RunId::parse("fedcba9876543210fedcba9876543210").expect("run ID"),
            "foreign-host".to_owned(),
            42,
            "1234.5".to_owned(),
            "test".to_owned(),
            15,
            "/foreign-journal".to_owned(),
        );
        let result = render_sync_rescan(
            &context(),
            check(),
            SyncRescanDiagnosis::Clean(Some(SyncCheckResult {
                snapshot: SyncSnapshot::default(),
                peer_observations: vec![SyncPeerObservation {
                    source_filename: OsString::from("foreign.check"),
                    classification: HeartbeatClassification::SchemaV2(heartbeat),
                    heartbeat: None,
                    is_live: false,
                }],
                live_peer_observations: Vec::new(),
            })),
        )
        .unwrap();

        assert_eq!(result.status, Status::Ok);
        assert!(result.detail.contains("foreign-host"));
        assert!(
            result
                .detail
                .contains("writer 01234567... run fedcba9876543210fedcba9876543210")
        );
        assert!(!result.detail.contains("machine"));
    }
}
