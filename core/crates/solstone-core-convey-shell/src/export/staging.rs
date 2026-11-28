// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Private out-of-journal staging directory, scratch-volume space accounting,
//! persisted `ready.json` metadata, and crash/orphan cleanup.

#![allow(
    clippy::disallowed_methods,
    reason = "export staging manages its own private temporary directory strictly outside the journal root"
)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use solstone_core_journal_archive::ExportMode;
use solstone_core_journal_io::AtomicWriteOptions;
use solstone_core_journal_io::atomic::atomic_replace;
use solstone_core_journal_io::disk_space::{
    JournalDiskSpace, journal_disk_floor_bytes, read_journal_disk_space, same_filesystem_volume,
};
use solstone_core_system::storage_privacy::{ensure_private_owner_dir, probe_private_owner_dir};
use solstone_core_trash::ErasureEpoch;

/// Fallback safety floor (1 GiB) only when volume total size is unknown (`0`).
pub(crate) const FALLBACK_FLOOR_BYTES: u64 = 1024 * 1024 * 1024;

/// Filename of persisted metadata inside the private staging directory.
pub(crate) const READY_META_FILE: &str = "ready.json";

/// Effective free-space floor that staging must preserve on its volume:
/// matches the journal's shared low-space floor (`min(20 GB, total / 4)`),
/// falling back to 1 GiB only when total volume size is reported as zero.
pub(crate) fn staging_disk_floor_bytes(total_bytes: u64) -> u64 {
    if total_bytes == 0 {
        FALLBACK_FLOOR_BYTES
    } else {
        journal_disk_floor_bytes(total_bytes)
    }
}

/// Compute required free bytes on the staging volume so packing preserves the
/// journal's low-space floor: `estimated_bytes + staging_disk_floor_bytes(total_bytes)`.
pub(crate) fn required_staging_bytes(estimated_bytes: u64, total_bytes: u64) -> u64 {
    estimated_bytes.saturating_add(staging_disk_floor_bytes(total_bytes))
}

/// Authorized requester owning a prepared copy or in-flight job.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum RequesterScope {
    Local,
    LinkedDevice { cid: String },
}

/// Lightweight snapshot of journal modification markers captured at `t=0` so
/// `/app/export/api/state` can truthfully report `contents_changed_since`.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ChangeFingerprint {
    pub(crate) events_size: u64,
    pub(crate) chronicle_days: usize,
    pub(crate) newest_day: Option<String>,
    pub(crate) newest_stream_marker: Option<String>,
}

impl ChangeFingerprint {
    pub(crate) fn capture(journal: &Path) -> Self {
        let events_size = fs::metadata(journal.join("events.jsonl"))
            .map(|m| m.len())
            .unwrap_or(0);
        let mut days = Vec::new();
        if let Ok(entries) = fs::read_dir(journal.join("chronicle")) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str()
                    && name.len() == 8
                    && name.bytes().all(|b| b.is_ascii_digit())
                    && entry.file_type().is_ok_and(|ft| ft.is_dir())
                {
                    days.push(name.to_owned());
                }
            }
        }
        days.sort();
        let chronicle_days = days.len();
        let newest_day = days.pop();
        let newest_stream_marker = newest_day.as_ref().and_then(|day| {
            fs::read_to_string(
                journal
                    .join("chronicle")
                    .join(day)
                    .join("health")
                    .join("stream.updated"),
            )
            .ok()
        });
        Self {
            events_size,
            chronicle_days,
            newest_day,
            newest_stream_marker,
        }
    }
}

/// Persisted metadata for an immutable prepared export copy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ReadyRecord {
    pub(crate) schema: u32,
    pub(crate) export_id: String,
    pub(crate) mode: ExportMode,
    pub(crate) filename: String,
    pub(crate) zip_filename: String,
    pub(crate) size_bytes: u64,
    pub(crate) sha256: String,
    pub(crate) created_at_epoch_secs: i64,
    pub(crate) last_activity_epoch_secs: i64,
    pub(crate) owner: RequesterScope,
    pub(crate) erasure_epoch: ErasureEpoch,
    pub(crate) fingerprint: ChangeFingerprint,
    #[serde(default)]
    pub(crate) days_count: u64,
    #[serde(default)]
    pub(crate) files_count: u64,
    #[serde(default)]
    pub(crate) omitted_media_files: u64,
    #[serde(default)]
    pub(crate) omitted_media_bytes: u64,
    #[serde(default)]
    pub(crate) offloaded_days: u64,
    #[serde(default)]
    pub(crate) offloaded_bytes: u64,
    #[serde(default)]
    pub(crate) offload_degraded: bool,
    #[serde(default)]
    pub(crate) skipped_symlink_count: u64,
    #[serde(default)]
    pub(crate) skipped_symlinks: Vec<String>,
}

