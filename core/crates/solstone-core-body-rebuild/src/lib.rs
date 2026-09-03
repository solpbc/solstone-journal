// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Host-only reconstruction of body dedupe state from immutable import bundles.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;

use rusqlite::{Connection, OpenFlags, params};
use sha2::{Digest, Sha256};
use solstone_core_body_source::{
    BodyEnvelope, BodyMonth, BodyRawRetention, BodyString, BodyValue, BundleClass,
    DirectoryObservation, PresentationRow, authorize_native_bundle, canonicalize,
    classify_bundle_directory, decode_body_envelope_with_manifest, parse, project,
};
use solstone_core_body_store::{BodyBundleReplay, BodyDedupeState, validate_legacy_body_row};
use solstone_core_journal_io::{
    AtomicWriteOptions, DirEntry, DirEntryKind, LockError, LockOptions, Removed,
    create_directory_with_mode, hold_lock, install_file, list_dir_entries,
    list_dir_entries_bounded, remove_file, sync_dir, write_bytes_exclusive,
};

#[cfg(test)]
mod test_support;

const IMPORTS_DIR: &str = "imports";
const DATABASE_REL: &str = "imports/health-dedupe.sqlite";
const TEMP_DATABASE_REL: &str = "imports/.health-dedupe.sqlite.rebuild";
const ENVELOPE_NAME: &str = "body-bundle.json";
const LEDGER_NAME: &str = "body-ledger.jsonl";
const MANIFEST_NAME: &str = "manifest.json";
const NORMALIZED_NAME: &str = "normalized";
const RAW_NAME: &str = "raw";
const RAW_INVENTORY_NAME: &str = "body-raw-inventory.jsonl";
const RAW_INVENTORY_FIELD: &str = "raw_inventory_sha256";
const MAX_DOCUMENT_BYTES: usize = 1_048_576;
const MAX_LEDGER_FRAME_BYTES: usize = 65_537;
const MAX_RAW_INVENTORY_BYTES: usize = 16 * 1024 * 1024;
const MAX_RAW_ASSETS: usize = 10_000;
const MAX_RAW_DIRECTORIES: usize = 1_024;
const MAX_RAW_DEPTH: usize = 128;
const MAX_RAW_ASSET_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_RAW_TOTAL_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const REBUILD_TIMESTAMP: &str = "1970-01-01T00:00:00Z";

const SCHEMA: &str = r#"
CREATE TABLE health_dedupe (
    dedupe_key TEXT PRIMARY KEY,
    source_family TEXT NOT NULL,
    source_record_id TEXT,
    record_type TEXT NOT NULL,
    start_time TEXT NOT NULL,
    end_time TEXT,
    value_hash TEXT,
    first_import_id TEXT,
    last_seen_import_id TEXT,
    normalized_ref TEXT,
    raw_ref TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_health_dedupe_source_record
ON health_dedupe (source_family, source_record_id);
CREATE INDEX idx_health_dedupe_record_time
ON health_dedupe (record_type, start_time, end_time);
"#;

/// Stable failure categories for a body-store rebuild.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BodyRebuildErrorKind {
    Journal,
    Authority,
    Envelope,
    NativeReplay,
    LegacyReplay,
    Sqlite,
    Publication,
}

impl BodyRebuildErrorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Journal => "journal",
            Self::Authority => "authority",
            Self::Envelope => "envelope",
            Self::NativeReplay => "native_replay",
            Self::LegacyReplay => "legacy_replay",
            Self::Sqlite => "sqlite",
            Self::Publication => "publication",
        }
    }
}

/// A bounded, owner-data-redacted rebuild failure.
#[derive(Clone, PartialEq, Eq)]
pub struct BodyRebuildError {
    kind: BodyRebuildErrorKind,
    stage: &'static str,
}

impl BodyRebuildError {
    fn new(kind: BodyRebuildErrorKind, stage: &'static str) -> Self {
        Self { kind, stage }
    }

    pub fn kind(&self) -> BodyRebuildErrorKind {
        self.kind
    }

    pub fn stage(&self) -> &'static str {
        self.stage
    }
}

impl fmt::Display for BodyRebuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "body-rebuild {}: {}",
            self.kind.as_str(),
            self.stage
        )
    }
}

impl fmt::Debug for BodyRebuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for BodyRebuildError {}

/// Completed rebuild counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyRebuildReport {
    native_bundles: u64,
    legacy_bundles: u64,
    rows: u64,
}

impl BodyRebuildReport {
    pub fn native_bundles(self) -> u64 {
        self.native_bundles
    }

    pub fn legacy_bundles(self) -> u64 {
        self.legacy_bundles
    }

    pub fn rows(self) -> u64 {
        self.rows
    }
}

struct ReplayResult {
    state: BodyDedupeState,
    native_bundles: u64,
    legacy_bundles: u64,
}

struct ObservedBundle {
    entry: DirEntry,
    children: Vec<DirEntry>,
    manifest: Option<Vec<u8>>,
    class: BundleClass,
}

struct RawInventoryTree {
    files: BTreeMap<String, (u64, String)>,
    directories: BTreeSet<String>,
}

