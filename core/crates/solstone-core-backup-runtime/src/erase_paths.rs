// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The repository side of removing erased things from every backup: run the
//! journal's own restic helper (`restic-erase`, restic 0.19.0 with one added
//! command) through the maintenance runtime prune uses, so own storage and the
//! storage sol pbc operates take the same path. The helper rewrites every
//! backup holding the named paths without them, removes the stored pieces only
//! they used, and scans every stored pack before it reports success. Stock
//! restic stays the program for backup, check and restore.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::DateTime;
use serde_json::Value;

use crate::engine::{BackupServices, Runtime, restic, run_program, runtime};
use crate::teardown::owned::StorageRetention;

/// The helper's file name, beside the bundled restic.
#[cfg(windows)]
pub const HELPER: &str = "restic-erase.exe";
#[cfg(not(windows))]
pub const HELPER: &str = "restic-erase";

/// The whole removal holds the exclusive lock; after waiting up to 30 minutes
/// for it, it gets this long to find, rewrite, clean up and scan.
pub const ERASE_TIMEOUT_SECONDS: u64 = 6 * 60 * 60;
const RETRY_LOCK: &str = "30m";
const RETENTION_CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// The helper beside this journal's restic, when the payload has one.
pub fn helper(services: &BackupServices<'_>) -> Option<PathBuf> {
    let restic = services.restic_path().ok()?;
    let helper = restic.with_file_name(HELPER);
    helper.is_file().then_some(helper)
}

/// Whether this installed journal carries the helper, beside its restic: the
/// choice is offered only where the backup half can run. Windows payloads
/// don't carry it yet.
pub fn helper_installed() -> bool {
    #[cfg(windows)]
    {
        false
    }
    #[cfg(not(windows))]
    {
        std::env::current_exe()
            .ok()
            .and_then(|exe| crate::resolve::resolve_restic_member(&exe).ok())
            .is_some_and(|restic| restic.with_file_name(HELPER).is_file())
    }
}

/// The destination, opened with the maintenance credential. `Ok(None)` when
/// backup isn't set up here.
pub struct Destination {
    runtime: Runtime,
    operated: bool,
}

pub fn open(journal: &Path, services: &BackupServices<'_>) -> Result<Option<Destination>, String> {
    let operated = solstone_core_backup::get_backup_config(journal)
        .ok()
        .and_then(|config| config.get("mode").cloned())
        == Some(Value::String("operated".into()));
    Ok(runtime(journal, services, "maintenance", false)?
        .map(|runtime| Destination { runtime, operated }))
}

/// The roots a backup of this journal sits at in a snapshot: the path the
/// backup itself gives restic, in restic's tree form.
pub fn journal_root(journal: &Path) -> Option<String> {
    let resolved = std::fs::canonicalize(journal).ok()?;
    Some(crate::restic_paths::restic_tree_path(&resolved))
}

/// Where a setup restore records the place its snapshot came from. In
/// `config/`, so it is backed up and a later whole-journal restore keeps it.
pub const ORIGINS_FILE: &str = "config/backup-origins.json";

/// Record that this journal was restored from `snapshot_id`, a backup of it at
/// `path` (restic's tree form): by the founder's 11-10 ruling a restored
/// journal is the same journal, so backups at that place are its own, and an
/// erase reaches them. Write-once per path; the first snapshot named is kept.
pub fn record_origin(journal: &Path, snapshot_id: &str, path: &str) -> std::io::Result<()> {
    let file = journal.join(ORIGINS_FILE);
    let mut all = read_origins(&file)?;
    if !path.starts_with('/')
        || all
            .iter()
            .any(|origin| origin.get("path").and_then(Value::as_str) == Some(path))
    {
        return Ok(());
    }
    all.push(serde_json::json!({"path": path, "snapshot": snapshot_id}));
    let bytes = serde_json::to_vec_pretty(&serde_json::json!({"schema": 1, "origins": all}))
        .map_err(std::io::Error::other)?;
    solstone_core_journal_io::atomic::atomic_replace(
        &file,
        &bytes,
        solstone_core_journal_io::AtomicWriteOptions { mode: Some(0o600) },
    )
    .map(|_| ())
    .map_err(|error| std::io::Error::other(error.to_string()))
}

