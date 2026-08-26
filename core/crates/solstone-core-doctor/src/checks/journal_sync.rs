// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
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
                .and_then(|peer| peer.heartbeat.as_ref())
                .map_or_else(
                    || clean.to_owned(),
                    |writer| {
                        format!(
                            "{clean}\n  last foreign writer: {} (machine {}...)",
                            writer.hostname,
                            writer.machine_id.chars().take(8).collect::<String>()
                        )
                    },
                );
            Ok(make_result(check, Status::Ok, detail, None::<String>))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::Utc;
    use solstone_core_system::lifecycle::{SyncCheckResult, SyncSnapshot};
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
}
