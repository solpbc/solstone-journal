// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use getrandom::fill as random_fill;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use solstone_core_body_rebuild::rebuild_body_store;
use solstone_core_body_source::{
    AppleSummaryPlan, BodyDay, BodyDigest, BodyEnvelope, BodyLedgerEvent, BodyManifestBinding,
    BodyMonth, BodyRawRetention, BodySourceFamily, BodySourceHash, BodyString, BodyValue, BundleId,
    Coordinate, DirectoryObservation, EnvelopeLedger, EnvelopeShard, FieldState,
    HealthRecordIdentity, LedgerCandidate, MAX_BODY_ROW_FRAME_BYTES, PresentationRow,
    authorize_native_bundle, canonicalize, encode_body_envelope, encode_body_ledger_event,
    health_record_dedupe_key, health_value_hash, parse, project,
};
use solstone_core_journal_io::{
    AtomicWriteOptions, LockOptions, StagedDirOptions, create_directory_with_mode, hold_lock,
    publish_staged_dir, remove_dir_all, write_bytes_exclusive, write_reader_exclusive,
};

use crate::bounded_file::{open_regular_file, read_bounded_regular};

const EMPTY_DIGEST: &str =
    "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const PLACEHOLDER_DIGEST: &str =
    "sha256:1111111111111111111111111111111111111111111111111111111111111111";
const PLACEHOLDER_DEDUPE: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";
const RAW_INVENTORY_NAME: &str = "body-raw-inventory.jsonl";
const RAW_INVENTORY_FIELD: &str = "raw_inventory_sha256";
const MAX_NORMALIZED_ROWS: usize = 100_000;
const MAX_NORMALIZED_BYTES: usize = 128 * 1024 * 1024;
pub(crate) const MAX_RAW_ASSETS: usize = 10_000;
const MAX_RAW_ASSET_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_RAW_TOTAL_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub(crate) const MAX_RAW_PATH_BYTES: usize = 4_096;
const MAX_RAW_INVENTORY_BYTES: usize = 16 * 1024 * 1024;
const MAX_IMPORT_ENTRIES_SCANNED: usize = 100_000;
const MAX_STALE_STAGING_DIRECTORIES: usize = 1_024;
const MAX_EXISTING_MANIFEST_BYTES: usize = 1_048_576;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyIngestErrorKind {
    Gate,
    Source,
    Normalize,
    Publication,
    Rebuild,
}

#[derive(Clone, PartialEq, Eq)]
pub struct BodyIngestError {
    kind: BodyIngestErrorKind,
    stage: &'static str,
}

impl BodyIngestError {
    pub(crate) const fn new(kind: BodyIngestErrorKind, stage: &'static str) -> Self {
        Self { kind, stage }
    }

    pub(crate) const fn gate(stage: &'static str) -> Self {
        Self::new(BodyIngestErrorKind::Gate, stage)
    }

    pub fn kind(&self) -> BodyIngestErrorKind {
        self.kind
    }

    pub fn stage(&self) -> &'static str {
        self.stage
    }
}

impl fmt::Display for BodyIngestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            BodyIngestErrorKind::Gate => "gate",
            BodyIngestErrorKind::Source => "source",
            BodyIngestErrorKind::Normalize => "normalize",
            BodyIngestErrorKind::Publication => "publication",
            BodyIngestErrorKind::Rebuild => "rebuild",
        };
        write!(formatter, "body-ingest {kind}: {}", self.stage)
    }
}

impl fmt::Debug for BodyIngestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for BodyIngestError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyIngestReport {
    bundle_id: Option<String>,
    rows: u64,
    days: Vec<String>,
    skipped: bool,
}

impl BodyIngestReport {
    pub(crate) fn preview(rows: u64, days: Vec<String>) -> Self {
        Self {
            bundle_id: None,
            rows,
            days,
            skipped: false,
        }
    }

    pub fn bundle_id(&self) -> Option<&str> {
        self.bundle_id.as_deref()
    }

    pub fn rows(&self) -> u64 {
        self.rows
    }

    pub fn days(&self) -> &[String] {
        &self.days
    }

    pub fn skipped(&self) -> bool {
        self.skipped
    }
}

pub(crate) struct NormalizedInput {
    pub row: Map<String, Value>,
    pub identity_metadata: Option<Map<String, Value>>,
    pub raw_locator: Option<String>,
}

pub(crate) enum RawAsset {
    File {
        source: PathBuf,
        relative: String,
    },
    ZipMember {
        archive: PathBuf,
        member: String,
        relative: String,
    },
    Bytes {
        bytes: Vec<u8>,
        relative: String,
    },
}

struct PreparedRawAsset {
    asset: RawAsset,
    relative: String,
    bytes: u64,
    sha256: String,
}

