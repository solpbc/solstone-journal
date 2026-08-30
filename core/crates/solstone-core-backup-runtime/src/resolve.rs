// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_artifact_download::ByteDownload;
use solstone_core_backup::get_backup_config;

use crate::engine::{AdmittedBackupMode, AdmittedCapability, ClosedToolError};
use crate::install::ensure_restic;
use crate::rclone_install::ensure_rclone;
use crate::runner::ToolRunner;

/// Optional install directories. Production always uses `Default` (HOME-based).
#[derive(Default)]
pub struct ToolInstallDirs<'a> {
    pub restic: Option<&'a Path>,
    pub rclone: Option<&'a Path>,
}

/// Absolute paths of pinned restic and, when required, rclone binaries.
#[derive(Debug)]
pub struct ResolvedTools {
    pub restic_path: PathBuf,
    pub rclone_path: Option<PathBuf>,
}

/// Resolve the pinned tools required by an already-admitted backup capability.
pub fn resolve_tools(
    capability: &AdmittedCapability,
    runner: &dyn ToolRunner,
    downloader: &dyn ByteDownload,
    dirs: ToolInstallDirs<'_>,
) -> Result<ResolvedTools, ClosedToolError> {
    let restic_path = ensure_restic(runner, false, dirs.restic, downloader)
        .map_err(|_| ClosedToolError::ResticUnavailable)?;
    let rclone_path = match &capability.mode {
        AdmittedBackupMode::Byo { .. } => None,
        AdmittedBackupMode::Operated { .. } => Some(
            ensure_rclone(runner, false, dirs.rclone, downloader)
                .map_err(|_| ClosedToolError::RcloneUnavailable)?,
        ),
    };
    Ok(ResolvedTools {
        restic_path,
        rclone_path,
    })
}

/// Resolve the pinned restic binary, and rclone when this journal needs an
/// operated append-only session.
pub fn resolve_operational_tools(
    runner: &dyn ToolRunner,
    downloader: &dyn ByteDownload,
    journal: &Path,
    append_only: bool,
    dirs: ToolInstallDirs<'_>,
) -> Result<ResolvedTools, String> {
    let restic_path = ensure_restic(runner, false, dirs.restic, downloader)
        .map_err(|_| "restic_unavailable".to_owned())?;
    let rclone_path = if append_only && journal_is_operated(journal) {
        Some(
            ensure_rclone(runner, false, dirs.rclone, downloader)
                .map_err(|_| "rclone_unavailable".to_owned())?,
        )
    } else {
        None
    };
    Ok(ResolvedTools {
        restic_path,
        rclone_path,
    })
}