/// Rebuilds and atomically publishes `imports/health-dedupe.sqlite`.
///
/// SQLite is the intentional host-only mutation engine for the unpublished
/// temporary database. Directory, lock, cleanup, and final publication
/// mutations all route through `solstone-core-journal-io`.
pub fn rebuild_body_store(journal_root: &Path) -> Result<BodyRebuildReport, BodyRebuildError> {
    if !journal_root.is_dir() {
        return Err(error(BodyRebuildErrorKind::Journal, "journal_root"));
    }
    let imports = journal_root.join(IMPORTS_DIR);
    create_directory_with_mode(&imports, 0o700)
        .map_err(|_| error(BodyRebuildErrorKind::Publication, "imports_directory"))?;
    let database_path = journal_root.join(DATABASE_REL);
    let _lock = hold_lock(
        &database_path,
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(|source| match source {
        LockError::Timeout(_) => error(BodyRebuildErrorKind::Publication, "database_lock_timeout"),
        LockError::Io { .. } => error(BodyRebuildErrorKind::Publication, "database_lock"),
    })?;

    let replay = replay_history(journal_root)?;
    let rows = u64::try_from(replay.state.len())
        .map_err(|_| error(BodyRebuildErrorKind::Sqlite, "row_count"))?;
    publish_database(journal_root, &replay.state)?;
    Ok(BodyRebuildReport {
        native_bundles: replay.native_bundles,
        legacy_bundles: replay.legacy_bundles,
        rows,
    })
}

fn replay_history(journal_root: &Path) -> Result<ReplayResult, BodyRebuildError> {
    let imports = journal_root.join(IMPORTS_DIR);
    let mut observed = Vec::new();
    for entry in list_dir_entries(&imports)
        .map_err(|_| error(BodyRebuildErrorKind::Journal, "list_imports"))?
    {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let children = list_dir_entries(&entry.path)
            .map_err(|_| error(BodyRebuildErrorKind::Journal, "list_import_bundle"))?;
        let manifest_entry = named_entry(&children, MANIFEST_NAME);
        let manifest = match manifest_entry {
            Some(entry) if entry.kind == DirEntryKind::File => Some(read_bounded_file(
                &entry.path,
                MAX_DOCUMENT_BYTES.saturating_add(1),
            )?),
            _ => None,
        };
        let observation = DirectoryObservation {
            name: entry.name.as_encoded_bytes(),
            envelope_present: is_regular_file(&children, ENVELOPE_NAME),
            ledger_present: is_regular_file(&children, LEDGER_NAME),
            manifest: manifest.as_deref(),
        };
        observed.push(ObservedBundle {
            class: classify_bundle_directory(observation),
            entry,
            children,
            manifest,
        });
    }

    let mut state = BodyDedupeState::new();
    let mut legacy_bundles = 0_u64;
    let mut native_bundles = 0_u64;

    // Every pre-native import predates every native authority, regardless of
    // their different directory-ID alphabets. Within each era, directory order
    // is the immutable replay order.
    for bundle in observed
        .iter()
        .filter(|bundle| bundle.class == BundleClass::LegacyCandidate)
    {
        let (next, replayed) = replay_legacy_bundle(bundle, state)?;
        state = next;
        legacy_bundles += u64::from(replayed);
    }
    for bundle in observed
        .iter()
        .filter(|bundle| bundle.class == BundleClass::NativeCandidate)
    {
        state = replay_native_bundle(bundle, state)?;
        native_bundles = native_bundles.saturating_add(1);
    }

    Ok(ReplayResult {
        state,
        native_bundles,
        legacy_bundles,
    })
}

fn replay_native_bundle(
    bundle: &ObservedBundle,
    state: BodyDedupeState,
) -> Result<BodyDedupeState, BodyRebuildError> {
    let observation = DirectoryObservation {
        name: bundle.entry.name.as_encoded_bytes(),
        envelope_present: is_regular_file(&bundle.children, ENVELOPE_NAME),
        ledger_present: is_regular_file(&bundle.children, LEDGER_NAME),
        manifest: bundle.manifest.as_deref(),
    };
    let authority = authorize_native_bundle(observation)
        .map_err(|_| error(BodyRebuildErrorKind::Authority, "native_authority"))?;
    let envelope_entry = required_regular_file(&bundle.children, ENVELOPE_NAME)?;
    let envelope_bytes =
        read_bounded_file(&envelope_entry.path, MAX_DOCUMENT_BYTES.saturating_add(1))?;
    let envelope = decode_body_envelope_with_manifest(&envelope_bytes, authority.binding())
        .map_err(|_| error(BodyRebuildErrorKind::Envelope, "decode_envelope"))?;
    replay_native_files(bundle, &envelope, state)
}

fn replay_native_files(
    bundle: &ObservedBundle,
    envelope: &BodyEnvelope,
    state: BodyDedupeState,
) -> Result<BodyDedupeState, BodyRebuildError> {
    let raw_inventory_digest = validate_raw_inventory(bundle, envelope.raw_retention())?;
    let ledger_entry = required_regular_file(&bundle.children, LEDGER_NAME)?;
    let ledger_file = open_nofollow(&ledger_entry.path)
        .map_err(|_| error(BodyRebuildErrorKind::Journal, "open_native_ledger"))?;
    let mut ledger = BufReader::new(ledger_file);
    let normalized_entry = named_entry(&bundle.children, NORMALIZED_NAME);
    let normalized_children = match normalized_entry {
        None if envelope.shards().is_empty() => Vec::new(),
        Some(entry) if entry.kind == DirEntryKind::Directory => list_dir_entries(&entry.path)
            .map_err(|_| error(BodyRebuildErrorKind::Journal, "list_native_shards"))?,
        _ => {
            return Err(error(
                BodyRebuildErrorKind::NativeReplay,
                "normalized_directory",
            ));
        }
    };
    let expected_names: BTreeSet<String> = envelope
        .shards()
        .iter()
        .map(|shard| {
            shard
                .path()
                .strip_prefix("normalized/")
                .expect("checked envelope shard path has normalized prefix")
                .to_owned()
        })
        .collect();
    let actual_names: BTreeSet<String> = normalized_children
        .iter()
        .map(|entry| entry.name.to_string_lossy().into_owned())
        .collect();
    if normalized_children
        .iter()
        .any(|entry| entry.kind != DirEntryKind::File)
        || actual_names != expected_names
    {
        return Err(error(BodyRebuildErrorKind::NativeReplay, "shard_inventory"));
    }

    let mut replay = BodyBundleReplay::with_state(envelope, state)
        .map_err(|_| error(BodyRebuildErrorKind::NativeReplay, "start_native_replay"))?;
    for (index, descriptor) in envelope.shards().iter().enumerate() {
        let name = descriptor
            .path()
            .strip_prefix("normalized/")
            .expect("checked envelope shard path has normalized prefix");
        let shard_entry = named_entry(&normalized_children, name)
            .expect("exact inventory comparison guarantees each shard entry");
        let shard_file = open_nofollow(&shard_entry.path)
            .map_err(|_| error(BodyRebuildErrorKind::Journal, "open_native_shard"))?;
        let mut shard = BufReader::new(shard_file);
        for _ in 0..descriptor.rows() {
            let row_frame = required_frame(&mut shard, MAX_DOCUMENT_BYTES, "native_row_frame")?;
            validate_row_raw_inventory(&row_frame, raw_inventory_digest.as_deref())?;
            let ledger_frame =
                required_frame(&mut ledger, MAX_LEDGER_FRAME_BYTES, "native_ledger_frame")?;
            let shard_index = u64::try_from(index)
                .map_err(|_| error(BodyRebuildErrorKind::NativeReplay, "shard_index"))?;
            replay
                .push(shard_index, &row_frame, &ledger_frame)
                .map_err(|_| error(BodyRebuildErrorKind::NativeReplay, "row_event_agreement"))?;
        }
        if next_frame(&mut shard, MAX_DOCUMENT_BYTES)?.is_some() {
            return Err(error(
                BodyRebuildErrorKind::NativeReplay,
                "extra_shard_bytes",
            ));
        }
    }
    if next_frame(&mut ledger, MAX_LEDGER_FRAME_BYTES)?.is_some() {
        return Err(error(
            BodyRebuildErrorKind::NativeReplay,
            "extra_ledger_bytes",
        ));
    }
    replay
        .finish()
        .map(|validated| validated.into_state())
        .map_err(|_| error(BodyRebuildErrorKind::NativeReplay, "finish_native_replay"))
}

fn validate_raw_inventory(
    bundle: &ObservedBundle,
    retention: BodyRawRetention,
) -> Result<Option<String>, BodyRebuildError> {
    let inventory_entry = named_entry(&bundle.children, RAW_INVENTORY_NAME);
    let raw_entry = named_entry(&bundle.children, RAW_NAME);
    if retention == BodyRawRetention::Discard {
        if inventory_entry.is_some() || raw_entry.is_some() {
            return Err(error(
                BodyRebuildErrorKind::NativeReplay,
                "raw_retention_mismatch",
            ));
        }
        return Ok(None);
    }
    let inventory_entry = inventory_entry
        .ok_or_else(|| error(BodyRebuildErrorKind::NativeReplay, "raw_inventory_missing"))?;
    if inventory_entry.kind != DirEntryKind::File {
        return Err(error(
            BodyRebuildErrorKind::NativeReplay,
            "raw_inventory_kind",
        ));
    }
    let raw_entry = raw_entry
        .filter(|entry| entry.kind == DirEntryKind::Directory)
        .ok_or_else(|| error(BodyRebuildErrorKind::NativeReplay, "raw_directory"))?;
    let inventory_bytes = read_bounded_file(
        &inventory_entry.path,
        MAX_RAW_INVENTORY_BYTES.saturating_add(1),
    )?;
    if inventory_bytes.len() > MAX_RAW_INVENTORY_BYTES || !inventory_bytes.ends_with(b"\n") {
        return Err(error(
            BodyRebuildErrorKind::NativeReplay,
            "raw_inventory_size",
        ));
    }
    let mut declared = BTreeMap::new();
    let mut declared_total = 0_u64;
    let mut previous_path: Option<String> = None;
    for line in inventory_bytes[..inventory_bytes.len() - 1].split(|byte| *byte == b'\n') {
        if line.is_empty() {
            return Err(error(
                BodyRebuildErrorKind::NativeReplay,
                "raw_inventory_row",
            ));
        }
        let body_value = parse(line)
            .map_err(|_| error(BodyRebuildErrorKind::NativeReplay, "raw_inventory_row"))?;
        if canonicalize(&body_value).ok().as_deref() != std::str::from_utf8(line).ok() {
            return Err(error(
                BodyRebuildErrorKind::NativeReplay,
                "raw_inventory_canonical",
            ));
        }
        let object = serde_json::from_slice::<serde_json::Value>(line)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .ok_or_else(|| error(BodyRebuildErrorKind::NativeReplay, "raw_inventory_row"))?;
        if object.len() != 3 {
            return Err(error(
                BodyRebuildErrorKind::NativeReplay,
                "raw_inventory_row",
            ));
        }
        let path = object
            .get("path")
            .and_then(serde_json::Value::as_str)
            .filter(|path| valid_raw_path(path))
            .ok_or_else(|| error(BodyRebuildErrorKind::NativeReplay, "raw_inventory_path"))?;
        if previous_path
            .as_deref()
            .is_some_and(|previous| previous >= path)
        {
            return Err(error(
                BodyRebuildErrorKind::NativeReplay,
                "raw_inventory_order",
            ));
        }
        previous_path = Some(path.to_owned());
        let bytes = object
            .get("bytes")
            .and_then(serde_json::Value::as_u64)
            .filter(|bytes| *bytes <= MAX_RAW_ASSET_BYTES)
            .ok_or_else(|| error(BodyRebuildErrorKind::NativeReplay, "raw_inventory_bytes"))?;
        declared_total = declared_total
            .checked_add(bytes)
            .filter(|total| *total <= MAX_RAW_TOTAL_BYTES)
            .ok_or_else(|| error(BodyRebuildErrorKind::NativeReplay, "raw_bytes_limit"))?;
        let sha256 = object
            .get("sha256")
            .and_then(serde_json::Value::as_str)
            .filter(|digest| valid_digest(digest))
            .ok_or_else(|| error(BodyRebuildErrorKind::NativeReplay, "raw_inventory_digest"))?;
        if declared
            .insert(path.to_owned(), (bytes, sha256.to_owned()))
            .is_some()
            || declared.len() > MAX_RAW_ASSETS
        {
            return Err(error(
                BodyRebuildErrorKind::NativeReplay,
                "raw_inventory_duplicate",
            ));
        }
    }
    if declared.is_empty() {
        return Err(error(
            BodyRebuildErrorKind::NativeReplay,
            "raw_inventory_empty",
        ));
    }
    let expected_directories = expected_raw_directories(declared.keys())?;
    let actual = collect_raw_files(&raw_entry.path)?;
    if actual.files != declared || actual.directories != expected_directories {
        return Err(error(
            BodyRebuildErrorKind::NativeReplay,
            "raw_inventory_mismatch",
        ));
    }
    Ok(Some(format!(
        "sha256:{:x}",
        Sha256::digest(&inventory_bytes)
    )))
}

fn expected_raw_directories<'a>(
    paths: impl Iterator<Item = &'a String>,
) -> Result<BTreeSet<String>, BodyRebuildError> {
    let mut directories = BTreeSet::new();
    for path in paths {
        let parts = path.split('/').collect::<Vec<_>>();
        if parts.len().saturating_sub(1) > MAX_RAW_DEPTH {
            return Err(error(
                BodyRebuildErrorKind::NativeReplay,
                "raw_asset_depth_limit",
            ));
        }
        for end in 1..parts.len() {
            directories.insert(parts[..end].join("/"));
            if directories.len() > MAX_RAW_DIRECTORIES {
                return Err(error(
                    BodyRebuildErrorKind::NativeReplay,
                    "raw_directory_limit",
                ));
            }
        }
    }
    Ok(directories)
}

