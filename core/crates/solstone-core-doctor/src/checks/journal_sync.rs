// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
use solstone_core_system::lifecycle::{SyncPeerIdentity, sync_peer_diagnostic};
use solstone_core_system::process::SystemProcessInstanceSource;
use solstone_core_system_health::{SyncRescanDiagnosis, describe_sync_rescan};

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
    render_sync_rescan(
        context,
        check,
        describe_sync_rescan(
            &context.journal_path,
            "doctor.check",
            context.now.timestamp() as f64,
            &process_source,
        ),
    )
}

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
        SyncRescanDiagnosis::Clean(result) => {
            let clean = format!("this device only ({})", context.hostname);
            let detail = result
                .as_ref()
                .and_then(|result| result.peer_observations.last())
                .map(sync_peer_diagnostic)
                .filter(|writer| !writer.identity.is_unidentified())
                .map_or_else(
                    || clean.to_owned(),
                    |writer| {
                        format!(
                            "{clean}\n  last foreign writer: {} ({})",
                            writer.hostname,
                            doctor_identity_label(&writer.identity)
                        )
                    },
                );
            Ok(make_result(check, Status::Ok, detail, None::<String>))
        }
    }
}

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

#[cfg(test)]
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
