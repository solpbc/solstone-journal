// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable, privacy-minimal record of photo library assets the journal has
//! received (or had trashed by the owner), keyed by opaque SHA-256 hashes of
//! `asset_id` (and `cloud_id` when present) under `config/photos/received/`.
//!
//! Mirrors `config/screenshots/received/`:
//! - Lives outside `config/photos-library/` so `turn off photos` deletes the
//!   entire holding area (scanned times, places, left-out counts) while keeping
//!   this opaque-hash ledger intact.
//! - Holds no delete list, no photo bytes, no filenames, and no locations.
//! - Survives segment trash, trash erase, backups/restores, and turn-off +
//!   turn-back-on so a photo the owner trashed is never asked for again, and
//!   two devices on one library produce strictly one copy per asset.
//! - Fails closed if an existing record cannot be read or parsed rather than
//!   treating it as absent.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace, create_directory_with_mode};
use solstone_core_segment::photo_observation::read_photo_observation;

const RECEIVED_DIR: &str = "config/photos/received";
const PRIVATE_DIR_MODE: u32 = 0o700;
const PRIVATE_FILE: AtomicWriteOptions = AtomicWriteOptions { mode: Some(0o600) };

/// Where the photos source first kept a library asset.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct PhotoReceipt {
    #[serde(default = "default_schema")]
    pub schema: u32,
    pub day: String,
    pub stream: String,
    pub segment: String,
    pub received_at: String,
}

const fn default_schema() -> u32 {
    1
}

/// Deterministic opaque hex key for a local or cloud photo asset identifier.
#[must_use]
pub fn id_hash(namespace: &str, id: &str) -> String {
    let mut h = Sha256::new();
    h.update(namespace.as_bytes());
    h.update(b":");
    h.update(id.as_bytes());
    format!("{:x}", h.finalize())
}

fn receipt_path(journal: &Path, hash: &str) -> PathBuf {
    journal
        .join(RECEIVED_DIR)
        .join(&hash[..2])
        .join(format!("{hash}.json"))
}

fn lookup_hash(journal: &Path, hash: &str) -> io::Result<Option<PhotoReceipt>> {
    match std::fs::read(receipt_path(journal, hash)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match std::fs::symlink_metadata(journal.join(RECEIVED_DIR)) {
                Ok(meta) if meta.is_dir() => Ok(None),
                Ok(_) => Err(io::Error::other(
                    "photos received ledger is not a directory",
                )),
                Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(err) => Err(err),
            }
        }
        Err(error) => Err(error),
    }
}

fn write_hash(journal: &Path, hash: &str, receipt: &PhotoReceipt) -> io::Result<()> {
    let path = receipt_path(journal, hash);
    if let Some(parent) = path.parent() {
        let root_dir = journal.join("config");
        create_directory_with_mode(&root_dir, PRIVATE_DIR_MODE).map_err(io::Error::other)?;
        let photos_dir = root_dir.join("photos");
        create_directory_with_mode(&photos_dir, PRIVATE_DIR_MODE).map_err(io::Error::other)?;
        let received_dir = journal.join(RECEIVED_DIR);
        create_directory_with_mode(&received_dir, PRIVATE_DIR_MODE).map_err(io::Error::other)?;
        create_directory_with_mode(parent, PRIVATE_DIR_MODE).map_err(io::Error::other)?;
    }
    let mut bytes = serde_json::to_vec(receipt).map_err(io::Error::other)?;
    bytes.push(b'\n');
    atomic_replace(&path, &bytes, PRIVATE_FILE).map_err(io::Error::other)?;
    Ok(())
}

/// Fail-closed lookup by `asset_id` and optional `cloud_id`.
///
/// Returns `Ok(Some(receipt))` if either identifier was previously received
/// (whether currently live in `chronicle/` or trashed/erased by the owner).
pub fn lookup_received(
    journal: &Path,
    asset_id: &str,
    cloud_id: Option<&str>,
) -> io::Result<Option<PhotoReceipt>> {
    let asset_hash = id_hash("asset", asset_id);
    if let Some(rec) = lookup_hash(journal, &asset_hash)? {
        return Ok(Some(rec));
    }
    if let Some(cid) = cloud_id.map(str::trim).filter(|s| !s.is_empty()) {
        let cloud_hash = id_hash("cloud", cid);
        if let Some(rec) = lookup_hash(journal, &cloud_hash)? {
            return Ok(Some(rec));
        }
    }
    Ok(None)
}

/// Record that `asset_id` (and optional `cloud_id`) has been received into the
/// journal at `(day, stream, segment)`.
pub fn record_received(
    journal: &Path,
    asset_id: &str,
    cloud_id: Option<&str>,
    receipt: &PhotoReceipt,
) -> io::Result<()> {
    write_hash(journal, &id_hash("asset", asset_id), receipt)?;
    if let Some(cid) = cloud_id.map(str::trim).filter(|s| !s.is_empty()) {
        write_hash(journal, &id_hash("cloud", cid), receipt)?;
    }
    Ok(())
}