fn journal_is_operated(journal: &Path) -> bool {
    let Ok(config) = get_backup_config(journal) else {
        return false;
    };
    config.get("enabled") == Some(&Value::Bool(true))
        && config.get("mode") == Some(&Value::String("operated".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Clock, prepare, reset_backup_path_resolution_attempts};
    use crate::rclone_install::{RCLONE_SCHEMA_VERSION, RCLONE_TOOL, RCLONE_VERSION};
    use crate::readiness::{
        RESTIC_SCHEMA_VERSION, RESTIC_TOOL, RESTIC_VERSION, binary_path, file_sha256,
        platform_info, sentinel_path,
    };
    use crate::runner::{ToolOutput, ToolRequest};
    use solstone_core_artifact_download::ByteDownloadError;
    use std::cell::Cell;
    use std::fs;
    use std::io;
    use std::time::Duration;

    struct ReadyRunner;

    impl ToolRunner for ReadyRunner {
        fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            let name = Path::new(&request.program)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            let stdout = if name == RCLONE_TOOL {
                format!("rclone v{RCLONE_VERSION}\n")
            } else {
                format!("restic {RESTIC_VERSION}\n")
            };
            Ok(ToolOutput {
                returncode: 0,
                stdout: stdout.into_bytes(),
                stderr: vec![],
            })
        }
    }

    struct CountingReadyRunner {
        calls: Cell<u32>,
    }

    impl ToolRunner for CountingReadyRunner {
        fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            self.calls.set(self.calls.get() + 1);
            ReadyRunner.run(request)
        }
    }

    struct PanicDownload;

    impl ByteDownload for PanicDownload {
        fn fetch(&self, _: &str, _: Duration) -> Result<Vec<u8>, ByteDownloadError> {
            panic!("must not download")
        }
    }

    struct FailingDownload {
        calls: Cell<u32>,
    }

    struct CountingDownload {
        calls: Cell<u32>,
    }

    struct FixedClock;

    impl Clock for FixedClock {
        fn now_unix(&self) -> i64 {
            50
        }

        fn iso_week(&self) -> u8 {
            7
        }
    }

    impl ByteDownload for FailingDownload {
        fn fetch(&self, _: &str, _: Duration) -> Result<Vec<u8>, ByteDownloadError> {
            self.calls.set(self.calls.get() + 1);
            Err(ByteDownloadError::Transport)
        }
    }

    impl ByteDownload for CountingDownload {
        fn fetch(&self, _: &str, _: Duration) -> Result<Vec<u8>, ByteDownloadError> {
            self.calls.set(self.calls.get() + 1);
            Err(ByteDownloadError::Transport)
        }
    }

    fn write_ready_restic(dir: &Path) -> PathBuf {
        let binary = binary_path(dir);
        fs::write(&binary, b"restic-fixture").unwrap();
        let digest = file_sha256(&binary).unwrap();
        let (os, arch) = platform_info().unwrap();
        fs::write(
            sentinel_path(dir),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": RESTIC_SCHEMA_VERSION,
                "tool": RESTIC_TOOL,
                "version": RESTIC_VERSION,
                "sha256": digest,
                "platform": {"os": os, "arch": arch},
                "binary_path": binary,
            }))
            .unwrap(),
        )
        .unwrap();
        binary
    }

    fn write_ready_rclone(dir: &Path) -> PathBuf {
        let binary = dir.join(RCLONE_TOOL);
        fs::write(&binary, b"rclone-fixture").unwrap();
        let digest = file_sha256(&binary).unwrap();
        let (os, arch) = platform_info().unwrap();
        fs::write(
            dir.join(".install-complete"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": RCLONE_SCHEMA_VERSION,
                "tool": RCLONE_TOOL,
                "version": RCLONE_VERSION,
                "sha256": digest,
                "platform": {"os": os, "arch": arch},
                "binary_path": binary,
            }))
            .unwrap(),
        )
        .unwrap();
        binary
    }

    fn byo_journal() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn operated_journal(enabled: bool) -> tempfile::TempDir {
        let journal = tempfile::tempdir().unwrap();
        solstone_core_backup::set_mode(journal.path(), "operated").unwrap();
        solstone_core_backup::set_enabled(journal.path(), enabled).unwrap();
        journal
    }

    fn configured_byo_journal() -> tempfile::TempDir {
        let journal = tempfile::tempdir().unwrap();
        let destination = solstone_core_backup::Destination {
            repository: "s3:repo".into(),
            backend: "s3".into(),
            credentials: serde_json::json!({
                "access_key_id": "access",
                "secret_access_key": "secret",
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        solstone_core_backup::set_destination(journal.path(), &destination).unwrap();
        solstone_core_backup::generate_and_store_keys(journal.path()).unwrap();
        solstone_core_backup::set_enabled(journal.path(), true).unwrap();
        journal
    }

    fn operated_capability(journal: &Path, clock: &FixedClock) -> AdmittedCapability {
        solstone_core_backup::set_mode(journal, "operated").unwrap();
        solstone_core_backup::save_hosted_binding(
            journal,
            &solstone_core_backup::HostedBinding {
                broker_endpoint: "https://broker".into(),
                account_id: "account".into(),
                instance_id: "instance".into(),
                bucket: "bucket".into(),
                prefix: "prefix".into(),
                broker_token: "token".into(),
            },
        )
        .unwrap();
        prepare(journal, clock).unwrap()
    }

    fn dirs<'a>(restic: &'a Path, rclone: Option<&'a Path>) -> ToolInstallDirs<'a> {
        ToolInstallDirs {
            restic: Some(restic),
            rclone,
        }
    }

    #[test]
    fn restic_resolves_from_ready_dir() {
        let restic_dir = tempfile::tempdir().unwrap();
        let expected = write_ready_restic(restic_dir.path());
        let journal = byo_journal();
        let tools = resolve_operational_tools(
            &ReadyRunner,
            &PanicDownload,
            journal.path(),
            true,
            dirs(restic_dir.path(), None),
        )
        .unwrap();
        assert_eq!(tools.restic_path, expected);
        assert_eq!(tools.rclone_path, None);
    }

    #[test]
    fn byo_append_only_does_not_resolve_rclone() {
        let restic_dir = tempfile::tempdir().unwrap();
        write_ready_restic(restic_dir.path());
        let journal = byo_journal();
        let tools = resolve_operational_tools(
            &ReadyRunner,
            &PanicDownload,
            journal.path(),
            true,
            dirs(restic_dir.path(), None),
        )
        .unwrap();
        assert_eq!(tools.rclone_path, None);
    }

    #[test]
    fn operated_enabled_append_only_resolves_rclone() {
        let restic_dir = tempfile::tempdir().unwrap();
        let rclone_dir = tempfile::tempdir().unwrap();
        write_ready_restic(restic_dir.path());
        let expected_rclone = write_ready_rclone(rclone_dir.path());
        let journal = operated_journal(true);
        let tools = resolve_operational_tools(
            &ReadyRunner,
            &PanicDownload,
            journal.path(),
            true,
            dirs(restic_dir.path(), Some(rclone_dir.path())),
        )
        .unwrap();
        assert_eq!(
            tools.rclone_path.as_deref(),
            Some(expected_rclone.as_path())
        );
    }

    #[test]
    fn operated_enabled_without_append_only_does_not_resolve_rclone() {
        let restic_dir = tempfile::tempdir().unwrap();
        write_ready_restic(restic_dir.path());
        let journal = operated_journal(true);
        let tools = resolve_operational_tools(
            &ReadyRunner,
            &PanicDownload,
            journal.path(),
            false,
            dirs(restic_dir.path(), None),
        )
        .unwrap();
        assert_eq!(tools.rclone_path, None);
    }

    #[test]
    fn operated_disabled_append_only_does_not_resolve_rclone() {
        let restic_dir = tempfile::tempdir().unwrap();
        write_ready_restic(restic_dir.path());
        let journal = operated_journal(false);
        let tools = resolve_operational_tools(
            &ReadyRunner,
            &PanicDownload,
            journal.path(),
            true,
            dirs(restic_dir.path(), None),
        )
        .unwrap();
        assert_eq!(tools.rclone_path, None);
    }

    #[test]
    fn restic_install_failure_is_restic_unavailable() {
        let restic_dir = tempfile::tempdir().unwrap();
        let journal = byo_journal();
        let downloader = FailingDownload {
            calls: Cell::new(0),
        };
        let error = resolve_operational_tools(
            &ReadyRunner,
            &downloader,
            journal.path(),
            false,
            dirs(restic_dir.path(), None),
        )
        .unwrap_err();
        assert_eq!(error, "restic_unavailable");
        assert!(downloader.calls.get() > 0);
    }

    #[test]
    fn rclone_install_failure_is_rclone_unavailable() {
        let restic_dir = tempfile::tempdir().unwrap();
        let rclone_dir = tempfile::tempdir().unwrap();
        write_ready_restic(restic_dir.path());
        let journal = operated_journal(true);
        let downloader = FailingDownload {
            calls: Cell::new(0),
        };
        let error = resolve_operational_tools(
            &ReadyRunner,
            &downloader,
            journal.path(),
            true,
            dirs(restic_dir.path(), Some(rclone_dir.path())),
        )
        .unwrap_err();
        assert_eq!(error, "rclone_unavailable");
        assert!(downloader.calls.get() > 0);
    }

    #[test]
    fn capability_byo_resolves_only_restic() {
        let restic_dir = tempfile::tempdir().unwrap();
        let expected = write_ready_restic(restic_dir.path());
        let journal = configured_byo_journal();
        let clock = FixedClock;

        let capability = prepare(journal.path(), &clock).unwrap();
        let tools = resolve_tools(
            &capability,
            &ReadyRunner,
            &PanicDownload,
            dirs(restic_dir.path(), None),
        )
        .unwrap();

        assert_eq!(tools.restic_path, expected);
        assert_eq!(tools.rclone_path, None);
        assert!(matches!(&capability.mode, AdmittedBackupMode::Byo { .. }));
    }

    #[test]
    fn capability_operated_resolves_restic_and_rclone() {
        let restic_dir = tempfile::tempdir().unwrap();
        let rclone_dir = tempfile::tempdir().unwrap();
        let expected_restic = write_ready_restic(restic_dir.path());
        let expected_rclone = write_ready_rclone(rclone_dir.path());
        let journal = configured_byo_journal();
        let clock = FixedClock;
        let capability = operated_capability(journal.path(), &clock);

        let tools = resolve_tools(
            &capability,
            &ReadyRunner,
            &PanicDownload,
            dirs(restic_dir.path(), Some(rclone_dir.path())),
        )
        .unwrap();

        assert_eq!(tools.restic_path, expected_restic);
        assert_eq!(
            tools.rclone_path.as_deref(),
            Some(expected_rclone.as_path())
        );
    }

    #[test]
    fn capability_tool_resolution_uses_admitted_mode_after_config_changes() {
        let restic_dir = tempfile::tempdir().unwrap();
        write_ready_restic(restic_dir.path());
        let journal = configured_byo_journal();
        let clock = FixedClock;
        let capability = prepare(journal.path(), &clock).unwrap();
        solstone_core_backup::set_mode(journal.path(), "operated").unwrap();

        let tools = resolve_tools(
            &capability,
            &ReadyRunner,
            &PanicDownload,
            dirs(restic_dir.path(), None),
        )
        .unwrap();

        assert_eq!(tools.rclone_path, None);
    }

    #[test]
    fn operated_capability_still_resolves_rclone_after_config_changes_to_byo() {
        let restic_dir = tempfile::tempdir().unwrap();
        let rclone_dir = tempfile::tempdir().unwrap();
        write_ready_restic(restic_dir.path());
        let expected_rclone = write_ready_rclone(rclone_dir.path());
        let journal = configured_byo_journal();
        let clock = FixedClock;
        let capability = operated_capability(journal.path(), &clock);
        solstone_core_backup::set_mode(journal.path(), "byo").unwrap();

        let tools = resolve_tools(
            &capability,
            &ReadyRunner,
            &PanicDownload,
            dirs(restic_dir.path(), Some(rclone_dir.path())),
        )
        .unwrap();

        assert_eq!(
            tools.rclone_path.as_deref(),
            Some(expected_rclone.as_path())
        );
    }

    #[test]
    fn capability_tool_resolution_maps_restic_unavailable() {
        let restic_dir = tempfile::tempdir().unwrap();
        let journal = configured_byo_journal();
        let clock = FixedClock;
        let downloader = FailingDownload {
            calls: Cell::new(0),
        };
        let capability = prepare(journal.path(), &clock).unwrap();

        let error = resolve_tools(
            &capability,
            &ReadyRunner,
            &downloader,
            dirs(restic_dir.path(), None),
        )
        .unwrap_err();

        assert!(matches!(error, ClosedToolError::ResticUnavailable));
        assert!(downloader.calls.get() > 0);
    }

    #[test]
    fn capability_tool_resolution_maps_operated_rclone_unavailable() {
        let restic_dir = tempfile::tempdir().unwrap();
        let rclone_dir = tempfile::tempdir().unwrap();
        write_ready_restic(restic_dir.path());
        let journal = configured_byo_journal();
        let clock = FixedClock;
        let downloader = FailingDownload {
            calls: Cell::new(0),
        };
        let capability = operated_capability(journal.path(), &clock);

        let error = resolve_tools(
            &capability,
            &ReadyRunner,
            &downloader,
            dirs(restic_dir.path(), Some(rclone_dir.path())),
        )
        .unwrap_err();

        assert!(matches!(error, ClosedToolError::RcloneUnavailable));
        assert!(downloader.calls.get() > 0);
    }

    #[test]
    fn dropping_capability_after_tool_resolution_leaves_state_unchanged() {
        let restic_dir = tempfile::tempdir().unwrap();
        let restic_binary = write_ready_restic(restic_dir.path());
        let restic_sentinel = sentinel_path(restic_dir.path());
        let rclone_dir = tempfile::tempdir().unwrap();
        let rclone_binary = write_ready_rclone(rclone_dir.path());
        let rclone_sentinel = rclone_dir.path().join(".install-complete");
        let journal = configured_byo_journal();
        let config = journal.path().join("config/journal.json");
        let clock = FixedClock;
        let runner = CountingReadyRunner {
            calls: Cell::new(0),
        };
        let downloader = CountingDownload {
            calls: Cell::new(0),
        };
        reset_backup_path_resolution_attempts();
        let capability = operated_capability(journal.path(), &clock);
        let tools = resolve_tools(
            &capability,
            &runner,
            &downloader,
            dirs(restic_dir.path(), Some(rclone_dir.path())),
        )
        .unwrap();
        let config_before = fs::read(&config).unwrap();
        let restic_binary_before = fs::read(&restic_binary).unwrap();
        let restic_sentinel_before = fs::read(&restic_sentinel).unwrap();
        let rclone_binary_before = fs::read(&rclone_binary).unwrap();
        let rclone_sentinel_before = fs::read(&rclone_sentinel).unwrap();
        let runner_calls = runner.calls.get();
        let downloader_calls = downloader.calls.get();

        drop(capability);
        assert_eq!(fs::read(&config).unwrap(), config_before);
        assert_eq!(fs::read(&restic_binary).unwrap(), restic_binary_before);
        assert_eq!(fs::read(&restic_sentinel).unwrap(), restic_sentinel_before);
        assert_eq!(fs::read(&rclone_binary).unwrap(), rclone_binary_before);
        assert_eq!(fs::read(&rclone_sentinel).unwrap(), rclone_sentinel_before);
        assert_eq!(runner.calls.get(), runner_calls);
        assert_eq!(downloader.calls.get(), downloader_calls);
        assert_eq!(crate::engine::backup_path_resolution_attempts(), 1);

        drop(tools);
        assert_eq!(fs::read(&config).unwrap(), config_before);
        assert_eq!(fs::read(&restic_binary).unwrap(), restic_binary_before);
        assert_eq!(fs::read(&restic_sentinel).unwrap(), restic_sentinel_before);
        assert_eq!(fs::read(&rclone_binary).unwrap(), rclone_binary_before);
        assert_eq!(fs::read(&rclone_sentinel).unwrap(), rclone_sentinel_before);
        assert_eq!(runner.calls.get(), runner_calls);
        assert_eq!(downloader.calls.get(), downloader_calls);
        assert_eq!(crate::engine::backup_path_resolution_attempts(), 1);
    }
}