fn collect_raw_files(root: &Path) -> Result<RawInventoryTree, BodyRebuildError> {
    collect_raw_files_with_limit(root, MAX_RAW_TOTAL_BYTES)
}

fn collect_raw_files_with_limit(
    root: &Path,
    total_limit: u64,
) -> Result<RawInventoryTree, BodyRebuildError> {
    let mut files = BTreeMap::new();
    let mut directories = BTreeSet::new();
    let mut total = 0_u64;
    let mut pending = vec![(root.to_owned(), 0_usize)];
    while let Some((directory, depth)) = pending.pop() {
        let remaining = MAX_RAW_ASSETS
            .saturating_add(MAX_RAW_DIRECTORIES)
            .saturating_sub(files.len().saturating_add(directories.len()));
        let entries = list_dir_entries_bounded(&directory, remaining)
            .map_err(|_| error(BodyRebuildErrorKind::Journal, "list_raw_assets"))?
            .ok_or_else(|| error(BodyRebuildErrorKind::NativeReplay, "raw_entry_limit"))?;
        for entry in entries {
            match entry.kind {
                DirEntryKind::Directory => {
                    let child_depth = depth.checked_add(1).ok_or_else(|| {
                        error(BodyRebuildErrorKind::NativeReplay, "raw_asset_depth_limit")
                    })?;
                    if child_depth > MAX_RAW_DEPTH {
                        return Err(error(
                            BodyRebuildErrorKind::NativeReplay,
                            "raw_asset_depth_limit",
                        ));
                    }
                    if directories.len() >= MAX_RAW_DIRECTORIES {
                        return Err(error(
                            BodyRebuildErrorKind::NativeReplay,
                            "raw_directory_limit",
                        ));
                    }
                    let relative = entry
                        .path
                        .strip_prefix(root)
                        .ok()
                        .and_then(Path::to_str)
                        .filter(|path| valid_raw_path(path))
                        .ok_or_else(|| error(BodyRebuildErrorKind::NativeReplay, "raw_asset_path"))?
                        .to_owned();
                    if !directories.insert(relative) {
                        return Err(error(
                            BodyRebuildErrorKind::NativeReplay,
                            "raw_inventory_mismatch",
                        ));
                    }
                    pending.push((entry.path, child_depth));
                }
                DirEntryKind::File => {
                    if files.len() >= MAX_RAW_ASSETS {
                        return Err(error(BodyRebuildErrorKind::NativeReplay, "raw_asset_limit"));
                    }
                    let relative = entry
                        .path
                        .strip_prefix(root)
                        .ok()
                        .and_then(Path::to_str)
                        .filter(|path| valid_raw_path(path))
                        .ok_or_else(|| {
                            error(BodyRebuildErrorKind::NativeReplay, "raw_asset_path")
                        })?;
                    let remaining = total_limit.checked_sub(total).ok_or_else(|| {
                        error(BodyRebuildErrorKind::NativeReplay, "raw_bytes_limit")
                    })?;
                    let value = hash_raw_file(&entry.path, remaining)?;
                    total = total.checked_add(value.0).ok_or_else(|| {
                        error(BodyRebuildErrorKind::NativeReplay, "raw_bytes_limit")
                    })?;
                    files.insert(relative.to_owned(), value);
                }
                DirEntryKind::Other => {
                    return Err(error(BodyRebuildErrorKind::NativeReplay, "raw_asset_kind"));
                }
            }
        }
    }
    Ok(RawInventoryTree { files, directories })
}

