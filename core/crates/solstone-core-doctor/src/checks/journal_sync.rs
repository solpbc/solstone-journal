// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
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
    render_sync_rescan(
        context,
        check,
        describe_sync_rescan(
            &context.journal_path,
            "doctor.check",
            context.machine_id.as_deref().unwrap_or(""),
            context.now.timestamp() as f64,
        ),
    )
}

fn render_sync_rescan(
    context: &CheckContext,
    check: Check,
    diagnosis: SyncRescanDiagnosis,
) -> RunnerResult {
    match diagnosis {
        SyncRescanDiagnosis::Conflict(message) => {
            Ok(make_result(check, Status::Fail, message, None::<String>))
        }
        SyncRescanDiagnosis::Unsafe(message) => {
            Ok(make_result(check, Status::Fail, message, None::<String>))
        }
        SyncRescanDiagnosis::Clean(result) => {
            let prefix = context
                .machine_id
                .as_deref()
                .map(|value| value.chars().take(8).collect::<String>())
                .unwrap_or_else(|| "(unknown)".into());
            let clean = format!(
                "this device only ({}, machine {}...)",
                context.hostname, prefix
            );
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
            machine_id: Some("12345678abcdef".to_owned()),
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
    fn rescan_diagnosis_has_clean_conflict_and_unsafe_doctor_branches() {
        let context = context();
        let clean =
            render_sync_rescan(&context, check(), SyncRescanDiagnosis::Clean(None)).unwrap();
        assert_eq!(clean.status, Status::Ok);
        assert!(clean.detail.starts_with("this device only"));

        let conflict = render_sync_rescan(
            &context,
            check(),
            SyncRescanDiagnosis::Conflict("conflict copy".to_owned()),
        )
        .unwrap();
        assert_eq!(conflict.status, Status::Fail);
        assert_eq!(conflict.detail, "conflict copy");

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