struct PreparedRawInventory {
    assets: Vec<PreparedRawAsset>,
    bytes: Vec<u8>,
    sha256: String,
}

struct PreparedRow {
    value: BodyValue,
    bytes: Vec<u8>,
    value_hash: BodyDigest,
}

struct PreparedShard {
    month: BodyMonth,
    bytes: Vec<u8>,
    rows: Vec<PreparedRow>,
}

pub(crate) fn publish(
    journal: &Path,
    family: BodySourceFamily,
    source_hash_text: String,
    retention: BodyRawRetention,
    inputs: Vec<NormalizedInput>,
    raw_assets: Vec<RawAsset>,
    force: bool,
) -> Result<BodyIngestReport, BodyIngestError> {
    let imports = journal.join("imports");
    create_directory_with_mode(&imports, 0o700)
        .map_err(|_| error(BodyIngestErrorKind::Publication, "imports_directory"))?;
    let _publication_lock = hold_lock(
        imports.join("native-body-publish"),
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(|_| error(BodyIngestErrorKind::Publication, "bundle_lock"))?;
    clean_stale_bundle_staging(journal, &imports)?;
    if inputs.len() > MAX_NORMALIZED_ROWS {
        return Err(error(BodyIngestErrorKind::Source, "row_limit"));
    }
    let source_hash = BodySourceHash::from_bytes_for_family(source_hash_text.as_bytes(), &family)
        .map_err(|_| error(BodyIngestErrorKind::Normalize, "source_hash"))?;
    let raw_inventory = prepare_raw_inventory(raw_assets)?;
    if (retention == BodyRawRetention::Discard) != raw_inventory.is_none() {
        return Err(error(
            BodyIngestErrorKind::Normalize,
            "raw_retention_inventory",
        ));
    }
    if !force
        && let Some(existing) = find_existing(
            &imports,
            family,
            &source_hash,
            retention,
            raw_inventory
                .as_ref()
                .map(|inventory| inventory.sha256.as_str()),
        )?
    {
        rebuild_body_store(journal)
            .map_err(|_| error(BodyIngestErrorKind::Rebuild, "dedupe_store"))?;
        return Ok(existing);
    }
    let raw_inventory_paths = raw_inventory
        .as_ref()
        .map(|inventory| {
            inventory
                .assets
                .iter()
                .map(|asset| asset.relative.as_str())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let bundle = new_bundle_id(&imports)?;
    let mut grouped: BTreeMap<String, Vec<NormalizedInput>> = BTreeMap::new();
    for mut input in inputs {
        if let Some(locator) = input.raw_locator.as_deref() {
            let path = locator.split_once('#').map_or(locator, |(path, _)| path);
            if !raw_inventory_paths.contains(path) {
                return Err(error(BodyIngestErrorKind::Normalize, "raw_locator"));
            }
        }
        if let Some(inventory) = &raw_inventory {
            input.row.insert(
                RAW_INVENTORY_FIELD.to_owned(),
                Value::String(inventory.sha256.clone()),
            );
        }
        let day = string_field(&input.row, "day")?;
        let checked_day = BodyDay::from_bytes(day.as_bytes())
            .map_err(|_| error(BodyIngestErrorKind::Normalize, "day"))?;
        if !source_hash.includes_day(&checked_day) {
            return Err(error(BodyIngestErrorKind::Normalize, "source_window"));
        }
        let month = checked_day.month().as_str().to_owned();
        input.row.insert(
            "import_id".to_owned(),
            Value::String(bundle.as_str().to_owned()),
        );
        input
            .row
            .insert("month".to_owned(), Value::String(month.clone()));
        grouped.entry(month).or_default().push(input);
    }

    let mut shards = Vec::new();
    let mut days = BTreeSet::new();
    for (month_text, month_inputs) in grouped {
        let month = BodyMonth::from_bytes(month_text.as_bytes())
            .map_err(|_| error(BodyIngestErrorKind::Normalize, "month"))?;
        let mut rows = Vec::with_capacity(month_inputs.len());
        let mut shard_bytes = Vec::new();
        for (offset, mut input) in month_inputs.into_iter().enumerate() {
            let line = u64::try_from(offset + 1)
                .map_err(|_| error(BodyIngestErrorKind::Normalize, "line"))?;
            input.row.insert(
                "normalized_ref".to_owned(),
                Value::String(format!(
                    "imports/{}/normalized/{}.jsonl#L{line}",
                    bundle.as_str(),
                    month.as_str()
                )),
            );
            if let Some(locator) = input.raw_locator {
                input.row.insert(
                    "raw_ref".to_owned(),
                    Value::String(format!("imports/{}/raw/{locator}", bundle.as_str())),
                );
            }
            input.row.insert(
                "dedupe_key".to_owned(),
                Value::String(PLACEHOLDER_DEDUPE.to_owned()),
            );
            let draft = decode_json_object(&input.row)?;
            let candidate = candidate(&draft, &bundle, month.as_str(), line)?;
            let identity_metadata = match input.identity_metadata {
                Some(metadata) => FieldState::Present(decode_json_map(&metadata)?),
                None => candidate.metadata().clone(),
            };
            let identity = HealthRecordIdentity {
                source_family: candidate.source_family().clone(),
                record_type: candidate.record_type().clone(),
                start_time: candidate.start_date().clone(),
                end_time: candidate.end_date().clone(),
                source_record_id: candidate.source_record_id().clone(),
                source_name: candidate.source_name().clone(),
                unit: candidate.unit().clone(),
                metadata: identity_metadata,
                value: candidate.value().clone(),
            };
            let dedupe = health_record_dedupe_key(&identity)
                .map_err(|_| error(BodyIngestErrorKind::Normalize, "dedupe_key"))?;
            let value_hash = health_value_hash(&identity.unit, &identity.metadata, &identity.value)
                .map_err(|_| error(BodyIngestErrorKind::Normalize, "value_hash"))?;
            input
                .row
                .insert("dedupe_key".to_owned(), Value::String(dedupe));
            let value = decode_json_object(&input.row)?;
            let mut bytes = canonicalize(&value)
                .map_err(|_| error(BodyIngestErrorKind::Normalize, "row_canonical"))?
                .into_bytes();
            bytes.push(b'\n');
            if bytes.len() > MAX_BODY_ROW_FRAME_BYTES {
                return Err(error(BodyIngestErrorKind::Source, "row_frame_limit"));
            }
            if shard_bytes
                .len()
                .checked_add(bytes.len())
                .is_none_or(|size| size > MAX_NORMALIZED_BYTES)
                || shards
                    .iter()
                    .try_fold(shard_bytes.len(), |total, shard: &PreparedShard| {
                        total.checked_add(shard.bytes.len())
                    })
                    .and_then(|total| total.checked_add(bytes.len()))
                    .is_none_or(|size| size > MAX_NORMALIZED_BYTES)
            {
                return Err(error(BodyIngestErrorKind::Source, "normalized_bytes_limit"));
            }
            shard_bytes.extend_from_slice(&bytes);
            days.insert(string_field(&input.row, "day")?.to_owned());
            rows.push(PreparedRow {
                value,
                bytes,
                value_hash: digest_from_text(&value_hash)?,
            });
        }
        shards.push(PreparedShard {
            month,
            bytes: shard_bytes,
            rows,
        });
    }

    let checked_days = days
        .iter()
        .map(|day| BodyDay::from_bytes(day.as_bytes()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| error(BodyIngestErrorKind::Normalize, "days"))?;
    let row_count = shards
        .iter()
        .try_fold(0_u64, |total, shard| {
            total.checked_add(u64::try_from(shard.rows.len()).ok()?)
        })
        .ok_or_else(|| error(BodyIngestErrorKind::Normalize, "row_count"))?;
    let descriptors = shard_descriptors(&bundle, &shards)?;
    let summary = if family == BodySourceFamily::AppleHealth {
        Some(
            AppleSummaryPlan::new(&bundle, checked_days.clone())
                .map_err(|_| error(BodyIngestErrorKind::Normalize, "summary_plan"))?,
        )
    } else {
        None
    };
    let provisional_ledger = if row_count == 0 {
        EnvelopeLedger::new(&bundle, 0, 0, digest_from_text(EMPTY_DIGEST)?)
    } else {
        EnvelopeLedger::new(
            &bundle,
            row_count,
            row_count,
            digest_from_text(PLACEHOLDER_DIGEST)?,
        )
    }
    .map_err(|_| error(BodyIngestErrorKind::Normalize, "ledger"))?;
    let provisional = BodyEnvelope::new(
        bundle.clone(),
        family,
        source_hash.clone(),
        retention,
        row_count,
        checked_days.clone(),
        descriptors.clone(),
        provisional_ledger,
        summary.clone(),
    )
    .map_err(|_| error(BodyIngestErrorKind::Normalize, "envelope"))?;
    let ledger_bytes = encode_ledger(&provisional, &bundle, &shards)?;
    let ledger = EnvelopeLedger::new(
        &bundle,
        u64::try_from(ledger_bytes.len())
            .map_err(|_| error(BodyIngestErrorKind::Normalize, "ledger_bytes"))?,
        row_count,
        digest_bytes(&ledger_bytes)?,
    )
    .map_err(|_| error(BodyIngestErrorKind::Normalize, "ledger"))?;
    let envelope = BodyEnvelope::new(
        bundle.clone(),
        family,
        source_hash.clone(),
        retention,
        row_count,
        checked_days.clone(),
        descriptors,
        ledger,
        summary,
    )
    .map_err(|_| error(BodyIngestErrorKind::Normalize, "envelope"))?;
    if encode_ledger(&envelope, &bundle, &shards)? != ledger_bytes {
        return Err(error(BodyIngestErrorKind::Normalize, "ledger_stability"));
    }
    let envelope_bytes = encode_body_envelope(&envelope)
        .map_err(|_| error(BodyIngestErrorKind::Normalize, "envelope_encode"))?;
    let binding = BodyManifestBinding::new(
        digest_bytes(&envelope_bytes)?,
        bundle.clone(),
        family,
        source_hash,
        row_count,
        checked_days,
        retention,
    )
    .map_err(|_| error(BodyIngestErrorKind::Normalize, "manifest"))?;
    let mut manifest_bytes = canonicalize(&BodyValue::Object(binding.to_body_object()))
        .map_err(|_| error(BodyIngestErrorKind::Normalize, "manifest_encode"))?
        .into_bytes();
    manifest_bytes.push(b'\n');

    let destination = imports.join(bundle.as_str());
    publish_staged_dir(
        &destination,
        StagedDirOptions {
            directory_mode: Some(0o700),
        },
        |staging| {
            populate(
                staging,
                &envelope_bytes,
                &ledger_bytes,
                &manifest_bytes,
                &shards,
                raw_inventory.as_ref(),
            )
        },
    )
    .map_err(|_| error(BodyIngestErrorKind::Publication, "bundle"))?;
    rebuild_body_store(journal).map_err(|_| error(BodyIngestErrorKind::Rebuild, "dedupe_store"))?;
    Ok(BodyIngestReport {
        bundle_id: Some(bundle.as_str().to_owned()),
        rows: row_count,
        days: days.into_iter().collect(),
        skipped: false,
    })
}

fn clean_stale_bundle_staging(journal: &Path, imports: &Path) -> Result<(), BodyIngestError> {
    let mut scanned = 0_usize;
    let mut removed = 0_usize;
    for entry in fs::read_dir(imports)
        .map_err(|_| error(BodyIngestErrorKind::Publication, "staging_cleanup"))?
    {
        scanned = scanned
            .checked_add(1)
            .filter(|count| *count <= MAX_IMPORT_ENTRIES_SCANNED)
            .ok_or_else(|| error(BodyIngestErrorKind::Publication, "imports_entry_limit"))?;
        let entry =
            entry.map_err(|_| error(BodyIngestErrorKind::Publication, "staging_cleanup"))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_bundle_staging_name(name) {
            continue;
        }
        if !entry
            .file_type()
            .map_err(|_| error(BodyIngestErrorKind::Publication, "staging_cleanup"))?
            .is_dir()
        {
            return Err(error(BodyIngestErrorKind::Publication, "staging_cleanup"));
        }
        removed = removed
            .checked_add(1)
            .filter(|count| *count <= MAX_STALE_STAGING_DIRECTORIES)
            .ok_or_else(|| error(BodyIngestErrorKind::Publication, "staging_entry_limit"))?;
        remove_dir_all(journal, &format!("imports/{name}"))
            .map_err(|_| error(BodyIngestErrorKind::Publication, "staging_cleanup"))?;
    }
    Ok(())
}

fn is_bundle_staging_name(name: &str) -> bool {
    let Some((destination, suffix)) = name
        .strip_prefix('.')
        .and_then(|name| name.split_once(".staging."))
    else {
        return false;
    };
    if BundleId::from_bytes(destination.as_bytes()).is_err() {
        return false;
    }
    let Some(process_and_time) = suffix.strip_suffix(".tmp") else {
        return false;
    };
    process_and_time
        .split_once('_')
        .is_some_and(|(process, time)| {
            !process.is_empty()
                && !time.is_empty()
                && process.bytes().all(|byte| byte.is_ascii_digit())
                && time.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn find_existing(
    imports: &Path,
    family: BodySourceFamily,
    source_hash: &BodySourceHash,
    retention: BodyRawRetention,
    raw_inventory_sha256: Option<&str>,
) -> Result<Option<BodyIngestReport>, BodyIngestError> {
    let entries = fs::read_dir(imports)
        .map_err(|_| error(BodyIngestErrorKind::Publication, "imports_scan"))?;
    for entry in entries {
        let entry = entry.map_err(|_| error(BodyIngestErrorKind::Publication, "imports_scan"))?;
        let file_type = entry
            .file_type()
            .map_err(|_| error(BodyIngestErrorKind::Publication, "imports_scan"))?;
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.as_encoded_bytes();
        if !name.starts_with(b"body-") {
            continue;
        }
        let path = entry.path();
        let manifest = read_bounded_regular(&path, "manifest.json", MAX_EXISTING_MANIFEST_BYTES)
            .map_err(|_| error(BodyIngestErrorKind::Publication, "existing_manifest"))?;
        let authority = authorize_native_bundle(DirectoryObservation {
            name,
            envelope_present: regular_file(&path.join("body-bundle.json"))?,
            ledger_present: regular_file(&path.join("body-ledger.jsonl"))?,
            manifest: Some(&manifest),
        })
        .map_err(|_| error(BodyIngestErrorKind::Publication, "existing_bundle"))?;
        let binding = authority.binding();
        if binding.source_type() != family
            || binding.source_hash() != source_hash
            || binding.raw_retention() != retention
        {
            continue;
        }
        if let Some(expected) = raw_inventory_sha256 {
            let (_, actual) = hash_file(
                &path.join(RAW_INVENTORY_NAME),
                MAX_RAW_INVENTORY_BYTES as u64,
            )?;
            if actual != expected {
                continue;
            }
        }
        return Ok(Some(BodyIngestReport {
            bundle_id: Some(authority.id().as_str().to_owned()),
            rows: binding.entry_count(),
            days: binding
                .days_affected()
                .iter()
                .map(|day| day.as_str().to_owned())
                .collect(),
            skipped: true,
        }));
    }
    Ok(None)
}

fn regular_file(path: &Path) -> Result<bool, BodyIngestError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_file()),
        Err(io_error) if io_error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(error(BodyIngestErrorKind::Publication, "existing_bundle")),
    }
}

fn populate(
    staging: &Path,
    envelope: &[u8],
    ledger: &[u8],
    manifest: &[u8],
    shards: &[PreparedShard],
    raw_inventory: Option<&PreparedRawInventory>,
) -> Result<(), BodyIngestError> {
    let options = AtomicWriteOptions { mode: Some(0o600) };
    for (name, bytes, stage) in [
        ("body-bundle.json", envelope, "envelope"),
        ("body-ledger.jsonl", ledger, "ledger"),
        ("manifest.json", manifest, "manifest"),
    ] {
        write_bytes_exclusive(staging.join(name), bytes, options)
            .map_err(|_| error(BodyIngestErrorKind::Publication, stage))?;
    }
    if !shards.is_empty() {
        create_directory_with_mode(&staging.join("normalized"), 0o700)
            .map_err(|_| error(BodyIngestErrorKind::Publication, "normalized_directory"))?;
        for shard in shards {
            write_bytes_exclusive(
                staging
                    .join("normalized")
                    .join(format!("{}.jsonl", shard.month.as_str())),
                &shard.bytes,
                options,
            )
            .map_err(|_| error(BodyIngestErrorKind::Publication, "normalized_shard"))?;
        }
    }
    let Some(raw_inventory) = raw_inventory else {
        return Ok(());
    };
    write_bytes_exclusive(
        staging.join(RAW_INVENTORY_NAME),
        &raw_inventory.bytes,
        options,
    )
    .map_err(|_| error(BodyIngestErrorKind::Publication, "raw_inventory"))?;
    for prepared in &raw_inventory.assets {
        let destination = match &prepared.asset {
            RawAsset::File { source, relative } => {
                let file = open_regular_file(source)
                    .map_err(|_| error(BodyIngestErrorKind::Source, "raw_source"))?;
                let mut reader = BufReader::new(file);
                write_raw(staging, relative, &mut reader, options)?
            }
            RawAsset::ZipMember {
                archive,
                member,
                relative,
            } => {
                let file = open_regular_file(archive)
                    .map_err(|_| error(BodyIngestErrorKind::Source, "raw_archive"))?;
                let mut archive = zip::ZipArchive::new(file)
                    .map_err(|_| error(BodyIngestErrorKind::Source, "raw_archive"))?;
                let mut reader = archive
                    .by_name(member)
                    .map_err(|_| error(BodyIngestErrorKind::Source, "raw_member"))?;
                write_raw(staging, relative, &mut reader, options)?
            }
            RawAsset::Bytes { bytes, relative } => {
                write_raw(staging, relative, &mut bytes.as_slice(), options)?
            }
        };
        let (bytes, sha256) = hash_file(&destination, MAX_RAW_ASSET_BYTES)?;
        if bytes != prepared.bytes || sha256 != prepared.sha256 {
            return Err(error(BodyIngestErrorKind::Publication, "raw_verification"));
        }
    }
    Ok(())
}

fn write_raw(
    staging: &Path,
    relative: &str,
    reader: &mut impl Read,
    options: AtomicWriteOptions,
) -> Result<PathBuf, BodyIngestError> {
    let mut destination = staging.join("raw");
    let mut components = 0_usize;
    for component in Path::new(relative).components() {
        let Component::Normal(component) = component else {
            return Err(error(BodyIngestErrorKind::Source, "raw_path"));
        };
        let component = component
            .to_str()
            .filter(|value| !value.is_empty() && !value.contains(['/', '\\']))
            .ok_or_else(|| error(BodyIngestErrorKind::Source, "raw_path"))?;
        destination.push(component);
        components += 1;
    }
    if components == 0 {
        return Err(error(BodyIngestErrorKind::Source, "raw_path"));
    }
    if let Some(parent) = destination.parent() {
        create_directory_with_mode(parent, 0o700)
            .map_err(|_| error(BodyIngestErrorKind::Publication, "raw_directory"))?;
    }
    write_reader_exclusive(&destination, reader, options)
        .map_err(|_| error(BodyIngestErrorKind::Publication, "raw_asset"))?;
    Ok(destination)
}

fn prepare_raw_inventory(
    assets: Vec<RawAsset>,
) -> Result<Option<PreparedRawInventory>, BodyIngestError> {
    if assets.is_empty() {
        return Ok(None);
    }
    if assets.len() > MAX_RAW_ASSETS {
        return Err(error(BodyIngestErrorKind::Source, "raw_asset_limit"));
    }
    let mut prepared = Vec::with_capacity(assets.len());
    let mut total = 0_u64;
    let mut previous: Option<String> = None;
    for asset in assets {
        let relative = raw_asset_relative(&asset).to_owned();
        validate_raw_relative(&relative)?;
        if previous.as_ref().is_some_and(|value| value >= &relative) {
            return Err(error(BodyIngestErrorKind::Source, "raw_inventory_order"));
        }
        previous = Some(relative.clone());
        let (bytes, sha256) = hash_raw_asset(&asset)?;
        total = total
            .checked_add(bytes)
            .filter(|total| *total <= MAX_RAW_TOTAL_BYTES)
            .ok_or_else(|| error(BodyIngestErrorKind::Source, "raw_bytes_limit"))?;
        prepared.push(PreparedRawAsset {
            asset,
            relative,
            bytes,
            sha256,
        });
    }
    let mut inventory_bytes = Vec::new();
    for asset in &prepared {
        let raw = serde_json::to_vec(&serde_json::json!({
            "bytes": asset.bytes,
            "path": asset.relative,
            "sha256": asset.sha256,
        }))
        .map_err(|_| error(BodyIngestErrorKind::Normalize, "raw_inventory"))?;
        let value =
            parse(&raw).map_err(|_| error(BodyIngestErrorKind::Normalize, "raw_inventory"))?;
        let line = canonicalize(&value)
            .map_err(|_| error(BodyIngestErrorKind::Normalize, "raw_inventory"))?;
        if inventory_bytes
            .len()
            .checked_add(line.len() + 1)
            .is_none_or(|size| size > MAX_RAW_INVENTORY_BYTES)
        {
            return Err(error(BodyIngestErrorKind::Source, "raw_inventory_limit"));
        }
        inventory_bytes.extend_from_slice(line.as_bytes());
        inventory_bytes.push(b'\n');
    }
    let sha256 = format!("sha256:{:x}", Sha256::digest(&inventory_bytes));
    Ok(Some(PreparedRawInventory {
        assets: prepared,
        bytes: inventory_bytes,
        sha256,
    }))
}

fn raw_asset_relative(asset: &RawAsset) -> &str {
    match asset {
        RawAsset::File { relative, .. }
        | RawAsset::ZipMember { relative, .. }
        | RawAsset::Bytes { relative, .. } => relative,
    }
}

fn validate_raw_relative(relative: &str) -> Result<(), BodyIngestError> {
    if relative.len() > MAX_RAW_PATH_BYTES || relative.is_empty() || relative.contains('\\') {
        return Err(error(BodyIngestErrorKind::Source, "raw_path"));
    }
    if Path::new(relative)
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(error(BodyIngestErrorKind::Source, "raw_path"));
    }
    Ok(())
}

fn hash_raw_asset(asset: &RawAsset) -> Result<(u64, String), BodyIngestError> {
    match asset {
        RawAsset::File { source, .. } => hash_file(source, MAX_RAW_ASSET_BYTES),
        RawAsset::ZipMember {
            archive, member, ..
        } => {
            let file = open_regular_file(archive)
                .map_err(|_| error(BodyIngestErrorKind::Source, "raw_archive"))?;
            let mut archive = zip::ZipArchive::new(file)
                .map_err(|_| error(BodyIngestErrorKind::Source, "raw_archive"))?;
            let mut reader = archive
                .by_name(member)
                .map_err(|_| error(BodyIngestErrorKind::Source, "raw_member"))?;
            hash_reader(&mut reader, MAX_RAW_ASSET_BYTES)
        }
        RawAsset::Bytes { bytes, .. } => {
            let size = u64::try_from(bytes.len())
                .map_err(|_| error(BodyIngestErrorKind::Source, "raw_asset_size"))?;
            if size > MAX_RAW_ASSET_BYTES {
                return Err(error(BodyIngestErrorKind::Source, "raw_asset_size"));
            }
            Ok((size, format!("sha256:{:x}", Sha256::digest(bytes))))
        }
    }
}

fn hash_file(path: &Path, limit: u64) -> Result<(u64, String), BodyIngestError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| error(BodyIngestErrorKind::Source, "raw_source"))?;
    if !metadata.file_type().is_file() || metadata.len() > limit {
        return Err(error(BodyIngestErrorKind::Source, "raw_asset_size"));
    }
    let mut file =
        open_regular_file(path).map_err(|_| error(BodyIngestErrorKind::Source, "raw_source"))?;
    hash_reader(&mut file, limit)
}

fn hash_reader(reader: &mut impl Read, limit: u64) -> Result<(u64, String), BodyIngestError> {
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| error(BodyIngestErrorKind::Source, "raw_read"))?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .filter(|total| *total <= limit)
            .ok_or_else(|| error(BodyIngestErrorKind::Source, "raw_asset_size"))?;
        digest.update(&buffer[..read]);
    }
    Ok((total, format!("sha256:{:x}", digest.finalize())))
}

