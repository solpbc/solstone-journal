// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The repository side of "remove older backups": list what the destination
//! holds, look for paths in one backup, and forget backups by id. Every call
//! goes through the maintenance runtime prune uses, so the operated destination
//! and the owner's own storage take the same path. Nothing here prunes: the
//! space the forgotten backups held is the nightly prune's.

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::{BackupServices, Runtime, restic, runtime, unlock};
use crate::teardown::owned::{StorageKeeps, StorageRetention};

/// Snapshot listing and the per-backup path look-up are read-only and quick.
pub const LIST_TIMEOUT_SECONDS: u64 = 10 * 60;
/// Each forget waits up to 30 minutes for the repository lock, then works.
pub const FORGET_TIMEOUT_SECONDS: u64 = 45 * 60;
/// How many backups one forget names: a stop part way loses at most one batch
/// of progress, and a backup waiting on the lock waits for one batch only.
pub const FORGET_BATCH: usize = 10;
/// How many paths one look-up names, keeping argv short.
const LS_BATCH: usize = 200;
const RETENTION_CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// One backup the destination holds, as restic lists it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SnapshotFacts {
    pub id: String,
    /// When the backup started, as restic recorded it (RFC 3339).
    pub time: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
}

impl SnapshotFacts {
    pub fn instant(&self) -> Option<DateTime<FixedOffset>> {
        DateTime::parse_from_rfc3339(&self.time).ok()
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|candidate| candidate == tag)
    }
}

/// Decode `restic snapshots --json`. Any row without a hex id or a readable
/// time fails the whole listing: a backup this cannot read is never counted,
/// kept or removed by guess.
pub fn decode_snapshots(listing: &Value) -> Option<Vec<SnapshotFacts>> {
    let rows = listing.as_array()?;
    rows.iter()
        .map(|row| {
            let facts: SnapshotFacts = serde_json::from_value(row.clone()).ok()?;
            let hex = !facts.id.is_empty() && facts.id.bytes().all(|b| b.is_ascii_hexdigit());
            (hex && facts.instant().is_some()).then_some(facts)
        })
        .collect()
}

/// An open view of this journal's backup destination.
pub struct Repository<'a> {
    services: &'a BackupServices<'a>,
    runtime: Runtime,
    operated: bool,
}

/// Opens the destination with the maintenance credential. `Ok(None)` when
/// backup isn't set up here.
pub fn open<'a>(
    journal: &Path,
    services: &'a BackupServices<'a>,
) -> Result<Option<Repository<'a>>, String> {
    let operated = solstone_core_backup::get_backup_config(journal)
        .ok()
        .and_then(|config| config.get("mode").cloned())
        == Some(Value::String("operated".into()));
    Ok(
        runtime(journal, services, "maintenance", false)?.map(|runtime| Repository {
            services,
            runtime,
            operated,
        }),
    )
}

/// Opens the destination for listing only, with the credential the backup
/// itself uses: on sol pbc's storage that is the append-only session, never
/// the one that can delete.
pub fn open_read<'a>(
    journal: &Path,
    services: &'a BackupServices<'a>,
) -> Result<Option<Repository<'a>>, String> {
    let operated = solstone_core_backup::get_backup_config(journal)
        .ok()
        .and_then(|config| config.get("mode").cloned())
        == Some(Value::String("operated".into()));
    Ok(
        runtime(journal, services, "backup", operated)?.map(|runtime| Repository {
            services,
            runtime,
            operated,
        }),
    )
}