impl ReadyRecord {
    pub(crate) const SCHEMA: u32 = 1;

    pub(crate) fn strong_etag(&self) -> String {
        format!("\"{}\"", self.sha256)
    }

    pub(crate) fn zip_path(&self, staging_dir: &Path) -> PathBuf {
        staging_dir.join(&self.zip_filename)
    }
}

/// Resolved private staging directory strictly outside the journal root.
#[derive(Clone, Debug)]
pub(crate) struct StagingArea {
    pub(crate) dir: PathBuf,
    pub(crate) same_volume_as_journal: bool,
}

impl StagingArea {
    /// Resolve and create an owner-private staging directory strictly outside `journal_root`.
    ///
    /// Prefers `<journal_parent>/.<journal_name>.export-staging` so staging
    /// stays on the journal's own volume; falls back to an isolated per-journal,
    /// per-account directory inside `std::env::temp_dir()` when the journal
    /// parent is unwritable. Validates non-symlink/non-reparse plain directory,
    /// current-account ownership, and private permissions/DACL. Fails closed
    /// (`Err`) rather than falling back to any shared directory.
    pub(crate) fn resolve(journal_root: &Path) -> io::Result<Self> {
        Self::resolve_with_volume_check(journal_root, same_filesystem)
    }

    pub(crate) fn resolve_with_volume_check<F>(
        journal_root: &Path,
        same_volume: F,
    ) -> io::Result<Self>
    where
        F: Fn(&Path, &Path) -> bool,
    {
        let canonical = fs::canonicalize(journal_root)?;
        if let Some(candidate) = sibling_path_for_canonical(&canonical)
            && ensure_private_dir(&candidate).is_ok()
            && fs::canonicalize(&candidate).is_ok_and(|c| !c.starts_with(&canonical))
        {
            let same_vol = same_volume(&canonical, &candidate);
            return Ok(Self {
                dir: candidate,
                same_volume_as_journal: same_vol,
            });
        }
        let fallback = temp_fallback_path_for_canonical(&canonical)?;
        ensure_private_dir(&fallback)?;
        let canonical_fallback = fs::canonicalize(&fallback)?;
        if canonical_fallback.starts_with(&canonical) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "export staging path resolves inside the journal root",
            ));
        }
        let same_vol = same_volume(&canonical, &fallback);
        Ok(Self {
            dir: fallback,
            same_volume_as_journal: same_vol,
        })
    }

    /// Resolve where private staging will live WITHOUT creating any directory.
    ///
    /// Uses the same candidate order and privacy/containment checks as
    /// [`Self::resolve`]: prefers `<journal_parent>/.<journal_name>.export-staging`,
    /// falls back to the per-journal, per-account temp directory when the sibling
    /// path is blocked or its parent cannot host a private directory, and fails
    /// closed (`Err`) if neither is usable.
    pub(crate) fn probe(journal_root: &Path) -> io::Result<Self> {
        Self::probe_with_volume_check(journal_root, same_filesystem)
    }

    pub(crate) fn probe_with_volume_check<F>(
        journal_root: &Path,
        same_volume: F,
    ) -> io::Result<Self>
    where
        F: Fn(&Path, &Path) -> bool,
    {
        let canonical = fs::canonicalize(journal_root)?;
        if let Some(candidate) = sibling_path_for_canonical(&canonical)
            && let Ok(area) = probe_candidate(&canonical, candidate, &same_volume)
        {
            return Ok(area);
        }
        let fallback = temp_fallback_path_for_canonical(&canonical)?;
        probe_candidate(&canonical, fallback, &same_volume)
    }

    /// Discover already-existing valid private staging directories for
    /// `journal_root` (sibling first, then per-journal temp fallback) without
    /// creating any new directory.
    pub(crate) fn discover_existing(journal_root: &Path) -> Vec<Self> {
        let Ok(canonical) = fs::canonicalize(journal_root) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        if let Some(candidate) = sibling_path_for_canonical(&canonical)
            && let Some(area) = open_existing_private(&canonical, candidate)
        {
            found.push(area);
        }
        if let Ok(fallback) = temp_fallback_path_for_canonical(&canonical)
            && let Some(area) = open_existing_private(&canonical, fallback)
        {
            found.push(area);
        }
        found
    }

    /// Read-only discovery of already-existing staging directories for
    /// `journal_root` without creating or modifying anything on disk.
    pub(crate) fn find_existing_readonly(journal_root: &Path) -> Vec<Self> {
        let Ok(canonical) = fs::canonicalize(journal_root) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        if let Some(candidate) = sibling_path_for_canonical(&canonical)
            && let Some(area) = inspect_existing_readonly(&canonical, candidate)
        {
            found.push(area);
        }
        if let Ok(fallback) = temp_fallback_path_for_canonical(&canonical)
            && let Some(area) = inspect_existing_readonly(&canonical, fallback)
        {
            found.push(area);
        }
        found
    }

    /// Ensure this staging directory exists on disk with owner-private access.
    pub(crate) fn ensure_dir(&self) -> io::Result<()> {
        ensure_private_dir(&self.dir)
    }

    #[cfg(test)]
    pub(crate) fn temp_fallback_path(journal_root: &Path) -> io::Result<PathBuf> {
        let canonical = fs::canonicalize(journal_root)?;
        temp_fallback_path_for_canonical(&canonical)
    }

    /// Construct a staging area at an explicit directory (used in tests).
    #[cfg(test)]
    pub(crate) fn at_dir(dir: PathBuf, same_volume_as_journal: bool) -> io::Result<Self> {
        ensure_private_dir(&dir)?;
        Ok(Self {
            dir,
            same_volume_as_journal,
        })
    }

    pub(crate) fn disk_space(&self) -> io::Result<JournalDiskSpace> {
        match read_journal_disk_space(&self.dir) {
            Ok(space) => Ok(space),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let parent = self.dir.parent().ok_or(error)?;
                read_journal_disk_space(parent)
            }
            Err(error) => Err(error),
        }
    }

    /// Remove any orphaned `.part` files left by an interrupted process, and
    /// remove any unreferenced `.zip` files not matching `keep_zip`.
    pub(crate) fn sweep_orphans(&self, keep_zip: Option<&str>) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if name == READY_META_FILE || name.ends_with(".lock") {
                continue;
            }
            if name.ends_with(".part") || name.ends_with(".tmp") {
                let _ = fs::remove_file(&path);
                continue;
            }
            if name.ends_with(".zip") && keep_zip != Some(name.as_str()) {
                let _ = fs::remove_file(&path);
            }
        }
    }

    pub(crate) fn write_ready_record(&self, record: &ReadyRecord) -> io::Result<()> {
        let mut bytes = serde_json::to_vec_pretty(record)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        bytes.push(b'\n');
        atomic_replace(
            self.dir.join(READY_META_FILE),
            &bytes,
            AtomicWriteOptions { mode: Some(0o600) },
        )
        .map_err(|error| io::Error::other(error.to_string()))
    }

    pub(crate) fn read_ready_record(&self) -> Option<ReadyRecord> {
        let bytes = fs::read(self.dir.join(READY_META_FILE)).ok()?;
        let record: ReadyRecord = serde_json::from_slice(&bytes).ok()?;
        if record.schema != ReadyRecord::SCHEMA {
            return None;
        }
        if record.zip_filename.contains('/')
            || record.zip_filename.contains('\\')
            || record.zip_filename.contains("..")
            || !record.zip_filename.ends_with(".zip")
        {
            return None;
        }
        let meta = fs::symlink_metadata(record.zip_path(&self.dir)).ok()?;
        if !meta.file_type().is_file() || meta.len() != record.size_bytes {
            return None;
        }
        Some(record)
    }

    pub(crate) fn clear_ready_files_only(&self, record: Option<&ReadyRecord>) {
        let _ = fs::remove_file(self.dir.join(READY_META_FILE));
        if let Some(rec) = record {
            let _ = fs::remove_file(rec.zip_path(&self.dir));
        }
        self.sweep_orphans(None);
    }

    pub(crate) fn clear_ready_files(&self, record: Option<&ReadyRecord>) {
        self.clear_ready_files_only(record);
        self.remove_dir_if_empty();
    }

    /// Remove the staging directory if and only if it is completely empty.
    ///
    /// Uses non-recursive `fs::remove_dir`, so any unexpected entry, active
    /// staging artifact, or mountpoint prevents removal safely.
    pub(crate) fn remove_dir_if_empty(&self) {
        let _ = fs::remove_dir(&self.dir);
    }
}