/// Record suppression for a `photos` segment about to be trashed (defense in
/// depth in case a segment was trashed via transcripts or another surface).
pub fn record_suppression_for_segment(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
) -> io::Result<()> {
    let seg_dir = journal
        .join("chronicle")
        .join(day)
        .join(stream)
        .join(segment);
    let mut asset_id: Option<String> = None;
    let mut cloud_id: Option<String> = None;

    if let Some(obs) = read_photo_observation(&seg_dir)
        && let Some(aid) = obs.source.asset_id.filter(|s| !s.trim().is_empty())
    {
        asset_id = Some(aid);
    }
    if let Ok(bytes) = std::fs::read(seg_dir.join("image_meta.json"))
        && let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes)
    {
        if asset_id.is_none() {
            asset_id = val
                .get("asset_id")
                .or_else(|| val.get("source_asset_id"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
        }
        cloud_id = val
            .get("cloud_id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
    }

    if let Some(aid) = asset_id {
        let receipt = PhotoReceipt {
            schema: 1,
            day: day.to_owned(),
            stream: stream.to_owned(),
            segment: segment.to_owned(),
            received_at: solstone_core_time::utc_rfc3339(jiff::Timestamp::now()),
        };
        record_received(journal, &aid, cloud_id.as_deref(), &receipt)?;
    }
    Ok(())
}

/// Whether the segment named in `receipt` is still live (not trashed/erased).
#[must_use]
pub fn is_segment_live(journal: &Path, receipt: &PhotoReceipt) -> bool {
    let dir = journal
        .join("chronicle")
        .join(&receipt.day)
        .join(&receipt.stream)
        .join(&receipt.segment);
    dir.is_dir() && !dir.join("tombstone.json").is_file()
}

/// Point-in-time directory index of `config/photos/received/` for bulk lookups
/// (`dashboard_state`, `lease_wanted`).
///
/// Enumerates the two-hex-prefix shards once so checking tens of thousands of
/// scanned assets against `0..N` received receipts avoids tens of thousands of
/// `ENOENT` syscalls (and avoids SHA-256 hashing altogether when empty), while
/// preserving the fail-closed semantics of [`lookup_received`]:
/// - A non-directory `config/photos/received` or unreadable/non-directory shard
///   returns an error.
/// - Any matched hash reads and parses its receipt JSON and returns an error on
///   malformed JSON or I/O failure.
pub struct ReceivedLedgerSnapshot {
    journal: PathBuf,
    present_hashes: HashSet<String>,
    parsed: HashMap<String, PhotoReceipt>,
    live_cache: HashMap<(String, String, String), bool>,
}

impl ReceivedLedgerSnapshot {
    /// Load the set of existing receipt hashes under `config/photos/received/`.
    pub fn load(journal: &Path) -> io::Result<Self> {
        let received_dir = journal.join(RECEIVED_DIR);
        match std::fs::symlink_metadata(&received_dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(io::Error::other(
                    "photos received ledger is not a directory",
                ));
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(Self::empty(journal));
            }
            Err(err) => return Err(err),
        }

        let mut present_hashes = HashSet::new();
        for shard_entry in std::fs::read_dir(&received_dir)? {
            let shard_entry = shard_entry?;
            let shard_name = shard_entry.file_name();
            let Some(shard_str) = shard_name.to_str() else {
                continue;
            };
            if shard_str.len() != 2 || !shard_str.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }
            let shard_type = shard_entry.file_type()?;
            if !shard_type.is_dir() {
                return Err(io::Error::other("photos received shard is not a directory"));
            }
            for item_entry in std::fs::read_dir(shard_entry.path())? {
                let item_entry = item_entry?;
                let item_name = item_entry.file_name();
                if let Some(stem) = item_name.to_str().and_then(|s| s.strip_suffix(".json")) {
                    present_hashes.insert(stem.to_owned());
                }
            }
        }

        Ok(Self {
            journal: journal.to_path_buf(),
            present_hashes,
            parsed: HashMap::new(),
            live_cache: HashMap::new(),
        })
    }

    #[must_use]
    pub fn empty(journal: &Path) -> Self {
        Self {
            journal: journal.to_path_buf(),
            present_hashes: HashSet::new(),
            parsed: HashMap::new(),
            live_cache: HashMap::new(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.present_hashes.is_empty()
    }

    fn read_cached_hash(&mut self, hash: &str) -> io::Result<PhotoReceipt> {
        if let Some(rec) = self.parsed.get(hash) {
            return Ok(rec.clone());
        }
        let bytes = std::fs::read(receipt_path(&self.journal, hash))?;
        let rec: PhotoReceipt = serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        self.parsed.insert(hash.to_owned(), rec.clone());
        Ok(rec)
    }

    /// Fail-closed lookup matching [`lookup_received`].
    pub fn lookup(
        &mut self,
        asset_id: &str,
        cloud_id: Option<&str>,
    ) -> io::Result<Option<PhotoReceipt>> {
        if self.present_hashes.is_empty() {
            return Ok(None);
        }
        let asset_hash = id_hash("asset", asset_id);
        if self.present_hashes.contains(&asset_hash) {
            return self.read_cached_hash(&asset_hash).map(Some);
        }
        if let Some(cid) = cloud_id.map(str::trim).filter(|s| !s.is_empty()) {
            let cloud_hash = id_hash("cloud", cid);
            if self.present_hashes.contains(&cloud_hash) {
                return self.read_cached_hash(&cloud_hash).map(Some);
            }
        }
        Ok(None)
    }

    /// Cached check of [`is_segment_live`] for a receipt returned by this snapshot.
    pub fn is_live(&mut self, receipt: &PhotoReceipt) -> bool {
        let key = (
            receipt.day.clone(),
            receipt.stream.clone(),
            receipt.segment.clone(),
        );
        if let Some(&live) = self.live_cache.get(&key) {
            return live;
        }
        let live = is_segment_live(&self.journal, receipt);
        self.live_cache.insert(key, live);
        live
    }
}
