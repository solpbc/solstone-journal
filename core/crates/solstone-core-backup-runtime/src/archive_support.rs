// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only transport checks shared with actual owned archive admission.

use std::{path::Path, sync::Arc};

use solstone_core_backup::{
    archive_admission::ArchiveConfiguration, archive_readback::ReadbackError,
};
use solstone_core_journal_io::JournalRoot;

use crate::{engine::ArchiveError, runner::ResticInvocation};

/// Check the selected transport against the retained root and, on mobile, the
/// current native installation. This opens no repository, asks no credential broker, creates
/// no state and grants no execution authority. The actual owner repeats these
/// checks and retains its own budget, activity, resources and settlement.
/// Desktop tool identity remains the installed creator's responsibility.
pub fn check_archive_creator_support(
    root: &Arc<JournalRoot>,
    config: &ArchiveConfiguration,
    restic: ResticInvocation<'_>,
    rclone: Option<&Path>,
) -> Result<(), ArchiveError> {
    root.revalidate_canonical_binding()
        .map_err(|_| ArchiveError::JournalIdentityChanged)?;
    config.revalidate(root)?;
    #[cfg(target_os = "ios")]
    let linked = match restic {
        ResticInvocation::Callable(authority) => {
            authority
                .revalidate_archive_root(root)
                .map_err(|_| ArchiveError::JournalIdentityChanged)?;
            true
        }
        ResticInvocation::Executable(_) => return Err(ArchiveError::ToolUnavailable),
    };
    #[cfg(not(target_os = "ios"))]
    let linked = false;
    #[cfg(target_os = "android")]
    {
        let expected = crate::resolve::android_archive_tools(config.binding().is_some())
            .map_err(|_| ArchiveError::ToolUnavailable)?;
        if restic.executable_path() != expected.restic.invocation().executable_path()
            || (config.binding().is_some() && rclone != expected.rclone_path.as_deref())
        {
            return Err(ArchiveError::Readback(ReadbackError::ToolChanged));
        }
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    let _ = restic;
    check_backend(config, linked, rclone)?;
    #[cfg(target_os = "ios")]
    if config.binding().is_none() {
        let destination = config.destination().ok_or(ArchiveError::NotConfigured)?;
        let environment =
            crate::engine::value_env(&destination).ok_or(ArchiveError::ConfigurationUnavailable)?;
        solstone_core_backup::ios_repository::parse(
            &destination.repository,
            (!environment.is_empty()).then_some(&environment),
        )
        .map_err(|_| ArchiveError::Readback(ReadbackError::UnsupportedBackend))?;
    }
    Ok(())
}

fn check_backend(
    config: &ArchiveConfiguration,
    linked: bool,
    rclone: Option<&Path>,
) -> Result<(), ArchiveError> {
    if config.binding().is_some() {
        if config.mode() != Some("operated") {
            return Err(ArchiveError::Readback(ReadbackError::UnsupportedBackend));
        }
        if !linked && rclone.is_none() {
            return Err(ArchiveError::ToolUnavailable);
        }
    } else if config.mode() != Some("byo")
        || config.destination().is_none_or(|destination| {
            if linked {
                destination.backend != "s3"
            } else {
                !matches!(destination.backend.as_str(), "s3" | "b2")
            }
        })
    {
        return Err(ArchiveError::Readback(ReadbackError::UnsupportedBackend));
    }
    if linked && rclone.is_some() {
        return Err(ArchiveError::Readback(ReadbackError::UnsupportedBackend));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn configuration(
        mode: &str,
        backend: &str,
    ) -> (tempfile::TempDir, Arc<JournalRoot>, ArchiveConfiguration) {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("config")).unwrap();
        std::fs::write(temp.path().join("config/journal.json"), json!({"backup":{
            "mode":mode,"destination":{"backend":backend,"repository":"s3:https://fixture.invalid/bucket","credentials":{}}
        }}).to_string()).unwrap();
        if mode == "operated" {
            std::fs::create_dir_all(temp.path().join("backup/hosted")).unwrap();
            std::fs::write(temp.path().join("backup/hosted/binding.json"),json!({
                "broker_endpoint":"https://fixture.invalid/credentials","account_id":"fixture-account",
                "instance_id":"fixture-instance","bucket":"fixture-bucket","prefix":"fixture-prefix/",
                "broker_token":"fixture-token"
            }).to_string()).unwrap();
        }
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let config = ArchiveConfiguration::capture(&root).unwrap();
        (temp, root, config)
    }

    #[test]
    fn maintained_creator_backend_policy_keeps_linked_and_process_transports_distinct() {
        for backend in ["s3", "b2"] {
            let (_temp, _root, config) = configuration("byo", backend);
            assert!(check_backend(&config, false, None).is_ok());
            assert_eq!(check_backend(&config, true, None).is_ok(), backend == "s3");
            assert!(check_backend(&config, true, Some(Path::new("rclone"))).is_err());
        }
        for backend in ["local", "sftp", "rest", "unknown"] {
            let (_temp, _root, config) = configuration("byo", backend);
            assert!(check_backend(&config, false, None).is_err());
            assert!(check_backend(&config, true, None).is_err());
        }
        let (_temp, _root, config) = configuration("operated", "s3");
        assert!(
            check_backend(&config, false, None).is_err(),
            "process writer needs append-only rclone"
        );
        assert!(check_backend(&config, false, Some(Path::new("rclone"))).is_ok());
        assert!(
            check_backend(&config, true, None).is_ok(),
            "linked writer owns create-only transport"
        );
        assert!(check_backend(&config, true, Some(Path::new("rclone"))).is_err());
    }

    #[test]
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    fn read_only_support_check_never_materializes_archive_or_repository_state() {
        let (temp, root, config) = configuration("byo", "s3");
        let before = std::fs::read(temp.path().join("config/journal.json")).unwrap();
        check_archive_creator_support(
            &root,
            &config,
            ResticInvocation::Executable(Path::new("never-executed")),
            None,
        )
        .unwrap();
        assert_eq!(
            std::fs::read(temp.path().join("config/journal.json")).unwrap(),
            before
        );
        let names: Vec<_> = std::fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("config")]);
        let names: Vec<_> = std::fs::read_dir(temp.path().join("config"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("journal.json")]);
    }
}
