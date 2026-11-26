// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Process-wide live presence registry for paired-device carriers and HTTP requests.
//!
//! `inspect_clients_at`, `known_sources::snapshot`, and `/app/network/api/devices`
//! consult [`live_seen_for`] directly so every in-process reader sees sub-millisecond
//! `last_seen_at` updates across router clones and door carrier loops, while disk
//! writes to `link/devices.json` are debounced to [`LAST_SEEN_PERSIST_INTERVAL`] per
//! `(journal_root, cid)`.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::ledger::{
    AuthorizationLedger, AuthorizedClientsMutationError, advance_last_seen, parse_rfc3339_utc,
};

/// Minimum interval between `link/devices.json` write-throughs for a single `cid`.
///
/// `8 s` debounce + `<= 18 s` client resting keepalive + `2 s` door poll `= 28 s`,
/// staying strictly below `CLIENT_ACTIVE_MS` (`30_000 ms`).
pub const LAST_SEEN_PERSIST_INTERVAL: Duration = Duration::from_secs(8);

#[derive(Debug)]
struct LiveDeviceEntry {
    last_seen_at: String,
    last_persisted_at: Option<Instant>,
    persisting: bool,
}

type RegistryMap = BTreeMap<PathBuf, BTreeMap<String, LiveDeviceEntry>>;