fn shard_descriptors(
    bundle: &BundleId,
    shards: &[PreparedShard],
) -> Result<Vec<EnvelopeShard>, BodyIngestError> {
    shards
        .iter()
        .enumerate()
        .map(|(index, shard)| {
            EnvelopeShard::new(
                bundle,
                u64::try_from(index)
                    .map_err(|_| error(BodyIngestErrorKind::Normalize, "shard_index"))?,
                shard.month.clone(),
                u64::try_from(shard.bytes.len())
                    .map_err(|_| error(BodyIngestErrorKind::Normalize, "shard_bytes"))?,
                u64::try_from(shard.rows.len())
                    .map_err(|_| error(BodyIngestErrorKind::Normalize, "shard_rows"))?,
                digest_bytes(&shard.bytes)?,
            )
            .map_err(|_| error(BodyIngestErrorKind::Normalize, "shard"))
        })
        .collect()
}

fn encode_ledger(
    envelope: &BodyEnvelope,
    bundle: &BundleId,
    shards: &[PreparedShard],
) -> Result<Vec<u8>, BodyIngestError> {
    let mut ledger = Vec::new();
    let mut sequence = 0_u64;
    for (shard_index, shard) in shards.iter().enumerate() {
        for (offset, row) in shard.rows.iter().enumerate() {
            sequence = sequence
                .checked_add(1)
                .ok_or_else(|| error(BodyIngestErrorKind::Normalize, "sequence"))?;
            let line = u64::try_from(offset + 1)
                .map_err(|_| error(BodyIngestErrorKind::Normalize, "line"))?;
            let candidate = candidate(&row.value, bundle, shard.month.as_str(), line)?;
            let event = BodyLedgerEvent::new(
                envelope,
                sequence,
                u64::try_from(shard_index)
                    .map_err(|_| error(BodyIngestErrorKind::Normalize, "shard_index"))?,
                line,
                digest_bytes(&row.bytes)?,
                row.value_hash.clone(),
                &candidate,
            )
            .map_err(|_| error(BodyIngestErrorKind::Normalize, "ledger_event"))?;
            ledger.extend_from_slice(
                &encode_body_ledger_event(&event)
                    .map_err(|_| error(BodyIngestErrorKind::Normalize, "ledger_event_encode"))?,
            );
        }
    }
    Ok(ledger)
}