fn hash_raw_file(path: &Path, total_remaining: u64) -> Result<(u64, String), BodyRebuildError> {
    let mut file =
        open_nofollow(path).map_err(|_| error(BodyRebuildErrorKind::Journal, "open_raw_asset"))?;
    let metadata = file
        .metadata()
        .map_err(|_| error(BodyRebuildErrorKind::Journal, "raw_asset_metadata"))?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_RAW_ASSET_BYTES {
        return Err(error(BodyRebuildErrorKind::NativeReplay, "raw_asset_size"));
    }
    if metadata.len() > total_remaining {
        return Err(error(BodyRebuildErrorKind::NativeReplay, "raw_bytes_limit"));
    }
    hash_raw_reader(&mut file, total_remaining)
}

fn hash_raw_reader(
    reader: &mut impl Read,
    total_remaining: u64,
) -> Result<(u64, String), BodyRebuildError> {
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let probe = total_remaining
            .saturating_sub(total)
            .saturating_add(1)
            .min(buffer.len() as u64) as usize;
        let read = reader
            .read(&mut buffer[..probe])
            .map_err(|_| error(BodyRebuildErrorKind::Journal, "read_raw_asset"))?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .filter(|total| *total <= MAX_RAW_ASSET_BYTES)
            .ok_or_else(|| error(BodyRebuildErrorKind::NativeReplay, "raw_asset_size"))?;
        if total > total_remaining {
            return Err(error(BodyRebuildErrorKind::NativeReplay, "raw_bytes_limit"));
        }
        digest.update(&buffer[..read]);
    }
    Ok((total, format!("sha256:{:x}", digest.finalize())))
}