fn read_origins(file: &Path) -> std::io::Result<Vec<Value>> {
    match std::fs::read(file) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|value| value.get("origins").and_then(Value::as_array).cloned())
            .ok_or_else(|| std::io::Error::other("backup origins unreadable")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

/// The places earlier copies of this journal were backed up from, recorded by
/// setup restores, in restic's tree form. Unreadable reads as none: those
/// backups are then counted elsewhere and said to still hold it, never
/// claimed reached.
pub fn origins(journal: &Path) -> Vec<String> {
    read_origins(&journal.join(ORIGINS_FILE))
        .unwrap_or_default()
        .iter()
        .filter_map(|origin| {
            origin
                .get("path")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .filter(|path| path.starts_with('/'))
        .collect()
}

/// What one run returned.
pub struct Erased {
    /// The helper's result document.
    pub result: Value,
    pub returncode: i32,
}

impl Destination {
    pub fn operated(&self) -> bool {
        self.operated
    }

    /// Run the helper on `request`, keeping the saved piece list at `state`.
    pub fn erase(
        &self,
        services: &BackupServices<'_>,
        helper: &Path,
        request: &Path,
        state: &Path,
    ) -> Result<Erased, String> {
        let args = vec![
            "--retry-lock".into(),
            RETRY_LOCK.into(),
            "erase-paths".into(),
            "--request".into(),
            crate::restic_filesystem_path(request),
            "--state".into(),
            crate::restic_filesystem_path(state),
        ];
        let output = run_program(services, &self.runtime, helper, args, ERASE_TIMEOUT_SECONDS)
            .map_err(|_| "helper_failed_to_run".to_owned())?;
        let result = output
            .stdout
            .lines()
            .rev()
            .find_map(|line| serde_json::from_str::<Value>(line).ok())
            .ok_or_else(|| crate::runner::reason_for_returncode(output.returncode).to_owned())?;
        Ok(Erased {
            result,
            returncode: output.returncode,
        })
    }

    /// The destination's backups carrying `tag`, as `restic snapshots` lists
    /// them (id, time, tags, `original` when rewritten).
    pub fn snapshots_tagged(
        &self,
        services: &BackupServices<'_>,
        tag: &str,
    ) -> Result<Vec<Value>, String> {
        let output = restic(
            services,
            &self.runtime,
            vec!["snapshots".into(), "--tag".into(), tag.into()],
            true,
            10 * 60,
            None,
        )?;
        if output.returncode != 0 {
            return Err(crate::runner::reason_for_returncode(output.returncode).into());
        }
        output
            .json
            .as_ref()
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| "unreadable_listing".into())
    }

    /// Whether the storage keeps what restic deletes. Storage sol pbc operates
    /// keeps no versions and locks nothing (its bucket read 2026-11-15: no lock
    /// rules, and R2 keeps no versions); the owner's own S3 or B2 is asked,
    /// read-only; anything else reads unknown, never as "removes".
    pub fn storage(&self, services: &BackupServices<'_>) -> StorageRetention {
        if self.operated {
            return StorageRetention::Removes;
        }
        let repository = &self.runtime.destination.repository;
        if repository.starts_with("s3:") || repository.starts_with("b2:") {
            let Some(amz_date) =
                DateTime::<chrono::Utc>::from_timestamp(services.clock.now_unix(), 0)
                    .map(|now| now.format("%Y%m%dT%H%M%SZ").to_string())
            else {
                return StorageRetention::Unknown;
            };
            return crate::teardown::owned::retention::observe(
                &self.runtime.destination,
                services.http,
                &solstone_core_backup::archive_admission::ArchiveCancellation::new(),
                RETENTION_CHECK_TIMEOUT,
                &amz_date,
            );
        }
        StorageRetention::Unknown
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod origin_tests {
    use super::*;

    #[test]
    fn a_restore_origin_is_recorded_once_and_read_back() {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path();
        std::fs::create_dir_all(journal.join("config")).unwrap();
        assert!(origins(journal).is_empty());
        record_origin(journal, &"a".repeat(64), "/home/old/journal").unwrap();
        record_origin(journal, &"b".repeat(64), "/home/old/journal").unwrap();
        record_origin(journal, &"c".repeat(64), "relative/not/kept").unwrap();
        record_origin(journal, &"d".repeat(64), "/Users/old/journal").unwrap();
        assert_eq!(
            origins(journal),
            ["/home/old/journal", "/Users/old/journal"]
        );
        std::fs::write(journal.join(ORIGINS_FILE), b"{").unwrap();
        assert!(origins(journal).is_empty(), "unreadable reads as none");
        assert!(
            record_origin(journal, &"e".repeat(64), "/x").is_err(),
            "never overwritten"
        );
    }
}
