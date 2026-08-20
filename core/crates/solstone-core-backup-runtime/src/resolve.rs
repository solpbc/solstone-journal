// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_artifact_download::ByteDownload;
use solstone_core_backup::get_backup_config;

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

    struct PanicDownload;

    impl ByteDownload for PanicDownload {
        fn fetch(&self, _: &str, _: Duration) -> Result<Vec<u8>, ByteDownloadError> {
            panic!("must not download")
        }
    }

    struct FailingDownload {
        calls: Cell<u32>,
    }

    impl ByteDownload for FailingDownload {
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
}