fn sibling_path_for_canonical(canonical: &Path) -> Option<PathBuf> {
    let parent = canonical.parent()?;
    let name = canonical.file_name()?;
    let mut dir_name = std::ffi::OsString::from(".");
    dir_name.push(name);
    dir_name.push(".export-staging");
    Some(parent.join(dir_name))
}

fn probe_candidate<F>(
    canonical_journal: &Path,
    candidate: PathBuf,
    same_volume: &F,
) -> io::Result<StagingArea>
where
    F: Fn(&Path, &Path) -> bool,
{
    let exists = probe_private_owner_dir(&candidate)?;
    if exists {
        let canonical_candidate = fs::canonicalize(&candidate)?;
        if canonical_candidate.starts_with(canonical_journal) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "export staging path resolves inside the journal root",
            ));
        }
        let same_vol = same_volume(canonical_journal, &candidate);
        return Ok(StagingArea {
            dir: candidate,
            same_volume_as_journal: same_vol,
        });
    }
    let parent = candidate.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "export staging path has no parent directory",
        )
    })?;
    let name = candidate.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "export staging path has no directory name",
        )
    })?;
    let canonical_parent = fs::canonicalize(parent)?;
    if canonical_parent.join(name).starts_with(canonical_journal) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "export staging path resolves inside the journal root",
        ));
    }
    // `candidate` does not exist yet; a new plain directory created inside
    // `parent` lives on `parent`'s filesystem volume.
    let same_vol = same_volume(canonical_journal, &canonical_parent);
    Ok(StagingArea {
        dir: candidate,
        same_volume_as_journal: same_vol,
    })
}