#[cfg(unix)]
fn open_nofollow(path: &Path) -> std::io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK | nix::libc::O_CLOEXEC)
        .open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "body bundle entry is not a regular file",
        ));
    }
    Ok(file)
}

#[cfg(windows)]
fn open_nofollow(path: &Path) -> std::io::Result<File> {
    // Open the reparse point itself. Its handle metadata then refuses a link
    // instead of resolving an attacker-controlled target between inspection
    // and the read.
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "body bundle entry is not a regular file",
        ));
    }
    Ok(file)
}

fn validate_row_raw_inventory(row: &[u8], expected: Option<&str>) -> Result<(), BodyRebuildError> {
    let value =
        parse(row).map_err(|_| error(BodyRebuildErrorKind::NativeReplay, "raw_inventory_row"))?;
    let BodyValue::Object(object) = value else {
        return Err(error(
            BodyRebuildErrorKind::NativeReplay,
            "raw_inventory_row",
        ));
    };
    let actual = object.get(&ascii_body_string(RAW_INVENTORY_FIELD));
    let matches = match (actual, expected) {
        (None, None) => true,
        (Some(BodyValue::String(actual)), Some(expected)) => actual
            .code_points()
            .iter()
            .copied()
            .eq(expected.bytes().map(u32::from)),
        _ => false,
    };
    if !matches {
        return Err(error(
            BodyRebuildErrorKind::NativeReplay,
            "raw_inventory_binding",
        ));
    }
    Ok(())
}