fn registry() -> &'static Mutex<RegistryMap> {
    static LIVE_PRESENCE: OnceLock<Mutex<RegistryMap>> = OnceLock::new();
    LIVE_PRESENCE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(crate) fn registry_root_key(journal_root: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(journal_root) {
        return canonical;
    }
    let mut normalized = PathBuf::new();
    for component in journal_root.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

pub(crate) fn now_rfc3339_utc() -> String {
    let now_ms = OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
    let timestamp = OffsetDateTime::from_unix_timestamp_nanos(now_ms * 1_000_000)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH);
    timestamp
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

/// Return whichever RFC 3339 UTC timestamp is newer between on-disk activity and
/// in-process live presence.
pub fn newer_last_seen_at(disk: Option<&str>, live: Option<&str>) -> Option<String> {
    match (disk, live) {
        (None, None) => None,
        (Some(disk), None) => Some(disk.to_owned()),
        (None, Some(live)) => Some(live.to_owned()),
        (Some(disk), Some(live)) => match (parse_rfc3339_utc(disk), parse_rfc3339_utc(live)) {
            (Some(disk_ts), Some(live_ts)) => {
                if live_ts > disk_ts {
                    Some(live.to_owned())
                } else {
                    Some(disk.to_owned())
                }
            }
            (None, Some(_)) => Some(live.to_owned()),
            _ => Some(disk.to_owned()),
        },
    }
}

/// Read the current in-process `cid -> last_seen_at` snapshot for `journal_root`.
pub fn live_seen_for(journal_root: &Path) -> BTreeMap<String, String> {
    let key = registry_root_key(journal_root);
    let guard = registry().lock().unwrap_or_else(|p| p.into_inner());
    guard
        .get(&key)
        .map(|entries| {
            entries
                .iter()
                .map(|(cid, entry)| (cid.clone(), entry.last_seen_at.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Evict `cid` from the live-presence registry for `journal_root`.
pub fn evict_live_device(journal_root: &Path, cid: &str) {
    let key = registry_root_key(journal_root);
    evict_by_key(&key, cid);
}

fn evict_by_key(key: &Path, cid: &str) {
    let mut guard = registry().lock().unwrap_or_else(|p| p.into_inner());
    let empty = if let Some(entries) = guard.get_mut(key) {
        entries.remove(cid);
        entries.is_empty()
    } else {
        false
    };
    if empty {
        guard.remove(key);
    }
}

/// Record live activity in memory and return `Some(stamp)` when a debounced disk
/// write-through is due and claimed by the caller.
///
/// Async callers pair this with [`complete_live_device_persist`] inside
/// `tokio::task::spawn_blocking` so worker threads never block on file locks.
pub fn begin_live_device_note(journal_root: &Path, cid: &str) -> Option<String> {
    begin_live_device_note_at(
        &registry_root_key(journal_root),
        cid,
        &now_rfc3339_utc(),
        Instant::now(),
        false,
    )
}

fn begin_live_device_note_at(
    key: &Path,
    cid: &str,
    stamp: &str,
    now_instant: Instant,
    force_persist: bool,
) -> Option<String> {
    let mut guard = registry().lock().unwrap_or_else(|p| p.into_inner());
    let entries = guard.entry(key.to_path_buf()).or_default();
    let entry = entries
        .entry(cid.to_owned())
        .or_insert_with(|| LiveDeviceEntry {
            last_seen_at: stamp.to_owned(),
            last_persisted_at: None,
            persisting: false,
        });
    let _ = advance_last_seen(&mut entry.last_seen_at, stamp);
    let persist_due = force_persist
        || entry.last_persisted_at.is_none_or(|last| {
            now_instant.saturating_duration_since(last) >= LAST_SEEN_PERSIST_INTERVAL
        });
    if persist_due && !entry.persisting {
        entry.persisting = true;
        Some(entry.last_seen_at.clone())
    } else {
        None
    }
}

/// Complete a disk write-through started by [`begin_live_device_note`].
pub fn complete_live_device_persist(journal_root: &Path, cid: &str, stamp: &str) {
    let key = registry_root_key(journal_root);
    let result = AuthorizationLedger::new(journal_root).touch_last_seen_at(cid, stamp);
    finish_persist_outcome(&key, cid, result, Instant::now());
}

fn finish_persist_outcome(
    key: &Path,
    cid: &str,
    result: Result<bool, AuthorizedClientsMutationError>,
    persisted_instant: Instant,
) {
    match result {
        Ok(true) => {
            let mut guard = registry().lock().unwrap_or_else(|p| p.into_inner());
            if let Some(entry) = guard.get_mut(key).and_then(|entries| entries.get_mut(cid)) {
                entry.persisting = false;
                entry.last_persisted_at = Some(persisted_instant);
            }
        }
        Ok(false) => {
            log::debug!("paired-device CID was absent while recording last seen");
            evict_by_key(key, cid);
        }
        Err(error) => {
            log::debug!("paired-device last-seen update failed: {error}");
            let mut guard = registry().lock().unwrap_or_else(|p| p.into_inner());
            if let Some(entry) = guard.get_mut(key).and_then(|entries| entries.get_mut(cid)) {
                entry.persisting = false;
            }
        }
    }
}

/// Record live device presence and write through to `link/devices.json` when the
/// `8 s` per-`cid` persist debounce has elapsed.
pub fn note_live_device(journal_root: &Path, cid: &str) {
    let key = registry_root_key(journal_root);
    let stamp = now_rfc3339_utc();
    if let Some(persist_stamp) = begin_live_device_note_at(&key, cid, &stamp, Instant::now(), false)
    {
        let result = AuthorizationLedger::new(journal_root).touch_last_seen_at(cid, &persist_stamp);
        finish_persist_outcome(&key, cid, result, Instant::now());
    }
}

/// Seed live device presence at carrier handshake and immediately attempt an
/// on-disk `last_seen_at` write-through.
pub fn seed_live_device_handshake(journal_root: &Path, cid: &str) {
    let key = registry_root_key(journal_root);
    let stamp = now_rfc3339_utc();
    if let Some(persist_stamp) = begin_live_device_note_at(&key, cid, &stamp, Instant::now(), true)
    {
        let result = AuthorizationLedger::new(journal_root).touch_last_seen_at(cid, &persist_stamp);
        finish_persist_outcome(&key, cid, result, Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::ledger::{
        AuthorizedClientsLoadError, ClientEntry, ClientRole, DeviceActivityRead,
        read_device_activity,
    };

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "sol-link-live-presence-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn transient_persist_failure_does_not_advance_eight_second_debounce_and_unpaired_evicts() {
        let dir = TempDir::new();
        let key = registry_root_key(dir.path());
        let t0 = Instant::now();

        let first =
            begin_live_device_note_at(&key, "sha256:test", "2026-08-13T00:00:01Z", t0, false);
        assert_eq!(first.as_deref(), Some("2026-08-13T00:00:01Z"));
        assert_eq!(
            live_seen_for(dir.path())
                .get("sha256:test")
                .map(String::as_str),
            Some("2026-08-13T00:00:01Z")
        );

        finish_persist_outcome(
            &key,
            "sha256:test",
            Err(AuthorizedClientsMutationError::Load(
                AuthorizedClientsLoadError::Unreadable {
                    path: dir.path().join("link/authorized_clients.json"),
                    source: io::Error::other("transient unreadable"),
                },
            )),
            t0 + Duration::from_secs(1),
        );

        let retry = begin_live_device_note_at(
            &key,
            "sha256:test",
            "2026-08-13T00:00:02Z",
            t0 + Duration::from_secs(2),
            false,
        );
        assert_eq!(
            retry.as_deref(),
            Some("2026-08-13T00:00:02Z"),
            "failed persist must leave the 8 s debounce unadvanced so the next tick retries"
        );
        finish_persist_outcome(&key, "sha256:test", Ok(true), t0 + Duration::from_secs(2));

        let within_window = begin_live_device_note_at(
            &key,
            "sha256:test",
            "2026-08-13T00:00:05Z",
            t0 + Duration::from_secs(9),
            false,
        );
        assert_eq!(
            within_window, None,
            "successful persist suppresses disk writes for the next 8 s"
        );
        assert_eq!(
            live_seen_for(dir.path())
                .get("sha256:test")
                .map(String::as_str),
            Some("2026-08-13T00:00:05Z"),
            "in-memory live presence still advances immediately inside the 8 s window"
        );

        let after_window = begin_live_device_note_at(
            &key,
            "sha256:test",
            "2026-08-13T00:00:11Z",
            t0 + Duration::from_secs(10),
            false,
        );
        assert_eq!(after_window.as_deref(), Some("2026-08-13T00:00:11Z"));
        finish_persist_outcome(&key, "sha256:test", Ok(false), t0 + Duration::from_secs(10));
        assert!(
            !live_seen_for(dir.path()).contains_key("sha256:test"),
            "Ok(false) must evict an unpaired cid from live presence"
        );
    }

    #[test]
    fn note_live_device_retries_after_real_ledger_write_failure_and_remove_evicts() {
        let dir = TempDir::new();
        let mut ledger = AuthorizationLedger::new(dir.path());
        ledger
            .add(ClientEntry::new(
                "sha256:real",
                "phone",
                "2026-08-13T00:00:00Z",
                "instance",
                ClientRole::Roleless,
            ))
            .unwrap();

        // Force `devices.json` write failure by placing a directory at its path.
        let devices_path = dir.path().join("link/devices.json");
        fs::create_dir_all(&devices_path).unwrap();
        note_live_device(dir.path(), "sha256:real");
        assert!(live_seen_for(dir.path()).contains_key("sha256:real"));

        // Clear the obstruction; the next call within < 8 s must retry and persist.
        fs::remove_dir_all(&devices_path).unwrap();
        note_live_device(dir.path(), "sha256:real");
        let DeviceActivityRead::Present(activity) = read_device_activity(&devices_path) else {
            panic!("retry after transient failure must write devices.json");
        };
        assert!(activity.contains_key("sha256:real"));

        // Removing the device from the ledger evicts its in-memory live presence.
        let outcome = ledger.remove("sha256:real").unwrap();
        assert!(outcome.authorized_removed);
        assert!(!live_seen_for(dir.path()).contains_key("sha256:real"));
    }
}