fn open_existing_private(canonical_journal: &Path, candidate: PathBuf) -> Option<StagingArea> {
    if !probe_private_owner_dir(&candidate).ok()? {
        return None;
    }
    ensure_private_dir(&candidate).ok()?;
    if fs::canonicalize(&candidate)
        .ok()?
        .starts_with(canonical_journal)
    {
        return None;
    }
    let same_volume_as_journal = same_filesystem(canonical_journal, &candidate);
    Some(StagingArea {
        dir: candidate,
        same_volume_as_journal,
    })
}

fn inspect_existing_readonly(canonical_journal: &Path, candidate: PathBuf) -> Option<StagingArea> {
    if !probe_private_owner_dir(&candidate).ok()? {
        return None;
    }
    if fs::canonicalize(&candidate)
        .ok()?
        .starts_with(canonical_journal)
    {
        return None;
    }
    let same_volume_as_journal = same_filesystem(canonical_journal, &candidate);
    Some(StagingArea {
        dir: candidate,
        same_volume_as_journal,
    })
}

fn current_account_tag() -> io::Result<String> {
    #[cfg(unix)]
    {
        Ok(format!("uid:{}", nix::unistd::geteuid().as_raw()))
    }
    #[cfg(windows)]
    {
        solstone_core_local_endpoint::owner_sid().map(|sid| format!("sid:{sid}"))
    }
}

fn temp_fallback_path_for_canonical(canonical: &Path) -> io::Result<PathBuf> {
    use sha2::{Digest, Sha256};
    let account = current_account_tag()?;
    let mut hasher = Sha256::new();
    hasher.update(account.as_bytes());
    hasher.update(b"\0");
    hasher.update(canonical.to_string_lossy().as_bytes());
    let hash_hex = format!("{:x}", hasher.finalize());
    Ok(std::env::temp_dir().join(format!("solstone-export-staging-{}", &hash_hex[..32])))
}

fn ensure_private_dir(path: &Path) -> io::Result<()> {
    ensure_private_owner_dir(path)
}

fn same_filesystem(a: &Path, b: &Path) -> bool {
    same_filesystem_volume(a, b).unwrap_or(false)
}