fn valid_raw_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4_096
        && !path.contains('\\')
        && Path::new(path)
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn valid_digest(digest: &str) -> bool {
    digest.len() == 71
        && digest.starts_with("sha256:")
        && digest[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn replay_legacy_bundle(
    bundle: &ObservedBundle,
    mut state: BodyDedupeState,
) -> Result<(BodyDedupeState, bool), BodyRebuildError> {
    let manifest_family = bundle
        .manifest
        .as_deref()
        .is_some_and(legacy_manifest_body_family);
    let normalized_entry = named_entry(&bundle.children, NORMALIZED_NAME);
    if normalized_entry.is_none() && !manifest_family {
        return Ok((state, false));
    }
    let import_id = bundle
        .entry
        .name
        .to_str()
        .ok_or_else(|| error(BodyRebuildErrorKind::LegacyReplay, "import_id"))?;
    let shards = match normalized_entry {
        None => Vec::new(),
        Some(entry) if entry.kind == DirEntryKind::Directory => list_dir_entries(&entry.path)
            .map_err(|_| error(BodyRebuildErrorKind::Journal, "list_legacy_shards"))?,
        Some(_) => {
            return Err(error(
                BodyRebuildErrorKind::LegacyReplay,
                "normalized_directory",
            ));
        }
    };
    for shard in shards {
        if shard.kind != DirEntryKind::File {
            return Err(error(
                BodyRebuildErrorKind::LegacyReplay,
                "legacy_shard_kind",
            ));
        }
        let name = shard
            .name
            .to_str()
            .ok_or_else(|| error(BodyRebuildErrorKind::LegacyReplay, "legacy_shard_name"))?;
        let month_text = name
            .strip_suffix(".jsonl")
            .ok_or_else(|| error(BodyRebuildErrorKind::LegacyReplay, "legacy_shard_name"))?;
        let month = BodyMonth::from_bytes(month_text.as_bytes())
            .map_err(|_| error(BodyRebuildErrorKind::LegacyReplay, "legacy_shard_month"))?;
        replay_legacy_shard(&shard.path, import_id, &month, &mut state)?;
    }
    Ok((state, true))
}

fn replay_legacy_shard(
    path: &Path,
    import_id: &str,
    month: &BodyMonth,
    state: &mut BodyDedupeState,
) -> Result<(), BodyRebuildError> {
    let file = open_nofollow(path)
        .map_err(|_| error(BodyRebuildErrorKind::Journal, "open_legacy_shard"))?;
    let mut reader = BufReader::new(file);
    let mut line = 0_u64;
    while let Some(frame) = next_frame(&mut reader, MAX_DOCUMENT_BYTES)? {
        line = line.saturating_add(1);
        if !frame.ends_with(b"\n") {
            return Err(error(
                BodyRebuildErrorKind::LegacyReplay,
                "legacy_row_framing",
            ));
        }
        let value = parse(&frame[..frame.len() - 1])
            .map_err(|_| error(BodyRebuildErrorKind::LegacyReplay, "legacy_row_json"))?;
        let coordinate = solstone_core_body_source::Coordinate::new(
            import_id,
            format!("{}.jsonl", month.as_str()),
            line,
        );
        let presentation = PresentationRow::new(&value, &coordinate)
            .map_err(|_| error(BodyRebuildErrorKind::LegacyReplay, "legacy_row_object"))?;
        let candidate = project(&presentation, coordinate)
            .map_err(|_| error(BodyRebuildErrorKind::LegacyReplay, "legacy_row_projection"))?;
        let checked = validate_legacy_body_row(&candidate, import_id, month, line)
            .map_err(|_| error(BodyRebuildErrorKind::LegacyReplay, "legacy_row_provenance"))?;
        state.apply_legacy(&checked);
    }
    Ok(())
}

fn publish_database(journal_root: &Path, state: &BodyDedupeState) -> Result<(), BodyRebuildError> {
    publish_database_with_installer(journal_root, state, |temporary_path, database_path| {
        install_file(
            temporary_path,
            database_path,
            AtomicWriteOptions { mode: Some(0o600) },
        )
        .map_err(|_| error(BodyRebuildErrorKind::Publication, "install_database"))
    })
}

fn publish_database_with_installer(
    journal_root: &Path,
    state: &BodyDedupeState,
    installer: impl FnOnce(&Path, &Path) -> Result<(), BodyRebuildError>,
) -> Result<(), BodyRebuildError> {
    let temp_path = journal_root.join(TEMP_DATABASE_REL);
    let database_path = journal_root.join(DATABASE_REL);
    remove_if_present(journal_root, TEMP_DATABASE_REL)?;
    write_bytes_exclusive(&temp_path, &[], AtomicWriteOptions { mode: Some(0o600) })
        .map_err(|_| error(BodyRebuildErrorKind::Publication, "create_temp_database"))?;

    let build_result = build_database(&temp_path, state);
    if let Err(error) = build_result {
        let _ = remove_file(journal_root, TEMP_DATABASE_REL);
        return Err(error);
    }

    let publication = (|| {
        checkpoint_existing_database(&database_path)?;
        for suffix in ["-wal", "-shm", "-journal"] {
            remove_if_present(journal_root, &format!("{DATABASE_REL}{suffix}"))?;
        }
        installer(&temp_path, &database_path)?;
        for suffix in ["-wal", "-shm", "-journal"] {
            remove_if_present(journal_root, &format!("{DATABASE_REL}{suffix}"))?;
        }
        sync_dir(journal_root, IMPORTS_DIR)
            .map_err(|_| error(BodyRebuildErrorKind::Publication, "sync_imports"))?;
        Ok(())
    })();
    if publication.is_err() {
        let _ = remove_file(journal_root, TEMP_DATABASE_REL);
    }
    publication
}

fn checkpoint_existing_database(path: &Path) -> Result<(), BodyRebuildError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            return Err(error(
                BodyRebuildErrorKind::Publication,
                "existing_database",
            ));
        }
    };
    if !metadata.file_type().is_file() {
        return Err(error(
            BodyRebuildErrorKind::Publication,
            "existing_database",
        ));
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| error(BodyRebuildErrorKind::Publication, "checkpoint_database"))?;
    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
        .map_err(|_| error(BodyRebuildErrorKind::Publication, "checkpoint_database"))?;
    if !journal_mode.eq_ignore_ascii_case("delete") {
        return Err(error(
            BodyRebuildErrorKind::Publication,
            "checkpoint_database",
        ));
    }
    connection
        .close()
        .map_err(|_| error(BodyRebuildErrorKind::Publication, "checkpoint_database"))?;
    Ok(())
}