impl Repository<'_> {
    pub fn operated(&self) -> bool {
        self.operated
    }

    /// Every backup the destination holds.
    pub fn snapshots(&self) -> Result<Vec<SnapshotFacts>, String> {
        let output = restic(
            self.services,
            &self.runtime,
            vec!["snapshots".into()],
            true,
            LIST_TIMEOUT_SECONDS,
            None,
        )?;
        if output.returncode != 0 {
            return Err(crate::runner::reason_for_returncode(output.returncode).into());
        }
        output
            .json
            .as_ref()
            .and_then(decode_snapshots)
            .ok_or_else(|| "unreadable_listing".into())
    }

    /// Which of `tree_paths` backup `snapshot` holds. A path that names a
    /// folder also matches what it directly holds.
    pub fn present(&self, snapshot: &str, tree_paths: &[String]) -> Result<Vec<String>, String> {
        let mut found = Vec::new();
        for chunk in tree_paths.chunks(LS_BATCH) {
            let mut args = vec!["ls".into(), "--json".into(), snapshot.to_owned()];
            args.extend(chunk.iter().cloned());
            let output = restic(
                self.services,
                &self.runtime,
                args,
                false,
                LIST_TIMEOUT_SECONDS,
                None,
            );
            // `ls --json` prints one object a line, so it skips the runner's
            // single-document parse and is read here.
            let output = output?;
            if output.returncode != 0 {
                return Err(crate::runner::reason_for_returncode(output.returncode).into());
            }
            found.extend(listed_paths(&output.stdout, chunk)?);
        }
        Ok(found)
    }

    /// Forgets the named backups. Waits on the repository lock like prune.
    pub fn forget(&self, ids: &[String]) -> Result<(), String> {
        if ids.is_empty() {
            return Ok(());
        }
        unlock(self.services, &self.runtime);
        let mut args = vec!["forget".into()];
        args.extend(ids.iter().cloned());
        let output = restic(
            self.services,
            &self.runtime,
            args,
            false,
            FORGET_TIMEOUT_SECONDS,
            None,
        )?;
        if output.returncode == 0 {
            Ok(())
        } else {
            Err(crate::runner::reason_for_returncode(output.returncode).into())
        }
    }

    /// Whether the storage keeps what restic deletes. sol pbc's operated
    /// storage keeps no versions and locks nothing; the owner's own S3 or B2
    /// is asked, read-only, and anything else reads unknown.
    pub fn storage(&self) -> StorageRetention {
        if self.operated {
            return StorageRetention::Removes;
        }
        let Some(amz_date) =
            DateTime::<chrono::Utc>::from_timestamp(self.services.clock.now_unix(), 0)
                .map(|now| now.format("%Y%m%dT%H%M%SZ").to_string())
        else {
            return StorageRetention::Unknown;
        };
        crate::teardown::owned::retention::observe(
            &self.runtime.destination,
            self.services.http,
            &solstone_core_backup::archive_admission::ArchiveCancellation::new(),
            RETENTION_CHECK_TIMEOUT,
            &amz_date,
        )
    }
}

/// The stable word for a storage retention answer, for state and the page.
pub fn storage_word(retention: StorageRetention) -> &'static str {
    match retention {
        StorageRetention::Removes => "removes",
        StorageRetention::Keeps(StorageKeeps::Versions) => "versions",
        StorageRetention::Keeps(StorageKeeps::ObjectLock | StorageKeeps::FileLock) => "locks",
        StorageRetention::Keeps(StorageKeeps::HiddenFiles) => "hidden_files",
        StorageRetention::Unknown => "unknown",
    }
}

/// The requested paths `restic ls` printed. Its first line describes the
/// backup; every other line is one entry.
fn listed_paths(stdout: &str, requested: &[String]) -> Result<Vec<String>, String> {
    let mut found = Vec::new();
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let entry: Value = serde_json::from_str(line).map_err(|_| "unreadable_listing")?;
        if entry.get("struct_type").and_then(Value::as_str) == Some("snapshot")
            || entry.get("message_type").and_then(Value::as_str) == Some("snapshot")
        {
            continue;
        }
        let Some(path) = entry.get("path").and_then(Value::as_str) else {
            continue;
        };
        if requested.iter().any(|wanted| {
            path == wanted
                || path
                    .strip_prefix(wanted.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        }) {
            found.push(path.to_owned());
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_listing_with_an_unreadable_row_reads_as_nothing() {
        let good = json!([{"id": "ab12", "time": "2026-11-14T10:00:00.5-07:00", "tags": ["x"], "paths": ["/j"]}]);
        assert_eq!(decode_snapshots(&good).unwrap()[0].tags, vec!["x"]);
        let bad_time = json!([{"id": "ab12", "time": "yesterday"}]);
        assert!(decode_snapshots(&bad_time).is_none());
        let bad_id = json!([{"id": "../x", "time": "2026-11-14T10:00:00Z"}]);
        assert!(decode_snapshots(&bad_id).is_none());
    }

    #[test]
    fn ls_lines_match_requested_paths_and_their_children_only() {
        let out = concat!(
            "{\"time\":\"2026-11-14T10:00:00Z\",\"tree\":\"a\",\"paths\":[\"/j\"],\"struct_type\":\"snapshot\"}\n",
            "{\"name\":\"t1\",\"type\":\"dir\",\"path\":\"/j/.trash/t1\",\"struct_type\":\"node\"}\n",
            "{\"name\":\"files\",\"type\":\"dir\",\"path\":\"/j/.trash/t1/files\",\"struct_type\":\"node\"}\n",
            "{\"name\":\"t10\",\"type\":\"dir\",\"path\":\"/j/.trash/t10\",\"struct_type\":\"node\"}\n",
        );
        let found = listed_paths(out, &["/j/.trash/t1".into()]).unwrap();
        assert_eq!(found, vec!["/j/.trash/t1", "/j/.trash/t1/files"]);
        assert!(listed_paths("not json", &[]).is_err());
    }
}