fn candidate(
    value: &BodyValue,
    bundle: &BundleId,
    month: &str,
    line: u64,
) -> Result<LedgerCandidate, BodyIngestError> {
    let coordinate = Coordinate::new(bundle.as_str(), format!("normalized/{month}.jsonl"), line);
    let row = PresentationRow::new(value, &coordinate)
        .map_err(|_| error(BodyIngestErrorKind::Normalize, "row"))?;
    project(&row, coordinate).map_err(|_| error(BodyIngestErrorKind::Normalize, "projection"))
}

fn decode_json_object(object: &Map<String, Value>) -> Result<BodyValue, BodyIngestError> {
    let bytes =
        serde_json::to_vec(object).map_err(|_| error(BodyIngestErrorKind::Normalize, "json"))?;
    let value = parse(&bytes).map_err(|_| error(BodyIngestErrorKind::Normalize, "json"))?;
    if !matches!(value, BodyValue::Object(_)) {
        return Err(error(BodyIngestErrorKind::Normalize, "json_object"));
    }
    Ok(value)
}

fn decode_json_map(
    object: &Map<String, Value>,
) -> Result<BTreeMap<BodyString, BodyValue>, BodyIngestError> {
    match decode_json_object(object)? {
        BodyValue::Object(object) => Ok(object),
        _ => unreachable!("decode_json_object returns an object"),
    }
}