fn build_database(path: &Path, state: &BodyDedupeState) -> Result<(), BodyRebuildError> {
    let mut connection = Connection::open(path)
        .map_err(|_| error(BodyRebuildErrorKind::Sqlite, "open_temp_database"))?;
    connection
        .execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;")
        .map_err(|_| error(BodyRebuildErrorKind::Sqlite, "configure_database"))?;
    connection
        .execute_batch(SCHEMA)
        .map_err(|_| error(BodyRebuildErrorKind::Sqlite, "create_schema"))?;
    let transaction = connection
        .transaction()
        .map_err(|_| error(BodyRebuildErrorKind::Sqlite, "begin_rows"))?;
    {
        let mut insert = transaction
            .prepare(
                "INSERT INTO health_dedupe (
                    dedupe_key, source_family, source_record_id, record_type,
                    start_time, end_time, value_hash, first_import_id,
                    last_seen_import_id, normalized_ref, raw_ref, created_at, updated_at
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .map_err(|_| error(BodyRebuildErrorKind::Sqlite, "prepare_rows"))?;
        for row in state.iter() {
            insert
                .execute(params![
                    row.dedupe_key(),
                    row.source_family().as_str(),
                    row.source_record_id(),
                    row.record_type(),
                    row.start_time(),
                    row.end_time(),
                    row.value_hash().map(|value| value.as_str()),
                    row.first_import_id(),
                    row.latest_import_id(),
                    row.normalized_ref(),
                    row.raw_ref(),
                    REBUILD_TIMESTAMP,
                    REBUILD_TIMESTAMP,
                ])
                .map_err(|_| error(BodyRebuildErrorKind::Sqlite, "insert_rows"))?;
        }
    }
    transaction
        .commit()
        .map_err(|_| error(BodyRebuildErrorKind::Sqlite, "commit_rows"))?;
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|_| error(BodyRebuildErrorKind::Sqlite, "integrity_check"))?;
    if integrity != "ok" {
        return Err(error(BodyRebuildErrorKind::Sqlite, "integrity_check"));
    }
    connection
        .close()
        .map_err(|_| error(BodyRebuildErrorKind::Sqlite, "close_database"))?;
    Ok(())
}

fn legacy_manifest_body_family(input: &[u8]) -> bool {
    let Ok(BodyValue::Object(object)) = parse(input) else {
        return false;
    };
    let key = ascii_body_string("source_type");
    let Some(BodyValue::String(value)) = object.get(&key) else {
        return false;
    };
    body_string_matches(value, "apple_health") || body_string_matches(value, "oura_api")
}

fn body_string_matches(value: &BodyString, expected: &str) -> bool {
    value
        .code_points()
        .iter()
        .copied()
        .eq(expected.bytes().map(u32::from))
}

fn ascii_body_string(value: &str) -> BodyString {
    BodyString::from_code_points(value.bytes().map(u32::from).collect())
        .expect("ASCII literal is a valid body string")
}

fn named_entry<'a>(entries: &'a [DirEntry], name: &str) -> Option<&'a DirEntry> {
    entries.iter().find(|entry| entry.name == OsStr::new(name))
}

fn is_regular_file(entries: &[DirEntry], name: &str) -> bool {
    named_entry(entries, name).is_some_and(|entry| entry.kind == DirEntryKind::File)
}

fn required_regular_file<'a>(
    entries: &'a [DirEntry],
    name: &str,
) -> Result<&'a DirEntry, BodyRebuildError> {
    named_entry(entries, name)
        .filter(|entry| entry.kind == DirEntryKind::File)
        .ok_or_else(|| error(BodyRebuildErrorKind::Journal, "required_bundle_file"))
}