fn string_field<'a>(
    object: &'a Map<String, Value>,
    name: &str,
) -> Result<&'a str, BodyIngestError> {
    object
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| error(BodyIngestErrorKind::Normalize, "required_field"))
}

pub(crate) fn sha256_hex(reader: &mut (impl Read + ?Sized)) -> Result<String, BodyIngestError> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|_| error(BodyIngestErrorKind::Source, "read"))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn digest_bytes(bytes: &[u8]) -> Result<BodyDigest, BodyIngestError> {
    digest_from_text(&format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn digest_from_text(value: &str) -> Result<BodyDigest, BodyIngestError> {
    BodyDigest::from_bytes(value.as_bytes())
        .map_err(|_| error(BodyIngestErrorKind::Normalize, "digest"))
}

fn new_bundle_id(imports: &Path) -> Result<BundleId, BodyIngestError> {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    for _ in 0..100 {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| error(BodyIngestErrorKind::Publication, "clock"))?
            .as_millis();
        let timestamp =
            u64::try_from(millis).map_err(|_| error(BodyIngestErrorKind::Publication, "clock"))?;
        if timestamp >= (1_u64 << 48) {
            return Err(error(BodyIngestErrorKind::Publication, "clock"));
        }
        let mut raw = [0_u8; 16];
        raw[..6].copy_from_slice(&timestamp.to_be_bytes()[2..]);
        random_fill(&mut raw[6..])
            .map_err(|_| error(BodyIngestErrorKind::Publication, "random"))?;
        let mut value = u128::from_be_bytes(raw);
        let mut encoded = [b'0'; 26];
        for slot in encoded.iter_mut().rev() {
            *slot = ALPHABET[(value & 31) as usize];
            value >>= 5;
        }
        let text = format!(
            "body-{}",
            std::str::from_utf8(&encoded).expect("ULID alphabet is ASCII")
        );
        let bundle = BundleId::from_bytes(text.as_bytes())
            .map_err(|_| error(BodyIngestErrorKind::Publication, "bundle_id"))?;
        match fs::symlink_metadata(imports.join(bundle.as_str())) {
            Err(io_error) if io_error.kind() == std::io::ErrorKind::NotFound => return Ok(bundle),
            Ok(_) => {}
            Err(_) => return Err(error(BodyIngestErrorKind::Publication, "bundle_probe")),
        }
    }
    Err(error(BodyIngestErrorKind::Publication, "bundle_collision"))
}

const fn error(kind: BodyIngestErrorKind, stage: &'static str) -> BodyIngestError {
    BodyIngestError::new(kind, stage)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    use nix::sys::stat::Mode;
    use nix::unistd::mkfifo;

    use super::*;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn temporary_imports() -> PathBuf {
        let imports = std::env::temp_dir().join(format!(
            "solstone-body-existing-manifest-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&imports).unwrap();
        imports
    }

    fn find_existing_error(imports: &Path) -> BodyIngestError {
        let family = BodySourceFamily::AppleHealth;
        let hash = BodySourceHash::from_bytes_for_family(
            b"1111111111111111111111111111111111111111111111111111111111111111",
            &family,
        )
        .unwrap();
        find_existing(imports, family, &hash, BodyRawRetention::Discard, None).unwrap_err()
    }

    #[test]
    fn existing_manifest_read_refuses_symlinks_fifos_and_oversized_documents() {
        let imports = temporary_imports();
        let bundle = imports.join("body-00000000000000000000000000");
        fs::create_dir(&bundle).unwrap();
        let manifest = bundle.join("manifest.json");
        let outside = imports.join("outside-manifest.json");
        fs::write(&outside, b"{}").unwrap();
        symlink(&outside, &manifest).unwrap();
        assert_eq!(find_existing_error(&imports).stage(), "existing_manifest");

        fs::remove_file(&manifest).unwrap();
        mkfifo(&manifest, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
        assert_eq!(find_existing_error(&imports).stage(), "existing_manifest");

        fs::remove_file(&manifest).unwrap();
        fs::write(&manifest, vec![b'x'; MAX_EXISTING_MANIFEST_BYTES + 1]).unwrap();
        assert_eq!(find_existing_error(&imports).stage(), "existing_manifest");

        fs::remove_dir_all(imports).unwrap();
    }
}