fn read_bounded_file(path: &Path, limit: usize) -> Result<Vec<u8>, BodyRebuildError> {
    let file = open_nofollow(path)
        .map_err(|_| error(BodyRebuildErrorKind::Journal, "open_bundle_file"))?;
    let mut bytes = Vec::new();
    file.take(u64::try_from(limit).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .map_err(|_| error(BodyRebuildErrorKind::Journal, "read_bundle_file"))?;
    Ok(bytes)
}

fn required_frame(
    reader: &mut impl BufRead,
    limit: usize,
    stage: &'static str,
) -> Result<Vec<u8>, BodyRebuildError> {
    let frame = next_frame(reader, limit)?
        .ok_or_else(|| error(BodyRebuildErrorKind::NativeReplay, stage))?;
    if !frame.ends_with(b"\n") {
        return Err(error(BodyRebuildErrorKind::NativeReplay, stage));
    }
    Ok(frame)
}

fn next_frame(
    reader: &mut impl BufRead,
    limit: usize,
) -> Result<Option<Vec<u8>>, BodyRebuildError> {
    let mut frame = Vec::new();
    loop {
        let available = reader
            .fill_buf()
            .map_err(|_| error(BodyRebuildErrorKind::Journal, "read_frame"))?;
        if available.is_empty() {
            return Ok((!frame.is_empty()).then_some(frame));
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        if frame.len().saturating_add(take) > limit {
            return Err(error(BodyRebuildErrorKind::NativeReplay, "frame_too_large"));
        }
        let complete = available.get(take.saturating_sub(1)) == Some(&b'\n');
        frame.extend_from_slice(&available[..take]);
        reader.consume(take);
        if complete {
            return Ok(Some(frame));
        }
    }
}

fn remove_if_present(journal_root: &Path, rel: &str) -> Result<(), BodyRebuildError> {
    match remove_file(journal_root, rel)
        .map_err(|_| error(BodyRebuildErrorKind::Publication, "remove_sidecar"))?
    {
        Removed::Unlinked | Removed::AlreadyAbsent => Ok(()),
    }
}

fn error(kind: BodyRebuildErrorKind, stage: &'static str) -> BodyRebuildError {
    BodyRebuildError::new(kind, stage)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::test_support::reserve_temp_path;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let path = reserve_temp_path("solstone-body-publication");
            fs::create_dir(&path).expect("temporary directory creates");
            let path = fs::canonicalize(path).expect("temporary directory canonicalizes");
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
    fn raw_hashing_stops_at_the_remaining_aggregate_budget() {
        let mut oversized = std::io::Cursor::new(b"abcdef".to_vec());
        let error = hash_raw_reader(&mut oversized, 3)
            .expect_err("the aggregate raw budget must stop the reader");
        assert_eq!(error.kind(), BodyRebuildErrorKind::NativeReplay);
        assert_eq!(error.stage(), "raw_bytes_limit");
        assert_eq!(oversized.position(), 4, "only remaining + 1 is read");

        let mut exact = std::io::Cursor::new(b"abc".to_vec());
        let (bytes, digest) = hash_raw_reader(&mut exact, 3).expect("exact budget succeeds");
        assert_eq!(bytes, 3);
        assert_eq!(
            digest,
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn failed_install_preserves_committed_state_that_started_only_in_wal() {
        let temporary = TempDir::new();
        let imports = temporary.path().join(IMPORTS_DIR);
        fs::create_dir(&imports).expect("imports creates");
        let source_path = temporary.path().join("wal-source.sqlite");
        let source = Connection::open(&source_path).expect("WAL source opens");
        source
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA wal_autocheckpoint=0;
                 CREATE TABLE previous_owner_state (value TEXT NOT NULL);
                 INSERT INTO previous_owner_state VALUES ('committed-in-wal');",
            )
            .expect("WAL-only owner state commits");
        let source_wal = PathBuf::from(format!("{}-wal", source_path.display()));
        assert!(fs::metadata(&source_wal).expect("source WAL exists").len() > 32);

        let database = temporary.path().join(DATABASE_REL);
        fs::copy(&source_path, &database).expect("main database copies");
        fs::copy(
            &source_wal,
            PathBuf::from(format!("{}-wal", database.display())),
        )
        .expect("WAL copies");
        let main_only = temporary.path().join("main-only.sqlite");
        fs::copy(&source_path, &main_only).expect("main-only proof copies");
        let main_only_connection = Connection::open(&main_only).expect("main-only opens");
        assert!(
            main_only_connection
                .query_row("SELECT value FROM previous_owner_state", [], |row| {
                    row.get::<_, String>(0)
                })
                .is_err(),
            "the committed witness must exist only in the WAL before publication"
        );
        drop(main_only_connection);
        drop(source);

        let failure = publish_database_with_installer(
            temporary.path(),
            &BodyDedupeState::new(),
            |_temporary_path, _database_path| {
                Err(error(
                    BodyRebuildErrorKind::Publication,
                    "injected_install_failure",
                ))
            },
        )
        .expect_err("injected install fails");
        assert_eq!(failure.stage(), "injected_install_failure");
        assert!(!temporary.path().join(TEMP_DATABASE_REL).exists());

        let prior = Connection::open(&database).expect("prior database remains readable");
        let value: String = prior
            .query_row("SELECT value FROM previous_owner_state", [], |row| {
                row.get(0)
            })
            .expect("committed WAL row survives failed install");
        assert_eq!(value, "committed-in-wal");
        assert!(!PathBuf::from(format!("{}-wal", database.display())).exists());
        assert!(!PathBuf::from(format!("{}-shm", database.display())).exists());
    }
}
